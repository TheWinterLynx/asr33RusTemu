//! Full typed configuration editor. This module renders drafts only; runtime
//! changes are applied by `EguiApp` after whole-config validation.

use super::theme::ThemeKind;
use crate::app::config_controller::{ChangeClass, SettingsState};
use crate::core::config::*;
use eframe::egui;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AppView {
    #[default]
    Terminal,
    Settings,
}

impl AppView {
    pub fn open_settings(&mut self) {
        *self = Self::Settings;
    }
    pub fn show_terminal(&mut self) {
        *self = Self::Terminal;
    }
}

pub const SETTINGS_REFERENCE_WIDTH: f32 = 1280.0;
pub const SETTINGS_REFERENCE_HEIGHT: f32 = 780.0;
pub const MIN_SETTINGS_SCALE: f32 = 0.80;
pub const MAX_SETTINGS_SCALE: f32 = 1.30;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsUiMetrics {
    pub scale: f32,
    pub title_font: f32,
    pub section_font: f32,
    pub body_font: f32,
    pub small_font: f32,
    pub nav_font: f32,
    pub badge_font: f32,
    pub button_height: f32,
    pub row_height: f32,
    pub row_spacing: f32,
    pub section_spacing: f32,
    pub padding: f32,
    pub nav_width: f32,
    pub label_width: f32,
    pub control_width: f32,
    pub footer_button_width: f32,
}

impl SettingsUiMetrics {
    #[must_use]
    pub fn from_viewport(viewport: egui::Vec2) -> Self {
        let viewport_scale =
            (viewport.x / SETTINGS_REFERENCE_WIDTH).min(viewport.y / SETTINGS_REFERENCE_HEIGHT);
        let scale = viewport_scale.clamp(MIN_SETTINGS_SCALE, MAX_SETTINGS_SCALE);
        Self {
            scale,
            title_font: 25.0 * scale,
            section_font: 18.0 * scale,
            body_font: 14.0 * scale,
            small_font: 12.0 * scale,
            nav_font: 15.0 * scale,
            badge_font: 11.0 * scale,
            button_height: 28.0 * scale,
            row_height: 31.0 * scale,
            row_spacing: 7.0 * scale,
            section_spacing: 15.0 * scale,
            padding: 14.0 * scale,
            nav_width: 155.0 * scale,
            label_width: 175.0 * scale,
            control_width: 310.0 * scale,
            footer_button_width: 76.0 * scale,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsAction {
    Apply,
    Save,
    Cancel,
    Revert,
    Reconnect,
    RefreshPorts,
    Close,
}

#[derive(Debug, Default)]
pub struct SettingsView {
    page: SettingsPage,
    pub error: Option<String>,
    confirm_discard: bool,
    pub applied_theme: ThemeKind,
    pub draft_theme: ThemeKind,
}

impl SettingsView {
    pub fn open(&mut self, state: &mut SettingsState, theme: ThemeKind) {
        state.open();
        self.applied_theme = theme;
        self.draft_theme = theme;
        self.error = None;
        self.confirm_discard = false;
    }

    pub fn request_close(&mut self, dirty: bool) -> Option<SettingsAction> {
        if dirty {
            self.confirm_discard = true;
            None
        } else {
            Some(SettingsAction::Close)
        }
    }

    pub fn keep_editing(&mut self) {
        self.confirm_discard = false;
    }

    #[must_use]
    pub const fn discard_confirmation_visible(&self) -> bool {
        self.confirm_discard
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        metrics: SettingsUiMetrics,
        state: &mut SettingsState,
        ports: &[String],
        connected: bool,
        active_serial: Option<&SerialConfig>,
    ) -> Option<SettingsAction> {
        let mut action = None;
        apply_settings_style(ui, metrics);
        egui::Panel::top("settings-view-header")
            .resizable(false)
            .exact_size(56.0 * metrics.scale)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Settings")
                            .size(metrics.title_font)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [metrics.button_height, metrics.button_height],
                                egui::Button::new("X"),
                            )
                            .on_hover_text("Close Settings")
                            .clicked()
                        {
                            action = self.request_close(
                                state.draft_dirty() || self.draft_theme != self.applied_theme,
                            );
                        }
                    });
                });
            });
        egui::Panel::bottom("settings-view-footer")
            .resizable(false)
            .show(ui, |ui| {
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [metrics.footer_button_width, metrics.button_height],
                            egui::Button::new("Save"),
                        )
                        .clicked()
                    {
                        action = Some(SettingsAction::Save);
                    }
                    if ui
                        .add_sized(
                            [metrics.footer_button_width, metrics.button_height],
                            egui::Button::new("Apply"),
                        )
                        .clicked()
                    {
                        action = Some(SettingsAction::Apply);
                    }
                    if ui
                        .add_sized(
                            [metrics.footer_button_width, metrics.button_height],
                            egui::Button::new("Revert"),
                        )
                        .clicked()
                    {
                        action = Some(SettingsAction::Revert);
                    }
                    if ui
                        .add_sized(
                            [metrics.footer_button_width, metrics.button_height],
                            egui::Button::new("Cancel"),
                        )
                        .clicked()
                    {
                        action = Some(SettingsAction::Cancel);
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
                            self.keep_editing();
                        }
                    });
                }
            });
        egui::Panel::left("settings-view-navigation")
            .resizable(false)
            .exact_size(metrics.nav_width)
            .show(ui, |ui| {
                ui.add_space(metrics.padding);
                for (page, label) in pages() {
                    if ui
                        .add_sized(
                            [
                                metrics.nav_width - 2.0 * metrics.padding,
                                metrics.button_height,
                            ],
                            egui::Button::selectable(
                                self.page == page,
                                egui::RichText::new(label).size(metrics.nav_font),
                            ),
                        )
                        .clicked()
                    {
                        self.page = page;
                    }
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("settings-content", self.page))
                .show(ui, |ui| {
                    ui.add_space(metrics.padding);
                    self.page(ui, metrics, state, ports, connected, active_serial);
                    ui.add_space(metrics.padding);
                });
        });
        action
    }

    fn page(
        &mut self,
        ui: &mut egui::Ui,
        metrics: SettingsUiMetrics,
        state: &mut SettingsState,
        ports: &[String],
        connected: bool,
        active_serial: Option<&SerialConfig>,
    ) {
        match self.page {
            SettingsPage::General => general(ui, metrics, state, &mut self.draft_theme),
            SettingsPage::Terminal => terminal(ui, metrics, &mut state.draft_config),
            SettingsPage::Connection => connection(
                ui,
                metrics,
                &mut state.draft_config,
                ports,
                connected,
                active_serial,
            ),
            SettingsPage::Throttle => throttle(ui, metrics, &mut state.draft_config),
            SettingsPage::TapeReader => tape_reader(ui, metrics, &mut state.draft_config),
            SettingsPage::TapePunch => tape_punch(ui, metrics, &mut state.draft_config),
            SettingsPage::Sound => sound(ui, metrics, &mut state.draft_config),
            SettingsPage::Ssh => ssh(ui, metrics, &mut state.draft_config),
        }
    }
}

