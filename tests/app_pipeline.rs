use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use asr33emu::adapters::paper_tape::PunchFile;
use asr33emu::adapters::transport::{Transport, TransportSendError, TransportSendFailure};
use asr33emu::app::paper_tape::{FeedResult, ReaderFeed};
use asr33emu::app::{
    AppRuntime, ConnectionState, ImmediateTransmit, PumpStatus, RuntimeEvent, RuntimeState,
    Scheduler,
};
use asr33emu::core::events::{
    ApplicationCommand, CommunicationMode, ThrottleMode, TransportCommand, TransportEvent,
    TransportOperation,
};
use asr33emu::core::paper_tape::{PaperTape, PunchMode, ReaderOptions, ReaderState, TapeReader};
use asr33emu::core::terminal::TerminalOptions;
use asr33emu::core::throttle::ThrottleConfig;
use tempfile::tempdir;

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
    lifecycle: Arc<(AtomicBool, AtomicBool)>,
    start_error: bool,
    receive_error: bool,
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
        if self.start_error {
            return Err(FakeTransportError("start failed"));
        }
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
        if self.receive_error {
            self.receive_error = false;
            return Err(FakeTransportError("receive channel failed"));
        }
        Ok(self.incoming.pop_front())
    }

    fn shutdown(&mut self) -> Result<(), Self::Error> {
        self.shutdown = true;
        self.lifecycle.0.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn join(&mut self) -> Result<(), Self::Error> {
        self.joined = true;
        self.lifecycle.1.store(true, Ordering::SeqCst);
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

fn connected(runtime: &Runtime) -> &FakeTransport {
    runtime.transport().expect("test runtime is connected")
}

fn connected_mut(runtime: &mut Runtime) -> &mut FakeTransport {
    runtime.transport_mut().expect("test runtime is connected")
}

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

fn disconnected_runtime() -> Runtime {
    let mut runtime = AppRuntime::new_disconnected(
        FakeScheduler::default(),
        TerminalOptions {
            columns: 16,
            rows: 4,
            scrollback: 4,
            autowrap: false,
        },
        ThrottleConfig::default(),
    )
    .expect("valid disconnected runtime");
    runtime.start().expect("runtime starts without transport");
    runtime
}

fn reader_feed(data: &[u8]) -> ReaderFeed {
    let mut reader = TapeReader::new(ReaderOptions {
        skip_leading_nulls: false,
        auto_stop: false,
        set_msb: false,
    });
    reader.load(PaperTape::new(data.to_vec()));
    assert!(reader.start());
    ReaderFeed::new(reader, Duration::ZERO)
}

fn offer_reader_byte(feed: &mut ReaderFeed, runtime: &mut Runtime, now: Duration) {
    feed.tick(now, |byte| {
        match runtime.try_transmit(vec![byte]).expect("runtime running") {
            ImmediateTransmit::Accepted => FeedResult::Accepted,
            ImmediateTransmit::Backpressured(data) | ImmediateTransmit::Disconnected(data) => {
                FeedResult::Backpressured(data[0])
            }
        }
    });
}

fn drain_reader(feed: &mut ReaderFeed, runtime: &mut Runtime, mut now: Duration) {
    while feed.reader().state() == ReaderState::Running || feed.pending_count() != 0 {
        offer_reader_byte(feed, runtime, now);
        runtime.scheduler_mut().now = now;
        runtime.tick().expect("reader pipeline advances");
        if feed.awaiting_confirmation() && runtime.transmit_idle() {
            feed.confirm_transmitted();
        }
        assert!(feed.pending_count() <= 1);
        now += Duration::from_millis(3);
    }
    runtime.pump().expect("reader pipeline drains");
}

#[test]
fn line_to_local_rolls_reader_back_before_switching_route() {
    let mut runtime = runtime(true);
    let mut feed = reader_feed(b"ABC");
    offer_reader_byte(&mut feed, &mut runtime, Duration::ZERO);
    assert_eq!(feed.reader().position(), 1);
    assert_eq!(feed.pending_count(), 1);

    assert!(feed.pause_for_mode_change());
    assert_eq!(feed.reader().state(), ReaderState::Stopped);
    assert_eq!(feed.reader().position(), 0);
    assert_eq!(feed.pending_count(), 0);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL mode accepted");
    runtime.tick().expect("old LINE byte is dropped");

    assert!(feed.reader_mut().start());
    drain_reader(&mut feed, &mut runtime, Duration::from_millis(3));
    assert_eq!(&line(&runtime, 0)[..3], "ABC");
    assert!(connected(&runtime).sent.is_empty());
}

#[test]
fn local_to_line_rolls_reader_back_before_switching_route() {
    let mut runtime = runtime(true);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL mode accepted");
    runtime.tick().expect("LOCAL mode applies");
    let mut feed = reader_feed(b"ABC");
    offer_reader_byte(&mut feed, &mut runtime, Duration::ZERO);
    assert_eq!(feed.reader().position(), 1);
    assert_eq!(feed.pending_count(), 1);

    assert!(feed.pause_for_mode_change());
    assert_eq!(feed.reader().state(), ReaderState::Stopped);
    assert_eq!(feed.reader().position(), 0);
    assert_eq!(feed.pending_count(), 0);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Line,
        ))
        .expect("LINE mode accepted");
    runtime.tick().expect("old LOCAL byte is dropped");

    assert!(feed.reader_mut().start());
    drain_reader(&mut feed, &mut runtime, Duration::from_millis(3));
    let sent = connected(&runtime)
        .sent
        .iter()
        .flat_map(|command| match command {
            TransportCommand::Send(data) => data.iter().copied(),
        })
        .collect::<Vec<_>>();
    assert_eq!(sent, b"ABC");
    assert_eq!(line(&runtime, 0), "                ");
}

