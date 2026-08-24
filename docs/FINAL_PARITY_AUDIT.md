# Final parity and acceptance audit

Branch: `agent/final-parity-audit`
Base: `master`
Primary target: Windows

## Purpose

This is the closing validation for the Python-to-Rust migration. It distinguishes behavior that can be certified automatically from behavior that requires an interactive Windows machine or a real/virtual serial endpoint.

## Automated gate

The workflow `.github/workflows/final-parity-validation.yml` runs on `windows-latest` and requires all of the following to pass:

- no tracked obsolete alternate-transport references;
- `cargo fmt --check`;
- `cargo clippy --all-targets --all-features -- -D warnings`;
- `cargo test --all`;
- the retained Python characterization suite;
- `cargo build --release`;
- `asr33emu.exe --help` as a Windows release-binary smoke test.

## Python -> Rust parity matrix

| Area | Rust evidence | Status before CI |
| --- | --- | --- |
| Serial-only product surface | typed Rust serial config and transport; obsolete transport grep gate | Covered |
| Explicit Connect/Disconnect/Reconnect lifecycle | `AppRuntime::new_disconnected`, connection-state integration tests, explicit `SerialTransport::open` path | Covered |
| Failed connect and retry | `connect_failure_disconnect_and_reconnect_do_not_stop_runtime` | Covered |
| No stale TX after disconnect/reconnect | app pipeline disconnect/backpressure tests | Covered |
| Terminal 7-bit behavior and parity masking | `tests/terminal_compat.rs`, keyboard tests | Covered |
| CR/LF/BS/TAB/VT/FF behavior | terminal compatibility tests and app-pipeline return-mode tests | Covered |
| Autowrap, overstrike and scrollback | `tests/terminal_compat.rs` | Covered |
| Printer enable/disable and forwarding | terminal/app pipeline tests | Covered |
| LINE / LOCAL routing and mode changes | app pipeline FIFO/routing tests | Covered |
| Throttled and unthrottled operation | throttle/core and app pipeline tests | Covered |
| ASR REPT / host typematic suppression | `src/ui/repeat.rs` tests, 100 ms ASR cadence | Covered |
| Keyboard uppercase/parity/Return modes | `src/ui/keyboard.rs` and app pipeline tests | Covered |
| Paper tape reader leader/trailer/autostop/MSB | `tests/paper_tape_compat.rs` and reader-feed pipeline tests | Covered |
| Paper tape punch append/overwrite and forwarding | paper-tape compatibility and app pipeline tests | Covered |
| Audio state: print/space/hum, CR/LF/bell, column bell | deterministic `src/core/audio.rs` tests | Covered |
| Mute/lid/tape-reader/key effect state | deterministic audio state machine and egui integration | Covered |
| Settings and YAML/CLI compatibility | Rust config tests plus retained Python differential config tests | Covered |
| Custom `font_path` with bundled fallback | Rust UI font loader tests | Covered |
| Release build on Windows | GitHub Actions gate | Pending CI |

## Intentional differences from Python

These are approved product/design decisions and are not parity defects:

- Rust is serial-only; there is no alternate transport selector.
- Rust starts disconnected and opens the configured serial endpoint only after explicit Connect/Reconnect.
- Tkinter/Pygame implementation details are not preserved; egui is the native Rust frontend.
- Rust configuration keeps parsed-file and effective configuration independently owned rather than reproducing Python's shallow-copy mutation artifact.
- `input_return_mode` is a Rust-side extension and is excluded from the shared legacy configuration snapshot comparison.

## Windows acceptance

### Automatically certifiable in Actions

- release-mode compilation on Windows;
- CLI startup/help path;
- configuration parsing and validation;
- terminal/keyboard/throttle/paper-tape behavior;
- connection lifecycle using the transport abstraction;
- serial settings conversion/platform code compilation;
- deterministic audio behavior;
- no obsolete alternate-transport references.

### Requires one interactive Windows pass

GitHub-hosted runners do not provide a physical COM endpoint, an interactive desktop suitable for judging egui behavior, speakers, or an ASR-connected peer. The following therefore cannot honestly be certified by Actions:

- select a real or virtual COM port in Settings and Connect;
- verify bidirectional bytes against a second endpoint/device;
- Disconnect and Reconnect the same port;
- verify a nonexistent or busy port reports an error without terminating the emulator;
- change baud/data/parity/stop settings and reconnect;
- exercise F1-F9, scrolling, right-click paste and REPT interactively;
- load/run/rewind paper tape and create a punch file from the GUI;
- audibly verify key, print, CR/LF, bell, lid, motor and tape-reader sounds;
- visually verify configured custom font and bundled-font fallback.

This residual checklist is an environmental acceptance test, not missing migration code.

## Closure criterion

Points 1 and 2 are complete when the Windows CI gate is green and this matrix has no unexplained gaps. Point 3 is complete to the maximum extent possible in CI when the release binary and all lifecycle/core tests pass; full physical/visual/audio acceptance requires the short interactive checklist above on a Windows workstation.
