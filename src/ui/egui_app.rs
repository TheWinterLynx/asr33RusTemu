use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Align2, FontData, FontDefinitions, FontFamily, FontId};

use crate::adapters::paper_tape::{PunchFile, load_reader_file};
use crate::adapters::transport::serial::SerialTransport;
use crate::app::paper_tape::{FeedResult, READER_FEED_INTERVAL, ReaderFeed};
use crate::app::{
    AppRuntime, ConnectionState, ImmediateTransmit, PumpStatus, RuntimeEvent, Scheduler,
    SystemScheduler,
};
use crate::core::config::{PunchConfigMode, SerialConfig, TapePunchConfig, TapeReaderConfig};
use crate::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};
use crate::core::paper_tape::{PunchMode, ReaderOptions, ReaderState, TapeReader};

use super::keyboard::{KeyboardInput, KeyboardOptions, encode_input};
use super::layout::{
    DockSplitState, DockWidthState, PanelPlacement, PanelPresentation, TerminalMetrics,
};
use super::tape_view::{
    ReaderTapeViewState, TapeRendererOptions, TapeSourceOrder, render_reader_tape, render_tape,
};
use super::theme::ThemeKind;

const FONT_NAME: &str = "teletype-33";
const IDLE_POLL: Duration = Duration::from_millis(16);
const BACKPRESSURE_RETRY: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyboardTarget {
    Terminal,
    UiText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TapeStepDirection {
    TowardStart,
    TowardEnd,
}

#[derive(Clone, Debug)]
pub struct UiOptions {
    pub title: String,
    pub backend_label: String,
    pub font_size: f32,
    pub keyboard: KeyboardOptions,
    pub communication_mode: CommunicationMode,
    pub throttle_mode: ThrottleMode,
    pub printer_enabled: bool,
    pub tape_reader: TapeReaderConfig,
    pub tape_punch: TapePunchConfig,
    pub serial_config: SerialConfig,
}

#[must_use]
pub const fn repaint_delay(status: PumpStatus) -> Option<Duration> {
    match status {
        PumpStatus::WorkRemaining => Some(Duration::ZERO),
        PumpStatus::Wait(duration) => Some(duration),
        PumpStatus::Idle => Some(IDLE_POLL),
        PumpStatus::Backpressured => Some(BACKPRESSURE_RETRY),
    }
}

pub struct EguiApp {
    runtime: AppRuntime<SerialTransport, SystemScheduler>,
    options: UiOptions,
    transport_error: Option<String>,
    shutdown_complete: bool,
    reader: ReaderFeed,
    reader_path: Option<PathBuf>,
    punch: Option<PunchFile>,
    punch_mode: PunchMode,
    reader_panel: PanelPresentation,
    punch_panel: PanelPresentation,
    theme: ThemeKind,
    dock_split: DockSplitState,
    dock_width: DockWidthState,
    reader_view: ReaderTapeViewState,
    reader_seek_position: usize,
    keyboard_target: KeyboardTarget,
    tape_error: Option<String>,
    scroll_top: Option<usize>,
    scroll_request: bool,
    available_ports: Vec<String>,
    port_error: Option<String>,
}

impl EguiApp {
    #[must_use]
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        runtime: AppRuntime<SerialTransport, SystemScheduler>,
        options: UiOptions,
    ) -> Self {
        install_font(&creation_context.egui_ctx);
        let reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: options.tape_reader.skip_leading_nulls,
            set_msb: options.tape_reader.set_msb,
            auto_stop: options.tape_reader.auto_stop,
        });
        let punch_mode = match options.tape_punch.mode {
            PunchConfigMode::Append => PunchMode::Append,
            PunchConfigMode::Overwrite => PunchMode::Overwrite,
        };
        let mut application = Self {
            runtime,
            options,
            transport_error: None,
            shutdown_complete: false,
            reader: ReaderFeed::new(reader, Duration::ZERO),
            reader_path: None,
            punch: None,
            punch_mode,
            reader_panel: PanelPresentation::docked(),
            punch_panel: PanelPresentation::docked(),
            theme: ThemeKind::Light,
            dock_split: DockSplitState::default(),
            dock_width: DockWidthState::default(),
            reader_view: ReaderTapeViewState::default(),
            reader_seek_position: 0,
            keyboard_target: KeyboardTarget::Terminal,
            tape_error: None,
            scroll_top: None,
            scroll_request: false,
            available_ports: Vec::new(),
            port_error: None,
        };
        application.refresh_ports();
        application.connect_selected();
        application.theme.apply(&creation_context.egui_ctx);
        application
    }

    fn refresh_ports(&mut self) {
        match serialport::available_ports() {
            Ok(ports) => {
                self.available_ports = ports.into_iter().map(|port| port.port_name).collect();
                self.port_error = None;
            }
            Err(error) => self.port_error = Some(format!("port enumeration failed: {error}")),
        }
    }

    fn connect_selected(&mut self) {
        match SerialTransport::open(self.options.serial_config.clone()) {
            Ok(transport) => match self.runtime.connect(transport) {
                Ok(()) => self.transport_error = None,
                Err(error) => self.transport_error = Some(error.to_string()),
            },
            Err(error) => {
                let message = error.to_string();
                self.runtime.record_connection_failure(message.clone());
                self.transport_error = Some(message);
            }
        }
    }

    fn disconnect(&mut self) {
        if self.options.communication_mode == CommunicationMode::Line
            && (self.reader.reader().state() == ReaderState::Running
                || self.reader.pending_count() != 0)
        {
            self.reader.rollback_unconfirmed();
            self.reader.reader_mut().stop();
            self.tape_error = Some(
                "reader paused: serial disconnected; position retained and stale TX discarded"
                    .to_owned(),
            );
        }
        if let Err(error) = self.runtime.disconnect() {
            self.transport_error = Some(error.to_string());
        }
    }

    fn submit(&mut self, context: &egui::Context, command: ApplicationCommand) {
        if let Err(error) = self.runtime.submit(command) {
            self.transport_error = Some(error.to_string());
        }
        context.request_repaint();
    }

    fn change_communication_mode(&mut self, context: &egui::Context, mode: CommunicationMode) {
        if self.options.communication_mode == mode {
            return;
        }
        if self.reader.pause_for_mode_change() {
            self.tape_error = Some("reader paused: communication mode changed".to_owned());
        }
        match self
            .runtime
            .submit(ApplicationCommand::SetCommunicationMode(mode))
        {
            Ok(()) => self.options.communication_mode = mode,
            Err(error) => self.transport_error = Some(error.to_string()),
        }
        context.request_repaint();
    }

    fn handle_keyboard(&mut self, context: &egui::Context) {
        let events = context.input(|input| input.events.clone());
        for event in &events {
            if self.handle_shortcut(context, event) {
                continue;
            }
            let logical = keyboard_input_for_target(self.keyboard_target, event);
            if let Some(input) = logical {
                match encode_input(&input, self.options.keyboard) {
                    Ok(bytes) if !bytes.is_empty() => {
                        self.submit(context, ApplicationCommand::Transmit(bytes));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.transport_error = Some(error.to_string());
                        context.request_repaint();
                    }
                }
            }
        }
    }

    fn handle_shortcut(&mut self, context: &egui::Context, event: &egui::Event) -> bool {
        let egui::Event::Key {
            key, pressed: true, ..
        } = event
        else {
            return false;
        };
        if apply_tape_shortcut(*key, &mut self.reader_panel, &mut self.punch_panel) {
            context.request_repaint();
            return true;
        }
        match key {
            egui::Key::F5 => {
                self.options.throttle_mode = match self.options.throttle_mode {
                    ThrottleMode::Throttled => ThrottleMode::Unthrottled,
                    ThrottleMode::Unthrottled => ThrottleMode::Throttled,
                };
                self.submit(
                    context,
                    ApplicationCommand::SetThrottleMode(self.options.throttle_mode),
                );
            }
            // F6/F7 are reserved for the future audio slice.
            egui::Key::F8 => {
                let mode = match self.options.communication_mode {
                    CommunicationMode::Line => CommunicationMode::Local,
                    CommunicationMode::Local => CommunicationMode::Line,
                };
                self.change_communication_mode(context, mode);
            }
            egui::Key::F9 => {
                self.options.printer_enabled = !self.options.printer_enabled;
                self.submit(
                    context,
                    ApplicationCommand::SetPrinterEnabled(self.options.printer_enabled),
                );
            }
            egui::Key::PageUp => {
                let bottom = self
                    .runtime
                    .terminal()
                    .line_history()
                    .len()
                    .saturating_sub(self.runtime.terminal().height());
                self.scroll_top = Some(self.scroll_top.unwrap_or(bottom).saturating_sub(12));
                self.scroll_request = true;
            }
            egui::Key::PageDown => {
                let bottom = self
                    .runtime
                    .terminal()
                    .line_history()
                    .len()
                    .saturating_sub(self.runtime.terminal().height());
                let next = self
                    .scroll_top
                    .unwrap_or(bottom)
                    .saturating_add(12)
                    .min(bottom);
                self.scroll_top = (next < bottom).then_some(next);
                self.scroll_request = true;
            }
            egui::Key::Home => {
                self.scroll_top = Some(0);
                self.scroll_request = true;
            }
            egui::Key::End => {
                self.scroll_top = None;
                self.scroll_request = true;
            }
            _ => return false,
        }
        context.request_repaint();
        true
    }

    fn drive_runtime(&mut self, context: &egui::Context) {
        let now = self.runtime.scheduler().now();
        let runtime = &mut self.runtime;
        let mut reader_disconnected = false;
        if let Some(delay) = self
            .reader
            .tick(now, |byte| match runtime.try_transmit(vec![byte]) {
                Ok(ImmediateTransmit::Accepted) => FeedResult::Accepted,
                Ok(ImmediateTransmit::Backpressured(data)) => {
                    FeedResult::Backpressured(data.first().copied().map_or(byte, |pending| pending))
                }
                Ok(ImmediateTransmit::Disconnected(data)) => {
                    reader_disconnected = true;
                    FeedResult::Backpressured(data.first().copied().map_or(byte, |pending| pending))
                }
                Err(_) => FeedResult::Backpressured(byte),
            })
        {
            context.request_repaint_after(delay);
        }
        if reader_disconnected {
            self.reader.reader_mut().stop();
            self.tape_error =
                Some("reader paused: LINE mode requires an active serial connection".to_owned());
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
        let reader_route_available = self.options.communication_mode == CommunicationMode::Local
            || self.runtime.connection_state() == &ConnectionState::Connected;
        if self.reader.awaiting_confirmation()
            && reader_route_available
            && self.runtime.transmit_idle()
        {
            self.reader.confirm_transmitted();
            self.reader_seek_position = self.reader.reader().position();
            context.request_repaint_after(READER_FEED_INTERVAL);
        }
    }

    fn collect_runtime_events(&mut self) {
        while let Some(event) = self.runtime.pop_event() {
            match event {
                RuntimeEvent::ConnectionFailed(message) => {
                    self.transport_error = Some(message);
                    self.pause_line_reader_after_failure();
                }
                RuntimeEvent::TransportFailed { operation, message } => {
                    self.transport_error = Some(format!("{operation:?}: {message}"));
                    self.pause_line_reader_after_failure();
                }
                RuntimeEvent::TerminalForwarded(data) => {
                    if let Some(punch) = self.punch.as_mut()
                        && let Err(error) = punch.punch(&data)
                    {
                        punch.stop();
                        self.tape_error = Some(format!("paper punch write failed: {error}"));
                    }
                }
            }
        }
    }

    fn pause_line_reader_after_failure(&mut self) {
        if self.options.communication_mode == CommunicationMode::Line
            && (self.reader.reader().state() == ReaderState::Running
                || self.reader.pending_count() != 0)
        {
            self.reader.rollback_unconfirmed();
            self.reader.reader_mut().stop();
            self.tape_error = Some(
                "reader paused after connection failure; position retained, external in-flight TX discarded"
                    .to_owned(),
            );
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let connection = self.runtime.connection_state().clone();
        let palette = self.theme.palette();
        ui.horizontal_wrapped(|ui| {
            ui.label("Rate");
            let rate_label = match self.options.throttle_mode {
                ThrottleMode::Throttled => "Throttled",
                ThrottleMode::Unthrottled => "Unthrottled",
            };
            if ui.button(rate_label).clicked() {
                self.options.throttle_mode = match self.options.throttle_mode {
                    ThrottleMode::Throttled => ThrottleMode::Unthrottled,
                    ThrottleMode::Unthrottled => ThrottleMode::Throttled,
                };
                self.submit(
                    ui.ctx(),
                    ApplicationCommand::SetThrottleMode(self.options.throttle_mode),
                );
            }
            ui.add_enabled(false, egui::Button::new("Sound —"))
                .on_disabled_hover_text("Audio not migrated yet");
            ui.add_enabled(false, egui::Button::new("Lid —"))
                .on_disabled_hover_text("Audio not migrated yet");
            ui.separator();
            ui.label("Comm");
            if ui
                .selectable_label(
                    self.options.communication_mode == CommunicationMode::Line,
                    "LINE",
                )
                .clicked()
            {
                self.change_communication_mode(ui.ctx(), CommunicationMode::Line);
            }
            if ui
                .selectable_label(
                    self.options.communication_mode == CommunicationMode::Local,
                    "LOCAL",
                )
                .clicked()
            {
                self.change_communication_mode(ui.ctx(), CommunicationMode::Local);
            }
            ui.label("Printer");
            let printer_label = if self.options.printer_enabled {
                "ON"
            } else {
                "OFF"
            };
            if ui.button(printer_label).clicked() {
                self.options.printer_enabled = !self.options.printer_enabled;
                self.submit(
                    ui.ctx(),
                    ApplicationCommand::SetPrinterEnabled(self.options.printer_enabled),
                );
            }
            ui.separator();
            let connected = connection == ConnectionState::Connected;
            ui.label(if connected {
                "● Connected"
            } else {
                "○ Disconnected"
            });
            let port_response = ui.add(
                egui::TextEdit::singleline(&mut self.options.serial_config.port)
                    .desired_width(70.0),
            );
            if port_response.has_focus() {
                self.keyboard_target = KeyboardTarget::UiText;
            }
            egui::ComboBox::from_id_salt("serial-port-list")
                .selected_text("Ports")
                .show_ui(ui, |ui| {
                    for port in &self.available_ports {
                        ui.selectable_value(
                            &mut self.options.serial_config.port,
                            port.clone(),
                            port,
                        );
                    }
                });
            if ui
                .small_button("↻")
                .on_hover_text("Refresh serial ports")
                .clicked()
            {
                self.refresh_ports();
            }
            if !connected && ui.button("Connect").clicked() {
                self.connect_selected();
            }
            if connected && ui.button("Disconnect").clicked() {
                self.disconnect();
            }
            ui.separator();
            let theme_label = match self.theme {
                ThemeKind::Light => "Light",
                ThemeKind::Dark => "Dark",
            };
            if ui
                .button(theme_label)
                .on_hover_text("Toggle Light/Dark theme")
                .clicked()
            {
                self.theme = match self.theme {
                    ThemeKind::Light => ThemeKind::Dark,
                    ThemeKind::Dark => ThemeKind::Light,
                };
                self.theme.apply(ui.ctx());
                ui.ctx().request_repaint();
            }
        });
        let error = self
            .transport_error
            .as_ref()
            .or(self.port_error.as_ref())
            .or(self.tape_error.as_ref())
            .cloned();
        if let Some(error) = error {
            ui.horizontal(|ui| {
                ui.colored_label(palette.error, error);
                if ui
                    .small_button("×")
                    .on_hover_text("Dismiss message")
                    .clicked()
                {
                    self.transport_error = None;
                    self.port_error = None;
                    self.tape_error = None;
                }
            });
        }
    }

    fn terminal(&mut self, ui: &mut egui::Ui) {
        let focus_response = ui.interact(
            ui.available_rect_before_wrap(),
            ui.id().with("terminal-keyboard-target"),
            egui::Sense::click(),
        );
        if focus_response.clicked() {
            focus_response.request_focus();
            self.keyboard_target = KeyboardTarget::Terminal;
        }
        let wheel = ui.ctx().input(|input| input.smooth_scroll_delta.y);
        if ui.rect_contains_pointer(ui.max_rect()) && wheel != 0.0 {
            let bottom = self
                .runtime
                .terminal()
                .line_history()
                .len()
                .saturating_sub(self.runtime.terminal().height());
            self.scroll_top = history_scroll_target(self.scroll_top, bottom, wheel);
            self.scroll_request = true;
        }
        let scroll_top = self.scroll_top;
        let scroll_request = self.scroll_request;
        self.scroll_request = false;
        let terminal = self.runtime.terminal();
        let history = terminal.line_history();
        let (cursor_column, cursor_line) = terminal.cursor_position();
        let palette = self.theme.palette();
        let font = FontId::new(self.options.font_size, FontFamily::Name(FONT_NAME.into()));
        let measured_width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap("M".to_owned(), font.clone(), palette.text)
                .size()
                .x
        });
        let metrics = TerminalMetrics::from_measured_width(measured_width, 1.0);
        let row_height = metrics.cell_height;
        let cell_width = metrics.cell_width;
        let rendered_rows = history.len().max(terminal.height());
        let grid_size = egui::vec2(
            cell_width * terminal.width() as f32,
            row_height * rendered_rows as f32,
        );

        egui::Frame::new()
            .fill(palette.paper)
            .inner_margin(metrics.margin)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .stick_to_bottom(scroll_top.is_none())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(grid_size, egui::Sense::hover());
                        if scroll_request {
                            let row = scroll_top.unwrap_or_else(|| history.len().saturating_sub(1));
                            let target = egui::Rect::from_min_size(
                                egui::pos2(rect.left(), rect.top() + row as f32 * row_height),
                                egui::vec2(cell_width, row_height),
                            );
                            ui.scroll_to_rect(target, Some(egui::Align::TOP));
                        }
                        let painter = ui.painter_at(rect);
                        for (row, line) in history.lines().iter().enumerate() {
                            let y = rect.top() + row as f32 * row_height;
                            for column in 0..line.width() {
                                let position =
                                    egui::pos2(rect.left() + column as f32 * cell_width, y);
                                for character in line.strike_stack(column) {
                                    painter.text(
                                        position,
                                        Align2::LEFT_TOP,
                                        character,
                                        font.clone(),
                                        palette.text,
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
                                    egui::Stroke::new(1.0, palette.cursor),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }
                    });
            });
        if self.scroll_top.is_some() {
            let rect = ui.max_rect();
            let badge = egui::Rect::from_min_size(
                rect.right_top() + egui::vec2(-142.0, 8.0),
                egui::vec2(134.0, 24.0),
            );
            ui.painter().rect_filled(badge, 3.0, palette.text);
            ui.painter().text(
                badge.center(),
                Align2::CENTER_CENTER,
                "VIEWING HISTORY",
                FontId::proportional(12.0),
                palette.paper,
            );
        }
    }

    fn reader_contents(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Load").clicked()
                && let Some(path) = reader_dialog(&self.options.tape_reader.initial_file_path)
            {
                match load_reader_file(&path) {
                    Ok(tape) => {
                        self.reader.clear_pending();
                        self.reader.reader_mut().load(tape);
                        self.reader_view.reset();
                        self.reader_seek_position = 0;
                        self.reader_path = Some(path);
                        self.tape_error = None;
                    }
                    Err(error) => self.tape_error = Some(format!("reader load failed: {error}")),
                }
            }
            if ui
                .add_enabled(
                    self.reader.reader().tape().is_some(),
                    egui::Button::new("Unload"),
                )
                .clicked()
            {
                self.reader.clear_pending();
                self.reader.reader_mut().unload();
                self.reader_view.reset();
                self.reader_seek_position = 0;
                self.reader_path = None;
            }
            if ui
                .add_enabled(
                    self.reader.reader().state() == ReaderState::Stopped,
                    egui::Button::new("ON"),
                )
                .clicked()
            {
                let offline_line = self.options.communication_mode == CommunicationMode::Line
                    && self.runtime.connection_state() != &ConnectionState::Connected;
                if offline_line {
                    self.tape_error =
                        Some("reader cannot start: LINE mode is disconnected".to_owned());
                } else {
                    self.reader.reader_mut().start();
                    self.reader_seek_position = self.reader.reader().position();
                    self.reader_view.follow_reader();
                    self.tape_error = None;
                }
            }
            if ui
                .add_enabled(
                    self.reader.reader().state() == ReaderState::Running,
                    egui::Button::new("OFF"),
                )
                .clicked()
            {
                self.reader.reader_mut().stop();
            }
            if ui
                .add_enabled(
                    self.reader.reader().state() == ReaderState::Stopped
                        && self.reader.reader().position() > 0,
                    egui::Button::new("Rewind"),
                )
                .clicked()
            {
                self.reader.clear_pending();
                self.reader.reader_mut().rewind();
                self.reader_view.follow_reader();
                self.reader_seek_position = 0;
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut self.options.tape_reader.auto_stop, "Auto-stop")
                .changed()
            {
                self.reader
                    .reader_mut()
                    .set_auto_stop(self.options.tape_reader.auto_stop);
            }
            if ui
                .checkbox(
                    &mut self.options.tape_reader.skip_leading_nulls,
                    "Skip leading nulls",
                )
                .changed()
            {
                self.reader
                    .reader_mut()
                    .set_skip_leading_nulls(self.options.tape_reader.skip_leading_nulls);
            }
            if ui
                .checkbox(&mut self.options.tape_reader.set_msb, "Set MSB")
                .changed()
            {
                self.reader
                    .reader_mut()
                    .set_msb(self.options.tape_reader.set_msb);
            }
        });
        let length = self.reader.reader().tape().map_or(0, |tape| tape.len());
        let percent = if length == 0 {
            0.0
        } else {
            100.0 * self.reader.reader().position() as f32 / length as f32
        };
        ui.label(format!(
            "File: {}",
            display_path(self.reader_path.as_deref())
        ));
        ui.label(format!(
            "{} bytes, position {}, {:.1}%, {:?}",
            length,
            self.reader.reader().position(),
            percent,
            self.reader.reader().stop_cause()
        ));
        let stopped = self.reader.reader().state() == ReaderState::Stopped;
        ui.horizontal(|ui| {
            ui.label("Position:");
            let length = self.reader.reader().tape().map_or(0, |tape| tape.len());
            ui.add_enabled(
                stopped,
                egui::DragValue::new(&mut self.reader_seek_position).range(0..=length),
            )
            .on_hover_text("Stored byte offset; changing this does not transmit data");
            if ui.add_enabled(stopped, egui::Button::new("Go")).clicked() {
                self.seek_reader(self.reader_seek_position);
            }
            if tape_step_button(ui, TapeStepDirection::TowardStart, stopped)
                .on_hover_text("Move tape one byte toward start")
                .clicked()
            {
                self.seek_reader(stepped_position(
                    self.reader.reader().position(),
                    length,
                    TapeStepDirection::TowardStart,
                ));
            }
            if tape_step_button(ui, TapeStepDirection::TowardEnd, stopped)
                .on_hover_text("Move tape one byte toward end")
                .clicked()
            {
                self.seek_reader(stepped_position(
                    self.reader.reader().position(),
                    length,
                    TapeStepDirection::TowardEnd,
                ));
            }
            if ui.button("Follow reader").clicked() {
                self.reader_view.follow_reader();
            }
            if !self.reader_view.follows_reader() {
                ui.label("Viewing tape history");
            }
        });
        let requested_seek = if let Some(tape) = self.reader.reader().tape() {
            render_reader_tape(
                ui,
                tape.bytes(),
                self.reader.reader().position(),
                self.reader.reader().state() == ReaderState::Running,
                self.options.tape_reader.set_msb,
                &mut self.reader_view,
                TapeRendererOptions {
                    max_rows: self.options.tape_reader.max_rows,
                    ghost_outline: self.options.tape_reader.ghost_outline,
                    bit_label_base: self.options.tape_reader.bit_label_base,
                    ascii_char_mask_msb: self.options.tape_reader.ascii_char_mask_msb,
                    source_order: TapeSourceOrder::NewestFirst,
                    mark_newest: false,
                    palette: self.theme.palette(),
                },
            )
        } else {
            None
        };
        if let Some(position) = requested_seek {
            self.seek_reader(position);
        }
    }

    fn seek_reader(&mut self, position: usize) {
        match self.reader.seek(position) {
            Ok(()) => {
                self.reader_seek_position = position;
                self.reader_view.follow_reader();
                self.tape_error = None;
            }
            Err(error) => self.tape_error = Some(format!("reader seek failed: {error:?}")),
        }
    }

    fn punch_contents(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Select/Load").clicked()
                && let Some(path) = punch_dialog(&self.options.tape_punch.initial_file_path)
            {
                match PunchFile::open(&path, self.punch_mode) {
                    Ok(punch) => {
                        self.punch = Some(punch);
                        self.tape_error = None;
                    }
                    Err(error) => self.tape_error = Some(format!("punch open failed: {error}")),
                }
            }
            if ui
                .add_enabled(self.punch.is_some(), egui::Button::new("Unload"))
                .clicked()
            {
                self.punch = None;
            }
            let stopped = self
                .punch
                .as_ref()
                .is_some_and(|p| p.state() != crate::core::paper_tape::PunchState::Running);
            if ui.add_enabled(stopped, egui::Button::new("ON")).clicked()
                && let Some(punch) = self.punch.as_mut()
            {
                punch.start();
            }
            let running = self
                .punch
                .as_ref()
                .is_some_and(|p| p.state() == crate::core::paper_tape::PunchState::Running);
            if ui.add_enabled(running, egui::Button::new("OFF")).clicked()
                && let Some(punch) = self.punch.as_mut()
            {
                punch.stop();
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (mode, label) in [
                (PunchMode::Append, "Append"),
                (PunchMode::Overwrite, "Overwrite"),
            ] {
                if ui
                    .selectable_label(self.punch_mode == mode, label)
                    .clicked()
                {
                    self.punch_mode = mode;
                    if let Some(punch) = self.punch.as_mut() {
                        punch.set_mode(mode);
                    }
                }
            }
        });
        let (path, bytes) = self
            .punch
            .as_ref()
            .map_or((None, &[][..]), |p| (Some(p.path()), p.bytes()));
        ui.label(format!("File: {}", display_path(path)));
        ui.label(format!(
            "{} bytes, {:?}",
            bytes.len(),
            self.punch.as_ref().map(PunchFile::state)
        ));
        render_tape(
            ui,
            bytes,
            TapeRendererOptions {
                max_rows: self.options.tape_punch.max_rows,
                ghost_outline: self.options.tape_punch.ghost_outline,
                bit_label_base: self.options.tape_punch.bit_label_base,
                ascii_char_mask_msb: self.options.tape_punch.ascii_char_mask_msb,
                source_order: TapeSourceOrder::OldestFirst,
                mark_newest: false,
                palette: self.theme.palette(),
            },
        );
    }

    fn panel_header(ui: &mut egui::Ui, placement: &mut PanelPresentation) {
        ui.horizontal(|ui| {
            if placement.placement() == PanelPlacement::Docked {
                if ui.small_button("Undock").clicked() {
                    placement.undock();
                }
            } else if ui.small_button("Dock").clicked() {
                placement.dock();
            }
            if ui.small_button("Hide").clicked() {
                placement.hide();
            }
        });
    }

    fn docked_tapes(&mut self, ui: &mut egui::Ui) {
        let punch_docked = self.punch_panel.placement() == PanelPlacement::Docked;
        let reader_docked = self.reader_panel.placement() == PanelPlacement::Docked;
        let both = punch_docked && reader_docked;
        let splitter_height = if both { 8.0 } else { 0.0 };
        let usable_height = (ui.available_height() - splitter_height).max(0.0);
        let (punch_height, reader_height) = self.dock_split.pane_heights(usable_height, both);
        if punch_docked {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), punch_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(egui::Vec2::ZERO);
                    ui.set_clip_rect(ui.max_rect());
                    ui.heading("Paper Tape Punch");
                    Self::panel_header(ui, &mut self.punch_panel);
                    self.punch_contents(ui);
                },
            );
        }
        if both {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), splitter_height),
                egui::Sense::drag(),
            );
            let response = response.on_hover_cursor(egui::CursorIcon::ResizeVertical);
            ui.painter().hline(
                rect.x_range(),
                rect.center().y,
                egui::Stroke::new(2.0, self.theme.palette().border),
            );
            if response.drag_started() {
                self.dock_split.begin_drag();
            }
            if response.dragged() {
                if let Some(delta) = response.total_drag_delta() {
                    self.dock_split.drag_from_origin(delta.y, usable_height);
                }
                ui.ctx().request_repaint();
            }
            if response.drag_stopped() {
                self.dock_split.end_drag();
            }
        }
        if reader_docked {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), reader_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(egui::Vec2::ZERO);
                    ui.set_clip_rect(ui.max_rect());
                    ui.heading("Paper Tape Reader");
                    Self::panel_header(ui, &mut self.reader_panel);
                    self.reader_contents(ui);
                },
            );
        }
    }

    fn undocked_tapes(&mut self, context: &egui::Context) {
        let palette = self.theme.palette();
        let window_frame = egui::Frame::new()
            .fill(palette.paper)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .inner_margin(10.0);
        if self.reader_panel.placement() == PanelPlacement::Undocked {
            let mut open = true;
            egui::Window::new("Paper Tape Reader")
                .open(&mut open)
                .default_width(330.0)
                .frame(window_frame)
                .show(context, |ui| {
                    Self::panel_header(ui, &mut self.reader_panel);
                    self.reader_contents(ui);
                });
            if !open {
                self.reader_panel.hide();
            }
        }
        if self.punch_panel.placement() == PanelPlacement::Undocked {
            let mut open = true;
            egui::Window::new("Paper Tape Punch")
                .open(&mut open)
                .default_width(330.0)
                .frame(window_frame)
                .show(context, |ui| {
                    Self::panel_header(ui, &mut self.punch_panel);
                    self.punch_contents(ui);
                });
            if !open {
                self.punch_panel.hide();
            }
        }
    }

    fn shutdown(&mut self) {
        if self.shutdown_complete {
            return;
        }
        self.reader.reader_mut().stop();
        self.reader.reader_mut().unload();
        self.punch = None;
        if let Err(error) = self.runtime.shutdown().and_then(|()| self.runtime.join()) {
            self.transport_error = Some(error.to_string());
        }
        self.shutdown_complete = true;
    }
}

