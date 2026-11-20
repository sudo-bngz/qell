//! Find and replace inside the open file: the bar that floats over the
//! top-right of the editor (⌘F / ⌘H), as in VS Code.
//!
//! Matches are found on the buffer's text and kept as positions, so the
//! editor can highlight them and jump between them.

use egui::{Color32, Key, Modifiers, Pos2, Rect, Vec2};
use qell_core::Position;

use crate::app::{Focus, QellApp};
use crate::commands::Command;
use crate::icons;

/// The colours the bar draws with.
struct Colors {
    text: Color32,
    dim: Color32,
    accent: Color32,
    error: Color32,
}

/// Where one text row is drawn, for the match highlights.
pub struct RowGeom {
    pub line: usize,
    pub y: f32,
    pub height: f32,
    /// Characters on the line, so a match can't run past its end.
    pub line_len: usize,
}

/// Which box the keyboard should go to next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Query,
    Replacement,
}

#[derive(Default)]
pub struct FindState {
    pub open: bool,
    /// The replace row is shown.
    pub replacing: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// One of the boxes has the keyboard.
    pub typing: bool,
    focus: Option<Field>,
    /// Match ranges in the current buffer, in order.
    pub matches: Vec<(Position, Position)>,
    /// Which match is highlighted, when there are any.
    pub current: usize,
    /// (buffer version, query, options) the matches were found for.
    key: Option<(i32, String, bool, bool, bool)>,
    /// A broken regex, shown in the bar.
    pub error: Option<String>,
}

/// Files above this size aren't searched while typing.
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_MATCHES: usize = 10_000;

/// Byte ranges of every match of `query` in `text`.
pub fn find_matches(text: &str, query: &str, case: bool, word: bool, regex: bool) -> Result<Vec<(usize, usize)>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = if regex { query.to_string() } else { regex::escape(query) };
    let pattern = if word { format!(r"\b(?:{pattern})\b") } else { pattern };
    let re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case)
        .build()
        .map_err(|e| e.to_string().lines().last().unwrap_or("invalid pattern").trim().to_string())?;
    Ok(re.find_iter(text).filter(|m| !m.is_empty()).take(MAX_MATCHES).map(|m| (m.start(), m.end())).collect())
}

impl FindState {
    /// Open the bar, seeded with `selection` when it is one line.
    pub fn start(&mut self, selection: Option<String>, replacing: bool) {
        if let Some(t) = selection.filter(|t| !t.is_empty() && !t.contains('\n')) {
            self.query = t;
            self.key = None;
        }
        self.open = true;
        self.replacing |= replacing;
        self.focus = Some(if replacing && !self.query.is_empty() { Field::Replacement } else { Field::Query });
    }

    pub fn close(&mut self) {
        self.open = false;
        self.typing = false;
        self.focus = None;
        self.matches.clear();
        self.key = None;
    }

    /// Match ranges on one line, as char columns.
    pub fn on_line(&self, line: usize) -> impl Iterator<Item = (usize, usize, bool)> + '_ {
        let first = self.matches.partition_point(|(_, e)| e.line < line);
        self.matches[first..].iter().take_while(move |(s, _)| s.line <= line).enumerate().map(move |(i, (s, e))| {
            let c0 = if s.line < line { 0 } else { s.col };
            let c1 = if e.line > line { usize::MAX } else { e.col };
            (c0, c1, first + i == self.current)
        })
    }
}

/// Recompute the matches when the text, the query or the options changed.
pub fn update(app: &mut QellApp) {
    if !app.find.open {
        return;
    }
    let Some(doc) = app.docs.get(app.active) else { return };
    let f = &app.find;
    let key = (doc.buffer.version(), f.query.clone(), f.case_sensitive, f.whole_word, f.regex);
    if f.key.as_ref() == Some(&key) {
        return;
    }
    let text = doc.buffer.text();
    let found = if text.len() > MAX_BYTES && app.find.query.len() < 3 {
        Ok(Vec::new())
    } else {
        find_matches(&text, &key.1, key.2, key.3, key.4)
    };
    let rope = doc.buffer.rope();
    let f = &mut app.find;
    f.key = Some(key);
    match found {
        Ok(ranges) => {
            f.error = None;
            f.matches = ranges
                .into_iter()
                .map(|(s, e)| {
                    let (s, e) = (rope.byte_to_char(s), rope.byte_to_char(e));
                    (doc.buffer.char_to_pos(s), doc.buffer.char_to_pos(e))
                })
                .collect();
        }
        Err(e) => {
            f.error = Some(e);
            f.matches.clear();
        }
    }
    f.current = f.current.min(f.matches.len().saturating_sub(1));
}

