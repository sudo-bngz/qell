//! The Search view (Ctrl+Shift+F / ⌘⇧F): search the whole workspace, from
//! the in-memory index, as you type.
//!
//! The same panel shows "Definitions of X" and "References to X" lists
//! when go to definition / find references has several answers.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CursorIcon, FontId, RichText};
use qell_index::{Query, SearchResults};

use crate::app::QellApp;
use crate::icons;

/// A place in a file, for result lists.
#[derive(Debug, Clone)]
pub struct ListItem {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub line: usize,
    pub col: usize,
    pub end_col: usize,
    pub preview: String,
    /// The highlighted part of `preview` (char range).
    pub preview_range: (usize, usize),
}

/// A titled list of places ("Definitions of make_point").
pub struct LocationList {
    pub title: String,
    pub items: Vec<ListItem>,
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    /// What to put in place of the matches, with its box shown or hidden.
    pub replace: String,
    pub show_replace: bool,
    pub focus_replace: bool,
    /// A replacement waiting to be confirmed: matches and files.
    pub confirm: Option<(usize, usize)>,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// The search box has the keyboard.
    pub typing: bool,
    pub focus_input: bool,
    /// When the query last changed (searches start after a short pause).
    changed_at: Option<Instant>,
    /// The search last started and its id.
    sent: Option<(u64, Query)>,
    pub results: Option<Result<SearchResults, String>>,
    collapsed: HashSet<PathBuf>,
    /// Index status the results were computed with; a change re-runs them.
    seen_status: qell_index::Status,
    /// Definitions / references, shown instead of search results.
    pub list: Option<LocationList>,
}

/// How long typing must pause before a search starts.
const DEBOUNCE: Duration = Duration::from_millis(120);
const ROW: f32 = 22.0;

impl SearchState {
    pub fn query(&self) -> Query {
        Query {
            pattern: self.query.clone(),
            is_regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        }
    }

    /// Run the search again, after the files changed under it.
    pub fn search_again(&mut self) {
        self.sent = None;
        self.changed_at = Some(Instant::now());
    }

    /// Open the view with the replace box showing.
    pub fn start_replace(&mut self, text: Option<String>) {
        self.start(text);
        self.show_replace = true;
    }

    /// Open the view on `text` (the selection, typically).
    pub fn start(&mut self, text: Option<String>) {
        if let Some(t) = text.filter(|t| !t.is_empty() && !t.contains('\n')) {
            self.query = t;
            self.changed_at = Some(Instant::now() - DEBOUNCE);
        }
        self.list = None;
        self.focus_input = true;
    }
}

/// Start, re-run or take finished searches. Called every frame.
pub fn update(app: &mut QellApp, ctx: &egui::Context) {
    for (id, _, result) in app.searcher.poll() {
        if app.search.sent.as_ref().is_some_and(|(s, _)| *s == id) {
            app.search.results = Some(result);
        }
    }
    let s = &mut app.search;
    let status = app.indexer.status();
    // Files changed or the index finished: the results may have too.
    let index_changed = status != s.seen_status && !status.indexing;
    if index_changed {
        s.seen_status = status;
    }
    if s.query.is_empty() {
        s.sent = None;
        s.results = None;
        return;
    }
    let due = s.changed_at.is_some_and(|t| t.elapsed() >= DEBOUNCE);
    let differs = s.sent.as_ref().is_none_or(|(_, q)| *q != s.query());
    if (due && differs) || (index_changed && s.sent.is_some()) {
        s.changed_at = None;
        let q = s.query();
        let id = app.searcher.search(q.clone());
        s.sent = Some((id, q));
    } else if let Some(t) = s.changed_at {
        ctx.request_repaint_after(DEBOUNCE.saturating_sub(t.elapsed()));
    }
}

/// One row of the virtualised list.
enum Row<'a> {
    File { path: &'a PathBuf, rel: &'a PathBuf, count: usize, collapsed: bool },
    Match { item: ListItem },
}

