use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, Align2, FontData, FontDefinitions, FontFamily, FontId};

use crate::adapters::audio::{AudioAvailability, AudioEngine};
use crate::adapters::paper_tape::{PunchFile, load_reader_file};
use crate::adapters::transport::serial::SerialTransport;
use crate::app::config_controller::{ConfigChangePlan, SettingsState, serial_reconnect_required};
use crate::app::paper_tape::{FeedResult, READER_FEED_INTERVAL, ReaderFeed};
use crate::app::{
    AppRuntime, ConnectionState, ImmediateTransmit, PumpStatus, RuntimeEvent, Scheduler,
    SystemScheduler,
};
use crate::core::config::{
    AppConfig, BackendKind, InputReturnMode, LidState, MuteState, PunchConfigMode, SerialConfig,
    TapePunchConfig, TapeReaderConfig, TerminalMode, ThrottleMode as ConfigThrottleMode,
};
use crate::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};
use crate::core::paper_tape::{
    PunchMode, PunchState, ReaderOptions, ReaderState, StopCause, TapeReader,
};

use super::keyboard::{KeyboardInput, KeyboardOptions, encode_input};
use super::layout::{
    DockSplitState, DockWidthState, PanelPlacement, PanelPresentation, TerminalMetrics,
};
use super::settings::{AppView, SettingsAction, SettingsUiMetrics, SettingsView};
use super::tape_view::{
    ReaderTapeViewState, TapePanelMetrics, TapeRendererOptions, TapeSourceOrder,
    render_reader_tape, render_tape,
};
use super::theme::ThemeKind;

