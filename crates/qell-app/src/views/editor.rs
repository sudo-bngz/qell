//! The text editor: tabs, gutter, highlighted lines, cursor, selection,
//! diagnostics underline and the completion popup.
//!
//! Rendering is virtualised: only the lines in the viewport are laid out,
//! one `LayoutJob` per line with a section per highlight span. egui caches
//! the resulting galleys, so an unchanged line costs almost nothing.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, Context, Event, Key, Pos2, Rect, Sense, Stroke, Vec2};
use qell_core::{Buffer, HighlightSpan, Position};
use qell_lsp::convert::from_lsp_position;
use qell_lsp::lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, DiagnosticSeverity};

use crate::app::{Focus, QellApp};
use crate::commands::Command;

const GUTTER_PAD: f32 = 12.0;
const TEXT_PAD: f32 = 8.0;

/// Central panel: tab strip + editor for the active document.
pub fn show_central(app: &mut QellApp, ctx: &Context) {
    let bg = app.palette.editor_bg;
    egui::CentralPanel::default().frame(egui::Frame::new().fill(bg)).show(ctx, |ui| {
        show_tabs(app, ui);
        show_editor(app, ui);
    });
    show_completion_popup(app, ctx);
}

fn show_tabs(app: &mut QellApp, ui: &mut egui::Ui) {
    let chrome = app.palette.chrome_bg;
    let editor_bg = app.palette.editor_bg;
    let accent = app.palette.accent;
    let text_dim = app.palette.text_dim;
    let text = app.palette.text;

    egui::Frame::new().fill(chrome).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let mut switch_to = None;
                let mut close = None;
                for (i, doc) in app.docs.iter().enumerate() {
                    let active = i == app.active;
                    let title = doc.title();
                    let fill = if active { editor_bg } else { chrome };
                    let git = doc.buffer.path().and_then(|p| app.git_status_of(p));
                    let color = match git {
                        Some(st) => app.palette.git_color(st),
                        None if active => text,
                        None => text_dim,
                    };
                    let inner = egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(12, 7)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            match doc.buffer.path() {
                                Some(p) => {
                                    let icon = app.config.file_icons.then(|| crate::icons::for_file(p));
                                    ui.label(crate::views::sidebar::icon_label(icon, &title, color));
                                }
                                None => {
                                    ui.label(egui::RichText::new(title).size(13.0).color(color));
                                }
                            }
                            ui.small_button("×").rect
                        })
                        .inner
                    });
                    // The whole tab is the click target; it sits above the ×,
                    // so tell the two apart by where the click landed.
                    let close_rect = inner.inner;
                    let resp = inner.response.interact(Sense::click());
                    if resp.clicked() {
                        if resp.interact_pointer_pos().is_some_and(|p| close_rect.contains(p)) {
                            close = Some(i);
                        } else {
                            switch_to = Some(i);
                        }
                    }
                    if resp.middle_clicked() {
                        close = Some(i);
                    }
                    if active {
                        let r = resp.rect;
                        ui.painter().hline(r.x_range(), r.top() + 1.0, Stroke::new(2.0_f32, accent));
                    }
                }
                if let Some(i) = switch_to {
                    app.active = i;
                    app.focus = Focus::Editor;
                    app.completion.open = false;
                }
                if let Some(i) = close {
                    app.active = i;
                    app.pending_commands.push(Command::CloseTab);
                }
            });
        });
    });
}

/// Text format for a token: its colour, plus italics for emphasis and an
/// underline for links (Markdown).
fn token_format(kind: qell_core::TokenKind, font_id: egui::FontId, color: Color32) -> TextFormat {
    use qell_core::TokenKind;
    TextFormat {
        font_id,
        color,
        italics: kind == TokenKind::Emphasis,
        underline: if kind == TokenKind::Link { Stroke::new(1.0_f32, color.gamma_multiply(0.6)) } else { Stroke::NONE },
        ..Default::default()
    }
}

/// Per-line view of the highlight spans: binary search for the first span
/// that could overlap a byte range.
fn spans_in(spans: &[HighlightSpan], start: usize, end: usize) -> &[HighlightSpan] {
    let first = spans.partition_point(|s| s.end <= start);
    let last = spans[first..].partition_point(|s| s.start < end) + first;
    &spans[first..last]
}

