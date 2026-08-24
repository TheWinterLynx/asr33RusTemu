//! Host keyboard repeat filtering for the native egui frontend.
//!
//! egui/winit normally emits a repeated `Key` event immediately followed by
//! the corresponding `Text` event. When repeat is disabled both halves must
//! be removed, otherwise printable keys would still repeat through `Text`.
//! Application function-key shortcuts remain single-shot in both modes, while
//! ordinary egui text fields retain normal host editing behaviour.

use eframe::egui;

use crate::app::config_controller::keyboard_repeat_enabled;

pub(super) fn filter_host_repeat_events(context: &egui::Context) {
    // This policy belongs to the ASR keyboard, not to Settings/COM/SSH text
    // editors. egui 0.36.1 exposes this integration-aware query under the
    // `egui_wants_keyboard_input` name.
    if context.egui_wants_keyboard_input() {
        return;
    }

    let repeat_enabled = keyboard_repeat_enabled();
    context.input_mut(|input| filter_events(&mut input.events, repeat_enabled));
}

fn filter_events(events: &mut Vec<egui::Event>, repeat_enabled: bool) {
    let mut filtered = Vec::with_capacity(events.len());
    let mut suppress_following_text = false;

    for event in events.drain(..) {
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: true,
                ..
            } if is_application_shortcut(key) => {
                // F1-F9 control emulator UI state rather than ASR keyboard
                // data. A held function key must never toggle repeatedly.
                suppress_following_text = false;
            }
            egui::Event::Key {
                pressed: true,
                repeat: true,
                ..
            } if !repeat_enabled => {
                // Drop the repeated physical-key event and the Text event that
                // winit normally emits immediately afterwards.
                suppress_following_text = true;
            }
            egui::Event::Text(_) if suppress_following_text => {
                suppress_following_text = false;
            }
            event => {
                suppress_following_text = false;
                filtered.push(event);
            }
        }
    }

    *events = filtered;
}

#[must_use]
const fn is_application_shortcut(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::F1
            | egui::Key::F2
            | egui::Key::F3
            | egui::Key::F4
            | egui::Key::F5
            | egui::Key::F6
            | egui::Key::F7
            | egui::Key::F8
            | egui::Key::F9
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn disabled_repeat_drops_repeated_key_and_its_text_event() {
        let mut events = vec![
            key(egui::Key::A, false),
            egui::Event::Text("a".to_owned()),
            key(egui::Key::A, true),
            egui::Event::Text("a".to_owned()),
        ];
        filter_events(&mut events, false);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events.first(),
            Some(egui::Event::Key { repeat: false, .. })
        ));
        assert!(matches!(
            events.get(1),
            Some(egui::Event::Text(text)) if text == "a"
        ));
    }

    #[test]
    fn enabled_repeat_preserves_terminal_repeat_events() {
        let mut events = vec![
            key(egui::Key::A, true),
            egui::Event::Text("a".to_owned()),
            key(egui::Key::Enter, true),
        ];
        filter_events(&mut events, true);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn emulator_function_shortcuts_never_autorepeat() {
        for shortcut in [
            egui::Key::F5,
            egui::Key::F6,
            egui::Key::F7,
            egui::Key::F8,
            egui::Key::F9,
        ] {
            let mut events = vec![key(shortcut, true)];
            filter_events(&mut events, true);
            assert!(events.is_empty(), "{shortcut:?} repeated unexpectedly");
        }
    }
}