fn toggle(ui: &mut egui::Ui, on: &mut bool, icon: char, tip: &str, accent: Color32, dim: Color32) -> bool {
    let color = if *on { Color32::WHITE } else { dim };
    let b = egui::Button::new(RichText::new(icon.to_string()).font(icons::font(14.0)).color(color))
        .fill(if *on { accent.gamma_multiply(0.6) } else { Color32::TRANSPARENT })
        .min_size(egui::vec2(22.0, 22.0));
    let clicked = ui.add(b).on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand).clicked();
    if clicked {
        *on = !*on;
    }
    clicked
}

pub fn show_contents(app: &mut QellApp, ui: &mut egui::Ui) {
    let dim = app.palette.text_dim;
    let text = app.palette.text;
    let accent = app.palette.accent;
    let status = app.indexer.status();
    app.search.typing = false;

    let mut toggle_replace = false;
    ui.horizontal(|ui| {
        let chevron = if app.search.show_replace { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
        let b = egui::Button::new(RichText::new(chevron.to_string()).font(icons::font(12.0)).color(dim))
            .frame(false)
            .min_size(egui::vec2(14.0, 14.0));
        toggle_replace = ui.add(b).on_hover_text("Toggle Replace").on_hover_cursor(CursorIcon::PointingHand).clicked();
        ui.label(RichText::new("SEARCH").size(11.0).color(dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if status.indexing && status.files == 0 {
                "indexing…".to_string()
            } else if status.indexing {
                format!("{} files · finding definitions…", status.files)
            } else {
                format!("{} files", status.files)
            };
            ui.label(RichText::new(label).size(11.0).color(dim)).on_hover_text(format!(
                "Indexed in memory: {} files, {} definitions.\nUpdated on save and when files change.",
                status.files, status.symbols
            ));
            if status.indexing {
                ui.add(egui::Spinner::new().size(10.0));
            }
        });
    });
    ui.add_space(4.0);

    // Query box and its toggles. The toggles are laid out first, from the
    // right, and the box takes what is left: sizing the box from a guess
    // of the buttons' width made the panel grow a little every frame.
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let s = &mut app.search;
            changed |= toggle(ui, &mut s.regex, icons::REGEX, "Use Regular Expression", accent, dim);
            changed |= toggle(ui, &mut s.whole_word, icons::WHOLE_WORD, "Match Whole Word", accent, dim);
            changed |= toggle(ui, &mut s.case_sensitive, icons::CASE_SENSITIVE, "Match Case", accent, dim);
            let resp = egui::TextEdit::singleline(&mut app.search.query)
                .id(egui::Id::new("qell-search-input"))
                .hint_text("Search")
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(6.0, 4.0))
                .show(ui)
                .response;
            if app.search.focus_input {
                resp.request_focus();
                app.search.focus_input = false;
            }
            if resp.has_focus() {
                app.search.typing = true;
            }
            if resp.changed() {
                changed = true;
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                // Enter: search now.
                app.search.changed_at = Some(Instant::now() - DEBOUNCE);
                app.search.sent = None;
            }
        });
    });
    if toggle_replace {
        app.search.show_replace = !app.search.show_replace;
        app.search.focus_replace = app.search.show_replace;
        app.search.confirm = None;
    }
    if app.search.show_replace {
        ui.add_space(2.0);
        replace_row(app, ui, dim, accent);
    }
    if changed {
        app.search.changed_at = Some(Instant::now());
        app.search.list = None;
        app.search.confirm = None;
    }
    ui.add_space(6.0);

    // A definitions / references list replaces the search results.
    if let Some(list) = &app.search.list {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&list.title).size(12.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b =
                    egui::Button::new(RichText::new(icons::CLOSE.to_string()).font(icons::font(13.0)).color(dim)).frame(false);
                close = ui.add(b).on_hover_text("Back to search").clicked();
            });
        });
        if close {
            app.search.list = None;
            return;
        }
        let items: Vec<ListItem> = list.items.clone();
        let rows = group(items.into_iter().map(|i| (i.path.clone(), i.rel.clone(), i)), &app.search.collapsed);
        show_rows(app, ui, rows, text, dim);
        return;
    }

    match &app.search.results {
        None if app.search.query.is_empty() => {
            ui.label(RichText::new("Type to search all files of the workspace.").size(12.0).color(dim));
        }
        None => {
            ui.add(egui::Spinner::new().size(12.0));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).size(12.0).color(app.palette.error));
        }
        Some(Ok(r)) => {
            let summary = if r.total == 0 {
                "No results.".to_string()
            } else {
                format!(
                    "{} result{} in {} file{}{}",
                    r.total,
                    if r.total == 1 { "" } else { "s" },
                    r.files.len(),
                    if r.files.len() == 1 { "" } else { "s" },
                    if r.truncated { " (showing the first ones)" } else { "" }
                )
            };
            ui.label(RichText::new(summary).size(11.5).color(dim));
            ui.add_space(2.0);
            let items: Vec<(PathBuf, PathBuf, ListItem)> = r
                .files
                .iter()
                .flat_map(|f| {
                    f.matches.iter().map(|m| {
                        let item = ListItem {
                            path: f.path.clone(),
                            rel: f.rel.clone(),
                            line: m.line,
                            col: m.start,
                            end_col: m.end,
                            preview: m.preview.clone(),
                            preview_range: m.preview_range,
                        };
                        (f.path.clone(), f.rel.clone(), item)
                    })
                })
                .collect();
            let rows = group(items.into_iter(), &app.search.collapsed);
            show_rows(app, ui, rows, text, dim);
        }
    }
}

