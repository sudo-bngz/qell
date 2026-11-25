//! The text editor: tabs, gutter, highlighted lines, cursor, selection,
//! diagnostics underline and the completion popup.
//!
//! Rendering is virtualised: only the lines in the viewport are laid out,
//! one `LayoutJob` per line with a section per highlight span. egui caches
//! the resulting galleys, so an unchanged line costs almost nothing.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, Context, Event, Key, Pos2, Rect, Sense, Stroke, Vec2};
use qell_core::structure::{active_guide, guide_indent, indent_of};
use qell_core::{Buffer, HighlightSpan, Position};
use qell_lsp::convert::from_lsp_position;
use qell_lsp::lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, DiagnosticSeverity, Documentation};

use crate::app::{Focus, QellApp};
use crate::commands::Command;
use crate::document::{DiskChange, LineMark};

const GUTTER_PAD: f32 = 12.0;
const TEXT_PAD: f32 = 8.0;

/// Central panel: tab strip + editor for the active document.
pub fn show_central(app: &mut QellApp, ctx: &Context) {
    let bg = app.palette.editor_bg;
    egui::CentralPanel::default().frame(egui::Frame::new().fill(bg)).show(ctx, |ui| {
        // The cluster view takes the whole area, k9s-style.
        if app.k8s.open {
            super::k8s::show(app, ui, ctx);
            return;
        }
        if app.aws.open {
            super::aws::show(app, ui, ctx);
            return;
        }
        show_tabs(app, ui);
        // A picture or a video is looked at, not edited.
        if super::media::show(app, ui, ctx) {
            return;
        }
        show_manifest_bar(app, ui);
        show_disk_bar(app, ui);
        if app.config.vim_mode {
            show_vim_bar(app, ui);
        }
        show_editor(app, ui);
    });
    show_completion_popup(app, ctx);
}

/// This tab is a cluster object's manifest: a strip saying which one,
/// and that saving sends it back.
fn show_manifest_bar(app: &mut QellApp, ui: &mut egui::Ui) {
    let Some(manifest) = app.active_manifest() else { return };
    let where_ = match (&manifest.context, &manifest.object.namespace) {
        (c, Some(ns)) if !c.is_empty() => format!("{c} · {ns}"),
        (c, None) if !c.is_empty() => c.clone(),
        (_, Some(ns)) => ns.clone(),
        (_, None) => String::new(),
    };
    let text = format!("{} ({where_}) — saving applies it to the cluster", manifest.object.slash());
    let (fg, accent) = (app.palette.text, app.palette.accent);
    egui::TopBottomPanel::top("manifest_bar")
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(accent.gamma_multiply(0.22)).inner_margin(egui::Margin::symmetric(10, 4)))
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(text).color(fg).size(12.0));
            });
        });
}

/// The file changed outside Qell and you have unsaved edits: a strip
/// above the text asking which version to keep.
fn show_disk_bar(app: &mut QellApp, ui: &mut egui::Ui) {
    let Some(conflict) = app.docs.get(app.active).and_then(|d| d.conflict) else { return };
    let name = app.docs[app.active].buffer.file_name();
    let (text, warn) = (app.palette.text, app.palette.warning);
    let deleted = conflict == DiskChange::Deleted;
    let mut choice: Option<bool> = None;
    egui::TopBottomPanel::top("disk_bar")
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(warn.gamma_multiply(0.22)).inner_margin(egui::Margin::symmetric(10, 5)))
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                let what = if deleted { "was deleted on disk" } else { "changed on disk" };
                ui.label(egui::RichText::new(format!("{name} {what}, and you have unsaved changes.")).color(text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let keep = if deleted { "Keep mine (save to recreate)" } else { "Keep mine" };
                    if ui.button(keep).on_hover_text("Leave the editor as it is; saving overwrites the file").clicked() {
                        choice = Some(false);
                    }
                    if !deleted && ui.button("Reload").on_hover_text("Take the file from disk (undo brings yours back)").clicked()
                    {
                        choice = Some(true);
                    }
                });
            });
        });
    match choice {
        Some(true) => app.reload_active(),
        Some(false) => app.keep_mine(),
        None => {}
    }
}

/// Vim's last line: the command being typed, a message, or the mode.
fn show_vim_bar(app: &mut QellApp, ui: &mut egui::Ui) {
    let font = app.palette.mono(app.font_size * 0.95);
    let height = ui.fonts_mut(|f| f.row_height(&font)).ceil() + 6.0;
    let (text, dim, error, chrome) = (app.palette.text, app.palette.text_dim, app.palette.error, app.palette.chrome_bg);
    egui::TopBottomPanel::bottom("vim_bar")
        .exact_height(height)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(chrome).inner_margin(egui::Margin::symmetric(8, 0)))
        .show_inside(ui, |ui| {
            let rect = ui.max_rect();
            let painter = ui.painter();
            let left = rect.left_center();
            let vim = &app.vim;
            if let Some(cmd) = &vim.cmdline {
                let r = painter.text(left, Align2::LEFT_CENTER, cmd, font.clone(), text);
                // The command line's cursor.
                let w = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
                painter.rect_filled(
                    Rect::from_min_size(r.right_top(), Vec2::new(w, r.height())),
                    0.0,
                    app.palette.cursor.gamma_multiply(0.7),
                );
            } else if !vim.message.is_empty() {
                painter.text(left, Align2::LEFT_CENTER, &vim.message, font.clone(), if vim.error { error } else { text });
            } else {
                let color = if vim.mode == crate::vim::Mode::Normal { dim } else { text };
                painter.text(left, Align2::LEFT_CENTER, vim.mode_label(), font.clone(), color);
            }
            // Keys of an unfinished command, as Vim's showcmd.
            painter.text(rect.right_center() - Vec2::new(24.0, 0.0), Align2::RIGHT_CENTER, vim.pending(), font.clone(), dim);
        });
}