/// Select the match after (or before) the cursor and scroll to it.
pub fn go(app: &mut QellApp, forward: bool) {
    update(app);
    if app.find.matches.is_empty() {
        return;
    }
    let Some(doc) = app.docs.get_mut(app.active) else { return };
    let (anchor, head) = (doc.buffer.cursor.selection.anchor, doc.buffer.cursor.head());
    let from = anchor.min(head);
    let f = &mut app.find;
    // Stay on the current match when the cursor is still on it.
    let on_current = f.matches.get(f.current).is_some_and(|(s, e)| *s == from && *e == head.max(anchor));
    f.current = if forward {
        match f.matches.iter().position(|(s, _)| *s > from || (on_current && *s == from)) {
            Some(i) if !on_current || f.matches[i].0 > from => i,
            _ => f.matches.iter().position(|(s, _)| *s > from).unwrap_or(0),
        }
    } else {
        f.matches.iter().rposition(|(s, _)| *s < from).unwrap_or(f.matches.len() - 1)
    };
    select_current(app);
}

fn select_current(app: &mut QellApp) {
    let Some(&(s, e)) = app.find.matches.get(app.find.current) else { return };
    let Some(doc) = app.docs.get_mut(app.active) else { return };
    doc.buffer.set_cursor(s, false);
    doc.buffer.set_cursor(e, true);
    doc.scroll_to_cursor = true;
    if app.config.vim_mode {
        app.vim.sync(&mut doc.buffer);
    }
}

/// What a match is replaced with (`$1` and `$name` in regex mode).
fn replacement_for(app: &QellApp, text: &str) -> String {
    let f = &app.find;
    if !f.regex {
        return f.replacement.clone();
    }
    let pattern = if f.whole_word { format!(r"\b(?:{})\b", f.query) } else { f.query.clone() };
    match regex::RegexBuilder::new(&pattern).case_insensitive(!f.case_sensitive).build() {
        Ok(re) => re.replace(text, f.replacement.as_str()).into_owned(),
        Err(_) => f.replacement.clone(),
    }
}

/// Replace the match the cursor is on, then move to the next one.
pub fn replace_current(app: &mut QellApp) {
    update(app);
    let Some(doc) = app.docs.get(app.active) else { return };
    if doc.read_only {
        return;
    }
    let (anchor, head) = (doc.buffer.cursor.selection.anchor, doc.buffer.cursor.head());
    let (from, to) = (anchor.min(head), anchor.max(head));
    // Not on a match yet: go to one first, as VS Code does.
    let Some(i) = app.find.matches.iter().position(|(s, e)| *s == from && *e == to) else {
        go(app, true);
        return;
    };
    let old = {
        let doc = &app.docs[app.active];
        let (s, e) = (doc.buffer.pos_to_char(from), doc.buffer.pos_to_char(to));
        doc.buffer.rope().slice(s..e).to_string()
    };
    let new = replacement_for(app, &old);
    app.find.current = i;
    let doc = &mut app.docs[app.active];
    doc.buffer.replace_range(from, to, &new);
    doc.scroll_to_cursor = true;
    update(app);
    // The next match is now at this index (this one is gone).
    app.find.current = i.min(app.find.matches.len().saturating_sub(1));
    select_current(app);
}

