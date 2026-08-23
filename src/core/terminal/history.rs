//! Terminal lines, overstrike cells, and bounded scrollback.

use std::collections::VecDeque;

/// One fixed-width terminal line. Each cell retains every overstrike.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Line {
    logical_number: u64,
    cells: Vec<Vec<char>>,
}

impl Line {
    #[must_use]
    pub fn new(width: usize, logical_number: u64) -> Self {
        Self {
            logical_number,
            cells: vec![Vec::new(); width],
        }
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.cells.len()
    }

    #[must_use]
    pub fn logical_number(&self) -> u64 {
        self.logical_number
    }

    pub fn add_char(&mut self, column: usize, character: char) {
        if let Some(cell) = self.cells.get_mut(column) {
            cell.push(character);
        }
    }

    #[must_use]
    pub fn strike_stack(&self, column: usize) -> &[char] {
        self.cells.get(column).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn top_characters(&self) -> String {
        self.cells
            .iter()
            .map(|cell| cell.last().copied().unwrap_or(' '))
            .collect()
    }
}

/// Bounded terminal scrollback, ordered from oldest to newest line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineHistory {
    width: usize,
    capacity: usize,
    lines: VecDeque<Line>,
}

impl LineHistory {
    #[must_use]
    pub fn new(capacity: usize, width: usize, initial_logical_number: u64) -> Self {
        let mut lines = VecDeque::with_capacity(capacity);
        lines.push_back(Line::new(width, initial_logical_number));
        Self {
            width,
            capacity,
            lines,
        }
    }

    pub fn add_char(&mut self, column: usize, character: char) {
        if let Some(line) = self.lines.back_mut() {
            line.add_char(column, character);
        }
    }

    pub fn add_line(&mut self, logical_number: u64) {
        self.lines.push_back(Line::new(self.width, logical_number));
        if self.lines.len() > self.capacity {
            self.lines.pop_front();
        }
    }

    /// Drop all retained paper and start again with one blank logical line.
    pub fn clear(&mut self, initial_logical_number: u64) {
        self.lines.clear();
        self.lines
            .push_back(Line::new(self.width, initial_logical_number));
    }

    #[must_use]
    pub fn line(&self, row: usize) -> Option<&Line> {
        self.lines.get(row)
    }

    #[must_use]
    pub fn lines(&self) -> &VecDeque<Line> {
        &self.lines
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    #[must_use]
    pub fn top_logical_number(&self) -> Option<u64> {
        self.lines.front().map(Line::logical_number)
    }

    #[must_use]
    pub fn bottom_logical_number(&self) -> Option<u64> {
        self.lines.back().map(Line::logical_number)
    }
}
