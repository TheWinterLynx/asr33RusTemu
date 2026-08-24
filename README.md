**ASR-33 terminal with paper tape reader/punch emulator.**

![screenshot](screenshot.png)

**Features:**

* Refactored and expanded (with the help of AI) version of Hugh Pyle's ttyemu project.
* Rust/egui target with serial transport.
* Legacy Python implementation supports Pygame and Tkinter frontends while it remains as the behavioural reference during migration.
* F1/F2 displays/hides the paper tape reader widget.
* F3/F4 displays/hides the paper tape punch.
* By default, output is limited to an authentic 10 characters per second. Hit F5 to unthrottle the speed.
* Hit F6 to mute the sound.
* Sound is generated using Hugh Pyle's ASR-33 sound recording. Hit F7 to close the lid.
* Hit F8 to switch between Line and Local modes.
* Hit F9 to turn the printer output on and off.
* Scrolling with page up/down, home/end and mouse wheel.
* YAML configuration plus command-line overrides.
* Teletype33.ttf font, with optional custom TTF/OTF loading in the Rust target.
* The Rust application starts disconnected; a configured serial port is opened only after an explicit Connect/Reconnect action.

***

**Rust build and run**

From the project directory:

```text
cargo run --release
```

The Rust target uses the serial configuration in `asr33_config.yaml`. Use the Settings screen to select the COM/tty port and serial parameters, then connect explicitly.

Recommended validation commands:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

***

**Legacy Python installation notes**

The Python implementation remains in the repository while migration and differential testing are completed.

**Windows:**
Install Python 3 from the Microsoft Store.
(Tested using Python 3.13 on Windows 11 Home, version 25H2.)

Python package installation:

* `python3 -m pip install PyYAML`
* `python3 -m pip install pyserial`
* `python3 -m pip install pygame-ce`
* `python3 -m pip install pillow`
* `python3 -m pip install fonttools`

**Ubuntu** (tested on 24.04.3):

* `sudo apt install python3-tk`
* `sudo apt install python3-pygame`
* `sudo apt install python3-pil.imagetk`
* `sudo apt install python3-fonttools`

Using serial ports in Ubuntu requires adding yourself to the `dialout` group:

```text
sudo usermod -a -G dialout $USER
```

Then reboot.

**Running the legacy Python emulator**

From the project directory:

```text
python3 ./asr33emu.py
```

This starts the emulator using `asr33_config.yaml`. Some configuration values can be overridden on the command line, or an alternate configuration file can be selected with `--config filename.yaml`. Run `python3 ./asr33emu.py --help` for the supported options.

***

**PiDP8/I usage notes**

For the paper tape reader to load binary tapes, SIMH 8-bit terminal mode is required. In your boot script, ensure the SIMH emulator Terminal Input (TTI) device is set to operate in 8-bit mode by including:

```text
set tti 8b
```

Some PDP8 programs expect the keyboard to send mark-parity (bit 7 set to 1). This results in non-standard ASCII characters being sent but seems to be necessary for some OS8 programs. FOCAL-69 also requires keyboard mark-parity. An ASR-33 printer seems to ignore the parity bit. This feature can be enabled in `asr33_config.yaml` by setting `keyboard_parity_mode` to `mark`. Set it to `space` to generate standard ASCII characters.

For most PDP8 communications you will also want `keyboard_uppercase_only` set to `true`.

It is convenient to load both the RIM and Binary loaders in the same PiDP8 startup configuration file. For a more realistic experience, use the front panel to load the RIM loader, the simulated paper tape reader to load the binary loader, and the front panel to start it running.

The self-starting EDU20C BASIC paper tape loads and runs using the emulator. It requires only the RIM loader and, at 10 characters per second, takes about 25 minutes to load and start. Turning off the data-rate throttle loads it much faster. ASR-33 teletypes connected to PDP8 systems typically had a hardware tape-reader auto-start/stop feature. The emulator includes trailer detection so the reader can stop before trailing bytes are fed into a program's startup dialogue.
