//! Deterministic, bounded paper-tape reader scheduling.

use std::time::Duration;

use crate::core::paper_tape::{ReaderState, ReaderStep, TapeReader};

pub const READER_FEED_INTERVAL: Duration = Duration::from_millis(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedResult {
    Accepted,
    Backpressured(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmedReaderByte {
    pub byte: u8,
    pub source_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InFlightReaderByte {
    byte: u8,
    source_offset: usize,
}

#[derive(Debug)]
pub struct ReaderFeed {
    reader: TapeReader,
    pending: Option<u8>,
    checkpoint: Option<usize>,
    in_flight: Option<InFlightReaderByte>,
    next_due: Duration,
}

impl ReaderFeed {
    #[must_use]
    pub fn new(reader: TapeReader, now: Duration) -> Self {
        Self {
            reader,
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
        let confirmed = self.in_flight.take().map(|in_flight| ConfirmedReaderByte {
            byte: in_flight.byte,
            source_offset: in_flight.source_offset,
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

    pub fn tick<F>(&mut self, now: Duration, mut transmit: F) -> Option<Duration>
    where
        F: FnMut(u8) -> FeedResult,
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
        let byte = match self.pending.take() {
            Some(byte) => byte,
            None => {
                let checkpoint = self.reader.position();
                match self.reader.step() {
                    ReaderStep::Byte(byte) => {
                        self.checkpoint = Some(checkpoint);
                        byte
                    }
                    ReaderStep::Idle | ReaderStep::Stopped(_) => return None,
                }
            }
        };
        match transmit(byte) {
            FeedResult::Accepted => {
                let source_offset = self
                    .checkpoint
                    .unwrap_or_else(|| self.reader.position().saturating_sub(1));
                self.in_flight = Some(InFlightReaderByte {
                    byte,
                    source_offset,
                });
                self.next_due = now + READER_FEED_INTERVAL;
            }
            FeedResult::Backpressured(byte) => {
                self.pending = Some(byte);
                return Some(Duration::from_millis(1));
            }
        }
        Some(READER_FEED_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::{FeedResult, READER_FEED_INTERVAL, ReaderFeed};
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
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);
        let mut accepted = Vec::new();
        let mut reject = true;
        let mut now = Duration::ZERO;
        while feed.reader().state() == ReaderState::Running || feed.pending_count() > 0 {
            let delay = feed.tick(now, |byte| {
                if reject {
                    reject = false;
                    FeedResult::Backpressured(byte)
                } else {
                    reject = true;
                    accepted.push(byte);
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
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);
        let mut emitted = Vec::new();
        feed.tick(Duration::ZERO, |byte| {
            emitted.push(byte);
            FeedResult::Accepted
        });
        feed.confirm_transmitted();
        feed.tick(Duration::from_millis(3), |byte| {
            emitted.push(byte);
            FeedResult::Accepted
        });
        feed.confirm_transmitted();
        feed.tick(Duration::from_millis(6), |byte| {
            emitted.push(byte);
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
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);
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
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);
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
}
