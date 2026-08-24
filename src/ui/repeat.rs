//! ASR-33 keyboard repeat policy for the native egui frontend.
//!
//! Host typematic is deliberately never forwarded to the terminal. Windows
//! can generate repeated key events much faster than a 10 cps ASR-33 can
//! consume them; forwarding those events fills the TX throttle and makes the
//! machine keep typing after the physical key has been released.
//!
//! The real Model 33 has a REPT key: while REPT and another key are held, the
//! selected character repeats at the machine cadence. Our Repeat ON control is
//! treated as a latched REPT function. We therefore track one held terminal
//! key and create our own paced repeat events at 10 cps. There is no host-style
//! typematic burst and no catch-up after a delayed frame. Releasing the key
//! cancels future repeats immediately. A character already in the mechanical/
//! TX pipeline may still complete, just as an in-flight cycle would on the
//! electromechanical machine.

use std::time::Duration;

use eframe::egui;

use crate::app::config_controller::keyboard_repeat_enabled;

/// A Model 33 character cycle is 100 ms at 110 baud / 10 characters per
/// second, so the first repeated character follows one normal character cycle
/// after the initial keypress.
const INITIAL_REPEAT_DELAY_SECONDS: f64 = 0.10;

/// ASR-33 nominal print/transmit cadence: ten characters per second.
const REPEAT_INTERVAL_SECONDS: f64 = 0.10;

#[derive(Clone, Debug)]
enum RepeatPayload {
    Text(String),
    Key {
        key: egui::Key,
        modifiers: egui::Modifiers,
    },
}

impl RepeatPayload {
    fn event(&self) -> egui::Event {
        match self {
            Self::Text(text) => egui::Event::Text(text.clone()),
            Self::Key { key, modifiers } => egui::Event::Key {
                key: *key,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: *modifiers,
            },
        }
    }
}

#[derive(Clone, Debug)]
struct HeldKey {
    key: egui::Key,
    payload: Option<RepeatPayload>,
    next_repeat_at: f64,
}

#[derive(Debug, Default)]
pub(super) struct RepeatController {
    held: Option<HeldKey>,
}

impl RepeatController {
    pub(super) fn process(&mut self, context: &egui::Context) {
        // Repeat belongs to the ASR keyboard, not to Settings/COM/SSH text
        // editors. Native text editors retain the host's normal typematic.
        if context.egui_wants_keyboard_input() {
            self.held = None;
            return;
        }

        let repeat_enabled = keyboard_repeat_enabled();
        let now = context.input(|input| input.time);
        context.input_mut(|input| {
            filter_and_track_events(&mut input.events, repeat_enabled, now, &mut self.held);
        });

        if !repeat_enabled {
            self.held = None;
            return;
        }

        let repeat_event = self.held.as_mut().and_then(|held| {
            if held.payload.is_some() && now >= held.next_repeat_at {
                // Never catch up missed periods. A late frame produces one
                // repeat and restarts the 100 ms mechanical interval from now.
                held.next_repeat_at = now + REPEAT_INTERVAL_SECONDS;
                held.payload.as_ref().map(RepeatPayload::event)
            } else {
                None
            }
        });

        if let Some(event) = repeat_event {
            context.input_mut(|input| input.events.push(event));
        }

        if let Some(held) = self.held.as_ref()
            && held.payload.is_some()
        {
            let delay = (held.next_repeat_at - now).max(0.0);
            context.request_repaint_after(Duration::from_secs_f64(delay));
        }
    }
}

