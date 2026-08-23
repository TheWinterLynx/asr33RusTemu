//! Deterministic ASR-33 mechanical-audio state.
//!
//! This module intentionally knows nothing about audio devices, files, GUI
//! frameworks, threads, or sleeps.  It translates terminal/user activity into
//! continuous-sound gains and a bounded queue of mechanical effects.

use std::collections::VecDeque;
use std::time::Duration;

use crate::core::config::LidState;
use crate::core::terminal::CharacterEvent;

pub const STATE_FADE_DURATION: Duration = Duration::from_millis(100);
/// One ASR-33 character time, plus a small scheduling guard.  The legacy
/// Python mixer kept its multi-character printer loop audible for 200 ms and
/// then faded it for another 100 ms; with the bundled 10 cps recordings that
/// exposed roughly three recorded strikes for one isolated character.  Keep
/// sustained 10 cps printing continuous, but return isolated activity to hum
/// after approximately one mechanical character cycle.
pub const INACTIVITY_TIMEOUT: Duration = Duration::from_millis(110);
pub const INACTIVITY_FADE_DURATION: Duration = Duration::from_millis(10);
pub const MUTE_FADE_DURATION: Duration = Duration::from_millis(200);
pub const DEFAULT_EFFECT_PLAY_TIME: Duration = Duration::from_millis(500);
pub const MOTOR_ON_PLAY_TIME: Duration = Duration::from_millis(1500);
pub const MOTOR_OFF_PLAY_TIME: Duration = Duration::from_millis(400);
pub const LID_PLAY_TIME: Duration = Duration::from_millis(250);
pub const BELL_PLAY_TIME: Duration = Duration::from_millis(500);
pub const PLATEN_PLAY_TIME: Duration = Duration::from_millis(100);
pub const CARRIAGE_RETURN_PLAY_TIME: Duration = Duration::from_millis(150);
pub const KEYPRESS_PLAY_TIME: Duration = Duration::from_millis(100);
/// Mechanical key effects are never allowed to build a rapid duplicate queue.
/// This is intentionally shorter than one ASR-33 character time so distinct
/// human key presses remain audible while duplicate host events are coalesced.
pub const KEYPRESS_DEBOUNCE: Duration = Duration::from_millis(80);
pub const MAX_PENDING_EFFECTS: usize = 4;
pub const COLUMN_BELL_COLUMN: usize = 62;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContinuousSound {
    PrintChars,
    PrintSpaces,
    Hum,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EffectSound {
    Key,
    Bell,
    CarriageReturn,
    Platen,
    MotorOn,
    MotorOff,
    Lid,
}

impl EffectSound {
    #[must_use]
    pub const fn legacy_play_time(self) -> Duration {
        match self {
            Self::Key => KEYPRESS_PLAY_TIME,
            Self::Bell => BELL_PLAY_TIME,
            Self::CarriageReturn => CARRIAGE_RETURN_PLAY_TIME,
            Self::Platen => PLATEN_PLAY_TIME,
            Self::MotorOn => MOTOR_ON_PLAY_TIME,
            Self::MotorOff => MOTOR_OFF_PLAY_TIME,
            Self::Lid => LID_PLAY_TIME,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectRequest {
    pub sound: EffectSound,
    pub max_play_time: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioSnapshot {
    pub print_chars_gain: f32,
    pub print_spaces_gain: f32,
    pub hum_gain: f32,
    pub tape_reader_gain: f32,
    pub effects_gain: f32,
    pub lid: LidState,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug)]
struct GainFade {
    start: Duration,
    duration: Duration,
    from: [f32; 3],
    to: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
struct ScalarFade {
    start: Duration,
    from: f32,
    to: f32,
}

#[derive(Debug)]
pub struct AudioStateMachine {
    lid: LidState,
    muted: bool,
    continuous: Option<ContinuousSound>,
    continuous_gains: [f32; 3],
    continuous_targets: [f32; 3],
    continuous_fade: Option<GainFade>,
    tape_reader_running: bool,
    mute_gain: f32,
    mute_fade: Option<ScalarFade>,
    last_event_time: Duration,
    last_keypress_time: Option<Duration>,
    effects: VecDeque<EffectRequest>,
}

impl AudioStateMachine {
    #[must_use]
    pub fn new(lid: LidState, muted: bool, now: Duration) -> Self {
        Self {
            lid,
            muted,
            continuous: None,
            continuous_gains: [0.0; 3],
            continuous_targets: [0.0; 3],
            continuous_fade: None,
            tape_reader_running: false,
            mute_gain: if muted { 0.0 } else { 1.0 },
            mute_fade: None,
            last_event_time: now,
            last_keypress_time: None,
            effects: VecDeque::new(),
        }
    }

    pub fn start(&mut self, now: Duration) {
        self.enqueue_effect(EffectSound::MotorOn, now);
    }

    pub fn prepare_shutdown(&mut self, now: Duration) {
        self.effects.clear();
        self.enqueue_effect(EffectSound::MotorOff, now);
    }

    pub fn keypress(&mut self, now: Duration) {
        self.advance(now);
        if self
            .last_keypress_time
            .is_some_and(|last| now.saturating_sub(last) < KEYPRESS_DEBOUNCE)
        {
            return;
        }
        self.last_keypress_time = Some(now);
        self.last_event_time = now;
        // A real ASR-33 cannot accumulate several independent key mechanisms
        // in parallel.  If the effect channel is already waiting to play a key
        // strike, collapse another host-side request into that pending strike.
        if !self
            .effects
            .iter()
            .any(|request| request.sound == EffectSound::Key)
        {
            self.push_effect(EffectSound::Key);
        }
    }

    pub fn process_character(&mut self, event: CharacterEvent, now: Duration) {
        self.advance(now);
        self.last_event_time = now;
        match event.character {
            '\r' => self.push_effect(EffectSound::CarriageReturn),
            '\n' => self.push_effect(EffectSound::Platen),
            '\u{7}' => self.push_effect(EffectSound::Bell),
            character if character <= ' ' || character > '~' => {
                self.transition_to(ContinuousSound::PrintSpaces, now);
            }
            _ => self.transition_to(ContinuousSound::PrintChars, now),
        }
        // The legacy frontends inspect the post-character terminal column and
        // enqueue the column bell after the ordinary character/effect event.
        if event.column == COLUMN_BELL_COLUMN {
            self.push_effect(EffectSound::Bell);
        }
    }

    pub fn set_tape_reader_running(&mut self, running: bool) {
        self.tape_reader_running = running;
    }

    pub fn set_muted(&mut self, muted: bool, now: Duration) {
        if self.muted == muted {
            return;
        }
        self.advance(now);
        self.muted = muted;
        self.mute_fade = Some(ScalarFade {
            start: now,
            from: self.mute_gain,
            to: if muted { 0.0 } else { 1.0 },
        });
    }

    pub fn set_lid(&mut self, lid: LidState, now: Duration) {
        if self.lid == lid {
            return;
        }
        self.lid = lid;
        self.enqueue_effect(EffectSound::Lid, now);
    }

    pub fn tick(&mut self, now: Duration) {
        self.advance(now);
        if self.continuous != Some(ContinuousSound::Hum)
            && now.saturating_sub(self.last_event_time) >= INACTIVITY_TIMEOUT
        {
            self.transition_to(ContinuousSound::Hum, now);
        }
    }

    #[must_use]
    pub fn snapshot(&mut self, now: Duration) -> AudioSnapshot {
        self.tick(now);
        AudioSnapshot {
            print_chars_gain: self.continuous_gains[0] * self.mute_gain,
            print_spaces_gain: self.continuous_gains[1] * self.mute_gain,
            hum_gain: self.continuous_gains[2] * self.mute_gain,
            tape_reader_gain: if self.tape_reader_running {
                self.mute_gain
            } else {
                0.0
            },
            effects_gain: self.mute_gain,
            lid: self.lid,
            muted: self.muted,
        }
    }

    #[must_use]
    pub fn current_continuous(&self) -> Option<ContinuousSound> {
        self.continuous
    }

    #[must_use]
    pub fn pending_effect_count(&self) -> usize {
        self.effects.len()
    }

    pub fn take_next_effect(&mut self) -> Option<EffectRequest> {
        self.effects.pop_front()
    }

    fn enqueue_effect(&mut self, sound: EffectSound, now: Duration) {
        self.advance(now);
        self.last_event_time = now;
        self.push_effect(sound);
    }

    fn push_effect(&mut self, sound: EffectSound) {
        if self.effects.len() < MAX_PENDING_EFFECTS {
            self.effects.push_back(EffectRequest {
                sound,
                max_play_time: sound.legacy_play_time(),
            });
        }
    }

    fn transition_to(&mut self, sound: ContinuousSound, now: Duration) {
        if self.continuous == Some(sound) {
            return;
        }
        self.advance(now);
        self.continuous = Some(sound);
        let target = match sound {
            ContinuousSound::PrintChars => [1.0, 0.0, 0.0],
            ContinuousSound::PrintSpaces => [0.0, 1.0, 0.0],
            ContinuousSound::Hum => [0.0, 0.0, 1.0],
        };
        self.continuous_targets = target;
        self.continuous_fade = Some(GainFade {
            start: now,
            duration: if sound == ContinuousSound::Hum {
                INACTIVITY_FADE_DURATION
            } else {
                STATE_FADE_DURATION
            },
            from: self.continuous_gains,
            to: target,
        });
    }

    fn advance(&mut self, now: Duration) {
        if let Some(fade) = self.continuous_fade {
            let progress = fade_progress(now, fade.start, fade.duration);
            self.continuous_gains = std::array::from_fn(|index| {
                lerp(fade.from[index], fade.to[index], progress)
            });
            if progress >= 1.0 {
                self.continuous_gains = self.continuous_targets;
                self.continuous_fade = None;
            }
        }
        if let Some(fade) = self.mute_fade {
            let progress = fade_progress(now, fade.start, MUTE_FADE_DURATION);
            self.mute_gain = lerp(fade.from, fade.to, progress).clamp(0.0, 1.0);
            if progress >= 1.0 {
                self.mute_gain = fade.to;
                self.mute_fade = None;
            }
        }
    }
}

fn fade_progress(now: Duration, start: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }
    (now.saturating_sub(start).as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

fn lerp(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(character: char, column: usize) -> CharacterEvent {
        CharacterEvent { character, column }
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }

    #[test]
    fn printable_and_space_activity_select_legacy_continuous_loops() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.process_character(event('A', 1), Duration::ZERO);
        assert_eq!(audio.current_continuous(), Some(ContinuousSound::PrintChars));
        let snapshot = audio.snapshot(STATE_FADE_DURATION);
        assert_near(snapshot.print_chars_gain, 1.0);
        assert_near(snapshot.print_spaces_gain, 0.0);

        audio.process_character(event(' ', 2), STATE_FADE_DURATION);
        assert_eq!(audio.current_continuous(), Some(ContinuousSound::PrintSpaces));
        let snapshot = audio.snapshot(STATE_FADE_DURATION * 2);
        assert_near(snapshot.print_chars_gain, 0.0);
        assert_near(snapshot.print_spaces_gain, 1.0);
    }

    #[test]
    fn carriage_return_line_feed_bell_and_column_bell_preserve_effect_order() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.process_character(event('\r', COLUMN_BELL_COLUMN), Duration::ZERO);
        audio.process_character(event('\n', 0), Duration::from_millis(1));
        audio.process_character(event('\u{7}', 0), Duration::from_millis(2));
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::CarriageReturn);
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::Bell);
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::Platen);
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::Bell);
    }

    #[test]
    fn isolated_print_activity_returns_to_hum_after_one_character_cycle() {
        let mut audio = AudioStateMachine::new(LidState::Down, false, Duration::ZERO);
        audio.process_character(event('X', 1), Duration::ZERO);
        audio.tick(INACTIVITY_TIMEOUT - Duration::from_millis(1));
        assert_eq!(audio.current_continuous(), Some(ContinuousSound::PrintChars));
        audio.tick(INACTIVITY_TIMEOUT);
        assert_eq!(audio.current_continuous(), Some(ContinuousSound::Hum));
        let snapshot = audio.snapshot(INACTIVITY_TIMEOUT + INACTIVITY_FADE_DURATION);
        assert_near(snapshot.print_chars_gain, 0.0);
        assert_near(snapshot.hum_gain, 1.0);
    }

    #[test]
    fn sustained_ten_cps_activity_does_not_fall_back_to_hum_between_characters() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        for index in 0..5 {
            let now = Duration::from_millis(index * 100);
            audio.process_character(event('A', index as usize + 1), now);
            audio.tick(now + Duration::from_millis(99));
            assert_eq!(audio.current_continuous(), Some(ContinuousSound::PrintChars));
        }
    }

    #[test]
    fn rapid_duplicate_keypresses_are_coalesced_instead_of_echoing() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.keypress(Duration::ZERO);
        audio.keypress(Duration::from_millis(5));
        audio.keypress(Duration::from_millis(20));
        assert_eq!(audio.pending_effect_count(), 1);
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::Key);

        audio.keypress(KEYPRESS_DEBOUNCE);
        assert_eq!(audio.pending_effect_count(), 1);
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::Key);
    }

    #[test]
    fn mute_and_unmute_are_two_hundred_millisecond_fades() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.set_tape_reader_running(true);
        audio.set_muted(true, Duration::ZERO);
        let half = audio.snapshot(MUTE_FADE_DURATION / 2);
        assert_near(half.tape_reader_gain, 0.5);
        let muted = audio.snapshot(MUTE_FADE_DURATION);
        assert_near(muted.tape_reader_gain, 0.0);
        assert!(muted.muted);

        audio.set_muted(false, MUTE_FADE_DURATION);
        let half = audio.snapshot(MUTE_FADE_DURATION + MUTE_FADE_DURATION / 2);
        assert_near(half.tape_reader_gain, 0.5);
        let unmuted = audio.snapshot(MUTE_FADE_DURATION * 2);
        assert_near(unmuted.tape_reader_gain, 1.0);
        assert!(!unmuted.muted);
    }

    #[test]
    fn lid_change_queues_one_effect_and_redundant_set_is_silent() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.set_lid(LidState::Down, Duration::ZERO);
        audio.set_lid(LidState::Down, Duration::from_millis(1));
        assert_eq!(audio.pending_effect_count(), 1);
        assert_eq!(audio.take_next_effect().unwrap().sound, EffectSound::Lid);
        assert_eq!(audio.snapshot(Duration::ZERO).lid, LidState::Down);
    }

    #[test]
    fn pending_effect_queue_retains_legacy_cap_of_four() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.process_character(event('\r', COLUMN_BELL_COLUMN), Duration::ZERO);
        audio.process_character(event('\n', 0), Duration::from_millis(1));
        audio.keypress(Duration::from_millis(100));
        assert_eq!(audio.pending_effect_count(), MAX_PENDING_EFFECTS);
        audio.process_character(event('\u{7}', 0), Duration::from_millis(101));
        assert_eq!(audio.pending_effect_count(), MAX_PENDING_EFFECTS);
    }

    #[test]
    fn tape_reader_loop_is_independent_and_obeys_global_mute() {
        let mut audio = AudioStateMachine::new(LidState::Up, false, Duration::ZERO);
        audio.set_tape_reader_running(true);
        assert_near(audio.snapshot(Duration::ZERO).tape_reader_gain, 1.0);
        audio.set_muted(true, Duration::ZERO);
        assert_near(audio.snapshot(MUTE_FADE_DURATION).tape_reader_gain, 0.0);
        audio.set_tape_reader_running(false);
        assert_near(audio.snapshot(MUTE_FADE_DURATION).tape_reader_gain, 0.0);
    }

    #[test]
    fn startup_and_shutdown_effects_use_legacy_play_times() {
        let mut audio = AudioStateMachine::new(LidState::Down, false, Duration::ZERO);
        audio.start(Duration::ZERO);
        let on = audio.take_next_effect().unwrap();
        assert_eq!(on.sound, EffectSound::MotorOn);
        assert_eq!(on.max_play_time, MOTOR_ON_PLAY_TIME);
        audio.prepare_shutdown(Duration::from_secs(1));
        let off = audio.take_next_effect().unwrap();
        assert_eq!(off.sound, EffectSound::MotorOff);
        assert_eq!(off.max_play_time, MOTOR_OFF_PLAY_TIME);
    }
}