const FONT_NAME: &str = "teletype-33";
const IDLE_POLL: Duration = Duration::from_millis(16);
const BACKPRESSURE_RETRY: Duration = Duration::from_millis(10);
const PASTE_REQUEST_TIMEOUT_SECONDS: f64 = 1.0;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortRefreshPolicy {
    Silent,
    ReportErrors,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StartupConnectionState {
    connection_state: ConnectionState,
    transport_error: Option<String>,
    active_serial_config: Option<SerialConfig>,
}

#[must_use]
const fn startup_connection_state(
    _backend: BackendKind,
    _desired_serial: &SerialConfig,
) -> StartupConnectionState {
    StartupConnectionState {
        connection_state: ConnectionState::Disconnected,
        transport_error: None,
        active_serial_config: None,
    }
}

#[must_use]
const fn toggled_lid(lid: LidState) -> LidState {
    match lid {
        LidState::Up => LidState::Down,
        LidState::Down => LidState::Up,
    }
}

fn port_enumeration_update(
    result: Result<Vec<String>, String>,
    policy: PortRefreshPolicy,
) -> (Vec<String>, Option<String>) {
    match result {
        Ok(ports) => (ports, None),
        Err(error) if policy == PortRefreshPolicy::ReportErrors => (
            Vec::new(),
            Some(format!("port enumeration failed: {error}")),
        ),
        Err(_) => (Vec::new(), None),
    }
}

fn open_serial_for_explicit_request<T, E>(
    config: SerialConfig,
    open: impl FnOnce(SerialConfig) -> Result<T, E>,
) -> Result<T, E> {
    open(config)
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
    pub backend_kind: BackendKind,
    pub config_path: PathBuf,
    pub disk_config: AppConfig,
    pub applied_config: AppConfig,
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
    settings_state: SettingsState,
    settings_view: SettingsView,
    current_view: AppView,
    active_serial_config: Option<SerialConfig>,
    audio: AudioEngine,
    audio_error: Option<String>,
    sound_muted: bool,
    lid_state: LidState,
    reader_audio_running: bool,
    paste_on_right_click: bool,
    right_click_paste_requested_at: Option<f64>,
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
        let input_return_mode = options.keyboard.return_mode;
        let settings_state = SettingsState::new(
            options.config_path.clone(),
            options.disk_config.clone(),
            options.applied_config.clone(),
        );
        let startup_connection =
            startup_connection_state(options.backend_kind, &options.serial_config);
        debug_assert_eq!(
            runtime.connection_state(),
            &startup_connection.connection_state
        );
        let lid_state = options.applied_config.sound.config.lid;
        let sound_muted = options.applied_config.sound.config.mute_state == MuteState::Muted;
        let paste_on_right_click = options.applied_config.terminal.config.paste_on_right_click;
        let audio = AudioEngine::start(lid_state, sound_muted);
        let mut application = Self {
            runtime,
            options,
            transport_error: startup_connection.transport_error,
            shutdown_complete: false,
            reader: ReaderFeed::new(reader, Duration::ZERO, input_return_mode),
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
            settings_state,
            settings_view: SettingsView::default(),
            current_view: AppView::Terminal,
            active_serial_config: startup_connection.active_serial_config,
            audio,
            audio_error: None,
            sound_muted,
            lid_state,
            reader_audio_running: false,
            paste_on_right_click,
            right_click_paste_requested_at: None,
        };
        application.refresh_ports(PortRefreshPolicy::Silent);
        application.theme.apply(&creation_context.egui_ctx);
        application
    }

    fn refresh_ports(&mut self, policy: PortRefreshPolicy) {
        let result = serialport::available_ports()
            .map(|ports| ports.into_iter().map(|port| port.port_name).collect())
            .map_err(|error| error.to_string());
        (self.available_ports, self.port_error) = port_enumeration_update(result, policy);
    }

    fn set_sound_muted(&mut self, muted: bool) {
        self.sound_muted = muted;
        self.audio.set_muted(muted);
    }

    fn toggle_sound_muted(&mut self) {
        self.set_sound_muted(!self.sound_muted);
    }

    fn set_lid_state(&mut self, lid: LidState) {
        self.lid_state = lid;
        self.audio.set_lid(lid);
    }

    fn toggle_lid(&mut self) {
        self.set_lid_state(toggled_lid(self.lid_state));
    }

    fn sync_reader_audio(&mut self) {
        let running = self.reader.reader().state() == ReaderState::Running;
        if running != self.reader_audio_running {
            self.reader_audio_running = running;
            self.audio.set_tape_reader_running(running);
        }
    }

    fn refresh_audio_status(&mut self) {
        if let Some(status) = self.audio.refresh_status() {
            match status {
                AudioAvailability::Available => self.audio_error = None,
                AudioAvailability::Unavailable(message) => {
                    self.audio_error = Some(format!("audio unavailable: {message}"));
                }
                AudioAvailability::Starting | AudioAvailability::Stopped => {}
            }
        }
    }

    fn connect_selected(&mut self) {
        if self.options.backend_kind != BackendKind::Serial {
            self.transport_error = Some("SSH backend not migrated yet".to_owned());
            return;
        }
        let selected = self.options.serial_config.clone();
        match open_serial_for_explicit_request(selected.clone(), SerialTransport::open) {
            Ok(transport) => match self.runtime.connect(transport) {
                Ok(()) => {
                    self.transport_error = None;
                    self.active_serial_config = Some(selected);
                    self.settings_state.clear_reconnect_required();
                }
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
        self.active_serial_config = None;
    }

    fn submit(&mut self, context: &egui::Context, command: ApplicationCommand) {
        if let Err(error) = self.runtime.submit(command) {
            self.transport_error = Some(error.to_string());
        }
        context.request_repaint();
    }

    fn submit_keyboard_input(
        &mut self,
        context: &egui::Context,
        input: KeyboardInput,
        play_keypress: bool,
    ) {
        match encode_input(&input, self.options.keyboard) {
            Ok(bytes) if !bytes.is_empty() => {
                if play_keypress {
                    self.audio.keypress();
                }
                self.submit(context, ApplicationCommand::Transmit(bytes));
            }
            Ok(_) => {}
            Err(error) => {
                self.transport_error = Some(error.to_string());
                context.request_repaint();
            }
        }
    }

    fn clear_paper(&mut self, context: &egui::Context) {
        match self.runtime.clear_paper() {
            Ok(()) => {
                self.scroll_top = None;
                self.scroll_request = true;
                context.request_repaint();
            }
            Err(error) => {
                self.transport_error = Some(error.to_string());
                context.request_repaint();
            }
        }
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

    fn apply_settings(&mut self, context: &egui::Context) -> Result<(), String> {
        self.settings_state
            .validate_draft()
            .map_err(|error| error.to_string())?;
        let old = self.settings_state.applied_config.clone();
        let new = self.settings_state.draft_config.clone();
        let plan = ConfigChangePlan::between(&old, &new);
        let old_terminal = &old.terminal.config;
        let terminal = &new.terminal.config;
        if old_terminal.mode != terminal.mode {
            self.change_communication_mode(
                context,
                match terminal.mode {
                    TerminalMode::Line => CommunicationMode::Line,
                    TerminalMode::Local => CommunicationMode::Local,
                },
            );
        }
        if old.data_throttle.config.mode != new.data_throttle.config.mode {
            let mode = match new.data_throttle.config.mode {
                ConfigThrottleMode::Throttled => ThrottleMode::Throttled,
                ConfigThrottleMode::Unthrottled => ThrottleMode::Unthrottled,
            };
            self.submit(context, ApplicationCommand::SetThrottleMode(mode));
            self.options.throttle_mode = mode;
        }
        if old.data_throttle.config.send_rate_cps != new.data_throttle.config.send_rate_cps {
            self.submit(
                context,
                ApplicationCommand::SetTxRate(new.data_throttle.config.send_rate_cps),
            );
        }
        if old.data_throttle.config.receive_rate_cps != new.data_throttle.config.receive_rate_cps {
            self.submit(
                context,
                ApplicationCommand::SetRxRate(new.data_throttle.config.receive_rate_cps),
            );
        }
        if old_terminal.no_print != terminal.no_print {
            self.options.printer_enabled = !terminal.no_print;
            self.submit(
                context,
                ApplicationCommand::SetPrinterEnabled(self.options.printer_enabled),
            );
        }
        if old.sound.config.lid != new.sound.config.lid {
            self.set_lid_state(new.sound.config.lid);
        }
        if old.sound.config.mute_state != new.sound.config.mute_state {
            self.set_sound_muted(new.sound.config.mute_state == MuteState::Muted);
        }
        self.options.keyboard.uppercase_only = terminal.keyboard_uppercase_only;
        self.options.keyboard.parity = terminal.keyboard_parity_mode;
        self.options.keyboard.return_mode = terminal.input_return_mode;
        self.paste_on_right_click = terminal.paste_on_right_click;
        self.reader.set_return_mode(terminal.input_return_mode);
        self.options.font_size = terminal.font_size as f32;
        self.options.tape_reader = new.tape_reader.config.clone();
        self.reader
            .reader_mut()
            .set_auto_stop(self.options.tape_reader.auto_stop);
        self.reader
            .reader_mut()
            .set_skip_leading_nulls(self.options.tape_reader.skip_leading_nulls);
        self.reader
            .reader_mut()
            .set_msb(self.options.tape_reader.set_msb);
        self.options.tape_punch = new.tape_punch.config.clone();
        self.punch_mode = match self.options.tape_punch.mode {
            PunchConfigMode::Append => PunchMode::Append,
            PunchConfigMode::Overwrite => PunchMode::Overwrite,
        };
        self.options.serial_config = new.backend.serial_config.clone();
        self.options.backend_kind = new.backend.kind;
        self.theme = self.settings_view.draft_theme;
        self.theme.apply(context);
        self.settings_view.applied_theme = self.theme;
        self.settings_state.commit_apply(&plan);
        self.settings_state.pending_reconnect = self.options.backend_kind == BackendKind::Serial
            && serial_reconnect_required(
                self.active_serial_config.as_ref(),
                &self.options.serial_config,
            );
        context.request_repaint();
        Ok(())
    }

    fn handle_settings_action(&mut self, context: &egui::Context, action: SettingsAction) {
        self.settings_view.error = None;
        match action {
            SettingsAction::Apply => {
                if let Err(error) = self.apply_settings(context) {
                    self.settings_view.error = Some(error);
                }
            }
            SettingsAction::Save => match self.apply_settings(context) {
                Ok(()) => {
                    if let Err(error) = self.settings_state.save_applied() {
                        self.settings_view.error = Some(format!("Applied, save failed: {error}"));
                    }
                }
                Err(error) => self.settings_view.error = Some(error),
            },
            SettingsAction::Cancel => {
                self.settings_state.cancel();
                self.settings_view.draft_theme = self.settings_view.applied_theme;
                self.current_view.show_terminal();
            }
            SettingsAction::Revert => {
                self.settings_state.revert();
                self.settings_view.draft_theme = self.settings_view.applied_theme;
            }
            SettingsAction::Reconnect => {
                if self.options.backend_kind == BackendKind::Serial {
                    self.disconnect();
                    self.connect_selected();
                } else {
                    self.settings_view.error = Some("SSH backend not migrated yet".to_owned());
                }
            }
            SettingsAction::RefreshPorts => self.refresh_ports(PortRefreshPolicy::ReportErrors),
            SettingsAction::Close => self.current_view.show_terminal(),
        }
    }

    fn handle_keyboard(&mut self, context: &egui::Context) {
        let (events, now) = context.input(|input| (input.events.clone(), input.time));
        if !paste_request_is_live(self.right_click_paste_requested_at, now) {
            self.right_click_paste_requested_at = None;
        }
        for event in &events {
            if let egui::Event::Paste(text) = event {
                let requested_at = self.right_click_paste_requested_at.take();
                if paste_request_is_live(requested_at, now) {
                    let normalized = normalize_paste_text(text, self.options.keyboard.return_mode);
                    self.submit_keyboard_input(context, KeyboardInput::Text(normalized), false);
                    continue;
                }
            }
            if self.handle_shortcut(context, event) {
                continue;
            }
            if let Some(input) = keyboard_input_for_target(self.keyboard_target, event) {
                self.submit_keyboard_input(context, input, true);
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
            egui::Key::F6 => self.toggle_sound_muted(),
            egui::Key::F7 => self.toggle_lid(),
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
        if let Some(delay) = self.reader.tick(now, |emission| {
            match runtime.try_transmit(emission.as_slice().to_vec()) {
                Ok(ImmediateTransmit::Accepted) => FeedResult::Accepted,
                Ok(ImmediateTransmit::Backpressured(_)) => FeedResult::Backpressured(emission),
                Ok(ImmediateTransmit::Disconnected(_)) => {
                    reader_disconnected = true;
                    FeedResult::Backpressured(emission)
                }
                Err(_) => FeedResult::Backpressured(emission),
            }
        }) {
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
        while let Some(event) = self.runtime.pop_character_event() {
            self.audio.character(event);
        }
        self.sync_reader_audio();
        self.refresh_audio_status();
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
            ui.label("Sound");
            let sound_label = if self.sound_muted { "Muted" } else { "On" };
            let sound_tooltip = match self.audio.availability() {
                AudioAvailability::Starting => "Audio device is starting (F6 toggles mute)".to_owned(),
                AudioAvailability::Available => "Toggle mechanical audio mute (F6)".to_owned(),
                AudioAvailability::Unavailable(message) => {
                    format!("Audio unavailable: {message}. F6 still changes the saved mute state")
                }
                AudioAvailability::Stopped => "Audio is stopped".to_owned(),
            };
            if ui
                .button(sound_label)
                .on_hover_text(sound_tooltip)
                .clicked()
            {
                self.toggle_sound_muted();
            }
            ui.label("Lid");
            let lid_label = match self.lid_state {
                LidState::Up => "Up",
                LidState::Down => "Down",
            };
            if ui
                .button(lid_label)
                .on_hover_text("Raise/lower the teletype lid and change acoustic samples (F7)")
                .clicked()
            {
                self.toggle_lid();
            }
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
            ui.label("EOL");
            for (mode, label, tooltip) in [
                (
                    InputReturnMode::Cr,
                    "Raw",
                    "Preserve input CR/LF exactly; keyboard Return sends CR",
                ),
                (
                    InputReturnMode::CrLf,
                    "CR+LF",
                    "Normalize keyboard Return and paper-tape line endings to CR+LF",
                ),
            ] {
                if ui
                    .selectable_value(&mut self.options.keyboard.return_mode, mode, label)
                    .on_hover_text(tooltip)
                    .changed()
                {
                    self.reader.set_return_mode(mode);
                }
            }
            ui.label(column_status_label(
                self.runtime.terminal().cursor_position().0,
                self.runtime.terminal().width(),
            ))
            .on_hover_text("Print-head column / terminal width");
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
                self.refresh_ports(PortRefreshPolicy::ReportErrors);
            }
            if !connected
                && ui
                    .add_enabled(
                        self.options.backend_kind == BackendKind::Serial,
                        egui::Button::new("Connect"),
                    )
                    .on_disabled_hover_text("SSH backend not migrated yet")
                    .clicked()
            {
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
            .or(self.audio_error.as_ref())
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
                    self.audio_error = None;
                }
            });
        }
    }

    fn sync_controls_to_applied(&mut self) {
        let serial_changed =
            self.settings_state.applied_config.backend.serial_config != self.options.serial_config;
        let communication_mode = self.options.communication_mode;
        let throttle_mode = self.options.throttle_mode;
        let printer_enabled = self.options.printer_enabled;
        let keyboard = self.options.keyboard;
        let paste_on_right_click = self.paste_on_right_click;
        let tape_reader = self.options.tape_reader.clone();
        let mut tape_punch = self.options.tape_punch.clone();
        tape_punch.mode = match self.punch_mode {
            PunchMode::Append => PunchConfigMode::Append,
            PunchMode::Overwrite => PunchConfigMode::Overwrite,
        };
        let serial = self.options.serial_config.clone();
        let lid_state = self.lid_state;
        let mute_state = if self.sound_muted {
            MuteState::Muted
        } else {
            MuteState::Unmuted
        };
        self.settings_state
            .update_applied_from_live_control(|config| {
                config.terminal.config.mode = match communication_mode {
                    CommunicationMode::Line => TerminalMode::Line,
                    CommunicationMode::Local => TerminalMode::Local,
                };
                config.terminal.config.keyboard_uppercase_only = keyboard.uppercase_only;
                config.terminal.config.keyboard_parity_mode = keyboard.parity;
                config.terminal.config.input_return_mode = keyboard.return_mode;
                config.terminal.config.paste_on_right_click = paste_on_right_click;
                config.terminal.config.no_print = !printer_enabled;
                config.data_throttle.config.mode = match throttle_mode {
                    ThrottleMode::Throttled => ConfigThrottleMode::Throttled,
                    ThrottleMode::Unthrottled => ConfigThrottleMode::Unthrottled,
                };
                config.tape_reader.config = tape_reader;
                config.tape_punch.config = tape_punch;
                config.backend.serial_config = serial;
                config.sound.config.lid = lid_state;
                config.sound.config.mute_state = mute_state;
            });
        if serial_changed {
            self.settings_state.pending_reconnect = serial_reconnect_required(
                self.active_serial_config.as_ref(),
                &self.options.serial_config,
            );
        }
        self.settings_view.applied_theme = self.theme;
        self.settings_view.draft_theme = self.theme;
    }

    fn terminal(&mut self, ui: &mut egui::Ui) {
        let terminal_rect = ui.max_rect();
        let terminal_focus_id = ui.id().with("terminal-keyboard-target");
        let (primary_clicked, secondary_clicked, click_time) = ui.ctx().input(|input| {
            let inside = input
                .pointer
                .interact_pos()
                .is_some_and(|position| terminal_rect.contains(position));
            (
                inside && input.pointer.button_clicked(egui::PointerButton::Primary),
                inside && input.pointer.button_clicked(egui::PointerButton::Secondary),
                input.time,
            )
        });
        if primary_clicked || secondary_clicked {
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(terminal_focus_id));
            self.keyboard_target = KeyboardTarget::Terminal;
        }
        if secondary_clicked && self.paste_on_right_click {
            self.right_click_paste_requested_at = Some(click_time);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            ui.ctx().request_repaint();
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

    fn reader_contents(&mut self, ui: &mut egui::Ui, metrics: TapePanelMetrics) {
        ui.horizontal(|ui| {
            if ui.button("Load").clicked()
                && let Some(path) = reader_dialog(&self.options.tape_reader.initial_file_path)
            {
                match load_reader_file(&path) {
                    Ok(tape) => {
                        self.reader.load(tape);
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
                self.reader.unload();
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
                self.reader.rewind();
                self.reader_view.follow_reader();
                self.reader_seek_position = 0;
            }
        });
        ui.add_space(metrics.group_spacing);
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
        tape_path_label(
            ui,
            "File",
            self.reader_path.as_deref(),
            self.theme.palette().muted_text,
        );
        let status = reader_status_label(
            self.reader.reader().state(),
            self.reader.reader().stop_cause(),
        );
        ui.label(format!(
            "{length} bytes · position {} · {:.1}% · {status}",
            self.reader.reader().position(),
            percent,
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
            ui.label(format!("/ {length}"));
            if ui.add_enabled(stopped, egui::Button::new("Go")).clicked() {
                self.seek_reader(self.reader_seek_position);
            }
            if tape_step_button(ui, TapeStepDirection::TowardStart, stopped, metrics.scale)
                .on_hover_text("Move tape one byte toward start")
                .clicked()
            {
                self.seek_reader(stepped_position(
                    self.reader.reader().position(),
                    length,
                    TapeStepDirection::TowardStart,
                ));
            }
            if tape_step_button(ui, TapeStepDirection::TowardEnd, stopped, metrics.scale)
                .on_hover_text("Move tape one byte toward end")
                .clicked()
            {
                self.seek_reader(stepped_position(
                    self.reader.reader().position(),
                    length,
                    TapeStepDirection::TowardEnd,
                ));
            }
            let follows_reader = self.reader_view.follows_reader();
            let follow =
                ui.selectable_label(follows_reader, "Follow")
                    .on_hover_text(if follows_reader {
                        "Viewport follows the read head; click for free inspection"
                    } else {
                        "Free tape inspection; reader position is unchanged; click to follow"
                    });
            if follow.clicked() {
                if follows_reader {
                    self.reader_view.inspect_manually();
                } else {
                    self.reader_view.follow_reader();
                }
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
                    metrics: metrics.tape,
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

    fn punch_contents(&mut self, ui: &mut egui::Ui, metrics: TapePanelMetrics) {
        ui.horizontal(|ui| {
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
        ui.add_space(metrics.group_spacing);
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
        tape_path_label(ui, "File", path, self.theme.palette().muted_text);
        ui.label(format!(
            "{} bytes · {}",
            bytes.len(),
            punch_status_label(self.punch.as_ref().map(PunchFile::state))
        ));
        render_tape(
            ui,
            bytes,
            TapeRendererOptions {
                metrics: metrics.tape,
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

    fn reader_panel_contents(&mut self, ui: &mut egui::Ui) {
        let metrics = tape_panel_metrics(ui);
        ui.scope(|ui| {
            apply_tape_panel_style(ui, metrics);
            ui.label(egui::RichText::new("Paper Tape Reader").size(metrics.title_font_size));
            Self::panel_header(ui, &mut self.reader_panel);
            ui.add_space(metrics.group_spacing);
            self.reader_contents(ui, metrics);
        });
    }

    fn punch_panel_contents(&mut self, ui: &mut egui::Ui) {
        let metrics = tape_panel_metrics(ui);
        ui.scope(|ui| {
            apply_tape_panel_style(ui, metrics);
            ui.label(egui::RichText::new("Paper Tape Punch").size(metrics.title_font_size));
            Self::panel_header(ui, &mut self.punch_panel);
            ui.add_space(metrics.group_spacing);
            self.punch_contents(ui, metrics);
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
                    self.punch_panel_contents(ui);
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
                    self.reader_panel_contents(ui);
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
                    self.reader_panel_contents(ui);
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
                    self.punch_panel_contents(ui);
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
        self.reader.unload();
        self.punch = None;
        self.audio.set_tape_reader_running(false);
        self.audio.shutdown();
        if let Err(error) = self.runtime.shutdown().and_then(|()| self.runtime.join()) {
            self.transport_error = Some(error.to_string());
        }
        self.audio.join();
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
        let title = match (
            self.runtime.connection_state(),
            self.active_serial_config.as_ref(),
        ) {
            (ConnectionState::Connected, Some(active)) => {
                format!("ASR-33 Emulator using {}:{}", active.port, active.baudrate)
            }
            _ => "ASR-33 Emulator — Disconnected".to_owned(),
        };
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
        match self.current_view {
            AppView::Terminal => {
                egui::Panel::top("application-menu")
                    .resizable(false)
                    .show(ui, |ui| {
                        egui::MenuBar::new().ui(ui, |ui| {
                            if ui.button("Settings").clicked() {
                                self.settings_view
                                    .open(&mut self.settings_state, self.theme);
                                self.current_view.open_settings();
                            }
                            if ui
                                .button("Clear Paper")
                                .on_hover_text(
                                    "Clear terminal paper and scrollback locally; no bytes are transmitted",
                                )
                                .clicked()
                            {
                                self.clear_paper(ui.ctx());
                            }
                        });
                    });
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
                self.sync_controls_to_applied();
            }
            AppView::Settings => {
                self.keyboard_target = KeyboardTarget::UiText;
                let viewport = ui.ctx().input(|input| {
                    input
                        .viewport()
                        .inner_rect
                        .map_or_else(|| input.content_rect().size(), |rect| rect.size())
                });
                let metrics = SettingsUiMetrics::from_viewport(viewport);
                let settings_action = self.settings_view.show(
                    ui,
                    metrics,
                    &mut self.settings_state,
                    &self.available_ports,
                    self.runtime.connection_state() == &ConnectionState::Connected,
                    self.active_serial_config.as_ref(),
                );
                if let Some(action) = settings_action {
                    self.handle_settings_action(ui.ctx(), action);
                }
            }
        }
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

fn tape_panel_metrics(ui: &egui::Ui) -> TapePanelMetrics {
    let available =
        (ui.available_rect_before_wrap().width() - ui.spacing().scroll.allocated_width()).max(0.0);
    TapePanelMetrics::for_available_width(available)
}

fn apply_tape_panel_style(ui: &mut egui::Ui, metrics: TapePanelMetrics) {
    let mut style = (**ui.style()).clone();
    for text_style in [
        egui::TextStyle::Body,
        egui::TextStyle::Button,
        egui::TextStyle::Monospace,
    ] {
        if let Some(font) = style.text_styles.get_mut(&text_style) {
            font.size = metrics.body_font_size;
        }
    }
    if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Small) {
        font.size = metrics.small_font_size;
    }
    if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
        font.size = metrics.title_font_size;
    }
    style.spacing.interact_size *= metrics.scale;
    style.spacing.interact_size.y = metrics.button_height;
    style.spacing.button_padding = egui::vec2(metrics.button_padding_x, metrics.button_padding_y);
    style.spacing.item_spacing = egui::Vec2::splat(metrics.item_spacing);
    style.spacing.icon_width = metrics.checkbox_size;
    style.spacing.icon_width_inner *= metrics.scale;
    style.spacing.icon_spacing *= metrics.scale;
    style.spacing.indent *= metrics.scale;
    style.spacing.extra_text_line_spacing *= metrics.scale;
    ui.set_style(style);
}

fn tape_path_label(ui: &mut egui::Ui, prefix: &str, path: Option<&Path>, color: egui::Color32) {
    let full_path = display_path(path);
    ui.add(
        egui::Label::new(egui::RichText::new(format!("{prefix}: {full_path}")).color(color))
            .truncate(),
    )
    .on_hover_text(full_path);
}

#[must_use]
fn column_status_label(column: usize, width: usize) -> String {
    format!("Col {column} / {width}")
}

#[must_use]
const fn reader_status_label(state: ReaderState, stop_cause: Option<StopCause>) -> &'static str {
    match (state, stop_cause) {
        (ReaderState::Unloaded, _) => "No tape loaded",
        (ReaderState::Running, _) => "Running",
        (ReaderState::Stopped, Some(StopCause::EndOfTape)) => "Stopped: end of tape",
        (ReaderState::Stopped, Some(StopCause::TrailingOctal200)) => "Stopped: trailer (octal 200)",
        (ReaderState::Stopped, Some(StopCause::TrailingNull)) => "Stopped: null trailer",
        (ReaderState::Stopped, None) => "Stopped",
    }
}

#[must_use]
const fn punch_status_label(state: Option<PunchState>) -> &'static str {
    match state {
        None | Some(PunchState::Unloaded) => "No tape loaded",
        Some(PunchState::Stopped) => "Stopped",
        Some(PunchState::Running) => "Running",
    }
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

fn paste_request_is_live(requested_at: Option<f64>, now: f64) -> bool {
    requested_at.is_some_and(|requested_at| {
        now >= requested_at && now - requested_at <= PASTE_REQUEST_TIMEOUT_SECONDS
    })
}

fn normalize_paste_text(text: &str, mode: InputReturnMode) -> String {
    let newline = match mode {
        InputReturnMode::Cr => "\r",
        InputReturnMode::CrLf => "\r\n",
    };
    let mut normalized = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    let _ = characters.next();
                }
                normalized.push_str(newline);
            }
            '\n' => normalized.push_str(newline),
            _ => normalized.push(character),
        }
    }
    normalized
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
    scale: f32,
) -> egui::Response {
    let size = egui::vec2(24.0, 20.0) * scale;
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
            egui::pos2(center.x, center.y - 4.0 * scale),
            egui::pos2(center.x - 5.0 * scale, center.y + 3.0 * scale),
            egui::pos2(center.x + 5.0 * scale, center.y + 3.0 * scale),
        ],
        TapeStepDirection::TowardEnd => [
            egui::pos2(center.x, center.y + 4.0 * scale),
            egui::pos2(center.x - 5.0 * scale, center.y - 3.0 * scale),
            egui::pos2(center.x + 5.0 * scale, center.y - 3.0 * scale),
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
        KeyboardTarget, PASTE_REQUEST_TIMEOUT_SECONDS, PortRefreshPolicy, TapeStepDirection,
        apply_tape_shortcut, column_status_label, history_scroll_target, keyboard_input_for_event,
        keyboard_input_for_target, normalize_paste_text, open_serial_for_explicit_request,
        paste_request_is_live, port_enumeration_update, punch_status_label, reader_status_label,
        repaint_delay, should_send_to_terminal, startup_connection_state, stepped_position,
        toggled_lid,
    };
    use crate::adapters::paper_tape::PunchFile;
    use crate::app::PumpStatus;
    use crate::core::config::{
        BackendKind, DataBits, InputReturnMode, LidState, SerialConfig, SerialParity, StopBits,
    };
    use crate::core::paper_tape::{
        PaperTape, PunchMode, PunchState, ReaderOptions, ReaderState, StopCause, TapeReader,
    };
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

    fn desired_serial(port: &str) -> SerialConfig {
        SerialConfig {
            port: port.to_owned(),
            baudrate: 19_200,
            databits: DataBits::Eight,
            parity: SerialParity::None,
            stopbits: StopBits::One,
        }
    }

    #[test]
    fn startup_never_activates_or_reports_configured_transport() {
        for (backend, port) in [
            (BackendKind::Serial, "COM4"),
            (BackendKind::Serial, "COM999"),
            (BackendKind::Ssh, "COM4"),
        ] {
            let desired = desired_serial(port);
            let state = startup_connection_state(backend, &desired);
            assert_eq!(
                state.connection_state,
                crate::app::ConnectionState::Disconnected
            );
            assert_eq!(state.active_serial_config, None);
            assert_eq!(state.transport_error, None);
            assert_eq!(desired.port, port, "desired configuration remains intact");
        }
    }

    #[test]
    fn serial_open_occurs_once_per_explicit_request_and_never_retries() {
        let attempts = std::cell::Cell::new(0);
        let desired = desired_serial("COM999");
        let open = |config: SerialConfig| -> Result<(), &'static str> {
            attempts.set(attempts.get() + 1);
            assert_eq!(config, desired);
            Err("cannot open serial port")
        };

        let startup = startup_connection_state(BackendKind::Serial, &desired);
        assert_eq!(attempts.get(), 0);
        assert_eq!(startup.transport_error, None);

        assert_eq!(
            open_serial_for_explicit_request(desired.clone(), open),
            Err("cannot open serial port")
        );
        assert_eq!(attempts.get(), 1);
        assert_eq!(
            open_serial_for_explicit_request(desired.clone(), open),
            Err("cannot open serial port")
        );
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn startup_port_enumeration_is_silent_but_explicit_refresh_reports_failure() {
        let silent = port_enumeration_update(
            Err("enumeration unavailable".to_owned()),
            PortRefreshPolicy::Silent,
        );
        assert_eq!(silent, (Vec::new(), None));

        let explicit = port_enumeration_update(
            Err("enumeration unavailable".to_owned()),
            PortRefreshPolicy::ReportErrors,
        );
        assert_eq!(
            explicit,
            (
                Vec::new(),
                Some("port enumeration failed: enumeration unavailable".to_owned())
            )
        );
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
    fn audio_lid_toggle_is_a_stable_two_state_transition() {
        assert_eq!(toggled_lid(LidState::Up), LidState::Down);
        assert_eq!(toggled_lid(LidState::Down), LidState::Up);
    }

    #[test]
    fn column_status_uses_terminal_column_and_width() {
        assert_eq!(column_status_label(0, 72), "Col 0 / 72");
        assert_eq!(column_status_label(3, 80), "Col 3 / 80");
    }

    #[test]
    fn right_click_paste_request_survives_backend_round_trip_but_expires() {
        let requested_at = Some(10.0);
        assert!(paste_request_is_live(requested_at, 10.0));
        assert!(paste_request_is_live(
            requested_at,
            10.0 + PASTE_REQUEST_TIMEOUT_SECONDS
        ));
        assert!(!paste_request_is_live(
            requested_at,
            10.0 + PASTE_REQUEST_TIMEOUT_SECONDS + 0.001
        ));
        assert!(!paste_request_is_live(None, 10.0));
    }

    #[test]
    fn right_click_paste_normalizes_all_host_line_endings_to_keyboard_eol() {
        let source = "ONE\r\nTWO\nTHREE\rFOUR";
        assert_eq!(
            normalize_paste_text(source, InputReturnMode::Cr),
            "ONE\rTWO\rTHREE\rFOUR"
        );
        assert_eq!(
            normalize_paste_text(source, InputReturnMode::CrLf),
            "ONE\r\nTWO\r\nTHREE\r\nFOUR"
        );
    }

    #[test]
    fn tape_statuses_are_user_facing_and_never_debug_options() {
        assert_eq!(
            reader_status_label(ReaderState::Unloaded, None),
            "No tape loaded"
        );
        assert_eq!(reader_status_label(ReaderState::Running, None), "Running");
        assert_eq!(
            reader_status_label(ReaderState::Stopped, Some(StopCause::TrailingOctal200)),
            "Stopped: trailer (octal 200)"
        );
        assert_eq!(punch_status_label(None), "No tape loaded");
        assert_eq!(punch_status_label(Some(PunchState::Running)), "Running");
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
