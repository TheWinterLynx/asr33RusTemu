//! Deterministic, bounded paper-tape reader scheduling.

use std::time::Duration;

use crate::core::config::InputReturnMode;
use crate::core::paper_tape::SeekError;
use crate::core::paper_tape::{PaperTape, ReaderState, ReaderStep, TapeReader};

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
    normalization_epoch: u64,
}

impl ReaderEmission {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.tx_bytes[..usize::from(self.len)]
    }
}

#[must_use]
pub fn normalize_reader_emission(
    emitted_byte: u8,
    source_offset: usize,
    next_physical_byte: Option<u8>,
    last_confirmed_source: Option<ConfirmedReaderByte>,
    return_mode: InputReturnMode,
) -> ([u8; 2], u8) {
    if matches!(return_mode, InputReturnMode::CrLf) {
        let control_msb = emitted_byte & 0x80;
        if emitted_byte & 0x7f == b'\r' {
            if matches!(next_physical_byte, Some(byte) if byte & 0x7f == b'\n') {
                return ([emitted_byte, 0], 1);
            }
            return ([emitted_byte, control_msb | b'\n'], 2);
        }
        if emitted_byte & 0x7f == b'\n' {
            let follows_confirmed_cr = matches!(
                last_confirmed_source,
                Some(previous)
                    if previous.source_offset.checked_add(1) == Some(source_offset)
                        && previous.byte & 0x7f == b'\r'
            );
            if follows_confirmed_cr {
                return ([emitted_byte, 0], 1);
            }
            return ([control_msb | b'\r', emitted_byte], 2);
        }
    }
    ([emitted_byte, 0], 1)
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
    last_confirmed_source: Option<ConfirmedReaderByte>,
    normalization_epoch: u64,
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
            last_confirmed_source: None,
            normalization_epoch: 0,
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
    pub fn load(&mut self, tape: PaperTape) {
        self.clear_pending();
        self.reader.load(tape);
    }
    pub fn unload(&mut self) -> Option<PaperTape> {
        self.clear_pending();
        self.reader.unload()
    }
    pub fn rewind(&mut self) -> bool {
        let rewound = self.reader.rewind();
        if rewound {
            self.clear_pending();
        }
        rewound
    }
    pub fn set_return_mode(&mut self, return_mode: InputReturnMode) {
        if self.return_mode != return_mode {
            self.return_mode = return_mode;
            self.last_confirmed_source = None;
            self.normalization_epoch = self.normalization_epoch.wrapping_add(1);
        }
    }
    #[must_use]
    pub fn pending_count(&self) -> usize {
        usize::from(self.pending.is_some() || self.in_flight.is_some())
    }
    pub fn clear_pending(&mut self) {
        self.pending = None;
        self.checkpoint = None;
        self.in_flight = None;
        self.last_confirmed_source = None;
        self.normalization_epoch = self.normalization_epoch.wrapping_add(1);
    }
    #[must_use]
    pub fn awaiting_confirmation(&self) -> bool {
        self.in_flight.is_some()
    }
    /// Commit the current reader position after its byte reaches the runtime's
    /// transport/terminal boundary.
    pub fn confirm_transmitted(&mut self) -> Option<ConfirmedReaderByte> {
        let confirmed = self.in_flight.take().map(|emission| {
            let confirmed = ConfirmedReaderByte {
                byte: emission.source_byte,
                source_offset: emission.source_offset,
            };
            if emission.normalization_epoch == self.normalization_epoch {
                self.last_confirmed_source = Some(confirmed);
            }
            confirmed
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
        self.last_confirmed_source = None;
        self.normalization_epoch = self.normalization_epoch.wrapping_add(1);
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
                        let (tx_bytes, len) = normalize_reader_emission(
                            byte,
                            checkpoint,
                            next_byte,
                            self.last_confirmed_source,
                            self.return_mode,
                        );
                        ReaderEmission {
                            source_byte: byte,
                            source_offset: checkpoint,
                            tx_bytes,
                            len,
                            normalization_epoch: self.normalization_epoch,
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
    use super::{
        ConfirmedReaderByte, FeedResult, READER_FEED_INTERVAL, ReaderFeed,
        normalize_reader_emission,
    };
    use crate::core::config::InputReturnMode;
    use crate::core::paper_tape::{PaperTape, ReaderOptions, ReaderState, TapeReader};
    use std::time::Duration;

    fn collect_feed(data: &[u8], mode: InputReturnMode, seek: Option<usize>) -> Vec<u8> {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            set_msb: false,
        });
        reader.load(PaperTape::new(data.to_vec()));
        if let Some(position) = seek {
            reader.seek(position).expect("test seek is valid");
        }
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO, mode);
        let mut output = Vec::new();
        let mut now = Duration::ZERO;
        while feed.reader().state() == ReaderState::Running || feed.pending_count() != 0 {
            feed.tick(now, |emission| {
                assert!(emission.as_slice().len() <= 2);
                output.extend_from_slice(emission.as_slice());
                FeedResult::Accepted
            });
            feed.confirm_transmitted();
            assert!(feed.pending_count() <= 1);
            now += READER_FEED_INTERVAL;
        }
        output
    }

    #[test]
    fn reader_feed_normalizes_all_physical_line_ending_conventions() {
        for raw in [b"A\nB".as_slice(), b"A\rB".as_slice(), b"A\r\nB".as_slice()] {
            assert_eq!(collect_feed(raw, InputReturnMode::Cr, None), raw);
        }
        assert_eq!(
            collect_feed(b"ONE\nTWO\nTHREE\n", InputReturnMode::CrLf, None),
            b"ONE\r\nTWO\r\nTHREE\r\n"
        );
        assert_eq!(
            collect_feed(b"ONE\rTWO\rTHREE\r", InputReturnMode::CrLf, None),
            b"ONE\r\nTWO\r\nTHREE\r\n"
        );
        assert_eq!(
            collect_feed(b"ONE\r\nTWO\r\n", InputReturnMode::CrLf, None),
            b"ONE\r\nTWO\r\n"
        );
        assert_eq!(
            collect_feed(b"A\rB\nC\r\nD", InputReturnMode::CrLf, None),
            b"A\r\nB\r\nC\r\nD"
        );
    }

    #[test]
    fn seek_to_physical_lf_breaks_confirmed_cr_adjacency() {
        assert_eq!(
            collect_feed(b"A\r\nB", InputReturnMode::CrLf, Some(2)),
            b"\r\nB"
        );
    }

    #[test]
    fn seek_and_mode_change_clear_confirmed_adjacency_context() {
        for change_mode in [false, true] {
            let mut reader = TapeReader::new(ReaderOptions {
                skip_leading_nulls: false,
                auto_stop: false,
                set_msb: false,
            });
            reader.load(PaperTape::new(b"\r\n".to_vec()));
            assert!(reader.start());
            let initial_mode = if change_mode {
                InputReturnMode::Cr
            } else {
                InputReturnMode::CrLf
            };
            let mut feed = ReaderFeed::new(reader, Duration::ZERO, initial_mode);
            feed.tick(Duration::ZERO, |_| FeedResult::Accepted);
            feed.confirm_transmitted();

            if change_mode {
                feed.set_return_mode(InputReturnMode::CrLf);
            } else {
                feed.reader_mut().stop();
                feed.seek(1).expect("manual seek to LF");
                assert!(feed.reader_mut().start());
            }

            let mut emitted = Vec::new();
            feed.tick(READER_FEED_INTERVAL, |emission| {
                emitted.extend_from_slice(emission.as_slice());
                FeedResult::Accepted
            });
            assert_eq!(emitted, b"\r\n");
        }
    }

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
        assert!(feed.rewind());
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
    fn normalization_helper_covers_raw_cr_lf_crlf_adjacency_and_msb() {
        for byte in [b'A', b'\r', b'\n', 0x8d, 0x8a] {
            assert_eq!(
                normalize_reader_emission(byte, 4, None, None, InputReturnMode::Cr),
                ([byte, 0], 1),
                "RAW preserves {byte:#04x}"
            );
        }
        assert_eq!(
            normalize_reader_emission(b'\r', 4, Some(b'A'), None, InputReturnMode::CrLf),
            (*b"\r\n", 2)
        );
        assert_eq!(
            normalize_reader_emission(0x8d, 4, Some(b'A'), None, InputReturnMode::CrLf),
            ([0x8d, 0x8a], 2)
        );
        assert_eq!(
            normalize_reader_emission(b'\r', 4, Some(b'\n'), None, InputReturnMode::CrLf),
            ([b'\r', 0], 1)
        );
        assert_eq!(
            normalize_reader_emission(b'\n', 4, None, None, InputReturnMode::CrLf),
            (*b"\r\n", 2)
        );
        assert_eq!(
            normalize_reader_emission(0x8a, 4, None, None, InputReturnMode::CrLf),
            ([0x8d, 0x8a], 2)
        );
        let confirmed_cr = Some(ConfirmedReaderByte {
            byte: b'\r',
            source_offset: 3,
        });
        assert_eq!(
            normalize_reader_emission(b'\n', 4, None, confirmed_cr, InputReturnMode::CrLf),
            ([b'\n', 0], 1)
        );
        assert_eq!(
            normalize_reader_emission(b'\n', 5, None, confirmed_cr, InputReturnMode::CrLf),
            (*b"\r\n", 2),
            "non-adjacent confirmed CR cannot suppress normalization"
        );
        assert_eq!(
            normalize_reader_emission(b'\n', 4, None, None, InputReturnMode::CrLf),
            (*b"\r\n", 2),
            "seek-to-LF has no confirmed predecessor"
        );
        assert_eq!(
            normalize_reader_emission(b'\r', 4, Some(0x8a), None, InputReturnMode::CrLf),
            ([b'\r', 0], 1)
        );
    }

    #[test]
    fn standalone_lf_retries_and_rolls_back_as_one_bounded_emission() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: false,
            auto_stop: false,
            set_msb: true,
        });
        reader.load(PaperTape::new(b"\nA".to_vec()));
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
        let mut after_rollback = Vec::new();
        feed.tick(Duration::from_millis(4), |emission| {
            after_rollback.extend_from_slice(emission.as_slice());
            FeedResult::Accepted
        });
        assert_eq!(after_rollback, [0x8d, 0x8a]);
        assert_eq!(
            feed.confirm_transmitted().map(|value| value.source_offset),
            Some(0)
        );
    }
}
