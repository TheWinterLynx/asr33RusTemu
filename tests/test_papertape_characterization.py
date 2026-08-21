"""Characterization of binary paper-tape rules without creating Tk widgets."""

import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from asr33_papertape import PapertapePunch, PapertapeReader


FIXTURE = Path(__file__).parent / "fixtures" / "paper_tape_leader_data_trailers.bin"


class ViewerStub:
    def __init__(self, autostop=True):
        self.autostop = autostop
        self.bytes_added = []
        self.unload_count = 0

    def add_byte(self, data):
        self.bytes_added.append(data)

    def set_to_off_state(self):
        pass

    def unload_tape(self):
        self.unload_count += 1

    def set_file_status(self, _line_one, _line_two=""):
        pass


class BackendStub:
    def __init__(self):
        self.sent = []

    def send_data(self, data):
        self.sent.append(data)


class PaperTapeReaderCharacterizationTests(unittest.TestCase):
    def make_reader(self):
        reader = PapertapeReader.__new__(PapertapeReader)
        reader.tape_loaded = False
        reader.tape_data = b""
        reader.position = 0
        reader.active = False
        reader.stop_cause = ""
        reader.trailing_o000_idx = None
        reader.trailing_o200_idx = None
        reader.papertape_viewer = ViewerStub()
        reader.backend = BackendStub()
        reader.thread_running = False
        reader.skip_leading_nulls = True
        reader.set_msb = False
        return reader

    def load_fixture(self, reader):
        reader.tape_data = FIXTURE.read_bytes()
        reader.tape_loaded = True
        reader.position = 0
        n = len(reader.tape_data)
        i = n - 1
        while i >= 0 and reader.tape_data[i] == 0o000:
            i -= 1
        reader.trailing_o000_idx = i + 1 if i < n - 1 else None
        j = i
        while j >= 0 and reader.tape_data[j] == 0o200:
            j -= 1
        reader.trailing_o200_idx = j + 1 if j < i else None

    def worker_step(self, reader):
        reader.thread_running = True

        def finish_iteration(_duration):
            reader.thread_running = False

        with patch("asr33_papertape.time.sleep", side_effect=finish_iteration):
            reader._tape_reader_worker()

    def test_loading_identifies_adjacent_0200_and_0000_trailers(self):
        reader = self.make_reader()
        with tempfile.NamedTemporaryFile(delete=False) as tape:
            tape.write(b"A\x80\x80\x00\x00")
            name = tape.name
        try:
            reader._load_tapefile(name)
        finally:
            os.unlink(name)
        self.assertEqual(reader.trailing_o200_idx, 1)
        self.assertEqual(reader.trailing_o000_idx, 3)
        self.assertFalse(reader._end_check(1))
        self.assertTrue(reader._end_check(2))
        self.assertEqual(reader.stop_cause, "trailing_o200")

    def test_zero_trailer_is_used_when_no_0200_trailer_exists(self):
        reader = self.make_reader()
        reader.tape_data = b"A\x00\x00"
        reader.trailing_o000_idx = 1
        self.assertFalse(reader._end_check(1))
        self.assertTrue(reader._end_check(2))
        self.assertEqual(reader.stop_cause, "trailing_o000")

    def test_disabled_autostop_only_stops_at_physical_end(self):
        reader = self.make_reader()
        reader.tape_data = b"A\x80"
        reader.trailing_o200_idx = 1
        reader.papertape_viewer.autostop = False
        self.assertFalse(reader._end_check(1))
        self.assertTrue(reader._end_check(2))
        self.assertEqual(reader.stop_cause, "end_of_tape")

    def test_on_skips_all_leading_nulls(self):
        reader = self.make_reader()
        reader.tape_loaded = True
        reader.tape_data = b"\x00\x00A"
        reader.skip_leading_nulls = True
        self.assertTrue(reader.on())
        self.assertEqual(reader.position, 2)
        self.assertTrue(reader.active)

    def test_shared_fixture_positions_and_autostop_are_stepwise_legacy_behavior(self):
        reader = self.make_reader()
        self.load_fixture(reader)
        self.assertEqual(reader.tape_data, b"\x00\x00AB\x80\x80\x00\x00")
        self.assertEqual(reader.trailing_o200_idx, 4)
        self.assertEqual(reader.trailing_o000_idx, 6)

        self.assertTrue(reader.on())
        self.assertEqual(reader.position, 2)

        self.worker_step(reader)
        self.assertEqual((reader.position, reader.active), (3, True))
        self.assertEqual(reader.backend.sent, [b"A"])

        self.worker_step(reader)
        self.assertEqual((reader.position, reader.active), (4, True))
        self.assertEqual(reader.backend.sent, [b"A", b"B"])

        # legacy behavior: the first 0200 trailer byte is emitted because the
        # end check uses position > trailer_start rather than >=.
        self.worker_step(reader)
        self.assertEqual((reader.position, reader.active), (5, True))
        self.assertEqual(reader.backend.sent, [b"A", b"B", b"\x80"])

        self.worker_step(reader)
        self.assertEqual((reader.position, reader.active), (5, False))
        self.assertEqual(reader.stop_cause, "trailing_o200")

    def test_shared_fixture_msb_option_transforms_each_emitted_byte(self):
        reader = self.make_reader()
        self.load_fixture(reader)
        reader.set_msb = True
        reader.on()
        self.worker_step(reader)
        self.worker_step(reader)
        self.assertEqual(reader.backend.sent, [b"\xc1", b"\xc2"])

    def test_rewind_only_changes_a_loaded_stopped_reader(self):
        reader = self.make_reader()
        self.load_fixture(reader)
        reader.position = 4
        reader.active = True
        reader.rewind_tape()
        self.assertEqual(reader.position, 4)
        reader.active = False
        reader.rewind_tape()
        self.assertEqual(reader.position, 0)
        self.assertEqual(reader.papertape_viewer.unload_count, 1)


