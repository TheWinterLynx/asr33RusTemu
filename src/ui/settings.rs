//! Full typed configuration editor. This module renders drafts only; runtime
//! changes are applied by `EguiApp` after whole-config validation.

use super::theme::ThemeKind;
use crate::app::config_controller::{ChangeClass, SettingsState};
use crate::core::config::*;
use eframe::egui;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsPage {
    #[default]
    General,
    Terminal,
    Connection,
    Throttle,
    TapeReader,
    TapePunch,
    Sound,
    Ssh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsAction {
    Apply,
    Save,
    Cancel,
    Revert,
    Reconnect,
    RefreshPorts,
}

#[derive(Debug, Default)]
pub struct SettingsWindow {
    pub open: bool,
    page: SettingsPage,
    pub error: Option<String>,
    confirm_discard: bool,
    pub applied_theme: ThemeKind,
    pub draft_theme: ThemeKind,
}

impl SettingsWindow {
    pub fn open(&mut self, state: &mut SettingsState, theme: ThemeKind) {
        state.open();
        self.applied_theme = theme;
        self.draft_theme = theme;
        self.error = None;
        self.open = true;
    }

    pub fn show(
        &mut self,
        context: &egui::Context,
        state: &mut SettingsState,
        ports: &[String],
        connected: bool,
        active_serial: Option<&SerialConfig>,
    ) -> Option<SettingsAction> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut action = None;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_size([850.0, 610.0])
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        for (page, label) in pages() {
                            ui.selectable_value(&mut self.page, page, label);
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("settings-content")
                        .show(ui, |ui| {
                            ui.set_min_width(610.0);
                            self.page(ui, state, ports, connected, active_serial);
                        });
                });
                ui.separator();
                ui.label(format!("Config file: {}", state.path().display()));
                if self.page == SettingsPage::Connection
                    && ui.small_button("Refresh serial ports").clicked()
                {
                    action = Some(SettingsAction::RefreshPorts);
                }
                let status = if state.draft_dirty() || self.draft_theme != self.applied_theme {
                    "Unapplied edits"
                } else if state.unsaved() {
                    "Applied, not saved"
                } else {
                    "No unapplied changes"
                };
                ui.label(status);
                if !state.pending_restart.is_empty() {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Restart required: {}", state.pending_restart.join(", ")),
                    );
                }
                if state.pending_reconnect {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Reconnect required: active connection still uses its previous parameters",
                    );
                }
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = Some(SettingsAction::Cancel);
                    }
                    if ui.button("Revert").clicked() {
                        action = Some(SettingsAction::Revert);
                    }
                    if ui.button("Apply").clicked() {
                        action = Some(SettingsAction::Apply);
                    }
                    if ui.button("Save").clicked() {
                        action = Some(SettingsAction::Save);
                    }
                    if state.pending_reconnect
                        && ui
                            .add_enabled(connected, egui::Button::new("Reconnect now"))
                            .clicked()
                    {
                        action = Some(SettingsAction::Reconnect);
                    }
                });
                if self.confirm_discard {
                    ui.separator();
                    ui.colored_label(ui.visuals().warn_fg_color, "Discard unapplied changes?");
                    ui.horizontal(|ui| {
                        if ui.button("Discard").clicked() {
                            action = Some(SettingsAction::Cancel);
                            self.confirm_discard = false;
                        }
                        if ui.button("Keep editing").clicked() {
                            self.confirm_discard = false;
                        }
                    });
                }
            });
        if !open {
            if state.draft_dirty() || self.draft_theme != self.applied_theme {
                self.confirm_discard = true;
                self.open = true;
            } else {
                self.open = false;
            }
        }
        action
    }

    fn page(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut SettingsState,
        ports: &[String],
        connected: bool,
        active_serial: Option<&SerialConfig>,
    ) {
        match self.page {
            SettingsPage::General => general(ui, state, &mut self.draft_theme),
            SettingsPage::Terminal => terminal(ui, &mut state.draft_config),
            SettingsPage::Connection => {
                connection(ui, &mut state.draft_config, ports, connected, active_serial)
            }
            SettingsPage::Throttle => throttle(ui, &mut state.draft_config),
            SettingsPage::TapeReader => tape_reader(ui, &mut state.draft_config),
            SettingsPage::TapePunch => tape_punch(ui, &mut state.draft_config),
            SettingsPage::Sound => sound(ui, &mut state.draft_config),
            SettingsPage::Ssh => ssh(ui, &mut state.draft_config),
        }
    }
}