fn show_editor(app: &mut QellApp, ui: &mut egui::Ui) {
    let Some(doc_idx) = app.docs.get(app.active).map(|_| app.active) else { return };
    let font = app.palette.mono(app.font_size);
    let font_small = app.palette.mono(app.font_size * 0.9);
    let (row_height, char_width) = ui.fonts_mut(|f| (f.row_height(&font).ceil() + 2.0, f.glyph_width(&font, 'M')));
    let tab_size = app.config.tab_size;
    let auto_complete = app.config.auto_complete;

    let has_focus_before = app.focus == Focus::Editor && !app.modal_open();
    let id = ui.id().with("editor").with(doc_idx);

    // ---- input ----------------------------------------------------------
    let mut want_completion = false;
    let mut close_completion = false;
    let mut accept = false;
    let mut consumed_events = Vec::new();
    let line_count;
    let visible_rows = ((ui.available_height() / row_height).floor() as usize).max(1);

    {
        let doc = &mut app.docs[doc_idx];
        line_count = doc.buffer.line_count();
        if has_focus_before {
            let events = ui.input(|i| i.events.clone());
            for (idx, ev) in events.iter().enumerate() {
                let b = &mut doc.buffer;
                let mut used = true;
                match ev {
                    Event::Text(t) => {
                        // Ctrl/Cmd combos are commands, not text.
                        let mods = ui.input(|i| i.modifiers);
                        if (mods.ctrl && !mods.alt) || mods.mac_cmd {
                            used = false;
                        } else {
                            b.insert(t);
                            doc.scroll_to_cursor = true;
                            let ident = t.chars().all(|c| c.is_alphanumeric() || c == '_');
                            if ident && (auto_complete || app.completion.open) {
                                want_completion = true;
                            } else if t == "." || t == ":" {
                                want_completion = auto_complete;
                            } else {
                                close_completion = true;
                            }
                        }
                    }
                    Event::Paste(t) => {
                        b.insert(t);
                        doc.scroll_to_cursor = true;
                        close_completion = true;
                    }
                    Event::Copy => ui.ctx().copy_text(copy_text(b)),
                    Event::Cut => {
                        let t = copy_text(b);
                        ui.ctx().copy_text(t);
                        if b.cursor.selection.is_empty() {
                            // Cut the whole line, like VSCode.
                            let l = b.cursor.head().line;
                            let end =
                                if l + 1 < b.line_count() { Position::new(l + 1, 0) } else { Position::new(l, b.line_len(l)) };
                            b.replace_range(Position::new(l, 0), end, "");
                        } else {
                            b.delete_selection();
                        }
                        doc.scroll_to_cursor = true;
                    }
                    Event::Key { key, pressed: true, modifiers, .. } => {
                        let m = *modifiers;
                        let ext = m.shift;
                        let word = m.ctrl || m.alt && cfg!(target_os = "macos");
                        if app.completion.open {
                            match key {
                                Key::ArrowDown => {
                                    app.completion.selected =
                                        (app.completion.selected + 1).min(app.completion.items.len().saturating_sub(1));
                                    consumed_events.push(idx);
                                    continue;
                                }
                                Key::ArrowUp => {
                                    app.completion.selected = app.completion.selected.saturating_sub(1);
                                    consumed_events.push(idx);
                                    continue;
                                }
                                Key::Enter | Key::Tab => {
                                    accept = true;
                                    consumed_events.push(idx);
                                    continue;
                                }
                                Key::Escape => {
                                    close_completion = true;
                                    consumed_events.push(idx);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        match key {
                            Key::ArrowLeft => {
                                if word {
                                    b.move_word(false, ext)
                                } else {
                                    b.move_left(ext)
                                }
                                close_completion = true;
                            }
                            Key::ArrowRight => {
                                if word {
                                    b.move_word(true, ext)
                                } else {
                                    b.move_right(ext)
                                }
                                close_completion = true;
                            }
                            Key::ArrowUp => {
                                b.move_vertical(-1, ext);
                                close_completion = true;
                            }
                            Key::ArrowDown => {
                                b.move_vertical(1, ext);
                                close_completion = true;
                            }
                            Key::PageUp => {
                                b.move_vertical(-(visible_rows as isize), ext);
                                close_completion = true;
                            }
                            Key::PageDown => {
                                b.move_vertical(visible_rows as isize, ext);
                                close_completion = true;
                            }
                            Key::Home => {
                                if m.ctrl {
                                    b.set_cursor(Position::new(0, 0), ext)
                                } else {
                                    b.move_home(ext)
                                }
                                close_completion = true;
                            }
                            Key::End => {
                                if m.ctrl {
                                    let last = b.line_count() - 1;
                                    b.set_cursor(Position::new(last, b.line_len(last)), ext);
                                } else {
                                    b.move_end(ext)
                                }
                                close_completion = true;
                            }
                            Key::Backspace => {
                                if word && b.cursor.selection.is_empty() {
                                    b.move_word(false, true);
                                    b.delete_selection();
                                } else {
                                    b.backspace();
                                }
                                let w = b.word_before_cursor();
                                if app.completion.open && !w.is_empty() {
                                    want_completion = true;
                                } else {
                                    close_completion = true;
                                }
                            }
                            Key::Delete => {
                                if word && b.cursor.selection.is_empty() {
                                    b.move_word(true, true);
                                    b.delete_selection();
                                } else {
                                    b.delete_forward();
                                }
                                close_completion = true;
                            }
                            Key::Enter => {
                                b.insert_newline();
                                close_completion = true;
                            }
                            Key::Tab => {
                                if ext {
                                    dedent_line(b, tab_size);
                                } else {
                                    let col = b.cursor.head().col;
                                    let n = tab_size - (col % tab_size);
                                    b.insert(&" ".repeat(n));
                                }
                                close_completion = true;
                            }
                            Key::Escape => {
                                let h = b.cursor.head();
                                b.set_cursor(h, false);
                                close_completion = true;
                            }
                            _ => used = false,
                        }
                        if used {
                            doc.scroll_to_cursor = true;
                        }
                    }
                    _ => used = false,
                }
                if used {
                    consumed_events.push(idx);
                }
            }
        }
    }
    if !consumed_events.is_empty() {
        // Don't let egui's own focus navigation or widgets see keys we used.
        ui.input_mut(|i| {
            let mut n = 0;
            i.events.retain(|_| {
                let keep = !consumed_events.contains(&n);
                n += 1;
                keep
            });
        });
    }
    if accept {
        app.accept_completion();
    } else if want_completion {
        app.request_completion();
    } else if close_completion {
        app.completion.open = false;
    }

    // ---- layout + paint ---------------------------------------------------
    let line_count = app.docs[doc_idx].buffer.line_count().max(line_count.min(1));
    let digits = line_count.to_string().len().max(3);
    let gutter_w = digits as f32 * char_width + GUTTER_PAD * 2.0;
    let text_x0 = gutter_w + TEXT_PAD;

    let palette_text = app.palette.text;
    let palette_gutter = app.palette.gutter;
    let palette_sel = app.palette.selection;
    let palette_cur_line = app.palette.current_line;
    let palette_cursor = app.palette.cursor;
    let palette_err = app.palette.error;
    let palette_warn = app.palette.warning;
    let token_colors: Vec<(qell_core::TokenKind, Color32)> =
        qell_core::TokenKind::ALL.iter().copied().map(|k| (k, app.palette.token(k))).collect();
    let color_of =
        |k: qell_core::TokenKind| token_colors.iter().find(|(kk, _)| *kk == k).map(|(_, c)| *c).unwrap_or(palette_text);

    ui.spacing_mut().item_spacing.y = 0.0;
    let mut clicked_editor = false;
    let mut cursor_screen: Option<Pos2> = None;

    let scroll =
        egui::ScrollArea::both().id_salt(id).auto_shrink([false, false]).show_rows(ui, row_height, line_count, |ui, range| {
            let doc = &mut app.docs[doc_idx];
            let max_cols = doc.buffer.rope().lines().map(|l| l.len_chars()).max().unwrap_or(0) as f32;
            let content_w = (text_x0 + (max_cols + 4.0) * char_width).max(ui.available_width());
            let origin = ui.cursor().min;
            let rows_h = (range.end - range.start) as f32 * row_height;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(content_w, rows_h), Sense::click_and_drag());
            let painter = ui.painter_at(rect.union(ui.clip_rect()));
            let content_top = origin.y - range.start as f32 * row_height;

            // Mouse → cursor.
            let pos_at = |p: Pos2| -> Position {
                let row = ((p.y - content_top) / row_height).floor().max(0.0) as usize;
                let row = row.min(line_count.saturating_sub(1));
                let col = ((p.x - (origin.x + text_x0)) / char_width + 0.5).floor().max(0.0) as usize;
                Position::new(row, col)
            };
            if resp.clicked() || resp.drag_started() {
                clicked_editor = true;
                if let Some(p) = resp.interact_pointer_pos() {
                    let shift = ui.input(|i| i.modifiers.shift);
                    if resp.double_clicked() {
                        let at = pos_at(p);
                        doc.buffer.set_cursor(at, false);
                        doc.buffer.move_word(false, false);
                        doc.buffer.move_word(true, true);
                    } else {
                        doc.buffer.set_cursor(pos_at(p), shift);
                    }
                    app.completion.open = false;
                }
            } else if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    doc.buffer.set_cursor(pos_at(p), true);
                }
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }

            let cursor = doc.buffer.cursor;
            let (sel_start, sel_end) = cursor.selection.ordered();
            let head = cursor.head();
            let spans = doc.spans(&mut app.languages).to_vec();
            let rope = doc.buffer.rope();

            for row in range.clone() {
                let y = content_top + row as f32 * row_height;
                let row_rect = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), row_height));

                // Current line background.
                if row == head.line && cursor.selection.is_empty() {
                    painter.rect_filled(row_rect, 0.0, palette_cur_line);
                }

                // Selection.
                if !cursor.selection.is_empty() && row >= sel_start.line && row <= sel_end.line {
                    let line_len = doc.buffer.line_len(row);
                    let c0 = if row == sel_start.line { sel_start.col } else { 0 };
                    let c1 = if row == sel_end.line { sel_end.col } else { line_len + 1 };
                    let x0 = origin.x + text_x0 + c0 as f32 * char_width;
                    let x1 = origin.x + text_x0 + c1 as f32 * char_width;
                    painter.rect_filled(Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + row_height)), 0.0, palette_sel);
                }

                // Gutter.
                let num_color = if row == head.line { palette_text } else { palette_gutter };
                painter.text(
                    Pos2::new(origin.x + gutter_w - GUTTER_PAD, y + row_height / 2.0),
                    Align2::RIGHT_CENTER,
                    row + 1,
                    font_small.clone(),
                    num_color,
                );

                // Text with highlighting.
                let line_start_byte = rope.line_to_byte(row);
                let line_text = doc.buffer.line(row);
                let line_end_byte = line_start_byte + line_text.len();
                let mut job = LayoutJob::default();
                let mut at = 0usize;
                for s in spans_in(&spans, line_start_byte, line_end_byte) {
                    let s0 = s.start.max(line_start_byte) - line_start_byte;
                    let s1 = s.end.min(line_end_byte) - line_start_byte;
                    if s0 > at && line_text.is_char_boundary(at) && line_text.is_char_boundary(s0) {
                        job.append(
                            &line_text[at..s0],
                            0.0,
                            TextFormat { font_id: font.clone(), color: palette_text, ..Default::default() },
                        );
                    }
                    if s1 > s0 && line_text.is_char_boundary(s0) && line_text.is_char_boundary(s1) {
                        job.append(&line_text[s0..s1], 0.0, token_format(s.kind, font.clone(), color_of(s.kind)));
                        at = s1;
                    }
                }
                if at < line_text.len() {
                    job.append(
                        &line_text[at..],
                        0.0,
                        TextFormat { font_id: font.clone(), color: palette_text, ..Default::default() },
                    );
                }
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                painter.galley(Pos2::new(origin.x + text_x0, y + 1.0), galley, palette_text);

                // Diagnostics underline.
                for d in &doc.diagnostics {
                    let (ds, de) = (d.range.start.line as usize, d.range.end.line as usize);
                    if row < ds || row > de {
                        continue;
                    }
                    let c0 = if row == ds { from_lsp_position(&line_text, d.range.start).col } else { 0 };
                    let c1 = if row == de { from_lsp_position(&line_text, d.range.end).col } else { line_text.chars().count() };
                    let c1 = c1.max(c0 + 1);
                    let color = match d.severity {
                        Some(DiagnosticSeverity::ERROR) => palette_err,
                        Some(DiagnosticSeverity::WARNING) => palette_warn,
                        _ => palette_gutter,
                    };
                    let x0 = origin.x + text_x0 + c0 as f32 * char_width;
                    let x1 = origin.x + text_x0 + c1 as f32 * char_width;
                    let yy = y + row_height - 2.0;
                    painter.line_segment([Pos2::new(x0, yy), Pos2::new(x1, yy)], Stroke::new(1.0_f32, color));
                }

                // Cursor.
                if row == head.line && has_focus_before {
                    let x = origin.x + text_x0 + head.col as f32 * char_width;
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(x - 0.5, y + 1.0), Vec2::new(2.0, row_height - 2.0)),
                        0.0,
                        palette_cursor,
                    );
                    cursor_screen = Some(Pos2::new(x, y + row_height));
                }
            }

            // Hover tooltip for diagnostics on the hovered line.
            if let Some(p) = resp.hover_pos() {
                let at = pos_at(p);
                let msgs: Vec<String> = doc
                    .diagnostics
                    .iter()
                    .filter(|d| {
                        let (ds, de) = (d.range.start.line as usize, d.range.end.line as usize);
                        at.line >= ds && at.line <= de
                    })
                    .map(|d| d.message.clone())
                    .collect();
                if !msgs.is_empty() {
                    resp.clone().on_hover_text(msgs.join("\n\n"));
                }
            }

            // Keep the cursor in view.
            if doc.scroll_to_cursor {
                doc.scroll_to_cursor = false;
                let x = origin.x + text_x0 + head.col as f32 * char_width;
                let y = content_top + head.line as f32 * row_height;
                let target = Rect::from_min_size(
                    Pos2::new(x - char_width * 2.0, y - row_height),
                    Vec2::new(char_width * 6.0, row_height * 3.0),
                );
                ui.scroll_to_rect(target, None);
            }
        });

    if clicked_editor
        || scroll.inner_rect.contains(ui.input(|i| i.pointer.interact_pos().unwrap_or(Pos2::ZERO)))
            && ui.input(|i| i.pointer.any_pressed())
    {
        app.focus = Focus::Editor;
    }
    if let Some(p) = cursor_screen {
        app.completion.anchor = p;
    }
}

