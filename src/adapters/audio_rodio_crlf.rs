//! Rodio-backed ASR-33 mechanical audio adapter.
//!
//! This mirrors the proven audio architecture used by RusTair: rodio owns the
//! platform buffering, resampling and mixing while the deterministic core owns
//! the mechanical sound state. CR+LF is treated as one carriage-return action
//! for audio, matching RusTair: the CR one-shot is allowed to ring cleanly and
//! an immediately following LF does not layer the platen sample on top of it.
//!
//! All bundled sound samples are compiled into the executable. Runtime audio
//! never requires a sibling `sounds` directory or extracts temporary files.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::core::config::LidState;
use crate::core::terminal::CharacterEvent;

const AUDIO_EVENT_CAPACITY: usize = 256;
const CRLF_PAIR_WINDOW: Duration = Duration::from_millis(300);

#[must_use]
fn is_crlf_pair(last_cr_at: Option<Duration>, now: Duration) -> bool {
    last_cr_at.is_some_and(|cr_at| now.saturating_sub(cr_at) <= CRLF_PAIR_WINDOW)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioAvailability {
    Starting,
    Available,
    Unavailable(String),
    Stopped,
}

impl AudioAvailability {
    fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }
}

#[derive(Clone, Copy, Debug)]
enum AudioControl {
    SetMuted(bool),
    SetLid(LidState),
    SetTapeReader(bool),
    Shutdown,
}

#[derive(Clone, Copy, Debug)]
enum AudioEvent {
    Character(CharacterEvent),
    Keypress,
}

pub struct AudioEngine {
    controls: Sender<AudioControl>,
    events: SyncSender<AudioEvent>,
    statuses: Receiver<AudioAvailability>,
    worker: Option<JoinHandle<()>>,
    availability: AudioAvailability,
    shutdown_requested: bool,
}

impl AudioEngine {
    #[must_use]
    pub fn start(lid: LidState, muted: bool) -> Self {
        let (control_tx, control_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(AUDIO_EVENT_CAPACITY);
        let (status_tx, status_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("asr33-audio".to_owned())
            .spawn(move || platform::run(control_rx, event_rx, status_tx, lid, muted));
        match worker {
            Ok(worker) => Self {
                controls: control_tx,
                events: event_tx,
                statuses: status_rx,
                worker: Some(worker),
                availability: AudioAvailability::Starting,
                shutdown_requested: false,
            },
            Err(error) => Self {
                controls: control_tx,
                events: event_tx,
                statuses: status_rx,
                worker: None,
                availability: AudioAvailability::unavailable(format!(
                    "audio worker could not start: {error}"
                )),
                shutdown_requested: true,
            },
        }
    }

    pub fn character(&self, event: CharacterEvent) {
        self.try_event(AudioEvent::Character(event));
    }

    pub fn keypress(&self) {
        self.try_event(AudioEvent::Keypress);
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.controls.send(AudioControl::SetMuted(muted));
    }

    pub fn set_lid(&self, lid: LidState) {
        let _ = self.controls.send(AudioControl::SetLid(lid));
    }

    pub fn set_tape_reader_running(&self, running: bool) {
        let _ = self.controls.send(AudioControl::SetTapeReader(running));
    }

    pub fn refresh_status(&mut self) -> Option<AudioAvailability> {
        let mut changed = None;
        while let Ok(status) = self.statuses.try_recv() {
            self.availability = status.clone();
            changed = Some(status);
        }
        changed
    }

    #[must_use]
    pub fn availability(&self) -> &AudioAvailability {
        &self.availability
    }

    pub fn shutdown(&mut self) {
        if self.shutdown_requested {
            return;
        }
        self.shutdown_requested = true;
        let _ = self.controls.send(AudioControl::Shutdown);
    }

    pub fn join(&mut self) {
        self.shutdown();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                self.availability =
                    AudioAvailability::unavailable("audio worker terminated unexpectedly");
                return;
            }
            let _ = self.refresh_status();
            if !matches!(self.availability, AudioAvailability::Unavailable(_)) {
                self.availability = AudioAvailability::Stopped;
            }
        }
    }