pub fn replace_all(app: &mut QellApp) {
    update(app);
    let Some(doc) = app.docs.get(app.active) else { return };
    if doc.read_only || app.find.matches.is_empty() {
        return;
    }
    let ranges = app.find.matches.clone();
    let count = ranges.len();
    let texts: Vec<String> = {
        let buf = &app.docs[app.active].buffer;
        ranges.iter().map(|(s, e)| buf.rope().slice(buf.pos_to_char(*s)..buf.pos_to_char(*e)).to_string()).collect()
    };
    let news: Vec<String> = texts.iter().map(|t| replacement_for(app, t)).collect();
    let doc = &mut app.docs[app.active];
    doc.buffer.begin_undo_group();
    // Back to front, so the earlier positions stay valid.
    for ((s, e), new) in ranges.iter().zip(news.iter()).rev() {
        doc.buffer.replace_range(*s, *e, new);
    }
    doc.buffer.end_undo_group();
    doc.scroll_to_cursor = true;
    app.notify(format!("Replaced {count} {}", if count == 1 { "occurrence" } else { "occurrences" }));
    update(app);
}

/// Draw the bar over the top-right of the editor, inside `area`.
pub fn show(app: &mut QellApp, ui: &mut egui::Ui, area: Rect) {
    if !app.find.open {
        app.find.typing = false;
        return;
    }
    update(app);
    let p = &app.palette;
    let (bg, border) = (p.chrome_bg, p.border);
    let colors = Colors { text: p.text, dim: p.text_dim, accent: p.accent, error: p.error };
    let dim = colors.dim;
    let width = 460.0_f32.min(area.width() - 24.0);
    let pos = Pos2::new(area.right() - width - 14.0, area.top() + 6.0);
    let mut typing = false;
    let mut close = false;
    let mut action: Option<fn(&mut QellApp)> = None;
    let mut go_next: Option<bool> = None;

    egui::Area::new(ui.id().with("find_widget")).order(egui::Order::Foreground).fixed_pos(pos).constrain_to(area).show(
        ui.ctx(),
        |ui| {
            egui::Frame::new()
                .fill(bg)
                .stroke(egui::Stroke::new(1.0_f32, border))
                .inner_margin(egui::Margin::symmetric(8, 5))
                .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(60) })
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                    // Fold the replace row open or shut, like VS Code's chevron.
                    ui.horizontal(|ui| {
                        let chevron = if app.find.replacing { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                        let rows_h = if app.find.replacing { 46.0 } else { 22.0 };
                        let btn = egui::Button::new(egui::RichText::new(chevron.to_string()).font(icons::font(13.0)).color(dim))
                            .frame(false)
                            .min_size(Vec2::new(16.0, rows_h));
                        if ui.add(btn).on_hover_text("Toggle Replace").clicked() {
                            app.find.replacing = !app.find.replacing;
                        }
                        ui.vertical(|ui| {
                            typing |= query_row(app, ui, &colors, &mut go_next, &mut close);
                            if app.find.replacing {
                                typing |= replace_row(app, ui, &colors, &mut action);
                            }
                        });
                    });
                });
        },
    );

    app.find.typing = typing;
    if let Some(forward) = go_next {
        go(app, forward);
    }
    if let Some(f) = action {
        f(app);
    }
    if close {
        app.find.close();
        app.focus = Focus::Editor;
    }
}