fn copy_text(b: &Buffer) -> String {
    if b.cursor.selection.is_empty() {
        let l = b.cursor.head().line;
        format!("{}\n", b.line(l))
    } else {
        b.selected_text()
    }
}

fn dedent_line(b: &mut Buffer, tab_size: usize) {
    let l = b.cursor.head().line;
    let line = b.line(l);
    let leading = line.chars().take_while(|c| *c == ' ').count();
    let remove = leading.min(tab_size);
    if remove > 0 {
        let head = b.cursor.head();
        b.replace_range(Position::new(l, 0), Position::new(l, remove), "");
        b.set_cursor(Position::new(l, head.col.saturating_sub(remove)), false);
    }
}

// ---- completion -------------------------------------------------------------

/// Completion items from words already in the buffer (no LSP fallback).
pub fn buffer_word_completions(b: &Buffer, prefix: &str) -> Vec<CompletionItem> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let text = b.text();
    let mut words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.len() > prefix.len() && w.starts_with(prefix))
        .collect();
    words.sort_unstable();
    words.dedup();
    words
        .into_iter()
        .take(50)
        .map(|w| CompletionItem { label: w.to_string(), kind: Some(CompletionItemKind::TEXT), ..Default::default() })
        .collect()
}

/// Work out what accepting `item` should do: `(start, end, replacement)`.
pub fn completion_edit(b: &Buffer, item: &CompletionItem, word_start: Position) -> (Position, Position, String) {
    let head = b.cursor.head();
    if let Some(CompletionTextEdit::Edit(e)) = &item.text_edit {
        let line = b.line(e.range.start.line as usize);
        let s = from_lsp_position(&line, e.range.start);
        let line_e = b.line(e.range.end.line as usize);
        let en = from_lsp_position(&line_e, e.range.end);
        return (s, en.max(head), strip_snippet(&e.new_text));
    }
    let text = item.insert_text.clone().unwrap_or_else(|| item.label.clone());
    (word_start, head, strip_snippet(&text))
}

