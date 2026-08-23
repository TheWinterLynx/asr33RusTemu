//! Native ASR-33 mechanical-audio adapter.
//!
//! The bundled Hugh Pyle recordings are treated as the source of truth for
//! timbre.  Continuous recordings (printer, hum and tape reader) are looped,
//! while mechanical effects are played as complete one-shots.  The Windows
//! backend uses waveOut on a dedicated worker thread; UI/core code never
//! blocks on audio and audio failure is never fatal to the emulator.

use std::error::Error;
use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};

#[cfg(any(windows, test))]
use std::fs;
#[cfg(any(windows, test))]
use std::path::{Path, PathBuf};
#[cfg(any(windows, test))]
use std::sync::Arc;

#[cfg(any(windows, test))]
use crate::core::audio::{ContinuousSound, EffectSound};
use crate::core::config::LidState;
use crate::core::terminal::CharacterEvent;

const AUDIO_EVENT_CAPACITY: usize = 256;

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
        loop {
            match self.statuses.try_recv() {
                Ok(status) => {
                    self.availability = status.clone();
                    changed = Some(status);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WavError {
    TooShort,
    NotRiffWave,
    TruncatedChunk,
    MissingFormat,
    MissingData,
    UnsupportedEncoding(u16),
    UnsupportedChannels(u16),
    UnsupportedBits(u16),
    InvalidBlockAlignment,
    EmptyData,
}

impl fmt::Display for WavError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => formatter.write_str("WAV file is too short"),
            Self::NotRiffWave => formatter.write_str("file is not RIFF/WAVE"),
            Self::TruncatedChunk => formatter.write_str("WAV chunk is truncated"),
            Self::MissingFormat => formatter.write_str("WAV fmt chunk is missing"),
            Self::MissingData => formatter.write_str("WAV data chunk is missing"),
            Self::UnsupportedEncoding(value) => {
                write!(formatter, "unsupported WAV encoding {value}")
            }
            Self::UnsupportedChannels(value) => {
                write!(formatter, "unsupported WAV channel count {value}")
            }
            Self::UnsupportedBits(value) => {
                write!(formatter, "unsupported WAV sample size {value} bits")
            }
            Self::InvalidBlockAlignment => formatter.write_str("invalid WAV block alignment"),
            Self::EmptyData => formatter.write_str("WAV contains no PCM frames"),
        }
    }
}

impl Error for WavError {}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct PcmClip {
    sample_rate: u32,
    channels: u16,
    samples: Arc<Vec<i16>>,
}

#[cfg(any(windows, test))]
impl PcmClip {
    fn frame_count(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    fn stereo_frame(&self, position: f64) -> Option<(f32, f32)> {
        let frame = position.floor() as usize;
        if frame >= self.frame_count() {
            return None;
        }
        let channels = usize::from(self.channels);
        let base = frame * channels;
        let left = f32::from(self.samples[base]) / 32_768.0;
        let right = if channels == 1 {
            left
        } else {
            f32::from(self.samples[base + 1]) / 32_768.0
        };
        Some((left, right))
    }
}

#[cfg(any(windows, test))]
fn parse_pcm_wave(bytes: &[u8]) -> Result<PcmClip, WavError> {
    if bytes.len() < 12 {
        return Err(WavError::TooShort);
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotRiffWave);
    }

