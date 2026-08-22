use eframe::egui::{self, Color32};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemePalette {
    pub app_background: Color32,
    pub paper: Color32,
    pub text: Color32,
    pub muted_text: Color32,
    pub panel: Color32,
    pub border: Color32,
    pub active: Color32,
    pub inactive: Color32,
    pub error: Color32,
    pub cursor: Color32,
    pub tape_hole: Color32,
    pub tape_ghost: Color32,
}

impl ThemeKind {
    #[must_use]
    pub const fn palette(self) -> ThemePalette {
        match self {
            Self::Light => ThemePalette {
                app_background: Color32::from_rgb(224, 216, 204),
                paper: Color32::from_rgb(255, 238, 221),
                text: Color32::from_rgb(76, 76, 76),
                muted_text: Color32::from_rgb(116, 105, 94),
                panel: Color32::from_rgb(244, 230, 214),
                border: Color32::from_rgb(176, 158, 140),
                active: Color32::from_rgb(190, 174, 151),
                inactive: Color32::from_rgb(231, 217, 201),
                error: Color32::from_rgb(153, 48, 38),
                cursor: Color32::from_rgb(76, 76, 76),
                tape_hole: Color32::from_rgb(70, 65, 59),
                tape_ghost: Color32::from_rgb(171, 151, 131),
            },
            Self::Dark => ThemePalette {
                app_background: Color32::from_rgb(31, 27, 24),
                paper: Color32::from_rgb(61, 49, 39),
                text: Color32::from_rgb(232, 214, 190),
                muted_text: Color32::from_rgb(174, 153, 130),
                panel: Color32::from_rgb(48, 40, 34),
                border: Color32::from_rgb(104, 86, 69),
                active: Color32::from_rgb(121, 95, 70),
                inactive: Color32::from_rgb(65, 54, 45),
                error: Color32::from_rgb(235, 132, 112),
                cursor: Color32::from_rgb(232, 214, 190),
                tape_hole: Color32::from_rgb(225, 204, 179),
                tape_ghost: Color32::from_rgb(116, 92, 72),
            },
        }
    }

    pub fn apply(self, context: &egui::Context) {
        let palette = self.palette();
        let mut visuals = match self {
            Self::Light => egui::Visuals::light(),
            Self::Dark => egui::Visuals::dark(),
        };
        visuals.panel_fill = palette.panel;
        visuals.window_fill = palette.panel;
        visuals.extreme_bg_color = palette.paper;
        visuals.faint_bg_color = palette.inactive;
        visuals.selection.bg_fill = palette.active;
        visuals.selection.stroke.color = palette.text;
        visuals.widgets.noninteractive.bg_fill = palette.panel;
        visuals.widgets.noninteractive.fg_stroke.color = palette.text;
        visuals.widgets.inactive.bg_fill = palette.inactive;
        visuals.widgets.inactive.fg_stroke.color = palette.text;
        visuals.widgets.hovered.bg_fill = palette.active;
        visuals.widgets.active.bg_fill = palette.active;
        visuals.window_stroke.color = palette.border;
        context.set_visuals(visuals);
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeKind;

    #[test]
    fn light_and_dark_palettes_are_complete_and_distinct() {
        let light = ThemeKind::Light.palette();
        let dark = ThemeKind::Dark.palette();
        assert_ne!(light, dark);
        assert_ne!(light.paper, light.text);
        assert_ne!(dark.paper, dark.text);
        assert_ne!(light.tape_hole, light.tape_ghost);
        assert_ne!(dark.tape_hole, dark.tape_ghost);
    }
}
