//! Pure Settings state and config-difference classification.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::config::SerialConfig;
use crate::core::config::{AppConfig, ValidationError};
use crate::core::config_store::{ConfigStore, ConfigStoreError};

static KEYBOARD_REPEAT_ENABLED: AtomicBool = AtomicBool::new(false);

#[must_use]
pub fn keyboard_repeat_enabled() -> bool {
    KEYBOARD_REPEAT_ENABLED.load(Ordering::Relaxed)
}

pub fn set_keyboard_repeat_enabled(enabled: bool) {
    KEYBOARD_REPEAT_ENABLED.store(enabled, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeClass {
    Live,
    Reconnect,
    Restart,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigChange {
    pub label: &'static str,
    pub class: ChangeClass,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigChangePlan {
    pub changes: Vec<ConfigChange>,
}

impl ConfigChangePlan {
    #[must_use]
    pub fn between(old: &AppConfig, new: &AppConfig) -> Self {
        let mut changes = Vec::new();
        let mut add = |changed, label, class| {
            if changed {
                changes.push(ConfigChange { label, class });
            }
        };
        let a = &old.terminal.config;
        let b = &new.terminal.config;
        add(a.mode != b.mode, "communication mode", ChangeClass::Live);
        add(a.autowrap != b.autowrap, "autowrap", ChangeClass::Restart);
        add(
            a.keyboard_uppercase_only != b.keyboard_uppercase_only,
            "uppercase keyboard",
            ChangeClass::Live,
        );
        add(
            a.keyboard_parity_mode != b.keyboard_parity_mode,
            "keyboard parity",
            ChangeClass::Live,
        );
        add(
            a.keyboard_repeat != b.keyboard_repeat,
            "keyboard repeat",
            ChangeClass::Live,
        );
        add(
            a.input_return_mode != b.input_return_mode,
            "input return mode",
            ChangeClass::Live,
        );
        add(
            a.paste_on_right_click != b.paste_on_right_click,
            "right-click paste",
            ChangeClass::Live,
        );
        add(a.no_print != b.no_print, "printer", ChangeClass::Live);
        add(a.font_size != b.font_size, "font size", ChangeClass::Live);
        add(
            (a.columns, a.rows, a.scrollback) != (b.columns, b.rows, b.scrollback),
            "terminal dimensions/history",
            ChangeClass::Restart,
        );
        add(
            a.send_cr_at_startup != b.send_cr_at_startup,
            "startup CR",
            ChangeClass::Restart,
        );
        add(
            a.font_path != b.font_path,
            "font path",
            ChangeClass::Restart,
        );
        add(
            old.data_throttle != new.data_throttle,
            "data throttle",
            ChangeClass::Live,
        );
        add(
            old.tape_reader != new.tape_reader,
            "paper-tape reader",
            ChangeClass::Live,
        );
        add(
            old.tape_punch != new.tape_punch,
            "paper-tape punch",
            ChangeClass::Live,
        );
        add(
            old.backend.serial_config != new.backend.serial_config,
            "serial connection",
            ChangeClass::Reconnect,
        );
        add(old.sound != new.sound, "sound", ChangeClass::Live);
        Self { changes }
    }

    #[must_use]
    pub fn requires_restart(&self) -> bool {
        self.changes.iter().any(|x| x.class == ChangeClass::Restart)
    }
    #[must_use]
    pub fn requires_reconnect(&self) -> bool {
        self.changes
            .iter()
            .any(|x| x.class == ChangeClass::Reconnect)
    }
}

#[must_use]
pub fn serial_reconnect_required(active: Option<&SerialConfig>, desired: &SerialConfig) -> bool {
    active.is_some_and(|active| active != desired)
}

#[derive(Clone, Debug)]
pub struct SettingsState {
    store: ConfigStore,
    runtime_start_config: AppConfig,
    pub disk_config: AppConfig,
    pub applied_config: AppConfig,
    pub draft_config: AppConfig,
    pub pending_restart: Vec<&'static str>,
    pub pending_reconnect: bool,
}

impl SettingsState {
    #[must_use]
    pub fn new(path: PathBuf, disk_config: AppConfig, applied_config: AppConfig) -> Self {
        set_keyboard_repeat_enabled(applied_config.terminal.config.keyboard_repeat);
        Self {
            store: ConfigStore::new(path),
            runtime_start_config: applied_config.clone(),
            disk_config,
            draft_config: applied_config.clone(),
            applied_config,
            pending_restart: Vec::new(),
            pending_reconnect: false,
        }
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        self.store.path()
    }
    #[must_use]
    pub fn draft_dirty(&self) -> bool {
        self.draft_config != self.applied_config
    }
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.applied_config != self.disk_config
    }
    pub fn open(&mut self) {
        self.draft_config = self.applied_config.clone();
    }
    pub fn cancel(&mut self) {
        self.open();
    }
    pub fn revert(&mut self) {
        self.open();
    }
    pub fn validate_draft(&self) -> Result<(), ValidationError> {
        self.draft_config.validate()
    }
    #[must_use]
    pub fn plan(&self) -> ConfigChangePlan {
        ConfigChangePlan::between(&self.applied_config, &self.draft_config)
    }
    pub fn commit_apply(&mut self, plan: &ConfigChangePlan) {
        self.applied_config = self.draft_config.clone();
        set_keyboard_repeat_enabled(self.applied_config.terminal.config.keyboard_repeat);
        self.pending_restart =
            ConfigChangePlan::between(&self.runtime_start_config, &self.applied_config)
                .changes
                .into_iter()
                .filter(|change| change.class == ChangeClass::Restart)
                .map(|change| change.label)
                .collect();
        self.pending_reconnect |= plan.requires_reconnect();
    }
    pub fn save_applied(&mut self) -> Result<(), ConfigStoreError> {
        self.store.save(&self.applied_config)?;
        self.disk_config = self.applied_config.clone();
        Ok(())
    }
    pub fn update_applied_from_live_control(&mut self, update: impl FnOnce(&mut AppConfig)) {
        update(&mut self.applied_config);
        // The quick repeat control lives outside the legacy EguiApp fields, so
        // the atomic live policy is authoritative for this one setting. This
        // copies it into the applied/draft config on the same terminal frame,
        // preserving normal dirty/Save semantics.
        self.applied_config.terminal.config.keyboard_repeat = keyboard_repeat_enabled();
        self.draft_config = self.applied_config.clone();
    }
    pub fn clear_reconnect_required(&mut self) {
        self.pending_reconnect = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{InputReturnMode, LidState, MuteState, TerminalMode};

    fn config() -> AppConfig {
        AppConfig::from_yaml_str(include_str!("../../asr33_config.yaml")).expect("fixture")
    }

    #[test]
    fn disk_applied_draft_transitions_are_explicit() {
        let a = config();
        let mut state = SettingsState::new("x.yaml".into(), a.clone(), a.clone());
        state.draft_config.terminal.config.columns += 1;
        assert!(state.draft_dirty());
        state.cancel();
        assert_eq!(state.draft_config, a);
        state.draft_config.terminal.config.columns += 1;
        let plan = state.plan();
        state.commit_apply(&plan);
        assert!(state.unsaved());
        assert!(!state.draft_dirty());
        assert!(
            state
                .pending_restart
                .contains(&"terminal dimensions/history")
        );
        state.draft_config.terminal.config.rows += 1;
        state.revert();
        assert_eq!(state.draft_config, state.applied_config);
        state.draft_config = a;
        let plan = state.plan();
        state.commit_apply(&plan);
        assert!(state.pending_restart.is_empty());
    }

    #[test]
    fn save_persists_complete_effective_state_and_advances_disk_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cli-selected.yaml");
        let a = config();
        let mut state = SettingsState::new(path.clone(), a.clone(), a);
        state.draft_config.terminal.config.columns = 93;
        let plan = state.plan();
        state.commit_apply(&plan);
        state.save_applied().expect("save");
        assert!(!state.unsaved());
        assert_eq!(state.path(), path);
        assert_eq!(
            ConfigStore::new(path)
                .load()
                .expect("load")
                .terminal
                .config
                .columns,
            93
        );
    }

    #[test]
    fn representative_changes_have_required_classification() {
        let a = config();
        let mut b = a.clone();
        b.terminal.config.mode = TerminalMode::Local;
        b.terminal.config.input_return_mode = InputReturnMode::CrLf;
        b.terminal.config.paste_on_right_click = !a.terminal.config.paste_on_right_click;
        b.terminal.config.keyboard_repeat = !a.terminal.config.keyboard_repeat;
        assert!(
            ConfigChangePlan::between(&a, &b)
                .changes
                .iter()
                .all(|x| x.class == ChangeClass::Live)
        );
        assert!(
            ConfigChangePlan::between(&a, &b)
                .changes
                .iter()
                .any(|x| x.label == "right-click paste" && x.class == ChangeClass::Live)
        );
        assert!(
            ConfigChangePlan::between(&a, &b)
                .changes
                .iter()
                .any(|x| x.label == "keyboard repeat" && x.class == ChangeClass::Live)
        );
        let mut b = a.clone();
        b.backend.serial_config.port = "COM5".into();
        assert!(ConfigChangePlan::between(&a, &b).requires_reconnect());
        let mut b = a.clone();
        b.terminal.config.columns += 1;
        b.terminal.config.send_cr_at_startup = !b.terminal.config.send_cr_at_startup;
        assert!(ConfigChangePlan::between(&a, &b).requires_restart());
    }

    #[test]
    fn sound_changes_are_live() {
        let a = config();
        let mut b = a.clone();
        b.sound.config.lid = match a.sound.config.lid {
            LidState::Up => LidState::Down,
            LidState::Down => LidState::Up,
        };
        b.sound.config.mute_state = match a.sound.config.mute_state {
            MuteState::Muted => MuteState::Unmuted,
            MuteState::Unmuted => MuteState::Muted,
        };
        let plan = ConfigChangePlan::between(&a, &b);
        assert!(
            plan.changes
                .iter()
                .any(|change| change.label == "sound" && change.class == ChangeClass::Live)
        );
    }

    #[test]
    fn desired_serial_differs_from_active_until_explicit_reconnect() {
        let a = config().backend.serial_config;
        let mut b = a.clone();
        b.baudrate /= 2;
        assert!(serial_reconnect_required(Some(&a), &b));
        assert!(!serial_reconnect_required(None, &b));
        assert!(!serial_reconnect_required(Some(&b), &b));
    }
}
