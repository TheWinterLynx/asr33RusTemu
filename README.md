# ASR-33 Teletype Emulator

ASR-33 terminal with paper tape reader/punch emulation, written in Rust with egui.

![screenshot](screenshot.png)

## Features

- Native Rust/egui application with serial transport.
- F1/F2 displays/hides the paper tape reader widget.
- F3/F4 displays/hides the paper tape punch.
- By default, output is limited to an authentic 10 characters per second. F5 toggles throttling.
- F6 mutes/unmutes sound.
- Sound uses Hugh Pyle's ASR-33 recordings. F7 opens/closes the lid sound profile.
- F8 switches between Line and Local modes.
- F9 turns printer output on/off.
- Scrolling with Page Up/Down, Home/End and mouse wheel.
- YAML configuration plus command-line overrides.
- Bundled Teletype33 font with optional custom TTF/OTF loading.
- The application starts disconnected; a configured serial port is opened only after an explicit Connect/Reconnect action.
- Windows release builds are single-file deployments: sounds, Teletype33 and the default YAML configuration are compiled into `asr33emu.exe`, and the MSVC CRT is statically linked.

## Build and run

From the project directory:

```text
cargo run --release
```

The application uses `asr33_config.yaml` when that file exists. If it is absent, the executable automatically uses its embedded default configuration, so `target/release/asr33emu.exe` can be copied by itself to an empty directory and started there.

Saving Settings creates or updates `asr33_config.yaml`. An explicit `--config filename.yaml` selects a user-supplied configuration file.

The bundled ASR-33 sound samples and Teletype33 font are read directly from the executable at runtime. A sibling `sounds` directory or font file is not required. Paper tapes and an explicitly selected custom `font_path` remain normal user files by design.

## Validation

Recommended local validation commands:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
```

A useful standalone deployment check on Windows is to copy only `target\release\asr33emu.exe` into an otherwise empty directory and launch it. The program should start with its embedded defaults, bundled font and bundled audio.

Configuration compatibility is protected by native Rust tests. `tests/config_compat.rs` contains the frozen final migration snapshot, so the test suite no longer requires an external reference runtime.

## PiDP8/I usage notes

For the paper tape reader to load binary tapes, SIMH 8-bit terminal mode is required. In your boot script, ensure the SIMH emulator Terminal Input (TTI) device is set to operate in 8-bit mode by including:

```text
set tti 8b
```

Some PDP8 programs expect the keyboard to send mark parity (bit 7 set to 1). This results in non-standard ASCII characters being sent but is necessary for some OS/8 programs. FOCAL-69 also requires keyboard mark parity. An ASR-33 printer appears to ignore the parity bit. This feature can be enabled in `asr33_config.yaml` by setting `keyboard_parity_mode` to `mark`. Set it to `space` to generate standard ASCII characters.

For most PDP8 communications you will also want `keyboard_uppercase_only` set to `true`.

It is convenient to load both the RIM and Binary loaders in the same PiDP8 startup configuration file. For a more realistic experience, use the front panel to load the RIM loader, the simulated paper tape reader to load the binary loader, and the front panel to start it running.

The self-starting EDU20C BASIC paper tape loads and runs using the emulator. It requires only the RIM loader and, at 10 characters per second, takes about 25 minutes to load and start. Turning off the data-rate throttle loads it much faster. ASR-33 teletypes connected to PDP8 systems typically had a hardware tape-reader auto-start/stop feature. The emulator includes trailer detection so the reader can stop before trailing bytes are fed into a program's startup dialogue.