    fn try_event(&self, event: AudioEvent) {
        match self.events.try_send(event) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.join();
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct SoundLibrary {
    sounds: &'static [(&'static str, &'static [u8])],
}

#[cfg(windows)]
impl SoundLibrary {
    fn embedded() -> Self {
        Self {
            sounds: super::embedded_sounds::EMBEDDED_SOUNDS,
        }
    }

    fn select(
        &self,
        lid: LidState,
        key: &str,
        variant: usize,
    ) -> Option<(&'static str, &'static [u8])> {
        let prefix = format!("{}-{key}", lid_prefix(lid));
        if let Some(sound) = self.sounds.iter().find(|(stem, _)| *stem == prefix) {
            return Some(*sound);
        }
        let variant_prefix = format!("{prefix}-");
        let matches = self
            .sounds
            .iter()
            .filter(|(stem, _)| stem.starts_with(&variant_prefix))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return None;
        }
        matches.get(variant % matches.len()).map(|sound| **sound)
    }
}

#[cfg(windows)]
fn lid_prefix(lid: LidState) -> &'static str {
    match lid {
        LidState::Up => "up",
        LidState::Down => "down",
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::thread;
    use std::time::{Duration, Instant};

    use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

    use super::{AudioAvailability, AudioControl, AudioEvent, SoundLibrary, is_crlf_pair};
    use crate::core::audio::{
        AudioSnapshot, AudioStateMachine, ContinuousSound, EffectRequest, EffectSound,
        MOTOR_OFF_PLAY_TIME,
    };
    use crate::core::config::LidState;

    const WORKER_POLL: Duration = Duration::from_millis(5);
    const SHUTDOWN_TAIL: Duration = Duration::from_millis(75);
    const MAX_ACTIVE_EFFECTS: usize = 8;

    pub(super) fn run(
        controls: std::sync::mpsc::Receiver<AudioControl>,
        events: std::sync::mpsc::Receiver<AudioEvent>,
        statuses: std::sync::mpsc::Sender<AudioAvailability>,
        lid: LidState,
        muted: bool,
    ) {
        let library = SoundLibrary::embedded();
        let mut output = match RodioOutput::open(&library, lid) {
            Ok(output) => output,
            Err(error) => {
                let _ = statuses.send(AudioAvailability::unavailable(error));
                wait_for_shutdown(controls);
                return;
            }
        };
        let _ = statuses.send(AudioAvailability::Available);

        let origin = Instant::now();
        let mut state = AudioStateMachine::new(lid, muted, Duration::ZERO);
        state.start(Duration::ZERO);
        let mut last_cr_at = None;
        let mut shutdown_deadline = None;
        let mut failed = false;

        loop {
            let now = origin.elapsed();
            loop {
                match controls.try_recv() {
                    Ok(AudioControl::SetMuted(value)) if shutdown_deadline.is_none() => {
                        state.set_muted(value, now);
                    }
                    Ok(AudioControl::SetLid(value)) if shutdown_deadline.is_none() => {
                        state.set_lid(value, now);
                    }
                    Ok(AudioControl::SetTapeReader(value)) if shutdown_deadline.is_none() => {
                        state.set_tape_reader_running(value);
                    }
                    Ok(AudioControl::Shutdown) => {
                        if shutdown_deadline.is_none() {
                            output.clear_effects();
                            state.prepare_shutdown(now);
                            shutdown_deadline = Some(now + MOTOR_OFF_PLAY_TIME + SHUTDOWN_TAIL);
                        }
                    }
                    Ok(_) => {}
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        if shutdown_deadline.is_none() {
                            output.clear_effects();
                            state.prepare_shutdown(now);
                            shutdown_deadline = Some(now + MOTOR_OFF_PLAY_TIME + SHUTDOWN_TAIL);
                        }
                        break;
                    }
                }
            }

            if shutdown_deadline.is_none() {
                for _ in 0..super::AUDIO_EVENT_CAPACITY {
                    match events.try_recv() {
                        Ok(AudioEvent::Character(event)) => match event.character {
                            '\r' => {
                                last_cr_at = Some(now);
                                state.process_character(event, now);
                            }
                            '\n' if is_crlf_pair(last_cr_at, now) => {
                                // RusTair's carriage-return sample represents the
                                // audible return action. Do not stack platen.wav
                                // on top when LF is the second half of CRLF.
                                last_cr_at = None;
                            }
                            _ => {
                                last_cr_at = None;
                                state.process_character(event, now);
                            }
                        },
                        Ok(AudioEvent::Keypress) => state.keypress(now),
                        Err(
                            std::sync::mpsc::TryRecvError::Empty
                            | std::sync::mpsc::TryRecvError::Disconnected,
                        ) => break,
                    }
                }
            }

            let snapshot = state.snapshot(now);
            if let Err(error) = output.service(&library, &mut state, snapshot) {
                failed = true;
                let _ = statuses.send(AudioAvailability::unavailable(error));
                break;
            }
            if shutdown_deadline.is_some_and(|deadline| now >= deadline) {
                break;
            }
            thread::sleep(WORKER_POLL);
        }

        output.stop_all();
        if !failed {
            let _ = statuses.send(AudioAvailability::Stopped);
        }
    }

