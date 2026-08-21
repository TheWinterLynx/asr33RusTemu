use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId};

use crate::adapters::paper_tape::{PunchFile, load_reader_file};
use crate::adapters::transport::serial::SerialTransport;
use crate::app::paper_tape::{FeedResult, READER_FEED_INTERVAL, ReaderFeed};
use crate::app::{
    AppRuntime, ConnectionState, ImmediateTransmit, PumpStatus, RuntimeEvent, Scheduler,
    SystemScheduler,
};
use crate::core::config::{
    BitLabelBase, PunchConfigMode, SerialConfig, TapePunchConfig, TapeReaderConfig,
};
use crate::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};
use crate::core::paper_tape::{PunchMode, ReaderOptions, ReaderState, TapeReader};

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
    reader_visible: bool,
    punch_visible: bool,
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
            reader_visible: false,
            punch_visible: false,
            tape_error: None,
            scroll_top: None,
            scroll_request: false,
            available_ports: Vec::new(),
            port_error: None,
        };
        application.refresh_ports();
        application.connect_selected();
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
        match self.runtime.set_communication_mode(mode) {
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
            let logical = keyboard_input_for_event(event);
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
        match key {
            egui::Key::F1 => self.reader_visible = true,
            egui::Key::F2 => self.reader_visible = false,
            egui::Key::F3 => self.punch_visible = true,
            egui::Key::F4 => self.punch_visible = false,
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
        ui.horizontal(|ui| {
            ui.label(format!("Connection: {connection:?}"));
            ui.separator();
            ui.label("Port:");
            ui.text_edit_singleline(&mut self.options.serial_config.port);
            egui::ComboBox::from_id_salt("serial-port-list")
                .selected_text("available")
                .show_ui(ui, |ui| {
                    for port in &self.available_ports {
                        ui.selectable_value(
                            &mut self.options.serial_config.port,
                            port.clone(),
                            port,
                        );
                    }
                });
            if ui.button("Refresh ports").clicked() {
                self.refresh_ports();
            }
            if connection != ConnectionState::Connected && ui.button("Connect").clicked() {
                self.connect_selected();
            }
            if connection == ConnectionState::Connected && ui.button("Disconnect").clicked() {
                self.disconnect();
            }
        });
        ui.label(format!(
            "{} baud, {:?} data bits, {:?} parity, {:?} stop bits",
            self.options.serial_config.baudrate,
            self.options.serial_config.databits,
            self.options.serial_config.parity,
            self.options.serial_config.stopbits
        ));
        if let ConnectionState::Failed { message } = &connection {
            ui.colored_label(
                Color32::LIGHT_RED,
                format!("Last connection error: {message}"),
            );
        }
        if let Some(error) = &self.port_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
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
            ui.separator();
            if ui
                .selectable_label(
                    self.options.throttle_mode == ThrottleMode::Throttled,
                    "THROTTLED",
                )
                .clicked()
            {
                self.options.throttle_mode = ThrottleMode::Throttled;
                self.submit(
                    ui.ctx(),
                    ApplicationCommand::SetThrottleMode(ThrottleMode::Throttled),
                );
            }
            if ui
                .selectable_label(
                    self.options.throttle_mode == ThrottleMode::Unthrottled,
                    "UNTHROTTLED",
                )
                .clicked()
            {
                self.options.throttle_mode = ThrottleMode::Unthrottled;
                self.submit(
                    ui.ctx(),
                    ApplicationCommand::SetThrottleMode(ThrottleMode::Unthrottled),
                );
            }
            ui.separator();
            let printer_label = if self.options.printer_enabled {
                "PRINTER ON"
            } else {
                "PRINTER OFF"
            };
            if ui.button(printer_label).clicked() {
                self.options.printer_enabled = !self.options.printer_enabled;
                self.submit(
                    ui.ctx(),
                    ApplicationCommand::SetPrinterEnabled(self.options.printer_enabled),
                );
            }
        });
        if let Some(error) = &self.transport_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(error) = &self.tape_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    fn terminal(&mut self, ui: &mut egui::Ui) {
        let scroll_top = self.scroll_top;
        let scroll_request = self.scroll_request;
        self.scroll_request = false;
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

    fn paper_tape_windows(&mut self, context: &egui::Context) {
        let mut reader_open = self.reader_visible;
        egui::Window::new("Paper Tape Reader")
            .open(&mut reader_open)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Load").clicked()
                        && let Some(path) =
                            reader_dialog(&self.options.tape_reader.initial_file_path)
                    {
                        match load_reader_file(&path) {
                            Ok(tape) => {
                                self.reader.clear_pending();
                                self.reader.reader_mut().load(tape);
                                self.reader_path = Some(path);
                                self.tape_error = None;
                            }
                            Err(error) => {
                                self.tape_error = Some(format!("reader load failed: {error}"))
                            }
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
                        self.reader_path = None;
                    }
                    if ui
                        .add_enabled(
                            self.reader.reader().state() == ReaderState::Stopped,
                            egui::Button::new("ON"),
                        )
                        .clicked()
                    {
                        let offline_line = self.options.communication_mode
                            == CommunicationMode::Line
                            && self.runtime.connection_state() != &ConnectionState::Connected;
                        if offline_line {
                            self.tape_error =
                                Some("reader cannot start: LINE mode is disconnected".to_owned());
                        } else {
                            self.reader.reader_mut().start();
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
                    }
                });
                ui.horizontal(|ui| {
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
                if let Some(tape) = self.reader.reader().tape() {
                    tape_view(
                        ui,
                        tape.bytes(),
                        self.reader.reader().position(),
                        self.options.tape_reader.max_rows,
                        self.options.tape_reader.ghost_outline,
                        self.options.tape_reader.bit_label_base,
                        self.options.tape_reader.ascii_char_mask_msb,
                    );
                }
            });
        self.reader_visible = reader_open;

        let mut punch_open = self.punch_visible;
        egui::Window::new("Paper Tape Punch")
            .open(&mut punch_open)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Select/Load").clicked()
                        && let Some(path) = punch_dialog(&self.options.tape_punch.initial_file_path)
                    {
                        match PunchFile::open(&path, self.punch_mode) {
                            Ok(punch) => {
                                self.punch = Some(punch);
                                self.tape_error = None;
                            }
                            Err(error) => {
                                self.tape_error = Some(format!("punch open failed: {error}"))
                            }
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
                ui.horizontal(|ui| {
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
                tape_view(
                    ui,
                    bytes,
                    bytes.len(),
                    self.options.tape_punch.max_rows,
                    self.options.tape_punch.ghost_outline,
                    self.options.tape_punch.bit_label_base,
                    self.options.tape_punch.ascii_char_mask_msb,
                );
            });
        self.punch_visible = punch_open;
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
        self.paper_tape_windows(ui.ctx());
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
        .add_filter("Paper tape", &["pt", "bin"])
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
        .save_file()
}

#[cfg(not(windows))]
fn punch_dialog(_initial_directory: &Path) -> Option<PathBuf> {
    None
}

fn visible_tape_rows(bytes: &[u8], max_rows: usize) -> (usize, &[u8]) {
    let start = bytes.len().saturating_sub(max_rows);
    (start, &bytes[start..])
}

fn tape_view(
    ui: &mut egui::Ui,
    bytes: &[u8],
    position: usize,
    max_rows: usize,
    ghost_outline: bool,
    bit_label_base: BitLabelBase,
    ascii_char_mask_msb: bool,
) {
    let (start, visible) = visible_tape_rows(bytes, max_rows);
    let labels = match bit_label_base {
        BitLabelBase::Zero => ["7", "6", "5", "4", "3", "2", "1", "0"],
        BitLabelBase::One => ["8", "7", "6", "5", "4", "3", "2", "1"],
    };
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id())
                .striped(true)
                .show(ui, |ui| {
                    ui.label("");
                    ui.label("offset");
                    for label in labels {
                        ui.label(label);
                    }
                    ui.label("feed");
                    ui.label("ASCII");
                    ui.end_row();
                    for (relative, byte) in visible.iter().copied().enumerate() {
                        let offset = start + relative;
                        ui.label(if offset == position { "▶" } else { "" });
                        ui.monospace(format!("{offset:06}"));
                        for bit in (0..8).rev() {
                            let punched = byte & (1 << bit) != 0;
                            let mark = if punched {
                                "●"
                            } else if ghost_outline {
                                "○"
                            } else {
                                " "
                            };
                            ui.monospace(mark);
                        }
                        ui.monospace("•");
                        let character = if ascii_char_mask_msb {
                            byte & 0x7f
                        } else {
                            byte
                        };
                        let display = if character.is_ascii_graphic() || character == b' ' {
                            char::from(character).to_string()
                        } else {
                            "·".to_owned()
                        };
                        ui.monospace(display);
                        ui.end_row();
                    }
                });
        });
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
    use super::{keyboard_input_for_event, repaint_delay, visible_tape_rows};
    use crate::app::PumpStatus;
    use crate::ui::keyboard::KeyboardInput;
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
    fn tape_visualization_keeps_only_the_configured_tail() {
        let bytes = [0, 1, 2, 3, 4];
        assert_eq!(visible_tape_rows(&bytes, 3), (2, &bytes[2..]));
        assert_eq!(visible_tape_rows(&bytes, 0), (5, &bytes[5..]));
        assert_eq!(visible_tape_rows(&bytes, 99), (0, &bytes[..]));
    }
}
