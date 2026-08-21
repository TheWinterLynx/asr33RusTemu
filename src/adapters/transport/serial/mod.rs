//! Blocking serial adapter with one worker owning the native port.

mod platform;
mod settings;

use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{Transport, TransportSendError, TransportSendFailure};
use crate::core::config::{SerialConfig, SerialParity, StopBits};
use crate::core::events::{TransportCommand, TransportEvent, TransportOperation};

const DEFAULT_QUEUE_CAPACITY: usize = 8;
const DEFAULT_RX_CAPACITY: usize = 8;
const IO_TIMEOUT: Duration = Duration::from_millis(20);
const READ_BUFFER_SIZE: usize = 4096;

trait PortIo: Read + Write + Send {}
impl<T> PortIo for T where T: Read + Write + Send {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    Queued,
    IgnoredEmpty,
}

#[derive(Debug)]
pub enum SerialAdapterError {
    Open(serialport::Error),
    Platform(io::Error),
    Spawn(io::Error),
    Backpressure,
    Closed,
    WorkerPanicked,
    Unsupported(String),
}

impl fmt::Display for SerialAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "cannot open serial port: {error}"),
            Self::Platform(error) => write!(formatter, "cannot configure serial port: {error}"),
            Self::Spawn(error) => write!(formatter, "cannot start serial worker: {error}"),
            Self::Backpressure => formatter.write_str("serial transmit queue is full"),
            Self::Closed => formatter.write_str("serial transport is closed"),
            Self::WorkerPanicked => formatter.write_str("serial worker panicked"),
            Self::Unsupported(message) => formatter.write_str(message),
        }
    }
}

impl Error for SerialAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::Platform(error) | Self::Spawn(error) => Some(error),
            Self::Backpressure | Self::Closed | Self::WorkerPanicked | Self::Unsupported(_) => None,
        }
    }
}

pub struct SerialTransport {
    port: Option<Box<dyn PortIo>>,
    commands: Option<CommandSender>,
    shutdown: Option<mpsc::Sender<()>>,
    events: Option<Receiver<TransportEvent>>,
    worker: Option<JoinHandle<()>>,
    tx_capacity: usize,
    rx_capacity: usize,
    info: String,
}

enum CommandSender {
    Bounded(SyncSender<TransportCommand>),
    Unbounded(mpsc::Sender<TransportCommand>),
}

impl SerialTransport {
    pub fn open(config: SerialConfig) -> Result<Self, SerialAdapterError> {
        Self::open_with_capacity(config, DEFAULT_QUEUE_CAPACITY)
    }

    pub fn open_with_capacity(
        config: SerialConfig,
        queue_capacity: usize,
    ) -> Result<Self, SerialAdapterError> {
        Self::open_with_capacities(config, queue_capacity, DEFAULT_RX_CAPACITY)
    }

    pub fn open_with_capacities(
        config: SerialConfig,
        tx_capacity: usize,
        rx_capacity: usize,
    ) -> Result<Self, SerialAdapterError> {
        Self::open_with(config, tx_capacity, rx_capacity, platform::open)
    }

    fn open_with<F>(
        config: SerialConfig,
        tx_capacity: usize,
        rx_capacity: usize,
        opener: F,
    ) -> Result<Self, SerialAdapterError>
    where
        F: FnOnce(&SerialConfig, Duration) -> Result<Box<dyn PortIo>, SerialAdapterError>,
    {
        if rx_capacity == 0 {
            return Err(SerialAdapterError::Unsupported(
                "serial receive queue capacity must be greater than zero".to_owned(),
            ));
        }
        let port = opener(&config, IO_TIMEOUT)?;
        Ok(Self::with_port(port, &config, tx_capacity, rx_capacity))
    }

    fn with_port(
        port: Box<dyn PortIo>,
        config: &SerialConfig,
        tx_capacity: usize,
        rx_capacity: usize,
    ) -> Self {
        Self {
            port: Some(port),
            commands: None,
            shutdown: None,
            events: None,
            worker: None,
            tx_capacity,
            rx_capacity,
            info: info_string(config),
        }
    }