#[test]
fn line_to_local_mode_command_preserves_fifo_transmit_order() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::Transmit(b"A".to_vec()))
        .expect("LINE transmit accepted");
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL mode accepted");
    runtime.pump().expect("queued LINE work drains");
    runtime
        .submit(ApplicationCommand::Transmit(b"B".to_vec()))
        .expect("LOCAL transmit accepted");
    runtime.pump().expect("LOCAL work drains");

    let sent = connected(&runtime)
        .sent
        .iter()
        .flat_map(|command| match command {
            TransportCommand::Send(data) => data.iter().copied(),
        })
        .collect::<Vec<_>>();
    assert_eq!(sent, b"A");
    assert_eq!(&line(&runtime, 0)[..1], "B");
}

#[test]
fn local_to_line_mode_command_preserves_fifo_transmit_order() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL mode accepted");
    runtime.pump().expect("LOCAL mode applies");
    runtime
        .submit(ApplicationCommand::Transmit(b"A".to_vec()))
        .expect("LOCAL transmit accepted");
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Line,
        ))
        .expect("LINE mode accepted");
    runtime.pump().expect("queued LOCAL work drains");
    runtime
        .submit(ApplicationCommand::Transmit(b"B".to_vec()))
        .expect("LINE transmit accepted");
    runtime.pump().expect("LINE work drains");

    let sent = connected(&runtime)
        .sent
        .iter()
        .flat_map(|command| match command {
            TransportCommand::Send(data) => data.iter().copied(),
        })
        .collect::<Vec<_>>();
    assert_eq!(sent, b"B");
    assert_eq!(&line(&runtime, 0)[..1], "A");
}

#[test]
fn runtime_starts_and_local_loopback_operates_without_transport() {
    let mut runtime = disconnected_runtime();
    assert_eq!(runtime.state(), RuntimeState::Running);
    assert_eq!(runtime.connection_state(), &ConnectionState::Disconnected);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("configuration works offline");
    runtime
        .submit(ApplicationCommand::SetThrottleMode(
            ThrottleMode::Unthrottled,
        ))
        .expect("throttle configuration works offline");
    runtime
        .submit(ApplicationCommand::SetPrinterEnabled(true))
        .expect("printer configuration works offline");
    runtime.tick().expect("offline configuration applies");
    runtime
        .submit(ApplicationCommand::Transmit(b"OFFLINE".to_vec()))
        .expect("LOCAL accepts bytes offline");
    runtime.pump().expect("offline pipeline drains");
    assert_eq!(&line(&runtime, 0)[..7], "OFFLINE");
}

