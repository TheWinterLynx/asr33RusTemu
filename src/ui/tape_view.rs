use eframe::egui::{self, Align2, FontId};

use crate::core::config::BitLabelBase;

use super::theme::ThemePalette;

pub const NATURAL_SCALE: f32 = 1.0;
pub const MIN_PANEL_SCALE: f32 = 0.64;
pub const MAX_PANEL_SCALE: f32 = 1.25;
pub const REFERENCE_PANEL_WIDTH: f32 = 382.0;
const NATURAL_PITCH: f32 = 18.0;
const NATURAL_DATA_RADIUS: f32 = 6.5;
const NATURAL_SPROCKET_RADIUS: f32 = 4.0;
const NATURAL_ROW_HEIGHT: f32 = 22.0;
const NATURAL_HEAD_GUTTER: f32 = 36.0;
const NATURAL_PADDING: f32 = 8.0;
const NATURAL_METADATA_GAP: f32 = 6.0;
const NATURAL_OFFSET_WIDTH: f32 = 44.0;
const NATURAL_ASCII_WIDTH: f32 = 18.0;
const NATURAL_NUMERIC_WIDTH: f32 = 88.0;
const HEAD_HOLE_GAP: f32 = 4.0;
pub const READER_SCROLL_ID: &str = "paper-tape-reader-scroll";
pub const PUNCH_SCROLL_ID: &str = "paper-tape-punch-scroll";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapePanelMetrics {
    pub scale: f32,
    pub reference_width: f32,
    pub scaled_width: f32,
    pub requires_horizontal_scroll: bool,
    pub title_font_size: f32,
    pub body_font_size: f32,
    pub small_font_size: f32,
    pub button_height: f32,
    pub button_padding_x: f32,
    pub button_padding_y: f32,
    pub item_spacing: f32,
    pub group_spacing: f32,
    pub checkbox_size: f32,
    pub tape: TapeRenderMetrics,
}

impl TapePanelMetrics {
    #[must_use]
    pub fn for_available_width(available_width: f32) -> Self {
        let width = if available_width.is_finite() {
            available_width.max(0.0)
        } else if available_width == f32::INFINITY {
            REFERENCE_PANEL_WIDTH * MAX_PANEL_SCALE
        } else {
            0.0
        };
        let scale = (width / REFERENCE_PANEL_WIDTH).clamp(MIN_PANEL_SCALE, MAX_PANEL_SCALE);
        Self::from_scale(scale, width < REFERENCE_PANEL_WIDTH * MIN_PANEL_SCALE)
    }

