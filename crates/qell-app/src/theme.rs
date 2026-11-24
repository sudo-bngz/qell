//! The look: a terminal's. One near-black surface, off-white text, white
//! for emphasis and a single warm tint; hairlines instead of panels, no
//! gradients, no rounded chrome, one monospace font throughout.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};
use qell_core::{Theme, TokenKind};
use qell_git::FileStatus;

/// Half-width of the panel resize handles (see `apply`).
pub const RESIZE_GRAB_RADIUS: f32 = 2.0;

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
    pub git: GitColors,
}

/// File-tree / tab colours for git states.
pub struct GitColors {
    pub modified: Color32,
    pub added: Color32,
    pub untracked: Color32,
    pub deleted: Color32,
    pub conflict: Color32,
    pub ignored: Color32,
}

fn c(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

impl Palette {
    pub fn dark() -> Self {
        let syntax = Theme::dark();
        // One surface: the editor, the panels and the bars are the same
        // near-black, told apart by a hairline and nothing else. The
        // accent is white — emphasis is brightness, not a colour.
        let bg = c(syntax.background);
        Self {
            editor_bg: bg,
            panel_bg: bg,
            chrome_bg: bg,
            border: Color32::from_rgb(0x2a, 0x26, 0x23),
            accent: Color32::WHITE,
            text: c(syntax.foreground),
            text_dim: Color32::from_rgb(0x8a, 0x83, 0x7c),
            selection: c(syntax.selection),
            current_line: c(syntax.current_line),
            gutter: c(syntax.gutter),
            cursor: c(syntax.cursor),
            error: Color32::from_rgb(0xe5, 0x6b, 0x5e),
            warning: Color32::from_rgb(0xd9, 0xb9, 0x8a),
            hover: Color32::from_rgb(0x1e, 0x1b, 0x19),
            // Git states keep a hue, since they are read at a glance, but
            // a quiet one.
            git: GitColors {
                modified: Color32::from_rgb(0xd9, 0xb9, 0x8a),
                added: Color32::from_rgb(0x9d, 0xb8, 0x82),
                untracked: Color32::from_rgb(0x9d, 0xb8, 0x82),
                deleted: Color32::from_rgb(0xc9, 0x6a, 0x5d),
                conflict: Color32::from_rgb(0xe5, 0x6b, 0x5e),
                ignored: Color32::from_rgb(0x6b, 0x65, 0x5f),
            },
            syntax,
        }
    }

    pub fn token(&self, kind: TokenKind) -> Color32 {
        c(self.syntax.color(kind))
    }

    pub fn git_color(&self, status: FileStatus) -> Color32 {
        let g = &self.git;
        match status {
            FileStatus::Modified => g.modified,
            FileStatus::Added | FileStatus::Renamed => g.added,
            FileStatus::Untracked => g.untracked,
            FileStatus::Deleted => g.deleted,
            FileStatus::Conflict => g.conflict,
            FileStatus::Ignored => g.ignored,
        }
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
        // What is selected is a lighter band with the text left as it is;
        // the accent is white and would swallow it.
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0_f32, self.accent);
        v.hyperlink_color = self.accent;
        v.override_text_color = Some(self.text);
        v.window_stroke = Stroke::new(1.0_f32, self.border);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.widgets.noninteractive.bg_fill = self.panel_bg;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text_dim);
        // Everything sits on one surface, so a control at rest needs a
        // shade of its own to be seen at all (a slider's rail, a box).
        v.widgets.inactive.bg_fill = self.border;
        v.widgets.inactive.weak_bg_fill = self.hover;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text);
        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.border);
        v.widgets.active.bg_fill = self.selection;
        v.widgets.active.weak_bg_fill = self.selection;
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
        // Interface text (tab titles, file names, status bar) is chrome, not
        // content: no text cursor on hover, no accidental selection.
        style.interaction.selectable_labels = false;
        // Panel resize handles straddle the border by this much on each
        // side. egui's 5 px reached into the terminal's padding, so a click
        // near its left edge (to focus it, or start a selection) grabbed the
        // file tree's handle and dragged the sidebar. 2 px each side keeps
        // the handle on the border, like VS Code's 4 px sash.
        style.interaction.resize_grab_radius_side = RESIZE_GRAB_RADIUS;
        ctx.set_style(style);
    }
}
