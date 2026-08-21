//! Minimal synchronous composition of transport, throttle, and terminal.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use crate::adapters::transport::{Transport, TransportSendError, TransportSendFailure};
use crate::core::events::{
    ApplicationCommand, DataFlow, ThrottleOutput, TransportCommand, TransportEvent,
    TransportOperation,
};
use crate::core::terminal::{Terminal, TerminalError, TerminalOptions};
use crate::core::throttle::{DataThrottle, ThrottleConfig, ThrottleStep};

const MAX_TICK_ROUNDS: usize = 32;

pub trait Scheduler {
    fn now(&self) -> Duration;
    fn wait(&mut self, duration: Duration);
}

#[derive(Debug)]
pub struct SystemScheduler {
    origin: Instant,
}

impl SystemScheduler {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for SystemScheduler {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn wait(&mut self, duration: Duration) {
        thread::sleep(duration);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Created,
    Running,
    Failed,
    Shutdown,
    Joined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    TransportFailed {
        operation: TransportOperation,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpStatus {
    Idle,
    Backpressured,
    TransportFailed,
    Wait(Duration),
    WorkRemaining,
}

enum FlushStatus {
    Empty,
    Sent,
    Backpressured,
}

#[derive(Debug)]
pub enum RuntimeError<E> {
    InvalidState(RuntimeState),
    Transport(E),
    TransportSend {
        failure: TransportSendFailure,
        message: String,
    },
    Terminal(TerminalError),
}

impl<E> fmt::Display for RuntimeError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidState(state) => write!(formatter, "runtime is not running: {state:?}"),
            Self::Transport(error) => write!(formatter, "transport error: {error}"),
            Self::TransportSend { failure, message } => {
                write!(formatter, "transport send {failure:?}: {message}")
            }
            Self::Terminal(error) => write!(formatter, "terminal error: {error}"),
        }
    }
}

impl<E> Error for RuntimeError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Terminal(error) => Some(error),
            Self::InvalidState(_) | Self::TransportSend { .. } => None,
        }
    }
}

pub struct AppRuntime<T, S>
where
    T: Transport,
    S: Scheduler,
{
    terminal: Terminal,
    throttle: DataThrottle,
    transport: T,
    scheduler: S,
    pending_application: VecDeque<ApplicationCommand>,
    pending_transport_event: Option<TransportEvent>,
    pending_transport: VecDeque<TransportCommand>,
    events: VecDeque<RuntimeEvent>,
    state: RuntimeState,
}

