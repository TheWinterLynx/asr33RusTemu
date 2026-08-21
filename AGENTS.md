# AGENTS.md

## Project purpose

This repository implements an ASR-33 Teletype emulator in Python. The project is being migrated to Rust.

The existing Python implementation is the executable behavioural specification until the Rust migration is complete. Preserve observable behaviour unless a task explicitly requests a behaviour change.

Windows is the primary target platform for the Rust migration. Linux support is
best-effort: preserve it where practical, but do not create additional Linux
work unless it is trivial or a task explicitly requests it.

Do not perform a mechanical line-by-line Python-to-Rust translation. Prefer an idiomatic Rust design with clear ownership, explicit state, narrow interfaces, and testable components.

## Current application architecture

The current Python application is assembled roughly as:

`communication backend -> data throttle -> terminal core -> frontend`

The wrapper in `asr33emu.py` wires these layers together with forward references.

Important existing components include:

- `asr33emu.py`: application composition / entry point.
- `asr33_config.py`: YAML and CLI configuration merging.
- `asr33_backend_serial.py`: serial transport.
- `asr33_backend_ssh.py`: SSH transport and host-key/authentication handling.
- `asr33_shim_throttle.py`: send/receive rate limiting and local loopback.
- `asr33_terminal.py`: terminal state, line history, overstrike, parity handling, cursor movement and escape stripping.
- `asr33_papertape.py`: paper-tape reader and punch behaviour plus file handling.
- `asr33_pt_animate_tk.py`: Tk paper-tape visualization.
- `asr33_sounds_sm.py`: ASR-33 audio state machine and playback.
- `asr33_frontend_tk.py`: Tk frontend.
- `asr33_frontend_pygame.py`: Pygame frontend.
- `asr33_config.yaml`: default configuration and feature surface.

The Python code currently mixes some device logic, threading and UI concerns. In particular, paper-tape behaviour is coupled to Tk UI code, and the Pygame frontend creates a hidden Tk root to reuse the paper-tape widgets. Do not preserve that coupling in the Rust design.

## Behaviour that must not be lost silently

Treat the following as compatibility requirements unless the task says otherwise:

- serial backend;
- SSH backend;
- configurable terminal rows, columns and scrollback;
- ASR-33 7-bit character behaviour;
- keyboard uppercase-only option;
- mark, space and even parity modes;
- carriage return, line feed, vertical tab, backspace, tab and form-feed handling as currently implemented;
- optional autowrap;
- overstrike support;
- ANSI CSI/OSC stripping used for modern-host compatibility;
- authentic throttled operation, normally 10 characters per second;
- unthrottled mode;
- local/loopback mode;
- printer enable/disable state;
- paper-tape reader and punch;
- tape reader leading-null skipping, MSB option and trailer auto-stop behaviour;
- paper-tape append/overwrite behaviour;
- sound state machine, mute and lid behaviour;
- column bell behaviour;
- YAML configuration plus CLI overrides;
- bundled Teletype font support;
- Windows and Linux behaviour where currently supported.

If a migration step intentionally changes or drops any of these behaviours, state it explicitly before implementing it.

## Migration strategy

For non-trivial migration work, do not immediately edit code.

First:

1. inspect the relevant Python implementation;
2. identify callers, consumers and cross-module dependencies;
3. identify observable behaviour and edge cases;
4. locate existing tests, if any;
5. add characterization tests where practical before replacing behaviour;
6. propose the Rust boundary and ownership model;
7. implement one coherent migration slice at a time.

Keep the Python implementation runnable during the migration unless a task explicitly authorizes removing it.

Prefer incremental vertical slices over a big-bang rewrite.

A migration step should leave the repository in a usable state and should be independently reviewable.

## Suggested Rust boundaries

Use these as architectural guidance, not as a requirement to create one crate per item:

- `config`: typed configuration and CLI parsing;
- `terminal`: pure terminal state and character processing;
- `transport`: transport abstraction;
- `transport::serial`: serial implementation;
- `transport::ssh`: SSH implementation;
- `throttle`: rate limiting and loopback policy;
- `paper_tape`: reader/punch state and file semantics without GUI dependencies;
- `audio`: sound events and audio state;
- `ui`: rendering, keyboard/mouse input and windows;
- `app`: composition and lifecycle.

Keep emulation/domain logic independent from the chosen GUI toolkit.

The paper-tape core must not depend on GUI widgets or file-dialog APIs. UI code may call into the paper-tape core.

The terminal core should be testable without serial ports, SSH, audio, GUI or real-time sleeps.

## Rust design rules