fn pages() -> [(SettingsPage, &'static str); 8] {
    [
        (SettingsPage::General, "General"),
        (SettingsPage::Terminal, "Terminal"),
        (SettingsPage::Connection, "Connection"),
        (SettingsPage::Throttle, "Throttle"),
        (SettingsPage::TapeReader, "Tape Reader"),
        (SettingsPage::TapePunch, "Tape Punch"),
        (SettingsPage::Sound, "Sound"),
        (SettingsPage::Ssh, "SSH"),
    ]
}
fn badge(ui: &mut egui::Ui, class: ChangeClass) {
    ui.weak(match class {
        ChangeClass::Live => "LIVE",
        ChangeClass::Reconnect => "RECONNECT",
        ChangeClass::Restart => "RESTART",
        ChangeClass::Unavailable => "NOT IMPLEMENTED",
        ChangeClass::Legacy => "LEGACY",
    });
}
fn row(ui: &mut egui::Ui, label: &str, class: ChangeClass, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        add(ui);
        badge(ui, class);
    });
}
fn path(ui: &mut egui::Ui, value: &mut std::path::PathBuf) {
    let mut text = value.to_string_lossy().into_owned();
    if ui.text_edit_singleline(&mut text).changed() {
        *value = text.into();
    }
}
fn optional_path(ui: &mut egui::Ui, value: &mut Option<std::path::PathBuf>) {
    let mut text = value
        .as_ref()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default();
    if ui.text_edit_singleline(&mut text).changed() {
        *value = (!text.trim().is_empty()).then(|| text.into());
    }
}
fn optional_text(ui: &mut egui::Ui, value: &mut Option<String>, password: bool) {
    let mut text = value.clone().unwrap_or_default();
    if ui
        .add(egui::TextEdit::singleline(&mut text).password(password))
        .changed()
    {
        *value = (!text.is_empty()).then_some(text);
    }
}

