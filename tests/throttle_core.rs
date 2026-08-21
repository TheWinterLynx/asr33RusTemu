use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use asr33emu::core::events::{
    ApplicationCommand, CommunicationMode, DataFlow, ThrottleMode, ThrottleOutput,
    TransportCommand, TransportEvent,
};
use asr33emu::core::throttle::{DataThrottle, EnqueueOutcome, ThrottleConfig, ThrottleStep};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SharedCase {
    name: String,
    direction: String,
    rate_cps: i64,
    throttled: bool,
    chunks_hex: Vec<String>,
    emissions_hex: Vec<String>,
    waits_ms: Vec<u64>,
}

fn fixture_cases() -> Vec<SharedCase> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/throttle_cases.json");
    let json = fs::read_to_string(path).expect("shared throttle fixture must be readable");
    serde_json::from_str(&json).expect("shared throttle fixture must be valid JSON")
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2));
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).expect("valid hex fixture"))
        .collect()
}

fn emitted_data(step: ThrottleStep) -> Option<Vec<u8>> {
    match step {
        ThrottleStep::Output {
            output: ThrottleOutput::ToTransport(TransportCommand::Send(data)),
            ..
        }
        | ThrottleStep::Output {
            output: ThrottleOutput::ToApplication(data),
            ..
        } => Some(data),
        ThrottleStep::Dropped { data, .. } => Some(data),
        ThrottleStep::Idle | ThrottleStep::Wait { .. } => None,
    }
}

#[test]
fn shared_python_rust_timing_and_chunk_vectors_match() {
    for case in fixture_cases() {
        let is_tx = case.direction == "tx";
        let config = ThrottleConfig {
            tx_rate_cps: if is_tx { case.rate_cps } else { 10 },
            rx_rate_cps: if is_tx { 10 } else { case.rate_cps },
            ..ThrottleConfig::default()
        };
        let mut throttle = DataThrottle::new(config, Duration::ZERO);
        if !case.throttled {
            throttle.set_throttle_mode(ThrottleMode::Unthrottled);
        }
        for chunk in &case.chunks_hex {
            let result = if is_tx {
                throttle.enqueue_tx(decode_hex(chunk))
            } else {
                throttle.enqueue_rx(decode_hex(chunk))
            };
            assert_eq!(result, Ok(EnqueueOutcome::Queued), "{}", case.name);
        }

        let mut now = Duration::ZERO;
        let mut waits = Vec::new();
        let mut emissions = Vec::new();
        loop {
            let step = if is_tx {
                throttle.poll_tx(now)
            } else {
                throttle.poll_rx(now)
            };
            match step {
                ThrottleStep::Idle => break,
                ThrottleStep::Wait { duration, .. } => {
                    waits.push(duration.as_millis() as u64);
                    now += duration;
                }
                other => {
                    if let Some(data) = emitted_data(other) {
                        emissions.push(data);
                    }
                }
            }
        }

        let expected = case
            .emissions_hex
            .iter()
            .map(|value| decode_hex(value))
            .collect::<Vec<_>>();
        assert_eq!(emissions, expected, "{} emissions", case.name);
        assert_eq!(waits, case.waits_ms, "{} waits", case.name);
    }
}

#[test]
fn tx_and_rx_have_independent_rates_and_timestamps() {
    let mut throttle = DataThrottle::new(
        ThrottleConfig {
            tx_rate_cps: 10,
            rx_rate_cps: 5,
            ..ThrottleConfig::default()
        },
        Duration::ZERO,
    );
    assert_eq!(throttle.enqueue_tx(vec![b'T']), Ok(EnqueueOutcome::Queued));
    assert_eq!(throttle.enqueue_rx(vec![b'R']), Ok(EnqueueOutcome::Queued));

    assert_eq!(
        throttle.poll_tx(Duration::ZERO),
        ThrottleStep::Wait {
            flow: DataFlow::Tx,
            duration: Duration::from_millis(100)
        }
    );
    assert_eq!(
        throttle.poll_rx(Duration::ZERO),
        ThrottleStep::Wait {
            flow: DataFlow::Rx,
            duration: Duration::from_millis(200)
        }
    );
}

#[test]
fn disabling_throttle_mid_chunk_emits_remainder_whole() {
    let mut throttle = DataThrottle::new(ThrottleConfig::default(), Duration::ZERO);
    throttle
        .enqueue_tx(b"ABC".to_vec())
        .expect("queue has space");
    assert!(matches!(
        throttle.poll_tx(Duration::ZERO),
        ThrottleStep::Wait { .. }
    ));
    assert_eq!(
        emitted_data(throttle.poll_tx(Duration::from_millis(100))),
        Some(b"A".to_vec())
    );
    throttle.set_throttle_mode(ThrottleMode::Unthrottled);
    assert_eq!(
        emitted_data(throttle.poll_tx(Duration::from_millis(100))),
        Some(b"BC".to_vec())
    );
}