/// We advertise no snippet support, but some servers send `$0`/`${1:x}`
/// anyway. Keep the placeholder text, drop the markers.
fn strip_snippet(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                Some('{') => {
                    chars.next();
                    let mut inner = String::new();
                    let mut depth = 1;
                    for c in chars.by_ref() {
                        if c == '{' {
                            depth += 1;
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        inner.push(c);
                    }
                    if let Some((_, placeholder)) = inner.split_once(':') {
                        out.push_str(&strip_snippet(placeholder));
                    }
                }
                Some(d) if d.is_ascii_digit() => {
                    while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                        chars.next();
                    }
                }
                _ => out.push('$'),
            }
        } else if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn kind_label(kind: Option<CompletionItemKind>) -> &'static str {
    match kind {
        Some(CompletionItemKind::FUNCTION) | Some(CompletionItemKind::METHOD) => "ƒ",
        Some(CompletionItemKind::VARIABLE) | Some(CompletionItemKind::FIELD) | Some(CompletionItemKind::PROPERTY) => "◇",
        Some(CompletionItemKind::CLASS) | Some(CompletionItemKind::STRUCT) | Some(CompletionItemKind::INTERFACE) => "◈",
        Some(CompletionItemKind::ENUM) | Some(CompletionItemKind::ENUM_MEMBER) => "≡",
        Some(CompletionItemKind::MODULE) => "▣",
        Some(CompletionItemKind::KEYWORD) => "k",
        Some(CompletionItemKind::SNIPPET) => "✂",
        Some(CompletionItemKind::CONSTANT) => "π",
        _ => "·",
    }
}