/// An input event as a Vim key, if it is one.
fn vim_key(ev: &Event, mods: egui::Modifiers) -> Option<crate::vim::VimKey> {
    use crate::vim::VimKey;
    match ev {
        Event::Text(t) if !mods.ctrl && !mods.mac_cmd => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(VimKey::Char(c)),
                (Some(_), Some(_)) => Some(VimKey::Paste(t.clone())),
                _ => None,
            }
        }
        Event::Paste(t) => Some(VimKey::Paste(t.clone())),
        Event::Key { key, pressed: true, modifiers, .. } => {
            let plain = !modifiers.ctrl && !modifiers.mac_cmd && !modifiers.alt;
            match key {
                Key::Escape => Some(VimKey::Esc),
                Key::Enter if plain => Some(VimKey::Enter),
                Key::Backspace if plain => Some(VimKey::Backspace),
                Key::Delete if plain => Some(VimKey::Char('x')),
                Key::ArrowLeft if plain => Some(VimKey::Left),
                Key::ArrowRight if plain => Some(VimKey::Right),
                Key::ArrowUp if plain => Some(VimKey::Up),
                Key::ArrowDown if plain => Some(VimKey::Down),
                // Ctrl-D, Ctrl-U, Ctrl-R, Ctrl-[…
                _ if modifiers.ctrl && !modifiers.shift && !modifiers.alt && !modifiers.mac_cmd => {
                    let name = key.name();
                    let c = name.chars().next().filter(|_| name.chars().count() == 1)?;
                    Some(VimKey::Ctrl(c.to_ascii_lowercase()))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// What a tab's right-click menu asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabMenu {
    Close,
    CloseOthers,
    CloseRight,
    CloseAll,
    CopyPath,
    CopyRelativePath,
    Reveal,
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
                let mut tab_menu: Option<(usize, TabMenu)> = None;
                let count = app.docs.len();
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
                                    ui.label(crate::views::sidebar::icon_label(icon, &title, color, app.font_size));
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
                    resp.context_menu(|ui| {
                        let mut pick = |ui: &mut egui::Ui, label: &str, enabled: bool, what: TabMenu| {
                            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                tab_menu = Some((i, what));
                                ui.close();
                            }
                        };
                        pick(ui, "Close", true, TabMenu::Close);
                        pick(ui, "Close Others", count > 1, TabMenu::CloseOthers);
                        pick(ui, "Close to the Right", i + 1 < count, TabMenu::CloseRight);
                        pick(ui, "Close All", true, TabMenu::CloseAll);
                        if doc.buffer.path().is_some() {
                            ui.separator();
                            pick(ui, "Copy Path", true, TabMenu::CopyPath);
                            pick(ui, "Copy Relative Path", true, TabMenu::CopyRelativePath);
                            pick(ui, super::sidebar::reveal_label(), true, TabMenu::Reveal);
                        }
                    });
                    if active {
                        let r = resp.rect;
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0_f32, accent));
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
                if let Some((i, what)) = tab_menu {
                    let path = app.docs[i].buffer.path().map(std::path::Path::to_path_buf);
                    match what {
                        TabMenu::Close => app.close_tabs_where(|j| j == i),
                        TabMenu::CloseOthers => app.close_tabs_where(|j| j != i),
                        TabMenu::CloseRight => app.close_tabs_where(|j| j > i),
                        TabMenu::CloseAll => app.close_tabs_where(|_| true),
                        TabMenu::CopyPath => ui.ctx().copy_text(path.map(|p| p.display().to_string()).unwrap_or_default()),
                        TabMenu::CopyRelativePath => {
                            let rel = path.map(|p| app.relative_path(&p)).unwrap_or_default();
                            ui.ctx().copy_text(rel);
                        }
                        TabMenu::Reveal => {
                            if let Some(p) = path {
                                app.reveal_in_file_manager(&p);
                            }
                        }
                    }
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

/// The symbol around char column `col` of `line` (touching it from either
/// side): an identifier, or for Terraform an address like
/// `aws_s3_bucket.logs.arn`. Char range.
pub fn word_range(line: &str, col: usize, terraform: bool) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || (terraform && (c == '.' || c == '-'));
    let col = col.min(chars.len());
    // On the word, or just after it (cursor at its end).
    let at = if chars.get(col).is_some_and(|c| is_word(*c)) {
        col
    } else if col > 0 && is_word(chars[col - 1]) {
        col - 1
    } else {
        return None;
    };
    let start = (0..=at).rev().take_while(|&i| is_word(chars[i])).last()?;
    let end = (at..chars.len()).take_while(|&i| is_word(chars[i])).last()? + 1;
    // Trim dots at the edges ("var.region." → "var.region").
    let (mut s, mut e) = (start, end);
    while s < e && chars[s] == '.' {
        s += 1;
    }
    while e > s && chars[e - 1] == '.' {
        e -= 1;
    }
    (e > s).then_some((s, e))
}

/// Bracket pair colours by nesting depth, as they always were.
const BRACKET_COLORS: [Color32; 3] =
    [Color32::from_rgb(0xff, 0xd7, 0x00), Color32::from_rgb(0xda, 0x70, 0xd6), Color32::from_rgb(0x17, 0x9f, 0xff)];
const BRACKET_UNMATCHED: Color32 = Color32::from_rgb(0xe5, 0x6b, 0x5e);
const BRACKET_MATCH_BG: Color32 = Color32::from_rgba_premultiplied(0x1a, 0x1a, 0x1a, 0x1a);
const BRACKET_MATCH_BORDER: Color32 = Color32::from_rgb(0x8a, 0x83, 0x7c);
/// Gutter marks for lines changed since the last commit.
const GUTTER_ADDED: Color32 = Color32::from_rgb(0x9d, 0xb8, 0x82);
const GUTTER_MODIFIED: Color32 = Color32::from_rgb(0xd9, 0xb9, 0x8a);
const GUTTER_DELETED: Color32 = Color32::from_rgb(0xc9, 0x6a, 0x5d);
const GUIDE: Color32 = Color32::from_rgb(0x24, 0x21, 0x1f);
const GUIDE_ACTIVE: Color32 = Color32::from_rgb(0x45, 0x40, 0x3b);

/// Append `text[s..e]` with `fmt`, except the bytes listed in `recolor`
/// (sorted), which keep the format but take their own colour.
fn append_recolored(job: &mut LayoutJob, text: &str, s: usize, e: usize, fmt: &TextFormat, recolor: &[(usize, Color32)]) {
    let mut at = s;
    for &(b, color) in recolor.iter().filter(|(b, _)| *b >= s && *b < e) {
        // Brackets are one byte (ASCII).
        if !text.is_char_boundary(b) || !text.is_char_boundary(b + 1) {
            continue;
        }
        if b > at {
            job.append(&text[at..b], 0.0, fmt.clone());
        }
        job.append(&text[b..b + 1], 0.0, TextFormat { color, ..fmt.clone() });
        at = b + 1;
    }
    if e > at {
        job.append(&text[at..e], 0.0, fmt.clone());
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
    let bracket_colors = app.config.bracket_pair_colors;
    let word_wrap = app.config.word_wrap;
    let vim_on = app.config.vim_mode;
    let auto_close = app.config.auto_close;
    let indent_guides = app.config.indent_guides;

    let has_focus_before = app.focus == Focus::Editor && !app.modal_open();
    let id = ui.id().with("editor").with(doc_idx);

    // Esc in a tab the cluster view opened goes back to the list, the way
    // Esc backs out of a view in k9s. In Vim mode the first Esc is still
    // Vim's, so it only applies from Normal mode.
    if app.docs[doc_idx].from_cluster
        && has_focus_before
        && !app.find.open
        && !app.completion.open
        && !(vim_on && app.vim.mode != crate::vim::Mode::Normal)
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
    {
        // A describe or YAML tab has served its purpose; a manifest you
        // may still be editing stays where it is.
        if app.docs[doc_idx].read_only {
            app.pending_commands.push(Command::CloseTab);
        } else {
            if app.docs[doc_idx].buffer.is_dirty() {
                app.notify("Manifest left unsaved in its tab.");
            }
            app.back_to_cluster = true;
        }
        return;
    }

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
        if vim_on {
            app.vim.sync(&mut doc.buffer);
        }
        if has_focus_before {
            let events = ui.input(|i| i.events.clone());
            let read_only = doc.read_only;
            for (idx, ev) in events.iter().enumerate() {
                if vim_on {
                    let key = vim_key(ev, ui.input(|i| i.modifiers)).filter(|k| app.vim.wants(k));
                    if let Some(key) = key {
                        let version = doc.buffer.version();
                        let vctx = crate::vim::Ctx { tab_size, page: visible_rows };
                        app.pending_vim.extend(app.vim.key(&mut doc.buffer, key, &vctx));
                        // A read-only view (a diff) takes motions, not changes.
                        if read_only && doc.buffer.version() != version {
                            app.vim.reset(&mut doc.buffer);
                            doc.buffer.undo();
                            app.vim.set_message("E21: Cannot make changes, this view is read-only", true);
                        }
                        doc.scroll_to_cursor = true;
                        doc.goal_col = None;
                        close_completion = true;
                        app.snippet = None;
                        consumed_events.push(idx);
                        // The command bar is drawn before the editor: show the result now.
                        ui.ctx().request_repaint();
                        continue;
                    }
                    // Outside Insert mode nothing else types into the text.
                    if app.vim.captures() && matches!(ev, Event::Key { .. } | Event::Text(_)) {
                        consumed_events.push(idx);
                        continue;
                    }
                }
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
                let vertical = matches!(
                    ev,
                    Event::Key { key: Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown, pressed: true, .. }
                );
                if !vertical && !matches!(ev, Event::PointerMoved(_) | Event::MouseMoved(_)) {
                    doc.goal_col = None;
                }
                // Up/Down follow wrapped rows (when the layout is up to date).
                let by_rows = doc.wrap.is_wrapped() && doc.wrap_is_current() && !app.completion.open;
                if by_rows && vertical {
                    if let Event::Key { key, modifiers, .. } = ev {
                        let rows = match key {
                            Key::ArrowUp => -1,
                            Key::ArrowDown => 1,
                            Key::PageUp => -(visible_rows as isize),
                            _ => visible_rows as isize,
                        };
                        doc.move_rows(rows, modifiers.shift);
                        doc.scroll_to_cursor = true;
                        close_completion = true;
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
                            // A bracket or quote may bring its other half.
                            let paired =
                                auto_close && t.chars().count() == 1 && b.insert_pair(t.chars().next().expect("one char"));
                            if !paired {
                                b.insert(t);
                            }
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
                        b.insert_pasted(t);
                        doc.scroll_to_cursor = true;
                        close_completion = true;
                    }
                    Event::Copy => ui.ctx().copy_text(copy_text(b)),
                    Event::Cut => {
                        ui.ctx().copy_text(cut(b));
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
                                } else if !(auto_close && b.backspace_pair()) {
                                    // Between the two halves of an empty
                                    // pair, both go.
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
                                b.clear_extra_cursors();
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
    // Completions don't follow several cursors around.
    let many_cursors = app.docs[doc_idx].buffer.has_extra_cursors();
    if accept && !many_cursors {
        app.accept_completion();
    } else if want_completion && !many_cursors {
        app.request_completion(trigger);
    } else if close_completion || many_cursors {
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

    // Minimap on the right, when on and there's room; the editor (and its
    // word wrap) gets the rest.
    let full = ui.available_rect_before_wrap();
    let mm_w = if app.config.minimap && full.width() > 480.0 { super::minimap::WIDTH } else { 0.0 };
    let mm_rect = Rect::from_min_max(Pos2::new(full.right() - mm_w, full.top()), full.max);
    let outer = ui;
    let mut editor_ui = outer
        .new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(full.min, Pos2::new(full.right() - mm_w, full.bottom()))));
    let ui = &mut editor_ui;

    // Word wrap: as many columns as fit beside the gutter (minus the
    // vertical scroll bar).
    let wrap_cols = if word_wrap { ((ui.available_width() - text_x0 - 14.0) / char_width).floor().max(1.0) as usize } else { 0 };
    app.docs[doc_idx].update_wrap(wrap_cols);
    let row_total = app.docs[doc_idx].wrap.row_count();

    ui.spacing_mut().item_spacing.y = 0.0;
    let mut clicked_editor = false;
    let mut go_to_definition = false;
    let mut menu_choice: Option<EditorMenu> = None;
    // A click on a change mark in the gutter: its line and where to put
    // the little Revert / Stage menu.
    let mut gutter_hunk: Option<(usize, Pos2)> = None;
    let mut cursor_screen: Option<Pos2> = None;

    // Clicks and drags on the minimap scroll the editor this frame.
    let mut area = egui::ScrollArea::both().id_salt(id).auto_shrink([false, false]);
    if mm_w > 0.0 {
        let mm = &mut app.docs[doc_idx].minimap;
        if let Some(y) = super::minimap::input(outer, mm_rect, id.with("minimap"), mm, row_total, row_height) {
            area = area.vertical_scroll_offset(y);
        }
    }
    let scroll = area.show_rows(ui, row_height, row_total, |ui, range| {
        let doc = &mut app.docs[doc_idx];
        // Taken out for the frame (restored at the end) so it can be read
        // while the buffer is changed by clicks.
        let wl = std::mem::take(&mut doc.wrap);
        let content_w = if wl.is_wrapped() {
            ui.available_width()
        } else {
            let max_cols = doc.buffer.rope().lines().map(|l| l.len_chars()).max().unwrap_or(0) as f32;
            (text_x0 + (max_cols + 4.0) * char_width).max(ui.available_width())
        };
        let origin = ui.cursor().min;
        let rows_h = (range.end - range.start) as f32 * row_height;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(content_w, rows_h), Sense::click_and_drag());
        if has_focus_before {
            super::hold_keyboard(ui, id.with("keyboard"), ui.clip_rect().min);
        }
        let painter = ui.painter_at(rect.union(ui.clip_rect()));
        let content_top = origin.y - range.start as f32 * row_height;

        // Mouse → cursor.
        let pos_at = |b: &Buffer, p: Pos2| -> Position {
            let row = ((p.y - content_top) / row_height).floor().max(0.0) as usize;
            let x = ((p.x - (origin.x + text_x0)) / char_width + 0.5).floor().max(0.0) as usize;
            wl.position_at(row.min(row_total.saturating_sub(1)), x, |l| b.line_len(l))
        };
        // A click on the gutter's change mark opens its menu instead of
        // moving the cursor.
        let on_mark = resp.clicked()
            && resp.interact_pointer_pos().is_some_and(|p| p.x >= origin.x + gutter_w - 9.0 && p.x < origin.x + gutter_w);
        if on_mark {
            if let Some(p) = resp.interact_pointer_pos() {
                let line = pos_at(&doc.buffer, p).line;
                let changed = doc.changes.get(line).is_some_and(|c| c.added || c.modified || c.deleted_above || c.deleted_below);
                if changed {
                    gutter_hunk = Some((line, p));
                    clicked_editor = true;
                }
            }
        }
        if !on_mark && (resp.clicked() || resp.drag_started()) {
            clicked_editor = true;
            if let Some(p) = resp.interact_pointer_pos() {
                let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
                doc.goal_col = None;
                // ⌥-click puts another cursor there (or takes it away).
                let add_cursor = alt && !shift;
                if add_cursor {
                    let at = pos_at(&doc.buffer, p);
                    doc.buffer.toggle_cursor(qell_core::Cursor::at(at));
                    app.completion.open = false;
                } else if !shift {
                    doc.buffer.clear_extra_cursors();
                }
                if !add_cursor {
                    if resp.double_clicked() {
                        let at = pos_at(&doc.buffer, p);
                        // The word under the pointer, on this line only (word
                        // jumps would run on past the line's end).
                        let line = doc.buffer.line(at.line);
                        if let Some((s, e)) = word_range(&line, at.col, false) {
                            doc.buffer.set_cursor(Position::new(at.line, s), false);
                            doc.buffer.set_cursor(Position::new(at.line, e), true);
                        } else {
                            doc.buffer.set_cursor(at, false);
                        }
                    } else {
                        let at = pos_at(&doc.buffer, p);
                        doc.buffer.set_cursor(at, shift);
                    }
                    app.completion.open = false;
                    // ⌘-click / Ctrl-click: go to definition.
                    if resp.clicked() && ui.input(|i| i.modifiers.command) {
                        go_to_definition = true;
                    }
                }
            }
        } else if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let at = pos_at(&doc.buffer, p);
                doc.buffer.set_cursor(at, true);
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        // Right-click: the cursor moves there (unless it's inside the
        // selection, which the menu then acts on), as in VS Code.
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let at = pos_at(&doc.buffer, p);
                let (s, e) = doc.buffer.cursor.selection.ordered();
                if doc.buffer.cursor.selection.is_empty() || at < s || at > e {
                    doc.buffer.set_cursor(at, false);
                }
            }
            clicked_editor = true;
        }
        let read_only = doc.read_only;
        resp.context_menu(|ui| editor_menu(ui, &mut menu_choice, read_only));
        // Holding ⌘ / Ctrl: underline the symbol under the mouse, like a
        // link, since clicking it goes to its definition.
        let link = resp.hover_pos().filter(|_| ui.input(|i| i.modifiers.command)).and_then(|p| {
            let vrow = ((p.y - content_top) / row_height).floor().max(0.0) as usize;
            let x = ((p.x - (origin.x + text_x0)) / char_width).floor().max(0.0) as usize;
            if vrow >= row_total {
                return None;
            }
            let Position { line: row, col } = wl.position_at(vrow, x, |l| doc.buffer.line_len(l));
            let line = doc.buffer.line(row);
            let terraform = doc.buffer.language() == Some("terraform");
            // Only when the mouse is on the word itself, not after it.
            word_range(&line, col + 1, terraform).filter(|(s, e)| col >= *s && col < *e).map(|(s, e)| (row, s, e))
        });
        if link.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let cursor = doc.buffer.cursor;
        let all_cursors = doc.buffer.cursors();

        let head = cursor.head();
        let vim_block = if vim_on { app.vim.block_cursor(&doc.buffer) } else { None };
        doc.update_structure(&mut app.languages, tab_size);
        doc.update_changes();
        let spans = doc.spans(&mut app.languages).to_vec();
        let rope = doc.buffer.rope();

        // Bracket pair at the cursor, highlighted with a box.
        let cursor_pair = if cursor.selection.is_empty() { doc.brackets.pair_at(head.line, head.col) } else { None };
        // Indent guides: the block around the cursor gets the bright one.
        let line_indent = |r: usize| indent_of(&doc.buffer.line(r));
        let unit = doc.indent_unit.max(1);
        let active = if indent_guides { active_guide(&line_indent, line_count, head.line, unit) } else { None };

        for vrow in range.clone() {
            // A row on screen: all of a line, or a piece of a wrapped one.
            let vr = wl.row(vrow);
            let row = vr.line;
            let y = content_top + vrow as f32 * row_height;
            let line_chars = doc.buffer.line_len(row);
            let seg_end = vr.end.min(line_chars);
            let last_seg = vr.end >= line_chars;
            let in_seg = |c: usize| c >= vr.start && (c < seg_end || last_seg);
            let xcol = |c: usize| origin.x + text_x0 + (vr.indent + c.saturating_sub(vr.start)) as f32 * char_width;
            // Clip a column range of the line to this row ([s, e), the
            // last row keeping anything past the end).
            let clip = |c0: usize, c1: usize| {
                let (s, e) = (c0.max(vr.start), if last_seg { c1 } else { c1.min(seg_end) });
                (e > s).then_some((s, e))
            };
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

            // Selections, one per cursor.
            for c in &all_cursors {
                let (sel_start, sel_end) = c.selection.ordered();
                if c.selection.is_empty() || row < sel_start.line || row > sel_end.line {
                    continue;
                }
                let line_len = doc.buffer.line_len(row);
                let c0 = if row == sel_start.line { sel_start.col } else { 0 };
                let c1 = if row == sel_end.line { sel_end.col } else { line_len + 1 };
                if let Some((c0, c1)) = clip(c0, c1) {
                    let (x0, x1) = (xcol(c0), xcol(c1));
                    painter.rect_filled(Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + row_height)), 0.0, palette_sel);
                }
            }

            // Find in file: every match, the current one brighter.
            if app.find.open {
                let geom = super::find::RowGeom { line: row, y, height: row_height, line_len: line_chars };
                super::find::paint_matches(&painter, &app.find, &geom, clip, xcol);
            }

            // Vim: matches of the last search.
            if vim_on {
                for (c0, c1) in app.vim.matches_in(&doc.buffer.line(row)) {
                    if let Some((c0, c1)) = clip(c0, c1) {
                        let r = Rect::from_min_max(Pos2::new(xcol(c0), y), Pos2::new(xcol(c1), y + row_height));
                        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(0xea, 0x5c, 0x00, 0x55));
                    }
                }
            }

            // Indent guides.
            if indent_guides {
                let level = guide_indent(&line_indent, line_count, row);
                for c in (0..level).step_by(unit) {
                    let on = active.is_some_and(|(ac, s, e)| ac == c && (s..=e).contains(&row));
                    let x = (origin.x + text_x0 + c as f32 * char_width).round() + 0.5;
                    let color = if on { GUIDE_ACTIVE } else { GUIDE };
                    painter.vline(x, y..=y + row_height, Stroke::new(1.0_f32, color));
                }
            }

            // Matching bracket: a box around each of the pair.
            if let Some((o, c)) = cursor_pair {
                for b in [o, c].iter().filter(|b| b.line == row && in_seg(b.col)) {
                    let x = xcol(b.col);
                    let r = Rect::from_min_size(Pos2::new(x, y + 1.0), Vec2::new(char_width, row_height - 2.0));
                    painter.rect_filled(r, 1.0, BRACKET_MATCH_BG);
                    painter.rect_stroke(r, 1.0, Stroke::new(1.0_f32, BRACKET_MATCH_BORDER), egui::StrokeKind::Inside);
                }
            }

            // Changed since git's version: a bar beside the line number
            // (green: added, blue: modified), a red wedge where lines went.
            if let Some(change) = doc.changes.get(row) {
                let x = origin.x + gutter_w - 7.0;
                if change.added || change.modified {
                    let color = if change.added { GUTTER_ADDED } else { GUTTER_MODIFIED };
                    painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new(3.0, row_height)), 0.0, color);
                }
                let wedge = |y: f32| {
                    let points = vec![Pos2::new(x, y - 4.0), Pos2::new(x + 5.0, y), Pos2::new(x, y + 4.0)];
                    painter.add(egui::Shape::convex_polygon(points, GUTTER_DELETED, Stroke::NONE));
                };
                if change.deleted_above && vr.start == 0 {
                    wedge(y);
                }
                if change.deleted_below && last_seg {
                    wedge(y + row_height);
                }
            }

            // Gutter: the number on a line's first row only.
            if vr.start == 0 {
                let num_color = if row == head.line { palette_text } else { palette_gutter };
                painter.text(
                    Pos2::new(origin.x + gutter_w - GUTTER_PAD, y + row_height / 2.0),
                    Align2::RIGHT_CENTER,
                    row + 1,
                    font_small.clone(),
                    num_color,
                );
            }

            // Text with highlighting.
            let line_start_byte = rope.line_to_byte(row);
            let line_text = doc.buffer.line(row);
            // This row's bytes of the line.
            let byte_of = |c: usize| line_text.char_indices().nth(c).map_or(line_text.len(), |(b, _)| b);
            let (bs, be) = (byte_of(vr.start), byte_of(seg_end));
            // Brackets recoloured by depth: (byte in line, colour).
            let recolor: Vec<(usize, Color32)> = if bracket_colors {
                doc.brackets
                    .on_line(row)
                    .iter()
                    .map(|b| {
                        let color = if b.partner.is_some() { BRACKET_COLORS[b.depth % 3] } else { BRACKET_UNMATCHED };
                        (b.byte - line_start_byte, color)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let mut job = LayoutJob::default();
            let mut at = bs;
            let plain = TextFormat { font_id: font.clone(), color: palette_text, ..Default::default() };
            for s in spans_in(&spans, line_start_byte + bs, line_start_byte + be) {
                let s0 = s.start.max(line_start_byte + bs) - line_start_byte;
                let s1 = s.end.min(line_start_byte + be) - line_start_byte;
                if s0 > at && line_text.is_char_boundary(at) && line_text.is_char_boundary(s0) {
                    append_recolored(&mut job, &line_text, at, s0, &plain, &recolor);
                }
                if s1 > s0 && line_text.is_char_boundary(s0) && line_text.is_char_boundary(s1) {
                    let fmt = token_format(s.kind, font.clone(), color_of(s.kind));
                    append_recolored(&mut job, &line_text, s0, s1, &fmt, &recolor);
                    at = s1;
                }
            }
            if at < be {
                append_recolored(&mut job, &line_text, at, be, &plain, &recolor);
            }
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(Pos2::new(xcol(vr.start), y + 1.0), galley, palette_text);

            // ⌘ / Ctrl held over a symbol: underline it as a link.
            if let Some((lrow, ls, le)) = link {
                if let Some((ls, le)) = clip(ls, le).filter(|_| lrow == row) {
                    let (x0, x1) = (xcol(ls), xcol(le));
                    painter.hline(x0..=x1, y + row_height - 2.0, Stroke::new(1.0_f32, Color32::from_rgb(0x4d, 0xaa, 0xfc)));
                }
            }

            // Diagnostics underline.
            for d in &doc.diagnostics {
                let (ds, de) = (d.range.start.line as usize, d.range.end.line as usize);
                if row < ds || row > de {
                    continue;
                }
                let c0 = if row == ds { from_lsp_position(&line_text, d.range.start).col } else { 0 };
                let c1 = if row == de { from_lsp_position(&line_text, d.range.end).col } else { line_text.chars().count() };
                let Some((c0, c1)) = clip(c0, c1.max(c0 + 1)) else { continue };
                let color = match d.severity {
                    Some(DiagnosticSeverity::ERROR) => palette_err,
                    Some(DiagnosticSeverity::WARNING) => palette_warn,
                    _ => palette_gutter,
                };
                let (x0, x1) = (xcol(c0), xcol(c1));
                let yy = y + row_height - 2.0;
                painter.line_segment([Pos2::new(x0, yy), Pos2::new(x1, yy)], Stroke::new(1.0_f32, color));
            }

            // Cursor.
            for c in &all_cursors {
                let at = vim_block.unwrap_or(c.head());
                if row == at.line && has_focus_before && wl.row_of(at) == vrow {
                    let x = xcol(at.col);
                    if vim_block.is_some() {
                        // Vim outside Insert mode: a block on the character.
                        let block = Rect::from_min_size(Pos2::new(x, y + 1.0), Vec2::new(char_width, row_height - 2.0));
                        painter.rect_filled(block, 0.0, palette_cursor.gamma_multiply(0.45));
                    } else {
                        painter.rect_filled(
                            Rect::from_min_size(Pos2::new(x - 0.5, y + 1.0), Vec2::new(2.0, row_height - 2.0)),
                            0.0,
                            palette_cursor,
                        );
                    }
                    // The completion popup follows the main cursor.
                    if c.head() == head {
                        cursor_screen = Some(Pos2::new(x, y + row_height));
                    }
                }
            }
        }

        // Hover tooltip for diagnostics on the hovered line.
        if let Some(p) = resp.hover_pos() {
            let at = pos_at(&doc.buffer, p);
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
            let hrow = wl.row_of(head);
            let x = origin.x + text_x0 + wl.screen_col(hrow, head.col) as f32 * char_width;
            let y = content_top + hrow as f32 * row_height;
            let target = Rect::from_min_size(
                Pos2::new(x - char_width * 2.0, y - row_height),
                Vec2::new(char_width * 6.0, row_height * 3.0),
            );
            ui.scroll_to_rect(target, None);
        }
        doc.wrap = wl;
    });

    if mm_w > 0.0 {
        let doc = &mut app.docs[doc_idx];
        doc.minimap.scroll_y = scroll.state.offset.y;
        doc.minimap.view_h = scroll.inner_rect.height();
        let spans = doc.spans(&mut app.languages).to_vec();
        super::minimap::show(outer, mm_rect, doc, &spans, &color_of, palette_text, row_height, app.palette.border);
    }
    outer.allocate_rect(full, Sense::hover());

    // The find bar floats over the top-right of the editor.
    super::find::show(app, outer, Rect::from_min_max(full.min, Pos2::new(full.right() - mm_w, full.bottom())));
    if go_to_definition {
        app.go_to_definition();
    }
    match menu_choice {
        Some(EditorMenu::Definition) => app.go_to_definition(),
        Some(EditorMenu::References) => app.find_references(),
        Some(EditorMenu::FindInFiles) => app.pending_commands.push(Command::ShowSearch),
        Some(EditorMenu::Copy) => ui.ctx().copy_text(copy_text(&app.docs[doc_idx].buffer)),
        Some(EditorMenu::Cut) => {
            let t = cut(&mut app.docs[doc_idx].buffer);
            ui.ctx().copy_text(t);
        }
        // Arrives next frame as a paste event, which the editor handles.
        Some(EditorMenu::Paste) => {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            app.focus = Focus::Editor;
        }
        Some(EditorMenu::SelectAll) => app.pending_commands.push(Command::SelectAll),
        Some(EditorMenu::Palette) => app.pending_commands.push(Command::CommandPalette),
        None => {}
    }
    if clicked_editor
        || scroll.inner_rect.contains(ui.input(|i| i.pointer.interact_pos().unwrap_or(Pos2::ZERO)))
            && ui.input(|i| i.pointer.any_pressed())
    {
        app.focus = Focus::Editor;
    }
    if let Some(p) = cursor_screen {
        app.completion.anchor = p;
    }
    if let Some((line, at)) = gutter_hunk {
        app.hunk_menu = Some((line, at));
    }
    hunk_menu(app, ui.ctx(), gutter_hunk.is_some());
}