#[test]
fn local_cr_overstrikes_and_local_lf_advances_without_carriage_return() {
    let mut cr_runtime = disconnected_runtime();
    cr_runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL");
    cr_runtime
        .submit(ApplicationCommand::SetThrottleMode(
            ThrottleMode::Unthrottled,
        ))
        .expect("unthrottled");
    cr_runtime.tick().expect("LOCAL mode applies");
    cr_runtime
        .submit(ApplicationCommand::Transmit(b"ABC\rDEF".to_vec()))
        .expect("CR sequence");
    cr_runtime.pump().expect("CR sequence drains");
    let cr_line = cr_runtime
        .terminal()
        .line_history()
        .line(0)
        .expect("first line");
    assert_eq!(cr_line.strike_stack(0), &['A', 'D']);
    assert_eq!(cr_line.strike_stack(1), &['B', 'E']);
    assert_eq!(cr_line.strike_stack(2), &['C', 'F']);

    let mut lf_runtime = disconnected_runtime();
    lf_runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL");
    lf_runtime
        .submit(ApplicationCommand::SetThrottleMode(
            ThrottleMode::Unthrottled,
        ))
        .expect("unthrottled");
    lf_runtime.tick().expect("LOCAL mode applies");
    lf_runtime
        .submit(ApplicationCommand::Transmit(b"ABC\nDEF".to_vec()))
        .expect("LF sequence");
    lf_runtime.pump().expect("LF sequence drains");
    assert_eq!(lf_runtime.terminal().cursor_position(), (6, 1));
    assert_eq!(&line(&lf_runtime, 1)[3..6], "DEF");
}

#[test]
fn offline_local_loopback_can_feed_an_active_punch() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("offline.pt");
    let mut punch = PunchFile::open(&path, PunchMode::Overwrite).expect("punch opens");
    assert!(punch.start());
    let mut runtime = disconnected_runtime();
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL configured");
    runtime
        .submit(ApplicationCommand::SetThrottleMode(
            ThrottleMode::Unthrottled,
        ))
        .expect("unthrottled configured");
    runtime.tick().expect("offline configuration applies");
    runtime
        .submit(ApplicationCommand::Transmit(b"PUNCH\x80".to_vec()))
        .expect("offline bytes accepted");
    runtime.pump().expect("loopback drains");
    while let Some(event) = runtime.pop_event() {
        if let RuntimeEvent::TerminalForwarded(data) = event {
            punch.punch(&data).expect("punch write succeeds");
        }
    }
    drop(punch);
    assert_eq!(
        std::fs::read(path).expect("punch file readable"),
        b"PUNCH\x80"
    );
}

#[test]
fn line_rejects_disconnected_transmit_without_replaying_it_on_connect() {
    let mut runtime = disconnected_runtime();
    for _ in 0..100 {
        assert!(matches!(
            runtime.submit(ApplicationCommand::Transmit(b"STALE".to_vec())),
            Err(asr33emu::app::RuntimeError::Disconnected)
        ));
    }
    assert_eq!(
        runtime
            .try_transmit(b"READER".to_vec())
            .expect("typed rejection"),
        ImmediateTransmit::Disconnected(b"READER".to_vec())
    );
    runtime
        .connect(FakeTransport::default())
        .expect("later connection works");
    runtime.pump().expect("nothing stale remains");
    assert!(connected(&runtime).sent.is_empty());
}

#[test]
fn connect_failure_disconnect_and_reconnect_do_not_stop_runtime() {
    let mut runtime = disconnected_runtime();
    let failed = FakeTransport {
        start_error: true,
        ..FakeTransport::default()
    };
    let failed_lifecycle = failed.lifecycle.clone();
    assert!(runtime.connect(failed).is_err());
    assert_eq!(runtime.state(), RuntimeState::Running);
    assert!(matches!(
        runtime.connection_state(),
        ConnectionState::Failed { .. }
    ));
    assert!(failed_lifecycle.0.load(Ordering::SeqCst));
    assert!(failed_lifecycle.1.load(Ordering::SeqCst));

    runtime
        .connect(FakeTransport::default())
        .expect("retry connects");
    assert_eq!(runtime.connection_state(), &ConnectionState::Connected);
    let first_lifecycle = connected(&runtime).lifecycle.clone();
    runtime.disconnect().expect("disconnect joins transport");
    assert_eq!(runtime.state(), RuntimeState::Running);
    assert_eq!(runtime.connection_state(), &ConnectionState::Disconnected);
    assert!(first_lifecycle.0.load(Ordering::SeqCst));
    assert!(first_lifecycle.1.load(Ordering::SeqCst));
    runtime
        .connect(FakeTransport::default())
        .expect("reconnect works");
    assert_eq!(runtime.connection_state(), &ConnectionState::Connected);
}

