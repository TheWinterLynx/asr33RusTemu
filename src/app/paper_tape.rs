//! Deterministic, bounded paper-tape reader scheduling.

use std::time::Duration;

use crate::core::paper_tape::{ReaderState, ReaderStep, TapeReader};

pub const READER_FEED_INTERVAL: Duration = Duration::from_millis(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedResult {
    Accepted,
    Backpressured(u8),
}

#[derive(Debug)]
pub struct ReaderFeed {
    reader: TapeReader,
    pending: Option<u8>,
    next_due: Duration,
}

impl ReaderFeed {
    #[must_use]
    pub fn new(reader: TapeReader, now: Duration) -> Self {
        Self {
            reader,
            pending: None,
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
        usize::from(self.pending.is_some())
    }

    pub fn tick<F>(&mut self, now: Duration, mut transmit: F) -> Option<Duration>
    where
        F: FnMut(u8) -> FeedResult,
    {
        if self.reader.state() != ReaderState::Running {
            self.pending = None;
            return None;
        }
        if now < self.next_due {
            return Some(self.next_due - now);
        }
        let byte = match self.pending.take() {
            Some(byte) => byte,
            None => match self.reader.step() {
                ReaderStep::Byte(byte) => byte,
                ReaderStep::Idle | ReaderStep::Stopped(_) => return None,
            },
        };
        match transmit(byte) {
            FeedResult::Accepted => self.next_due = now + READER_FEED_INTERVAL,
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
        feed.tick(Duration::from_millis(3), |byte| {
            emitted.push(byte);
            FeedResult::Accepted
        });
        feed.tick(Duration::from_millis(6), |byte| {
            emitted.push(byte);
            FeedResult::Accepted
        });
        assert_eq!(emitted, b"A\x80");
        assert_eq!(feed.reader().state(), ReaderState::Stopped);
    }
}