Prefer idiomatic Rust.

Avoid:

- translating every Python class directly into a Rust struct;
- unnecessary `clone()` calls;
- pervasive `Arc<Mutex<T>>`;
- global mutable state;
- `unwrap()` / `expect()` on normal production error paths;
- stringly typed state when an enum is appropriate;
- speculative traits and generic abstractions;
- coupling domain logic to GUI toolkit types;
- sleeps embedded in logic that should be deterministic under tests.

Prefer:

- explicit ownership;
- enums for finite states such as terminal mode, parity mode, lid state and throttle mode;
- typed configuration via `serde` or an equivalent approach;
- `Result`-based error propagation;
- channels/message passing where it simplifies ownership between I/O workers and UI/core;
- monotonic time for rate-control logic;
- dependency injection of time or scheduling where useful for deterministic tests;
- small cohesive modules;
- explicit lifecycle and shutdown semantics for worker threads/tasks.

Do not introduce async Rust merely because it exists. Use it only when it materially simplifies the transport/lifecycle architecture.

## Dependency selection

Before adding a major Rust dependency, inspect its maintenance status, platform support and fit for this project.

Do not silently select a GUI, SSH, serial or audio library for the whole migration as part of an unrelated task.

Major technology choices should be documented with the alternatives considered and the reason for the selection.

## Testing and compatibility

The repository currently has little or no formal automated test coverage. Treat this as a migration risk.

Before replacing important Python behaviour, add characterization tests where practical.

High-priority characterization areas:

- parity encoding and masking;
- escape-sequence stripping;
- cursor and line-history behaviour;
- autowrap and overstrike;
- tab/CR/LF/control-character handling;
- throttle timing policy without depending on wall-clock sleeps where possible;
- loopback behaviour;
- paper-tape leading-null skipping;
- paper-tape trailer auto-stop rules;
- paper-tape punch append/overwrite semantics;
- configuration merging and CLI overrides.

Where practical, use the same fixtures against Python and Rust and compare outputs. Differential tests are preferred for behaviour that is difficult to specify manually.

Do not weaken or rewrite a characterization test merely to make a new implementation pass unless the expected behaviour is intentionally being changed.

## UI migration

Do not reproduce Tkinter/Pygame implementation details unless they are required for observable behaviour.

Separate:

- terminal/device state;
- input commands;
- render state;
- actual toolkit widgets/windows.

The current two-front-end design may be consolidated in Rust, but that is an architectural decision and must not silently remove required features.

## Concurrency and thread safety

The Python implementation uses several threads and queues for serial I/O, throttling, paper tape and audio.

Before translating a threaded component, identify:

- who owns its state;
- which direction data flows;
- what can block;
- queue/backpressure behaviour;
- startup ordering;
- shutdown ordering;
- which operations must run on the UI thread.

Prefer an architecture where worker components communicate through bounded channels and the UI owns UI state.

Do not add locks around local temporary objects and assume that provides cross-thread synchronization. Synchronization must protect the actual shared state.

## Scope discipline

Keep diffs tightly scoped to the requested task.

Do not:

- perform unrelated refactors;
- rename unrelated symbols or files;
- reformat unrelated code;
- fix unrelated warnings unless they block the requested work;
- remove Python components simply because a Rust equivalent has started to exist.

If an unrelated problem is discovered, report it instead of silently folding it into the change.

## Root-cause rule

For bug fixes, identify the root cause before implementing the fix.

Do not hide symptoms with special cases or workarounds when the underlying problem can reasonably be corrected.

## Validation

For Python-only changes, run the relevant available checks and at minimum ensure changed Python files parse/compile.

Once a Rust workspace exists, the default completion checks are:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Run narrower targeted tests during development, but run the complete applicable validation before considering a migration slice complete.

If a check cannot be run because of missing hardware, credentials, GUI/display support or an external service, report that explicitly.

## Final review

Before finishing a code-changing task, review the complete diff as if reviewing another developer's pull request.

Look for:

- accidental behaviour changes;
- missing compatibility behaviour;
- unnecessary complexity;
- duplicated logic;
- inappropriate shared ownership;
- race conditions or shutdown problems;
- hidden GUI dependencies in domain modules;
- unbounded queues where backpressure matters;
- dead code;
- weak error handling;
- missing tests;
- changes outside scope.

Fix issues found during this review before finishing.

## Completion report

When finishing a migration task, report:

1. what changed;
2. which Python behaviour it replaces or preserves;
3. important architectural decisions;
4. tests and validation commands executed;
5. known limitations or behaviours not yet migrated;
6. the recommended next migration slice.
