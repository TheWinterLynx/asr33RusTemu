"""Executable specification of the current Python terminal behaviour."""

import unittest

from asr33_terminal import EscapeShim, Terminal
from tests.helpers import ConfigStub, DataSink


def terminal_config(**overrides):
    values = {
        "columns": 8,
        "rows": 2,
        "scrollback": 2,
        "autowrap": False,
    }
    values.update(overrides)
    return ConfigStub(values)


def top_chars(line):
    return "".join(
        stack[-1] if stack else " "
        for stack in (line.get_strike_stack(i) for i in range(line.width))
    )


class ParityCharacterizationTests(unittest.TestCase):
    def setUp(self):
        self.terminal = Terminal(None, None, terminal_config())

    def test_even_parity_sets_msb_only_for_odd_population(self):
        self.assertEqual(self.terminal.encode_even_parity(b"AC\x7f"), b"A\xc3\xff")

    def test_mask_parity_always_clears_the_msb(self):
        self.assertEqual(self.terminal.mask_parity_bit(b"\x00\x7f\x80\xff"), b"\x00\x7f\x00\x7f")


class EscapeShimCharacterizationTests(unittest.TestCase):
    def test_strips_csi_and_both_osc_terminators_across_chunks(self):
        shim = EscapeShim()
        parts = [
            shim.feed("A\x1b[31"),
            shim.feed("mB\x1b]title"),
            shim.feed("\x07C\x1b]other\x1b"),
            shim.feed("\\D"),
        ]
        self.assertEqual("".join(parts), "ABCD")

    def test_swallows_unknown_single_character_escape_sequence(self):
        self.assertEqual(EscapeShim().feed("A\x1b7B"), "AB")


class TerminalCharacterizationTests(unittest.TestCase):
    def test_control_characters_follow_current_cursor_rules(self):
        terminal = Terminal(None, None, terminal_config(columns=16))
        terminal.receive_data(b"AB\bX\tY\fZ\rQ\nR\vS")

        # legacy behavior: carriage return does not clear existing strikes,
        # and LF/VT advance a line without returning to column zero.
        self.assertEqual(top_chars(terminal.line_history.get_line(0)), "QX      Z       ")
        self.assertEqual(top_chars(terminal.line_history.get_line(1)), " R              ")
        self.assertEqual(top_chars(terminal.line_history.get_line(2)), "  S             ")
        self.assertEqual(terminal.get_cursor_position(), (3, 2))

    def test_form_feed_moves_left_instead_of_starting_a_new_page(self):
        # legacy behavior: form feed is implemented as a one-column backspace.
        terminal = Terminal(None, None, terminal_config())
        terminal.receive_data(b"AB\fX")
        self.assertEqual(terminal.line_history.get_line(0).get_strike_stack(1), ["B", "X"])

    def test_disabled_autowrap_overstrikes_the_last_column(self):
        # legacy behavior: once the cursor reaches the edge it is clamped, so
        # subsequent printable bytes accumulate as overstrikes in the last cell.
        terminal = Terminal(None, None, terminal_config(columns=4, autowrap=False))
        terminal.receive_data(b"ABCDE")
        self.assertEqual(top_chars(terminal.line_history.get_line(0)), "ABCE")
        self.assertEqual(terminal.line_history.get_line(0).get_strike_stack(3), ["D", "E"])
        self.assertEqual(terminal.get_cursor_position(), (3, 0))

    def test_enabled_autowrap_starts_a_new_logical_line(self):
        terminal = Terminal(None, None, terminal_config(columns=4, autowrap=True))
        terminal.receive_data(b"ABCDE")
        self.assertEqual(top_chars(terminal.line_history.get_line(0)), "ABCD")
        self.assertEqual(top_chars(terminal.line_history.get_line(1)), "E   ")
        self.assertEqual(terminal.get_cursor_position(), (1, 1))

    def test_scrollback_discards_old_lines_but_retains_logical_numbers(self):
        terminal = Terminal(None, None, terminal_config(rows=2, scrollback=1))
        terminal.receive_data(b"0\n1\n2\n3")
        self.assertEqual(len(terminal.line_history), 3)
        self.assertEqual(terminal.line_history.top_lln(), 1)
        self.assertEqual(terminal.line_history.bottom_lln(), 3)
        # legacy behavior: LF preserves the current column, producing the
        # diagonal placement below when the input contains no CR bytes.
        self.assertEqual(
            [top_chars(terminal.line_history.get_line(i)) for i in range(3)],
            [" 1      ", "  2     ", "   3    "],
        )

    def test_remote_msb_is_masked_for_display_but_original_bytes_reach_frontend(self):
        frontend = DataSink()
        terminal = Terminal(None, frontend, terminal_config())
        terminal.receive_data(b"\xc1")
        self.assertEqual(top_chars(terminal.line_history.get_line(0))[0], "A")
        self.assertEqual(frontend.received, [b"\xc1"])

    def test_printer_off_still_forwards_bytes_but_does_not_queue_sound(self):
        frontend = DataSink()
        terminal = Terminal(None, frontend, terminal_config())
        terminal.disable_printing()
        terminal.receive_data(b"A")
        self.assertEqual(top_chars(terminal.line_history.get_line(0)), " " * 8)
        self.assertEqual(terminal.sound_queue_len(), 0)
        self.assertEqual(frontend.received, [b"A"])

    def test_character_queue_includes_controls_with_post_processing_column(self):
        terminal = Terminal(None, None, terminal_config())
        terminal.receive_data(b"A\r\n\x01")
        events = []
        while terminal.sound_queue_len() > 0:
            events.append(terminal.pop_char_from_sound_queue())
        self.assertEqual(events, [("A", 1), ("\r", 0), ("\n", 0), ("\x01", 0)])


if __name__ == "__main__":
    unittest.main()
