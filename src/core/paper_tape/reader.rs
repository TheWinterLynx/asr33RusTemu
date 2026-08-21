//! Deterministic paper-tape reader state machine.

use super::PaperTape;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderOptions {
    pub skip_leading_nulls: bool,
    pub set_msb: bool,
    pub auto_stop: bool,
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