    pub fn start(&mut self) -> Result<(), SerialAdapterError> {
        if self.worker.is_some() {
            return Ok(());
        }
        let port = self.port.take().ok_or(SerialAdapterError::Closed)?;
        let (command_tx, command_rx) = if self.tx_capacity == 0 {
            let (sender, receiver) = mpsc::channel();
            (CommandSender::Unbounded(sender), receiver)
        } else {
            let (sender, receiver) = mpsc::sync_channel(self.tx_capacity);
            (CommandSender::Bounded(sender), receiver)
        };
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(self.rx_capacity);
        let worker = thread::Builder::new()
            .name("asr33-serial".to_owned())
            .spawn(move || run_worker(port, command_rx, shutdown_rx, event_tx))
            .map_err(SerialAdapterError::Spawn)?;
        self.commands = Some(command_tx);
        self.shutdown = Some(shutdown_tx);
        self.events = Some(event_rx);
        self.worker = Some(worker);
        Ok(())
    }

    pub fn send(&self, command: TransportCommand) -> Result<SendOutcome, SerialAdapterError> {
        if matches!(&command, TransportCommand::Send(data) if data.is_empty()) {
            return Ok(SendOutcome::IgnoredEmpty);
        }
        let sender = self.commands.as_ref().ok_or(SerialAdapterError::Closed)?;
        match sender {
            CommandSender::Bounded(sender) => match sender.try_send(command) {
                Ok(()) => Ok(SendOutcome::Queued),
                Err(TrySendError::Full(_)) => Err(SerialAdapterError::Backpressure),
                Err(TrySendError::Disconnected(_)) => Err(SerialAdapterError::Closed),
            },
            CommandSender::Unbounded(sender) => sender
                .send(command)
                .map(|()| SendOutcome::Queued)
                .map_err(|_| SerialAdapterError::Closed),
        }
    }

    pub fn try_recv(&self) -> Result<Option<TransportEvent>, SerialAdapterError> {
        let events = self.events.as_ref().ok_or(SerialAdapterError::Closed)?;
        match events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) if self.worker.is_none() => Ok(None),
            Err(TryRecvError::Disconnected) => Err(SerialAdapterError::Closed),
        }
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<TransportEvent>, SerialAdapterError> {
        let events = self.events.as_ref().ok_or(SerialAdapterError::Closed)?;
        match events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) if self.worker.is_none() => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(SerialAdapterError::Closed),
        }
    }

    pub fn info(&self) -> &str {
        &self.info
    }

    pub fn close(&mut self) -> Result<(), SerialAdapterError> {
        self.shutdown()?;
        self.join()
    }

    pub fn shutdown(&mut self) -> Result<(), SerialAdapterError> {
        self.commands.take();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.port.take();
        Ok(())
    }

    pub fn join(&mut self) -> Result<(), SerialAdapterError> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| SerialAdapterError::WorkerPanicked)?;
        }
        Ok(())
    }
}

impl Transport for SerialTransport {
    type Error = SerialAdapterError;

    fn start(&mut self) -> Result<(), Self::Error> {
        SerialTransport::start(self)
    }

    fn send(&mut self, command: TransportCommand) -> Result<(), TransportSendError> {
        if matches!(&command, TransportCommand::Send(data) if data.is_empty()) {
            return Ok(());
        }
        let Some(sender) = self.commands.as_ref() else {
            return Err(transport_send_error(
                TransportSendFailure::Closed,
                command,
                "serial transport is closed",
            ));
        };
        match sender {
            CommandSender::Bounded(sender) => match sender.try_send(command) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(command)) => Err(transport_send_error(
                    TransportSendFailure::Backpressure,
                    command,
                    "serial transmit queue is full",
                )),
                Err(TrySendError::Disconnected(command)) => Err(transport_send_error(
                    TransportSendFailure::Closed,
                    command,
                    "serial transport is closed",
                )),
            },
            CommandSender::Unbounded(sender) => sender.send(command).map_err(|error| {
                transport_send_error(
                    TransportSendFailure::Closed,
                    error.0,
                    "serial transport is closed",
                )
            }),
        }
    }

    fn try_recv(&mut self) -> Result<Option<TransportEvent>, Self::Error> {
        SerialTransport::try_recv(self)
    }

    fn shutdown(&mut self) -> Result<(), Self::Error> {
        SerialTransport::shutdown(self)
    }

    fn join(&mut self) -> Result<(), Self::Error> {
        SerialTransport::join(self)
    }
}