#[test]
fn rate_change_mid_chunk_applies_to_next_chunk() {
    let mut throttle = DataThrottle::new(ThrottleConfig::default(), Duration::ZERO);
    throttle
        .enqueue_tx(b"AB".to_vec())
        .expect("queue has space");
    assert_eq!(
        throttle.poll_tx(Duration::ZERO),
        ThrottleStep::Wait {
            flow: DataFlow::Tx,
            duration: Duration::from_millis(100),
        }
    );
    assert_eq!(
        emitted_data(throttle.poll_tx(Duration::from_millis(100))),
        Some(b"A".to_vec())
    );
    throttle.set_tx_rate(5);
    assert_eq!(
        throttle.poll_tx(Duration::from_millis(100)),
        ThrottleStep::Wait {
            flow: DataFlow::Tx,
            duration: Duration::from_millis(100)
        }
    );
}

#[test]
fn loopback_routes_tx_ignores_remote_rx_and_drops_stale_data_after_mode_change() {
    let mut throttle = DataThrottle::new(ThrottleConfig::default(), Duration::ZERO);
    throttle.set_throttle_mode(ThrottleMode::Unthrottled);
    throttle.set_communication_mode(CommunicationMode::Local);
    assert_eq!(
        throttle.enqueue_tx(b"LOCAL".to_vec()),
        Ok(EnqueueOutcome::Queued)
    );
    assert_eq!(
        throttle.handle_transport_event(TransportEvent::Received(b"REMOTE".to_vec())),
        Ok(EnqueueOutcome::IgnoredDuringLoopback)
    );
    assert_eq!(
        throttle.poll_loopback(Duration::ZERO),
        ThrottleStep::Output {
            flow: DataFlow::Loopback,
            output: ThrottleOutput::ToApplication(b"LOCAL".to_vec())
        }
    );

    throttle
        .enqueue_tx(b"STALE".to_vec())
        .expect("queue has space");
    throttle.set_communication_mode(CommunicationMode::Line);
    assert_eq!(
        throttle.poll_loopback(Duration::ZERO),
        ThrottleStep::Dropped {
            flow: DataFlow::Loopback,
            data: b"STALE".to_vec()
        }
    );
    assert_eq!(throttle.poll_rx(Duration::ZERO), ThrottleStep::Idle);
}

#[test]
fn entering_local_mode_clears_stale_loopback_chunks() {
    let mut throttle = DataThrottle::new(ThrottleConfig::default(), Duration::ZERO);
    throttle.set_communication_mode(CommunicationMode::Local);
    throttle
        .enqueue_tx(b"STALE".to_vec())
        .expect("queue has space");
    throttle.set_communication_mode(CommunicationMode::Local);
    assert_eq!(throttle.poll_loopback(Duration::ZERO), ThrottleStep::Idle);
}

#[test]
fn bounded_queues_report_backpressure_in_chunks_without_reordering() {
    let mut throttle = DataThrottle::new(
        ThrottleConfig {
            tx_queue_capacity: 2,
            rx_queue_capacity: 1,
            ..ThrottleConfig::default()
        },
        Duration::ZERO,
    );
    assert_eq!(throttle.enqueue_tx(vec![1]), Ok(EnqueueOutcome::Queued));
    assert_eq!(throttle.enqueue_tx(vec![2, 3]), Ok(EnqueueOutcome::Queued));
    let error = throttle
        .enqueue_tx(vec![4])
        .expect_err("third chunk must backpressure");
    assert_eq!(error.backpressure.flow, DataFlow::Tx);
    assert_eq!(error.backpressure.capacity_chunks, 2);
    assert_eq!(error.data, vec![4]);

    throttle.set_throttle_mode(ThrottleMode::Unthrottled);
    assert_eq!(
        emitted_data(throttle.poll_tx(Duration::ZERO)),
        Some(vec![1])
    );
    assert_eq!(
        emitted_data(throttle.poll_tx(Duration::ZERO)),
        Some(vec![2, 3])
    );
}

#[test]
fn application_and_transport_contracts_drive_policy_without_callbacks() {
    let mut throttle = DataThrottle::new(ThrottleConfig::default(), Duration::ZERO);
    assert_eq!(
        throttle.handle_application_command(ApplicationCommand::SetThrottleMode(
            ThrottleMode::Unthrottled,
        )),
        Ok(EnqueueOutcome::Applied)
    );
    assert_eq!(
        throttle.handle_application_command(ApplicationCommand::Transmit(vec![0x41])),
        Ok(EnqueueOutcome::Queued)
    );
    assert_eq!(
        throttle.poll_tx(Duration::ZERO),
        ThrottleStep::Output {
            flow: DataFlow::Tx,
            output: ThrottleOutput::ToTransport(TransportCommand::Send(vec![0x41]))
        }
    );
}
