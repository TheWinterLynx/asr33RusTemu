//! Native Rust user interface.

mod egui_app;
pub mod keyboard;
pub mod layout;
pub mod tape_view;
pub mod theme;

pub use egui_app::{EguiApp, UiOptions, repaint_delay};
