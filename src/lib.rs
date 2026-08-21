//! Incremental Rust migration of the ASR-33 emulator.

/// Domain and emulation logic, independent of GUI and runtime libraries.
pub mod core;

/// Integrations with external systems such as transports, audio, and files.
pub mod adapters;

/// Application composition and lifecycle.
pub mod app;

/// Native egui frontend and UI-independent keyboard adaptation.
pub mod ui;