impl Drop for SerialTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn run_worker(
    mut port: Box<dyn PortIo>,
    commands: Receiver<TransportCommand>,
    shutdown: Receiver<()>,
    events: SyncSender<TransportEvent>,
) {
    let mut buffer = [0_u8; READ_BUFFER_SIZE];
    let mut pending_rx = None;
    loop {
        match shutdown.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }

        if let Some(event) = pending_rx.take() {
            match events.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => pending_rx = Some(event),
                Err(TrySendError::Disconnected(_)) => return,
            }
        }

        // Process at most one TX chunk per turn so sustained output cannot
        // starve RX now that one worker owns both directions.
        match commands.try_recv() {
            Ok(TransportCommand::Send(data)) => {
                if let Err(error) = port.write_all(&data) {
                    report_error(&events, &shutdown, TransportOperation::Write, error);
                    return;
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return,
        }

        if pending_rx.is_some() {
            match shutdown.recv_timeout(IO_TIMEOUT) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            }
        }

        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(length) => {
                let event = TransportEvent::Received(buffer[..length].to_vec());
                match events.try_send(event) {
                    Ok(()) => {}
                    Err(TrySendError::Full(event)) => pending_rx = Some(event),
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                report_error(&events, &shutdown, TransportOperation::Read, error);
                return;
            }
        }
    }
}

fn report_error(
    events: &SyncSender<TransportEvent>,
    shutdown: &Receiver<()>,
    operation: TransportOperation,
    error: io::Error,
) {
    let mut event = TransportEvent::Failed {
        operation,
        message: error.to_string(),
    };
    loop {
        match events.try_send(event) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(returned)) => event = returned,
        }
        match shutdown.recv_timeout(IO_TIMEOUT) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn transport_send_error(
    failure: TransportSendFailure,
    command: TransportCommand,
    message: &str,
) -> TransportSendError {
    TransportSendError {
        failure,
        command,
        message: message.to_owned(),
    }
}

fn info_string(config: &SerialConfig) -> String {
    format!(
        "{}:{}-{}{}{}",
        config.port,
        config.baudrate,
        u8::from(config.databits),
        parity_label(config.parity),
        stop_bits_label(config.stopbits)
    )
}

fn parity_label(parity: SerialParity) -> char {
    match parity {
        SerialParity::None => 'N',
        SerialParity::Even => 'E',
        SerialParity::Odd => 'O',
        SerialParity::Mark => 'M',
        SerialParity::Space => 'S',
    }
}

