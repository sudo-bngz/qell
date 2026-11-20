//! The minimap: the whole file in miniature beside the editor, like VS
//! Code's. Each character is drawn as a tiny 2×4-texel version of its glyph
//! (shrunk from the editor's own font) in its syntax colour; a slider shows
//! the part on screen, and clicking or dragging scrolls.
//!
//! Only the rows the minimap shows are drawn, into a texture rebuilt when
//! they change (scrolling, editing). Long files are never squeezed into
//! one picture, so they stay sharp.

use std::sync::OnceLock;

use egui::{Color32, ColorImage, FontFamily, FontId, Id, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use qell_core::{HighlightSpan, TokenKind};

use crate::document::Document;

/// Characters drawn per row; longer lines are cut.
const MAX_COLS: usize = 100;
/// Width of the minimap, in points (one per character, plus a margin).
pub const WIDTH: f32 = MAX_COLS as f32 + 8.0;
/// Height of one text row in the minimap, in points.
const ROW_PX: f32 = 2.0;
/// Texels per character: 2 wide, 4 tall (one per pixel on a Retina screen,
/// which is what VS Code draws).
const CELL_W: usize = 2;
const CELL_H: usize = 4;

/// Per-document minimap state.
#[derive(Default)]
pub struct Minimap {
    tex: Option<TextureHandle>,
    /// (buffer version, wrap width, first row, row count) the texture shows.
    key: Option<(i32, usize, usize, usize)>,
    /// Editor scroll offset and viewport height, from the last frame.
    pub scroll_y: f32,
    pub view_h: f32,
    /// Drag in progress: (pointer y, scroll offset) when it started.
    drag_from: Option<(f32, f32)>,
}

// ---- tiny glyphs ---------------------------------------------------------------

/// Coverage (0–255) of each of the 2×4 cells of a character, row by row.
type Mask = [u8; CELL_W * CELL_H];

/// Masks of the printable ASCII characters (`!` to `~`), from the editor's
/// monospace font.
pub struct Glyphs {
    ascii: [Mask; 94],
}

/// For characters outside ASCII: a plain block in the middle of the cell.
const OTHER: Mask = [0, 0, 150, 150, 150, 150, 0, 0];

impl Glyphs {
    fn mask(&self, c: char) -> Option<&Mask> {
        match c {
            '!'..='~' => Some(&self.ascii[c as usize - '!' as usize]),
            c if c.is_whitespace() => None,
            _ => Some(&OTHER),
        }
    }

    /// Shrink each glyph of the monospace font to 2×4 cells: lay the
    /// characters out large, then average the atlas pixels falling in each
    /// cell.
    fn build(ctx: &egui::Context) -> Self {
        let font = FontId::new(32.0, FontFamily::Monospace);
        let chars: String = ('!'..='~').collect();
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(chars, font, Color32::WHITE));
        let atlas = ctx.fonts(|f| f.image());
        let mut ascii = [[0u8; CELL_W * CELL_H]; 94];
        let Some(row) = galley.rows.first() else { return Self { ascii } };
        let row_h = row.rect().height().max(1.0);
        let top = row.rect().top();
        for (i, g) in row.glyphs.iter().enumerate().take(94) {
            let uv = g.uv_rect;
            let (w, h) = ((uv.max[0] - uv.min[0]) as usize, (uv.max[1] - uv.min[1]) as usize);
            if w == 0 || h == 0 {
                continue;
            }
            // Atlas texels per point.
            let scale = w as f32 / uv.size.x.max(0.001);
            let mut sum = [0f32; CELL_W * CELL_H];
            let mut count = [0f32; CELL_W * CELL_H];
            // Every cell counts all its area (empty parts included).
            let cell_area = (g.advance_width / CELL_W as f32) * (row_h / CELL_H as f32) * scale * scale;
            for ty in 0..h {
                for tx in 0..w {
                    let a = atlas[(uv.min[0] as usize + tx, uv.min[1] as usize + ty)].a() as f32;
                    // Where this texel is, in points within the glyph's cell.
                    let x = uv.offset.x + tx as f32 / scale;
                    let y = g.pos.y + uv.offset.y + ty as f32 / scale - top;
                    let cx = ((x / g.advance_width) * CELL_W as f32).floor();
                    let cy = ((y / row_h) * CELL_H as f32).floor();
                    if (0.0..CELL_W as f32).contains(&cx) && (0.0..CELL_H as f32).contains(&cy) {
                        let k = cy as usize * CELL_W + cx as usize;
                        sum[k] += a;
                        count[k] += 1.0;
                    }
                }
            }
            for k in 0..CELL_W * CELL_H {
                let _ = count[k];
                // Thin strokes cover little of a cell: boost so they show.
                ascii[i][k] = (sum[k] / cell_area.max(1.0) * 2.2).min(255.0) as u8;
            }
        }
        Self { ascii }
    }
}

/// The glyph masks, built once from the font.
pub fn glyphs(ctx: &egui::Context) -> &'static Glyphs {
    static GLYPHS: OnceLock<Glyphs> = OnceLock::new();
    GLYPHS.get_or_init(|| Glyphs::build(ctx))
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

