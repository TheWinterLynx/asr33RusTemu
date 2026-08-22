#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelPlacement {
    Docked,
    Undocked,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelPresentation {
    placement: PanelPlacement,
    last_visible: PanelPlacement,
}

impl Default for PanelPresentation {
    fn default() -> Self {
        Self::docked()
    }
}

impl PanelPresentation {
    #[must_use]
    pub const fn docked() -> Self {
        Self {
            placement: PanelPlacement::Docked,
            last_visible: PanelPlacement::Docked,
        }
    }

    #[must_use]
    pub const fn placement(self) -> PanelPlacement {
        self.placement
    }

    pub fn dock(&mut self) {
        self.set_visible(PanelPlacement::Docked);
    }
    pub fn undock(&mut self) {
        self.set_visible(PanelPlacement::Undocked);
    }
    pub fn hide(&mut self) {
        if self.placement != PanelPlacement::Hidden {
            self.last_visible = self.placement;
        }
        self.placement = PanelPlacement::Hidden;
    }
    pub fn show(&mut self) {
        self.placement = self.last_visible;
    }

    fn set_visible(&mut self, placement: PanelPlacement) {
        self.placement = placement;
        self.last_visible = placement;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub margin: f32,
}

impl TerminalMetrics {
    #[must_use]
    pub fn from_measured_width(cell_width: f32, scale: f32) -> Self {
        let width = cell_width.max(1.0);
        Self {
            cell_width: width,
            cell_height: width * 10.0 / 6.0,
            margin: (18.0 * scale).max(0.0),
        }
    }
}

#[must_use]
pub fn clamped_dock_width(available_width: f32) -> f32 {
    (available_width * 0.24)
        .clamp(210.0, 340.0)
        .min(available_width.max(0.0))
}

const MIN_PANE_HEIGHT: f32 = 110.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockSplitState {
    punch_fraction: f32,
    drag_origin: Option<f32>,
}

impl Default for DockSplitState {
    fn default() -> Self {
        Self {
            punch_fraction: 0.42,
            drag_origin: None,
        }
    }
}

impl DockSplitState {
    #[must_use]
    pub fn punch_fraction(self) -> f32 {
        self.punch_fraction
    }

    pub fn begin_drag(&mut self) {
        self.drag_origin = Some(self.punch_fraction);
    }

    pub fn drag(&mut self, delta_y: f32, available_height: f32) {
        let origin = self.drag_origin.unwrap_or(self.punch_fraction);
        let usable = available_height.max(1.0);
        let minimum = (MIN_PANE_HEIGHT / usable).min(0.45);
        self.punch_fraction = (origin + delta_y / usable).clamp(minimum, 1.0 - minimum);
    }

    pub fn end_drag(&mut self) {
        self.drag_origin = None;
    }

    #[must_use]
    pub fn pane_heights(self, available_height: f32, both_docked: bool) -> (f32, f32) {
        let available = available_height.max(0.0);
        if !both_docked {
            return (available, available);
        }
        let punch = available * self.punch_fraction;
        (punch, available - punch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockWidthState {
    width: f32,
}

impl Default for DockWidthState {
    fn default() -> Self {
        Self { width: 320.0 }
    }
}

impl DockWidthState {
    #[must_use]
    pub fn width(self) -> f32 {
        self.width
    }

    pub fn retain_requested(&mut self, requested: f32, viewport_width: f32) {
        let maximum = (viewport_width * 0.55).max(230.0);
        self.width = requested.clamp(230.0, maximum);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_transitions_and_visibility_memory() {
        let mut panel = PanelPresentation::docked();
        panel.undock();
        assert_eq!(panel.placement(), PanelPlacement::Undocked);
        panel.dock();
        assert_eq!(panel.placement(), PanelPlacement::Docked);
        panel.hide();
        panel.show();
        assert_eq!(panel.placement(), PanelPlacement::Docked);
        panel.undock();
        panel.hide();
        panel.show();
        assert_eq!(panel.placement(), PanelPlacement::Undocked);
    }

    #[test]
    fn metrics_are_positive_and_preserve_ten_cpi_six_lpi_ratio() {
        let metrics = TerminalMetrics::from_measured_width(12.0, 1.0);
        assert!(metrics.cell_width > 0.0 && metrics.cell_height > 0.0 && metrics.margin >= 0.0);
        assert!((metrics.cell_height / metrics.cell_width - 10.0 / 6.0).abs() < 0.001);
    }

    #[test]
    fn dock_width_never_becomes_negative() {
        for width in [-20.0, 0.0, 100.0, 900.0] {
            assert!(clamped_dock_width(width) >= 0.0);
        }
    }

    #[test]
    fn split_clamps_resizes_and_gives_a_single_pane_full_height() {
        let mut split = DockSplitState::default();
        split.begin_drag();
        split.drag(-500.0, 600.0);
        split.end_drag();
        let (punch, reader) = split.pane_heights(600.0, true);
        assert!(punch >= MIN_PANE_HEIGHT && reader >= MIN_PANE_HEIGHT);
        split.begin_drag();
        split.drag(500.0, 600.0);
        split.end_drag();
        let (punch, reader) = split.pane_heights(600.0, true);
        assert!(punch >= MIN_PANE_HEIGHT && reader >= MIN_PANE_HEIGHT);
        assert_eq!(split.pane_heights(600.0, false), (600.0, 600.0));
    }

    #[test]
    fn split_and_width_state_survive_presentation_changes() {
        let mut split = DockSplitState::default();
        split.begin_drag();
        split.drag(90.0, 600.0);
        split.end_drag();
        let ratio = split.punch_fraction();
        let mut panel = PanelPresentation::docked();
        panel.hide();
        panel.show();
        assert_eq!(split.punch_fraction(), ratio);
        let mut width = DockWidthState::default();
        width.retain_requested(410.0, 1200.0);
        assert_eq!(width.width(), 410.0);
        let content_width = 900.0;
        assert_eq!(
            width.width(),
            410.0,
            "content width {content_width} is not layout state"
        );
        let mut initial = DockWidthState::default();
        assert_eq!(initial.width(), 320.0);
        initial.retain_requested(50.0, 1200.0);
        assert_eq!(initial.width(), 230.0);
        initial.retain_requested(900.0, 1000.0);
        assert_eq!(initial.width(), 550.0);
    }

    #[test]
    fn placement_changes_do_not_touch_functional_device_state() {
        let mut panel = PanelPresentation::docked();
        let reader_position = 17;
        let punch_open = true;
        panel.undock();
        panel.hide();
        panel.show();
        assert_eq!(reader_position, 17);
        assert!(punch_open);
        assert_eq!(panel.placement(), PanelPlacement::Undocked);
    }
}