fn apply_settings_style(ui: &mut egui::Ui, metrics: SettingsUiMetrics) {
    let mut style = (**ui.style()).clone();
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::proportional(metrics.body_font),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::proportional(metrics.body_font),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::proportional(metrics.small_font),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::proportional(metrics.section_font),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::monospace(metrics.badge_font),
    );
    style.spacing.item_spacing = egui::vec2(metrics.row_spacing, metrics.row_spacing);
    style.spacing.interact_size.y = metrics.row_height;
    style.spacing.indent = metrics.label_width;
    style.spacing.text_edit_width = metrics.control_width;
    ui.set_style(style);
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
    let (short, explanation) = match class {
        ChangeClass::Live => ("LIVE", "Applies to the current session"),
        ChangeClass::Reconnect => ("RECONNECT", "Requires an explicit serial reconnect"),
        ChangeClass::Restart => ("RESTART", "Takes effect after restarting the application"),
        ChangeClass::Unavailable => ("N/A", "Stored for compatibility; not implemented yet"),
        ChangeClass::Legacy => ("LEGACY", "Legacy compatibility input"),
    };
    ui.label(egui::RichText::new(short).monospace().weak())
        .on_hover_text(explanation);
}
fn row(
    ui: &mut egui::Ui,
    label: &str,
    help: &str,
    class: ChangeClass,
    add: impl FnOnce(&mut egui::Ui),
) {
    let body = ui.text_style_height(&egui::TextStyle::Body);
    let row_height = ui.spacing().interact_size.y.max(body * 2.0);
    let label_width = ui.spacing().indent;
    let control_width = ui
        .spacing()
        .text_edit_width
        .min((ui.available_width() - label_width - body * 6.0).max(body * 8.0));
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), row_height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_sized([label_width, row_height], egui::Label::new(label))
                .on_hover_cursor(egui::CursorIcon::Help)
                .on_hover_text(help);
            ui.allocate_ui_with_layout(
                egui::vec2(control_width, row_height),
                egui::Layout::left_to_right(egui::Align::Center),
                add,
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                badge(ui, class)
            });
        },
    );
}
fn section(ui: &mut egui::Ui, metrics: SettingsUiMetrics, title: &str) {
    ui.add_space(metrics.section_spacing);
    ui.label(
        egui::RichText::new(title)
            .size(metrics.section_font)
            .strong(),
    );
    ui.separator();
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

fn general(
    ui: &mut egui::Ui,
    metrics: SettingsUiMetrics,
    state: &mut SettingsState,
    theme: &mut ThemeKind,
) {
    ui.heading("General / Appearance");
    section(ui, metrics, "Appearance");
    row(
        ui,
        "Theme",
        "Selects the application's light or dark visual theme.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(theme, ThemeKind::Light, "Light");
            ui.selectable_value(theme, ThemeKind::Dark, "Dark");
        },
    );
    section(ui, metrics, "Compatibility");
    let legacy_frontend = match state.draft_config.frontend.kind {
        FrontendConfigValue::LegacyTkinter => "tkinter",
        FrontendConfigValue::LegacyPygame => "pygame",
    };
    ui.label(format!(
        "frontend.type = {legacy_frontend} (read-only; Rust always uses egui)"
    ))
    .on_hover_cursor(egui::CursorIcon::Help)
    .on_hover_text("Legacy frontend selection from Python. The Rust application always uses egui.");
}
fn terminal(ui: &mut egui::Ui, metrics: SettingsUiMetrics, c: &mut AppConfig) {
    let t = &mut c.terminal.config;
    ui.heading("Terminal");
    section(ui, metrics, "Behavior");
    row(
        ui,
        "Communication",
        "LINE uses the external connection. LOCAL loops input back to the emulated terminal.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut t.mode, TerminalMode::Line, "LINE");
            ui.selectable_value(&mut t.mode, TerminalMode::Local, "LOCAL");
        },
    );
    row(
        ui,
        "Autowrap",
        "Controls whether printing past the final column continues on the next line.",
        ChangeClass::Restart,
        |ui| {
            ui.checkbox(&mut t.autowrap, "");
        },
    );
    let mut enabled = !t.no_print;
    row(
        ui,
        "Printer enabled",
        "Enables terminal-paper printing. Paper-punch forwarding remains independent.",
        ChangeClass::Live,
        |ui| {
            if ui.checkbox(&mut enabled, "").changed() {
                t.no_print = !enabled;
            }
        },
    );
    section(ui, metrics, "Keyboard");
    row(
        ui,
        "Uppercase only",
        "Converts keyboard text to uppercase, matching typical ASR-33 operation.",
        ChangeClass::Live,
        |ui| {
            ui.checkbox(&mut t.keyboard_uppercase_only, "");
        },
    );
    row(
        ui,
        "Keyboard parity",
        "Selects the parity bit applied to keyboard characters before transmission.",
        ChangeClass::Live,
        |ui| {
            for (x, n) in [
                (KeyboardParityMode::Mark, "Mark"),
                (KeyboardParityMode::Space, "Space"),
                (KeyboardParityMode::Even, "Even"),
            ] {
                let help = match x {
                    KeyboardParityMode::Mark => "Always sets bit 7.",
                    KeyboardParityMode::Space => "Always clears bit 7.",
                    KeyboardParityMode::Even => "Sets bit 7 as needed for even parity.",
                };
                ui.selectable_value(&mut t.keyboard_parity_mode, x, n)
                    .on_hover_text(help);
            }
        },
    );
    row(
        ui,
        "Key repeat",
        "Allows the host operating system to repeat a held key. Disabled by default to match the ASR-33 keyboard unless repeat is explicitly enabled.",
        ChangeClass::Live,
        |ui| {
            ui.checkbox(&mut t.keyboard_repeat, "");
        },
    );
    row(
        ui,
        "Input return",
        "Controls how keyboard Return and paper-tape line endings are emitted.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut t.input_return_mode, InputReturnMode::Cr, "Raw")
                .on_hover_text("Preserves tape CR/LF exactly; keyboard Return sends CR only.");
            ui.selectable_value(&mut t.input_return_mode, InputReturnMode::CrLf, "CR+LF")
                .on_hover_text("Normalizes keyboard Return and tape line endings to CR+LF.");
        },
    );
    row(
        ui,
        "Paste on right click",
        "Pastes clipboard text into the terminal when its paper is right-clicked, using the current uppercase, parity and Input return settings.",
        ChangeClass::Live,
        |ui| {
            ui.checkbox(&mut t.paste_on_right_click, "");
        },
    );
    section(ui, metrics, "Dimensions and font");
    row(
        ui,
        "Columns",
        "Number of printable character columns in the emulated terminal.",
        ChangeClass::Restart,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.columns).range(1..=10000));
        },
    );
    row(
        ui,
        "Rows",
        "Number of terminal rows used by the logical display.",
        ChangeClass::Restart,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.rows).range(1..=10000));
        },
    );
    row(
        ui,
        "Scrollback",
        "Maximum number of previous terminal lines retained for history.",
        ChangeClass::Restart,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.scrollback));
        },
    );
    row(
        ui,
        "Font path",
        "Path to the font used to render terminal text.",
        ChangeClass::Restart,
        |ui| optional_path(ui, &mut t.font_path),
    );
    row(
        ui,
        "Font size",
        "Size of the font used to render the terminal paper.",
        ChangeClass::Live,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.font_size).range(1..=200));
        },
    );
    section(ui, metrics, "Startup");
    row(
        ui,
        "Send CR at startup",
        "Sends one literal carriage return when the first LINE connection of a new session is established.",
        ChangeClass::Restart,
        |ui| {
            ui.checkbox(&mut t.send_cr_at_startup, "")
                .on_hover_text("Takes effect on the next application session");
        },
    );
}
fn connection(
    ui: &mut egui::Ui,
    metrics: SettingsUiMetrics,
    c: &mut AppConfig,
    ports: &[String],
    connected: bool,
    active_serial: Option<&SerialConfig>,
) {
    ui.heading("Connection");
    section(ui, metrics, "Status");
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
    section(ui, metrics, "Serial connection");
    let s = &mut c.backend.serial_config;
    row(
        ui,
        "Port",
        "Serial port used for the external connection, for example COM4 on Windows.",
        ChangeClass::Reconnect,
        |ui| {
            ui.text_edit_singleline(&mut s.port);
            egui::ComboBox::from_id_salt("settings-ports")
                .selected_text("Available")
                .show_ui(ui, |ui| {
                    for p in ports {
                        ui.selectable_value(&mut s.port, p.clone(), p);
                    }
                });
        },
    );
    row(
        ui,
        "Baudrate",
        "Serial line speed in bits per second.",
        ChangeClass::Reconnect,
        |ui| {
            ui.add(egui::DragValue::new(&mut s.baudrate).range(1..=u32::MAX));
        },
    );
    row(
        ui,
        "Data bits",
        "Number of data bits in each serial character.",
        ChangeClass::Reconnect,
        |ui| {
            for (x, n) in [
                (DataBits::Five, "5"),
                (DataBits::Six, "6"),
                (DataBits::Seven, "7"),
                (DataBits::Eight, "8"),
            ] {
                ui.selectable_value(&mut s.databits, x, n);
            }
        },
    );
    row(
        ui,
        "Parity",
        "Serial-port parity mode used by the external connection.",
        ChangeClass::Reconnect,
        |ui| {
            for (x, n) in [
                (SerialParity::None, "N"),
                (SerialParity::Even, "E"),
                (SerialParity::Odd, "O"),
                (SerialParity::Mark, "M"),
                (SerialParity::Space, "S"),
            ] {
                let help = match x {
                    SerialParity::None => "No serial parity bit.",
                    SerialParity::Even => "Uses even serial parity.",
                    SerialParity::Odd => "Uses odd serial parity.",
                    SerialParity::Mark => "Always marks the serial parity bit.",
                    SerialParity::Space => "Always spaces the serial parity bit.",
                };
                ui.selectable_value(&mut s.parity, x, n).on_hover_text(help);
            }
        },
    );
    row(
        ui,
        "Stop bits",
        "Number of serial stop bits sent after each character.",
        ChangeClass::Reconnect,
        |ui| {
            for (x, n) in [
                (StopBits::One, "1"),
                (StopBits::OnePointFive, "1.5"),
                (StopBits::Two, "2"),
            ] {
                ui.selectable_value(&mut s.stopbits, x, n);
            }
        },
    );
    section(ui, metrics, "Backend");
    row(
        ui,
        "Backend type",
        "Selects the external backend. Serial is available; SSH is not migrated yet.",
        ChangeClass::Unavailable,
        |ui| {
            ui.selectable_value(&mut c.backend.kind, BackendKind::Serial, "Serial");
            ui.selectable_value(&mut c.backend.kind, BackendKind::Ssh, "SSH (not migrated)");
        },
    );
}
fn throttle(ui: &mut egui::Ui, metrics: SettingsUiMetrics, c: &mut AppConfig) {
    let t = &mut c.data_throttle.config;
    ui.heading("Data Rate / Throttle");
    section(ui, metrics, "Pacing");
    row(
        ui,
        "Mode",
        "Enables authentic character-rate pacing or removes artificial delay.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut t.mode, ThrottleMode::Throttled, "Throttled");
            ui.selectable_value(&mut t.mode, ThrottleMode::Unthrottled, "Unthrottled");
        },
    );
    row(
        ui,
        "Send rate (cps)",
        "Maximum transmit rate in characters per second while throttling is enabled.",
        ChangeClass::Live,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.send_rate_cps).range(1..=i64::MAX));
        },
    );
    row(
        ui,
        "Receive rate (cps)",
        "Maximum receive rate in characters per second while throttling is enabled.",
        ChangeClass::Live,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.receive_rate_cps).range(1..=i64::MAX));
        },
    );
}
fn tape_reader(ui: &mut egui::Ui, metrics: SettingsUiMetrics, c: &mut AppConfig) {
    let t = &mut c.tape_reader.config;
    ui.heading("Paper Tape Reader");
    section(ui, metrics, "Behavior");
    for (label, help, v) in [
        (
            "Skip leading nulls",
            "When starting at the beginning, skips leading 0x00 bytes before reading data.",
            &mut t.skip_leading_nulls,
        ),
        (
            "Auto-stop trailers",
            "Stops automatically when the detected trailing leader or trailer pattern is reached.",
            &mut t.auto_stop,
        ),
        (
            "Set MSB",
            "Sets bit 7 on each byte emitted by the paper-tape reader.",
            &mut t.set_msb,
        ),
    ] {
        row(ui, label, help, ChangeClass::Live, |ui| {
            ui.checkbox(v, "");
        });
    }
    section(ui, metrics, "Display");
    row(
        ui,
        "Max visible rows",
        "Legacy reader display limit retained for compatibility; the full Rust reader tape remains inspectable.",
        ChangeClass::Live,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.max_rows).range(1..=10000));
        },
    );
    for (label, help, v) in [
        (
            "Ghost outline",
            "Shows outlines for unpunched holes to make the tape pattern easier to read.",
            &mut t.ghost_outline,
        ),
        (
            "ASCII masks MSB",
            "Ignores bit 7 when displaying the ASCII character beside each tape byte.",
            &mut t.ascii_char_mask_msb,
        ),
    ] {
        row(ui, label, help, ChangeClass::Live, |ui| {
            ui.checkbox(v, "");
        });
    }
    row(
        ui,
        "Bit label base",
        "Chooses whether tape data-hole labels start from 0 or 1.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut t.bit_label_base, BitLabelBase::Zero, "0");
            ui.selectable_value(&mut t.bit_label_base, BitLabelBase::One, "1");
        },
    );
    section(ui, metrics, "Files");
    row(
        ui,
        "Initial file path",
        "Initial directory used when opening the paper-tape file chooser.",
        ChangeClass::Live,
        |ui| path(ui, &mut t.initial_file_path),
    );
    ui.weak("Behavioral changes affect future bytes and never reposition loaded tape.");
}
fn tape_punch(ui: &mut egui::Ui, metrics: SettingsUiMetrics, c: &mut AppConfig) {
    let t = &mut c.tape_punch.config;
    ui.heading("Paper Tape Punch");
    section(ui, metrics, "Behavior");
    row(
        ui,
        "Next file mode",
        "Selects how the next punch file opens; changing it never reopens the current file.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut t.mode, PunchConfigMode::Append, "Append")
                .on_hover_text("Preserves existing bytes and appends newly punched data.");
            ui.selectable_value(&mut t.mode, PunchConfigMode::Overwrite, "Overwrite")
                .on_hover_text("Truncates the next selected file when it is opened.");
        },
    );
    section(ui, metrics, "Display");
    row(
        ui,
        "Max visible rows",
        "Limits newest punched rows shown in the preview without discarding stored bytes.",
        ChangeClass::Live,
        |ui| {
            ui.add(egui::DragValue::new(&mut t.max_rows).range(1..=10000));
        },
    );
    for (label, help, v) in [
        (
            "Ghost outline",
            "Shows outlines for unpunched hole positions in the tape preview.",
            &mut t.ghost_outline,
        ),
        (
            "ASCII masks MSB",
            "Ignores bit 7 when displaying the ASCII character beside each punched byte.",
            &mut t.ascii_char_mask_msb,
        ),
    ] {
        row(ui, label, help, ChangeClass::Live, |ui| {
            ui.checkbox(v, "");
        });
    }
    row(
        ui,
        "Bit label base",
        "Chooses whether tape data-hole labels start from 0 or 1.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut t.bit_label_base, BitLabelBase::Zero, "0");
            ui.selectable_value(&mut t.bit_label_base, BitLabelBase::One, "1");
        },
    );
    section(ui, metrics, "Files");
    row(
        ui,
        "Initial file path",
        "Initial directory used when selecting the paper-tape output file.",
        ChangeClass::Live,
        |ui| path(ui, &mut t.initial_file_path),
    );
    ui.weak("Mode and initial path apply to the next selected file; an open file is never reopened or truncated.");
}
fn sound(ui: &mut egui::Ui, metrics: SettingsUiMetrics, c: &mut AppConfig) {
    let s = &mut c.sound.config;
    ui.heading("Sound");
    ui.weak("Native ASR-33 mechanical audio. Apply or Save changes the current session immediately.");
    section(ui, metrics, "Mechanical audio");
    row(
        ui,
        "Lid",
        "Selects the up/down acoustic sample family and plays the lid mechanism when the state changes.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut s.lid, LidState::Up, "Up");
            ui.selectable_value(&mut s.lid, LidState::Down, "Down");
        },
    );
    row(
        ui,
        "Mute",
        "Mutes or unmutes all ASR-33 mechanical audio with the legacy 200 ms fade.",
        ChangeClass::Live,
        |ui| {
            ui.selectable_value(&mut s.mute_state, MuteState::Muted, "Muted");
            ui.selectable_value(&mut s.mute_state, MuteState::Unmuted, "Unmuted");
        },
    );
}
fn ssh(ui: &mut egui::Ui, metrics: SettingsUiMetrics, c: &mut AppConfig) {
    let s = &mut c.backend.ssh_config;
    ui.heading("SSH");
    ui.colored_label(
        ui.visuals().warn_fg_color,
        "SSH backend not migrated yet; values can be safely persisted.",
    );
    section(ui, metrics, "Connection");
    row(
        ui,
        "Host",
        "Hostname or IP address of the SSH server.",
        ChangeClass::Unavailable,
        |ui| {
            ui.text_edit_singleline(&mut s.host);
        },
    );
    row(
        ui,
        "Port",
        "TCP port used by SSH, normally 22.",
        ChangeClass::Unavailable,
        |ui| {
            ui.add(egui::DragValue::new(&mut s.port).range(1..=u16::MAX));
        },
    );
    row(
        ui,
        "Username",
        "Username used to authenticate to the SSH server.",
        ChangeClass::Unavailable,
        |ui| {
            ui.text_edit_singleline(&mut s.username);
        },
    );
    section(ui, metrics, "Authentication");
    row(
        ui,
        "Key file",
        "Path to the private key used for SSH public-key authentication.",
        ChangeClass::Unavailable,
        |ui| optional_path(ui, &mut s.key_filename),
    );
    row(
        ui,
        "Password",
        "Password used to authenticate to the SSH server when password authentication is enabled.",
        ChangeClass::Unavailable,
        |ui| {
            optional_text(ui, &mut s.password, true);
        },
    );
    row(
        ui,
        "Use agent",
        "Allows authentication through the user's SSH authentication agent.",
        ChangeClass::Unavailable,
        |ui| {
            ui.checkbox(&mut s.use_agent, "");
        },
    );
    section(ui, metrics, "Host verification");
    row(
        ui,
        "Policy",
        "Controls how unknown or changed SSH host keys are handled.",
        ChangeClass::Unavailable,
        |ui| {
            for (x, n) in [
                (HostKeyPolicy::Strict, "Strict"),
                (HostKeyPolicy::AcceptNew, "Accept new"),
                (HostKeyPolicy::Off, "Off"),
            ] {
                let help = match x {
                    HostKeyPolicy::Strict => {
                        "Requires a matching key already present in known hosts."
                    }
                    HostKeyPolicy::AcceptNew => {
                        "Accepts new host keys but rejects changed known keys."
                    }
                    HostKeyPolicy::Off => "Disables known-host verification.",
                };
                ui.selectable_value(&mut s.host_key_policy, x, n)
                    .on_hover_text(help);
            }
        },
    );
    row(
        ui,
        "Expected fingerprint",
        "Optional expected SSH host-key fingerprint used to verify the remote server.",
        ChangeClass::Unavailable,
        |ui| {
            optional_text(ui, &mut s.expected_fingerprint, false);
        },
    );
    row(
        ui,
        "Known hosts",
        "Path to the known-hosts file used for SSH host-key verification.",
        ChangeClass::Unavailable,
        |ui| path(ui, &mut s.known_hosts_file),
    );
    row(
        ui,
        "TOFU prompt",
        "Stores whether first-use host keys should require confirmation once SSH is migrated.",
        ChangeClass::Unavailable,
        |ui| {
            ui.checkbox(&mut s.tofu_prompt, "");
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AppConfig {
        AppConfig::from_yaml_str(include_str!("../../asr33_config.yaml")).expect("fixture")
    }

    #[test]
    fn settings_scale_uses_global_reference_viewport_and_clamps() {
        let reference = SettingsUiMetrics::from_viewport(egui::vec2(1280.0, 780.0));
        let larger = SettingsUiMetrics::from_viewport(egui::vec2(1920.0, 1080.0));
        let smaller = SettingsUiMetrics::from_viewport(egui::vec2(800.0, 600.0));
        let minimum = SettingsUiMetrics::from_viewport(egui::vec2(100.0, 100.0));
        let maximum = SettingsUiMetrics::from_viewport(egui::vec2(8000.0, 8000.0));
        assert_eq!(reference.scale, 1.0);
        assert!(larger.scale > reference.scale);
        assert!(smaller.scale < reference.scale);
        assert_eq!(minimum.scale, MIN_SETTINGS_SCALE);
        assert_eq!(maximum.scale, MAX_SETTINGS_SCALE);
    }

    #[test]
    fn selected_page_cannot_affect_metrics_for_the_same_viewport() {
        let viewport = egui::vec2(1280.0, 780.0);
        let expected = SettingsUiMetrics::from_viewport(viewport);
        for page in pages().map(|(page, _)| page) {
            let mut view = SettingsView {
                page,
                ..SettingsView::default()
            };
            assert_eq!(SettingsUiMetrics::from_viewport(viewport), expected);
            view.page = SettingsPage::General;
            assert_eq!(SettingsUiMetrics::from_viewport(viewport), expected);
        }
    }

    #[test]
    fn full_view_navigation_preserves_dirty_close_confirmation() {
        let base = config();
        let mut state = SettingsState::new("settings.yaml".into(), base.clone(), base);
        let mut current = AppView::Terminal;
        let mut settings = SettingsView::default();
        current.open_settings();
        settings.open(&mut state, ThemeKind::Light);
        assert_eq!(current, AppView::Settings);

        state.draft_config.terminal.config.columns += 1;
        assert_eq!(settings.request_close(state.draft_dirty()), None);
        assert!(settings.discard_confirmation_visible());
        assert_eq!(current, AppView::Settings);
        settings.keep_editing();
        assert!(!settings.discard_confirmation_visible());

        assert_eq!(settings.request_close(state.draft_dirty()), None);
        state.cancel();
        current.show_terminal();
        assert_eq!(current, AppView::Terminal);
        assert!(!state.draft_dirty());

        current.open_settings();
        settings.open(&mut state, ThemeKind::Light);
        assert!(!settings.discard_confirmation_visible());
        assert_eq!(settings.request_close(false), Some(SettingsAction::Close));
        current.show_terminal();
        assert_eq!(current, AppView::Terminal);
    }
}