/// The Revert / Stage menu a change mark in the gutter opens.
fn hunk_menu(app: &mut QellApp, ctx: &egui::Context, just_opened: bool) {
    let Some((line, at)) = app.hunk_menu else { return };
    let (mut revert, mut stage, mut close) = (false, false, false);
    let area = egui::Area::new(egui::Id::new("qell-hunk-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at + Vec2::new(6.0, 6.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    revert = ui.button("Revert").on_hover_text("Put these lines back to git's version").clicked();
                    stage = ui.button("Stage").on_hover_text("Put this change into the index").clicked();
                });
            });
        });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        close = true;
    }
    // A click anywhere else closes it — but not the click that opened it.
    let outside = !area.response.rect.contains(ctx.pointer_interact_pos().unwrap_or(Pos2::ZERO));
    if !just_opened && outside && ctx.input(|i| i.pointer.any_click()) {
        close = true;
    }
    if revert {
        app.revert_hunk(line);
    }
    if stage {
        app.stage_hunk(line);
    }
    if revert || stage || close {
        app.hunk_menu = None;
    }
}

/// Cut the selection, or the whole line without one (like VS Code), and
/// return the text for the clipboard.
fn cut(b: &mut Buffer) -> String {
    let t = copy_text(b);
    if b.cursor.selection.is_empty() {
        let l = b.cursor.head().line;
        let end = if l + 1 < b.line_count() { Position::new(l + 1, 0) } else { Position::new(l, b.line_len(l)) };
        b.replace_range(Position::new(l, 0), end, "");
    } else {
        b.delete_selection();
    }
    t
}

