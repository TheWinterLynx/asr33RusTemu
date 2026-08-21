use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::time::Duration;

use asr33emu::adapters::transport::{Transport, TransportSendError, TransportSendFailure};
use asr33emu::app::{AppRuntime, PumpStatus, RuntimeEvent, RuntimeState, Scheduler};
use asr33emu::core::events::{
    ApplicationCommand, CommunicationMode, ThrottleMode, TransportCommand, TransportEvent,
    TransportOperation,
};
use asr33emu::core::terminal::TerminalOptions;
use asr33emu::core::throttle::ThrottleConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeTransportError(&'static str);

impl fmt::Display for FakeTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for FakeTransportError {}

#[derive(Debug, Default)]
struct FakeTransport {
    incoming: VecDeque<TransportEvent>,
    sent: Vec<TransportCommand>,
    started: bool,
    blocked: bool,
    shutdown: bool,
    joined: bool,
}

impl FakeTransport {
    fn receive(&mut self, data: &[u8]) {
        self.incoming
            .push_back(TransportEvent::Received(data.to_vec()));
    }

    fn fail(&mut self, operation: TransportOperation, message: &str) {
        self.incoming.push_back(TransportEvent::Failed {
            operation,
            message: message.to_owned(),
        });
    }
}

impl Transport for FakeTransport {
    type Error = FakeTransportError;

    fn start(&mut self) -> Result<(), Self::Error> {
        self.started = true;
        Ok(())
    }

    fn send(&mut self, command: TransportCommand) -> Result<(), TransportSendError> {
        if self.shutdown {
            return Err(TransportSendError {
                failure: TransportSendFailure::Closed,
                command,
                message: "fake transport is closed".to_owned(),
            });
        }
        if self.blocked {
            return Err(TransportSendError {
                failure: TransportSendFailure::Backpressure,
                command,
                message: "fake transport is full".to_owned(),
            });
        }
        self.sent.push(command);
        Ok(())
    }

    fn try_recv(&mut self) -> Result<Option<TransportEvent>, Self::Error> {
        Ok(self.incoming.pop_front())
    }

    fn shutdown(&mut self) -> Result<(), Self::Error> {
        self.shutdown = true;
        Ok(())
    }

    fn join(&mut self) -> Result<(), Self::Error> {
        self.joined = true;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct FakeScheduler {
    now: Duration,
    waits: Vec<Duration>,
}

impl Scheduler for FakeScheduler {
    fn now(&self) -> Duration {
        self.now
    }

    fn wait(&mut self, duration: Duration) {
        self.waits.push(duration);
        self.now += duration;
    }
}

type Runtime = AppRuntime<FakeTransport, FakeScheduler>;

fn runtime(throttled: bool) -> Runtime {
    runtime_with_throttle(
        throttled,
        ThrottleConfig {
            tx_rate_cps: 10,
            rx_rate_cps: 10,
            tx_queue_capacity: 8,
            rx_queue_capacity: 8,
        },
    )
}

fn runtime_with_throttle(throttled: bool, throttle_config: ThrottleConfig) -> Runtime {
    let mut runtime = AppRuntime::new(
        FakeTransport::default(),
        FakeScheduler::default(),
        TerminalOptions {
            columns: 16,
            rows: 4,
            scrollback: 4,
            autowrap: false,
        },
        throttle_config,
    )
    .expect("valid runtime configuration");
    assert_eq!(runtime.state(), RuntimeState::Created);
    runtime.start().expect("fake transport starts");
    if !throttled {
        runtime
            .submit(ApplicationCommand::SetThrottleMode(
                ThrottleMode::Unthrottled,
            ))
            .expect("runtime is running");
    }
    runtime
}

#[test]
fn capacity_one_application_ingress_is_lossless_across_multiple_chunks() {
    let mut runtime = runtime_with_throttle(
        true,
        ThrottleConfig {
            tx_queue_capacity: 1,
            rx_queue_capacity: 1,
            ..ThrottleConfig::default()
        },
    );
    for chunk in [b"A".to_vec(), b"BC".to_vec(), b"D".to_vec()] {
        runtime
            .submit(ApplicationCommand::Transmit(chunk))
            .expect("application ingress owns rejected chunks");
    }
    runtime.pump().expect("all pending TX eventually drains");
    let bytes = runtime
        .transport()
        .sent
        .iter()
        .flat_map(|command| match command {
            TransportCommand::Send(data) => data.iter().copied(),
        })
        .collect::<Vec<_>>();
    assert_eq!(bytes, b"ABCD");
}

#[test]
fn startup_cr_uses_legacy_pre_configuration_line_queue_semantics() {
    for (mode, expected_transport) in [
        (CommunicationMode::Line, b"\r".as_slice()),
        (CommunicationMode::Local, b"".as_slice()),
    ] {
        let mut runtime = AppRuntime::new(
            FakeTransport::default(),
            FakeScheduler::default(),
            TerminalOptions::default(),
            ThrottleConfig {
                tx_rate_cps: 0,
                rx_rate_cps: 0,
                tx_queue_capacity: 1,
                rx_queue_capacity: 1,
            },
        )
        .expect("valid runtime");
        runtime
            .start_with_initial_commands([
                ApplicationCommand::Transmit(b"\r".to_vec()),
                ApplicationCommand::SetCommunicationMode(mode),
            ])
            .expect("startup succeeds");
        runtime.pump().expect("startup queue drains");
        let sent = runtime
            .transport()
            .sent
            .iter()
            .flat_map(|command| match command {
                TransportCommand::Send(data) => data.iter().copied(),
            })
            .collect::<Vec<_>>();
        assert_eq!(sent, expected_transport);
        assert_eq!(runtime.terminal().cursor_position(), (0, 0));
    }
}

#[test]
fn capacity_one_transport_ingress_is_lossless_across_multiple_chunks() {
    let mut runtime = runtime_with_throttle(
        true,
        ThrottleConfig {
            tx_queue_capacity: 1,
            rx_queue_capacity: 1,
            ..ThrottleConfig::default()
        },
    );
    for chunk in [b"A".as_slice(), b"BC".as_slice(), b"D".as_slice()] {
        runtime.transport_mut().receive(chunk);
    }
    assert_eq!(
        runtime.tick().expect("bounded tick succeeds"),
        PumpStatus::Wait(Duration::from_millis(100))
    );
    runtime.pump().expect("all pending RX eventually drains");
    assert_eq!(&line(&runtime, 0)[..4], "ABCD");
}

#[test]
fn tick_bounds_work_under_continuous_unthrottled_rx() {
    let mut runtime = runtime(false);
    for _ in 0..100 {
        runtime.transport_mut().receive(b"X");
    }
    assert_eq!(
        runtime.tick().expect("tick succeeds"),
        PumpStatus::WorkRemaining
    );
    assert!(!runtime.transport().incoming.is_empty());
    runtime.pump().expect("remaining finite input drains");
    assert!(runtime.transport().incoming.is_empty());
}

fn line(runtime: &Runtime, row: usize) -> String {
    runtime
        .terminal()
        .line_history()
        .line(row)
        .expect("line exists")
        .top_characters()
}

#[test]
fn rx_bytes_flow_through_throttle_into_terminal() {
    let mut runtime = runtime(false);
    runtime.transport_mut().receive(b"HELLO");
    assert_eq!(runtime.pump().expect("pipeline succeeds"), PumpStatus::Idle);
    assert_eq!(&line(&runtime, 0)[..5], "HELLO");
    assert_eq!(runtime.terminal().cursor_position(), (5, 0));
}

#[test]
fn rx_cr_lf_and_multiple_chunks_update_terminal_in_order() {
    let mut runtime = runtime(false);
    runtime.transport_mut().receive(b"AB\r");
    runtime.transport_mut().receive(b"\nCD");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(&line(&runtime, 0)[..2], "AB");
    assert_eq!(&line(&runtime, 1)[..2], "CD");
    assert_eq!(runtime.terminal().cursor_position(), (2, 1));
}

#[test]
fn tx_bytes_reach_transport_in_chunk_order_when_unthrottled() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::Transmit(b"AB".to_vec()))
        .expect("TX queued");
    runtime
        .submit(ApplicationCommand::Transmit(b"CD".to_vec()))
        .expect("TX queued");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(
        runtime.transport().sent,
        [
            TransportCommand::Send(b"AB".to_vec()),
            TransportCommand::Send(b"CD".to_vec())
        ]
    );
}

#[test]
fn throttled_tx_uses_scheduler_and_preserves_byte_order() {
    let mut runtime = runtime(true);
    runtime
        .submit(ApplicationCommand::Transmit(b"AB".to_vec()))
        .expect("TX queued");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(
        runtime.transport().sent,
        [
            TransportCommand::Send(vec![b'A']),
            TransportCommand::Send(vec![b'B'])
        ]
    );
    assert_eq!(
        runtime.scheduler().waits,
        [Duration::from_millis(100), Duration::from_millis(100)]
    );
}

#[test]
fn nonblocking_tick_allows_throttle_change_mid_chunk() {
    let mut runtime = runtime(true);
    runtime
        .submit(ApplicationCommand::Transmit(b"AB".to_vec()))
        .expect("TX queued");
    assert_eq!(
        runtime.tick().expect("tick succeeds"),
        PumpStatus::Wait(Duration::from_millis(100))
    );
    runtime.scheduler_mut().wait(Duration::from_millis(100));
    assert_eq!(
        runtime.tick().expect("first byte is emitted"),
        PumpStatus::Wait(Duration::from_millis(100))
    );
    assert_eq!(
        runtime.transport().sent,
        [TransportCommand::Send(vec![b'A'])]
    );
    runtime
        .submit(ApplicationCommand::SetThrottleMode(
            ThrottleMode::Unthrottled,
        ))
        .expect("mode changes between ticks");
    assert_eq!(
        runtime.tick().expect("remainder is emitted"),
        PumpStatus::Idle
    );
    assert_eq!(
        runtime.transport().sent,
        [
            TransportCommand::Send(vec![b'A']),
            TransportCommand::Send(vec![b'B'])
        ]
    );
}

#[test]
fn local_loopback_never_sends_to_transport_and_line_mode_resumes_tx() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("mode changes");
    runtime
        .submit(ApplicationCommand::Transmit(b"LOCAL".to_vec()))
        .expect("loopback queued");
    runtime.pump().expect("pipeline succeeds");
    assert!(runtime.transport().sent.is_empty());
    assert_eq!(&line(&runtime, 0)[..5], "LOCAL");

    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Line,
        ))
        .expect("mode changes");
    runtime
        .submit(ApplicationCommand::Transmit(b"LINE".to_vec()))
        .expect("TX queued");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(
        runtime.transport().sent,
        [TransportCommand::Send(b"LINE".to_vec())]
    );
}

