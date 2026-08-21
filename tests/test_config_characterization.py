"""Characterization of YAML loading and command-line override semantics."""

import copy
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

from asr33_config import ASR33Config, ConfigNode


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent


class ConfigNodeCharacterizationTests(unittest.TestCase):
    def test_missing_attribute_raises_but_get_returns_default(self):
        node = ConfigNode({"terminal": {"rows": 24}})
        self.assertEqual(node.terminal.rows, 24)
        self.assertEqual(node.get("terminal", "missing", default=7), 7)
        with self.assertRaisesRegex(AttributeError, "No such config key: missing"):
            _ = node.missing


class ConfigMergeCharacterizationTests(unittest.TestCase):
    def make_loader_without_parsing_constructor(self, arguments):
        with patch.object(sys, "argv", ["asr33emu.py", *arguments]):
            loader = ASR33Config.__new__(ASR33Config)
            loader.args = loader.parse_args("test")
        return loader

    def base_config(self):
        return {
            "frontend": {"type": "tkinter"},
            "backend": {"type": "serial", "serial_config": {
                "port": "COM4", "baudrate": 110, "databits": 8,
                "parity": "N", "stopbits": 1,
            }},
            "terminal": {"config": {"mode": "line", "columns": 72, "rows": 24, "scrollback": 200}},
            "data_throttle": {"config": {"send_rate_cps": 10, "receive_rate_cps": 10}},
            "sound": {"config": {"mute_state": "unmuted"}},
        }

    def test_cli_overrides_both_throttle_directions_and_serial_alias(self):
        raw = self.base_config()
        loader = self.make_loader_without_parsing_constructor([
            "--frontend", "pygame", "--term_mode", "local",
            "--throttle_rate", "37", "--baudrate", "19200", "--mute",
        ])
        merged = loader.merge_with_args(raw)
        self.assertEqual(merged["frontend"]["type"], "pygame")
        self.assertEqual(merged["terminal"]["config"]["mode"], "local")
        self.assertEqual(merged["data_throttle"]["config"]["send_rate_cps"], 37)
        self.assertEqual(merged["data_throttle"]["config"]["receive_rate_cps"], 37)
        self.assertEqual(merged["backend"]["serial_config"]["baudrate"], 19200)
        self.assertEqual(merged["sound"]["config"]["mute_state"], "muted")

    def test_merge_mutates_nested_values_in_the_raw_configuration(self):
        # legacy behavior: dict.copy() is shallow, so applying CLI overrides to
        # the merged tree also changes nested values in the parsed YAML tree.
        raw = self.base_config()
        original = copy.deepcopy(raw)
        loader = self.make_loader_without_parsing_constructor(["--columns", "80"])
        merged = loader.merge_with_args(raw)
        self.assertEqual(merged["terminal"]["config"]["columns"], 80)
        self.assertEqual(raw["terminal"]["config"]["columns"], 80)
        self.assertEqual(original["terminal"]["config"]["columns"], 72)


class ConfigEndToEndCharacterizationTests(unittest.TestCase):
    def load(self, filename, *arguments):
        path = REPOSITORY_ROOT / filename
        with patch.object(sys, "argv", ["asr33emu.py", "--config", str(path), *arguments]):
            return ASR33Config("characterization")

    def test_default_yaml_is_loaded_by_the_real_python_loader(self):
        config = self.load("asr33_config.yaml")
        raw = config.get_yaml_config()._data
        effective = config.get_merged_config()._data
        self.assertEqual(raw, effective)
        self.assertEqual(effective["frontend"]["type"], "tkinter")
        self.assertTrue(effective["terminal"]["config"]["autowrap"])
        self.assertEqual(effective["terminal"]["config"]["keyboard_parity_mode"], "space")
        self.assertEqual(effective["backend"]["serial_config"]["baudrate"], 19200)

    def test_strict_yaml_is_loaded_by_the_real_python_loader(self):
        config = self.load("asr33_strict.yaml")
        effective = config.get_merged_config()._data
        self.assertFalse(effective["terminal"]["config"]["autowrap"])
        self.assertTrue(effective["terminal"]["config"]["keyboard_uppercase_only"])
        self.assertEqual(effective["terminal"]["config"]["keyboard_parity_mode"], "mark")
        self.assertEqual(effective["backend"]["serial_config"]["baudrate"], 110)

    def test_real_yaml_receives_all_supported_cli_overrides(self):
        config = self.load(
            "asr33_config.yaml",
            "--frontend", "pygame",
            "--backend", "ssh",
            "--term_mode", "local",
            "--columns", "80",
            "--rows", "30",
            "--scrollback", "400",
            "--throttle_rate", "37",
            "--mute",
            "--baudrate", "9600",
            "--databits", "7",
            "--parity", "E",
            "--stopbits", "2",
        )
        effective = config.get_merged_config()._data
        self.assertEqual(effective["frontend"]["type"], "pygame")
        self.assertEqual(effective["backend"]["type"], "ssh")
        self.assertEqual(effective["terminal"]["config"]["mode"], "local")
        self.assertEqual(effective["terminal"]["config"]["columns"], 80)
        self.assertEqual(effective["terminal"]["config"]["rows"], 30)
        self.assertEqual(effective["terminal"]["config"]["scrollback"], 400)
        self.assertEqual(effective["data_throttle"]["config"]["send_rate_cps"], 37)
        self.assertEqual(effective["data_throttle"]["config"]["receive_rate_cps"], 37)
        self.assertEqual(effective["sound"]["config"]["mute_state"], "muted")
        self.assertEqual(effective["backend"]["serial_config"]["baudrate"], 9600)
        self.assertEqual(effective["backend"]["serial_config"]["databits"], 7)
        self.assertEqual(effective["backend"]["serial_config"]["parity"], "E")
        self.assertEqual(effective["backend"]["serial_config"]["stopbits"], 2)


if __name__ == "__main__":
    unittest.main()
