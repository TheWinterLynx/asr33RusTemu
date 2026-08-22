use eframe::egui::{self, Align2, FontId};

use crate::core::config::BitLabelBase;

use super::theme::ThemePalette;

pub const DATA_RADIUS: f32 = 6.5;
pub const SPROCKET_RADIUS: f32 = 4.0;
const PITCH: f32 = 18.0;
const ROW_HEIGHT: f32 = 22.0;
const TAPE_WIDTH: f32 = PITCH * 9.0;
const TOTAL_WIDTH: f32 = TAPE_WIDTH + 160.0;

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

#[derive(Clone, Debug, PartialEq)]
pub struct ReaderTapeViewState {
    follow_reader: bool,
    drag_origin: Option<usize>,
}

impl Default for ReaderTapeViewState {
    fn default() -> Self {
        Self {
            follow_reader: true,
            drag_origin: None,
        }
    }
}

impl ReaderTapeViewState {
    #[must_use]
    pub const fn follows_reader(&self) -> bool {
        self.follow_reader
    }

    pub fn follow_reader(&mut self) {
        self.follow_reader = true;
    }

    pub fn inspect_manually(&mut self) {
        self.follow_reader = false;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[must_use]
pub fn position_from_total_drag(origin: usize, total_delta_y: f32, tape_length: usize) -> usize {
    if !total_delta_y.is_finite() {
        return origin.min(tape_length);
    }
    let row_delta = (-total_delta_y / ROW_HEIGHT).round() as isize;
    origin.saturating_add_signed(row_delta).min(tape_length)
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

/// Render the complete stored tape with row virtualization. The returned
/// position is a physical seek requested by dragging the tape, not scrolling.
pub fn render_reader_tape(
    ui: &mut egui::Ui,
    bytes: &[u8],
    reader_position: usize,
    reader_running: bool,
    set_msb: bool,
    state: &mut ReaderTapeViewState,
    options: TapeRendererOptions,
) -> Option<usize> {
    let available_height = ui.available_height().max(ROW_HEIGHT);
    let mut area = egui::ScrollArea::both()
        .max_height(available_height)
        .auto_shrink([false, false]);
    if state.follow_reader {
        let centered = reader_position as f32 * ROW_HEIGHT - available_height * 0.45;
        area = area.vertical_scroll_offset(centered.max(0.0));
    }
    let columns = tape_columns(options.bit_label_base);
    let mut requested = None;
    let output = area.show_rows(
        ui,
        ROW_HEIGHT,
        bytes.len().saturating_add(1),
        |ui, range| {
            let height = ROW_HEIGHT * range.len() as f32;
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(TOTAL_WIDTH, height),
                if reader_running {
                    egui::Sense::hover()
                } else {
                    egui::Sense::drag()
                },
            );
            let response = response.on_hover_cursor(if reader_running {
                egui::CursorIcon::Default
            } else {
                egui::CursorIcon::ResizeVertical
            });
            if response.drag_started() {
                state.drag_origin = Some(reader_position);
            }
            if let (Some(origin), Some(delta)) = (state.drag_origin, response.total_drag_delta()) {
                requested = Some(position_from_total_drag(origin, delta.y, bytes.len()));
            }
            if response.drag_stopped() {
                state.drag_origin = None;
            }
            let painter = ui.painter_at(rect);
            let tape_origin = rect.left() + 12.0;
            for (visible_index, offset) in range.enumerate() {
                let center_y = rect.top() + ROW_HEIGHT * (visible_index as f32 + 0.5);
                if offset == reader_position {
                    painter.rect_filled(
                        egui::Rect::from_center_size(
                            egui::pos2(rect.center().x, center_y),
                            egui::vec2(TOTAL_WIDTH, ROW_HEIGHT),
                        ),
                        0.0,
                        options.palette.active,
                    );
                    painter.text(
                        egui::pos2(rect.left(), center_y),
                        Align2::LEFT_CENTER,
                        "▶ HEAD",
                        FontId::proportional(10.0),
                        options.palette.active,
                    );
                }
                let Some(&byte) = bytes.get(offset) else {
                    continue;
                };
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
                            if byte & (1 << bit) != 0 {
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
                let text_x = tape_origin + TAPE_WIDTH + 8.0;
                painter.text(
                    egui::pos2(text_x, center_y),
                    Align2::LEFT_CENTER,
                    format!(
                        "{offset}: {}",
                        tape_ascii(byte, options.ascii_char_mask_msb)
                    ),
                    FontId::monospace(12.0),
                    options.palette.text,
                );
                painter.text(
                    egui::pos2(text_x + 50.0, center_y),
                    Align2::LEFT_CENTER,
                    format_tape_numeric(byte),
                    FontId::monospace(12.0),
                    options.palette.text,
                );
                if offset == reader_position && set_msb {
                    painter.text(
                        egui::pos2(text_x + 50.0, center_y + 9.0),
                        Align2::LEFT_CENTER,
                        format!("TX {}", format_tape_numeric(byte | 0x80)),
                        FontId::monospace(9.0),
                        options.palette.muted_text,
                    );
                }
            }
        },
    );
    if ui.rect_contains_pointer(output.inner_rect)
        && ui.input(|input| input.smooth_scroll_delta.y != 0.0)
    {
        state.inspect_manually();
    }
    requested
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn punch_preview_is_bounded_and_newest_first() {
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
    fn physical_drag_uses_total_delta_and_clamps_without_rebound() {
        assert_eq!(position_from_total_drag(50, 0.0, 100), 50);
        assert_eq!(position_from_total_drag(50, 22.0, 100), 49);
        assert_eq!(position_from_total_drag(50, 44.0, 100), 48);
        assert_eq!(position_from_total_drag(50, -66.0, 100), 53);
        assert_eq!(position_from_total_drag(0, 44.0, 100), 0);
        assert_eq!(position_from_total_drag(99, -440.0, 100), 100);
        assert_eq!(position_from_total_drag(25, f32::NAN, 100), 25);
    }

    #[test]
    fn reader_view_follow_and_manual_inspection_are_independent_state() {
        let mut state = ReaderTapeViewState::default();
        assert!(state.follows_reader());
        state.inspect_manually();
        assert!(!state.follows_reader());
        state.follow_reader();
        assert!(state.follows_reader());
        state.reset();
        assert!(state.follows_reader());
    }
}
