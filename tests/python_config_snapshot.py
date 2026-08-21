"""Print the Python loader's effective configuration as canonical JSON."""

import json
from pathlib import Path
import sys


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPOSITORY_ROOT))

from asr33_config import ASR33Config  # noqa: E402


def main():
    config = ASR33Config("differential configuration snapshot")
    effective = config.get_merged_config()._data
    json.dump(effective, sys.stdout, sort_keys=True, separators=(",", ":"))


if __name__ == "__main__":
    main()
