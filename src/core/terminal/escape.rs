//! Stream-safe filtering of ANSI CSI and OSC escape sequences.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

/// Compatibility filter used to make modern hosts usable with the ASR-33.
#[derive(Debug, Default)]
pub struct EscapeFilter {
    state: EscapeState,
}

impl EscapeFilter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume an ASCII chunk and return bytes not belonging to CSI/OSC or
    /// single-character escape sequences. State is retained between calls.
    #[must_use]
    pub fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());

        for &byte in data {
            match self.state {
                EscapeState::Ground => {
                    if byte == 0x1b {
                        self.state = EscapeState::Escape;
                    } else {
                        output.push(byte);
                    }
                }
                EscapeState::Escape => {
                    self.state = match byte {
                        b'[' => EscapeState::Csi,
                        b']' => EscapeState::Osc,
                        _ => EscapeState::Ground,
                    };
                }
                EscapeState::Csi => {
                    if (b'@'..=b'~').contains(&byte) {
                        self.state = EscapeState::Ground;
                    }
                }
                EscapeState::Osc => match byte {
                    0x07 => self.state = EscapeState::Ground,
                    0x1b => self.state = EscapeState::OscEscape,
                    _ => {}
                },
                EscapeState::OscEscape => {
                    self.state = if byte == b'\\' {
                        EscapeState::Ground
                    } else {
                        EscapeState::Osc
                    };
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::EscapeFilter;

    #[test]
    fn strips_sequences_across_chunks() {
        let mut filter = EscapeFilter::new();
        let chunks: [&[u8]; 4] = [b"A\x1b[31", b"mB\x1b]title", b"\x07C\x1b]other\x1b", b"\\D"];
        let output: Vec<u8> = chunks
            .into_iter()
            .flat_map(|chunk| filter.feed(chunk))
            .collect();
        assert_eq!(output, b"ABCD");
    }

    #[test]
    fn swallows_unknown_single_character_escape() {
        assert_eq!(EscapeFilter::new().feed(b"A\x1b7B"), b"AB");
    }
}
