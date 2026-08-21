"""Shared 7-bit printable-character vectors for Python and Rust."""

import csv
from pathlib import Path
import unittest


FIXTURE = Path(__file__).parent / "fixtures" / "ascii_7bit_printable.csv"


class AsciiPrintableCharacterizationTests(unittest.TestCase):
    def test_fixture_exhaustively_matches_python_isprintable(self):
        with FIXTURE.open(newline="", encoding="ascii") as source:
            rows = list(csv.DictReader(source))

        self.assertEqual([int(row["byte"]) for row in rows], list(range(128)))
        for row in rows:
            byte = int(row["byte"])
            expected = row["printable"] == "true"
            self.assertEqual(chr(byte).isprintable(), expected, f"byte {byte:#04x}")


if __name__ == "__main__":
    unittest.main()
