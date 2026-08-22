//! Deterministic paper-tape reader state machine.

use super::PaperTape;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderOptions {
    pub skip_leading_nulls: bool,
    pub set_msb: bool,
    pub auto_stop: bool,
}

#[cfg(test)]
mod seek_tests {
    use super::*;

    #[test]
    fn loaded_tape_is_inspectable_and_seek_does_not_emit() {
        let bytes = b"ABCDEF".to_vec();
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            set_msb: false,
            auto_stop: false,
        });
        reader.load(PaperTape::new(bytes.clone()));
        assert_eq!(reader.position(), 0);
        assert_eq!(reader.tape().map(PaperTape::bytes), Some(bytes.as_slice()));
        reader.seek(5).expect("valid forward seek");
        reader.seek(2).expect("valid backward seek");
        reader.seek(4).expect("valid forward seek");
        assert_eq!(reader.position(), 4);
        assert_eq!(reader.tape().map(PaperTape::bytes), Some(bytes.as_slice()));
    }

    #[test]
    fn seek_from_middle_starts_at_requested_byte_and_running_seek_is_rejected() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            set_msb: false,
            auto_stop: false,
        });
        reader.load(PaperTape::new(b"ABCDEF".to_vec()));
        reader.seek(3).expect("valid seek");
        assert!(reader.start());
        assert_eq!(reader.seek(0), Err(SeekError::Running));
        let mut emitted = Vec::new();
        while let ReaderStep::Byte(byte) = reader.step() {
            emitted.push(byte);
        }
        assert_eq!(emitted, b"DEF");
    }

    #[test]
    fn seek_validates_tape_and_range_and_rewind_keeps_data() {
        let mut reader = TapeReader::new(ReaderOptions::default());
        assert_eq!(reader.seek(0), Err(SeekError::NoTape));
        reader.load(PaperTape::new(b"ABC".to_vec()));
        assert_eq!(
            reader.seek(4),
            Err(SeekError::OutOfRange {
                requested: 4,
                length: 3
            })
        );
        reader.seek(3).expect("end position is valid");
        assert!(reader.rewind());
        assert_eq!(reader.position(), 0);
        assert_eq!(reader.tape().map(PaperTape::bytes), Some(&b"ABC"[..]));
    }

    #[test]
    fn leading_nulls_remain_visible_until_start_and_msb_changes_only_emission() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: true,
            set_msb: true,
            auto_stop: false,
        });
        reader.load(PaperTape::new(b"\0\0A".to_vec()));
        assert_eq!(reader.position(), 0);
        assert_eq!(reader.tape().map(PaperTape::bytes), Some(&b"\0\0A"[..]));
        assert!(reader.start());
        assert_eq!(reader.position(), 2);
        assert_eq!(reader.step(), ReaderStep::Byte(0xc1));
        assert_eq!(reader.tape().map(PaperTape::bytes), Some(&b"\0\0A"[..]));
    }

    #[test]
    fn start_from_middle_preserves_existing_trailer_autostop_quirk() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            set_msb: false,
            auto_stop: true,
        });
        reader.load(PaperTape::new(b"AB\x80\x80\0\0".to_vec()));
        reader.seek(2).expect("seek to first trailer byte");
        assert!(reader.start());
        assert_eq!(reader.step(), ReaderStep::Byte(0x80));
        assert_eq!(
            reader.step(),
            ReaderStep::Stopped(StopCause::TrailingOctal200)
        );
        assert_eq!(reader.position(), 3);
    }
}