#[test]
fn disconnect_discards_throttled_and_transport_pending_tx_before_reconnect() {
    let mut runtime = runtime(true);
    connected_mut(&mut runtime).blocked = true;
    runtime
        .submit(ApplicationCommand::Transmit(b"OLD".to_vec()))
        .expect("connected TX accepted");
    runtime.scheduler_mut().now = Duration::from_secs(1);
    assert_eq!(
        runtime.tick().expect("TX reaches transport"),
        PumpStatus::Backpressured
    );
    runtime.disconnect().expect("disconnect clears external TX");
    runtime
        .connect(FakeTransport::default())
        .expect("new transport connects");
    runtime.scheduler_mut().now = Duration::from_secs(10);
    runtime.pump().expect("new connection is idle");
    assert!(connected(&runtime).sent.is_empty());
}

#[test]
fn paper_reader_rolls_back_unconfirmed_byte_and_resumes_without_loss() {
    for capacity in [1, 8] {
        let mut runtime = runtime_with_throttle(
            true,
            ThrottleConfig {
                tx_rate_cps: 10,
                rx_rate_cps: 10,
                tx_queue_capacity: capacity,
                rx_queue_capacity: 1,
            },
        );
        let expected = b"ABCDE";
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            set_msb: false,
        });
        reader.load(PaperTape::new(expected.to_vec()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);

        assert_eq!(
            feed.tick(Duration::ZERO, |byte| {
                match runtime.try_transmit(vec![byte]).expect("connected") {
                    ImmediateTransmit::Accepted => FeedResult::Accepted,
                    ImmediateTransmit::Backpressured(data)
                    | ImmediateTransmit::Disconnected(data) => FeedResult::Backpressured(data[0]),
                }
            }),
            Some(Duration::from_millis(3))
        );
        assert_eq!(feed.reader().position(), 1);
        assert_eq!(feed.pending_count(), 1);
        for now in [3, 6, 9] {
            assert_eq!(
                feed.tick(Duration::from_millis(now), |_| FeedResult::Accepted),
                None
            );
            assert_eq!(feed.reader().position(), 1);
            assert_eq!(feed.pending_count(), 1);
        }
        runtime
            .disconnect()
            .expect("connection drops before TX is sent");
        feed.rollback_unconfirmed();
        feed.reader_mut().stop();
        assert_eq!(feed.reader().position(), 0);
        assert_eq!(feed.pending_count(), 0);

        runtime
            .connect(FakeTransport::default())
            .expect("reconnect succeeds");
        assert!(feed.reader_mut().start());
        let mut now = Duration::from_secs(1);
        while feed.reader().state() == ReaderState::Running || feed.pending_count() != 0 {
            feed.tick(now, |byte| {
                match runtime.try_transmit(vec![byte]).expect("connected") {
                    ImmediateTransmit::Accepted => FeedResult::Accepted,
                    ImmediateTransmit::Backpressured(data)
                    | ImmediateTransmit::Disconnected(data) => FeedResult::Backpressured(data[0]),
                }
            });
            runtime.scheduler_mut().now = now;
            runtime.tick().expect("pipeline advances");
            if feed.awaiting_confirmation() && runtime.transmit_idle() {
                feed.confirm_transmitted();
            }
            assert!(feed.pending_count() <= 1);
            now += Duration::from_millis(100);
        }
        runtime.pump().expect("pipeline drains");
        let sent = connected(&runtime)
            .sent
            .iter()
            .flat_map(|command| match command {
                TransportCommand::Send(data) => data.iter().copied(),
            })
            .collect::<Vec<_>>();
        assert_eq!(sent, expected, "capacity {capacity}");
    }
}

