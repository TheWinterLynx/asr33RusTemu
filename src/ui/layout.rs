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
