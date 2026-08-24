//! Host keyboard repeat filtering for the native egui frontend.
//!
//! egui/winit normally emits a repeated `Key` event immediately followed by
//! the corresponding `Text` event.  When repeat is disabled both halves must
//! be removed, otherwise printable keys would still repeat through `Text`.

use eframe::egui;

use crate::app::config_controller::keyboard_repeat_enabled;

pub(super) fn filter_host_repeat_events(context: &egui::Context) {
    if keyboard_repeat_enabled() {
        return;
    }

    context.input_mut(|input| {
        let mut filtered = Vec::with_capacity(input.events.len());
        let mut suppress_following_text = false;

        for event in input.events.drain(..) {
            match event {
                egui::Event::Key {
                    pressed: true,
                    repeat: true,
                    ..
                } => {
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

        input.events = filtered;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_is_linked() {
        // Event filtering itself is exercised through egui's InputState at
        // runtime; keep a small compile-time test so this module stays linked.
        let _ = filter_host_repeat_events as fn(&egui::Context);
    }
}