#[test]
fn startup_cr_is_session_scoped_and_only_armed_for_initial_line_mode() {
    let mut local = disconnected_runtime();
    local.configure_startup_cr(true);
    assert!(
        local
            .connect(FakeTransport {
                start_error: true,
                ..FakeTransport::default()
            })
            .is_err()
    );
    while local.pop_event().is_some() {}
    local
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL configured");
    local.tick().expect("mode applied");
    local
        .connect(FakeTransport::default())
        .expect("LOCAL connects");
    local.pump().expect("LOCAL startup settles");
    assert!(connected(&local).sent.is_empty());
    assert_eq!(line(&local, 0), "                ");
    assert!(local.pop_event().is_none());
    local.disconnect().expect("LOCAL connection closes");
    local
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Line,
        ))
        .expect("LINE configured later");
    local.tick().expect("LINE mode applies");
    local
        .connect(FakeTransport::default())
        .expect("later LINE reconnect succeeds");
    local.scheduler_mut().now = Duration::from_secs(1);
    local.pump().expect("later LINE connection settles");
    assert!(connected(&local).sent.is_empty());

    let mut line = disconnected_runtime();
    line.configure_startup_cr(true);
    assert!(
        line.connect(FakeTransport {
            start_error: true,
            ..FakeTransport::default()
        })
        .is_err()
    );
    line.connect(FakeTransport::default())
        .expect("first LINE connection succeeds");
    line.scheduler_mut().now = Duration::from_secs(1);
    line.pump().expect("startup CR drains");
    assert_eq!(connected(&line).sent, [TransportCommand::Send(vec![0x0d])]);
    line.disconnect().expect("first connection closes");
    line.connect(FakeTransport::default())
        .expect("reconnect succeeds");
    line.scheduler_mut().now = Duration::from_secs(2);
    line.pump().expect("reconnect settles");
    assert!(connected(&line).sent.is_empty());
}

#[test]
fn paper_reader_is_lossless_through_capacity_one_line_pipeline() {
    let mut runtime = runtime_with_throttle(
        false,
        ThrottleConfig {
            tx_queue_capacity: 1,
            rx_queue_capacity: 1,
            ..ThrottleConfig::default()
        },
    );
    let expected = (1_u8..=24).collect::<Vec<_>>();
    let mut reader = TapeReader::new(ReaderOptions {
        skip_leading_nulls: false,
        auto_stop: false,
        set_msb: false,
    });
    reader.load(PaperTape::new(expected.clone()));
    assert!(reader.start());
    let mut feed = ReaderFeed::new(reader, Duration::ZERO);
    let mut now = Duration::ZERO;

    while feed.reader().state() == ReaderState::Running || feed.pending_count() != 0 {
        let delay = feed.tick(now, |byte| match runtime.try_transmit(vec![byte]) {
            Ok(ImmediateTransmit::Accepted) => FeedResult::Accepted,
            Ok(ImmediateTransmit::Backpressured(data)) => {
                FeedResult::Backpressured(data.first().copied().map_or(byte, |value| value))
            }
            Ok(ImmediateTransmit::Disconnected(_)) => panic!("test transport disconnected"),
            Err(error) => panic!("reader transmit failed: {error}"),
        });
        runtime.tick().expect("runtime tick succeeds");
        if feed.awaiting_confirmation() && runtime.transmit_idle() {
            feed.confirm_transmitted();
        }
        assert!(feed.pending_count() <= 1);
        now += delay.unwrap_or(Duration::from_millis(3));
        runtime.scheduler_mut().now = now;
    }
    runtime.pump().expect("pipeline drains");

    let sent = connected(&runtime)
        .sent
        .iter()
        .flat_map(|command| match command {
            TransportCommand::Send(data) => data.iter().copied(),
        })
        .collect::<Vec<_>>();
    assert_eq!(sent, expected);
}