impl eframe::App for EguiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme
            .palette()
            .app_background
            .to_normalized_gamma_f32()
    }

    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if context.input(|input| input.viewport().close_requested()) {
            self.shutdown();
            return;
        }
        self.drive_runtime(context);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let title = match self.runtime.connection_state() {
            ConnectionState::Connected => format!(
                "ASR-33 Emulator using {}:{}",
                self.options.serial_config.port, self.options.serial_config.baudrate
            ),
            _ => "ASR-33 Emulator — Disconnected".to_owned(),
        };
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
        egui::Panel::bottom("operation-bar")
            .resizable(false)
            .show(ui, |ui| self.controls(ui));
        if self.reader_panel.placement() == PanelPlacement::Docked
            || self.punch_panel.placement() == PanelPlacement::Docked
        {
            let viewport_width = ui.available_width();
            let maximum = (viewport_width * 0.55).max(230.0);
            let response = egui::Panel::left("paper-tape-dock")
                .default_size(self.dock_width.width())
                .min_size(230.0)
                .max_size(maximum)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(self.theme.palette().paper)
                        .inner_margin(10.0),
                )
                .show(ui, |ui| self.docked_tapes(ui));
            self.dock_width
                .retain_requested(response.response.rect.width(), viewport_width);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.theme.palette().app_background))
            .show(ui, |ui| self.terminal(ui));
        self.undocked_tapes(ui.ctx());
        self.handle_keyboard(ui.ctx());
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

