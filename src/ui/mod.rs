//! Native Rust user interface.

mod egui_app;
pub mod keyboard;
pub mod layout;
mod repeat;
pub mod settings;
pub mod tape_view;
pub mod theme;

use std::path::Path;

use eframe::egui::{FontData, FontDefinitions, FontFamily};

pub use egui_app::{UiOptions, repaint_delay};

const TERMINAL_FONT_NAME: &str = "teletype-33";

/// Thin input-policy wrapper around the established egui application.
///
/// Keeping repeat policy here means the terminal/paste routing in
/// `egui_app.rs` remains untouched. Host typematic is filtered before the
/// inner app sees the frame, and optional ASR-paced repeat is synthesized at
/// the same boundary.
pub struct EguiApp {
    inner: egui_app::EguiApp,
    repeat: repeat::RepeatController,
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
        let custom_font_path = options.applied_config.terminal.config.font_path.clone();
        let inner = egui_app::EguiApp::new(creation_context, runtime, options);

        // The inner UI installs the bundled Teletype33 first, guaranteeing a
        // usable fallback. A configured font then replaces the terminal family
        // only when the file can actually be read.
        if let Some(path) = custom_font_path.as_deref()
            && let Err(error) = install_custom_terminal_font(&creation_context.egui_ctx, path)
        {
            eprintln!(
                "asr33emu: custom terminal font {} could not be loaded: {error}; using bundled Teletype33.ttf",
                path.display()
            );
        }

        Self {
            inner,
            repeat: repeat::RepeatController::default(),
        }
    }
}

fn install_custom_terminal_font(
    context: &eframe::egui::Context,
    path: &Path,
) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err("font file is empty".to_owned());
    }

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        TERMINAL_FONT_NAME.to_owned(),
        FontData::from_owned(bytes).into(),
    );
    fonts
        .families
        .entry(FontFamily::Name(TERMINAL_FONT_NAME.into()))
        .or_default()
        .push(TERMINAL_FONT_NAME.to_owned());
    context.set_fonts(fonts);
    Ok(())
}

impl eframe::App for EguiApp {
    fn clear_color(&self, visuals: &eframe::egui::Visuals) -> [f32; 4] {
        <egui_app::EguiApp as eframe::App>::clear_color(&self.inner, visuals)
    }

    fn logic(&mut self, context: &eframe::egui::Context, frame: &mut eframe::Frame) {
        self.repeat.process(context);
        <egui_app::EguiApp as eframe::App>::logic(&mut self.inner, context, frame);
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        <egui_app::EguiApp as eframe::App>::ui(&mut self.inner, ui, frame);
    }

    fn on_exit(&mut self) {
        <egui_app::EguiApp as eframe::App>::on_exit(&mut self.inner);
    }
}

#[cfg(test)]
mod tests {
    use super::install_custom_terminal_font;
    use eframe::egui;

    #[test]
    fn custom_font_loader_rejects_missing_and_empty_files_without_panicking() {
        let context = egui::Context::default();
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("missing.ttf");
        assert!(install_custom_terminal_font(&context, &missing).is_err());

        let empty = dir.path().join("empty.ttf");
        std::fs::write(&empty, b"").expect("empty fixture");
        assert!(install_custom_terminal_font(&context, &empty).is_err());
    }

    #[test]
    fn configured_font_bytes_are_accepted_by_the_egui_font_registry() {
        let context = egui::Context::default();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Teletype33.ttf");
        std::fs::write(&path, include_bytes!("../../Teletype33.ttf")).expect("font fixture");
        install_custom_terminal_font(&context, &path).expect("custom font loads");
    }
}