#[test]
fn paper_reader_uses_local_loopback_without_reaching_transport() {
    let mut runtime = disconnected_runtime();
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("mode accepted");
    runtime.tick().expect("mode applied");
    let mut reader = TapeReader::new(ReaderOptions {
        skip_leading_nulls: false,
        auto_stop: false,
        set_msb: false,
    });
    reader.load(PaperTape::new(b"LOCAL".to_vec()));
    assert!(reader.start());
    let mut feed = ReaderFeed::new(reader, Duration::ZERO);
    let mut now = Duration::ZERO;
    while feed.reader().state() == ReaderState::Running {
        let delay = feed.tick(now, |byte| match runtime.try_transmit(vec![byte]) {
            Ok(ImmediateTransmit::Accepted) => FeedResult::Accepted,
            Ok(ImmediateTransmit::Backpressured(data)) => {
                FeedResult::Backpressured(data.first().copied().map_or(byte, |value| value))
            }
            Ok(ImmediateTransmit::Disconnected(_)) => panic!("test transport disconnected"),
            Err(error) => panic!("reader transmit failed: {error}"),
        });
        runtime.tick().expect("loopback tick succeeds");
        if feed.awaiting_confirmation() && runtime.transmit_idle() {
            feed.confirm_transmitted();
        }
        now += delay.unwrap_or(Duration::from_millis(3));
        runtime.scheduler_mut().now = now;
    }
    runtime.pump().expect("loopback drains");

    assert!(runtime.transport().is_none());
    let line = &runtime.terminal().line_history().lines()[0];
    let rendered = (0..line.width())
        .filter_map(|column| line.strike_stack(column).last())
        .collect::<String>();
    assert_eq!(rendered, "LOCAL");
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
    let bytes = connected(&runtime)
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
        let sent = connected(&runtime)
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
        connected_mut(&mut runtime).receive(chunk);
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
        connected_mut(&mut runtime).receive(b"X");
    }
    assert_eq!(
        runtime.tick().expect("tick succeeds"),
        PumpStatus::WorkRemaining
    );
    assert!(!connected(&runtime).incoming.is_empty());
    runtime.pump().expect("remaining finite input drains");
    assert!(connected(&runtime).incoming.is_empty());
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
    connected_mut(&mut runtime).receive(b"HELLO");
    assert_eq!(runtime.pump().expect("pipeline succeeds"), PumpStatus::Idle);
    assert_eq!(&line(&runtime, 0)[..5], "HELLO");
    assert_eq!(runtime.terminal().cursor_position(), (5, 0));
}

#[test]
fn rx_cr_lf_and_multiple_chunks_update_terminal_in_order() {
    let mut runtime = runtime(false);
    connected_mut(&mut runtime).receive(b"AB\r");
    connected_mut(&mut runtime).receive(b"\nCD");
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
        connected(&runtime).sent,
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
        connected(&runtime).sent,
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
        connected(&runtime).sent,
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
        connected(&runtime).sent,
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
    assert!(connected(&runtime).sent.is_empty());
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
        connected(&runtime).sent,
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
    connected_mut(&mut runtime).receive(b"REMOTE");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(line(&runtime, 0), "                ");
}

#[test]
fn printer_state_controls_terminal_without_affecting_transport_boundaries() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetPrinterEnabled(false))
        .expect("printer disables");
    connected_mut(&mut runtime).receive(b"HIDDEN");
    runtime.pump().expect("pipeline succeeds");
    assert!(!runtime.terminal().printing_enabled());
    assert_eq!(line(&runtime, 0), "                ");

    runtime
        .submit(ApplicationCommand::SetPrinterEnabled(true))
        .expect("printer enables");
    connected_mut(&mut runtime).receive(b"VISIBLE");
    runtime.pump().expect("pipeline succeeds");
    assert_eq!(&line(&runtime, 0)[..7], "VISIBLE");
}

#[test]
fn transport_backpressure_retains_pending_command_for_retry() {
    let mut runtime = runtime(false);
    connected_mut(&mut runtime).blocked = true;
    runtime
        .submit(ApplicationCommand::Transmit(b"AB".to_vec()))
        .expect("TX queued");
    assert_eq!(
        runtime.pump().expect("backpressure is observable"),
        PumpStatus::Backpressured
    );
    assert!(connected(&runtime).sent.is_empty());

    connected_mut(&mut runtime).blocked = false;
    assert_eq!(runtime.pump().expect("retry succeeds"), PumpStatus::Idle);
    assert_eq!(
        connected(&runtime).sent,
        [TransportCommand::Send(b"AB".to_vec())]
    );
}