    fn wait_for_shutdown(controls: std::sync::mpsc::Receiver<AudioControl>) {
        while let Ok(command) = controls.recv() {
            if matches!(command, AudioControl::Shutdown) {
                break;
            }
        }
    }

    struct ContinuousChannels {
        chars: Sink,
        spaces: Sink,
        hum: Sink,
        tape: Sink,
    }

    struct ActiveEffect {
        sink: Sink,
    }

    struct RodioOutput {
        stream: OutputStream,
        continuous: ContinuousChannels,
        effects: VecDeque<ActiveEffect>,
        lid: LidState,
        variant: usize,
    }

    impl RodioOutput {
        fn open(library: &SoundLibrary, lid: LidState) -> Result<Self, String> {
            let stream = OutputStreamBuilder::open_default_stream().map_err(|error| {
                format!("rodio could not open the default output device: {error}")
            })?;
            let mut output = Self {
                continuous: ContinuousChannels {
                    chars: Sink::connect_new(stream.mixer()),
                    spaces: Sink::connect_new(stream.mixer()),
                    hum: Sink::connect_new(stream.mixer()),
                    tape: Sink::connect_new(stream.mixer()),
                },
                stream,
                effects: VecDeque::new(),
                lid,
                variant: 0,
            };
            output.reload_continuous(library, lid)?;
            Ok(output)
        }

        fn service(
            &mut self,
            library: &SoundLibrary,
            state: &mut AudioStateMachine,
            snapshot: AudioSnapshot,
        ) -> Result<(), String> {
            if snapshot.lid != self.lid {
                self.reload_continuous(library, snapshot.lid)?;
            }
            self.continuous.chars.set_volume(snapshot.print_chars_gain);
            self.continuous
                .spaces
                .set_volume(snapshot.print_spaces_gain);
            self.continuous.hum.set_volume(snapshot.hum_gain);
            self.continuous.tape.set_volume(snapshot.tape_reader_gain);

            self.effects.retain(|effect| !effect.sink.empty());
            for effect in &self.effects {
                effect.sink.set_volume(snapshot.effects_gain);
            }

            while self.effects.len() < MAX_ACTIVE_EFFECTS {
                let Some(request) = state.take_next_effect() else {
                    break;
                };
                self.start_effect(library, snapshot.lid, request, snapshot.effects_gain)?;
            }
            Ok(())
        }

        fn reload_continuous(
            &mut self,
            library: &SoundLibrary,
            lid: LidState,
        ) -> Result<(), String> {
            self.continuous.chars.stop();
            self.continuous.spaces.stop();
            self.continuous.hum.stop();
            self.continuous.tape.stop();

            self.continuous = ContinuousChannels {
                chars: self.make_loop(library, lid, ContinuousSound::PrintChars)?,
                spaces: self.make_loop(library, lid, ContinuousSound::PrintSpaces)?,
                hum: self.make_loop(library, lid, ContinuousSound::Hum)?,
                tape: self.make_named_loop(library, lid, "tape-reader")?,
            };
            self.lid = lid;
            Ok(())
        }

        fn make_loop(
            &mut self,
            library: &SoundLibrary,
            lid: LidState,
            sound: ContinuousSound,
        ) -> Result<Sink, String> {
            let key = match sound {
                ContinuousSound::PrintChars => "print-chars",
                ContinuousSound::PrintSpaces => "print-spaces",
                ContinuousSound::Hum => "hum",
            };
            self.make_named_loop(library, lid, key)
        }

