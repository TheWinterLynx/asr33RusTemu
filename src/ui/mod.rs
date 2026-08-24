//! Native Rust user interface.

mod egui_app;
mod repeat;
pub mod keyboard;
pub mod layout;
pub mod settings;
pub mod tape_view;
pub mod theme;

pub use egui_app::{UiOptions, repaint_delay};

use crate::app::config_controller::{keyboard_repeat_enabled, set_keyboard_repeat_enabled};

/// Thin input-policy wrapper around the established egui application.
///
/// Keeping repeat filtering here means the terminal/paste routing in
/// `egui_app.rs` remains untouched. The filter runs before the inner app sees
/// the frame's egui input events.
pub struct EguiApp {
    inner: egui_app::EguiApp,
}

impl EguiApp {
    #[must_use]
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        runtime: crate::app::AppRuntime<
            crate::adapters::transport::serial::SerialTransport,
            crate::app::SystemScheduler,
        >,
        options: UiOptions,
    ) -> Self {
        Self {
            inner: egui_app::EguiApp::new(creation_context, runtime, options),
        }
    }
}

fn keyboard_quick_controls(ui: &mut eframe::egui::Ui) {
    ui.horizontal(|ui| {
        ui.label("Keyboard");
        ui.label("Repeat");
        let enabled = keyboard_repeat_enabled();
        if ui
            .button(if enabled { "ON" } else { "OFF" })
            .on_hover_text("Allow a held ASR-33 key to repeat; default is OFF")
            .clicked()
        {
            set_keyboard_repeat_enabled(!enabled);
            ui.ctx().request_repaint();
        }
        ui.weak("EOL and the other operational controls remain in the main bar above");
    });
}

impl eframe::App for EguiApp {
    fn clear_color(&self, visuals: &eframe::egui::Visuals) -> [f32; 4] {
        <egui_app::EguiApp as eframe::App>::clear_color(&self.inner, visuals)
    }

    fn logic(&mut self, context: &eframe::egui::Context, frame: &mut eframe::Frame) {
        repeat::filter_host_repeat_events(context);
        <egui_app::EguiApp as eframe::App>::logic(&mut self.inner, context, frame);
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        eframe::egui::Panel::bottom("keyboard-quick-controls")
            .resizable(false)
            .show(ui, keyboard_quick_controls);
        <egui_app::EguiApp as eframe::App>::ui(&mut self.inner, ui, frame);
    }

    fn on_exit(&mut self) {
        <egui_app::EguiApp as eframe::App>::on_exit(&mut self.inner);
    }
}
