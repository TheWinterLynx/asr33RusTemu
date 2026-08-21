//! Deterministic data-routing and byte-pacing policy.
//!
//! This module decides *whether* a scheduler should wait, but never sleeps and
//! never reads a clock. Queue capacities represent chunk backpressure just as
//! the Python implementation's bounded queues do.

use std::collections::VecDeque;
use std::time::Duration;

use super::events::{
    ApplicationCommand, CommunicationMode, DataFlow, ThrottleMode, ThrottleOutput,
    TransportCommand, TransportEvent,
};

const LOOPBACK_QUEUE_CAPACITY: usize = 8;
const PERCEPTION_THRESHOLD: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrottleConfig {
    pub tx_rate_cps: i64,
    pub rx_rate_cps: i64,
    pub tx_queue_capacity: usize,
    pub rx_queue_capacity: usize,
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            tx_rate_cps: 10,
            rx_rate_cps: 10,
            tx_queue_capacity: 8,
            rx_queue_capacity: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Queued,
    Applied,
    IgnoredEmpty,
    IgnoredDuringLoopback,
    IgnoredTransportFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backpressure {
    pub flow: DataFlow,
    pub capacity_chunks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedData {
    pub backpressure: Backpressure,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThrottleStep {
    Idle,
    Wait {
        flow: DataFlow,
        duration: Duration,
    },
    Output {
        flow: DataFlow,
        output: ThrottleOutput,
    },
    Dropped {
        flow: DataFlow,
        data: Vec<u8>,
    },
}

#[derive(Debug)]
struct FlowState {
    queue: VecDeque<Vec<u8>>,
    capacity: usize,
    active: Option<ActiveChunk>,
    last_event: Duration,
}

impl FlowState {
    fn new(capacity: usize, initial_time: Duration) -> Self {
        Self {
            queue: VecDeque::with_capacity(capacity),
            capacity,
            active: None,
            last_event: initial_time,
        }
    }

    fn enqueue(&mut self, flow: DataFlow, data: Vec<u8>) -> Result<(), RejectedData> {
        // `queue.Queue(maxsize=0)` is unbounded in Python.
        if self.capacity > 0 && self.queue.len() >= self.capacity {
            return Err(RejectedData {
                backpressure: Backpressure {
                    flow,
                    capacity_chunks: self.capacity,
                },
                data,
            });
        }
        self.queue.push_back(data);
        Ok(())
    }

    fn ensure_active(&mut self, rate_cps: i64) -> bool {
        if self.active.is_none() {
            self.active = self
                .queue
                .pop_front()
                .map(|bytes| ActiveChunk::new(bytes, rate_cps));
        }
        self.active.is_some()
    }

    fn clear(&mut self) {
        self.queue.clear();
        self.active = None;
    }

    fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.active.is_none()
    }
}

#[derive(Debug)]
struct ActiveChunk {
    bytes: Vec<u8>,
    position: usize,
    rate_cps: i64,
}

impl ActiveChunk {
    fn new(bytes: Vec<u8>, rate_cps: i64) -> Self {
        Self {
            bytes,
            position: 0,
            rate_cps,
        }
    }

    fn take_one(&mut self) -> Vec<u8> {
        let byte = self.bytes[self.position];
        self.position += 1;
        vec![byte]
    }

    fn take_remainder(&mut self) -> Vec<u8> {
        let remainder = self.bytes[self.position..].to_vec();
        self.position = self.bytes.len();
        remainder
    }

    fn complete(&self) -> bool {
        self.position == self.bytes.len()
    }
}

#[derive(Debug)]
pub struct DataThrottle {
    mode: ThrottleMode,
    communication: CommunicationMode,
    tx_rate_cps: i64,
    rx_rate_cps: i64,
    tx: FlowState,
    rx: FlowState,
    loopback: FlowState,
}

impl DataThrottle {
    pub fn new(config: ThrottleConfig, initial_time: Duration) -> Self {
        Self {
            mode: ThrottleMode::Throttled,
            communication: CommunicationMode::Line,
            tx_rate_cps: config.tx_rate_cps,
            rx_rate_cps: config.rx_rate_cps,
            tx: FlowState::new(config.tx_queue_capacity, initial_time),
            rx: FlowState::new(config.rx_queue_capacity, initial_time),
            loopback: FlowState::new(LOOPBACK_QUEUE_CAPACITY, initial_time),
        }
    }

    pub fn handle_application_command(
        &mut self,
        command: ApplicationCommand,
    ) -> Result<EnqueueOutcome, RejectedData> {
        match command {
            ApplicationCommand::Transmit(data) => self.enqueue_tx(data),
            ApplicationCommand::SetCommunicationMode(mode) => {
                self.set_communication_mode(mode);
                Ok(EnqueueOutcome::Applied)
            }
            ApplicationCommand::SetThrottleMode(mode) => {
                self.mode = mode;
                Ok(EnqueueOutcome::Applied)
            }
            ApplicationCommand::SetTxRate(rate) => {
                self.tx_rate_cps = rate;
                Ok(EnqueueOutcome::Applied)
            }
            ApplicationCommand::SetRxRate(rate) => {
                self.rx_rate_cps = rate;
                Ok(EnqueueOutcome::Applied)
            }
            ApplicationCommand::SetPrinterEnabled(_) => Ok(EnqueueOutcome::Applied),
        }
    }

    pub fn handle_transport_event(
        &mut self,
        event: TransportEvent,
    ) -> Result<EnqueueOutcome, RejectedData> {
        match event {
            TransportEvent::Received(data) => self.enqueue_rx(data),
            TransportEvent::Failed { .. } => Ok(EnqueueOutcome::IgnoredTransportFailure),
        }
    }

    pub fn enqueue_tx(&mut self, data: Vec<u8>) -> Result<EnqueueOutcome, RejectedData> {
        if data.is_empty() {
            return Ok(EnqueueOutcome::IgnoredEmpty);
        }
        let (flow, state) = match self.communication {
            CommunicationMode::Line => (DataFlow::Tx, &mut self.tx),
            CommunicationMode::Local => (DataFlow::Loopback, &mut self.loopback),
        };
        state.enqueue(flow, data)?;
        Ok(EnqueueOutcome::Queued)
    }

    pub fn enqueue_rx(&mut self, data: Vec<u8>) -> Result<EnqueueOutcome, RejectedData> {
        if data.is_empty() {
            return Ok(EnqueueOutcome::IgnoredEmpty);
        }
        if self.communication == CommunicationMode::Local {
            return Ok(EnqueueOutcome::IgnoredDuringLoopback);
        }
        self.rx.enqueue(DataFlow::Rx, data)?;
        Ok(EnqueueOutcome::Queued)
    }

    pub fn set_communication_mode(&mut self, mode: CommunicationMode) {
        // Every legacy `enable_loopback()` call clears queued loopback data,
        // even when local mode was already enabled.
        if mode == CommunicationMode::Local {
            self.loopback.queue.clear();
        }
        self.communication = mode;
    }

    #[must_use]
    pub fn communication_mode(&self) -> CommunicationMode {
        self.communication
    }

    /// Discard only bytes destined for an external transport.
    ///
    /// RX, terminal history, and local loopback state are intentionally kept.
    pub fn clear_external_tx(&mut self) {
        self.tx.clear();
    }

    #[must_use]
    pub fn transmit_idle(&self) -> bool {
        self.tx.is_idle() && self.loopback.is_idle()
    }

    pub fn set_throttle_mode(&mut self, mode: ThrottleMode) {
        self.mode = mode;
    }

    pub fn set_tx_rate(&mut self, rate_cps: i64) {
        self.tx_rate_cps = rate_cps;
    }

    pub fn set_rx_rate(&mut self, rate_cps: i64) {
        self.rx_rate_cps = rate_cps;
    }

    pub fn poll_tx(&mut self, now: Duration) -> ThrottleStep {
        let deliver = self.communication == CommunicationMode::Line;
        Self::poll_flow(
            &mut self.tx,
            DataFlow::Tx,
            self.tx_rate_cps,
            self.mode,
            now,
            deliver,
        )
    }

    pub fn poll_rx(&mut self, now: Duration) -> ThrottleStep {
        let deliver = self.communication == CommunicationMode::Line;
        Self::poll_flow(
            &mut self.rx,
            DataFlow::Rx,
            self.rx_rate_cps,
            self.mode,
            now,
            deliver,
        )
    }

    pub fn poll_loopback(&mut self, now: Duration) -> ThrottleStep {
        let deliver = self.communication == CommunicationMode::Local;
        Self::poll_flow(
            &mut self.loopback,
            DataFlow::Loopback,
            self.tx_rate_cps,
            self.mode,
            now,
            deliver,
        )
    }

    fn poll_flow(
        state: &mut FlowState,
        flow: DataFlow,
        rate_cps: i64,
        mode: ThrottleMode,
        now: Duration,
        deliver: bool,
    ) -> ThrottleStep {
        if !state.ensure_active(rate_cps) {
            return ThrottleStep::Idle;
        }

        // Python passes the rate into a whole-chunk processing call, so a rate
        // change only applies to the next chunk. Throttle mode, by contrast,
        // is intentionally observed between bytes.
        let active_rate = state
            .active
            .as_ref()
            .map_or(rate_cps, |active| active.rate_cps);
        // Legacy behavior: zero and negative rates bypass serialization.
        let unthrottled = mode == ThrottleMode::Unthrottled || active_rate <= 0;
        if !unthrottled {
            let interval = Duration::from_secs(1).div_f64(active_rate as f64);
            let elapsed = now.saturating_sub(state.last_event);
            let remaining = interval.saturating_sub(elapsed);
            // Legacy perception threshold: waits of exactly 20 ms (or less)
            // are skipped instead of accumulated.
            if remaining > PERCEPTION_THRESHOLD {
                return ThrottleStep::Wait {
                    flow,
                    duration: remaining,
                };
            }
        }

        let active = match state.active.as_mut() {
            Some(active) => active,
            None => return ThrottleStep::Idle,
        };
        let data = if unthrottled {
            active.take_remainder()
        } else {
            active.take_one()
        };
        if active.complete() {
            state.active = None;
        }
        state.last_event = now;

        if !deliver {
            return ThrottleStep::Dropped { flow, data };
        }
        let output = match flow {
            DataFlow::Tx => ThrottleOutput::ToTransport(TransportCommand::Send(data)),
            DataFlow::Rx | DataFlow::Loopback => ThrottleOutput::ToApplication(data),
        };
        ThrottleStep::Output { flow, output }
    }
}