class PaperTapePunchCharacterizationTests(unittest.TestCase):
    def test_append_preview_reads_existing_bytes_and_appends_new_bytes(self):
        punch = PapertapePunch.__new__(PapertapePunch)
        viewer = ViewerStub()
        with tempfile.NamedTemporaryFile(delete=False) as tape:
            tape.write(b"OLD")
            name = tape.name
        try:
            handle = punch._open_for_append_with_preview(name, viewer)
            try:
                handle.write(b"NEW")
            finally:
                handle.close()
            with open(name, "rb") as result:
                self.assertEqual(result.read(), b"OLDNEW")
        finally:
            os.unlink(name)
        self.assertEqual(viewer.bytes_added, [b"OLD"])

    def test_overwrite_mode_truncates_on_load_and_only_punches_while_active(self):
        punch = PapertapePunch.__new__(PapertapePunch)
        punch.master = None
        punch.tape_loaded = False
        punch.active = False
        punch.tape_file = None
        punch.pt_name_path = None
        punch.init_name_path = None
        punch.file_write_mode = "overwrite"
        punch.papertape_viewer = ViewerStub()

        with tempfile.NamedTemporaryFile(delete=False) as tape:
            tape.write(b"OLD")
            name = tape.name
        try:
            with patch("asr33_papertape.get_reader_file_selection", return_value=name):
                self.assertEqual(punch.load_tape(), "loaded")
            self.assertFalse(punch.active)
            punch.punch_bytes(b"IGNORED")
            self.assertTrue(punch.on())
            punch.punch_bytes(b"NEW")
            punch.tape_file.close()
            punch.tape_file = None
            with open(name, "rb") as result:
                self.assertEqual(result.read(), b"NEW")
        finally:
            os.unlink(name)


if __name__ == "__main__":
    unittest.main()
