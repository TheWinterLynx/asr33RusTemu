# Final migration and acceptance audit

Branch: `agent/self-contained-exe`
Base: `master`
Primary target: Windows

## Status

The application migration is complete on this branch. The repository application and automated test suite are Rust-only; the former reference implementation and its test harness have been removed after compatibility expectations were frozen in native Rust tests.

The Windows release executable has also been validated as a single-file deployment: it starts from an otherwise empty directory with its default configuration, Teletype33 font and ASR-33 sound samples embedded.

## Local release gate

The required local gate is:

- `cargo fmt --check`;
- `cargo clippy --all-targets --all-features -- -D warnings`;
- `cargo test --all`;
- `cargo build --release`;
- launch a copied `asr33emu.exe` from an otherwise empty directory.

No hosted automation is required for this branch.

## Compatibility matrix

| Area | Rust evidence | Status |
| --- | --- | --- |
| Serial-only product surface | typed serial config and transport | Covered |
| Explicit Connect/Disconnect/Reconnect | `AppRuntime::new_disconnected`, connection lifecycle tests | Covered |
| Failed connect and retry | app pipeline connection-failure tests | Covered |
| No stale TX after reconnect | app pipeline disconnect/backpressure tests | Covered |
| Terminal 7-bit and parity masking | `tests/terminal_compat.rs` and keyboard tests | Covered |
| CR/LF/BS/TAB/VT/FF | terminal and app-pipeline tests | Covered |
| Autowrap, overstrike and scrollback | `tests/terminal_compat.rs` | Covered |
| Printer state and forwarding | terminal/app pipeline tests | Covered |
| Line / Local routing | app pipeline routing tests | Covered |
| Throttled/unthrottled operation | throttle/core and app pipeline tests | Covered |
| ASR REPT / typematic suppression | `src/ui/repeat.rs` tests | Covered |
| Keyboard uppercase/parity/Return modes | `src/ui/keyboard.rs` and app pipeline tests | Covered |
| Paper reader leader/trailer/autostop/MSB | `tests/paper_tape_compat.rs` and pipeline tests | Covered |
| Punch append/overwrite | paper-tape compatibility and pipeline tests | Covered |
| Audio state and margin bell | deterministic core audio tests plus audio adapter tests | Covered |
| Mute/lid/tape-reader/key audio states | deterministic audio state and UI integration | Covered |
| Settings and YAML/CLI | native config tests plus frozen final compatibility snapshot | Covered |
| Custom font with bundled fallback | Rust UI font loader tests | Covered |
| Embedded default configuration | startup config tests and standalone EXE acceptance | Covered |
| Embedded font | compile-time `include_bytes!` plus standalone EXE acceptance | Covered |
| Embedded sound library | compile-time WAV embedding and Windows adapter test | Covered |
| Single-file Windows release | local isolated-directory acceptance | Covered |

## Frozen configuration contract

`tests/config_compat.rs` now contains the final characterized configuration snapshot directly in Rust. The test compares the entire shared configuration structure, with the native `input_return_mode` extension excluded from the historical snapshot by design.

This preserves regression protection without executing any external reference runtime.

## Intentional design decisions

- One serial transport; no transport selector.
- Startup remains disconnected until explicit Connect/Reconnect.
- The native egui frontend is the only frontend.
- Parsed-file and effective configurations are independently owned.
- `input_return_mode` is a native extension.
- Default configuration, Teletype33 and all runtime sound samples are embedded into the Windows executable.
- Explicit custom configuration files, custom fonts and paper tapes remain user-supplied external files.

## Interactive Windows acceptance

The following were accepted on a Windows workstation as the final deployment-level checks:

- release build succeeds;
- single EXE launches from an empty directory;
- embedded default configuration is used when no YAML exists;
- bundled font renders without a sibling font file;
- bundled sounds work without a sibling `sounds` directory;
- Settings can persist `asr33_config.yaml` beside the executable and that file is read on the next launch.

Serial hardware/virtual-COM behaviour should still be exercised whenever serial lifecycle code changes, because it depends on an actual Windows endpoint.

## Closure criterion

Migration closure is reached when the Rust-only tree passes the local release gate after removal of the reference files. Any future changes are normal product maintenance rather than migration work.
