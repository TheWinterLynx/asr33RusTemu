use asr33emu::core::paper_tape::{
    PaperTape, PunchMode, PunchState, ReaderOptions, ReaderState, ReaderStep, StopCause, TapePunch,
    TapeReader, TrailerPositions,
};

const SHARED_TAPE: &[u8] = include_bytes!("fixtures/paper_tape_leader_data_trailers.bin");

fn shared_reader(options: ReaderOptions) -> TapeReader {
    let mut reader = TapeReader::new(options);
    reader.load(PaperTape::new(SHARED_TAPE.to_vec()));
    reader
}

#[test]
fn shared_fixture_has_expected_binary_layout_and_trailers() {
    assert_eq!(SHARED_TAPE, b"\x00\x00AB\x80\x80\x00\x00");
    let tape = PaperTape::new(SHARED_TAPE.to_vec());
    assert_eq!(
        tape.trailers(),
        TrailerPositions {
            octal_200: Some(4),
            null: Some(6),
        }
    );
}

#[test]
fn reader_matches_python_positions_and_legacy_autostop_step_by_step() {
    let mut reader = shared_reader(ReaderOptions::default());
    assert_eq!(reader.state(), ReaderState::Stopped);
    assert!(reader.start());
    assert_eq!(reader.position(), 2);

    assert_eq!(reader.step(), ReaderStep::Byte(b'A'));
    assert_eq!(
        (reader.position(), reader.state()),
        (3, ReaderState::Running)
    );
    assert_eq!(reader.step(), ReaderStep::Byte(b'B'));
    assert_eq!(
        (reader.position(), reader.state()),
        (4, ReaderState::Running)
    );

    assert_eq!(reader.step(), ReaderStep::Byte(0x80));
    assert_eq!(
        (reader.position(), reader.state()),
        (5, ReaderState::Running)
    );
    assert_eq!(
        reader.step(),
        ReaderStep::Stopped(StopCause::TrailingOctal200)
    );
    assert_eq!(
        (reader.position(), reader.state()),
        (5, ReaderState::Stopped)
    );
    assert_eq!(reader.stop_cause(), Some(StopCause::TrailingOctal200));
}

#[test]
fn reader_sets_msb_on_emitted_data() {
    let mut reader = shared_reader(ReaderOptions {
        set_msb: true,
        ..ReaderOptions::default()
    });
    assert!(reader.start());
    assert_eq!(reader.step(), ReaderStep::Byte(0xc1));
    assert_eq!(reader.step(), ReaderStep::Byte(0xc2));
}

#[test]
fn disabled_autostop_reads_trailers_until_physical_end() {
    let mut reader = shared_reader(ReaderOptions {
        auto_stop: false,
        ..ReaderOptions::default()
    });
    assert!(reader.start());
    let mut emitted = Vec::new();
    loop {
        match reader.step() {
            ReaderStep::Byte(byte) => emitted.push(byte),
            ReaderStep::Stopped(cause) => {
                assert_eq!(cause, StopCause::EndOfTape);
                break;
            }
            ReaderStep::Idle => panic!("running reader became idle"),
        }
    }
    assert_eq!(emitted, b"AB\x80\x80\x00\x00");
    assert_eq!(reader.position(), SHARED_TAPE.len());
}

#[test]
fn null_only_trailer_preserves_strict_comparison_legacy_behavior() {
    let mut reader = TapeReader::new(ReaderOptions {
        skip_leading_nulls: false,
        ..ReaderOptions::default()
    });
    reader.load(PaperTape::new(b"A\x00\x00".to_vec()));
    assert!(reader.start());
    assert_eq!(reader.step(), ReaderStep::Byte(b'A'));
    assert_eq!(reader.step(), ReaderStep::Byte(0));
    assert_eq!(reader.position(), 2);
    assert_eq!(reader.step(), ReaderStep::Stopped(StopCause::TrailingNull));
}

#[test]
fn rewind_only_changes_a_loaded_stopped_reader() {
    let mut reader = shared_reader(ReaderOptions::default());
    assert!(reader.start());
    assert_eq!(reader.step(), ReaderStep::Byte(b'A'));
    assert!(!reader.rewind());
    reader.stop();
    assert!(reader.rewind());
    assert_eq!(reader.position(), 0);
    assert!(!reader.rewind());
}

#[test]
fn unloaded_reader_cannot_start_or_advance() {
    let mut reader = TapeReader::new(ReaderOptions::default());
    assert!(!reader.start());
    assert_eq!(reader.step(), ReaderStep::Idle);
    assert_eq!(reader.state(), ReaderState::Unloaded);
}

#[test]
fn append_punch_preserves_existing_bytes() {
    let mut punch = TapePunch::new(PunchMode::Append);
    punch.load(b"OLD");
    assert_eq!(punch.state(), PunchState::Stopped);
    assert!(punch.start());
    assert!(punch.punch(b"NEW"));
    assert_eq!(punch.bytes(), Some(b"OLDNEW".as_slice()));
    assert_eq!(punch.unload(), Some(b"OLDNEW".to_vec()));
    assert_eq!(punch.state(), PunchState::Unloaded);
}

#[test]
fn overwrite_punch_discards_existing_bytes_when_loaded() {
    let mut punch = TapePunch::new(PunchMode::Overwrite);
    punch.load(b"OLD");
    assert_eq!(punch.bytes(), Some(b"".as_slice()));
    assert!(!punch.punch(b"IGNORED"));
    assert!(punch.start());
    assert!(punch.punch(b"NEW"));
    assert_eq!(punch.unload(), Some(b"NEW".to_vec()));
}

#[test]
fn changing_punch_mode_does_not_rewrite_an_already_loaded_tape() {
    let mut punch = TapePunch::new(PunchMode::Append);
    punch.load(b"OLD");
    punch.set_mode(PunchMode::Overwrite);
    assert_eq!(punch.bytes(), Some(b"OLD".as_slice()));
    assert!(punch.start());
    assert!(punch.punch(b"NEW"));
    assert_eq!(punch.bytes(), Some(b"OLDNEW".as_slice()));
}
