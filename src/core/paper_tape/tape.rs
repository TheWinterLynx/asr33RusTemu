//! Immutable paper-tape bytes and trailer metadata.

/// Start positions of contiguous trailer regions at the physical end of a tape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrailerPositions {
    pub octal_200: Option<usize>,
    pub null: Option<usize>,
}

/// An immutable loaded paper tape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperTape {
    bytes: Vec<u8>,
    trailers: TrailerPositions,
}

impl PaperTape {
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        let trailers = detect_trailers(&bytes);
        Self { bytes, trailers }
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn trailers(&self) -> TrailerPositions {
        self.trailers
    }
}

fn detect_trailers(bytes: &[u8]) -> TrailerPositions {
    let mut before_nulls = bytes.len();
    while before_nulls > 0 && bytes[before_nulls - 1] == 0o000 {
        before_nulls -= 1;
    }
    let null = (before_nulls < bytes.len()).then_some(before_nulls);

    let mut before_octal_200 = before_nulls;
    while before_octal_200 > 0 && bytes[before_octal_200 - 1] == 0o200 {
        before_octal_200 -= 1;
    }
    let octal_200 = (before_octal_200 < before_nulls).then_some(before_octal_200);

    TrailerPositions { octal_200, null }
}

#[cfg(test)]
mod tests {
    use super::{PaperTape, TrailerPositions};

    #[test]
    fn detects_adjacent_trailer_regions() {
        let tape = PaperTape::new(b"AB\x80\x80\x00\x00".to_vec());
        assert_eq!(
            tape.trailers(),
            TrailerPositions {
                octal_200: Some(2),
                null: Some(4),
            }
        );
    }

    #[test]
    fn detects_null_only_and_octal_200_only_trailers() {
        assert_eq!(
            PaperTape::new(b"A\x00".to_vec()).trailers(),
            TrailerPositions {
                octal_200: None,
                null: Some(1),
            }
        );
        assert_eq!(
            PaperTape::new(b"A\x80".to_vec()).trailers(),
            TrailerPositions {
                octal_200: Some(1),
                null: None,
            }
        );
    }
}
