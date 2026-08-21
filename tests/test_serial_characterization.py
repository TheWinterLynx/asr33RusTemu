"""Characterization tests for the legacy pyserial adapter without hardware."""

import queue
import unittest
from contextlib import ExitStack
from unittest.mock import patch

from asr33_backend_serial import SerialBackend
from tests.helpers import ConfigStub, DataSink


# Public pyserial values used by the real backend. The environment may contain
# the unrelated `serial` package rather than pyserial, so hardware-independent
# characterization injects the constructor and uses the documented values.
FIVEBITS = 5
SIXBITS = 6
SEVENBITS = 7
EIGHTBITS = 8
PARITY_NONE = "N"
PARITY_EVEN = "E"
PARITY_ODD = "O"
PARITY_MARK = "M"
PARITY_SPACE = "S"
STOPBITS_ONE = 1
STOPBITS_ONE_POINT_FIVE = 1.5
STOPBITS_TWO = 2


class _FakeThread:
    instances = []

    def __init__(self, target, daemon):
        self.target = target
        self.daemon = daemon
        self.started = False
        self.joined = False
        self.instances.append(self)

    def start(self):
        self.started = True

    def join(self):
        self.joined = True


class _FakeSerial:
    def __init__(self, **settings):
        self.settings = settings
        self.port = settings["port"]
        self.baudrate = settings["baudrate"]
        self.bytesize = settings["bytesize"]
        self.parity = settings["parity"]
        self.stopbits = settings["stopbits"]
        self.is_open = True
        self.buffer_sizes = []
        self.rx = bytearray()
        self.reads = []
        self.writes = []
        self.read_error = None
        self.write_error = None

    @property
    def in_waiting(self):
        return len(self.rx)

    def set_buffer_size(self, **sizes):
        self.buffer_sizes.append(sizes)

    def read(self, size):
        if self.read_error is not None:
            raise self.read_error
        data = bytes(self.rx[:size])
        del self.rx[:size]
        self.reads.append(size)
        return data

    def write(self, data):
        if self.write_error is not None:
            raise self.write_error
        self.writes.append(data)
        return len(data)

    def close(self):
        self.is_open = False