impl Default for ReaderOptions {
    fn default() -> Self {
        Self {
            skip_leading_nulls: true,
            set_msb: false,
            auto_stop: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReaderState {
    #[default]
    Unloaded,
    Stopped,
    Running,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopCause {
    EndOfTape,
    TrailingOctal200,
    TrailingNull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderStep {
    Idle,
    Byte(u8),
    Stopped(StopCause),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeekError {
    NoTape,
    Running,
    OutOfRange { requested: usize, length: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TapeReader {
    tape: Option<PaperTape>,
    position: usize,
    state: ReaderState,
    stop_cause: Option<StopCause>,
    options: ReaderOptions,
}

impl TapeReader {
    #[must_use]
    pub fn new(options: ReaderOptions) -> Self {
        Self {
            tape: None,
            position: 0,
            state: ReaderState::Unloaded,
            stop_cause: None,
            options,
        }
    }

    pub fn load(&mut self, tape: PaperTape) {
        self.tape = Some(tape);
        self.position = 0;
        self.state = ReaderState::Stopped;
        // legacy behavior: loading does not clear a previous stop cause. It is
        // cleared when the reader is turned on.
    }

    pub fn unload(&mut self) -> Option<PaperTape> {
        self.position = 0;
        self.state = ReaderState::Unloaded;
        self.tape.take()
    }

    #[must_use]
    pub fn state(&self) -> ReaderState {
        self.state
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    #[must_use]
    pub fn stop_cause(&self) -> Option<StopCause> {
        self.stop_cause
    }

    #[must_use]
    pub fn tape(&self) -> Option<&PaperTape> {
        self.tape.as_ref()
    }

    pub fn set_auto_stop(&mut self, enabled: bool) {
        self.options.auto_stop = enabled;
    }

    pub fn set_skip_leading_nulls(&mut self, enabled: bool) {
        self.options.skip_leading_nulls = enabled;
    }

    pub fn set_msb(&mut self, enabled: bool) {
        self.options.set_msb = enabled;
    }

    pub fn start(&mut self) -> bool {
        let Some(tape) = self.tape.as_ref() else {
            return false;
        };

        if self.options.skip_leading_nulls {
            while self.position < tape.len() && tape.bytes()[self.position] == 0o000 {
                self.position += 1;
            }
        }
        self.state = ReaderState::Running;
        self.stop_cause = None;
        true
    }

    pub fn stop(&mut self) {
        if self.tape.is_some() {
            self.state = ReaderState::Stopped;
        }
    }

    pub fn rewind(&mut self) -> bool {
        if self.tape.is_some() && self.state != ReaderState::Running && self.position > 0 {
            self.position = 0;
            true
        } else {
            false
        }
    }

    pub fn seek(&mut self, position: usize) -> Result<(), SeekError> {
        let Some(tape) = self.tape.as_ref() else {
            return Err(SeekError::NoTape);
        };
        if self.state == ReaderState::Running {
            return Err(SeekError::Running);
        }
        if position > tape.len() {
            return Err(SeekError::OutOfRange {
                requested: position,
                length: tape.len(),
            });
        }
        self.position = position;
        self.stop_cause = None;
        Ok(())
    }

    pub(crate) fn restore_position(&mut self, position: usize) {
        if self
            .tape
            .as_ref()
            .is_some_and(|tape| position <= tape.len())
        {
            self.position = position;
            self.stop_cause = None;
        }
    }

    /// Advance the reader by one worker iteration without sleeping or I/O.
    #[must_use]
    pub fn step(&mut self) -> ReaderStep {
        if self.state != ReaderState::Running {
            return ReaderStep::Idle;
        }
        let Some(tape) = self.tape.as_ref() else {
            self.state = ReaderState::Unloaded;
            return ReaderStep::Idle;
        };

        if self.position >= tape.len() {
            return self.stop_for(StopCause::EndOfTape);
        }

        if self.options.auto_stop {
            let trailers = tape.trailers();
            if let Some(start) = trailers.octal_200 {
                // legacy behavior: the strict comparison emits the first 0200
                // trailer byte and stops on the following iteration.
                if self.position > start {
                    return self.stop_for(StopCause::TrailingOctal200);
                }
            } else if let Some(start) = trailers.null
                && self.position > start
            {
                return self.stop_for(StopCause::TrailingNull);
            }
        }

        let mut byte = tape.bytes()[self.position];
        if self.options.set_msb {
            byte |= 0x80;
        }
        self.position += 1;
        ReaderStep::Byte(byte)
    }

    fn stop_for(&mut self, cause: StopCause) -> ReaderStep {
        self.state = ReaderState::Stopped;
        self.stop_cause = Some(cause);
        ReaderStep::Stopped(cause)
    }
}
