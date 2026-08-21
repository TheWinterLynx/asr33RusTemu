use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId};

use crate::adapters::transport::serial::SerialTransport;
use crate::app::{AppRuntime, PumpStatus, RuntimeEvent, RuntimeState, SystemScheduler};
use crate::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};

use super::keyboard::{KeyboardInput, KeyboardOptions, encode_input};

const FONT_NAME: &str = "teletype-33";
const IDLE_POLL: Duration = Duration::from_millis(16);
const BACKPRESSURE_RETRY: Duration = Duration::from_millis(10);

#[derive(Clone, Debug)]
pub struct UiOptions {
    pub title: String,
    pub backend_label: String,
    pub font_size: f32,
    pub keyboard: KeyboardOptions,
    pub communication_mode: CommunicationMode,
    pub throttle_mode: ThrottleMode,
    pub printer_enabled: bool,
}

#[must_use]
pub const fn repaint_delay(status: PumpStatus) -> Option<Duration> {
    match status {
        PumpStatus::WorkRemaining => Some(Duration::ZERO),
        PumpStatus::Wait(duration) => Some(duration),
        PumpStatus::Idle => Some(IDLE_POLL),
        PumpStatus::Backpressured => Some(BACKPRESSURE_RETRY),
        PumpStatus::TransportFailed => Some(IDLE_POLL),
    }
}

pub struct EguiApp {
    runtime: AppRuntime<SerialTransport, SystemScheduler>,
    options: UiOptions,
    transport_error: Option<String>,
    shutdown_complete: bool,
}

impl EguiApp {
    #[must_use]
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        runtime: AppRuntime<SerialTransport, SystemScheduler>,
        options: UiOptions,
    ) -> Self {
        install_font(&creation_context.egui_ctx);
        Self {
            runtime,
            options,
            transport_error: None,
            shutdown_complete: false,
        }
    }

    fn submit(&mut self, command: ApplicationCommand) {
        if let Err(error) = self.runtime.submit(command) {
            self.transport_error = Some(error.to_string());
        }
    }

    fn handle_keyboard(&mut self, context: &egui::Context) {
        let events = context.input(|input| input.events.clone());
        for event in events {
            let logical = match event {
                egui::Event::Text(text) => Some(KeyboardInput::Text(text)),
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => map_key(key, modifiers),
                _ => None,
            };
            if let Some(input) = logical {
                match encode_input(&input, self.options.keyboard) {
                    Ok(bytes) if !bytes.is_empty() => {
                        self.submit(ApplicationCommand::Transmit(bytes));
                    }
                    Ok(_) => {}
                    Err(error) => self.transport_error = Some(error.to_string()),
                }
            }
        }
    }

    fn drive_runtime(&mut self, context: &egui::Context) {
        if self.runtime.state() == RuntimeState::Failed {
            context.request_repaint_after(IDLE_POLL);
            self.collect_runtime_events();
            return;
        }
        match self.runtime.tick() {
            Ok(status) => {
                if let Some(delay) = repaint_delay(status) {
                    if delay.is_zero() {
                        context.request_repaint();
                    } else {
                        context.request_repaint_after(delay);
                    }
                }
            }
            Err(error) => {
                self.transport_error = Some(error.to_string());
                context.request_repaint_after(IDLE_POLL);
            }
        }
        self.collect_runtime_events();
    }

    fn collect_runtime_events(&mut self) {
        while let Some(event) = self.runtime.pop_event() {
            let RuntimeEvent::TransportFailed { operation, message } = event;
            self.transport_error = Some(format!("{operation:?}: {message}"));
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(&self.options.backend_label);
            ui.separator();
            ui.label(format!("runtime: {:?}", self.runtime.state()));
            ui.separator();

            if ui
                .selectable_label(
                    self.options.communication_mode == CommunicationMode::Line,
                    "LINE",
                )
                .clicked()
            {
                self.options.communication_mode = CommunicationMode::Line;
                self.submit(ApplicationCommand::SetCommunicationMode(
                    CommunicationMode::Line,
                ));
            }
            if ui
                .selectable_label(
                    self.options.communication_mode == CommunicationMode::Local,
                    "LOCAL",
                )
                .clicked()
            {
                self.options.communication_mode = CommunicationMode::Local;
                self.submit(ApplicationCommand::SetCommunicationMode(
                    CommunicationMode::Local,
                ));
            }
            ui.separator();
            if ui
                .selectable_label(
                    self.options.throttle_mode == ThrottleMode::Throttled,
                    "THROTTLED",
                )
                .clicked()
            {
                self.options.throttle_mode = ThrottleMode::Throttled;
                self.submit(ApplicationCommand::SetThrottleMode(ThrottleMode::Throttled));
            }
            if ui
                .selectable_label(
                    self.options.throttle_mode == ThrottleMode::Unthrottled,
                    "UNTHROTTLED",
                )
                .clicked()
            {
                self.options.throttle_mode = ThrottleMode::Unthrottled;
                self.submit(ApplicationCommand::SetThrottleMode(
                    ThrottleMode::Unthrottled,
                ));
            }
            ui.separator();
            let printer_label = if self.options.printer_enabled {
                "PRINTER ON"
            } else {
                "PRINTER OFF"
            };
            if ui.button(printer_label).clicked() {
                self.options.printer_enabled = !self.options.printer_enabled;
                self.submit(ApplicationCommand::SetPrinterEnabled(
                    self.options.printer_enabled,
                ));
            }
        });
        if let Some(error) = &self.transport_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    fn terminal(&self, ui: &mut egui::Ui) {
        let terminal = self.runtime.terminal();
        let history = terminal.line_history();
        let (cursor_column, cursor_line) = terminal.cursor_position();
        let font = FontId::new(self.options.font_size, FontFamily::Name(FONT_NAME.into()));
        let row_height = self.options.font_size * 1.25;
        let cell_width = self.options.font_size * 0.62;
        let rendered_rows = history.len().max(terminal.height());
        let grid_size = egui::vec2(
            cell_width * terminal.width() as f32,
            row_height * rendered_rows as f32,
        );

        egui::ScrollArea::both()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(grid_size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                for (row, line) in history.lines().iter().enumerate() {
                    let y = rect.top() + row as f32 * row_height;
                    for column in 0..line.width() {
                        let position = egui::pos2(rect.left() + column as f32 * cell_width, y);
                        for character in line.strike_stack(column) {
                            painter.text(
                                position,
                                Align2::LEFT_TOP,
                                character,
                                font.clone(),
                                Color32::LIGHT_GRAY,
                            );
                        }
                    }
                    if line.logical_number() == cursor_line {
                        let cursor = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + cursor_column as f32 * cell_width, y),
                            egui::vec2(cell_width, row_height),
                        );
                        painter.rect_stroke(
                            cursor,
                            0.0,
                            egui::Stroke::new(1.0, Color32::LIGHT_GREEN),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            });
    }

    fn shutdown(&mut self) {
        if self.shutdown_complete {
            return;
        }
        if let Err(error) = self.runtime.shutdown().and_then(|()| self.runtime.join()) {
            self.transport_error = Some(error.to_string());
        }
        self.shutdown_complete = true;
    }
}

