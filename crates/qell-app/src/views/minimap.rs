//! The minimap: the whole file in miniature beside the editor, like VS
//! Code's. Each character is a 1×2 point block in its syntax colour; a
//! slider shows the part on screen, and clicking or dragging scrolls.
//!
//! The picture is a texture rebuilt when the text (or the wrap width)
//! changes, at most a few times a second while typing.

use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Id, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use qell_core::{HighlightSpan, TokenKind};

use crate::document::Document;

/// Characters drawn per row; longer lines are cut.
const MAX_COLS: usize = 100;
/// Width of the minimap, in points (one per character, plus a margin).
pub const WIDTH: f32 = MAX_COLS as f32 + 8.0;
/// Height of one text row in the minimap.
const ROW_PX: f32 = 2.0;
/// Texture rows at most (GPU size limits); longer files are sampled.
const MAX_TEX_ROWS: usize = 6000;
/// Rebuild at most this often while the text keeps changing.
const REBUILD_EVERY: Duration = Duration::from_millis(150);

/// Per-document minimap state.
#[derive(Default)]
pub struct Minimap {
    tex: Option<TextureHandle>,
    /// (buffer version, wrap width, rows) the texture shows.
    key: Option<(i32, usize, usize)>,
    built: Option<Instant>,
    /// Editor scroll offset and viewport height, from the last frame.
    pub scroll_y: f32,
    pub view_h: f32,
    /// Drag in progress: (pointer y, scroll offset) when it started.
    drag_from: Option<(f32, f32)>,
}

/// Where things are in the minimap, in points from its top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// How far the miniature is scrolled (when taller than the minimap).
    pub offset: f32,
    pub slider_top: f32,
    pub slider_h: f32,
    /// Height of the whole miniature.
    pub total: f32,
    /// Largest editor scroll offset.
    pub max_scroll: f32,
}

/// Lay out the minimap for a file of `rows` rows of `row_height` points,
/// scrolled to `scroll_y` in a `view_h` editor, in a `height` tall minimap.
/// A miniature taller than the minimap scrolls along with the editor.
pub fn geometry(rows: usize, row_height: f32, scroll_y: f32, view_h: f32, height: f32) -> Geometry {
    let total = rows as f32 * ROW_PX;
    let max_scroll = (rows as f32 * row_height - view_h).max(0.0);
    let offset =
        if total > height && max_scroll > 0.0 { (scroll_y / max_scroll).clamp(0.0, 1.0) * (total - height) } else { 0.0 };
    let slider_h = (view_h / row_height * ROW_PX).max(6.0);
    Geometry { offset, slider_top: scroll_y / row_height * ROW_PX - offset, slider_h, total, max_scroll }
}

/// Clicks and drags on the minimap: the editor scroll offset to apply.
pub fn input(ui: &Ui, rect: Rect, id: Id, mm: &mut Minimap, rows: usize, row_height: f32) -> Option<f32> {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    let g = geometry(rows, row_height, mm.scroll_y, mm.view_h, rect.height());
    let pointer = resp.interact_pointer_pos()?;
    let py = pointer.y - rect.top();
    // A row of the miniature, centred in the editor.
    let center_on = |py: f32| {
        let row = (py + g.offset) / ROW_PX;
        (row * row_height - mm.view_h / 2.0).clamp(0.0, g.max_scroll)
    };
    if resp.drag_started() || resp.clicked() {
        let on_slider = py >= g.slider_top && py <= g.slider_top + g.slider_h;
        let start = if on_slider { mm.scroll_y } else { center_on(py) };
        mm.drag_from = resp.drag_started().then_some((pointer.y, start));
        return Some(start);
    }
    if resp.dragged() {
        let (y0, s0) = mm.drag_from?;
        let dy = pointer.y - y0;
        // Dragging the slider: the slider follows the pointer.
        let per_point =
            if g.total > rect.height() { g.max_scroll / (rect.height() - g.slider_h).max(1.0) } else { row_height / ROW_PX };
        return Some((s0 + dy * per_point).clamp(0.0, g.max_scroll));
    }
    if resp.drag_stopped() {
        mm.drag_from = None;
    }
    None
}

/// Rebuild the texture if the text changed (throttled while typing).
pub fn update(
    ctx: &egui::Context,
    doc: &mut Document,
    spans: &[HighlightSpan],
    color_of: &dyn Fn(TokenKind) -> Color32,
    text: Color32,
) {
    let key = (doc.buffer.version(), doc.wrap.width(), doc.wrap.row_count());
    let mm = &doc.minimap;
    if mm.key == Some(key) && mm.tex.is_some() {
        return;
    }
    if let Some(t) = mm.built.filter(|_| mm.tex.is_some()) {
        if t.elapsed() < REBUILD_EVERY {
            ctx.request_repaint_after(REBUILD_EVERY - t.elapsed());
            return;
        }
    }
    let image = image(doc, spans, color_of, text);
    let mm = &mut doc.minimap;
    match &mut mm.tex {
        Some(t) => t.set(image, TextureOptions::LINEAR),
        None => mm.tex = Some(ctx.load_texture("minimap", image, TextureOptions::LINEAR)),
    }
    mm.key = Some(key);
    mm.built = Some(Instant::now());
}

