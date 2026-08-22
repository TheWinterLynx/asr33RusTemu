use eframe::egui::{self, Align2, FontId};

use crate::core::config::BitLabelBase;

use super::theme::ThemePalette;

pub const DATA_RADIUS: f32 = 6.5;
pub const SPROCKET_RADIUS: f32 = 4.0;
const PITCH: f32 = 18.0;
const ROW_HEIGHT: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapeColumn {
    Data(u8),
    Sprocket,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapeRow {
    pub offset: usize,
    pub byte: u8,
    pub ascii: char,
    pub marker: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedTape {
    pub rows: Vec<TapeRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapeSourceOrder {
    NewestFirst,
    OldestFirst,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapeViewHistory {
    bytes: Vec<u8>,
    max_rows: usize,
}

impl TapeViewHistory {
    #[must_use]
    pub fn new(max_rows: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_rows,
        }
    }

    pub fn push_confirmed(&mut self, byte: u8) {
        if self.max_rows == 0 {
            return;
        }
        self.bytes.insert(0, byte);
        self.bytes.truncate(self.max_rows);
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[must_use]
pub fn tape_columns(base: BitLabelBase) -> [TapeColumn; 9] {
    let first = match base {
        BitLabelBase::Zero => 0,
        BitLabelBase::One => 1,
    };
    [
        TapeColumn::Data(first),
        TapeColumn::Data(first + 1),
        TapeColumn::Data(first + 2),
        TapeColumn::Sprocket,
        TapeColumn::Data(first + 3),
        TapeColumn::Data(first + 4),
        TapeColumn::Data(first + 5),
        TapeColumn::Data(first + 6),
        TapeColumn::Data(first + 7),
    ]
}

#[must_use]
pub fn format_tape_numeric(byte: u8) -> String {
    format!("0x{byte:02X} 0o{byte:03o}")
}

#[must_use]
pub fn tape_ascii(byte: u8, mask_msb: bool) -> char {
    let value = if mask_msb { byte & 0x7f } else { byte };
    if value.is_ascii_graphic() || value == b' ' {
        char::from(value)
    } else {
        '·'
    }
}

#[must_use]
pub fn prepare_tape(
    bytes: &[u8],
    max_rows: usize,
    mask_msb: bool,
    order: TapeSourceOrder,
    mark_newest: bool,
) -> PreparedTape {
    let values: Box<dyn Iterator<Item = (usize, u8)> + '_> = match order {
        TapeSourceOrder::NewestFirst => Box::new(bytes.iter().copied().enumerate()),
        TapeSourceOrder::OldestFirst => Box::new(bytes.iter().copied().enumerate().rev()),
    };
    let rows = values
        .take(max_rows)
        .enumerate()
        .map(|(visual_index, (offset, byte))| TapeRow {
            offset,
            byte,
            ascii: tape_ascii(byte, mask_msb),
            marker: mark_newest && visual_index == 0,
        })
        .collect();
    PreparedTape { rows }
}

pub struct TapeRendererOptions {
    pub max_rows: usize,
    pub ghost_outline: bool,
    pub bit_label_base: BitLabelBase,
    pub ascii_char_mask_msb: bool,
    pub source_order: TapeSourceOrder,
    pub mark_newest: bool,
    pub palette: ThemePalette,
}

pub fn render_tape(ui: &mut egui::Ui, bytes: &[u8], options: TapeRendererOptions) {
    let prepared = prepare_tape(
        bytes,
        options.max_rows,
        options.ascii_char_mask_msb,
        options.source_order,
        options.mark_newest,
    );
    let columns = tape_columns(options.bit_label_base);
    let tape_width = PITCH * 9.0;
    let numeric_width = 126.0;
    let total_width = tape_width + numeric_width + 34.0;
    let height = ROW_HEIGHT * (prepared.rows.len() as f32 + 1.0);
    let max_height = ui.available_height().max(0.0);
    egui::ScrollArea::both()
        .max_height(max_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(total_width, height), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let tape_origin = rect.left() + 12.0;
            for (index, column) in columns.iter().enumerate() {
                let label = match column {
                    TapeColumn::Data(bit) => bit.to_string(),
                    TapeColumn::Sprocket => "S".to_owned(),
                };
                painter.text(
                    egui::pos2(tape_origin + PITCH * (index as f32 + 0.5), rect.top()),
                    Align2::CENTER_TOP,
                    label,
                    FontId::monospace(11.0),
                    options.palette.muted_text,
                );
            }
            for (row_index, row) in prepared.rows.iter().enumerate() {
                let center_y = rect.top() + ROW_HEIGHT * (row_index as f32 + 1.5);
                if row.marker {
                    painter.text(
                        egui::pos2(rect.left(), center_y),
                        Align2::LEFT_CENTER,
                        "▶",
                        FontId::proportional(11.0),
                        options.palette.active,
                    );
                }
                for (column_index, column) in columns.iter().enumerate() {
                    let center =
                        egui::pos2(tape_origin + PITCH * (column_index as f32 + 0.5), center_y);
                    match column {
                        TapeColumn::Sprocket => {
                            painter.circle_filled(
                                center,
                                SPROCKET_RADIUS,
                                options.palette.tape_hole,
                            );
                        }
                        TapeColumn::Data(label) => {
                            let bit = match options.bit_label_base {
                                BitLabelBase::Zero => *label,
                                BitLabelBase::One => label - 1,
                            };
                            let punched = row.byte & (1 << bit) != 0;
                            if punched {
                                painter.circle_filled(
                                    center,
                                    DATA_RADIUS,
                                    options.palette.tape_hole,
                                );
                            } else if options.ghost_outline {
                                painter.circle_stroke(
                                    center,
                                    DATA_RADIUS,
                                    egui::Stroke::new(1.0, options.palette.tape_ghost),
                                );
                            }
                        }
                    }
                }
                let text_x = tape_origin + tape_width + 8.0;
                painter.text(
                    egui::pos2(text_x, center_y),
                    Align2::LEFT_CENTER,
                    row.ascii,
                    FontId::monospace(12.0),
                    options.palette.text,
                );
                painter.text(
                    egui::pos2(text_x + 22.0, center_y),
                    Align2::LEFT_CENTER,
                    format_tape_numeric(row.byte),
                    FontId::monospace(12.0),
                    options.palette.text,
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::paper_tape::{FeedResult, ReaderFeed};
    use crate::core::paper_tape::{PaperTape, ReaderOptions, TapeReader};
    use std::time::Duration;

    fn confirm_next(feed: &mut ReaderFeed, history: &mut TapeViewHistory, now: Duration) {
        feed.tick(now, |_| FeedResult::Accepted);
        let confirmed = feed.confirm_transmitted().expect("byte reaches boundary");
        history.push_confirmed(confirmed.byte);
    }

    #[test]
    fn physical_columns_match_legacy_and_have_one_small_sprocket() {
        assert_eq!(
            tape_columns(BitLabelBase::One),
            [
                TapeColumn::Data(1),
                TapeColumn::Data(2),
                TapeColumn::Data(3),
                TapeColumn::Sprocket,
                TapeColumn::Data(4),
                TapeColumn::Data(5),
                TapeColumn::Data(6),
                TapeColumn::Data(7),
                TapeColumn::Data(8)
            ]
        );
        assert_eq!(
            tape_columns(BitLabelBase::Zero),
            [
                TapeColumn::Data(0),
                TapeColumn::Data(1),
                TapeColumn::Data(2),
                TapeColumn::Sprocket,
                TapeColumn::Data(3),
                TapeColumn::Data(4),
                TapeColumn::Data(5),
                TapeColumn::Data(6),
                TapeColumn::Data(7)
            ]
        );
        assert_eq!(
            tape_columns(BitLabelBase::One)
                .iter()
                .filter(|column| matches!(column, TapeColumn::Data(_)))
                .count(),
            8
        );
        let radii = [SPROCKET_RADIUS, DATA_RADIUS];
        assert!(radii[0] < radii[1]);
    }

    #[test]
    fn numeric_ascii_rows_marker_and_limit_are_pure() {
        assert_eq!(format_tape_numeric(0x00), "0x00 0o000");
        assert_eq!(format_tape_numeric(0x15), "0x15 0o025");
        assert_eq!(format_tape_numeric(0x80), "0x80 0o200");
        assert_eq!(format_tape_numeric(0xff), "0xFF 0o377");
        assert_eq!(tape_ascii(0xc1, true), 'A');
        assert_eq!(tape_ascii(0xc1, false), '·');
        let bytes = [0, 1, 2, 3];
        let prepared = prepare_tape(&bytes, 2, false, TapeSourceOrder::OldestFirst, true);
        assert_eq!(prepared.rows.len(), 2);
        assert!(prepared.rows[0].marker);
        assert_eq!(
            prepared.rows.iter().map(|row| row.byte).collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(bytes, [0, 1, 2, 3]);
    }

    #[test]
    fn history_is_bounded_and_punch_preview_is_newest_first() {
        let mut history = TapeViewHistory::new(3);
        for byte in b"ABCDE" {
            history.push_confirmed(*byte);
        }
        assert_eq!(history.bytes(), b"EDC");
        history.clear();
        assert!(history.bytes().is_empty());
        let punch = prepare_tape(b"ABCDE", 3, false, TapeSourceOrder::OldestFirst, false);
        assert_eq!(
            punch.rows.iter().map(|row| row.byte).collect::<Vec<_>>(),
            b"EDC"
        );
        let appended = prepare_tape(b"ABCDEF", 3, false, TapeSourceOrder::OldestFirst, false);
        assert_eq!(
            appended.rows.iter().map(|row| row.byte).collect::<Vec<_>>(),
            b"FED"
        );
    }

    #[test]
    fn reader_history_tracks_only_confirmed_progress_and_msb_at_emission() {
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: true,
            auto_stop: false,
            set_msb: false,
        });
        reader.load(PaperTape::new(b"\0\0ABC".to_vec()));
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);
        let mut history = TapeViewHistory::new(3);
        assert!(
            history.bytes().is_empty(),
            "load does not visualize source bytes"
        );
        assert!(feed.reader_mut().start());
        confirm_next(&mut feed, &mut history, Duration::ZERO);
        assert_eq!(history.bytes(), b"A");
        feed.reader_mut().set_msb(true);
        confirm_next(&mut feed, &mut history, Duration::from_millis(3));
        confirm_next(&mut feed, &mut history, Duration::from_millis(6));
        assert_eq!(history.bytes(), &[0xc3, 0xc2, b'A']);
    }

    #[test]
    fn rollback_rewind_and_unload_never_leave_visual_rows() {
        let mut reader = TapeReader::new(ReaderOptions::default());
        reader.load(PaperTape::new(b"AB".to_vec()));
        assert!(reader.start());
        let mut feed = ReaderFeed::new(reader, Duration::ZERO);
        let mut history = TapeViewHistory::new(4);
        feed.tick(Duration::ZERO, |_| FeedResult::Accepted);
        feed.rollback_unconfirmed();
        assert!(history.bytes().is_empty());
        assert_eq!(feed.reader().position(), 0);
        confirm_next(&mut feed, &mut history, Duration::from_millis(3));
        assert_eq!(history.bytes(), b"A");
        feed.reader_mut().stop();
        assert!(feed.reader_mut().rewind());
        history.clear();
        assert_eq!(feed.reader().position(), 0);
        assert!(feed.reader().tape().is_some());
        assert!(history.bytes().is_empty());
        feed.reader_mut().unload();
        history.clear();
        assert!(history.bytes().is_empty());
    }
}
