"""Characterization of data routing that does not rely on wall-clock timing."""

import json
import queue
from pathlib import Path
import unittest
from unittest.mock import patch

from asr33_shim_throttle import DataThrottle
from tests.helpers import ConfigStub, DataSink, SendSink


class ThrottleCharacterizationTests(unittest.TestCase):
    def make_throttle(self):
        lower = SendSink()
        upper = DataSink()
        throttle = DataThrottle(
            lower_layer=lower,
            upper_layer=upper,
            config=ConfigStub({"send_rate_cps": 10, "receive_rate_cps": 10}),
        )
        return throttle, lower, upper

    def test_startup_cr_queued_before_line_mode_is_sent_to_backend(self):
        throttle, lower, _upper = self.make_throttle()
        throttle.send_data(b"\r")
        throttle.disable_loopback()
        throttle._process_queue_item(
            throttle._send_queue, 0, throttle._send_data_to_backend, 0.0
        )
        self.assertEqual(lower.sent, [b"\r"])

    def test_startup_cr_queued_before_local_mode_is_discarded_not_looped_back(self):
        throttle, lower, upper = self.make_throttle()
        throttle.send_data(b"\r")
        throttle.enable_loopback()
        throttle._process_queue_item(
            throttle._send_queue, 0, throttle._send_data_to_backend, 0.0
        )
        self.assertEqual(lower.sent, [])
        self.assertEqual(upper.received, [])
        self.assertTrue(throttle._loopback_queue.empty())

    def test_shared_timing_and_chunk_vectors(self):
        fixture_path = Path(__file__).parent / "fixtures" / "throttle_cases.json"
        cases = json.loads(fixture_path.read_text(encoding="utf-8"))

        for case in cases:
            with self.subTest(case=case["name"]):
                throttle, lower, upper = self.make_throttle()
                if not case["throttled"]:
                    throttle.disable_throttling()

                is_tx = case["direction"] == "tx"
                work_queue = throttle._send_queue if is_tx else throttle._receive_queue
                destination = (
                    throttle._send_data_to_backend
                    if is_tx
                    else throttle._send_data_to_upper_layer
                )
                emitted = lower.sent if is_tx else upper.received
                clock = _FakeClock()
                last_event = 0.0

                with (
                    patch("asr33_shim_throttle.time.monotonic", clock.monotonic),
                    patch("asr33_shim_throttle.time.sleep", clock.sleep),
                ):
                    for chunk_hex in case["chunks_hex"]:
                        work_queue.put(bytes.fromhex(chunk_hex))
                        last_event = throttle._process_queue_item(
                            work_queue,
                            case["rate_cps"],
                            destination,
                            last_event,
                        )

                self.assertEqual([item.hex() for item in emitted], case["emissions_hex"])
                self.assertEqual(
                    [round(delay * 1000) for delay in clock.sleeps],
                    case["waits_ms"],
                )

    def test_unthrottled_queue_processing_preserves_whole_chunk(self):
        throttle, lower, _ = self.make_throttle()
        throttle.disable_throttling()
        throttle._send_queue.put(b"ABC")
        throttle._process_queue_item(throttle._send_queue, 10, throttle._send_data_to_backend, 0.0)
        self.assertEqual(lower.sent, [b"ABC"])

    def test_throttled_processing_serializes_a_chunk_at_ten_cps(self):
        throttle, lower, _ = self.make_throttle()
        throttle._send_queue.put(b"AB")
        with (
            patch(
                "asr33_shim_throttle.time.monotonic",
                side_effect=[0.0, 0.1, 0.1, 0.2],
            ),
            patch("asr33_shim_throttle.time.sleep") as sleep,
        ):
            last_event = throttle._process_queue_item(
                throttle._send_queue,
                10,
                throttle._send_data_to_backend,
                0.0,
            )
        self.assertEqual(lower.sent, [b"A", b"B"])
        self.assertEqual(last_event, 0.2)
        self.assertEqual(sleep.call_count, 2)
        for call in sleep.call_args_list:
            self.assertAlmostEqual(call.args[0], 0.1)

    def test_throttle_skips_delays_at_or_below_perception_threshold(self):
        # legacy behavior: waits of 20 ms or less are omitted rather than
        # accumulated, so configured rates of 50 CPS or more can run faster.
        throttle, lower, _ = self.make_throttle()
        throttle._send_queue.put(b"AB")
        with (
            patch(
                "asr33_shim_throttle.time.monotonic",
                side_effect=[0.0, 0.0, 0.0, 0.0],
            ),
            patch("asr33_shim_throttle.time.sleep") as sleep,
        ):
            throttle._process_queue_item(
                throttle._send_queue,
                100,
                throttle._send_data_to_backend,
                0.0,
            )
        self.assertEqual(lower.sent, [b"A", b"B"])
        sleep.assert_not_called()

    def test_local_mode_routes_keyboard_data_to_loopback_queue(self):
        throttle, _, _ = self.make_throttle()
        throttle.enable_loopback()
        throttle.send_data(b"A")
        self.assertEqual(throttle._loopback_queue.get_nowait(), b"A")
        self.assertTrue(throttle._send_queue.empty())

    def test_local_mode_discards_remote_input(self):
        throttle, _, _ = self.make_throttle()
        throttle.enable_loopback()
        throttle.receive_data(b"REMOTE")
        self.assertTrue(throttle._receive_queue.empty())

    def test_line_mode_uses_separate_send_and_receive_queues(self):
        throttle, _, _ = self.make_throttle()
        throttle.send_data(b"TX")
        throttle.receive_data(b"RX")
        self.assertEqual(throttle._send_queue.get_nowait(), b"TX")
        self.assertEqual(throttle._receive_queue.get_nowait(), b"RX")

    def test_tx_and_rx_keep_independent_rates_and_timestamps(self):
        throttle, lower, upper = self.make_throttle()
        throttle._send_queue.put(b"T")
        throttle._receive_queue.put(b"R")
        clock = _FakeClock()
        with (
            patch("asr33_shim_throttle.time.monotonic", clock.monotonic),
            patch("asr33_shim_throttle.time.sleep", clock.sleep),
        ):
            tx_last = throttle._process_queue_item(
                throttle._send_queue, 10, throttle._send_data_to_backend, 0.0
            )
            rx_last = throttle._process_queue_item(
                throttle._receive_queue, 5, throttle._send_data_to_upper_layer, 0.0
            )
        self.assertEqual(lower.sent, [b"T"])
        self.assertEqual(upper.received, [b"R"])
        self.assertEqual(clock.sleeps, [0.1, 0.1])
        self.assertEqual(tx_last, 0.1)
        self.assertEqual(rx_last, 0.2)

    def test_disabling_throttle_mid_chunk_emits_remainder_whole(self):
        throttle, lower, _ = self.make_throttle()
        throttle._send_queue.put(b"ABC")
        clock = _FakeClock()

        def destination(data):
            lower.send_data(data)
            if data == b"A":
                throttle.disable_throttling()

        with (
            patch("asr33_shim_throttle.time.monotonic", clock.monotonic),
            patch("asr33_shim_throttle.time.sleep", clock.sleep),
        ):
            throttle._process_queue_item(throttle._send_queue, 10, destination, 0.0)
        self.assertEqual(lower.sent, [b"A", b"BC"])
        self.assertEqual(clock.sleeps, [0.1])

    def test_rate_change_mid_chunk_applies_to_next_chunk(self):
        throttle, lower, _ = self.make_throttle()
        throttle._send_queue.put(b"AB")
        clock = _FakeClock()

        def destination(data):
            lower.send_data(data)
            throttle.set_send_rate(5)

        with (
            patch("asr33_shim_throttle.time.monotonic", clock.monotonic),
            patch("asr33_shim_throttle.time.sleep", clock.sleep),
        ):
            throttle._process_queue_item(
                throttle._send_queue, throttle._send_rate, destination, 0.0
            )
        self.assertEqual(lower.sent, [b"A", b"B"])
        self.assertEqual(clock.sleeps, [0.1, 0.1])

    def test_loopback_delivery_and_mode_change_drop_stale_data(self):
        throttle, lower, upper = self.make_throttle()
        throttle.disable_throttling()
        throttle.enable_loopback()
        throttle.send_data(b"LOCAL")
        throttle._process_queue_item(
            throttle._loopback_queue,
            10,
            throttle._send_loopback_to_upper_layer,
            0.0,
        )
        self.assertEqual(upper.received, [b"LOCAL"])
        self.assertEqual(lower.sent, [])

        throttle.send_data(b"STALE")
        throttle.disable_loopback()
        throttle._process_queue_item(
            throttle._loopback_queue,
            10,
            throttle._send_loopback_to_upper_layer,
            0.0,
        )
        self.assertEqual(upper.received, [b"LOCAL"])
        self.assertTrue(throttle._loopback_queue.empty())

    def test_enabling_loopback_clears_stale_loopback_queue(self):
        throttle, _, _ = self.make_throttle()
        throttle.enable_loopback()
        throttle._loopback_queue.put(b"STALE")
        throttle.enable_loopback()
        self.assertTrue(throttle._loopback_queue.empty())

    def test_queue_capacity_exposes_chunk_backpressure(self):
        lower = SendSink()
        upper = DataSink()
        throttle = DataThrottle(
            lower_layer=lower,
            upper_layer=upper,
            config=ConfigStub({"send_rate_cps": 10, "receive_rate_cps": 10}),
            send_queue_size=2,
            receive_queue_size=3,
        )
        for item in (b"A", b"BC"):
            throttle._send_queue.put_nowait(item)
        for item in (b"D", b"EF", b"GHI"):
            throttle._receive_queue.put_nowait(item)
        for index in range(8):
            throttle._loopback_queue.put_nowait(bytes([index]))

        self.assertTrue(throttle._send_queue.full())
        self.assertTrue(throttle._receive_queue.full())
        self.assertTrue(throttle._loopback_queue.full())
        with self.assertRaises(queue.Full):
            throttle._send_queue.put_nowait(b"overflow")


class _FakeClock:
    def __init__(self):
        self.now = 0.0
        self.sleeps = []

    def monotonic(self):
        return self.now

    def sleep(self, duration):
        self.sleeps.append(duration)
        self.now += duration


if __name__ == "__main__":
    unittest.main()