#[test]
fn remote_rx_is_ignored_in_local_mode() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("mode changes");
    runtime.transport_mut().receive(b"REMOTE");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(line(&runtime, 0), "                ");
}

#[test]
fn printer_state_controls_terminal_without_affecting_transport_boundaries() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetPrinterEnabled(false))
        .expect("printer disables");
    runtime.transport_mut().receive(b"HIDDEN");
    runtime.pump().expect("pipeline succeeds");
    assert!(!runtime.terminal().printing_enabled());
    assert_eq!(line(&runtime, 0), "                ");

    runtime
        .submit(ApplicationCommand::SetPrinterEnabled(true))
        .expect("printer enables");
    runtime.transport_mut().receive(b"VISIBLE");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(&line(&runtime, 0)[..7], "VISIBLE");
}

#[test]
fn transport_backpressure_retains_pending_command_for_retry() {
    let mut runtime = runtime(false);
    runtime.transport_mut().blocked = true;
    runtime
        .submit(ApplicationCommand::Transmit(b"AB".to_vec()))
        .expect("TX queued");
    assert_eq!(
        runtime.pump().expect("backpressure is observable"),
        PumpStatus::Backpressured
    );
    assert!(runtime.transport().sent.is_empty());

    runtime.transport_mut().blocked = false;
    assert_eq!(runtime.pump().expect("retry succeeds"), PumpStatus::Idle);
    assert_eq!(
        runtime.transport().sent,
        [TransportCommand::Send(b"AB".to_vec())]
    );
}