/// The replace box, its button, and the confirmation it asks for.
fn replace_row(app: &mut QellApp, ui: &mut egui::Ui, dim: Color32, accent: Color32) {
    let (total, files) = match &app.search.results {
        Some(Ok(r)) => (r.total, r.files.len()),
        _ => (0, 0),
    };
    let mut ask = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can = total > 0 && !app.search.query.is_empty();
            let b = egui::Button::new(RichText::new("Replace All").size(11.5))
                .fill(if can { accent.gamma_multiply(0.6) } else { Color32::TRANSPARENT })
                .min_size(egui::vec2(22.0, 22.0));
            let b = ui.add_enabled(can, b).on_hover_text("Replace every match in every file");
            ask = b.on_hover_cursor(CursorIcon::PointingHand).clicked();
            let resp = egui::TextEdit::singleline(&mut app.search.replace)
                .id(egui::Id::new("qell-replace-input"))
                .hint_text("Replace")
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(6.0, 4.0))
                .show(ui)
                .response;
            if app.search.focus_replace {
                resp.request_focus();
                app.search.focus_replace = false;
            }
            if resp.has_focus() {
                app.search.typing = true;
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && can {
                ask = true;
            }
        });
    });
    if ask {
        app.search.confirm = Some((total, files));
    }
    // Replacing writes to files, so it is asked for twice.
    let Some((matches, in_files)) = app.search.confirm else { return };
    ui.add_space(2.0);
    let what = if app.search.replace.is_empty() { "Delete".to_string() } else { "Replace".to_string() };
    let plural = |n: usize, s: &str| if n == 1 { format!("1 {s}") } else { format!("{n} {s}s") };
    ui.label(
        RichText::new(format!("{what} {} in {}?", plural(matches, "match"), plural(in_files, "file"))).size(11.5).color(dim),
    );
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(RichText::new(what).size(11.5)).fill(accent.gamma_multiply(0.6))).clicked() {
            app.search.confirm = None;
            app.replace_in_files();
        }
        if ui.add(egui::Button::new(RichText::new("Cancel").size(11.5)).frame(false)).clicked() {
            app.search.confirm = None;
        }
    });
}

/// Flatten items (already sorted by file) into file headers + matches.
fn group(
    items: impl Iterator<Item = (PathBuf, PathBuf, ListItem)>,
    collapsed: &HashSet<PathBuf>,
) -> Vec<(PathBuf, PathBuf, Vec<ListItem>, bool)> {
    let mut out: Vec<(PathBuf, PathBuf, Vec<ListItem>, bool)> = Vec::new();
    for (path, rel, item) in items {
        match out.last_mut() {
            Some((p, _, list, _)) if *p == path => list.push(item),
            _ => {
                let c = collapsed.contains(&path);
                out.push((path, rel, vec![item], c));
            }
        }
    }
    out
}

