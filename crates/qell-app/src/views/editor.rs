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
use qell_lsp::lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, DiagnosticSeverity, Documentation};

use crate::app::{Focus, QellApp};
use crate::commands::Command;
use crate::document::LineMark;

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
                    let resp = inner.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
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

/// First line of the next (or previous) block of changed lines in a diff,
/// wrapping around. `None` when nothing changed.
fn next_change(marks: &[LineMark], from: usize, forward: bool) -> Option<usize> {
    let changed = |i: usize| marks.get(i).is_some_and(|m| *m != LineMark::Context);
    // Starts of change blocks.
    let starts: Vec<usize> = (0..marks.len()).filter(|&i| changed(i) && (i == 0 || !changed(i - 1))).collect();
    if forward {
        starts.iter().copied().find(|&s| s > from).or(starts.first().copied())
    } else {
        starts.iter().rev().copied().find(|&s| s < from).or(starts.last().copied())
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
    let mut trigger: Option<char> = None;
    let mut next_stop = false;
    let triggers: Vec<String> = {
        let lang = app.docs.get(doc_idx).and_then(|d| d.buffer.language()).unwrap_or("");
        let t = app.lsp.completion_triggers(lang);
        if t.is_empty() {
            vec![".".into(), ":".into()]
        } else {
            t
        }
    };
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
            let read_only = doc.read_only;
            for (idx, ev) in events.iter().enumerate() {
                // A diff can be navigated, selected and copied, not edited.
                let edits = matches!(
                    ev,
                    Event::Text(_)
                        | Event::Paste(_)
                        | Event::Cut
                        | Event::Key { key: Key::Backspace | Key::Delete | Key::Enter | Key::Tab, pressed: true, .. }
                );
                if read_only && edits {
                    consumed_events.push(idx);
                    continue;
                }
                // F7 / Shift+F7: next / previous change of a diff, like VS Code.
                if let Event::Key { key: Key::F7, pressed: true, modifiers, .. } = ev {
                    if !doc.line_marks.is_empty() {
                        let line = doc.buffer.cursor.head().line;
                        if let Some(target) = next_change(&doc.line_marks, line, !modifiers.shift) {
                            doc.buffer.set_cursor(Position::new(target, 0), false);
                            doc.scroll_to_cursor = true;
                        }
                        consumed_events.push(idx);
                        continue;
                    }
                }
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
                            let single = t.chars().next().filter(|_| t.chars().count() == 1);
                            if ident && (auto_complete || app.completion.open) {
                                want_completion = true;
                            } else if let Some(c) = single.filter(|c| triggers.iter().any(|t| t.starts_with(*c))) {
                                // The server's own trigger characters (`.`,
                                // `:`, `>`, `"`…) open a fresh list.
                                want_completion = auto_complete;
                                trigger = Some(c);
                                app.completion.open = false;
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
                                Key::ArrowDown | Key::ArrowUp => {
                                    // Move through the list in the order it is shown.
                                    let order = &app.completion_order;
                                    if let Some(pos) = order.iter().position(|&i| i == app.completion.selected) {
                                        let next = if *key == Key::ArrowDown {
                                            (pos + 1).min(order.len() - 1)
                                        } else {
                                            pos.saturating_sub(1)
                                        };
                                        app.completion.selected = order[next];
                                    }
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
                            Key::Tab if !ext && app.snippet.is_some() => {
                                // Jump to the next snippet placeholder (after
                                // the loop: it needs the whole app).
                                next_stop = true;
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
                                app.snippet = None;
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
    if next_stop && !app.next_snippet_stop() {
        // The session ended (cursor left it): Tab indents after all.
        let doc = &mut app.docs[doc_idx];
        let col = doc.buffer.cursor.head().col;
        doc.buffer.insert(&" ".repeat(tab_size - (col % tab_size)));
    }
    if accept {
        app.accept_completion();
    } else if want_completion {
        app.request_completion(trigger);
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

                // Inline diff: added / removed line backgrounds (VS Code's
                // diffEditor colours) and a bar in the gutter.
                let mark = doc.line_marks.get(row).copied().unwrap_or(LineMark::Context);
                if mark != LineMark::Context {
                    let (bg, bar) = match mark {
                        LineMark::Added => {
                            (Color32::from_rgba_unmultiplied(0x9b, 0xb9, 0x55, 0x33), Color32::from_rgb(0x48, 0x7e, 0x02))
                        }
                        _ => (Color32::from_rgba_unmultiplied(0xff, 0x00, 0x00, 0x33), Color32::from_rgb(0xf8, 0x51, 0x49)),
                    };
                    painter.rect_filled(row_rect, 0.0, bg);
                    let bar_rect = Rect::from_min_size(Pos2::new(origin.x + 2.0, y), Vec2::new(3.0, row_height));
                    painter.rect_filled(bar_rect, 0.0, bar);
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
/// The replacement may be a snippet; the caller expands it.
pub fn completion_edit(b: &Buffer, item: &CompletionItem, word_start: Position) -> (Position, Position, String) {
    let head = b.cursor.head();
    if let Some(CompletionTextEdit::Edit(e)) = &item.text_edit {
        let line = b.line(e.range.start.line as usize);
        let s = from_lsp_position(&line, e.range.start);
        let line_e = b.line(e.range.end.line as usize);
        let en = from_lsp_position(&line_e, e.range.end);
        return (s, en.max(head), e.new_text.clone());
    }
    let text = item.insert_text.clone().unwrap_or_else(|| item.label.clone());
    (word_start, head, text)
}

/// VS Code's icon and colour for a completion kind.
fn kind_icon(kind: Option<CompletionItemKind>, text: Color32) -> (char, Color32) {
    let purple = Color32::from_rgb(0xb1, 0x80, 0xd7);
    let blue = Color32::from_rgb(0x75, 0xbe, 0xff);
    let orange = Color32::from_rgb(0xee, 0x9d, 0x28);
    use CompletionItemKind as K;
    match kind.unwrap_or(K::TEXT) {
        K::METHOD | K::FUNCTION | K::CONSTRUCTOR => ('\u{ea8c}', purple),
        K::FIELD => ('\u{eb5f}', blue),
        K::VARIABLE => ('\u{ea88}', blue),
        K::PROPERTY => ('\u{eb65}', text),
        K::CLASS => ('\u{eb5b}', orange),
        K::STRUCT => ('\u{ea91}', text),
        K::INTERFACE => ('\u{eb61}', blue),
        K::MODULE => ('\u{ea8b}', text),
        K::ENUM | K::VALUE => ('\u{ea95}', orange),
        K::ENUM_MEMBER => ('\u{eb5e}', blue),
        K::EVENT => ('\u{ea86}', orange),
        K::KEYWORD => ('\u{eb62}', text),
        K::SNIPPET => ('\u{eb66}', text),
        K::COLOR => ('\u{eb5c}', text),
        K::FILE => ('\u{eb60}', text),
        K::FOLDER => ('\u{ea83}', text),
        K::REFERENCE => ('\u{eb36}', text),
        K::CONSTANT => ('\u{eb5d}', text),
        K::OPERATOR => ('\u{eb64}', text),
        K::TYPE_PARAMETER => ('\u{ea92}', text),
        K::UNIT => ('\u{ea96}', text),
        _ => ('\u{ea93}', text), // text and anything else
    }
}

fn documentation_text(doc: &Documentation) -> String {
    match doc {
        Documentation::String(s) => s.clone(),
        Documentation::MarkupContent(m) => m.value.clone(),
    }
}

/// Render Markdown documentation simply: fenced code in monospace on a
/// darker block, the rest as wrapped text without the markup characters.
fn show_markdown(ui: &mut egui::Ui, md: &str, mono: &egui::FontId, code_bg: Color32, dim: Color32) {
    let mut in_code = false;
    let mut block = String::new();
    let flush = |ui: &mut egui::Ui, block: &mut String, code: bool| {
        let text = block.trim_end_matches('\n').to_string();
        block.clear();
        if text.trim().is_empty() {
            return;
        }
        if code {
            egui::Frame::new().fill(code_bg).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(text).font(mono.clone()));
            });
        } else {
            for para in text.split("\n\n") {
                let line = para.replace('\n', " ");
                let heading = line.starts_with('#');
                let clean = line.trim_start_matches('#').trim().replace(['*', '`'], "");
                let rt = egui::RichText::new(clean).size(12.5);
                ui.label(if heading { rt.strong() } else { rt.color(dim) });
            }
        }
        ui.add_space(4.0);
    };
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            flush(ui, &mut block, in_code);
            in_code = !in_code;
            continue;
        }
        block.push_str(line);
        block.push('\n');
    }
    flush(ui, &mut block, in_code);
}

fn show_completion_popup(app: &mut QellApp, ctx: &Context) {
    if !app.completion.open || app.focus != Focus::Editor {
        return;
    }
    let Some(doc) = app.docs.get(app.active) else { return };
    let prefix = doc.buffer.word_before_cursor();

    // Client-side filter and rank against the typed prefix; servers send a
    // superset and expect the client to narrow it.
    let mut visible: Vec<(usize, usize, Vec<usize>)> = app
        .completion
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let key = it.filter_text.as_deref().unwrap_or(&it.label);
            let (score, _) = crate::commands::fuzzy_match(&prefix, key)?;
            // Highlight against the label, which is what is shown.
            let hits = crate::commands::fuzzy_match(&prefix, &it.label).map(|(_, h)| h).unwrap_or_default();
            // A case-sensitive prefix match ranks first.
            let bonus = if key.starts_with(prefix.as_str()) { 0 } else { 1000 };
            Some((i, score + bonus, hits))
        })
        .collect();
    let all = &app.completion.items;
    visible.sort_by(|a, b| {
        let items = all;
        let sa = items[a.0].sort_text.as_deref().unwrap_or(&items[a.0].label);
        let sb = items[b.0].sort_text.as_deref().unwrap_or(&items[b.0].label);
        a.1.cmp(&b.1).then_with(|| sa.cmp(sb))
    });
    if visible.is_empty() {
        app.completion.open = false;
        return;
    }
    if !visible.iter().any(|v| v.0 == app.completion.selected) {
        app.completion.selected = visible[0].0;
    }
    // Arrow keys move through the visible order, not the server's.
    let order: Vec<usize> = visible.iter().map(|v| v.0).collect();
    app.completion_order = order;
    app.resolve_selected_completion();
    let items = &app.completion.items;

    let text = app.palette.text;
    let dim = app.palette.text_dim;
    let accent = Color32::from_rgb(0x2a, 0xaa, 0xff);
    let sel_bg = app.palette.selection;
    let font = egui::FontId::monospace(app.font_size.max(12.0) - 1.0);
    let small = egui::FontId::proportional(12.0);
    let selected = app.completion.selected;
    let anchor = app.completion.anchor;
    const ROW: f32 = 22.0;
    const WIDTH: f32 = 440.0;
    let mut clicked = None;

    let popup = egui::Area::new(egui::Id::new("completion_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor + Vec2::new(0.0, 2.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(2)).show(ui, |ui| {
                ui.set_width(WIDTH);
                egui::ScrollArea::vertical().max_height(ROW * 10.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, _, hits) in visible.iter().take(200) {
                        let it = &items[*i];
                        let is_sel = *i == selected;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(WIDTH, ROW), Sense::click());
                        let p = ui.painter_at(rect);
                        if is_sel {
                            p.rect_filled(rect, 2.0, sel_bg);
                        }
                        let (icon, icon_color) = kind_icon(it.kind, text);
                        p.text(
                            rect.left_center() + Vec2::new(8.0, 0.0),
                            Align2::LEFT_CENTER,
                            icon,
                            crate::icons::font(14.0),
                            icon_color,
                        );

                        // Label with the typed characters highlighted, then
                        // the signature-ish label details, dimmed.
                        let mut job = LayoutJob::default();
                        for (ci, ch) in it.label.chars().enumerate() {
                            let hit = hits.contains(&ci);
                            let color = if hit { accent } else { text };
                            job.append(&ch.to_string(), 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
                        }
                        if let Some(d) = it.label_details.as_ref().and_then(|d| d.detail.as_deref()) {
                            job.append(d, 0.0, TextFormat { font_id: font.clone(), color: dim, ..Default::default() });
                        }
                        let label = ui.fonts_mut(|f| f.layout_job(job));
                        let label_w = label.size().x;
                        p.galley(rect.left_center() + Vec2::new(28.0, -label.size().y / 2.0), label, text);

                        // Right side: where it comes from / its type.
                        let right = it
                            .label_details
                            .as_ref()
                            .and_then(|d| d.description.clone())
                            .or_else(|| it.detail.clone())
                            .unwrap_or_default();
                        // One line only: details are often multi-line signatures.
                        let right = right.lines().next().unwrap_or("").trim().to_string();
                        let room = WIDTH - 28.0 - label_w - 20.0;
                        if !right.is_empty() && room > 40.0 {
                            let max_chars = (room / 7.0) as usize;
                            let shown: String = if right.chars().count() > max_chars {
                                right.chars().take(max_chars.saturating_sub(1)).chain(std::iter::once('…')).collect()
                            } else {
                                right
                            };
                            p.text(rect.right_center() - Vec2::new(8.0, 0.0), Align2::RIGHT_CENTER, shown, small.clone(), dim);
                        }
                        if resp.clicked() {
                            clicked = Some(*i);
                        }
                        if is_sel {
                            resp.scroll_to_me(None);
                        }
                    }
                });
            });
        });

    // Documentation of the selected item, beside the list.
    if let Some(it) = items.get(selected) {
        let doc_text = it.documentation.as_ref().map(documentation_text).unwrap_or_default();
        let detail = it.detail.clone().unwrap_or_default();
        if !doc_text.trim().is_empty() || !detail.trim().is_empty() {
            let list = popup.response.rect;
            let screen = ctx.content_rect();
            const DOC_W: f32 = 380.0;
            let pos = if list.right() + 4.0 + DOC_W <= screen.right() {
                list.right_top() + Vec2::new(4.0, 0.0)
            } else {
                list.left_top() - Vec2::new(4.0 + DOC_W, 0.0)
            };
            let code_bg = app.palette.editor_bg;
            egui::Area::new(egui::Id::new("completion_docs")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(DOC_W - 16.0);
                    // No scroll area: inside a floating panel it keeps the
                    // height it had before the docs arrived. Long docs are
                    // cut instead, like VS Code's suggest details.
                    if !detail.trim().is_empty() {
                        ui.label(egui::RichText::new(&detail).font(font.clone()));
                        if !doc_text.trim().is_empty() {
                            ui.separator();
                        }
                    }
                    const MAX_LINES: usize = 18;
                    let mut lines: Vec<&str> = doc_text.lines().collect();
                    if lines.len() > MAX_LINES {
                        lines.truncate(MAX_LINES);
                        // Don't leave a code fence open.
                        if lines.iter().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1 {
                            lines.push("```");
                        }
                        lines.push("…");
                    }
                    show_markdown(ui, &lines.join("\n"), &font, code_bg, dim);
                });
            });
        }
    }

    if let Some(i) = clicked {
        app.completion.selected = i;
        app.accept_completion();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_navigation_wraps() {
        use LineMark::*;
        let marks = [Context, Removed, Added, Context, Context, Added, Context];
        assert_eq!(next_change(&marks, 0, true), Some(1));
        assert_eq!(next_change(&marks, 1, true), Some(5));
        assert_eq!(next_change(&marks, 5, true), Some(1));
        assert_eq!(next_change(&marks, 5, false), Some(1));
        assert_eq!(next_change(&marks, 1, false), Some(5));
        assert_eq!(next_change(&[Context, Context], 0, true), None);
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
