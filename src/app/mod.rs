//! Minimal synchronous composition of transport, throttle, and terminal.

pub mod paper_tape;

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

pub mod config_controller;

use crate::adapters::transport::{Transport, TransportSendError, TransportSendFailure};
use crate::core::events::{
    ApplicationCommand, CommunicationMode, DataFlow, ThrottleOutput, TransportCommand,
    TransportEvent, TransportOperation,
};
use crate::core::terminal::{CharacterEvent, Terminal, TerminalError, TerminalOptions};
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
    Shutdown,
    Joined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connected,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    ConnectionFailed(String),
    TransportFailed {
        operation: TransportOperation,
        message: String,
    },
    TerminalForwarded(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImmediateTransmit {
    Accepted,
    Backpressured(Vec<u8>),
    Disconnected(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpStatus {
    Idle,
    Backpressured,
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
    Disconnected,
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
            Self::Disconnected => formatter.write_str("serial connection is disconnected"),
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
            Self::InvalidState(_) | Self::TransportSend { .. } | Self::Disconnected => None,
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
    transport: Option<T>,
    scheduler: S,
    pending_application: VecDeque<ApplicationCommand>,
    pending_transport_event: Option<TransportEvent>,
    pending_transport: VecDeque<TransportCommand>,
    events: VecDeque<RuntimeEvent>,
    state: RuntimeState,
    connection: ConnectionState,
    startup_cr_pending: bool,
    startup_cr_consumed: bool,
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
            transport: Some(transport),
            scheduler,
            pending_application: VecDeque::new(),
            pending_transport_event: None,
            pending_transport: VecDeque::new(),
            events: VecDeque::new(),
            state: RuntimeState::Created,
            connection: ConnectionState::Disconnected,
            startup_cr_pending: false,
            startup_cr_consumed: false,
        })
    }

    pub fn new_disconnected(
        scheduler: S,
        terminal_options: TerminalOptions,
        throttle_config: ThrottleConfig,
    ) -> Result<Self, RuntimeError<T::Error>> {
        let terminal = Terminal::new(terminal_options).map_err(RuntimeError::Terminal)?;
        let initial_time = scheduler.now();
        Ok(Self {
            terminal,
            throttle: DataThrottle::new(throttle_config, initial_time),
            transport: None,
            scheduler,
            pending_application: VecDeque::new(),
            pending_transport_event: None,
            pending_transport: VecDeque::new(),
            events: VecDeque::new(),
            state: RuntimeState::Created,
            connection: ConnectionState::Disconnected,
            startup_cr_pending: false,
            startup_cr_consumed: false,
        })
    }

    pub fn start(&mut self) -> Result<(), RuntimeError<T::Error>> {
        self.start_with_initial_commands(std::iter::empty())
    }

    pub fn start_with_initial_commands<I>(
        &mut self,
        commands: I,
    ) -> Result<(), RuntimeError<T::Error>>
    where
        I: IntoIterator<Item = ApplicationCommand>,
    {
        match self.state {
            RuntimeState::Created => {
                for command in commands {
                    if let Err(rejected) = self.apply_application_command(command) {
                        self.pending_application.push_back(rejected);
                    }
                }
                self.state = RuntimeState::Running;
                if let Some(transport) = self.transport.take() {
                    self.connect(transport)
                } else {
                    Ok(())
                }
            }
            RuntimeState::Running => Ok(()),
            state => Err(RuntimeError::InvalidState(state)),
        }
    }

    pub fn submit(&mut self, command: ApplicationCommand) -> Result<(), RuntimeError<T::Error>> {
        self.require_running()?;
        if matches!(command, ApplicationCommand::Transmit(_))
            && self.throttle.communication_mode() == CommunicationMode::Line
            && self.connection != ConnectionState::Connected
        {
            return Err(RuntimeError::Disconnected);
        }
        self.pending_application.push_back(command);
        Ok(())
    }

    pub fn try_transmit(
        &mut self,
        data: Vec<u8>,
    ) -> Result<ImmediateTransmit, RuntimeError<T::Error>> {
        self.require_running()?;
        if self.throttle.communication_mode() == CommunicationMode::Line
            && self.connection != ConnectionState::Connected
        {
            return Ok(ImmediateTransmit::Disconnected(data));
        }
        match self.throttle.enqueue_tx(data) {
            Ok(_) => Ok(ImmediateTransmit::Accepted),
            Err(rejected) => Ok(ImmediateTransmit::Backpressured(rejected.data)),
        }
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
        self.disconnect()?;
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
        self.state = RuntimeState::Joined;
        Ok(())
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub fn pop_character_event(&mut self) -> Option<CharacterEvent> {
        self.terminal.pop_character_event()
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn pop_event(&mut self) -> Option<RuntimeEvent> {
        self.events.pop_front()
    }

    pub fn transport(&self) -> Option<&T> {
        self.transport.as_ref()
    }

    pub fn transport_mut(&mut self) -> Option<&mut T> {
        self.transport.as_mut()
    }

    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }

    pub fn scheduler_mut(&mut self) -> &mut S {
        &mut self.scheduler
    }

    #[must_use]
    pub fn connection_state(&self) -> &ConnectionState {
        &self.connection
    }

    pub fn configure_startup_cr(&mut self, enabled: bool) {
        self.startup_cr_pending = enabled
            && !self.startup_cr_consumed
            && self.throttle.communication_mode() == CommunicationMode::Line;
    }

    pub fn connect(&mut self, mut transport: T) -> Result<(), RuntimeError<T::Error>> {
        self.require_running()?;
        if self.connection == ConnectionState::Connected {
            return Ok(());
        }
        match transport.start() {
            Ok(()) => {
                self.transport = Some(transport);
                self.connection = ConnectionState::Connected;
                if self.startup_cr_pending && !self.startup_cr_consumed {
                    if self.throttle.communication_mode() == CommunicationMode::Line {
                        if self.throttle.enqueue_tx(vec![b'\r']).is_ok() {
                            self.startup_cr_pending = false;
                            self.startup_cr_consumed = true;
                        }
                    } else {
                        self.startup_cr_pending = false;
                        self.startup_cr_consumed = true;
                    }
                }
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                let _ = transport.shutdown();
                let _ = transport.join();
                self.record_connection_failure(message);
                Err(RuntimeError::Transport(error))
            }
        }
    }

    pub fn record_connection_failure(&mut self, message: String) {
        self.clear_external_work();
        self.connection = ConnectionState::Failed {
            message: message.clone(),
        };
        self.events
            .push_back(RuntimeEvent::ConnectionFailed(message));
    }

    pub fn disconnect(&mut self) -> Result<(), RuntimeError<T::Error>> {
        self.clear_external_work();
        let result = if let Some(mut transport) = self.transport.take() {
            let shutdown = transport.shutdown();
            let join = transport.join();
            shutdown.and(join).map_err(RuntimeError::Transport)
        } else {
            Ok(())
        };
        match result {
            Ok(()) => {
                self.connection = ConnectionState::Disconnected;
                Ok(())
            }
            Err(error) => {
                self.connection = ConnectionState::Failed {
                    message: error.to_string(),
                };
                Err(error)
            }
        }
    }

    /// Whether every accepted application TX has reached its destination
    /// boundary: the transport adapter in LINE or Terminal in LOCAL.
    #[must_use]
    pub fn transmit_idle(&self) -> bool {
        self.throttle.transmit_idle()
            && self.pending_transport.is_empty()
            && !self
                .pending_application
                .iter()
                .any(|command| matches!(command, ApplicationCommand::Transmit(_)))
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
                let Some(transport) = self.transport.as_mut() else {
                    return Ok(false);
                };
                match transport.try_recv() {
                    Ok(Some(event)) => event,
                    Ok(None) => return Ok(false),
                    Err(error) => {
                        let message = error.to_string();
                        self.events
                            .push_back(RuntimeEvent::ConnectionFailed(message.clone()));
                        self.fail_active_connection(message);
                        return Ok(true);
                    }
                }
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
                self.events.push_back(RuntimeEvent::TransportFailed {
                    operation,
                    message: message.clone(),
                });
                self.fail_active_connection(message);
            }
        }
        Ok(true)
    }

    fn drain_one_application(&mut self) -> bool {
        let Some(command) = self.pending_application.pop_front() else {
            return false;
        };
        match self.apply_application_command(command) {
            Ok(()) => true,
            Err(rejected) => {
                self.pending_application.push_front(rejected);
                false
            }
        }
    }

    fn apply_application_command(
        &mut self,
        command: ApplicationCommand,
    ) -> Result<(), ApplicationCommand> {
        match command {
            ApplicationCommand::SetPrinterEnabled(enabled) => {
                if enabled {
                    self.terminal.enable_printing();
                } else {
                    self.terminal.disable_printing();
                }
                Ok(())
            }
            command => match self.throttle.handle_application_command(command) {
                Ok(_) => Ok(()),
                Err(rejected) => Err(ApplicationCommand::Transmit(rejected.data)),
            },
        }
    }

    fn apply_output(&mut self, output: ThrottleOutput) -> Result<(), RuntimeError<T::Error>> {
        match output {
            ThrottleOutput::ToTransport(command) => {
                self.pending_transport.push_back(command);
            }
            ThrottleOutput::ToApplication(data) => {
                let effects = self
                    .terminal
                    .receive_data(&data)
                    .map_err(RuntimeError::Terminal)?;
                if !effects.forwarded_bytes.is_empty() {
                    self.events
                        .push_back(RuntimeEvent::TerminalForwarded(effects.forwarded_bytes));
                }
            }
        }
        Ok(())
    }

    fn flush_one_transport(&mut self) -> Result<FlushStatus, RuntimeError<T::Error>> {
        let Some(command) = self.pending_transport.pop_front() else {
            return Ok(FlushStatus::Empty);
        };
        let Some(transport) = self.transport.as_mut() else {
            return Ok(FlushStatus::Empty);
        };
        match transport.send(command) {
            Ok(()) => Ok(FlushStatus::Sent),
            Err(TransportSendError {
                failure: TransportSendFailure::Backpressure,
                command,
                ..
            }) => {
                self.pending_transport.push_front(command);
                Ok(FlushStatus::Backpressured)
            }
            Err(error) => {
                let failure = error.failure;
                let message = error.message;
                self.fail_active_connection(message.clone());
                Err(RuntimeError::TransportSend { failure, message })
            }
        }
    }

    fn clear_external_work(&mut self) {
        self.throttle.clear_external_tx();
        self.pending_transport.clear();
        self.pending_transport_event = None;
        self.pending_application
            .retain(|command| !matches!(command, ApplicationCommand::Transmit(_)));
    }

    fn fail_active_connection(&mut self, message: String) {
        self.clear_external_work();
        if let Some(mut transport) = self.transport.take() {
            let _ = transport.shutdown();
            let _ = transport.join();
        }
        self.connection = ConnectionState::Failed { message };
    }
}

impl<T, S> Drop for AppRuntime<T, S>
where
    T: Transport,
    S: Scheduler,
{
    fn drop(&mut self) {
        if self.state != RuntimeState::Joined
            && let Some(mut transport) = self.transport.take()
        {
            let _ = transport.shutdown();
            let _ = transport.join();
        }
    }
}