fn display_path(path: Option<&Path>) -> String {
    path.map_or_else(|| "(none)".to_owned(), |path| path.display().to_string())
}

#[cfg(windows)]
fn reader_dialog(initial_directory: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(initial_directory)
        .add_filter(
            "Tape files",
            &["pt", "pb", "pa", "pr", "bpt", "apt", "rpt", "tap"],
        )
        .add_filter("Source files", &["pa", "ba", "ft", "fc", "tx"])
        .add_filter("Misc files", &["raw", "asc", "s19", "S29", "srec"])
        .add_filter("All files", &["*"])
        .pick_file()
}

#[cfg(not(windows))]
fn reader_dialog(_initial_directory: &Path) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn punch_dialog(initial_directory: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(initial_directory)
        .set_file_name("tape.pt")
        .add_filter(
            "Tape files",
            &["pt", "pb", "pa", "pr", "bpt", "apt", "rpt", "tap"],
        )
        .add_filter("Source files", &["pa", "ba", "ft", "fc", "tx"])
        .add_filter("Misc files", &["raw", "asc", "s19", "S29", "srec"])
        .add_filter("All files", &["*"])
        .save_file()
}

#[cfg(not(windows))]
fn punch_dialog(_initial_directory: &Path) -> Option<PathBuf> {
    None
}