impl eframe::App for EguiApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if context.input(|input| input.viewport().close_requested()) {
            self.shutdown();
            return;
        }
        self.drive_runtime(context);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_keyboard(ui.ctx());
        egui::Panel::top("status").show(ui, |ui| self.controls(ui));
        egui::CentralPanel::default().show(ui, |ui| self.terminal(ui));
    }

    fn on_exit(&mut self) {
        self.shutdown();
    }
}

impl Drop for EguiApp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn install_font(context: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        FontData::from_static(include_bytes!("../../Teletype33.ttf")).into(),
    );
    fonts
        .families
        .entry(FontFamily::Name(FONT_NAME.into()))
        .or_default()
        .push(FONT_NAME.to_owned());
    context.set_fonts(fonts);
}

fn map_key(key: egui::Key, modifiers: egui::Modifiers) -> Option<KeyboardInput> {
    if modifiers.ctrl {
        let character = match key {
            egui::Key::A => 'A',
            egui::Key::B => 'B',
            egui::Key::C => 'C',
            egui::Key::D => 'D',
            egui::Key::E => 'E',
            egui::Key::F => 'F',
            egui::Key::G => 'G',
            egui::Key::H => 'H',
            egui::Key::I => 'I',
            egui::Key::J => 'J',
            egui::Key::K => 'K',
            egui::Key::L => 'L',
            egui::Key::M => 'M',
            egui::Key::N => 'N',
            egui::Key::O => 'O',
            egui::Key::P => 'P',
            egui::Key::Q => 'Q',
            egui::Key::R => 'R',
            egui::Key::S => 'S',
            egui::Key::T => 'T',
            egui::Key::U => 'U',
            egui::Key::V => 'V',
            egui::Key::W => 'W',
            egui::Key::X => 'X',
            egui::Key::Y => 'Y',
            egui::Key::Z => 'Z',
            _ => return None,
        };
        return Some(KeyboardInput::Control(character));
    }
    match key {
        egui::Key::Enter => Some(KeyboardInput::Return),
        egui::Key::Backspace => Some(KeyboardInput::Backspace),
        egui::Key::Tab => Some(KeyboardInput::Tab),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::repaint_delay;
    use crate::app::PumpStatus;
    use std::time::Duration;

    #[test]
    fn maps_every_runtime_status_to_non_blocking_repaint_policy() {
        assert_eq!(
            repaint_delay(PumpStatus::Wait(Duration::from_millis(7))),
            Some(Duration::from_millis(7))
        );
        assert_eq!(
            repaint_delay(PumpStatus::WorkRemaining),
            Some(Duration::ZERO)
        );
        assert_eq!(
            repaint_delay(PumpStatus::Idle),
            Some(Duration::from_millis(16))
        );
        assert_eq!(
            repaint_delay(PumpStatus::Backpressured),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            repaint_delay(PumpStatus::TransportFailed),
            Some(Duration::from_millis(16))
        );
    }
}