fn general(ui: &mut egui::Ui, state: &mut SettingsState, theme: &mut ThemeKind) {
    ui.heading("General / Appearance");
    row(ui, "Theme", ChangeClass::Live, |ui| {
        ui.selectable_value(theme, ThemeKind::Light, "Light");
        ui.selectable_value(theme, ThemeKind::Dark, "Dark");
    });
    ui.separator();
    ui.heading("Legacy compatibility");
    ui.label(format!(
        "frontend.type = {:?} (read-only; Rust always uses egui)",
        state.draft_config.frontend.kind
    ));
}
fn terminal(ui: &mut egui::Ui, c: &mut AppConfig) {
    let t = &mut c.terminal.config;
    ui.heading("Terminal");
    row(ui, "Communication", ChangeClass::Live, |ui| {
        ui.selectable_value(&mut t.mode, TerminalMode::Line, "LINE");
        ui.selectable_value(&mut t.mode, TerminalMode::Local, "LOCAL");
    });
    row(ui, "Columns", ChangeClass::Restart, |ui| {
        ui.add(egui::DragValue::new(&mut t.columns).range(1..=10000));
    });
    row(ui, "Rows", ChangeClass::Restart, |ui| {
        ui.add(egui::DragValue::new(&mut t.rows).range(1..=10000));
    });
    row(ui, "Scrollback", ChangeClass::Restart, |ui| {
        ui.add(egui::DragValue::new(&mut t.scrollback));
    });
    row(ui, "Autowrap", ChangeClass::Restart, |ui| {
        ui.checkbox(&mut t.autowrap, "");
    });
    row(ui, "Uppercase only", ChangeClass::Live, |ui| {
        ui.checkbox(&mut t.keyboard_uppercase_only, "");
    });
    row(ui, "Keyboard parity", ChangeClass::Live, |ui| {
        for (x, n) in [
            (KeyboardParityMode::Mark, "Mark"),
            (KeyboardParityMode::Space, "Space"),
            (KeyboardParityMode::Even, "Even"),
        ] {
            ui.selectable_value(&mut t.keyboard_parity_mode, x, n);
        }
    });
    row(ui, "Input return", ChangeClass::Live, |ui| {
        ui.selectable_value(&mut t.input_return_mode, InputReturnMode::Cr, "Raw");
        ui.selectable_value(&mut t.input_return_mode, InputReturnMode::CrLf, "CR+LF");
    });
    row(ui, "Send CR at startup", ChangeClass::Restart, |ui| {
        ui.checkbox(&mut t.send_cr_at_startup, "")
            .on_hover_text("Next application session only");
    });
    let mut enabled = !t.no_print;
    row(ui, "Printer enabled", ChangeClass::Live, |ui| {
        if ui.checkbox(&mut enabled, "").changed() {
            t.no_print = !enabled;
        }
    });
    row(ui, "Font path", ChangeClass::Restart, |ui| {
        optional_path(ui, &mut t.font_path)
    });
    row(ui, "Font size", ChangeClass::Live, |ui| {
        ui.add(egui::DragValue::new(&mut t.font_size).range(1..=200));
    });
}
fn connection(
    ui: &mut egui::Ui,
    c: &mut AppConfig,
    ports: &[String],
    connected: bool,
    active_serial: Option<&SerialConfig>,
) {
    ui.heading("Connection");
    ui.label(if connected {
        "Connected"
    } else {
        "Disconnected"
    });
    if let Some(active) = active_serial {
        ui.label(format!(
            "Active: {} @ {} baud, {:?}/{:?}/{:?}",
            active.port, active.baudrate, active.databits, active.parity, active.stopbits
        ));
        ui.weak("Edits below are desired settings and do not alter this live connection until Reconnect now.");
    }
    row(ui, "Backend", ChangeClass::Unavailable, |ui| {
        ui.selectable_value(&mut c.backend.kind, BackendKind::Serial, "Serial");
        ui.selectable_value(&mut c.backend.kind, BackendKind::Ssh, "SSH (not migrated)");
    });
    let s = &mut c.backend.serial_config;
    row(ui, "Port", ChangeClass::Reconnect, |ui| {
        ui.text_edit_singleline(&mut s.port);
        egui::ComboBox::from_id_salt("settings-ports")
            .selected_text("Available")
            .show_ui(ui, |ui| {
                for p in ports {
                    ui.selectable_value(&mut s.port, p.clone(), p);
                }
            });
    });
    row(ui, "Baudrate", ChangeClass::Reconnect, |ui| {
        ui.add(egui::DragValue::new(&mut s.baudrate).range(1..=u32::MAX));
    });
    row(ui, "Data bits", ChangeClass::Reconnect, |ui| {
        for (x, n) in [
            (DataBits::Five, "5"),
            (DataBits::Six, "6"),
            (DataBits::Seven, "7"),
            (DataBits::Eight, "8"),
        ] {
            ui.selectable_value(&mut s.databits, x, n);
        }
    });
    row(ui, "Parity", ChangeClass::Reconnect, |ui| {
        for (x, n) in [
            (SerialParity::None, "N"),
            (SerialParity::Even, "E"),
            (SerialParity::Odd, "O"),
            (SerialParity::Mark, "M"),
            (SerialParity::Space, "S"),
        ] {
            ui.selectable_value(&mut s.parity, x, n);
        }
    });
    row(ui, "Stop bits", ChangeClass::Reconnect, |ui| {
        for (x, n) in [
            (StopBits::One, "1"),
            (StopBits::OnePointFive, "1.5"),
            (StopBits::Two, "2"),
        ] {
            ui.selectable_value(&mut s.stopbits, x, n);
        }
    });
}
fn throttle(ui: &mut egui::Ui, c: &mut AppConfig) {
    let t = &mut c.data_throttle.config;
    ui.heading("Data Rate / Throttle");
    row(ui, "Mode", ChangeClass::Live, |ui| {
        ui.selectable_value(&mut t.mode, ThrottleMode::Throttled, "Throttled");
        ui.selectable_value(&mut t.mode, ThrottleMode::Unthrottled, "Unthrottled");
    });
    row(ui, "Send rate (cps)", ChangeClass::Live, |ui| {
        ui.add(egui::DragValue::new(&mut t.send_rate_cps).range(1..=i64::MAX));
    });
    row(ui, "Receive rate (cps)", ChangeClass::Live, |ui| {
        ui.add(egui::DragValue::new(&mut t.receive_rate_cps).range(1..=i64::MAX));
    });
}
fn tape_reader(ui: &mut egui::Ui, c: &mut AppConfig) {
    let t = &mut c.tape_reader.config;
    ui.heading("Paper Tape Reader");
    row(ui, "Max visible rows", ChangeClass::Live, |ui| {
        ui.add(egui::DragValue::new(&mut t.max_rows).range(1..=10000));
    });
    row(ui, "Initial file path", ChangeClass::Live, |ui| {
        path(ui, &mut t.initial_file_path)
    });
    for (label, v) in [
        ("Skip leading nulls", &mut t.skip_leading_nulls),
        ("Auto-stop trailers", &mut t.auto_stop),
        ("Set MSB", &mut t.set_msb),
        ("Ghost outline", &mut t.ghost_outline),
        ("ASCII masks MSB", &mut t.ascii_char_mask_msb),
    ] {
        row(ui, label, ChangeClass::Live, |ui| {
            ui.checkbox(v, "");
        });
    }
    row(ui, "Bit label base", ChangeClass::Live, |ui| {
        ui.selectable_value(&mut t.bit_label_base, BitLabelBase::Zero, "0");
        ui.selectable_value(&mut t.bit_label_base, BitLabelBase::One, "1");
    });
    ui.weak("Behavioral changes affect future bytes and never reposition loaded tape.");
}
fn tape_punch(ui: &mut egui::Ui, c: &mut AppConfig) {
    let t = &mut c.tape_punch.config;
    ui.heading("Paper Tape Punch");
    row(ui, "Max visible rows", ChangeClass::Live, |ui| {
        ui.add(egui::DragValue::new(&mut t.max_rows).range(1..=10000));
    });
    row(ui, "Initial file path", ChangeClass::Live, |ui| {
        path(ui, &mut t.initial_file_path)
    });
    row(ui, "Next file mode", ChangeClass::Live, |ui| {
        ui.selectable_value(&mut t.mode, PunchConfigMode::Append, "Append");
        ui.selectable_value(&mut t.mode, PunchConfigMode::Overwrite, "Overwrite");
    });
    for (label, v) in [
        ("Ghost outline", &mut t.ghost_outline),
        ("ASCII masks MSB", &mut t.ascii_char_mask_msb),
    ] {
        row(ui, label, ChangeClass::Live, |ui| {
            ui.checkbox(v, "");
        });
    }
    row(ui, "Bit label base", ChangeClass::Live, |ui| {
        ui.selectable_value(&mut t.bit_label_base, BitLabelBase::Zero, "0");
        ui.selectable_value(&mut t.bit_label_base, BitLabelBase::One, "1");
    });
    ui.weak("Mode and initial path apply to the next selected file; an open file is never reopened or truncated.");
}
fn sound(ui: &mut egui::Ui, c: &mut AppConfig) {
    let s = &mut c.sound.config;
    ui.heading("Sound");
    ui.colored_label(ui.visuals().warn_fg_color, "Audio not implemented yet");
    row(ui, "Lid", ChangeClass::Unavailable, |ui| {
        ui.selectable_value(&mut s.lid, LidState::Up, "Up");
        ui.selectable_value(&mut s.lid, LidState::Down, "Down");
    });
    row(ui, "Mute", ChangeClass::Unavailable, |ui| {
        ui.selectable_value(&mut s.mute_state, MuteState::Muted, "Muted");
        ui.selectable_value(&mut s.mute_state, MuteState::Unmuted, "Unmuted");
    });
}
fn ssh(ui: &mut egui::Ui, c: &mut AppConfig) {
    let s = &mut c.backend.ssh_config;
    ui.heading("SSH");
    ui.colored_label(
        ui.visuals().warn_fg_color,
        "SSH backend not migrated yet; values can be safely persisted.",
    );
    ui.heading("Connection");
    row(ui, "Host", ChangeClass::Unavailable, |ui| {
        ui.text_edit_singleline(&mut s.host);
    });
    row(ui, "Port", ChangeClass::Unavailable, |ui| {
        ui.add(egui::DragValue::new(&mut s.port).range(1..=u16::MAX));
    });
    row(ui, "Username", ChangeClass::Unavailable, |ui| {
        ui.text_edit_singleline(&mut s.username);
    });
    ui.heading("Authentication");
    row(ui, "Key file", ChangeClass::Unavailable, |ui| {
        optional_path(ui, &mut s.key_filename)
    });
    row(ui, "Password", ChangeClass::Unavailable, |ui| {
        optional_text(ui, &mut s.password, true);
    });
    row(ui, "Use agent", ChangeClass::Unavailable, |ui| {
        ui.checkbox(&mut s.use_agent, "");
    });
    ui.heading("Host key");
    row(ui, "Policy", ChangeClass::Unavailable, |ui| {
        for (x, n) in [
            (HostKeyPolicy::Strict, "Strict"),
            (HostKeyPolicy::AcceptNew, "Accept new"),
            (HostKeyPolicy::Off, "Off"),
        ] {
            ui.selectable_value(&mut s.host_key_policy, x, n);
        }
    });
    row(ui, "Expected fingerprint", ChangeClass::Unavailable, |ui| {
        optional_text(ui, &mut s.expected_fingerprint, false);
    });
    row(ui, "Known hosts", ChangeClass::Unavailable, |ui| {
        path(ui, &mut s.known_hosts_file)
    });
    row(ui, "TOFU prompt", ChangeClass::Unavailable, |ui| {
        ui.checkbox(&mut s.tofu_prompt, "");
    });
}
