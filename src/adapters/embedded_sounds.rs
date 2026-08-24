//! Compile-time embedded ASR-33 sound library.
//!
//! The release executable must not depend on a sibling `sounds` directory.
//! Keep the repository WAV files as build-time source assets and bake their
//! bytes directly into the binary with `include_bytes!`.

#[cfg(windows)]
pub(super) const EMBEDDED_SOUNDS: &[(&str, &[u8])] = &[
    ("down-bell", include_bytes!("../../sounds/down-bell.wav")),
    ("down-cr-01", include_bytes!("../../sounds/down-cr-01.wav")),
    ("down-cr-02", include_bytes!("../../sounds/down-cr-02.wav")),
    ("down-cr-03", include_bytes!("../../sounds/down-cr-03.wav")),
    ("down-hum", include_bytes!("../../sounds/down-hum.wav")),
    (
        "down-key-01",
        include_bytes!("../../sounds/down-key-01.wav"),
    ),
    (
        "down-key-02",
        include_bytes!("../../sounds/down-key-02.wav"),
    ),
    (
        "down-key-03",
        include_bytes!("../../sounds/down-key-03.wav"),
    ),
    (
        "down-key-04",
        include_bytes!("../../sounds/down-key-04.wav"),
    ),
    (
        "down-key-05",
        include_bytes!("../../sounds/down-key-05.wav"),
    ),
    (
        "down-key-06",
        include_bytes!("../../sounds/down-key-06.wav"),
    ),
    (
        "down-key-07",
        include_bytes!("../../sounds/down-key-07.wav"),
    ),
    ("down-lid", include_bytes!("../../sounds/down-lid.wav")),
    (
        "down-motor-off",
        include_bytes!("../../sounds/down-motor-off.wav"),
    ),
    (
        "down-motor-on",
        include_bytes!("../../sounds/down-motor-on.wav"),
    ),
    (
        "down-platen",
        include_bytes!("../../sounds/down-platen.wav"),
    ),
    (
        "down-print-chars-01",
        include_bytes!("../../sounds/down-print-chars-01.wav"),
    ),
    (
        "down-print-chars-02",
        include_bytes!("../../sounds/down-print-chars-02.wav"),
    ),
    (
        "down-print-spaces-01",
        include_bytes!("../../sounds/down-print-spaces-01.wav"),
    ),
    (
        "down-print-spaces-02 ",
        include_bytes!("../../sounds/down-print-spaces-02 .wav"),
    ),
    (
        "down-print-spaces-02",
        include_bytes!("../../sounds/down-print-spaces-02.wav"),
    ),
    (
        "down-tape-reader",
        include_bytes!("../../sounds/down-tape-reader.wav"),
    ),
    ("up-bell", include_bytes!("../../sounds/up-bell.wav")),
    ("up-cr-01", include_bytes!("../../sounds/up-cr-01.wav")),
    ("up-cr-02", include_bytes!("../../sounds/up-cr-02.wav")),
    ("up-hum", include_bytes!("../../sounds/up-hum.wav")),
    ("up-key-01", include_bytes!("../../sounds/up-key-01.wav")),
    ("up-key-02", include_bytes!("../../sounds/up-key-02.wav")),
    ("up-key-03", include_bytes!("../../sounds/up-key-03.wav")),
    ("up-key-04", include_bytes!("../../sounds/up-key-04.wav")),
    ("up-key-05", include_bytes!("../../sounds/up-key-05.wav")),
    ("up-key-06", include_bytes!("../../sounds/up-key-06.wav")),
    ("up-key-07", include_bytes!("../../sounds/up-key-07.wav")),
    ("up-lid", include_bytes!("../../sounds/up-lid.wav")),
    (
        "up-motor-off",
        include_bytes!("../../sounds/up-motor-off.wav"),
    ),
    (
        "up-motor-on",
        include_bytes!("../../sounds/up-motor-on.wav"),
    ),
    ("up-platen", include_bytes!("../../sounds/up-platen.wav")),
    (
        "up-print-chars-01",
        include_bytes!("../../sounds/up-print-chars-01.wav"),
    ),
    (
        "up-print-chars-02",
        include_bytes!("../../sounds/up-print-chars-02.wav"),
    ),
    (
        "up-print-spaces-01",
        include_bytes!("../../sounds/up-print-spaces-01.wav"),
    ),
    (
        "up-print-spaces-02",
        include_bytes!("../../sounds/up-print-spaces-02.wav"),
    ),
    (
        "up-tape-reader",
        include_bytes!("../../sounds/up-tape-reader.wav"),
    ),
];