/// The query box, its toggles, the match count and the arrows.
fn query_row(app: &mut QellApp, ui: &mut egui::Ui, c: &Colors, go_next: &mut Option<bool>, close: &mut bool) -> bool {
    let (text, dim, accent, error_color) = (c.text, c.dim, c.accent, c.error);
    let mut typing = false;
    ui.horizontal(|ui| {
        // Right to left: the buttons keep their size, the box takes the rest.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let icon_btn = |ui: &mut egui::Ui, icon: char, tip: &str| {
                ui.add(
                    egui::Button::new(egui::RichText::new(icon.to_string()).font(icons::font(13.0)).color(dim))
                        .frame(false)
                        .min_size(Vec2::new(20.0, 20.0)),
                )
                .on_hover_text(tip.to_string())
                .clicked()
            };
            if icon_btn(ui, icons::CLOSE, "Close (Escape)") {
                *close = true;
            }
            if arrow_button(ui, false, dim, "Next match (Enter)") {
                *go_next = Some(true);
            }
            if arrow_button(ui, true, dim, "Previous match (Shift+Enter)") {
                *go_next = Some(false);
            }
            // How many matches, or why there are none.
            let (label, color) = match (&app.find.error, app.find.matches.len()) {
                (Some(e), _) => (e.clone(), error_color),
                (None, 0) if app.find.query.is_empty() => (String::new(), dim),
                (None, 0) => ("No results".into(), dim),
                (None, n) => (format!("{} of {n}", app.find.current + 1), dim),
            };
            ui.label(egui::RichText::new(label).size(11.0).color(color));
            ui.add_space(2.0);

            let toggle = |ui: &mut egui::Ui, on: &mut bool, icon: char, tip: &str| {
                let color = if *on { accent } else { dim };
                let btn = egui::Button::new(egui::RichText::new(icon.to_string()).font(icons::font(13.0)).color(color))
                    .frame(*on)
                    .min_size(Vec2::new(20.0, 20.0));
                if ui.add(btn).on_hover_text(tip.to_string()).clicked() {
                    *on = !*on;
                }
            };
            toggle(ui, &mut app.find.regex, icons::REGEX, "Use Regular Expression");
            toggle(ui, &mut app.find.whole_word, icons::WHOLE_WORD, "Match Whole Word");
            toggle(ui, &mut app.find.case_sensitive, icons::CASE_SENSITIVE, "Match Case");

            // The box takes what's left.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let id = ui.id().with("find_query");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.find.query)
                        .id(id)
                        .hint_text("Find")
                        .text_color(text)
                        .desired_width(f32::INFINITY),
                );
                if app.find.focus == Some(Field::Query) {
                    app.find.focus = None;
                    resp.request_focus();
                    select_all_in(ui, id);
                }
                typing = resp.has_focus();
                // Plain Enter takes the focus away from a single-line box;
                // ⇧Enter doesn't, so take that key before the box sees it.
                if typing && ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Enter)) {
                    *go_next = Some(false);
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    *go_next = Some(!ui.input(|i| i.modifiers.shift));
                    resp.request_focus();
                    typing = true;
                }
            });
        });
    });
    typing
}

/// The replacement box and its two buttons.
fn replace_row(app: &mut QellApp, ui: &mut egui::Ui, c: &Colors, action: &mut Option<fn(&mut QellApp)>) -> bool {
    let (text, dim) = (c.text, c.dim);
    let mut typing = false;
    let read_only = app.docs.get(app.active).is_some_and(|d| d.read_only);
    let any = !app.find.matches.is_empty() && !read_only;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = |ui: &mut egui::Ui, label: &str, tip: &str, on: bool| {
                ui.add_enabled(
                    on,
                    egui::Button::new(egui::RichText::new(label).size(11.0).color(dim)).min_size(Vec2::new(22.0, 20.0)),
                )
                .on_hover_text(tip.to_string())
                .clicked()
            };
            if btn(ui, "all", "Replace All", any) {
                *action = Some(replace_all);
            }
            if btn(ui, "one", "Replace", any) {
                *action = Some(replace_current);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let id = ui.id().with("find_replacement");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.find.replacement)
                        .id(id)
                        .hint_text("Replace")
                        .text_color(text)
                        .desired_width(f32::INFINITY),
                );
                if app.find.focus == Some(Field::Replacement) {
                    app.find.focus = None;
                    resp.request_focus();
                    select_all_in(ui, id);
                }
                typing = resp.has_focus();
                // Enter replaces this match, ⌘/Ctrl+Enter replaces them all.
                if typing && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
                    *action = Some(replace_all);
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    *action = Some(replace_current);
                    resp.request_focus();
                    typing = true;
                }
            });
        });
    });
    typing
}

/// A small up or down triangle. Drawn rather than typed: the bundled icon
/// font has no arrows.
fn arrow_button(ui: &mut egui::Ui, up: bool, color: Color32, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip.to_string());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, Color32::from_white_alpha(14));
    }
    let c = rect.center();
    let (w, h) = (4.5, 3.0);
    let tip_y = if up { c.y - h } else { c.y + h };
    let base_y = if up { c.y + h } else { c.y - h };
    let points = vec![Pos2::new(c.x, tip_y), Pos2::new(c.x - w, base_y), Pos2::new(c.x + w, base_y)];
    ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
    resp.clicked()
}