    #[must_use]
    pub fn from_scale(scale: f32, requires_horizontal_scroll: bool) -> Self {
        let scale = scale.clamp(MIN_PANEL_SCALE, MAX_PANEL_SCALE);
        Self {
            scale,
            reference_width: REFERENCE_PANEL_WIDTH,
            scaled_width: REFERENCE_PANEL_WIDTH * scale,
            requires_horizontal_scroll,
            title_font_size: 20.0 * scale,
            body_font_size: 14.0 * scale,
            small_font_size: 12.0 * scale,
            button_height: 24.0 * scale,
            button_padding_x: 7.0 * scale,
            button_padding_y: 3.0 * scale,
            item_spacing: 5.0 * scale,
            group_spacing: 5.0 * scale,
            checkbox_size: 18.0 * scale,
            tape: TapeRenderMetrics::for_scale(scale, requires_horizontal_scroll),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapeRenderMetrics {
    pub scale: f32,
    pub requires_horizontal_scroll: bool,
    pub head_gutter_width: f32,
    pub pitch: f32,
    pub data_radius: f32,
    pub sprocket_radius: f32,
    pub row_height: f32,
    pub label_font_size: f32,
    pub data_font_size: f32,
    pub numeric_font_size: f32,
    pub offset_font_size: f32,
    pub metadata_gap: f32,
    pub offset_width: f32,
    pub ascii_width: f32,
    pub tape_width: f32,
    pub total_width: f32,
    pub padding: f32,
}

impl TapeRenderMetrics {
    pub const NATURAL_WIDTH: f32 = NATURAL_PADDING * 2.0
        + NATURAL_HEAD_GUTTER
        + NATURAL_PITCH * 9.0
        + NATURAL_METADATA_GAP * 3.0
        + NATURAL_OFFSET_WIDTH
        + NATURAL_ASCII_WIDTH
        + NATURAL_NUMERIC_WIDTH;
    #[must_use]
    pub fn for_scale(scale: f32, requires_horizontal_scroll: bool) -> Self {
        let scaled = |value: f32| value * scale;
        let padding = scaled(NATURAL_PADDING);
        let head_gutter_width = scaled(NATURAL_HEAD_GUTTER);
        let tape_width = scaled(NATURAL_PITCH * 9.0);
        let metadata_gap = scaled(NATURAL_METADATA_GAP);
        let offset_width = scaled(NATURAL_OFFSET_WIDTH);
        let ascii_width = scaled(NATURAL_ASCII_WIDTH);
        let numeric_width = scaled(NATURAL_NUMERIC_WIDTH);
        let total_width = padding * 2.0
            + head_gutter_width
            + tape_width
            + metadata_gap * 3.0
            + offset_width
            + ascii_width
            + numeric_width;
        Self {
            scale,
            requires_horizontal_scroll,
            head_gutter_width,
            pitch: scaled(NATURAL_PITCH),
            data_radius: scaled(NATURAL_DATA_RADIUS),
            sprocket_radius: scaled(NATURAL_SPROCKET_RADIUS),
            row_height: scaled(NATURAL_ROW_HEIGHT),
            label_font_size: scaled(11.0),
            data_font_size: scaled(12.0),
            numeric_font_size: scaled(11.0),
            offset_font_size: scaled(11.0),
            metadata_gap,
            offset_width,
            ascii_width,
            tape_width,
            total_width,
            padding,
        }
    }

    #[must_use]
    pub fn tape_origin(self, left: f32) -> f32 {
        left + self.padding + self.head_gutter_width
    }

    #[must_use]
    pub fn head_marker_right(self, left: f32) -> f32 {
        self.tape_origin(left) - HEAD_HOLE_GAP * self.scale
    }

    #[must_use]
    pub fn first_hole_left(self, left: f32) -> f32 {
        self.tape_origin(left) + self.pitch * 0.5 - self.data_radius
    }
}

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
    pub metrics: TapeRenderMetrics,
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
    follow_scroll_pending: bool,
    last_reader_position: Option<usize>,
    last_scroll_offset: f32,
    last_row_height: Option<f32>,
    drag_origin: Option<usize>,
}

impl Default for ReaderTapeViewState {
    fn default() -> Self {
        Self {
            follow_reader: true,
            follow_scroll_pending: true,
            last_reader_position: None,
            last_scroll_offset: 0.0,
            last_row_height: None,
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
        self.follow_scroll_pending = true;
    }

    pub fn inspect_manually(&mut self) {
        self.follow_reader = false;
        self.follow_scroll_pending = false;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn requested_follow_offset(
        &mut self,
        reader_position: usize,
        viewport_height: f32,
        row_height: f32,
    ) -> Option<f32> {
        let row_height_changed = self
            .last_row_height
            .is_some_and(|previous| (previous - row_height).abs() > f32::EPSILON);
        if self.follow_reader && self.last_reader_position != Some(reader_position) {
            self.follow_scroll_pending = true;
        }
        self.last_reader_position = Some(reader_position);
        let requested = if self.follow_reader && (self.follow_scroll_pending || row_height_changed)
        {
            self.follow_scroll_pending = false;
            Some((reader_position as f32 * row_height - viewport_height * 0.45).max(0.0))
        } else if !self.follow_reader && row_height_changed {
            let logical_top = self.last_scroll_offset
                / self.last_row_height.unwrap_or(row_height).max(f32::EPSILON);
            Some(logical_top * row_height)
        } else {
            None
        };
        self.last_row_height = Some(row_height);
        requested
    }

    fn observe_scroll(
        &mut self,
        offset: f32,
        maximum_offset: f32,
        wheel: bool,
        requested_offset: Option<f32>,
    ) {
        let scrollbar_changed =
            requested_offset.is_none() && (offset - self.last_scroll_offset).abs() > f32::EPSILON;
        let overrode_follow = requested_offset
            .map(|expected| expected.clamp(0.0, maximum_offset))
            .is_some_and(|expected| (offset - expected).abs() > 1.0);
        if wheel || scrollbar_changed || overrode_follow {
            self.inspect_manually();
        }
        self.last_scroll_offset = offset;
    }
}

#[must_use]
pub const fn head_colors(palette: ThemePalette) -> (egui::Color32, egui::Color32) {
    (palette.active, palette.text)
}

fn paint_head_marker(
    painter: &egui::Painter,
    left: f32,
    center_y: f32,
    metrics: TapeRenderMetrics,
    color: egui::Color32,
) {
    let marker_right = metrics.head_marker_right(left);
    let marker_half_height = 4.0 * metrics.scale;
    let marker_width = 7.0 * metrics.scale;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(marker_right - marker_width, center_y - marker_half_height),
            egui::pos2(marker_right - marker_width, center_y + marker_half_height),
            egui::pos2(marker_right, center_y),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

#[must_use]
pub fn position_from_total_drag(origin: usize, total_delta_y: f32, tape_length: usize) -> usize {
    position_from_total_drag_with_row_height(origin, total_delta_y, tape_length, NATURAL_ROW_HEIGHT)
}

#[must_use]
pub fn position_from_total_drag_with_row_height(
    origin: usize,
    total_delta_y: f32,
    tape_length: usize,
    row_height: f32,
) -> usize {
    if !total_delta_y.is_finite() || !row_height.is_finite() || row_height <= 0.0 {
        return origin.min(tape_length);
    }
    let row_delta = (-total_delta_y / row_height).round() as isize;
    origin.saturating_add_signed(row_delta).min(tape_length)
}

pub fn render_tape(ui: &mut egui::Ui, bytes: &[u8], options: TapeRendererOptions) {
    let metrics = options.metrics;
    let prepared = prepare_tape(
        bytes,
        options.max_rows,
        options.ascii_char_mask_msb,
        options.source_order,
        options.mark_newest,
    );
    let columns = tape_columns(options.bit_label_base);
    let height = metrics.row_height * (prepared.rows.len() as f32 + 1.0);
    let max_height = ui.available_height().max(0.0);
    egui::ScrollArea::vertical()
        .hscroll(metrics.requires_horizontal_scroll)
        .id_salt(PUNCH_SCROLL_ID)
        .max_height(max_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(metrics.total_width, height),
                egui::Sense::hover(),
            );
            let painter = ui.painter_at(rect);
            let tape_origin = metrics.tape_origin(rect.left());
            for (index, column) in columns.iter().enumerate() {
                let label = match column {
                    TapeColumn::Data(bit) => bit.to_string(),
                    TapeColumn::Sprocket => "S".to_owned(),
                };
                painter.text(
                    egui::pos2(
                        tape_origin + metrics.pitch * (index as f32 + 0.5),
                        rect.top(),
                    ),
                    Align2::CENTER_TOP,
                    label,
                    FontId::monospace(metrics.label_font_size),
                    options.palette.muted_text,
                );
            }
            for (row_index, row) in prepared.rows.iter().enumerate() {
                let center_y = rect.top() + metrics.row_height * (row_index as f32 + 1.5);
                if row.marker {
                    painter.text(
                        egui::pos2(rect.left(), center_y),
                        Align2::LEFT_CENTER,
                        "▶",
                        FontId::proportional(metrics.label_font_size),
                        options.palette.active,
                    );
                }
                for (column_index, column) in columns.iter().enumerate() {
                    let center = egui::pos2(
                        tape_origin + metrics.pitch * (column_index as f32 + 0.5),
                        center_y,
                    );
                    match column {
                        TapeColumn::Sprocket => {
                            painter.circle_filled(
                                center,
                                metrics.sprocket_radius,
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
                                    metrics.data_radius,
                                    options.palette.tape_hole,
                                );
                            } else if options.ghost_outline {
                                painter.circle_stroke(
                                    center,
                                    metrics.data_radius,
                                    egui::Stroke::new(1.0, options.palette.tape_ghost),
                                );
                            }
                        }
                    }
                }
                let text_x = tape_origin + metrics.tape_width + metrics.metadata_gap;
                painter.text(
                    egui::pos2(text_x, center_y),
                    Align2::LEFT_CENTER,
                    row.ascii,
                    FontId::monospace(metrics.data_font_size),
                    options.palette.text,
                );
                painter.text(
                    egui::pos2(
                        text_x + metrics.ascii_width + metrics.metadata_gap,
                        center_y,
                    ),
                    Align2::LEFT_CENTER,
                    format_tape_numeric(row.byte),
                    FontId::monospace(metrics.numeric_font_size),
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
    let metrics = options.metrics;
    let available_height = ui.available_height().max(metrics.row_height);
    let wheel = ui.rect_contains_pointer(ui.available_rect_before_wrap())
        && ui.input(|input| input.smooth_scroll_delta.y != 0.0);
    if wheel {
        state.inspect_manually();
    }
    let requested_offset =
        state.requested_follow_offset(reader_position, available_height, metrics.row_height);
    let mut area = egui::ScrollArea::vertical()
        .hscroll(metrics.requires_horizontal_scroll)
        .id_salt(READER_SCROLL_ID)
        .max_height(available_height)
        .auto_shrink([false, false]);
    if let Some(offset) = requested_offset {
        area = area.vertical_scroll_offset(offset);
    }
    let columns = tape_columns(options.bit_label_base);
    let mut requested = None;
    let output = area.show_rows(
        ui,
        metrics.row_height,
        bytes.len().saturating_add(1),
        |ui, range| {
            let height = metrics.row_height * range.len() as f32;
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(metrics.total_width, height),
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
                requested = Some(position_from_total_drag_with_row_height(
                    origin,
                    delta.y,
                    bytes.len(),
                    metrics.row_height,
                ));
            }
            if response.drag_stopped() {
                state.drag_origin = None;
            }
            let painter = ui.painter_at(rect);
            let tape_origin = metrics.tape_origin(rect.left());
            for (visible_index, offset) in range.enumerate() {
                let center_y = rect.top() + metrics.row_height * (visible_index as f32 + 0.5);
                let is_head = offset == reader_position;
                if is_head {
                    let (head_background, _) = head_colors(options.palette);
                    painter.rect_filled(
                        egui::Rect::from_center_size(
                            egui::pos2(rect.center().x, center_y),
                            egui::vec2(metrics.total_width, metrics.row_height),
                        ),
                        0.0,
                        head_background,
                    );
                }
                let Some(&byte) = bytes.get(offset) else {
                    if is_head {
                        let (_, foreground) = head_colors(options.palette);
                        paint_head_marker(&painter, rect.left(), center_y, metrics, foreground);
                    }
                    continue;
                };
                for (column_index, column) in columns.iter().enumerate() {
                    let center = egui::pos2(
                        tape_origin + metrics.pitch * (column_index as f32 + 0.5),
                        center_y,
                    );
                    match column {
                        TapeColumn::Sprocket => {
                            painter.circle_filled(
                                center,
                                metrics.sprocket_radius,
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
                                    metrics.data_radius,
                                    options.palette.tape_hole,
                                );
                            } else if options.ghost_outline {
                                painter.circle_stroke(
                                    center,
                                    metrics.data_radius,
                                    egui::Stroke::new(1.0, options.palette.tape_ghost),
                                );
                            }
                        }
                    }
                }
                let text_x = tape_origin + metrics.tape_width + metrics.metadata_gap;
                painter.text(
                    egui::pos2(text_x, center_y),
                    Align2::LEFT_CENTER,
                    offset.to_string(),
                    FontId::monospace(metrics.offset_font_size),
                    options.palette.text,
                );
                painter.text(
                    egui::pos2(text_x + metrics.offset_width, center_y),
                    Align2::LEFT_CENTER,
                    tape_ascii(byte, options.ascii_char_mask_msb),
                    FontId::monospace(metrics.data_font_size),
                    options.palette.text,
                );
                let numeric_x =
                    text_x + metrics.offset_width + metrics.ascii_width + metrics.metadata_gap;
                painter.text(
                    egui::pos2(numeric_x, center_y),
                    Align2::LEFT_CENTER,
                    format_tape_numeric(byte),
                    FontId::monospace(metrics.numeric_font_size),
                    options.palette.text,
                );
                if offset == reader_position && set_msb {
                    painter.text(
                        egui::pos2(numeric_x, center_y + 8.0 * metrics.scale),
                        Align2::LEFT_CENTER,
                        format!("TX {}", format_tape_numeric(byte | 0x80)),
                        FontId::monospace(metrics.offset_font_size),
                        options.palette.muted_text,
                    );
                }
                if is_head {
                    let (_, head_foreground) = head_colors(options.palette);
                    paint_head_marker(&painter, rect.left(), center_y, metrics, head_foreground);
                }
            }
        },
    );
    let maximum_offset = (output.content_size.y - output.inner_rect.height()).max(0.0);
    state.observe_scroll(
        output.state.offset.y,
        maximum_offset,
        wheel,
        requested_offset,
    );
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
        let natural = TapePanelMetrics::for_available_width(REFERENCE_PANEL_WIDTH).tape;
        let radii = [natural.sprocket_radius, natural.data_radius];
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
        let requested = state.requested_follow_offset(100, 220.0, NATURAL_ROW_HEIGHT);
        assert!(requested.is_some());
        state.observe_scroll(requested.unwrap_or_default(), 10_000.0, true, requested);
        assert!(!state.follows_reader(), "mouse wheel disables follow");
        let position = 17;
        state.observe_scroll(1_000.0, 10_000.0, false, None);
        assert_eq!(position, 17, "viewport scroll is not reader position");
        state.follow_reader();
        assert!(
            state.follows_reader(),
            "Follow reader click restores follow"
        );
        let requested = state.requested_follow_offset(101, 220.0, NATURAL_ROW_HEIGHT);
        assert!(
            requested.is_some(),
            "advancing reader requests one recenter"
        );
        state.observe_scroll(requested.unwrap_or_default(), 10_000.0, false, requested);
        assert!(state.follows_reader());
        assert_eq!(
            state.requested_follow_offset(101, 220.0, NATURAL_ROW_HEIGHT),
            None
        );
        state.inspect_manually();
        assert!(!state.follows_reader());
        state.reset();
        assert!(state.follows_reader());
    }

    #[test]
    fn scrollbar_override_disables_follow_and_scroll_identities_are_distinct() {
        let mut state = ReaderTapeViewState::default();
        let requested = state.requested_follow_offset(500, 220.0, NATURAL_ROW_HEIGHT);
        state.observe_scroll(8_000.0, 10_000.0, false, requested);
        assert!(!state.follows_reader());
        assert_ne!(READER_SCROLL_ID, PUNCH_SCROLL_ID);
    }

    #[test]
    fn head_foreground_contrasts_with_background_in_both_themes() {
        for theme in [
            crate::ui::theme::ThemeKind::Light,
            crate::ui::theme::ThemeKind::Dark,
        ] {
            let (background, foreground) = head_colors(theme.palette());
            assert_ne!(background, foreground);
        }
    }

    #[test]
    fn panel_scale_uses_reference_minimum_and_maximum_widths() {
        let natural = TapePanelMetrics::for_available_width(REFERENCE_PANEL_WIDTH);
        assert_eq!(natural.scale, NATURAL_SCALE);
        let compact = TapePanelMetrics::for_available_width(REFERENCE_PANEL_WIDTH * 0.8);
        assert_eq!(compact.scale, 0.8);
        let expanded = TapePanelMetrics::for_available_width(REFERENCE_PANEL_WIDTH * 1.1);
        assert_eq!(expanded.scale, 1.1);
        let maximum = TapePanelMetrics::for_available_width(f32::INFINITY);
        assert_eq!(maximum.scale, MAX_PANEL_SCALE);
        let minimum = TapePanelMetrics::for_available_width(1.0);
        assert_eq!(minimum.scale, MIN_PANEL_SCALE);
        assert!(minimum.requires_horizontal_scroll);
    }

    #[test]
    fn panel_scale_is_finite_and_monotonic() {
        let widths = [0.0, 180.0, 250.0, 350.0, 500.0, f32::INFINITY];
        let mut previous = MIN_PANEL_SCALE;
        for width in widths {
            let metrics = TapePanelMetrics::for_available_width(width);
            assert!(metrics.scale.is_finite());
            assert!((MIN_PANEL_SCALE..=MAX_PANEL_SCALE).contains(&metrics.scale));
            assert!(metrics.scale >= previous);
            previous = metrics.scale;
        }
        assert_eq!(
            TapePanelMetrics::for_available_width(f32::NAN).scale,
            MIN_PANEL_SCALE
        );
    }

    #[test]
    fn every_panel_and_tape_size_derives_from_the_same_scale() {
        for scale in [MIN_PANEL_SCALE, 0.8, NATURAL_SCALE, MAX_PANEL_SCALE] {
            let metrics = TapePanelMetrics::from_scale(scale, false);
            let ratios = [
                metrics.button_height / 24.0,
                metrics.button_padding_x / 7.0,
                metrics.button_padding_y / 3.0,
                metrics.body_font_size / 14.0,
                metrics.small_font_size / 12.0,
                metrics.title_font_size / 20.0,
                metrics.checkbox_size / 18.0,
                metrics.item_spacing / 5.0,
                metrics.group_spacing / 5.0,
                metrics.scaled_width / metrics.reference_width,
                metrics.tape.pitch / NATURAL_PITCH,
                metrics.tape.data_radius / NATURAL_DATA_RADIUS,
                metrics.tape.row_height / NATURAL_ROW_HEIGHT,
                metrics.tape.data_font_size / 12.0,
            ];
            for ratio in ratios {
                assert!((ratio - scale).abs() < 0.001);
            }
            assert!(metrics.tape.sprocket_radius < metrics.tape.data_radius);
            assert!(metrics.tape.pitch > 2.0 * metrics.tape.data_radius);
        }
    }

    #[test]
    fn head_gutter_never_overlaps_first_hole_at_any_supported_scale() {
        for scale in [MIN_PANEL_SCALE, 0.84, NATURAL_SCALE, MAX_PANEL_SCALE] {
            let metrics = TapePanelMetrics::from_scale(scale, false).tape;
            let marker_right = metrics.head_marker_right(0.0);
            let required_gap = HEAD_HOLE_GAP * metrics.scale;
            assert!(marker_right + required_gap <= metrics.first_hole_left(0.0));
        }
    }

    #[test]
    fn bit_labels_remain_centered_over_uniform_columns_at_every_scale() {
        for scale in [MIN_PANEL_SCALE, 0.8, NATURAL_SCALE, MAX_PANEL_SCALE] {
            let metrics = TapePanelMetrics::from_scale(scale, false).tape;
            let origin = metrics.tape_origin(0.0);
            let centers = (0..9)
                .map(|index| origin + metrics.pitch * (index as f32 + 0.5))
                .collect::<Vec<_>>();
            for pair in centers.windows(2) {
                assert!((pair[1] - pair[0] - metrics.pitch).abs() < 0.001);
            }
            assert_eq!(tape_columns(BitLabelBase::One)[3], TapeColumn::Sprocket);
        }
    }

    #[test]
    fn physical_drag_tracks_one_visual_row_at_every_scale() {
        for scale in [NATURAL_SCALE, 0.8, MIN_PANEL_SCALE, MAX_PANEL_SCALE] {
            let metrics = TapePanelMetrics::from_scale(scale, false).tape;
            assert_eq!(
                position_from_total_drag_with_row_height(
                    50,
                    metrics.row_height,
                    100,
                    metrics.row_height,
                ),
                49
            );
            assert_eq!(
                position_from_total_drag_with_row_height(
                    50,
                    -metrics.row_height,
                    100,
                    metrics.row_height,
                ),
                51
            );
        }
    }

    #[test]
    fn resize_preserves_logical_top_row_while_not_following() {
        let mut state = ReaderTapeViewState::default();
        state.inspect_manually();
        state.last_scroll_offset = 500.0 * NATURAL_ROW_HEIGHT;
        state.last_row_height = Some(NATURAL_ROW_HEIGHT);
        let compact_row_height = NATURAL_ROW_HEIGHT * 0.7;
        assert_eq!(
            state.requested_follow_offset(100, 220.0, compact_row_height),
            Some(500.0 * compact_row_height)
        );
        assert!(!state.follows_reader());
    }

    #[test]
    fn calculating_metrics_never_changes_reader_model_or_manual_positioning() {
        use crate::core::paper_tape::{PaperTape, ReaderOptions, ReaderStep, TapeReader};

        let bytes = [b'A'; 100].into_iter().chain([0, b'B']).collect::<Vec<_>>();
        let mut reader = TapeReader::new(ReaderOptions {
            skip_leading_nulls: true,
            set_msb: false,
            auto_stop: false,
        });
        reader.load(PaperTape::new(bytes.clone()));
        reader.seek(100).expect("manual seek");
        for width in [500.0, 350.0, 250.0, 180.0] {
            let _ = TapePanelMetrics::for_available_width(width);
        }
        assert_eq!(reader.position(), 100);
        assert_eq!(reader.tape().map(PaperTape::bytes), Some(bytes.as_slice()));
        assert!(reader.start());
        assert_eq!(reader.step(), ReaderStep::Byte(0));
    }
}