impl<T, S> AppRuntime<T, S>
where
    T: Transport,
    S: Scheduler,
{
    pub fn new(
        transport: T,
        scheduler: S,
        terminal_options: TerminalOptions,
        throttle_config: ThrottleConfig,
    ) -> Result<Self, RuntimeError<T::Error>> {
        let terminal = Terminal::new(terminal_options).map_err(RuntimeError::Terminal)?;
        let initial_time = scheduler.now();
        Ok(Self {
            terminal,
            throttle: DataThrottle::new(throttle_config, initial_time),
            transport,
            scheduler,
            pending_application: VecDeque::new(),
            pending_transport_event: None,
            pending_transport: VecDeque::new(),
            events: VecDeque::new(),
            state: RuntimeState::Created,
        })
    }

    pub fn start(&mut self) -> Result<(), RuntimeError<T::Error>> {
        match self.state {
            RuntimeState::Created => {
                self.transport.start().map_err(RuntimeError::Transport)?;
                self.state = RuntimeState::Running;
                Ok(())
            }
            RuntimeState::Running => Ok(()),
            state => Err(RuntimeError::InvalidState(state)),
        }
    }

    pub fn submit(&mut self, command: ApplicationCommand) -> Result<(), RuntimeError<T::Error>> {
        self.require_running()?;
        self.pending_application.push_back(command);
        Ok(())
    }

    pub fn pump(&mut self) -> Result<PumpStatus, RuntimeError<T::Error>> {
        loop {
            match self.tick()? {
                PumpStatus::Wait(duration) => self.scheduler.wait(duration),
                PumpStatus::WorkRemaining => {}
                status => return Ok(status),
            }
        }
    }

    /// Advance all work that is ready at the scheduler's current time without
    /// blocking. A caller may interleave commands before honoring `Wait`.
    pub fn tick(&mut self) -> Result<PumpStatus, RuntimeError<T::Error>> {
        self.require_running()?;
        for _ in 0..MAX_TICK_ROUNDS {
            let application_progressed = self.drain_one_application();
            let defer_ingress = application_progressed && !self.pending_application.is_empty();
            let mut progressed = application_progressed;
            match self.flush_one_transport()? {
                FlushStatus::Empty => {}
                FlushStatus::Sent => progressed = true,
                FlushStatus::Backpressured => return Ok(PumpStatus::Backpressured),
            }

            if !defer_ingress {
                progressed |= self.receive_one_transport_event()?;
            }
            if self.state == RuntimeState::Failed {
                return Ok(PumpStatus::TransportFailed);
            }

            let now = self.scheduler.now();
            let mut next_wait = None;
            for flow in [DataFlow::Tx, DataFlow::Loopback, DataFlow::Rx] {
                let step = match flow {
                    DataFlow::Tx => self.throttle.poll_tx(now),
                    DataFlow::Rx => self.throttle.poll_rx(now),
                    DataFlow::Loopback => self.throttle.poll_loopback(now),
                };
                match step {
                    ThrottleStep::Idle => {}
                    ThrottleStep::Wait { duration, .. } => {
                        next_wait = Some(
                            next_wait.map_or(duration, |current: Duration| current.min(duration)),
                        );
                    }
                    ThrottleStep::Output { output, .. } => {
                        self.apply_output(output)?;
                        progressed = true;
                    }
                    ThrottleStep::Dropped { .. } => progressed = true,
                }
            }

            if progressed {
                continue;
            }
            if let Some(duration) = next_wait {
                return Ok(PumpStatus::Wait(duration));
            }
            return Ok(PumpStatus::Idle);
        }
        Ok(PumpStatus::WorkRemaining)
    }

    pub fn shutdown(&mut self) -> Result<(), RuntimeError<T::Error>> {
        if matches!(self.state, RuntimeState::Shutdown | RuntimeState::Joined) {
            return Ok(());
        }
        self.transport.shutdown().map_err(RuntimeError::Transport)?;
        self.pending_transport.clear();
        self.pending_application.clear();
        self.pending_transport_event = None;
        self.state = RuntimeState::Shutdown;
        Ok(())
    }

    pub fn join(&mut self) -> Result<(), RuntimeError<T::Error>> {
        if self.state == RuntimeState::Joined {
            return Ok(());
        }
        self.shutdown()?;
        self.transport.join().map_err(RuntimeError::Transport)?;
        self.state = RuntimeState::Joined;
        Ok(())
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn pop_event(&mut self) -> Option<RuntimeEvent> {
        self.events.pop_front()
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }

    pub fn scheduler_mut(&mut self) -> &mut S {
        &mut self.scheduler
    }

    fn require_running(&self) -> Result<(), RuntimeError<T::Error>> {
        if self.state == RuntimeState::Running {
            Ok(())
        } else {
            Err(RuntimeError::InvalidState(self.state))
        }
    }

    fn receive_one_transport_event(&mut self) -> Result<bool, RuntimeError<T::Error>> {
        let event = match self.pending_transport_event.take() {
            Some(event) => event,
            None => {
                let Some(event) = self.transport.try_recv().map_err(RuntimeError::Transport)?
                else {
                    return Ok(false);
                };
                event
            }
        };
        match event {
            TransportEvent::Received(data) => {
                if let Err(rejected) = self.throttle.enqueue_rx(data) {
                    self.pending_transport_event = Some(TransportEvent::Received(rejected.data));
                    return Ok(false);
                }
            }
            TransportEvent::Failed { operation, message } => {
                self.events
                    .push_back(RuntimeEvent::TransportFailed { operation, message });
                self.state = RuntimeState::Failed;
            }
        }
        Ok(true)
    }

    fn drain_one_application(&mut self) -> bool {
        let Some(command) = self.pending_application.pop_front() else {
            return false;
        };
        match command {
            ApplicationCommand::SetPrinterEnabled(enabled) => {
                if enabled {
                    self.terminal.enable_printing();
                } else {
                    self.terminal.disable_printing();
                }
                true
            }
            command => match self.throttle.handle_application_command(command) {
                Ok(_) => true,
                Err(rejected) => {
                    self.pending_application
                        .push_front(ApplicationCommand::Transmit(rejected.data));
                    false
                }
            },
        }
    }

    fn apply_output(&mut self, output: ThrottleOutput) -> Result<(), RuntimeError<T::Error>> {
        match output {
            ThrottleOutput::ToTransport(command) => {
                self.pending_transport.push_back(command);
            }
            ThrottleOutput::ToApplication(data) => {
                self.terminal
                    .receive_data(&data)
                    .map_err(RuntimeError::Terminal)?;
            }
        }
        Ok(())
    }

    fn flush_one_transport(&mut self) -> Result<FlushStatus, RuntimeError<T::Error>> {
        let Some(command) = self.pending_transport.pop_front() else {
            return Ok(FlushStatus::Empty);
        };
        match self.transport.send(command) {
            Ok(()) => Ok(FlushStatus::Sent),
            Err(TransportSendError {
                failure: TransportSendFailure::Backpressure,
                command,
                ..
            }) => {
                self.pending_transport.push_front(command);
                Ok(FlushStatus::Backpressured)
            }
            Err(error) => Err(RuntimeError::TransportSend {
                failure: error.failure,
                message: error.message,
            }),
        }
    }
}

impl<T, S> Drop for AppRuntime<T, S>
where
    T: Transport,
    S: Scheduler,
{
    fn drop(&mut self) {
        if self.state != RuntimeState::Joined {
            let _ = self.transport.shutdown();
            let _ = self.transport.join();
        }
    }
}
