//! Deterministic, bounded paper-tape reader scheduling.

use std::time::Duration;

use crate::core::config::InputReturnMode;
use crate::core::paper_tape::SeekError;
use crate::core::paper_tape::{ReaderState, ReaderStep, TapeReader};

pub const READER_FEED_INTERVAL: Duration = Duration::from_millis(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedResult {
    Accepted,
    Backpressured(ReaderEmission),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderEmission {
    pub source_byte: u8,
    pub source_offset: usize,
    tx_bytes: [u8; 2],
    len: u8,
}

impl ReaderEmission {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.tx_bytes[..usize::from(self.len)]
    }
}

#[must_use]
pub const fn reader_tx_bytes(
    emitted_byte: u8,
    next_physical_byte: Option<u8>,
    return_mode: InputReturnMode,
) -> ([u8; 2], u8) {
    let should_add_lf = matches!(return_mode, InputReturnMode::CrLf)
        && emitted_byte & 0x7f == b'\r'
        && !matches!(next_physical_byte, Some(byte) if byte & 0x7f == b'\n');
    if should_add_lf {
        ([emitted_byte, emitted_byte & 0x80 | b'\n'], 2)
    } else {
        ([emitted_byte, 0], 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmedReaderByte {
    pub byte: u8,
    pub source_offset: usize,
}

#[derive(Debug)]
pub struct ReaderFeed {
    reader: TapeReader,
    return_mode: InputReturnMode,
    pending: Option<ReaderEmission>,
    checkpoint: Option<usize>,
    in_flight: Option<ReaderEmission>,
    next_due: Duration,
}

impl ReaderFeed {
    #[must_use]
    pub fn new(reader: TapeReader, now: Duration, return_mode: InputReturnMode) -> Self {
        Self {
            reader,
            return_mode,
            pending: None,
            checkpoint: None,
            in_flight: None,
            next_due: now,
        }
    }
    #[must_use]
    pub fn reader(&self) -> &TapeReader {
        &self.reader
    }
    pub fn reader_mut(&mut self) -> &mut TapeReader {
        &mut self.reader
    }
    pub fn set_return_mode(&mut self, return_mode: InputReturnMode) {
        self.return_mode = return_mode;
    }
    #[must_use]
    pub fn pending_count(&self) -> usize {
        usize::from(self.pending.is_some() || self.in_flight.is_some())
    }
    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.checkpoint = None;
        self.in_flight = None;
    }
    #[must_use]
    pub fn awaiting_confirmation(&self) -> bool {
        self.in_flight.is_some()
    }
    /// Commit the current reader position after its byte reaches the runtime's
    /// transport/terminal boundary.
    pub fn confirm_transmitted(&mut self) -> Option<ConfirmedReaderByte> {
        let confirmed = self.in_flight.take().map(|emission| ConfirmedReaderByte {
            byte: emission.source_byte,
            source_offset: emission.source_offset,
        });
        self.checkpoint = None;
        confirmed
    }
    /// Restore the position captured before the single unconfirmed byte.
    pub fn rollback_unconfirmed(&mut self) {
        if let Some(position) = self.checkpoint.take() {
            self.reader.restore_position(position);
        }
        self.pending = None;
        self.in_flight = None;
    }

    /// Roll back the one in-flight byte and stop before changing its route.
    pub fn pause_for_mode_change(&mut self) -> bool {
        if self.reader.state() == ReaderState::Running || self.pending_count() != 0 {
            self.rollback_unconfirmed();
            self.reader.stop();
            true
        } else {
            false
        }
    }

    pub fn seek(&mut self, position: usize) -> Result<(), SeekError> {
        if self.reader.state() == ReaderState::Running {
            return Err(SeekError::Running);
        }
        self.rollback_unconfirmed();
        self.reader.seek(position)
    }

    pub fn tick<F>(&mut self, now: Duration, mut transmit: F) -> Option<Duration>
    where
        F: FnMut(ReaderEmission) -> FeedResult,
    {
        if self.reader.state() != ReaderState::Running {
            return None;
        }
        if self.in_flight.is_some() {
            return None;
        }
        if now < self.next_due {
            return Some(self.next_due - now);
        }
        let emission = match self.pending.take() {
            Some(emission) => emission,
            None => {
                let checkpoint = self.reader.position();
                match self.reader.step() {
                    ReaderStep::Byte(byte) => {
                        self.checkpoint = Some(checkpoint);
                        let next_byte = self
                            .reader
                            .tape()
                            .and_then(|tape| tape.bytes().get(self.reader.position()))
                            .copied();
                        let (tx_bytes, len) = reader_tx_bytes(byte, next_byte, self.return_mode);
                        ReaderEmission {
                            source_byte: byte,
                            source_offset: checkpoint,
                            tx_bytes,
                            len,
                        }
                    }
                    ReaderStep::Idle | ReaderStep::Stopped(_) => return None,
                }
            }
        };
        match transmit(emission) {
            FeedResult::Accepted => {
                self.in_flight = Some(emission);
                self.next_due = now + READER_FEED_INTERVAL;
            }
            FeedResult::Backpressured(emission) => {
                self.pending = Some(emission);
                return Some(Duration::from_millis(1));
            }
        }
        Some(READER_FEED_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::{FeedResult, READER_FEED_INTERVAL, ReaderFeed, reader_tx_bytes};
    use crate::core::config::InputReturnMode;
    use crate::core::paper_tape::{PaperTape, ReaderOptions, ReaderState, TapeReader};
    use std::time::Duration;

    #[test]
    fn pacing_and_backpressure_are_lossless_with_one_pending_byte() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            ..ReaderOptions::default()
        });
        reader.load(PaperTape::new((0_u8..32).collect()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, InputReturnMode::Cr);
        let mut accepted = Vec::new();
        let mut reject = true;
        let mut now = Duration::ZERO;
        while feed.reader().state() == ReaderState::Running || feed.pending_count() > 0 {
            let delay = feed.tick(now, |emission| {
                if reject {
                    reject = false;
                    FeedResult::Backpressured(emission)
                } else {
                    reject = true;
                    accepted.extend_from_slice(emission.as_slice());
                    FeedResult::Accepted
                }
            });
            if feed.awaiting_confirmation() {
                feed.confirm_transmitted();
            }
            assert!(feed.pending_count() <= 1);
            now += delay.unwrap_or(READER_FEED_INTERVAL);
        }
        assert_eq!(accepted, (0_u8..32).collect::<Vec<_>>());
    }

    #[test]
    fn stopped_reader_never_emits_pending_or_later_bytes() {
        let mut reader = TapeReader::new(ReaderOptions::default());
        reader.load(PaperTape::new(b"A\x80\x80".to_vec()));
        reader.start();
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, InputReturnMode::Cr);
        let mut emitted = Vec::new();
        feed.tick(Duration::ZERO, |emission| {
            emitted.extend_from_slice(emission.as_slice());
            FeedResult::Accepted
        });
        feed.confirm_transmitted();
        feed.tick(Duration::from_millis(3), |emission| {
            emitted.extend_from_slice(emission.as_slice());
            FeedResult::Accepted
        });
        feed.confirm_transmitted();
        feed.tick(Duration::from_millis(6), |emission| {
            emitted.extend_from_slice(emission.as_slice());
            FeedResult::Accepted
        });
        assert_eq!(emitted, b"A\x80");
        assert_eq!(feed.reader().state(), ReaderState::Stopped);
    }

    #[test]
    fn disconnected_pause_retains_exactly_one_pending_byte_without_busy_loop() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            set_msb: false,
        });
        reader.load(PaperTape::new(b"ABC".to_vec()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, InputReturnMode::Cr);
        assert_eq!(
            feed.tick(Duration::ZERO, FeedResult::Backpressured),
            Some(Duration::from_millis(1))
        );
        feed.reader_mut().stop();
        assert_eq!(
            feed.tick(Duration::from_millis(1), |_| FeedResult::Accepted),
            None
        );
        assert_eq!(feed.pending_count(), 1);
        assert_eq!(feed.reader().position(), 1);
        feed.rollback_unconfirmed();
        assert_eq!(feed.pending_count(), 0);
        assert_eq!(feed.reader().position(), 0);
    }

    #[test]
    fn confirmation_returns_emitted_byte_and_rollback_returns_nothing() {
        let mut reader = TapeReader::new(ReaderOptions {
            set_msb: true,
            ..ReaderOptions::default()
        });
        reader.load(PaperTape::new(b"A".to_vec()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, InputReturnMode::Cr);
        feed.tick(Duration::ZERO, |_| FeedResult::Accepted);
        assert_eq!(
            feed.confirm_transmitted(),
            Some(super::ConfirmedReaderByte {
                byte: 0xc1,
                source_offset: 0
            })
        );

        feed.reader_mut().stop();
        assert!(feed.reader_mut().rewind());
        assert!(feed.reader_mut().start());
        feed.tick(READER_FEED_INTERVAL, |_| FeedResult::Accepted);
        feed.rollback_unconfirmed();
        assert_eq!(feed.confirm_transmitted(), None);
        assert_eq!(feed.reader().position(), 0);
    }

    #[test]
    fn seek_requires_stopped_reader_and_rolls_back_pending_byte() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            set_msb: false,
        });
        reader.load(PaperTape::new(b"ABCDEF".to_vec()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, InputReturnMode::Cr);
        feed.tick(Duration::ZERO, FeedResult::Backpressured);
        assert_eq!(feed.pending_count(), 1);
        assert!(feed.seek(3).is_err());
        assert_eq!(feed.reader().position(), 1);
        feed.reader_mut().stop();
        feed.seek(3).expect("stopped feed can seek safely");
        assert_eq!(feed.pending_count(), 0);
        assert_eq!(feed.reader().position(), 3);
        assert!(feed.reader_mut().start());
        let mut emitted = Vec::new();
        let mut now = READER_FEED_INTERVAL;
        while feed.reader().state() == ReaderState::Running || feed.pending_count() != 0 {
            feed.tick(now, |emission| {
                emitted.extend_from_slice(emission.as_slice());
                FeedResult::Accepted
            });
            feed.confirm_transmitted();
            now += READER_FEED_INTERVAL;
        }
        assert_eq!(emitted, b"DEF");
    }

    #[test]
    fn return_expansion_is_bounded_preserves_msb_and_skips_existing_lf() {
        assert_eq!(
            reader_tx_bytes(b'\r', Some(b'A'), InputReturnMode::CrLf),
            (*b"\r\n", 2)
        );
        assert_eq!(
            reader_tx_bytes(0x8d, Some(b'A'), InputReturnMode::CrLf),
            ([0x8d, 0x8a], 2)
        );
        assert_eq!(
            reader_tx_bytes(b'\r', Some(b'\n'), InputReturnMode::CrLf),
            ([b'\r', 0], 1)
        );
        assert_eq!(
            reader_tx_bytes(b'\r', Some(0x8a), InputReturnMode::CrLf),
            ([b'\r', 0], 1)
        );
        assert_eq!(
            reader_tx_bytes(b'\r', None, InputReturnMode::Cr),
            ([b'\r', 0], 1)
        );
    }

    #[test]
    fn crlf_emission_rolls_back_as_one_physical_byte_under_backpressure() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            set_msb: true,
        });
        reader.load(PaperTape::new(b"\rA".to_vec()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, InputReturnMode::CrLf);
        let mut first = None;
        feed.tick(Duration::ZERO, |emission| {
            first = Some(emission);
            FeedResult::Backpressured(emission)
        });
        assert_eq!(first.expect("one emission").as_slice(), [0x8d, 0x8a]);
        assert_eq!(feed.pending_count(), 1);
        assert_eq!(feed.reader().position(), 1);
        let mut retried = Vec::new();
        feed.tick(Duration::from_millis(1), |emission| {
            retried.extend_from_slice(emission.as_slice());
            FeedResult::Accepted
        });
        assert_eq!(retried, [0x8d, 0x8a]);
        assert!(feed.awaiting_confirmation());
        feed.rollback_unconfirmed();
        assert_eq!(feed.reader().position(), 0);
        assert_eq!(feed.pending_count(), 0);
    }
}
