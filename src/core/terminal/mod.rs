//! Deterministic ASR-33 terminal state and byte processing.

mod escape;
mod history;
mod parity;

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

pub use escape::EscapeFilter;
pub use history::{Line, LineHistory};
pub use parity::{encode_even_parity, mask_parity_bit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalOptions {
    pub columns: usize,
    pub rows: usize,
    pub scrollback: usize,
    pub autowrap: bool,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            columns: 72,
            rows: 24,
            scrollback: 200,
            autowrap: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalError {
    ZeroColumns,
    HistoryCapacityOverflow,
    LogicalLineOverflow,
}

impl fmt::Display for TerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroColumns => formatter.write_str("terminal must have at least one column"),
            Self::HistoryCapacityOverflow => {
                formatter.write_str("terminal history capacity overflow")
            }
            Self::LogicalLineOverflow => {
                formatter.write_str("terminal logical line number overflow")
            }
        }
    }
}

impl Error for TerminalError {}

/// Notification retained for the future audio adapter. It belongs to terminal
/// output and has no dependency on an audio implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CharacterEvent {
    pub character: char,
    pub column: usize,
}

/// Effects produced while receiving a backend chunk.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReceiveEffects {
    /// Original eight-bit bytes, before parity masking and escape filtering.
    pub forwarded_bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Terminal {
    width: usize,
    height: usize,
    autowrap: bool,
    current_column: usize,
    current_line_number: u64,
    line_history: LineHistory,
    escape_filter: EscapeFilter,
    character_events: VecDeque<CharacterEvent>,
    printing_enabled: bool,
}

impl Terminal {
    pub fn new(options: TerminalOptions) -> Result<Self, TerminalError> {
        if options.columns == 0 {
            return Err(TerminalError::ZeroColumns);
        }
        let history_capacity = options
            .rows
            .checked_add(options.scrollback)
            .ok_or(TerminalError::HistoryCapacityOverflow)?;
        if history_capacity == 0 {
            return Err(TerminalError::HistoryCapacityOverflow);
        }

        Ok(Self {
            width: options.columns,
            height: options.rows,
            autowrap: options.autowrap,
            current_column: 0,
            current_line_number: 0,
            line_history: LineHistory::new(history_capacity, options.columns, 0),
            escape_filter: EscapeFilter::new(),
            character_events: VecDeque::new(),
            printing_enabled: true,
        })
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub fn line_history(&self) -> &LineHistory {
        &self.line_history
    }

    #[must_use]
    pub fn cursor_position(&self) -> (usize, u64) {
        (self.current_column, self.current_line_number)
    }

    #[must_use]
    pub fn printing_enabled(&self) -> bool {
        self.printing_enabled
    }

    pub fn enable_printing(&mut self) {
        self.printing_enabled = true;
    }

    pub fn disable_printing(&mut self) {
        self.printing_enabled = false;
    }

    /// Clear only the visible/retained terminal paper.
    ///
    /// This is a local UI operation: it does not emit bytes, alter printer
    /// enablement, reset escape parsing, or affect transport state.
    pub fn clear_paper(&mut self) {
        self.current_column = 0;
        self.current_line_number = 0;
        self.line_history.clear(0);
    }

    #[must_use]
    pub fn character_event_count(&self) -> usize {
        self.character_events.len()
    }

    pub fn pop_character_event(&mut self) -> Option<CharacterEvent> {
        self.character_events.pop_front()
    }

    /// Process bytes received from a backend.
    ///
    /// The returned bytes intentionally remain unmasked and unfiltered so an
    /// outer application can reproduce the Python frontend/punch callback.
    pub fn receive_data(&mut self, data: &[u8]) -> Result<ReceiveEffects, TerminalError> {
        let effects = ReceiveEffects {
            forwarded_bytes: data.to_vec(),
        };
        let masked = mask_parity_bit(data);
        let filtered = self.escape_filter.feed(&masked);

        if self.printing_enabled {
            for byte in filtered {
                self.process_byte(byte)?;
            }
        }

        Ok(effects)
    }

    fn process_byte(&mut self, byte: u8) -> Result<(), TerminalError> {
        let character = char::from(byte);
        match byte {
            b'\r' => self.current_column = 0,
            b'\n' | 0x0b => self.advance_line()?,
            0x08 | 0x0c => {
                // legacy behavior: form feed and backspace both move left.
                self.current_column = self.current_column.saturating_sub(1);
            }
            b'\t' => {
                self.current_column = (self.current_column.saturating_add(8)) & !7;
            }
            _ if is_legacy_printable_ascii(byte) => {
                self.line_history.add_char(self.current_column, character);
                self.current_column = self.current_column.saturating_add(1);
            }
            _ => {}
        }

        if self.autowrap && self.current_column >= self.width {
            self.current_column -= self.width;
            self.advance_line()?;
        }

        // legacy behavior: without autowrap, bytes continue overstriking the
        // last column after the cursor reaches the right edge.
        self.current_column = self.current_column.min(self.width - 1);
        self.character_events.push_back(CharacterEvent {
            character,
            column: self.current_column,
        });
        Ok(())
    }

    fn advance_line(&mut self) -> Result<(), TerminalError> {
        self.current_line_number = self
            .current_line_number
            .checked_add(1)
            .ok_or(TerminalError::LogicalLineOverflow)?;
        self.line_history.add_line(self.current_line_number);
        Ok(())
    }
}

/// Python's `str.isprintable()` classification for masked seven-bit ASCII.
#[must_use]
pub const fn is_legacy_printable_ascii(byte: u8) -> bool {
    byte >= 0x20 && byte <= 0x7e
}

#[cfg(test)]
mod tests {
    use super::{Terminal, TerminalOptions};

    #[test]
    fn clear_paper_resets_text_history_and_cursor_without_changing_printer_state() {
        let mut terminal = Terminal::new(TerminalOptions {
            columns: 8,
            rows: 2,
            scrollback: 3,
            autowrap: false,
        })
        .expect("terminal");
        terminal
            .receive_data(b"ABC\r\nDEF")
            .expect("terminal input");
        assert!(terminal.line_history().len() > 1);
        assert_ne!(terminal.cursor_position(), (0, 0));
        terminal.disable_printing();
        let queued_audio_events = terminal.character_event_count();

        terminal.clear_paper();

        assert_eq!(terminal.cursor_position(), (0, 0));
        assert_eq!(terminal.line_history().len(), 1);
        assert_eq!(
            terminal
                .line_history()
                .line(0)
                .expect("blank line")
                .top_characters(),
            "        "
        );
        assert!(!terminal.printing_enabled());
        assert_eq!(terminal.character_event_count(), queued_audio_events);
    }
}
