import unittest
from types import SimpleNamespace

from asr33_frontend_tk import ASR33TkFrontend
from asr33_terminal import Terminal


class _Backend:
    def __init__(self):
        self.sent = []

    def send_data(self, data):
        self.sent.append(data)


def _frontend(*, uppercase=False, parity="space"):
    frontend = ASR33TkFrontend.__new__(ASR33TkFrontend)
    frontend.keyboard_uppercase_only = uppercase
    frontend.keyboard_parity_mode = parity
    frontend._term = Terminal.__new__(Terminal)
    frontend._backend = _Backend()
    frontend._sounds = None
    frontend.screen_top_lln = 123
    return frontend


class KeyboardCharacterizationTests(unittest.TestCase):
    def assert_key(self, character, expected, *, uppercase=False, parity="space"):
        frontend = _frontend(uppercase=uppercase, parity=parity)
        result = frontend._keypress(SimpleNamespace(char=character))
        self.assertEqual(frontend._backend.sent, [expected])
        self.assertEqual(frontend.screen_top_lln, None)
        self.assertEqual(result, "break")

    def test_printable_ascii_and_lowercase_are_transmitted(self):
        self.assert_key("a", b"a")
        self.assert_key("~", b"~")

    def test_uppercase_only_uses_python_upper_before_ascii_encoding(self):
        self.assert_key("a", b"A", uppercase=True)
        # legacy behavior: a multi-character uppercase expansion is reduced
        # to its first encoded byte by parity handling.
        self.assert_key("ß", b"S", uppercase=True)

    def test_control_characters_with_event_text_are_transmitted(self):
        self.assert_key("\r", b"\r")
        self.assert_key("\x08", b"\x08")
        self.assert_key("\t", b"\t")
        self.assert_key("\x03", b"\x03")

    def test_keyboard_parity_modes(self):
        self.assert_key("A", b"\xc1", parity="mark")
        self.assert_key("A", b"A", parity="space")
        self.assert_key("A", b"A", parity="even")
        self.assert_key("C", b"\xc3", parity="even")

    def test_empty_event_text_is_ignored(self):
        frontend = _frontend()
        self.assertIsNone(frontend._keypress(SimpleNamespace(char="")))
        self.assertEqual(frontend._backend.sent, [])
        self.assertEqual(frontend.screen_top_lln, 123)

    def test_non_ascii_character_raises_in_legacy_frontend(self):
        with self.assertRaises(UnicodeEncodeError):
            _frontend()._keypress(SimpleNamespace(char="é"))


if __name__ == "__main__":
    unittest.main()