/// Select a text box's contents, so typing replaces the seeded query.
fn select_all_in(ui: &egui::Ui, id: egui::Id) {
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(usize::MAX),
        )));
        state.store(ui.ctx(), id);
    }
}

/// Shortcuts that work while the bar has the keyboard.
pub fn handle_keys(app: &mut QellApp, ctx: &egui::Context) {
    if !app.find.open {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        app.find.close();
        app.focus = Focus::Editor;
        return;
    }
    for (cmd, forward) in [(Command::FindNext, true), (Command::FindPrevious, false)] {
        if cmd.shortcut().is_some_and(|sc| ctx.input_mut(|i| i.consume_shortcut(&sc))) {
            go(app, forward);
        }
    }
}

/// Paint the match highlights of one row; `x` maps a column to a position.
pub fn paint_matches(
    painter: &egui::Painter,
    find: &FindState,
    row: &RowGeom,
    clip: impl Fn(usize, usize) -> Option<(usize, usize)>,
    x: impl Fn(usize) -> f32,
) {
    for (c0, c1, current) in find.on_line(row.line) {
        let Some((c0, c1)) = clip(c0, c1.min(row.line_len)) else { continue };
        let rect = Rect::from_min_max(Pos2::new(x(c0), row.y), Pos2::new(x(c1.max(c0 + 1)), row.y + row.height));
        let (fill, stroke) = if current {
            (Color32::from_rgba_unmultiplied(0x9e, 0x6a, 0x03, 0xcc), Color32::from_rgb(0xf2, 0xcc, 0x60))
        } else {
            (Color32::from_rgba_unmultiplied(0x62, 0x4a, 0x00, 0x99), Color32::TRANSPARENT)
        };
        painter.rect_filled(rect, 1.0, fill);
        if stroke != Color32::TRANSPARENT {
            painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(text: &'a str, q: &str, case: bool, word: bool, regex: bool) -> Vec<&'a str> {
        find_matches(text, q, case, word, regex).unwrap().into_iter().map(|(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn plain_and_case() {
        let t = "Foo foo FOO bar";
        assert_eq!(find(t, "foo", false, false, false).len(), 3);
        assert_eq!(find(t, "foo", true, false, false), vec!["foo"]);
        assert_eq!(find(t, "", false, false, false).len(), 0);
        // Not a regex unless asked: the dot is literal.
        assert_eq!(find("a.c abc", "a.c", false, false, false), vec!["a.c"]);
        assert_eq!(find("a.c abc", "a.c", false, false, true), vec!["a.c", "abc"]);
    }

    #[test]
    fn whole_word_and_groups() {
        let t = "bar barn embark bar";
        assert_eq!(find(t, "bar", false, true, false), vec!["bar", "bar"]);
        assert_eq!(find("x1 x22 y3", r"[xy]\d+", false, false, true), vec!["x1", "x22", "y3"]);
        assert!(find_matches("a", "(", false, false, true).is_err());
        // An empty match (`^`) is skipped rather than looping.
        assert_eq!(find("a\nb", "^", false, false, true).len(), 0);
    }

    #[test]
    fn lines_of_a_multiline_match() {
        let mut f = FindState { query: "x".into(), ..Default::default() };
        f.matches = vec![(Position::new(1, 2), Position::new(3, 4)), (Position::new(5, 0), Position::new(5, 2))];
        f.current = 1;
        assert_eq!(f.on_line(0).count(), 0);
        assert_eq!(f.on_line(1).collect::<Vec<_>>(), vec![(2, usize::MAX, false)]);
        assert_eq!(f.on_line(2).collect::<Vec<_>>(), vec![(0, usize::MAX, false)]);
        assert_eq!(f.on_line(3).collect::<Vec<_>>(), vec![(0, 4, false)]);
        assert_eq!(f.on_line(5).collect::<Vec<_>>(), vec![(0, 2, true)]);
    }
}