fn show_rows(
    app: &mut QellApp,
    ui: &mut egui::Ui,
    groups: Vec<(PathBuf, PathBuf, Vec<ListItem>, bool)>,
    text: Color32,
    dim: Color32,
) {
    let mut rows: Vec<Row> = Vec::new();
    for (path, rel, items, collapsed) in &groups {
        rows.push(Row::File { path, rel, count: items.len(), collapsed: *collapsed });
        if !collapsed {
            rows.extend(items.iter().map(|i| Row::Match { item: i.clone() }));
        }
    }
    let hover_bg = app.palette.hover;
    let match_bg = Color32::from_rgba_unmultiplied(0xea, 0x5c, 0x00, 0x55); // VS Code's find match
    let mut toggle_file = None;
    let mut open = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in &rows[range] {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
            let p = ui.painter_at(rect);
            if resp.hovered() {
                p.rect_filled(rect, 0.0, hover_bg);
            }
            match row {
                Row::File { path, rel, count, collapsed } => {
                    let chevron = if *collapsed { icons::CHEVRON_RIGHT } else { icons::CHEVRON_DOWN };
                    p.text(rect.left_center() + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, chevron, icons::font(12.0), dim);
                    let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let dir = rel.parent().map(|d| d.display().to_string()).unwrap_or_default();
                    let icon = app.config.file_icons.then(|| icons::for_file(rel));
                    let mut job = super::sidebar::icon_label(icon, &name, text);
                    if !dir.is_empty() {
                        job.append(
                            &dir,
                            6.0,
                            TextFormat { font_id: FontId::proportional(11.0), color: dim, ..Default::default() },
                        );
                    }
                    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 60.0);
                    let g = ui.fonts_mut(|f| f.layout_job(job));
                    p.galley(egui::pos2(rect.left() + 20.0, rect.center().y - g.size().y / 2.0), g, text);
                    // Count badge.
                    let badge = ui.painter().layout_no_wrap(count.to_string(), FontId::proportional(10.5), text);
                    let w = (badge.size().x + 10.0).max(18.0);
                    let r =
                        egui::Rect::from_center_size(rect.right_center() - egui::vec2(6.0 + w / 2.0, 0.0), egui::vec2(w, 16.0));
                    p.rect_filled(r, 8.0, app.palette.hover.gamma_multiply(2.0));
                    p.galley(r.center() - badge.size() / 2.0, badge, text);
                    if resp.on_hover_text(path.display().to_string()).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        toggle_file = Some((*path).clone());
                    }
                }
                Row::Match { item } => {
                    // The line, with the match highlighted.
                    let chars: Vec<char> = item.preview.chars().collect();
                    let (a, b) = (item.preview_range.0.min(chars.len()), item.preview_range.1.min(chars.len()));
                    let fmt = |c: Color32, bg: Color32| TextFormat {
                        font_id: FontId::proportional(12.5),
                        color: c,
                        background: bg,
                        ..Default::default()
                    };
                    let mut job = LayoutJob::default();
                    job.append(&chars[..a].iter().collect::<String>(), 0.0, fmt(dim, Color32::TRANSPARENT));
                    job.append(&chars[a..b].iter().collect::<String>(), 0.0, fmt(text, match_bg));
                    job.append(&chars[b..].iter().collect::<String>(), 0.0, fmt(dim, Color32::TRANSPARENT));
                    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 34.0);
                    let g = ui.fonts_mut(|f| f.layout_job(job));
                    p.galley(egui::pos2(rect.left() + 30.0, rect.center().y - g.size().y / 2.0), g, text);
                    let resp = resp
                        .on_hover_text(format!("{}:{}", item.rel.display(), item.line + 1))
                        .on_hover_cursor(CursorIcon::PointingHand);
                    if resp.clicked() {
                        open = Some(item.clone());
                    }
                }
            }
        }
    });
    if let Some(p) = toggle_file {
        if !app.search.collapsed.remove(&p) {
            app.search.collapsed.insert(p);
        }
    }
    if let Some(item) = open {
        app.open_location(&item.path, item.line, item.col, item.end_col);
    }
}