fn filter_and_track_events(
    events: &mut Vec<egui::Event>,
    repeat_enabled: bool,
    now: f64,
    held: &mut Option<HeldKey>,
) {
    let mut filtered = Vec::with_capacity(events.len());
    let mut suppress_following_text = false;

    for event in events.drain(..) {
        match event {
            // Host typematic is always discarded. When ASR repeat is enabled,
            // RepeatController synthesizes a paced event instead.
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
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => {
                suppress_following_text = false;
                if repeat_enabled && !is_application_shortcut(key) {
                    *held = Some(HeldKey {
                        key,
                        payload: key_route_payload(key, modifiers),
                        next_repeat_at: now + INITIAL_REPEAT_DELAY_SECONDS,
                    });
                } else if !repeat_enabled {
                    *held = None;
                }
                filtered.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
            }
            egui::Event::Key {
                key,
                pressed: false,
                repeat,
                modifiers,
                ..
            } => {
                suppress_following_text = false;
                if held.as_ref().is_some_and(|current| current.key == key) {
                    *held = None;
                }
                filtered.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat,
                    modifiers,
                });
            }
            egui::Event::Text(text) => {
                suppress_following_text = false;
                if repeat_enabled
                    && let Some(current) = held.as_mut()
                    && current.payload.is_none()
                {
                    current.payload = Some(RepeatPayload::Text(text.clone()));
                }
                filtered.push(egui::Event::Text(text));
            }
            event => {
                suppress_following_text = false;
                filtered.push(event);
            }
        }
    }

    *events = filtered;
}

fn key_route_payload(key: egui::Key, modifiers: egui::Modifiers) -> Option<RepeatPayload> {
    if modifiers.ctrl
        || matches!(key, egui::Key::Enter | egui::Key::Backspace | egui::Key::Tab)
    {
        Some(RepeatPayload::Key { key, modifiers })
    } else {
        None
    }
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

    fn key(key: egui::Key, pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn host_typematic_is_never_forwarded_even_when_asr_repeat_is_enabled() {
        for enabled in [false, true] {
            let mut held = None;
            let mut events = vec![
                key(egui::Key::A, true, true),
                egui::Event::Text("a".to_owned()),
            ];
            filter_and_track_events(&mut events, enabled, 1.0, &mut held);
            assert!(events.is_empty());
        }
    }

    #[test]
    fn printable_key_is_armed_from_initial_key_plus_text() {
        let mut held = None;
        let mut events = vec![
            key(egui::Key::A, true, false),
            egui::Event::Text("a".to_owned()),
        ];
        filter_and_track_events(&mut events, true, 2.0, &mut held);
        assert_eq!(events.len(), 2);
        let current = held.expect("held key");
        assert_eq!(current.key, egui::Key::A);
        assert!(matches!(current.payload, Some(RepeatPayload::Text(ref text)) if text == "a"));
        assert_eq!(current.next_repeat_at, 2.0 + INITIAL_REPEAT_DELAY_SECONDS);
    }

    #[test]
    fn release_cancels_future_repeat_immediately() {
        let mut held = None;
        let mut press = vec![
            key(egui::Key::A, true, false),
            egui::Event::Text("a".to_owned()),
        ];
        filter_and_track_events(&mut press, true, 3.0, &mut held);
        assert!(held.is_some());

        let mut release = vec![key(egui::Key::A, false, false)];
        filter_and_track_events(&mut release, true, 3.05, &mut held);
        assert!(held.is_none());
        assert_eq!(release.len(), 1);
    }

    #[test]
    fn enter_can_repeat_through_key_route_without_text_event() {
        let mut held = None;
        let mut events = vec![key(egui::Key::Enter, true, false)];
        filter_and_track_events(&mut events, true, 4.0, &mut held);
        assert!(matches!(
            held.and_then(|held| held.payload),
            Some(RepeatPayload::Key {
                key: egui::Key::Enter,
                ..
            })
        ));
    }

    #[test]
    fn application_shortcuts_never_arm_terminal_repeat() {
        for shortcut in [
            egui::Key::F5,
            egui::Key::F6,
            egui::Key::F7,
            egui::Key::F8,
            egui::Key::F9,
        ] {
            let mut held = None;
            let mut events = vec![key(shortcut, true, false)];
            filter_and_track_events(&mut events, true, 5.0, &mut held);
            assert!(held.is_none(), "{shortcut:?} armed repeat unexpectedly");
        }
    }
}