fn show_completion_popup(app: &mut QellApp, ctx: &Context) {
    if !app.completion.open || app.focus != Focus::Editor {
        return;
    }
    let Some(doc) = app.docs.get(app.active) else { return };
    let prefix = doc.buffer.word_before_cursor().to_lowercase();

    // Client-side filter against the current prefix; servers usually send
    // a superset and expect the client to narrow it.
    let mut visible: Vec<usize> = app
        .completion
        .items
        .iter()
        .enumerate()
        .filter(|(_, it)| {
            let key = it.filter_text.as_deref().unwrap_or(&it.label).to_lowercase();
            prefix.is_empty() || crate::commands::fuzzy_score(&prefix, &key).is_some()
        })
        .map(|(i, _)| i)
        .collect();
    visible.sort_by_key(|&i| {
        let it = &app.completion.items[i];
        let key = it.filter_text.as_deref().unwrap_or(&it.label).to_lowercase();
        let starts = !key.starts_with(&prefix);
        (starts, it.sort_text.clone().unwrap_or_else(|| it.label.clone()))
    });
    if visible.is_empty() {
        app.completion.open = false;
        return;
    }
    if !visible.contains(&app.completion.selected) {
        app.completion.selected = visible[0];
    }
    let anchor = app.completion.anchor;
    let font = app.palette.mono(app.font_size);
    let accent = app.palette.accent;
    let text_dim = app.palette.text_dim;
    let selected = app.completion.selected;
    let mut clicked = None;

    egui::Area::new(egui::Id::new("completion_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor + Vec2::new(0.0, 2.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_height(240.0);
                ui.set_min_width(260.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for &i in visible.iter().take(100) {
                        let it = &app.completion.items[i];
                        let is_sel = i == selected;
                        let detail = it.detail.clone().unwrap_or_default();
                        let resp = ui
                            .horizontal(|ui| {
                                let w = ui.available_width().max(260.0);
                                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::click());
                                if is_sel {
                                    ui.painter().rect_filled(rect, 0.0, accent.gamma_multiply(0.35));
                                }
                                let p = ui.painter();
                                p.text(
                                    rect.left_center() + Vec2::new(6.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    kind_label(it.kind),
                                    font.clone(),
                                    text_dim,
                                );
                                p.text(
                                    rect.left_center() + Vec2::new(26.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    &it.label,
                                    font.clone(),
                                    ui.visuals().text_color(),
                                );
                                if !detail.is_empty() {
                                    p.text(
                                        rect.right_center() - Vec2::new(6.0, 0.0),
                                        Align2::RIGHT_CENTER,
                                        detail,
                                        egui::FontId::monospace(font.size * 0.85),
                                        text_dim,
                                    );
                                }
                                resp
                            })
                            .inner;
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        if is_sel {
                            resp.scroll_to_me(None);
                        }
                    }
                });
            });
        });

    if let Some(i) = clicked {
        app.completion.selected = i;
        app.accept_completion();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_markers_are_stripped() {
        assert_eq!(strip_snippet("println!(\"$1\")$0"), "println!(\"\")");
        assert_eq!(strip_snippet("foo(${1:bar}, ${2:baz})"), "foo(bar, baz)");
        assert_eq!(strip_snippet("plain"), "plain");
    }

    #[test]
    fn buffer_words() {
        let b = Buffer::from_text("alpha alphabet beta alpha", None);
        let items = buffer_word_completions(&b, "alp");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["alpha", "alphabet"]);
    }

    #[test]
    fn span_window() {
        use qell_core::TokenKind::*;
        let spans = vec![
            HighlightSpan { start: 0, end: 2, kind: Keyword },
            HighlightSpan { start: 5, end: 8, kind: String },
            HighlightSpan { start: 10, end: 12, kind: Number },
        ];
        let w = spans_in(&spans, 4, 11);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].kind, String);
    }
}
