//! The Outline: a section under the file tree listing what the open file
//! defines, nested by indentation, the one around the cursor highlighted.
//! Click a row to go there.
//!
//! Qell reads the symbols itself for the languages it parses, and asks the
//! language server for the others. Both are redone a moment after the
//! text stops changing, not on every key.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Sense};
use qell_core::TokenKind;
use qell_lsp::RequestId;

use crate::app::{QellApp, SidebarView, SymbolRow};

/// Wait this long after an edit before reading the symbols again.
const SETTLE: Duration = Duration::from_millis(400);

#[derive(Default)]
pub struct Outline {
    /// Folded to its header.
    pub collapsed: bool,
    pub rows: Vec<SymbolRow>,
    /// A language server request in flight.
    pub pending: Option<RequestId>,
    /// Why the list is empty, when it isn't for lack of symbols.
    pub note: Option<String>,
    /// File and buffer version the rows are for.
    key: Option<(PathBuf, i32)>,
    /// When the text was first seen changed, to let it settle.
    changed: Option<Instant>,
    /// The row highlighted last frame.
    last_current: Option<usize>,
}

/// Keep the rows in step with the active tab (while the Outline shows).
pub fn update(app: &mut QellApp, ctx: &egui::Context) {
    let shown = app.config.outline && app.show_sidebar && app.sidebar_view == SidebarView::Explorer;
    if !shown || app.outline.collapsed {
        return;
    }
    let Some(doc) = app.docs.get(app.active) else { return };
    let Some(path) = doc.buffer.path().map(|p| p.to_path_buf()) else {
        app.outline.rows.clear();
        app.outline.key = None;
        app.outline.note = None;
        return;
    };
    let key = (path, doc.buffer.version());
    if app.outline.key.as_ref() == Some(&key) {
        return;
    }
    // Same file, new text: wait for the typing to pause. Another file:
    // read it now.
    let same_file = app.outline.key.as_ref().is_some_and(|(p, _)| *p == key.0);
    if same_file {
        let since = *app.outline.changed.get_or_insert_with(Instant::now);
        if since.elapsed() < SETTLE {
            ctx.request_repaint_after(SETTLE - since.elapsed());
            return;
        }
    }
    app.outline.changed = None;
    let lang = doc.buffer.language().unwrap_or("").to_string();
    app.outline.key = Some(key);
    app.outline.note = None;
    if let Some(rows) = app.local_symbols(app.active) {
        app.outline.rows = rows;
        app.outline.pending = None;
        return;
    }
    match app.request_outline_symbols() {
        Some(id) => app.outline.pending = Some(id),
        None => {
            app.outline.rows.clear();
            app.outline.note = Some(if lang.is_empty() {
                "Qell doesn't know this file's language.".to_string()
            } else {
                format!("No language server for {lang}. Add one under [lsp] in config.toml to see what this file defines.")
            });
        }
    }
}

/// The section, at the bottom of the Explorer.
pub fn show(app: &mut QellApp, ui: &mut egui::Ui) {
    let collapsed = app.outline.collapsed;
    let panel = egui::TopBottomPanel::bottom("outline")
        .resizable(!collapsed)
        .default_height(220.0)
        .height_range(if collapsed { 22.0..=22.0 } else { 80.0..=(ui.available_height() - 80.0).max(80.0) })
        .frame(egui::Frame::new().fill(app.palette.panel_bg).inner_margin(egui::Margin { top: 4, ..Default::default() }))
        .show_separator_line(true);
    panel.show_inside(ui, |ui| {
        let arrow = if collapsed { "▸" } else { "▾" };
        let header = ui.add(
            egui::Label::new(RichText::new(format!("{arrow} OUTLINE")).size(11.0).color(app.palette.text_dim))
                .sense(Sense::click()),
        );
        if header.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.outline.collapsed = !collapsed;
        }
        if collapsed {
            return;
        }
        ui.add_space(2.0);
        egui::ScrollArea::vertical().id_salt("outline_rows").auto_shrink([false, false]).show(ui, |ui| rows(app, ui));
    });
}

fn rows(app: &mut QellApp, ui: &mut egui::Ui) {
    let dim = app.palette.text_dim;
    if let Some(note) = &app.outline.note {
        ui.label(RichText::new(note).size(11.0).italics().color(dim));
        return;
    }
    if app.outline.rows.is_empty() {
        let text = if app.outline.pending.is_some() { "Reading…" } else { "Nothing defined in this file." };
        ui.label(RichText::new(text).size(11.0).italics().color(dim));
        return;
    }
    let Some(doc) = app.docs.get(app.active) else { return };
    let tab = app.config.tab_size.max(1);
    let unit = if doc.indent_unit <= 1 { tab } else { doc.indent_unit };
    let cursor_line = doc.buffer.cursor.head().line;
    // The symbol around the cursor: the last one starting at or above it.
    let current = app.outline.rows.iter().rposition(|r| r.line <= cursor_line);
    let depths: Vec<usize> = app
        .outline
        .rows
        .iter()
        .map(|r| {
            let line = doc.buffer.line(r.line);
            let lead = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            qell_core::columns::disp(&qell_core::columns::display_cols(&line, tab), lead) / unit
        })
        .collect();
    let size = app.font_size;
    let last = app.outline.last_current;
    let mut go = None;
    for (i, row) in app.outline.rows.iter().enumerate() {
        let (glyph, kind_color) = kind_mark(&row.kind, |k| app.palette.token(k));
        let selected = Some(i) == current;
        let text_color = if selected { app.palette.text } else { app.palette.text.gamma_multiply(0.85) };
        let mut job = egui::text::LayoutJob::default();
        let mono = egui::FontId::monospace(size);
        let fmt = |color: Color32| egui::TextFormat { font_id: mono.clone(), color, ..Default::default() };
        job.append(&"  ".repeat(depths[i].min(8)), 0.0, fmt(dim));
        job.append(glyph, 0.0, fmt(kind_color));
        job.append(&format!(" {}", row.name), 0.0, fmt(text_color));
        job.append(
            &format!("  {}", row.kind),
            0.0,
            egui::TextFormat { font_id: egui::FontId::proportional(size * 0.8), color: dim, ..Default::default() },
        );
        let resp = ui.add(egui::Button::selectable(selected, job).frame_when_inactive(false));
        // Follow the cursor: bring the row in view when it becomes the one.
        if selected && last != current {
            resp.scroll_to_me(None);
        }
        if resp.on_hover_text(format!("line {}", row.line + 1)).clicked() {
            go = Some(row.clone());
        }
    }
    app.outline.last_current = current;
    if let Some(row) = go {
        if let Some(path) = app.docs.get(app.active).and_then(|d| d.buffer.path()).map(|p| p.to_path_buf()) {
            app.open_location(&path, row.line, row.col, row.end_col);
        }
    }
}

/// A one-letter mark per kind, coloured like that kind of code.
fn kind_mark(kind: &str, color: impl Fn(TokenKind) -> Color32) -> (&'static str, Color32) {
    match kind {
        "function" | "method" | "constructor" => ("ƒ", color(TokenKind::Function)),
        "class" | "struct" | "interface" | "enum" | "type" | "trait" => ("◆", color(TokenKind::Type)),
        "module" | "namespace" | "package" => ("▣", color(TokenKind::Keyword)),
        "constant" => ("π", color(TokenKind::Constant)),
        "field" | "property" => ("·", color(TokenKind::Property)),
        "resource" | "data" => ("▤", color(TokenKind::Type)),
        _ => ("▪", color(TokenKind::Variable)),
    }
}