        fn make_named_loop(
            &mut self,
            library: &SoundLibrary,
            lid: LidState,
            key: &str,
        ) -> Result<Sink, String> {
            let sink = Sink::connect_new(self.stream.mixer());
            sink.set_volume(0.0);
            let variant = self.next_variant();
            if let Some((name, bytes)) = library.select(lid, key, variant) {
                let source = Decoder::try_from(Cursor::new(bytes))
                    .map_err(|error| format!("cannot decode embedded sound {name}: {error}"))?;
                sink.append(source.repeat_infinite());
            }
            Ok(sink)
        }

        fn start_effect(
            &mut self,
            library: &SoundLibrary,
            lid: LidState,
            request: EffectRequest,
            volume: f32,
        ) -> Result<(), String> {
            let key = match request.sound {
                EffectSound::Key => "key",
                EffectSound::Bell => "bell",
                EffectSound::CarriageReturn => "cr",
                EffectSound::Platen => "platen",
                EffectSound::MotorOn => "motor-on",
                EffectSound::MotorOff => "motor-off",
                EffectSound::Lid => "lid",
            };
            let variant = self.next_variant();
            let Some((name, bytes)) = library.select(lid, key, variant) else {
                return Ok(());
            };
            let source = Decoder::try_from(Cursor::new(bytes))
                .map_err(|error| format!("cannot decode embedded sound {name}: {error}"))?;
            let sink = Sink::connect_new(self.stream.mixer());
            sink.set_volume(volume);

            match request.sound {
                EffectSound::Key
                | EffectSound::Bell
                | EffectSound::CarriageReturn
                | EffectSound::Platen => sink.append(source),
                EffectSound::MotorOn | EffectSound::MotorOff | EffectSound::Lid => {
                    sink.append(source.take_duration(request.max_play_time));
                }
            }
            self.effects.push_back(ActiveEffect { sink });
            Ok(())
        }

        fn next_variant(&mut self) -> usize {
            let current = self.variant;
            self.variant = self.variant.wrapping_add(1);
            current
        }

        fn clear_effects(&mut self) {
            for effect in self.effects.drain(..) {
                effect.sink.stop();
            }
        }

        fn stop_all(&mut self) {
            self.continuous.chars.stop();
            self.continuous.spaces.stop();
            self.continuous.hum.stop();
            self.continuous.tape.stop();
            self.clear_effects();
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
    use std::time::Duration;

    use super::{AudioAvailability, AudioControl, AudioEvent};
    use crate::core::config::LidState;

    pub(super) fn run(
        controls: Receiver<AudioControl>,
        events: Receiver<AudioEvent>,
        statuses: Sender<AudioAvailability>,
        _lid: LidState,
        _muted: bool,
    ) {
        let _ = statuses.send(AudioAvailability::unavailable(
            "native ASR-33 audio output is currently implemented for Windows",
        ));
        loop {
            loop {
                match events.try_recv() {
                    Ok(AudioEvent::Character(event)) => {
                        let _ = (event.character, event.column);
                    }
                    Ok(AudioEvent::Keypress) => {}
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            match controls.recv_timeout(Duration::from_millis(50)) {
                Ok(AudioControl::SetMuted(value)) => {
                    let _ = value;
                }
                Ok(AudioControl::SetLid(value)) => {
                    let _ = value;
                }
                Ok(AudioControl::SetTapeReader(value)) => {
                    let _ = value;
                }
                Ok(AudioControl::Shutdown) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = statuses.send(AudioAvailability::Stopped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_pair_window_coalesces_only_nearby_line_feed() {
        let cr_at = Duration::from_millis(100);
        assert!(is_crlf_pair(Some(cr_at), Duration::from_millis(100)));
        assert!(is_crlf_pair(Some(cr_at), Duration::from_millis(400)));
        assert!(!is_crlf_pair(Some(cr_at), Duration::from_millis(401)));
        assert!(!is_crlf_pair(None, Duration::from_millis(100)));
    }

    #[test]
    fn availability_represents_device_failure_without_making_it_fatal() {
        assert_eq!(
            AudioAvailability::unavailable("no output device"),
            AudioAvailability::Unavailable("no output device".to_owned())
        );
    }

    #[cfg(windows)]
    #[test]
    fn embedded_sound_library_contains_both_lid_profiles() {
        let library = SoundLibrary::embedded();
        assert!(library.select(LidState::Up, "bell", 0).is_some());
        assert!(library.select(LidState::Down, "bell", 0).is_some());
        assert!(library.select(LidState::Up, "tape-reader", 0).is_some());
        assert!(library.select(LidState::Down, "print-chars", 1).is_some());
    }
}