class SerialBackendCharacterizationTests(unittest.TestCase):
    def setUp(self):
        _FakeThread.instances = []

    def make_backend(self, values=None, send_queue_size=8):
        created = []

        def open_serial(**settings):
            port = _FakeSerial(**settings)
            created.append(port)
            return port

        with ExitStack() as stack:
            stack.enter_context(
                patch.multiple(
                    "asr33_backend_serial.serial",
                    EIGHTBITS=EIGHTBITS,
                    PARITY_NONE=PARITY_NONE,
                    STOPBITS_ONE=STOPBITS_ONE,
                    create=True,
                )
            )
            stack.enter_context(
                patch(
                "asr33_backend_serial.serial.Serial",
                side_effect=open_serial,
                create=True,
                )
            )
            stack.enter_context(
                patch("asr33_backend_serial.threading.Thread", _FakeThread)
            )
            backend = SerialBackend(
                DataSink(), ConfigStub(values), send_queue_size=send_queue_size
            )
        return backend, created[0]

    def test_open_uses_defaults_and_starts_two_daemon_workers(self):
        backend, port = self.make_backend()
        self.assertEqual(
            port.settings,
            {
                "port": "COM4",
                "baudrate": 9600,
                "bytesize": EIGHTBITS,
                "parity": PARITY_NONE,
                "stopbits": STOPBITS_ONE,
                "timeout": 0,
            },
        )
        self.assertEqual(len(_FakeThread.instances), 2)
        self.assertTrue(all(thread.daemon for thread in _FakeThread.instances))
        self.assertTrue(all(thread.started for thread in _FakeThread.instances))
        backend.close()

    def test_open_passes_all_supported_line_settings_unchanged(self):
        databits = [FIVEBITS, SIXBITS, SEVENBITS, EIGHTBITS]
        parities = [
            PARITY_NONE,
            PARITY_EVEN,
            PARITY_ODD,
            PARITY_MARK,
            PARITY_SPACE,
        ]
        stopbits = [STOPBITS_ONE, STOPBITS_ONE_POINT_FIVE, STOPBITS_TWO]
        for data_bits in databits:
            for parity in parities:
                for stop_bits in stopbits:
                    with self.subTest(data_bits=data_bits, parity=parity, stop_bits=stop_bits):
                        backend, port = self.make_backend(
                            {
                                "port": "TEST",
                                "baudrate": 110,
                                "databits": data_bits,
                                "parity": parity,
                                "stopbits": stop_bits,
                            }
                        )
                        self.assertEqual(port.bytesize, data_bits)
                        self.assertEqual(port.parity, parity)
                        self.assertEqual(port.stopbits, stop_bits)
                        self.assertEqual(port.baudrate, 110)
                        backend.close()

    def test_windows_sets_driver_buffer_sizes_but_other_platforms_do_not(self):
        with patch("asr33_backend_serial.sys.platform", "win32"):
            windows_backend, windows_port = self.make_backend()
        self.assertEqual(windows_port.buffer_sizes, [{"rx_size": 8, "tx_size": 4096}])
        windows_backend.close()

        with patch("asr33_backend_serial.sys.platform", "linux"):
            linux_backend, linux_port = self.make_backend()
        self.assertEqual(linux_port.buffer_sizes, [])
        linux_backend.close()

    def test_rx_delivers_available_bytes_as_one_ordered_chunk(self):
        backend, port = self.make_backend()
        port.rx.extend(b"ABC")

        def stop_after_delivery(_duration):
            backend._running = False

        with patch("asr33_backend_serial.time.sleep", side_effect=stop_after_delivery):
            backend._serial_rx_worker()
        self.assertEqual(port.reads, [3])
        self.assertEqual(backend.upper_layer.received, [b"ABC"])
        backend.close()

    def test_tx_preserves_chunk_and_queue_order(self):
        backend, port = self.make_backend()
        backend.send_data(b"AB")
        backend.send_data(b"CD")

        def stop_after_second_write(_duration):
            if len(port.writes) == 2:
                backend._running = False

        with patch("asr33_backend_serial.time.sleep", side_effect=stop_after_second_write):
            backend._serial_tx_worker()
        self.assertEqual(port.writes, [b"AB", b"CD"])
        backend.close()

    def test_empty_tx_is_ignored_and_capacity_is_measured_in_chunks(self):
        backend, _ = self.make_backend(send_queue_size=2)
        backend.send_data(b"")
        backend.send_data(b"A")
        backend.send_data(b"BC")
        self.assertTrue(backend._send_queue.full())
        with self.assertRaises(queue.Full):
            backend._send_queue.put_nowait(b"overflow")
        backend.close()

    def test_open_error_propagates_before_workers_exist(self):
        with (
            patch.multiple(
                "asr33_backend_serial.serial",
                EIGHTBITS=EIGHTBITS,
                PARITY_NONE=PARITY_NONE,
                STOPBITS_ONE=STOPBITS_ONE,
                create=True,
            ),
            patch(
                "asr33_backend_serial.serial.Serial",
                side_effect=OSError("cannot open"),
                create=True,
            ),
            patch("asr33_backend_serial.threading.Thread", _FakeThread),
        ):
            with self.assertRaisesRegex(OSError, "cannot open"):
                SerialBackend(DataSink(), ConfigStub())
        self.assertEqual(_FakeThread.instances, [])

    def test_read_and_write_errors_escape_the_worker(self):
        rx_backend, rx_port = self.make_backend()
        rx_port.rx.extend(b"X")
        rx_port.read_error = OSError("read failed")
        with self.assertRaisesRegex(OSError, "read failed"):
            rx_backend._serial_rx_worker()
        rx_backend.close()

        tx_backend, tx_port = self.make_backend()
        tx_port.write_error = OSError("write failed")
        tx_backend.send_data(b"X")
        with self.assertRaisesRegex(OSError, "write failed"):
            tx_backend._serial_tx_worker()
        tx_backend.close()

    def test_close_joins_both_workers_then_closes_the_port(self):
        backend, port = self.make_backend()
        backend.close()
        self.assertFalse(backend._running)
        self.assertTrue(all(thread.joined for thread in _FakeThread.instances))
        self.assertFalse(port.is_open)

    def test_info_string_uses_pyserial_rendering(self):
        backend, _ = self.make_backend(
            {
                "port": "COM9",
                "baudrate": 110,
                "databits": SEVENBITS,
                "parity": PARITY_EVEN,
                "stopbits": STOPBITS_TWO,
            }
        )
        self.assertEqual(backend.get_info_string(), "COM9:110-7E2")
        backend.close()


if __name__ == "__main__":
    unittest.main()