fn history_scroll_target(current: Option<usize>, bottom: usize, wheel: f32) -> Option<usize> {
    let current = current.unwrap_or(bottom);
    let next = if wheel > 0.0 {
        current.saturating_sub(3)
    } else {
        current.saturating_add(3).min(bottom)
    };
    (next < bottom).then_some(next)
}

fn keyboard_input_for_event(event: &egui::Event) -> Option<KeyboardInput> {
    match event {
        egui::Event::Text(text) => Some(KeyboardInput::Text(text.clone())),
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => map_key(*key, *modifiers),
        _ => None,
    }
}

#[must_use]
const fn should_send_to_terminal(target: KeyboardTarget) -> bool {
    matches!(target, KeyboardTarget::Terminal)
}

fn keyboard_input_for_target(target: KeyboardTarget, event: &egui::Event) -> Option<KeyboardInput> {
    should_send_to_terminal(target)
        .then(|| keyboard_input_for_event(event))
        .flatten()
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

#[must_use]
fn stepped_position(position: usize, tape_length: usize, direction: TapeStepDirection) -> usize {
    match direction {
        TapeStepDirection::TowardStart => position.saturating_sub(1),
        TapeStepDirection::TowardEnd => position.saturating_add(1).min(tape_length),
    }
}

fn tape_step_button(
    ui: &mut egui::Ui,
    direction: TapeStepDirection,
    enabled: bool,
) -> egui::Response {
    let size = egui::vec2(24.0, 20.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        visuals.bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let center = rect.center();
    let points = match direction {
        TapeStepDirection::TowardStart => [
            egui::pos2(center.x, center.y - 4.0),
            egui::pos2(center.x - 5.0, center.y + 3.0),
            egui::pos2(center.x + 5.0, center.y + 3.0),
        ],
        TapeStepDirection::TowardEnd => [
            egui::pos2(center.x, center.y + 4.0),
            egui::pos2(center.x - 5.0, center.y - 3.0),
            egui::pos2(center.x + 5.0, center.y - 3.0),
        ],
    };
    let color = if enabled {
        visuals.fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points.to_vec(),
        color,
        egui::Stroke::NONE,
    ));
    response
}

fn apply_tape_shortcut(
    key: egui::Key,
    reader: &mut PanelPresentation,
    punch: &mut PanelPresentation,
) -> bool {
    match key {
        egui::Key::F1 => reader.show(),
        egui::Key::F2 => reader.hide(),
        egui::Key::F3 => punch.show(),
        egui::Key::F4 => punch.hide(),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        KeyboardTarget, TapeStepDirection, apply_tape_shortcut, history_scroll_target,
        keyboard_input_for_event, keyboard_input_for_target, repaint_delay,
        should_send_to_terminal, stepped_position,
    };
    use crate::adapters::paper_tape::PunchFile;
    use crate::app::PumpStatus;
    use crate::core::paper_tape::{PaperTape, PunchMode, ReaderOptions, ReaderState, TapeReader};
    use crate::ui::keyboard::KeyboardInput;
    use crate::ui::layout::{PanelPlacement, PanelPresentation};
    use eframe::egui::{Event, Key, Modifiers};
    use std::time::Duration;

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

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
    }

    #[test]
    fn printable_physical_input_uses_only_the_text_route() {
        let events = [
            key_event(Key::A, Modifiers::NONE),
            Event::Text("a".to_owned()),
        ];
        let inputs = events
            .iter()
            .filter_map(keyboard_input_for_event)
            .collect::<Vec<_>>();
        assert_eq!(inputs, [KeyboardInput::Text("a".to_owned())]);
    }

    #[test]
    fn return_backspace_and_tab_each_use_only_the_key_route() {
        for (key, expected) in [
            (Key::Enter, KeyboardInput::Return),
            (Key::Backspace, KeyboardInput::Backspace),
            (Key::Tab, KeyboardInput::Tab),
        ] {
            assert_eq!(
                keyboard_input_for_event(&key_event(key, Modifiers::NONE)),
                Some(expected)
            );
        }
    }

    #[test]
    fn ctrl_letter_uses_key_route_and_clipboard_events_are_not_transmitted() {
        let control_a = key_event(
            Key::A,
            Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
        );
        assert_eq!(
            keyboard_input_for_event(&control_a),
            Some(KeyboardInput::Control('A'))
        );
        assert_eq!(keyboard_input_for_event(&Event::Copy), None);
        assert_eq!(keyboard_input_for_event(&Event::Cut), None);
        assert_eq!(keyboard_input_for_event(&Event::Paste("x".into())), None);
    }

    #[test]
    fn paper_tape_and_legacy_shortcuts_never_become_terminal_input() {
        for key in [
            Key::F1,
            Key::F2,
            Key::F3,
            Key::F4,
            Key::F5,
            Key::F6,
            Key::F7,
            Key::F8,
            Key::F9,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
        ] {
            assert_eq!(
                keyboard_input_for_event(&key_event(key, Modifiers::NONE)),
                None,
                "{key:?} must not transmit bytes"
            );
        }
    }

    #[test]
    fn explicit_keyboard_target_routes_text_and_releases_after_terminal_click() {
        assert!(!should_send_to_terminal(KeyboardTarget::UiText));
        assert!(should_send_to_terminal(KeyboardTarget::Terminal));
        for character in "COM5".chars() {
            assert_eq!(
                keyboard_input_for_target(
                    KeyboardTarget::UiText,
                    &Event::Text(character.to_string())
                ),
                None
            );
        }
        assert_eq!(
            keyboard_input_for_target(KeyboardTarget::Terminal, &Event::Text("A".to_owned())),
            Some(KeyboardInput::Text("A".to_owned()))
        );
    }

    #[test]
    fn f1_f2_and_f3_f4_preserve_tape_visibility_semantics() {
        let mut reader = PanelPresentation::docked();
        let mut punch = PanelPresentation::docked();
        assert!(apply_tape_shortcut(Key::F2, &mut reader, &mut punch));
        assert_eq!(reader.placement(), PanelPlacement::Hidden);
        assert!(apply_tape_shortcut(Key::F1, &mut reader, &mut punch));
        assert_eq!(reader.placement(), PanelPlacement::Docked);
        punch.undock();
        assert!(apply_tape_shortcut(Key::F4, &mut reader, &mut punch));
        assert_eq!(punch.placement(), PanelPlacement::Hidden);
        assert!(apply_tape_shortcut(Key::F3, &mut reader, &mut punch));
        assert_eq!(punch.placement(), PanelPlacement::Undocked);
    }

    #[test]
    fn mouse_history_scroll_enters_and_leaves_history_view() {
        assert_eq!(history_scroll_target(None, 20, 1.0), Some(17));
        assert_eq!(history_scroll_target(Some(17), 20, -1.0), None);
    }

    #[test]
    fn placement_changes_preserve_real_reader_and_punch_state() {
        let mut reader = TapeReader::new(ReaderOptions::default());
        reader.load(PaperTape::new(b"ABC".to_vec()));
        assert!(reader.start());
        let _ = reader.step();
        let directory = tempfile::tempdir().expect("temporary tape directory");
        let mut punch = PunchFile::open(&directory.path().join("out.pt"), PunchMode::Append)
            .expect("punch file opens");
        assert!(punch.start());
        let mut placement = PanelPresentation::docked();
        placement.undock();
        placement.hide();
        placement.show();
        assert_eq!(reader.position(), 1);
        assert_eq!(reader.state(), ReaderState::Running);
        assert_eq!(punch.state(), crate::core::paper_tape::PunchState::Running);
    }

    #[test]
    fn tape_step_buttons_move_one_byte_and_clamp_to_loaded_tape() {
        assert_eq!(stepped_position(0, 6, TapeStepDirection::TowardStart), 0);
        assert_eq!(stepped_position(4, 6, TapeStepDirection::TowardStart), 3);
        assert_eq!(stepped_position(4, 6, TapeStepDirection::TowardEnd), 5);
        assert_eq!(stepped_position(6, 6, TapeStepDirection::TowardEnd), 6);
    }
}