#[test]
fn transport_failure_disconnects_but_keeps_runtime_running() {
    let mut runtime = runtime(false);
    let lifecycle = connected(&runtime).lifecycle.clone();
    connected_mut(&mut runtime).fail(TransportOperation::Read, "device removed");
    assert_eq!(
        runtime.pump().expect("failure is an event"),
        PumpStatus::Idle
    );
    assert_eq!(runtime.state(), RuntimeState::Running);
    assert_eq!(
        runtime.connection_state(),
        &ConnectionState::Failed {
            message: "device removed".to_owned()
        }
    );
    assert_eq!(
        runtime.pop_event(),
        Some(RuntimeEvent::TransportFailed {
            operation: TransportOperation::Read,
            message: "device removed".to_owned()
        })
    );
    assert!(runtime.transport().is_none());
    assert!(lifecycle.0.load(Ordering::SeqCst));
    assert!(lifecycle.1.load(Ordering::SeqCst));
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL remains available");
    runtime.tick().expect("LOCAL mode applies");
    runtime
        .submit(ApplicationCommand::Transmit(b"OK".to_vec()))
        .expect("offline loopback remains available");
    runtime.pump().expect("offline loopback drains");
    assert_eq!(&line(&runtime, 0)[..2], "OK");
}

#[test]
fn direct_try_recv_error_fails_only_connection_and_allows_reconnect() {
    let mut runtime = runtime(false);
    let lifecycle = connected(&runtime).lifecycle.clone();
    connected_mut(&mut runtime).receive_error = true;
    assert_eq!(
        runtime.tick().expect("connection error is handled"),
        PumpStatus::Idle
    );
    assert_eq!(runtime.state(), RuntimeState::Running);
    assert_eq!(
        runtime.connection_state(),
        &ConnectionState::Failed {
            message: "receive channel failed".to_owned()
        }
    );
    assert!(runtime.transport().is_none());
    assert!(lifecycle.0.load(Ordering::SeqCst));
    assert!(lifecycle.1.load(Ordering::SeqCst));
    assert_eq!(
        runtime.pop_event(),
        Some(RuntimeEvent::ConnectionFailed(
            "receive channel failed".to_owned()
        ))
    );
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("LOCAL remains configurable");
    runtime.tick().expect("LOCAL applies");
    runtime
        .submit(ApplicationCommand::Transmit(b"LOCAL".to_vec()))
        .expect("LOCAL works offline");
    runtime.pump().expect("LOCAL drains");
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Line,
        ))
        .expect("LINE can be restored");
    runtime.tick().expect("LINE applies");
    runtime
        .connect(FakeTransport::default())
        .expect("reconnect succeeds");
    assert_eq!(runtime.connection_state(), &ConnectionState::Connected);
}

#[test]
fn terminal_forwarding_event_preserves_raw_bytes_when_printer_is_off() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetPrinterEnabled(false))
        .expect("runtime running");
    let original = b"\xc1\x1b[31mX\x1b[0m";
    connected_mut(&mut runtime).receive(original);
    runtime.pump().expect("RX drains");
    assert_eq!(
        runtime.pop_event(),
        Some(RuntimeEvent::TerminalForwarded(original.to_vec()))
    );
    assert_eq!(line(&runtime, 0), "                ");
}

#[test]
fn local_loopback_produces_raw_terminal_forwarding_event() {
    let mut runtime = runtime(false);
    runtime
        .submit(ApplicationCommand::SetCommunicationMode(
            CommunicationMode::Local,
        ))
        .expect("runtime running");
    runtime.pump().expect("mode applies");
    runtime
        .try_transmit(b"LOCAL\x80".to_vec())
        .expect("immediate submission succeeds");
    runtime.pump().expect("loopback drains");
    assert_eq!(
        runtime.pop_event(),
        Some(RuntimeEvent::TerminalForwarded(b"LOCAL\x80".to_vec()))
    );
}

#[test]
fn shutdown_and_join_are_idempotent_and_reject_later_bytes() {
    let mut runtime = runtime(false);
    let lifecycle = connected(&runtime).lifecycle.clone();
    runtime.shutdown().expect("shutdown succeeds");
    runtime.shutdown().expect("shutdown is idempotent");
    assert!(lifecycle.0.load(Ordering::SeqCst));
    assert!(
        runtime
            .submit(ApplicationCommand::Transmit(vec![1]))
            .is_err()
    );
    assert!(runtime.pump().is_err());
    assert!(runtime.transport().is_none());
    assert_eq!(line(&runtime, 0), "                ");

    runtime.join().expect("join succeeds");
    runtime.join().expect("join is idempotent");
    assert_eq!(runtime.state(), RuntimeState::Joined);
    assert!(lifecycle.1.load(Ordering::SeqCst));
}
