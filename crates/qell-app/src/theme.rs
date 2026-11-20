//! The look. One dark theme in the spirit of VSCodium's "Dark Modern":
//! flat surfaces, one accent colour, no gradients, no rounded chrome.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};
use qell_core::{Theme, TokenKind};

pub struct Palette {
    pub syntax: Theme,
    pub editor_bg: Color32,
    pub panel_bg: Color32,
    pub chrome_bg: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub selection: Color32,
    pub current_line: Color32,
    pub gutter: Color32,
    pub cursor: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub hover: Color32,
}

fn c(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

impl Palette {
    pub fn dark() -> Self {
        let syntax = Theme::dark();
        Self {
            editor_bg: c(syntax.background),
            panel_bg: Color32::from_rgb(0x18, 0x18, 0x18),
            chrome_bg: Color32::from_rgb(0x18, 0x18, 0x18),
            border: Color32::from_rgb(0x2b, 0x2b, 0x2b),
            accent: Color32::from_rgb(0x00, 0x78, 0xd4),
            text: c(syntax.foreground),
            text_dim: Color32::from_rgb(0x9d, 0x9d, 0x9d),
            selection: c(syntax.selection),
            current_line: c(syntax.current_line),
            gutter: c(syntax.gutter),
            cursor: c(syntax.cursor),
            error: Color32::from_rgb(0xf1, 0x4c, 0x4c),
            warning: Color32::from_rgb(0xcc, 0xa7, 0x00),
            hover: Color32::from_rgb(0x2a, 0x2d, 0x2e),
            syntax,
        }
    }

    pub fn token(&self, kind: TokenKind) -> Color32 {
        c(self.syntax.color(kind))
    }

    pub fn mono(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }

    /// Apply to egui so built-in widgets (menus, buttons, scrollbars) match.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = Visuals::dark();
        v.panel_fill = self.panel_bg;
        v.window_fill = self.panel_bg;
        v.extreme_bg_color = self.editor_bg;
        v.faint_bg_color = self.hover;
        v.selection.bg_fill = self.accent;
        v.selection.stroke = Stroke::new(1.0_f32, self.text);
        v.hyperlink_color = self.accent;
        v.override_text_color = Some(self.text);
        v.window_stroke = Stroke::new(1.0_f32, self.border);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.widgets.noninteractive.bg_fill = self.panel_bg;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text_dim);
        v.widgets.inactive.bg_fill = self.panel_bg;
        v.widgets.inactive.weak_bg_fill = self.panel_bg;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text);
        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.border);
        v.widgets.active.bg_fill = self.accent;
        v.widgets.active.weak_bg_fill = self.accent;
        v.widgets.open.bg_fill = self.hover;
        v.widgets.open.weak_bg_fill = self.hover;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::ZERO;
            w.expansion = 0.0;
        }
        ctx.set_visuals(v);

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.window_margin = egui::Margin::same(6);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        ctx.set_style(style);
    }
}