fn stop_bits_label(stop_bits: StopBits) -> &'static str {
    match stop_bits {
        StopBits::One => "1",
        StopBits::OnePointFive => "1.5",
        StopBits::Two => "2",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver, Sender};

    use super::*;
    use crate::core::config::DataBits;

    type FakeTransport = (
        SerialTransport,
        Sender<Result<Vec<u8>, io::Error>>,
        Receiver<Vec<u8>>,
    );

    struct FakePort {
        reads: Receiver<Result<Vec<u8>, io::Error>>,
        writes: Sender<Vec<u8>>,
        pending: Vec<u8>,
    }

    struct WriteErrorPort;

    impl Read for WriteErrorPort {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }

    impl Write for WriteErrorPort {
        fn write(&mut self, _data: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct GatedWritePort {
        writes: Sender<Vec<u8>>,
        gate: Receiver<()>,
    }

    impl Read for GatedWritePort {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }

    impl Write for GatedWritePort {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.writes
                .send(data.to_vec())
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            self.gate.recv().map_err(|_| io::ErrorKind::BrokenPipe)?;
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for FakePort {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.pending.is_empty() {
                match self.reads.try_recv() {
                    Ok(Ok(data)) => self.pending = data,
                    Ok(Err(error)) => return Err(error),
                    Err(_) => return Err(io::ErrorKind::WouldBlock.into()),
                }
            }
            let length = self.pending.len().min(buffer.len());
            buffer[..length].copy_from_slice(&self.pending[..length]);
            self.pending.drain(..length);
            Ok(length)
        }
    }

    impl Write for FakePort {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.writes
                .send(data.to_vec())
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn config() -> SerialConfig {
        SerialConfig {
            port: "TEST".to_owned(),
            baudrate: 110,
            databits: DataBits::Seven,
            parity: SerialParity::Mark,
            stopbits: StopBits::OnePointFive,
        }
    }

    fn transport() -> FakeTransport {
        let (read_tx, read_rx) = mpsc::channel();
        let (write_tx, write_rx) = mpsc::channel();
        let port = FakePort {
            reads: read_rx,
            writes: write_tx,
            pending: Vec::new(),
        };
        let mut transport = SerialTransport::with_port(Box::new(port), &config(), 8, 8);
        assert!(transport.worker.is_none());
        transport.start().expect("fake worker must start");
        (transport, read_tx, write_rx)
    }

    #[test]
    fn worker_routes_tx_and_rx_in_order() {
        let (mut transport, read_tx, write_rx) = transport();
        assert_eq!(
            transport
                .send(TransportCommand::Send(b"AB".to_vec()))
                .expect("queue has capacity"),
            SendOutcome::Queued
        );
        assert_eq!(
            transport
                .send(TransportCommand::Send(b"CD".to_vec()))
                .expect("queue has capacity"),
            SendOutcome::Queued
        );
        assert_eq!(
            write_rx.recv_timeout(Duration::from_secs(1)),
            Ok(b"AB".to_vec())
        );
        assert_eq!(
            write_rx.recv_timeout(Duration::from_secs(1)),
            Ok(b"CD".to_vec())
        );

        read_tx.send(Ok(b"RX".to_vec())).expect("worker is alive");
        assert_eq!(
            transport
                .recv_timeout(Duration::from_secs(1))
                .expect("worker remains connected"),
            Some(TransportEvent::Received(b"RX".to_vec()))
        );
        transport.close().expect("worker must stop");
    }

    #[test]
    fn created_transport_has_no_worker_until_start() {
        let (_read_tx, read_rx) = mpsc::channel();
        let (write_tx, _write_rx) = mpsc::channel();
        let port = FakePort {
            reads: read_rx,
            writes: write_tx,
            pending: Vec::new(),
        };
        let mut transport = SerialTransport::with_port(Box::new(port), &config(), 8, 1);
        assert!(transport.worker.is_none());
        assert!(matches!(
            transport.send(TransportCommand::Send(vec![1])),
            Err(SerialAdapterError::Closed)
        ));
        transport.start().expect("start creates worker");
        assert!(transport.worker.is_some());
        transport.close().expect("worker stops");
    }

    #[test]
    fn bounded_rx_retains_pending_chunk_and_keeps_tx_live() {
        let (read_tx, read_rx) = mpsc::channel();
        let (write_tx, write_rx) = mpsc::channel();
        let port = FakePort {
            reads: read_rx,
            writes: write_tx,
            pending: Vec::new(),
        };
        let mut transport = SerialTransport::with_port(Box::new(port), &config(), 8, 1);
        transport.start().expect("fake worker starts");
        for chunk in [b"A".to_vec(), b"BC".to_vec(), b"D".to_vec()] {
            read_tx.send(Ok(chunk)).expect("worker is alive");
        }
        transport
            .send(TransportCommand::Send(b"TX".to_vec()))
            .expect("TX remains available while RX is full");
        assert_eq!(
            write_rx.recv_timeout(Duration::from_secs(1)),
            Ok(b"TX".to_vec())
        );
        let mut received = Vec::new();
        for _ in 0..3 {
            match transport
                .recv_timeout(Duration::from_secs(1))
                .expect("worker remains connected")
                .expect("RX event arrives")
            {
                TransportEvent::Received(data) => received.extend(data),
                TransportEvent::Failed { message, .. } => panic!("unexpected failure: {message}"),
            }
        }
        assert_eq!(received, b"ABCD");
        transport.close().expect("worker stops");
    }

    #[test]
    fn close_rejects_new_commands_and_joins_worker() {
        let (mut transport, _read_tx, _write_rx) = transport();
        assert_eq!(
            transport
                .send(TransportCommand::Send(Vec::new()))
                .expect("empty commands are ignored"),
            SendOutcome::IgnoredEmpty
        );
        transport.close().expect("worker must stop");
        assert!(transport.worker.is_none());
        assert!(matches!(
            transport.send(TransportCommand::Send(vec![1])),
            Err(SerialAdapterError::Closed)
        ));
    }

    #[test]
    fn worker_reports_read_errors_and_terminates() {
        let (mut transport, read_tx, _write_rx) = transport();
        read_tx
            .send(Err(io::Error::other("read failed")))
            .expect("worker is alive");
        assert_eq!(
            transport
                .recv_timeout(Duration::from_secs(1))
                .expect("error event must arrive"),
            Some(TransportEvent::Failed {
                operation: TransportOperation::Read,
                message: "read failed".to_owned(),
            })
        );
        transport.close().expect("failed worker can be joined");
    }

    #[test]
    fn worker_reports_write_errors_and_terminates() {
        let mut transport = SerialTransport::with_port(Box::new(WriteErrorPort), &config(), 8, 8);
        transport.start().expect("fake worker must start");
        transport
            .send(TransportCommand::Send(vec![1]))
            .expect("queue has capacity");
        assert_eq!(
            transport
                .recv_timeout(Duration::from_secs(1))
                .expect("error event must arrive"),
            Some(TransportEvent::Failed {
                operation: TransportOperation::Write,
                message: "write failed".to_owned(),
            })
        );
        transport.close().expect("failed worker can be joined");
    }

    #[test]
    fn opening_errors_are_returned_before_a_worker_is_started() {
        let result = SerialTransport::open_with(config(), 8, 8, |_config, _timeout| {
            Err(SerialAdapterError::Unsupported("open failed".to_owned()))
        });
        assert!(matches!(
            result,
            Err(SerialAdapterError::Unsupported(message)) if message == "open failed"
        ));
    }

    #[test]
    fn bounded_command_queue_reports_backpressure() {
        let (write_tx, write_rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel();
        let port = GatedWritePort {
            writes: write_tx,
            gate: gate_rx,
        };
        let mut transport = SerialTransport::with_port(Box::new(port), &config(), 1, 8);
        transport.start().expect("fake worker must start");
        transport
            .send(TransportCommand::Send(vec![1]))
            .expect("first command starts writing");
        assert_eq!(write_rx.recv_timeout(Duration::from_secs(1)), Ok(vec![1]));
        transport
            .send(TransportCommand::Send(vec![2]))
            .expect("second command fills queue");
        assert!(matches!(
            transport.send(TransportCommand::Send(vec![3])),
            Err(SerialAdapterError::Backpressure)
        ));
        transport.commands.take();
        transport
            .shutdown
            .take()
            .expect("shutdown sender exists")
            .send(())
            .expect("worker is alive");
        gate_tx.send(()).expect("release active write");
        transport.close().expect("worker must stop");
    }

    #[test]
    fn zero_capacity_preserves_python_unbounded_queue_behavior() {
        let (read_tx, read_rx) = mpsc::channel();
        let (write_tx, _write_rx) = mpsc::channel();
        drop(read_tx);
        let port = FakePort {
            reads: read_rx,
            writes: write_tx,
            pending: Vec::new(),
        };
        let mut transport = SerialTransport::with_port(Box::new(port), &config(), 0, 8);
        transport.start().expect("fake worker must start");
        for value in 0..100_u8 {
            assert_eq!(
                transport
                    .send(TransportCommand::Send(vec![value]))
                    .expect("unbounded queue never reports capacity"),
                SendOutcome::Queued
            );
        }
        transport.close().expect("worker must stop");
    }

    #[test]
    fn info_string_preserves_extended_settings() {
        let (mut transport, _read_tx, _write_rx) = transport();
        assert_eq!(transport.info(), "TEST:110-7M1.5");
        transport.close().expect("worker must stop");
    }
}