    let mut format = None;
    let mut data = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let start = offset + 8;
        let end = start.checked_add(size).ok_or(WavError::TruncatedChunk)?;
        if end > bytes.len() {
            return Err(WavError::TruncatedChunk);
        }
        if id == b"fmt " {
            if size < 16 {
                return Err(WavError::TruncatedChunk);
            }
            format = Some((
                u16::from_le_bytes([bytes[start], bytes[start + 1]]),
                u16::from_le_bytes([bytes[start + 2], bytes[start + 3]]),
                u32::from_le_bytes([
                    bytes[start + 4],
                    bytes[start + 5],
                    bytes[start + 6],
                    bytes[start + 7],
                ]),
                u16::from_le_bytes([bytes[start + 12], bytes[start + 13]]),
                u16::from_le_bytes([bytes[start + 14], bytes[start + 15]]),
            ));
        } else if id == b"data" && data.is_none() {
            data = Some(&bytes[start..end]);
        }
        offset = end + (size & 1);
    }

    let (encoding, channels, sample_rate, block_align, bits) =
        format.ok_or(WavError::MissingFormat)?;
    if encoding != 1 {
        return Err(WavError::UnsupportedEncoding(encoding));
    }
    if !matches!(channels, 1 | 2) {
        return Err(WavError::UnsupportedChannels(channels));
    }
    if bits != 16 {
        return Err(WavError::UnsupportedBits(bits));
    }
    let expected_align = channels * 2;
    if block_align != expected_align {
        return Err(WavError::InvalidBlockAlignment);
    }
    let data = data.ok_or(WavError::MissingData)?;
    if data.is_empty() || data.len() % usize::from(expected_align) != 0 {
        return Err(WavError::EmptyData);
    }
    let samples = data
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect::<Vec<_>>();
    Ok(PcmClip {
        sample_rate,
        channels,
        samples: Arc::new(samples),
    })
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct LoadedSound {
    stem: String,
    clip: Arc<PcmClip>,
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct SoundLibrary {
    sounds: Vec<LoadedSound>,
}

#[cfg(any(windows, test))]
impl SoundLibrary {
    fn load(directory: &Path) -> Result<Self, String> {
        let entries = fs::read_dir(directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("wav"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut sounds = Vec::new();
        for path in paths {
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(clip) = parse_pcm_wave(&bytes) else {
                continue;
            };
            sounds.push(LoadedSound {
                stem: stem.to_owned(),
                clip: Arc::new(clip),
            });
        }
        if sounds.is_empty() {
            return Err(format!(
                "no usable PCM WAV files found in {}",
                directory.display()
            ));
        }
        Ok(Self { sounds })
    }

    fn select(&self, lid: LidState, key: &str, variant: usize) -> Option<Arc<PcmClip>> {
        let prefix = format!("{}-{key}", lid_prefix(lid));
        if let Some(exact) = self.sounds.iter().find(|sound| sound.stem == prefix) {
            return Some(Arc::clone(&exact.clip));
        }
        let matches = self
            .sounds
            .iter()
            .filter(|sound| sound.stem.starts_with(&prefix))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return None;
        }
        matches
            .get(variant % matches.len())
            .map(|sound| Arc::clone(&sound.clip))
    }
}

#[cfg(any(windows, test))]
fn lid_prefix(lid: LidState) -> &'static str {
    match lid {
        LidState::Up => "up",
        LidState::Down => "down",
    }
}

#[cfg(any(windows, test))]
fn continuous_key(sound: ContinuousSound) -> &'static str {
    match sound {
        ContinuousSound::PrintChars => "print-chars",
        ContinuousSound::PrintSpaces => "print-spaces",
        ContinuousSound::Hum => "hum",
    }
}

#[cfg(any(windows, test))]
fn effect_key(sound: EffectSound) -> &'static str {
    match sound {
        EffectSound::Key => "key",
        EffectSound::Bell => "bell",
        EffectSound::CarriageReturn => "cr",
        EffectSound::Platen => "platen",
        EffectSound::MotorOn => "motor-on",
        EffectSound::MotorOff => "motor-off",
        EffectSound::Lid => "lid",
    }
}

#[cfg(any(windows, test))]
fn default_sound_directories() -> Vec<PathBuf> {
    fn push_unique(directories: &mut Vec<PathBuf>, path: PathBuf) {
        if !directories.contains(&path) {
            directories.push(path);
        }
    }

    let mut directories = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        push_unique(&mut directories, parent.join("sounds"));
    }
    if let Ok(current) = std::env::current_dir() {
        push_unique(&mut directories, current.join("sounds"));
    }
    push_unique(
        &mut directories,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sounds"),
    );
    directories
}

#[cfg(any(windows, test))]
fn load_default_library() -> Result<SoundLibrary, String> {
    let mut errors = Vec::new();
    for directory in default_sound_directories() {
        match SoundLibrary::load(&directory) {
            Ok(library) => return Ok(library),
            Err(error) => errors.push(error),
        }
    }
    Err(errors.join("; "))
}