/// Draw the minimap: the visible rows of the miniature, the cursor's row
/// and the slider. The texture is rebuilt when those rows or the text change.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &Ui,
    rect: Rect,
    doc: &mut Document,
    spans: &[HighlightSpan],
    color_of: &dyn Fn(TokenKind) -> Color32,
    text: Color32,
    row_height: f32,
    border: Color32,
) {
    let rows = doc.wrap.row_count();
    let g = geometry(rows, row_height, doc.minimap.scroll_y, doc.minimap.view_h, rect.height());
    // The rows in view, and how far the first one sits above the top.
    let first = (g.offset / ROW_PX).floor() as usize;
    let count = ((rect.height() / ROW_PX).ceil() as usize + 1).min(rows.saturating_sub(first)).max(1);
    let key = (doc.buffer.version(), doc.wrap.width(), first, count);
    if doc.minimap.key != Some(key) || doc.minimap.tex.is_none() {
        let image = image(ui.ctx(), doc, spans, color_of, text, first, count);
        // One texel per pixel on a 2× (Retina) screen: sample exactly there,
        // smooth the scaling anywhere else.
        let ppp = ui.ctx().pixels_per_point();
        let filter = if (ppp - 2.0).abs() < 0.01 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
        let mm = &mut doc.minimap;
        match &mut mm.tex {
            Some(t) => t.set(image, filter),
            None => mm.tex = Some(ui.ctx().load_texture("minimap", image, filter)),
        }
        mm.key = Some(key);
    }

    let painter = ui.painter_at(rect);
    painter.vline(rect.left(), rect.y_range(), egui::Stroke::new(1.0_f32, border));
    let top = rect.top() - (g.offset - first as f32 * ROW_PX);
    if let Some(tex) = &doc.minimap.tex {
        // On whole pixels, so each texel lands on one.
        let ppp = ui.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let r =
            Rect::from_min_size(Pos2::new(snap(rect.left() + 4.0), snap(top)), Vec2::new(MAX_COLS as f32, count as f32 * ROW_PX));
        painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
    // The cursor's row, as a thin line.
    let cursor_row = doc.wrap.row_of(doc.buffer.cursor.head());
    let cy = rect.top() + cursor_row as f32 * ROW_PX - g.offset;
    if (rect.top()..rect.bottom()).contains(&cy) {
        painter.hline(rect.left() + 1.0..=rect.right(), cy + 0.5, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    }
    // The visible part.
    let hot = ui.rect_contains_pointer(rect) || doc.minimap.drag_from.is_some();
    let slider =
        Rect::from_min_size(Pos2::new(rect.left() + 1.0, rect.top() + g.slider_top), Vec2::new(rect.width() - 1.0, g.slider_h));
    painter.rect_filled(slider, 0.0, Color32::from_white_alpha(if hot { 26 } else { 14 }));
}

/// Rows `first..first + count` of the miniature: each character a 2×4
/// texel glyph in its syntax colour.
fn image(
    ctx: &egui::Context,
    doc: &Document,
    spans: &[HighlightSpan],
    color_of: &dyn Fn(TokenKind) -> Color32,
    text: Color32,
    first: usize,
    count: usize,
) -> ColorImage {
    let glyphs = glyphs(ctx);
    let wrap = &doc.wrap;
    let rope = doc.buffer.rope();
    let mut img = ColorImage::filled([MAX_COLS * CELL_W, count * CELL_H], Color32::TRANSPARENT);
    // Characters and colours of the line being drawn (rows of one line share it).
    let mut cached: Option<(usize, Vec<(char, Color32)>)> = None;
    for i in 0..count {
        let row = wrap.row(first + i);
        if first + i >= wrap.row_count() {
            break;
        }
        if cached.as_ref().is_none_or(|(l, _)| *l != row.line) {
            let line = doc.buffer.line(row.line);
            let start = rope.line_to_byte(row.line);
            let mut si = spans.partition_point(|s| s.end <= start);
            let chars = line
                .char_indices()
                .map(|(b, ch)| {
                    let at = start + b;
                    while si < spans.len() && spans[si].end <= at {
                        si += 1;
                    }
                    (ch, spans.get(si).filter(|s| s.start <= at).map_or(text, |s| color_of(s.kind)))
                })
                .collect();
            cached = Some((row.line, chars));
        }
        let chars = &cached.as_ref().expect("just set").1;
        let end = row.end.min(chars.len());
        for (j, &(ch, color)) in chars[row.start.min(end)..end].iter().enumerate() {
            let col = row.indent + j;
            if col >= MAX_COLS {
                break;
            }
            let Some(mask) = glyphs.mask(ch) else { continue };
            for cy in 0..CELL_H {
                for cx in 0..CELL_W {
                    let a = mask[cy * CELL_W + cx];
                    if a > 0 {
                        img[(col * CELL_W + cx, i * CELL_H + cy)] = color.gamma_multiply(a as f32 / 255.0);
                    }
                }
            }
        }
    }
    img
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
    fn glyphs_look_like_their_characters() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let g = Glyphs::build(&ctx);
        let m = |c| *g.mask(c).unwrap();
        let ink = |mask: Mask, row: usize| mask[row * CELL_W] as u32 + mask[row * CELL_W + 1] as u32;
        // '_' sits at the bottom, '^' and '\'' at the top, '.' low, '|' all the way.
        assert!(ink(m('_'), 3) > ink(m('_'), 0), "{:?}", m('_'));
        assert!(ink(m('^'), 0) + ink(m('^'), 1) > ink(m('^'), 3), "{:?}", m('^'));
        assert!(ink(m('.'), 2) + ink(m('.'), 3) > ink(m('.'), 0), "{:?}", m('.'));
        assert!((1..3).all(|r| ink(m('|'), r) > 0), "{:?}", m('|'));
        // Heavier letters have more ink than light punctuation.
        let total = |mask: Mask| mask.iter().map(|&a| a as u32).sum::<u32>();
        assert!(total(m('M')) > total(m('.')));
        assert!(g.mask(' ').is_none());
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