/// What the editor's right-click menu asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorMenu {
    Definition,
    References,
    FindInFiles,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Palette,
}

fn editor_menu(ui: &mut egui::Ui, choice: &mut Option<EditorMenu>, read_only: bool) {
    let ctx = ui.ctx().clone();
    let cmd = |c: Command| c.shortcut_text(&ctx);
    let key = |k: Key| ctx.format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, k));
    let mut item = |ui: &mut egui::Ui, label: &str, shortcut: String, enabled: bool, which: EditorMenu| {
        if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(shortcut)).clicked() {
            *choice = Some(which);
            ui.close();
        }
    };
    item(ui, "Go to Definition", cmd(Command::GoToDefinition), true, EditorMenu::Definition);
    item(ui, "Find All References", cmd(Command::FindReferences), true, EditorMenu::References);
    item(ui, "Find in Files", cmd(Command::ShowSearch), true, EditorMenu::FindInFiles);
    ui.separator();
    item(ui, "Cut", key(Key::X), !read_only, EditorMenu::Cut);
    item(ui, "Copy", key(Key::C), true, EditorMenu::Copy);
    item(ui, "Paste", key(Key::V), !read_only, EditorMenu::Paste);
    item(ui, "Select All", cmd(Command::SelectAll), true, EditorMenu::SelectAll);
    ui.separator();
    item(ui, "Command Palette…", cmd(Command::CommandPalette), true, EditorMenu::Palette);
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

/// The icon for a completion kind. The shape tells the kinds apart;
/// the three tones only group them (callables, data, types).
fn kind_icon(kind: Option<CompletionItemKind>, text: Color32) -> (char, Color32) {
    let purple = Color32::WHITE;
    let blue = Color32::from_rgb(0xb4, 0xad, 0xa5);
    let orange = Color32::from_rgb(0xd9, 0xb9, 0x8a);
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
    let accent = app.palette.accent;
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
    fn word_under_cursor() {
        let l = "    let p = make_point(1, 2);";
        assert_eq!(word_range(l, 13, false), Some((12, 22))); // inside
        assert_eq!(word_range(l, 22, false), Some((12, 22))); // just after
        assert_eq!(word_range(l, 3, false), None); // in the indentation
        let tf = "  bucket = aws_s3_bucket.logs.arn";
        assert_eq!(word_range(tf, 20, true), Some((11, 33)));
        assert_eq!(word_range(tf, 20, false), Some((11, 24)));
        assert_eq!(word_range("x = var.region.", 10, true), Some((4, 14)));
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