fn priority_mix(background: f32, effect: f32) -> f32 {
    let effect = effect.clamp(-1.0, 1.0);
    let candidate = effect + background;
    if (-1.0..=1.0).contains(&candidate) {
        return candidate;
    }
    if background == 0.0 {
        return effect;
    }
    let allowed_background = if candidate > 1.0 {
        1.0 - effect
    } else {
        -1.0 - effect
    };
    let scale = (allowed_background / background).clamp(0.0, 1.0);
    (effect + background * scale).clamp(-1.0, 1.0)
}

#[cfg(windows)]
mod platform {
    use std::mem::size_of;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, TryRecvError};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Media::Audio::{
        HWAVEOUT, WAVERR_STILLPLAYING, WAVEFORMATEX, WAVEHDR, waveOutClose, waveOutOpen,
        waveOutPrepareHeader, waveOutReset, waveOutUnprepareHeader, waveOutWrite,
    };

    use super::{
        AUDIO_EVENT_CAPACITY, AudioAvailability, AudioControl, AudioEvent, PcmClip, SoundLibrary,
        continuous_key, effect_key, load_default_library, priority_mix,
    };
    use crate::core::audio::{ContinuousSound, EffectSound};
    use crate::core::config::LidState;

    const OUTPUT_SAMPLE_RATE: u32 = 48_000;
    const OUTPUT_CHANNELS: u16 = 2;
    const OUTPUT_BITS: u16 = 16;
    const BUFFER_FRAMES: usize = 240; // 5 ms at 48 kHz
    const BUFFER_COUNT: usize = 4;
    const TARGET_QUEUED_BUFFERS: usize = 3;
    const WORKER_POLL: Duration = Duration::from_millis(1);
    const PRINT_ATTACK: Duration = Duration::from_millis(8);
    const PRINT_HOLD: Duration = Duration::from_millis(105);
    const PRINT_RELEASE: Duration = Duration::from_millis(20);
    const MUTE_FADE: Duration = Duration::from_millis(200);
    const KEYPRESS_DEBOUNCE: Duration = Duration::from_millis(70);
    const SHUTDOWN_TAIL: Duration = Duration::from_millis(500);
    const MAX_EFFECT_VOICES: usize = 12;
    const EFFECT_BACKGROUND_DUCK: f32 = 0.35;

    pub(super) fn run(
        controls: Receiver<AudioControl>,
        events: Receiver<AudioEvent>,
        statuses: Sender<AudioAvailability>,
        mut lid: LidState,
        muted: bool,
    ) {
        let library = match load_default_library() {
            Ok(library) => library,
            Err(error) => {
                let _ = statuses.send(AudioAvailability::unavailable(error));
                wait_for_shutdown(controls);
                return;
            }
        };
        let mut output = match WaveOutput::open() {
            Ok(output) => output,
            Err(error) => {
                let _ = statuses.send(AudioAvailability::unavailable(error));
                wait_for_shutdown(controls);
                return;
            }
        };
        let _ = statuses.send(AudioAvailability::Available);

        let origin = Instant::now();
        let mut model = AudioModel::new(muted);
        let mut mixer = Mixer::new(&library, lid);
        mixer.trigger_effect(&library, lid, EffectSound::MotorOn);
        let mut last_keypress = None;
        let mut shutdown_deadline = None;
        let mut failed = false;

        loop {
            let now = origin.elapsed();
            loop {
                match controls.try_recv() {
                    Ok(AudioControl::SetMuted(value)) if shutdown_deadline.is_none() => {
                        model.set_muted(value, now);
                    }
                    Ok(AudioControl::SetLid(value)) if shutdown_deadline.is_none() => {
                        if value != lid {
                            lid = value;
                            mixer.reload_continuous(&library, lid);
                            mixer.trigger_effect(&library, lid, EffectSound::Lid);
                        }
                    }
                    Ok(AudioControl::SetTapeReader(value)) if shutdown_deadline.is_none() => {
                        model.set_tape_reader_running(value);
                    }
                    Ok(AudioControl::Shutdown) => {
                        if shutdown_deadline.is_none() {
                            mixer.clear_effects();
                            mixer.trigger_effect(&library, lid, EffectSound::MotorOff);
                            shutdown_deadline = Some(now + SHUTDOWN_TAIL);
                        }
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if shutdown_deadline.is_none() {
                            mixer.clear_effects();
                            mixer.trigger_effect(&library, lid, EffectSound::MotorOff);
                            shutdown_deadline = Some(now + SHUTDOWN_TAIL);
                        }
                        break;
                    }
                }
            }

            if shutdown_deadline.is_none() {
                for _ in 0..AUDIO_EVENT_CAPACITY {
                    match events.try_recv() {
                        Ok(AudioEvent::Character(event)) => {
                            handle_character(event, now, &library, lid, &mut model, &mut mixer);
                        }
                        Ok(AudioEvent::Keypress) => {
                            let accepted = last_keypress
                                .is_none_or(|last| now.saturating_sub(last) >= KEYPRESS_DEBOUNCE);
                            if accepted {
                                last_keypress = Some(now);
                                mixer.trigger_effect(&library, lid, EffectSound::Key);
                            }
                        }
                        Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                    }
                }
            }

            if let Err(error) = output.service(&mut mixer, &mut model, now) {
                failed = true;
                let _ = statuses.send(AudioAvailability::unavailable(error));
                break;
            }
            if shutdown_deadline.is_some_and(|deadline| now >= deadline) {
                break;
            }
            thread::sleep(WORKER_POLL);
        }
        output.close();
        if !failed {
            let _ = statuses.send(AudioAvailability::Stopped);
        }
    }

    fn handle_character(
        event: crate::core::terminal::CharacterEvent,
        now: Duration,
        library: &SoundLibrary,
        lid: LidState,
        model: &mut AudioModel,
        mixer: &mut Mixer,
    ) {
        model.note_activity(now);
        match event.character {
            '\r' => mixer.trigger_effect(library, lid, EffectSound::CarriageReturn),
            '\n' => mixer.trigger_effect(library, lid, EffectSound::Platen),
            '\u{7}' => mixer.trigger_effect(library, lid, EffectSound::Bell),
            character if character <= ' ' || character > '~' => {
                if model.select_print(ContinuousSound::PrintSpaces, now) {
                    mixer.rewind_print(ContinuousSound::PrintSpaces);
                }
            }
            _ => {
                if model.select_print(ContinuousSound::PrintChars, now) {
                    mixer.rewind_print(ContinuousSound::PrintChars);
                }
            }
        }
        if event.column == 62 {
            mixer.trigger_effect(library, lid, EffectSound::Bell);
        }
    }

    fn wait_for_shutdown(controls: Receiver<AudioControl>) {
        while let Ok(command) = controls.recv() {
            if matches!(command, AudioControl::Shutdown) {
                break;
            }
        }
    }

    #[derive(Clone, Copy)]
    struct GainFade {
        start: Duration,
        duration: Duration,
        from: [f32; 3],
        to: [f32; 3],
    }

    #[derive(Clone, Copy)]
    struct ScalarFade {
        start: Duration,
        from: f32,
        to: f32,
    }

    #[derive(Clone, Copy)]
    struct AudioSnapshot {
        print_chars: f32,
        print_spaces: f32,
        hum: f32,
        tape: f32,
        effects: f32,
    }

    struct AudioModel {
        mode: ContinuousSound,
        gains: [f32; 3],
        targets: [f32; 3],
        fade: Option<GainFade>,
        last_activity: Duration,
        tape_reader_running: bool,
        muted: bool,
        mute_gain: f32,
        mute_fade: Option<ScalarFade>,
    }

    impl AudioModel {
        fn new(muted: bool) -> Self {
            Self {
                mode: ContinuousSound::Hum,
                gains: [0.0, 0.0, 1.0],
                targets: [0.0, 0.0, 1.0],
                fade: None,
                last_activity: Duration::ZERO,
                tape_reader_running: false,
                muted,
                mute_gain: if muted { 0.0 } else { 1.0 },
                mute_fade: None,
            }
        }

        fn note_activity(&mut self, now: Duration) {
            self.last_activity = now;
        }

        fn select_print(&mut self, sound: ContinuousSound, now: Duration) -> bool {
            self.last_activity = now;
            if self.mode == sound {
                return false;
            }
            self.advance(now);
            self.mode = sound;
            let target = match sound {
                ContinuousSound::PrintChars => [1.0, 0.0, 0.0],
                ContinuousSound::PrintSpaces => [0.0, 1.0, 0.0],
                ContinuousSound::Hum => [0.0, 0.0, 1.0],
            };
            self.start_fade(target, now, PRINT_ATTACK);
            true
        }

        fn set_tape_reader_running(&mut self, running: bool) {
            self.tape_reader_running = running;
        }

        fn set_muted(&mut self, muted: bool, now: Duration) {
            if muted == self.muted {
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

        fn snapshot(&mut self, now: Duration) -> AudioSnapshot {
            self.advance(now);
            if self.mode != ContinuousSound::Hum
                && now.saturating_sub(self.last_activity) >= PRINT_HOLD
            {
                self.mode = ContinuousSound::Hum;
                self.start_fade([0.0, 0.0, 1.0], now, PRINT_RELEASE);
            }
            self.advance(now);
            AudioSnapshot {
                print_chars: self.gains[0] * self.mute_gain,
                print_spaces: self.gains[1] * self.mute_gain,
                hum: self.gains[2] * self.mute_gain,
                tape: if self.tape_reader_running {
                    self.mute_gain
                } else {
                    0.0
                },
                effects: self.mute_gain,
            }
        }

        fn start_fade(&mut self, target: [f32; 3], now: Duration, duration: Duration) {
            self.targets = target;
            self.fade = Some(GainFade {
                start: now,
                duration,
                from: self.gains,
                to: target,
            });
        }

        fn advance(&mut self, now: Duration) {
            if let Some(fade) = self.fade {
                let progress = fade_progress(now, fade.start, fade.duration);
                self.gains = std::array::from_fn(|index| {
                    lerp(fade.from[index], fade.to[index], progress)
                });
                if progress >= 1.0 {
                    self.gains = self.targets;
                    self.fade = None;
                }
            }
            if let Some(fade) = self.mute_fade {
                let progress = fade_progress(now, fade.start, MUTE_FADE);
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

    struct LoopVoice {
        clip: Option<Arc<PcmClip>>,
        position: f64,
    }

    impl LoopVoice {
        fn new(clip: Option<Arc<PcmClip>>) -> Self {
            Self {
                clip,
                position: 0.0,
            }
        }

        fn rewind(&mut self) {
            self.position = 0.0;
        }

        fn next(&mut self) -> (f32, f32) {
            let Some(clip) = self.clip.as_ref() else {
                return (0.0, 0.0);
            };
            let frame_count = clip.frame_count();
            if frame_count == 0 {
                return (0.0, 0.0);
            }
            let frame = clip.stereo_frame(self.position).unwrap_or((0.0, 0.0));
            self.position += f64::from(clip.sample_rate) / f64::from(OUTPUT_SAMPLE_RATE);
            if self.position >= frame_count as f64 {
                self.position %= frame_count as f64;
            }
            frame
        }
    }

    struct EffectVoice {
        sound: EffectSound,
        clip: Arc<PcmClip>,
        position: f64,
    }

    impl EffectVoice {
        fn next(&mut self) -> Option<(f32, f32)> {
            let frame = self.clip.stereo_frame(self.position)?;
            self.position += f64::from(self.clip.sample_rate) / f64::from(OUTPUT_SAMPLE_RATE);
            Some(frame)
        }
    }

    struct Mixer {
        chars: LoopVoice,
        spaces: LoopVoice,
        hum: LoopVoice,
        tape: LoopVoice,
        effects: Vec<EffectVoice>,
        variant: usize,
    }

    impl Mixer {
        fn new(library: &SoundLibrary, lid: LidState) -> Self {
            let mut mixer = Self {
                chars: LoopVoice::new(None),
                spaces: LoopVoice::new(None),
                hum: LoopVoice::new(None),
                tape: LoopVoice::new(None),
                effects: Vec::new(),
                variant: 0,
            };
            mixer.reload_continuous(library, lid);
            mixer
        }

        fn clear_effects(&mut self) {
            self.effects.clear();
        }

        fn reload_continuous(&mut self, library: &SoundLibrary, lid: LidState) {
            let chars = self.select_continuous(library, lid, ContinuousSound::PrintChars);
            let spaces = self.select_continuous(library, lid, ContinuousSound::PrintSpaces);
            let hum = self.select_continuous(library, lid, ContinuousSound::Hum);
            let tape_variant = self.next_variant();
            let tape = library.select(lid, "tape-reader", tape_variant);
            self.chars = LoopVoice::new(chars);
            self.spaces = LoopVoice::new(spaces);
            self.hum = LoopVoice::new(hum);
            self.tape = LoopVoice::new(tape);
        }

        fn rewind_print(&mut self, sound: ContinuousSound) {
            match sound {
                ContinuousSound::PrintChars => self.chars.rewind(),
                ContinuousSound::PrintSpaces => self.spaces.rewind(),
                ContinuousSound::Hum => {}
            }
        }

        fn select_continuous(
            &mut self,
            library: &SoundLibrary,
            lid: LidState,
            sound: ContinuousSound,
        ) -> Option<Arc<PcmClip>> {
            let variant = self.next_variant();
            library.select(lid, continuous_key(sound), variant)
        }

        fn next_variant(&mut self) -> usize {
            let current = self.variant;
            self.variant = self.variant.wrapping_add(1);
            current
        }

        fn trigger_effect(&mut self, library: &SoundLibrary, lid: LidState, sound: EffectSound) {
            self.effects.retain(|voice| voice.position < voice.clip.frame_count() as f64);
            if self.effects.len() >= MAX_EFFECT_VOICES {
                return;
            }
            let same_kind = self
                .effects
                .iter()
                .filter(|voice| voice.sound == sound)
                .count();
            if same_kind >= effect_polyphony(sound) {
                return;
            }
            let variant = self.next_variant();
            if let Some(clip) = library.select(lid, effect_key(sound), variant) {
                self.effects.push(EffectVoice {
                    sound,
                    clip,
                    position: 0.0,
                });
            }
        }

        fn render(&mut self, samples: &mut [i16], model: &mut AudioModel, now: Duration) {
            let snapshot = model.snapshot(now);
            for frame in samples.chunks_exact_mut(2) {
                let (chars_l, chars_r) = self.chars.next();
                let (spaces_l, spaces_r) = self.spaces.next();
                let (hum_l, hum_r) = self.hum.next();
                let (tape_l, tape_r) = self.tape.next();

                let background_gain_sum = snapshot.print_chars
                    + snapshot.print_spaces
                    + snapshot.hum
                    + snapshot.tape;
                let background_scale = 1.0 / background_gain_sum.max(1.0);
                let mut background_l = (chars_l * snapshot.print_chars
                    + spaces_l * snapshot.print_spaces
                    + hum_l * snapshot.hum
                    + tape_l * snapshot.tape)
                    * background_scale;
                let mut background_r = (chars_r * snapshot.print_chars
                    + spaces_r * snapshot.print_spaces
                    + hum_r * snapshot.hum
                    + tape_r * snapshot.tape)
                    * background_scale;

                let mut effect_l = 0.0f32;
                let mut effect_r = 0.0f32;
                let mut active_effects = 0usize;
                let mut index = 0usize;
                while index < self.effects.len() {
                    if let Some((left, right)) = self.effects[index].next() {
                        effect_l += left;
                        effect_r += right;
                        active_effects += 1;
                        index += 1;
                    } else {
                        self.effects.remove(index);
                    }
                }
                if active_effects > 1 {
                    let scale = 1.0 / active_effects as f32;
                    effect_l *= scale;
                    effect_r *= scale;
                }
                effect_l *= snapshot.effects;
                effect_r *= snapshot.effects;

                if active_effects != 0 {
                    let peak = effect_l.abs().max(effect_r.abs());
                    let duck = (1.0 - peak * (1.0 - EFFECT_BACKGROUND_DUCK))
                        .clamp(EFFECT_BACKGROUND_DUCK, 1.0);
                    background_l *= duck;
                    background_r *= duck;
                }

                frame[0] = float_to_i16(priority_mix(background_l, effect_l));
                frame[1] = float_to_i16(priority_mix(background_r, effect_r));
            }
        }
    }

    fn effect_polyphony(sound: EffectSound) -> usize {
        match sound {
            EffectSound::Key => 2,
            EffectSound::Bell => 2,
            EffectSound::CarriageReturn | EffectSound::Platen => 2,
            EffectSound::MotorOn | EffectSound::MotorOff | EffectSound::Lid => 1,
        }
    }

    fn float_to_i16(sample: f32) -> i16 {
        (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16
    }

    struct OutputBuffer {
        samples: Vec<i16>,
        header: Box<WAVEHDR>,
        queued: bool,
        prepared: bool,
    }

    impl OutputBuffer {
        fn new() -> Self {
            let mut samples = vec![0i16; BUFFER_FRAMES * usize::from(OUTPUT_CHANNELS)];
            let header = Box::new(WAVEHDR {
                lpData: samples.as_mut_ptr().cast::<u8>(),
                dwBufferLength: (samples.len() * size_of::<i16>()) as u32,
                dwBytesRecorded: 0,
                dwUser: 0,
                dwFlags: 0,
                dwLoops: 0,
                lpNext: ptr::null_mut(),
                reserved: 0,
            });
            Self {
                samples,
                header,
                queued: false,
                prepared: false,
            }
        }

        fn try_reclaim(&mut self, handle: HWAVEOUT) -> Result<bool, String> {
            if !self.queued {
                return Ok(true);
            }
            let result = unsafe {
                waveOutUnprepareHeader(handle, self.header.as_mut(), size_of::<WAVEHDR>() as u32)
            };
            if result == 0 {
                self.prepared = false;
                self.queued = false;
                Ok(true)
            } else if result == WAVERR_STILLPLAYING {
                Ok(false)
            } else {
                Err(format!(
                    "waveOutUnprepareHeader failed with MMRESULT {result}"
                ))
            }
        }

        fn queue(
            &mut self,
            handle: HWAVEOUT,
            mixer: &mut Mixer,
            model: &mut AudioModel,
            now: Duration,
        ) -> Result<(), String> {
            debug_assert!(!self.queued);
            debug_assert!(!self.prepared);
            mixer.render(&mut self.samples, model, now);
            let prepare = unsafe {
                waveOutPrepareHeader(handle, self.header.as_mut(), size_of::<WAVEHDR>() as u32)
            };
            if prepare != 0 {
                return Err(format!(
                    "waveOutPrepareHeader failed with MMRESULT {prepare}"
                ));
            }
            self.prepared = true;
            let write = unsafe {
                waveOutWrite(handle, self.header.as_mut(), size_of::<WAVEHDR>() as u32)
            };
            if write != 0 {
                let _ = unsafe {
                    waveOutUnprepareHeader(
                        handle,
                        self.header.as_mut(),
                        size_of::<WAVEHDR>() as u32,
                    )
                };
                self.prepared = false;
                return Err(format!("waveOutWrite failed with MMRESULT {write}"));
            }
            self.queued = true;
            Ok(())
        }

        fn unprepare_after_reset(&mut self, handle: HWAVEOUT) {
            if self.prepared {
                let _ = unsafe {
                    waveOutUnprepareHeader(handle, self.header.as_mut(), size_of::<WAVEHDR>() as u32)
                };
                self.prepared = false;
            }
            self.queued = false;
        }
    }

    struct WaveOutput {
        handle: HWAVEOUT,
        buffers: Vec<OutputBuffer>,
        closed: bool,
    }

    impl WaveOutput {
        fn open() -> Result<Self, String> {
            let format = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: OUTPUT_CHANNELS,
                nSamplesPerSec: OUTPUT_SAMPLE_RATE,
                nAvgBytesPerSec: OUTPUT_SAMPLE_RATE
                    * u32::from(OUTPUT_CHANNELS)
                    * u32::from(OUTPUT_BITS / 8),
                nBlockAlign: OUTPUT_CHANNELS * (OUTPUT_BITS / 8),
                wBitsPerSample: OUTPUT_BITS,
                cbSize: 0,
            };
            let mut handle: HWAVEOUT = ptr::null_mut();
            let result = unsafe { waveOutOpen(&mut handle, u32::MAX, &format, 0, 0, 0) };
            if result != 0 {
                return Err(format!("waveOutOpen failed with MMRESULT {result}"));
            }
            let buffers = (0..BUFFER_COUNT).map(|_| OutputBuffer::new()).collect();
            Ok(Self {
                handle,
                buffers,
                closed: false,
            })
        }

        fn service(
            &mut self,
            mixer: &mut Mixer,
            model: &mut AudioModel,
            now: Duration,
        ) -> Result<(), String> {
            let handle = self.handle;
            for buffer in &mut self.buffers {
                let _ = buffer.try_reclaim(handle)?;
            }
            let mut queued = self.buffers.iter().filter(|buffer| buffer.queued).count();
            while queued < TARGET_QUEUED_BUFFERS {
                let Some(buffer) = self
                    .buffers
                    .iter_mut()
                    .find(|buffer| !buffer.queued && !buffer.prepared)
                else {
                    break;
                };
                buffer.queue(handle, mixer, model, now)?;
                queued += 1;
            }
            Ok(())
        }

        fn close(&mut self) {
            if self.closed || self.handle.is_null() {
                return;
            }
            let handle = self.handle;
            unsafe {
                let _ = waveOutReset(handle);
            }
            for buffer in &mut self.buffers {
                buffer.unprepare_after_reset(handle);
            }
            unsafe {
                let _ = waveOutClose(handle);
            }
            self.handle = ptr::null_mut();
            self.closed = true;
        }
    }

    impl Drop for WaveOutput {
        fn drop(&mut self) {
            self.close();
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

    fn make_pcm_wave(channels: u16, sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = samples.len() * 2;
        let riff_len = 4 + (8 + 16) + (8 + data_len);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(riff_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        let block_align = channels * 2;
        bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parses_pcm_wave_without_touching_an_audio_device() {
        let wave = make_pcm_wave(2, 48_000, &[100, -100, 200, -200]);
        let clip = parse_pcm_wave(&wave).expect("valid synthetic WAV");
        assert_eq!(clip.sample_rate, 48_000);
        assert_eq!(clip.channels, 2);
        assert_eq!(clip.frame_count(), 2);
        let (left, right) = clip.stereo_frame(0.0).expect("first frame");
        assert!(left > 0.0);
        assert!(right < 0.0);
    }

    #[test]
    fn rejects_non_pcm_and_invalid_channel_layouts() {
        let mut wave = make_pcm_wave(2, 48_000, &[0, 0]);
        wave[20..22].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            parse_pcm_wave(&wave).expect_err("float WAV must be rejected"),
            WavError::UnsupportedEncoding(3)
        );

        let wave = make_pcm_wave(3, 48_000, &[0, 0, 0]);
        assert_eq!(
            parse_pcm_wave(&wave).expect_err("three-channel WAV must be rejected"),
            WavError::UnsupportedChannels(3)
        );
    }

    #[test]
    fn mono_frames_are_mirrored_to_stereo() {
        let clip = parse_pcm_wave(&make_pcm_wave(1, 44_100, &[1234])).expect("mono WAV");
        let (left, right) = clip.stereo_frame(0.0).expect("first frame");
        assert_eq!(left, right);
    }

    #[test]
    fn priority_mix_preserves_a_standalone_effect_and_never_overflows() {
        assert!((priority_mix(0.0, 0.75) - 0.75).abs() < 0.0001);
        for (background, effect) in [(0.9, 0.8), (-0.9, -0.8), (0.8, -0.6), (-0.8, 0.6)] {
            let mixed = priority_mix(background, effect);
            assert!((-1.0..=1.0).contains(&mixed));
        }
    }

    #[test]
    fn availability_represents_device_failure_without_making_it_fatal() {
        assert_eq!(
            AudioAvailability::unavailable("no output device"),
            AudioAvailability::Unavailable("no output device".to_owned())
        );
    }

    #[test]
    fn legacy_sound_names_are_mapped_for_both_lid_families() {
        assert_eq!(lid_prefix(LidState::Up), "up");
        assert_eq!(lid_prefix(LidState::Down), "down");
        assert_eq!(continuous_key(ContinuousSound::PrintChars), "print-chars");
        assert_eq!(continuous_key(ContinuousSound::PrintSpaces), "print-spaces");
        assert_eq!(continuous_key(ContinuousSound::Hum), "hum");
        assert_eq!(effect_key(EffectSound::CarriageReturn), "cr");
        assert_eq!(effect_key(EffectSound::MotorOn), "motor-on");
        assert_eq!(effect_key(EffectSound::MotorOff), "motor-off");
    }
}
