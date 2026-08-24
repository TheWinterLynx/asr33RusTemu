# AGENTS.md

## Project purpose

This repository implements an ASR-33 Teletype emulator in Rust with egui. Windows is the primary target platform; Linux support is best-effort unless a task explicitly requires it.

The application supports **serial transport only**. Do not introduce an alternate transport, network transport dependency, transport selector, or alternate transport configuration unless a future task explicitly changes that product decision.

The application deliberately starts **disconnected**. A configured COM/tty port describes the desired serial connection but must only be opened after an explicit Connect/Reconnect action. Do not restore constructor-time serial auto-open behaviour or hidden reconnect loops.

## Current architecture

The main Rust boundaries are:

- `src/core/config.rs`: typed YAML/CLI configuration;
- `src/core/terminal`: terminal state, history, control characters and parity handling;
- `src/core/throttle.rs`: deterministic rate-control policy;
- `src/core/paper_tape`: reader/punch device semantics;
- `src/core/audio.rs`: deterministic audio state and events;
- `src/adapters/transport/serial`: serial transport and platform settings;
- `src/adapters/audio_margin.rs` + `audio_rodio_crlf.rs`: audio policy and playback;
- `src/adapters/embedded_sounds.rs`: compile-time bundled WAV assets;
- `src/app`: application composition and lifecycle;
- `src/ui`: egui rendering, settings, keyboard/repeat and paper-tape visualization.

Keep emulation/domain logic independent from GUI widgets, file dialogs, serial ports, audio devices and wall-clock sleeps wherever practical.

## Product behaviour that must not be lost silently

Treat the following as compatibility requirements unless a task explicitly changes them:

- serial transport;
- explicit Connect/Disconnect/Reconnect lifecycle;
- configurable rows, columns and scrollback;
- ASR-33 7-bit character behaviour;
- keyboard uppercase-only option;
- mark, space and even parity modes;
- carriage return, line feed, vertical tab, backspace, tab and form-feed handling;
- optional autowrap and overstrike;
- ANSI CSI/OSC stripping used for modern-host compatibility;
- authentic throttled operation, normally 10 characters per second;
- unthrottled mode;
- Line/Local mode;
- printer enable/disable state;
- paper-tape reader and punch;
- leading-null skipping, MSB option and trailer auto-stop behaviour;
- punch append/overwrite semantics;
- sound state machine, mute, lid and margin-bell behaviour;
- YAML configuration plus CLI overrides;
- bundled Teletype font and optional custom font path;
- single-file Windows release behaviour: default config, font and sounds embedded in `asr33emu.exe`.

Intentional design decisions:

- one serial transport, no transport selector;
- startup remains disconnected until explicit Connect/Reconnect;
- parsed-file configuration and effective configuration are independently owned;
- `input_return_mode` is part of the native Rust configuration surface.

## Rust design rules

Prefer idiomatic Rust with explicit ownership, narrow interfaces and deterministic tests.

Avoid:

- unnecessary `clone()` calls;
- pervasive `Arc<Mutex<T>>`;
- global mutable state;
- `unwrap()` / `expect()` on normal production error paths;
- stringly typed finite state where an enum is appropriate;
- speculative traits and abstractions;
- coupling device/core logic to GUI toolkit types;
- sleeps embedded in logic that should be testable deterministically;
- dead compatibility modules kept after their replacement is complete.

Prefer:

- enums for finite states;
- typed configuration via serde;
- `Result`-based error propagation;
- bounded channels/message passing where it simplifies worker ownership;
- monotonic time or injected scheduling for rate control;
- small cohesive modules;
- explicit startup/shutdown semantics for worker threads.

Do not introduce async Rust merely because it exists. Use it only when it materially simplifies lifecycle architecture.

## Testing and compatibility

Automated test coverage is a release requirement. High-priority areas include:

- parity encoding and masking;
- escape-sequence stripping;
- cursor, line-history, autowrap and overstrike behaviour;
- tab/CR/LF/control-character handling;
- throttled and unthrottled scheduling;
- Line/Local routing;
- paper-tape reader/punch edge cases;
- configuration merging and CLI overrides;
- explicit serial connection lifecycle;
- REPT/typematic behaviour;
- custom font loading with bundled fallback;
- deterministic audio state;
- embedded configuration/font/audio deployment behaviour.

The final migration configuration behaviour is frozen natively in `tests/config_compat.rs`. Do not weaken that snapshot merely to make a change pass; update it only when the configuration contract is intentionally changed.

## Scope discipline

Keep diffs tightly scoped. Do not perform unrelated refactors, rename unrelated symbols/files, reformat unrelated code or fix unrelated warnings unless they block the requested work.

For bug fixes, identify and correct the root cause rather than hiding symptoms with special cases.

## Validation

For Rust changes, the default completion checks are:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
```

For single-file deployment changes, also launch a release `asr33emu.exe` copied into an otherwise empty directory on Windows.

If a check cannot be run because of missing hardware, GUI/display support or an external service, report that explicitly.

## Final review

Before finishing a code-changing task, review the complete diff for accidental behaviour changes, missing compatibility behaviour, unnecessary complexity, duplicated logic, race conditions, shutdown problems, hidden GUI dependencies, unbounded queues, dead code, weak error handling, missing tests and scope creep.

## Completion report

Report:

1. what changed;
2. important architectural decisions;
3. tests/validation performed;
4. known limitations;
5. the recommended next step.