/// The miniature: one texel per character, two per row (the second fainter,
/// which separates the lines).
fn image(doc: &Document, spans: &[HighlightSpan], color_of: &dyn Fn(TokenKind) -> Color32, text: Color32) -> ColorImage {
    let wrap = &doc.wrap;
    let rows = wrap.row_count();
    let step = rows.div_ceil(MAX_TEX_ROWS).max(1);
    let tex_rows = rows.div_ceil(step).max(1);
    let mut img = ColorImage::filled([MAX_COLS, tex_rows * 2], Color32::TRANSPARENT);
    let rope = doc.buffer.rope();
    let mut colors: Vec<Option<Color32>> = Vec::new();
    for line in 0..doc.buffer.line_count() {
        let row_range = wrap.rows_of_line(line);
        // Skip lines none of whose rows are sampled.
        if !row_range.clone().any(|r| r % step == 0) {
            continue;
        }
        let text_of_line = doc.buffer.line(line);
        let line_start = rope.line_to_byte(line);
        // Colour of each char (None for whitespace).
        colors.clear();
        let mut si = spans.partition_point(|s| s.end <= line_start);
        for (b, ch) in text_of_line.char_indices() {
            if ch.is_whitespace() {
                colors.push(None);
                continue;
            }
            let at = line_start + b;
            while si < spans.len() && spans[si].end <= at {
                si += 1;
            }
            let c = spans.get(si).filter(|s| s.start <= at).map_or(text, |s| color_of(s.kind));
            colors.push(Some(c));
        }
        for r in row_range {
            if r % step != 0 {
                continue;
            }
            let row = wrap.row(r);
            let y = (r / step) * 2;
            let end = row.end.min(colors.len());
            for (i, c) in colors[row.start.min(end)..end].iter().enumerate() {
                let x = row.indent + i;
                if x >= MAX_COLS {
                    break;
                }
                if let Some(c) = c {
                    img[(x, y)] = c.gamma_multiply(0.85);
                    img[(x, y + 1)] = c.gamma_multiply(0.35);
                }
            }
        }
    }
    img
}

/// Draw the miniature and the slider.
pub fn paint(ui: &Ui, rect: Rect, mm: &Minimap, rows: usize, row_height: f32, cursor_row: usize, border: Color32) {
    let painter = ui.painter_at(rect);
    painter.vline(rect.left(), rect.y_range(), egui::Stroke::new(1.0_f32, border));
    let g = geometry(rows, row_height, mm.scroll_y, mm.view_h, rect.height());
    let left = rect.left() + 4.0;
    if let Some(tex) = &mm.tex {
        let shown = g.total.min(rect.height());
        if g.total > 0.0 {
            let uv = Rect::from_min_max(Pos2::new(0.0, g.offset / g.total), Pos2::new(1.0, (g.offset + shown) / g.total));
            let r = Rect::from_min_size(Pos2::new(left, rect.top()), Vec2::new(MAX_COLS as f32, shown));
            painter.image(tex.id(), r, uv, Color32::WHITE);
        }
    }
    // The cursor's row, as a thin line.
    let cy = rect.top() + cursor_row as f32 * ROW_PX - g.offset;
    if (rect.top()..rect.bottom()).contains(&cy) {
        painter.hline(rect.left() + 1.0..=rect.right(), cy + 0.5, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    }
    // The visible part.
    let hot = ui.rect_contains_pointer(rect) || mm.drag_from.is_some();
    let slider =
        Rect::from_min_size(Pos2::new(rect.left() + 1.0, rect.top() + g.slider_top), Vec2::new(rect.width() - 1.0, g.slider_h));
    painter.rect_filled(slider, 0.0, Color32::from_white_alpha(if hot { 26 } else { 14 }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_file_fits_and_slider_tracks_scroll() {
        // 100 rows of 20 pt, 400 pt editor: miniature 200 pt in a 600 pt minimap.
        let g = geometry(100, 20.0, 0.0, 400.0, 600.0);
        assert_eq!((g.offset, g.slider_top, g.slider_h, g.total), (0.0, 0.0, 40.0, 200.0));
        let g = geometry(100, 20.0, 1600.0, 400.0, 600.0);
        assert_eq!(g.slider_top, 160.0);
        assert_eq!(g.max_scroll, 1600.0);
    }

    #[test]
    fn long_file_scrolls_the_miniature() {
        // 1000 rows: miniature 2000 pt in a 500 pt minimap.
        let g = geometry(1000, 20.0, 0.0, 400.0, 500.0);
        assert_eq!(g.offset, 0.0);
        let end = geometry(1000, 20.0, 19600.0, 400.0, 500.0);
        assert_eq!(end.offset, 1500.0);
        // At the end, the slider sits at the bottom of the minimap.
        assert_eq!(end.slider_top + end.slider_h, 500.0);
        let mid = geometry(1000, 20.0, 9800.0, 400.0, 500.0);
        assert_eq!(mid.offset, 750.0);
    }
}