#[test]
fn transport_failure_becomes_application_event_and_failed_state() {
    let mut runtime = runtime(false);
    runtime
        .transport_mut()
        .fail(TransportOperation::Read, "device removed");
    assert_eq!(
        runtime.pump().expect("failure is an event"),
        PumpStatus::TransportFailed
    );
    assert_eq!(runtime.state(), RuntimeState::Failed);
    assert_eq!(
        runtime.pop_event(),
        Some(RuntimeEvent::TransportFailed {
            operation: TransportOperation::Read,
            message: "device removed".to_owned()
        })
    );
    runtime
        .join()
        .expect("failed transport shuts down and joins");
    assert_eq!(runtime.state(), RuntimeState::Joined);
    assert!(runtime.transport().shutdown);
    assert!(runtime.transport().joined);
}

#[test]
fn shutdown_and_join_are_idempotent_and_reject_later_bytes() {
    let mut runtime = runtime(false);
    runtime.shutdown().expect("shutdown succeeds");
    runtime.shutdown().expect("shutdown is idempotent");
    assert!(runtime.transport().shutdown);
    assert!(
        runtime
            .submit(ApplicationCommand::Transmit(vec![1]))
            .is_err()
    );
    runtime.transport_mut().receive(b"AFTER");
    assert!(runtime.pump().is_err());
    assert!(runtime.transport().sent.is_empty());
    assert_eq!(line(&runtime, 0), "                ");

    runtime.join().expect("join succeeds");
    runtime.join().expect("join is idempotent");
    assert_eq!(runtime.state(), RuntimeState::Joined);
    assert!(runtime.transport().joined);
}
