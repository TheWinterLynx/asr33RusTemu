#!/usr/bin/env python3

"""
ASR-33 Emulator Wrapper with YAML config.

Usage examples:
    python asr33emu.py --config config.yaml
    python asr33emu.py --frontend tkinter --baudrate 110
"""

from asr33_config import ASR33Config
from asr33_backend_serial import SerialBackend
from asr33_shim_throttle import DataThrottle
from asr33_terminal import Terminal
from asr33_sounds_sm import ASR33AudioModule as ASR33_Sounds
from asr33_frontend_pygame import ASR33PygameFrontend as PygameFrontend
from asr33_frontend_tk import ASR33TkFrontend as TkFrontend


DEFAULT_FRONTEND = "tkinter"


class EmulatorWrapper:
    """ASR-33 Emulator Wrapper Class"""

    def __init__(self):
        # Initialize selections as None placeholders first.
        self.comm_backend = None
        self.data_throttle = None
        self.term = None
        self.frontend = None

        # Load config if provided
        config = ASR33Config(description="ASR-33 Teletype Emulator")
        cfg_data = None
        try:
            if config is not None:
                cfg_data = config.get_merged_config()
        except FileNotFoundError:
            pass

        if cfg_data is None:
            raise RuntimeError("No configuration file found.")

        # Extract relevant config sections
        try:
            frontend_cfg = cfg_data.frontend
            backend_cfg = cfg_data.backend
            data_throttle_cfg = cfg_data.data_throttle
            terminal_cfg = cfg_data.terminal
        except AttributeError as e:
            raise RuntimeError(f"Missing configuration section: {e}") from e

        # Serial transport
        cfg = backend_cfg.serial_config
        self.comm_backend = SerialBackend(
            upper_layer=None,  # Forward reference set later
            config=cfg,
        )

        # Comm backend feeds data to DataThrottle, which feeds data to Terminal
        # Get DataThrottle config from YAML file or use defaults
        cfg = data_throttle_cfg.config
        self.data_throttle = DataThrottle(
            lower_layer=self.comm_backend,
            upper_layer=None,  # Forward reference set later
            config=cfg,
        )

        # Terminal
        # Get Terminal config from YAML file or use defaults
        cfg = terminal_cfg.config
        self.term = Terminal(
            comm_interface=self.data_throttle,
            frontend=None,  # Forward reference set later
            config=cfg,
        )

        # ASR-33 sound support
        self.sound = ASR33_Sounds()

        # Frontend
        frontend_type = frontend_cfg.get("type", default=DEFAULT_FRONTEND)
        if frontend_type == "pygame":
            self.frontend = PygameFrontend(
                terminal=self.term,
                backend=self.data_throttle,
                config=cfg_data,
                sound=self.sound,
            )
        elif frontend_type == "tkinter":
            self.frontend = TkFrontend(
                terminal=self.term,
                backend=self.data_throttle,
                config=cfg_data,
                sound=self.sound,
            )
        else:
            raise ValueError(f"Unsupported frontend: {frontend_type}")

        # Assign layers that were forward referenced earlier
        if self.comm_backend is not None:
            self.comm_backend.upper_layer = self.data_throttle
        self.data_throttle.upper_layer = self.term
        self.term.frontend = self.frontend

    def run(self):
        """Run the emulator main loop."""
        if self.frontend is None:
            raise RuntimeError("Frontend not initialized")
        self.frontend.run()


if __name__ == "__main__":
    EmulatorWrapper().run()
