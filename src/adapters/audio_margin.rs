//! ASR-33 right-margin audio policy layered over the proven rodio backend.
//!
//! The deterministic terminal reports the post-character carriage column.
//! RusTair rings the warning bell eight positions before the 72-column edge
//! and plays a carriage-return effect when automatic wrap completes.  Keep
//! those policy details here so the rodio mixer itself remains untouched.

use crate::core::config::LidState;
use crate::core::terminal::CharacterEvent;

pub use super::audio_rodio_crlf::{AudioAvailability, WavError};

const LEGACY_AUDIO_BELL_COLUMN: usize = 62;
const ASR33_MARGIN_BELL_POST_COLUMN: usize = 65;

pub struct AudioEngine {
    inner: super::audio_rodio_crlf::AudioEngine,
}

impl AudioEngine {
    #[must_use]
    pub fn start(lid: LidState, muted: bool) -> Self {
        Self {
            inner: super::audio_rodio_crlf::AudioEngine::start(lid, muted),
        }
    }

    pub fn character(&self, event: CharacterEvent) {
        let printable = (' '..='~').contains(&event.character);

        // The old standalone audio state had a fixed post-character bell at
        // column 62. Suppress only that audio-only trigger; terminal state and
        // transmitted bytes are untouched.
        let mut forwarded = event;
        if printable && forwarded.column == LEGACY_AUDIO_BELL_COLUMN {
            forwarded.column = LEGACY_AUDIO_BELL_COLUMN.saturating_sub(1);
        }
        self.inner.character(forwarded);

        // RusTair checks column == width - 8 before printing. With the
        // standalone's post-character event semantics, a 72-column page
        // therefore rings after the character advances the carriage to 65.
        if printable && event.column == ASR33_MARGIN_BELL_POST_COLUMN {
            self.inner.character(CharacterEvent {
                character: '\u{7}',
                column: 0,
            });
        }

        // A printable character reported at post-column zero can only be the
        // character that triggered terminal autowrap. Give that automatic
        // return the same clean CR one-shot used by explicit carriage return.
        if printable && event.column == 0 {
            self.inner.character(CharacterEvent {
                character: '\r',
                column: 0,
            });
        }
    }

    pub fn keypress(&self) {
        self.inner.keypress();
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.set_muted(muted);
    }

    pub fn set_lid(&self, lid: LidState) {
        self.inner.set_lid(lid);
    }

    pub fn set_tape_reader_running(&self, running: bool) {
        self.inner.set_tape_reader_running(running);
    }

    pub fn refresh_status(&mut self) -> Option<AudioAvailability> {
        self.inner.refresh_status()
    }

    #[must_use]
    pub fn availability(&self) -> &AudioAvailability {
        self.inner.availability()
    }

    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }

    pub fn join(&mut self) {
        self.inner.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asr33_margin_position_matches_72_column_rustair_semantics() {
        assert_eq!(ASR33_MARGIN_BELL_POST_COLUMN, 72 - 8 + 1);
    }

    #[test]
    fn legacy_and_asr33_margin_columns_are_distinct() {
        assert_ne!(LEGACY_AUDIO_BELL_COLUMN, ASR33_MARGIN_BELL_POST_COLUMN);
    }
}
