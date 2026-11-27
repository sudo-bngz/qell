//! The integrated terminal panel.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, Event, Key, Pos2, Rect, Sense, Stroke, Vec2};
use qell_term::{Key as TermKey, SelectionKind, Snapshot, Terminal};

use crate::app::{Focus, QellApp};
use crate::commands::Command;

pub fn show(app: &mut QellApp, ctx: &Context) {
    let bg = app.palette.panel_bg;
    let border = app.palette.border;
    egui::TopBottomPanel::bottom("terminal_panel")
        .resizable(true)
        .default_height(240.0)
        .height_range(80.0..=1200.0)
        .frame(egui::Frame::new().fill(bg).stroke(Stroke::new(1.0_f32, border)).inner_margin(egui::Margin::symmetric(6, 4)))
        .show(ctx, |ui| {
            // A click anywhere in the panel (padding included) focuses the
            // shell, not only on the character grid. Registered first so
            // the tabs and buttons on top keep their own clicks. Covers the
            // padding (6 px sides, 4 px top) except the resize handles.
            let panel = ui.max_rect().expand2(Vec2::new(3.0, 1.0));
            if ui.interact(panel, ui.id().with("terminal_panel_bg"), Sense::click()).clicked()
                && !app.problems_open
                && !app.tests_open
            {
                app.focus = Focus::Terminal;
            }
            show_header(app, ui);
            ui.add_space(2.0);
            if app.problems_open {
                super::problems::show(app, ui);
            } else if app.tests_open {
                super::tests::show(app, ui);
            } else {
                show_grid(app, ui);
            }
        });
}

/// Tab strip on the left, panel actions on the right.
fn show_header(app: &mut QellApp, ui: &mut egui::Ui) {
    let text = app.palette.text;
    let text_dim = app.palette.text_dim;
    let accent = app.palette.accent;
    let multiple = app.terminals.len() > 1;

    ui.horizontal(|ui| {
        // Actions first (right-to-left) so the tabs get what's left.
        let mut new = false;
        let mut restart = false;
        let mut hide = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            hide = ui.small_button("×").on_hover_text("Hide panel").clicked();
            if !app.problems_open && !app.tests_open {
                restart = ui.small_button("⟳").on_hover_text("Restart this shell").clicked();
            }
            let new_tip = format!("New terminal ({})", Command::NewTerminal.shortcut_text(ui.ctx()));
            new = ui.small_button("+").on_hover_text(new_tip).clicked();

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                egui::ScrollArea::horizontal().id_salt("terminal_tabs").auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        // Problems first, then the shells, as in VS Code.
                        problems_tab(app, ui);
                        tests_tab(app, ui);
                        let mut switch_to = None;
                        let mut kill = None;
                        for (i, tab) in app.terminals.iter().enumerate() {
                            let active = i == app.active_terminal && !app.problems_open && !app.tests_open;
                            let full = tab.label();
                            let label =
                                if multiple { format!("{}: {}", tab.number, shorten(&full, 32)) } else { shorten(&full, 48) };
                            let color = if active { text } else { text_dim };
                            let inner = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.label(egui::RichText::new(label).size(11.0).color(color));
                                    // Only offer × per tab when there's a choice;
                                    // the panel × covers the single-shell case.
                                    multiple.then(|| ui.small_button("×").rect)
                                })
                                .inner
                            });
                            // The tab sits above its ×: decide by click position.
                            let close_rect = inner.inner;
                            let resp = inner
                                .response
                                .interact(Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(full);
                            if resp.clicked() {
                                let on_close = resp.interact_pointer_pos().zip(close_rect).is_some_and(|(p, r)| r.contains(p));
                                if on_close {
                                    kill = Some(i);
                                } else {
                                    switch_to = Some(i);
                                }
                            }
                            if resp.middle_clicked() {
                                kill = Some(i);
                            }
                            if active {
                                let r = resp.rect;
                                ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.5_f32, accent));
                            }
                        }
                        if let Some(i) = switch_to {
                            app.active_terminal = i;
                            app.problems_open = false;
                            app.tests_open = false;
                            app.focus = Focus::Terminal;
                        }
                        if let Some(i) = kill {
                            app.kill_terminal(i);
                        }
                    });
                });
            });
        });
        if new {
            app.problems_open = false;
            app.tests_open = false;
            app.new_terminal();
            app.focus = Focus::Terminal;
        }
        if restart {
            app.restart_terminal(app.active_terminal);
        }
        if hide {
            app.show_terminal = false;
            app.focus = Focus::Editor;
        }
    });
}

/// The Problems tab: how many errors and warnings, and opens the list.
fn problems_tab(app: &mut QellApp, ui: &mut egui::Ui) {
    use qell_lsp::lsp_types::DiagnosticSeverity;
    let all = app.problems.values().flatten();
    let (mut errors, mut warnings) = (0, 0);
    for d in all {
        match d.severity {
            Some(DiagnosticSeverity::ERROR) => errors += 1,
            Some(DiagnosticSeverity::WARNING) => warnings += 1,
            _ => {}
        }
    }
    let open = app.problems_open;
    let fmt = |color: Color32| egui::text::TextFormat { font_id: egui::FontId::proportional(11.0), color, ..Default::default() };
    let mut job = egui::text::LayoutJob::default();
    job.append("PROBLEMS", 0.0, fmt(if open { app.palette.text } else { app.palette.text_dim }));
    if errors > 0 {
        job.append(&errors.to_string(), 6.0, fmt(app.palette.error));
    }
    if warnings > 0 {
        job.append(&warnings.to_string(), 6.0, fmt(app.palette.warning));
    }
    let inner = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| ui.label(job));
    let shortcut = Command::ShowProblems.shortcut_text(ui.ctx());
    let resp = inner
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("Errors and warnings from the language servers ({shortcut})"));
    if resp.clicked() {
        app.problems_open = true;
        app.tests_open = false;
        if app.focus == Focus::Terminal {
            app.focus = Focus::Editor;
        }
    }
    if open {
        let r = resp.rect;
        ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.5_f32, app.palette.accent));
    }
    ui.add_space(6.0);
}

/// The Tests tab, once tests have run: how many passed and failed.
fn tests_tab(app: &mut QellApp, ui: &mut egui::Ui) {
    use qell_core::testing::Status;
    let Some(run) = &app.tests.run else { return };
    let named = || run.outcomes.iter().filter(|o| !o.name.is_empty());
    let passed = named().filter(|o| o.status == Status::Passed).count();
    let failed =
        named().filter(|o| o.status == Status::Failed).count() + run.outcomes.iter().filter(|o| o.name.is_empty()).count();
    let open = app.tests_open;
    let fmt = |color: Color32| egui::text::TextFormat { font_id: egui::FontId::proportional(11.0), color, ..Default::default() };
    let mut job = egui::text::LayoutJob::default();
    job.append("TESTS", 0.0, fmt(if open { app.palette.text } else { app.palette.text_dim }));
    if run.exit.is_none() {
        job.append("…", 6.0, fmt(app.palette.accent));
    }
    if failed > 0 {
        job.append(&format!("✗ {failed}"), 6.0, fmt(app.palette.error));
    }
    if passed > 0 {
        job.append(&format!("✓ {passed}"), 6.0, fmt(super::tests::PASS));
    }
    let inner = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| ui.label(job));
    let resp = inner
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Results of the last test run");
    if resp.clicked() {
        app.tests_open = true;
        app.problems_open = false;
        if app.focus == Focus::Terminal {
            app.focus = Focus::Editor;
        }
    }
    if open {
        let r = resp.rect;
        ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.5_f32, app.palette.accent));
    }
    ui.add_space(6.0);
}

/// Trim long shell titles ("user@host: /a/very/long/path") from the left,
/// where the least useful part is.
fn shorten(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}

/// Right-click menu of the terminal.
fn context_menu(ui: &mut egui::Ui, term: &Terminal, focus: &mut Focus) {
    let copy = egui::Button::new("Copy").shortcut_text(if cfg!(target_os = "macos") { "⌘C" } else { "Ctrl+Shift+C" });
    if ui.add_enabled(term.selection_text().is_some(), copy).clicked() {
        if let Some(t) = term.selection_text() {
            ui.ctx().copy_text(t);
        }
        ui.close();
    }
    let paste = egui::Button::new("Paste").shortcut_text(if cfg!(target_os = "macos") { "⌘V" } else { "Ctrl+Shift+V" });
    if ui.add(paste).clicked() {
        // Arrives next frame as a paste event, which the terminal handles.
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        *focus = Focus::Terminal;
        ui.close();
    }
    if ui.button("Select All").clicked() {
        term.select_all();
        ui.close();
    }
    ui.separator();
    if ui.button("Clear Terminal").clicked() {
        term.clear();
        ui.close();
    }
}

fn to_color(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn show_grid(app: &mut QellApp, ui: &mut egui::Ui) {
    let active = app.active_terminal;
    let modal = app.modal_open();
    let Some(term) = app.terminals.get_mut(active).map(|t| &mut t.term) else {
        ui.label("Starting shell…");
        return;
    };
    let size = if app.config.terminal_font_size > 0.0 { app.config.terminal_font_size } else { app.font_size };
    let font = app.palette.mono(size);
    let (row_height, char_width) = ui.fonts_mut(|f| (f.row_height(&font).ceil(), f.glyph_width(&font, 'M')));

    let avail = ui.available_size();
    let cols = ((avail.x / char_width).floor() as usize).max(2);
    let rows = ((avail.y / row_height).floor() as usize).max(1);
    term.resize(cols, rows);

    let (rect, resp) = ui.allocate_exact_size(avail, Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() {
        app.focus = Focus::Terminal;
    }
    let focused = app.focus == Focus::Terminal && !modal;
    if focused {
        super::hold_keyboard(ui, ui.id().with("terminal_keyboard"), rect.min);
    }

    // ---- mouse selection ----
    // Screen cell under a point, and whether it's the cell's right half.
    let cell_at = |p: Pos2| {
        let fx = ((p.x - rect.left()) / char_width).max(0.0);
        let col = (fx.floor() as usize).min(cols - 1);
        let row = (((p.y - rect.top()) / row_height).floor().max(0.0) as usize).min(rows - 1);
        (col, row, fx.fract() > 0.5)
    };
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.triple_clicked() {
            let (c, r, _) = cell_at(p);
            term.select_start(c, r, false, SelectionKind::Lines);
        } else if resp.double_clicked() {
            let (c, r, _) = cell_at(p);
            term.select_start(c, r, false, SelectionKind::Words);
        } else if resp.drag_started() {
            // From where the button went down, not where the drag was noticed.
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            let (c, r, right) = cell_at(origin);
            term.select_start(c, r, right, SelectionKind::Chars);
        } else if resp.dragged() {
            // Past the top or bottom edge: scroll to extend the selection.
            if p.y < rect.top() {
                term.scroll(1);
            } else if p.y > rect.bottom() {
                term.scroll(-1);
            }
            let (c, r, right) = cell_at(p);
            term.select_update(c, r, right);
            ui.ctx().request_repaint();
        } else if resp.clicked() {
            term.clear_selection();
        }
    }
    resp.context_menu(|ui| context_menu(ui, term, &mut app.focus));

    // ---- input ----
    if focused {
        let events = ui.input(|i| i.events.clone());
        let events = crate::views::ime::as_text(events);
        let mut used = Vec::new();
        for (idx, ev) in events.iter().enumerate() {
            match ev {
                Event::Text(t) => {
                    let mods = ui.input(|i| i.modifiers);
                    if mods.ctrl && !mods.alt {
                        continue;
                    }
                    term.clear_selection();
                    term.send_text(t);
                    term.scroll_to_bottom();
                }
                Event::Paste(t) => {
                    term.clear_selection();
                    term.send_text(t);
                }
                // With a selection, Copy copies it: ⌘C on macOS, and Ctrl+C
                // or Ctrl+Shift+C elsewhere (as in VS Code on Windows).
                Event::Copy if term.selection_text().is_some() => {
                    if let Some(t) = term.selection_text() {
                        ui.ctx().copy_text(t);
                    }
                }
                // On Linux and Windows, Ctrl+C reaches us as Copy: in a
                // terminal it means interrupt, as in VS Code. On macOS Copy
                // is ⌘C (Ctrl+C arrives as a key and is handled below), and
                // there is no selection to copy, so it does nothing.
                Event::Copy if !cfg!(target_os = "macos") => term.send_key(TermKey::Ctrl('c')),
                Event::Cut if !cfg!(target_os = "macos") => term.send_key(TermKey::Ctrl('x')),
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let k = match key {
                        Key::Enter => Some(TermKey::Enter),
                        Key::Backspace => Some(TermKey::Backspace),
                        Key::Tab => Some(TermKey::Tab),
                        Key::Escape => Some(TermKey::Escape),
                        Key::ArrowUp => Some(TermKey::Up),
                        Key::ArrowDown => Some(TermKey::Down),
                        Key::ArrowLeft => Some(TermKey::Left),
                        Key::ArrowRight => Some(TermKey::Right),
                        Key::Home => Some(TermKey::Home),
                        Key::End => Some(TermKey::End),
                        Key::PageUp => Some(TermKey::PageUp),
                        Key::PageDown => Some(TermKey::PageDown),
                        Key::Delete => Some(TermKey::Delete),
                        Key::Insert => Some(TermKey::Insert),
                        _ if modifiers.ctrl && !modifiers.shift => {
                            // Ctrl+letter: egui names keys A..Z; take the first char.
                            let name = key.name();
                            let ch = name.chars().next().filter(|_| name.len() == 1);
                            ch.map(|c| TermKey::Ctrl(c.to_ascii_lowercase()))
                        }
                        _ => None,
                    };
                    if let Some(k) = k {
                        if modifiers.shift && matches!(k, TermKey::PageUp | TermKey::PageDown) {
                            term.scroll(if k == TermKey::PageUp { rows as i32 } else { -(rows as i32) });
                        } else {
                            term.send_key(k);
                            term.scroll_to_bottom();
                        }
                    } else {
                        continue;
                    }
                }
                _ => continue,
            }
            used.push(idx);
        }
        if !used.is_empty() {
            ui.input_mut(|i| {
                let mut n = 0;
                i.events.retain(|_| {
                    let keep = !used.contains(&n);
                    n += 1;
                    keep
                });
            });
        }
    }
    if resp.hovered() {
        let dy = ui.input(|i| i.raw_scroll_delta.y);
        if dy.abs() > 0.0 {
            term.scroll((dy / row_height * 2.0).round() as i32);
        }
    }

    // ---- paint ----
    let snap: Snapshot = term.snapshot();
    let painter = ui.painter_at(rect);
    let default_bg = app.palette.panel_bg;
    for (r, row) in snap.cells.iter().enumerate().take(rows) {
        let y = rect.top() + r as f32 * row_height;
        let mut job = LayoutJob::default();
        let mut run = String::new();
        let mut run_fmt: Option<TextFormat> = None;
        let flush = |job: &mut LayoutJob, run: &mut String, fmt: &Option<TextFormat>| {
            if let (Some(f), false) = (fmt, run.is_empty()) {
                job.append(run, 0.0, f.clone());
                run.clear();
            }
        };
        for (col, cell) in row.iter().take(cols).enumerate() {
            let bg = if cell.selected { app.palette.selection } else { to_color(cell.bg) };
            if bg != default_bg {
                let x = rect.left() + col as f32 * char_width;
                painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new(char_width, row_height)), 0.0, bg);
            }
            let fmt = TextFormat {
                font_id: font.clone(),
                color: to_color(cell.fg),
                italics: cell.italic,
                underline: if cell.underline { Stroke::new(1.0_f32, to_color(cell.fg)) } else { Stroke::NONE },
                ..Default::default()
            };
            let same = run_fmt
                .as_ref()
                .is_some_and(|f| f.color == fmt.color && f.italics == fmt.italics && f.underline == fmt.underline);
            if !same {
                flush(&mut job, &mut run, &run_fmt);
                run_fmt = Some(fmt);
            }
            // A wide glyph already occupies two cells; its spacer draws nothing.
            if !cell.wide_spacer {
                run.push(cell.c);
            }
        }
        flush(&mut job, &mut run, &run_fmt);
        if job.text.trim().is_empty() {
            continue;
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(Pos2::new(rect.left(), y), galley, app.palette.text);
    }

    // Cursor.
    if let Some((c, r)) = snap.cursor {
        if r < rows && c < cols {
            let x = rect.left() + c as f32 * char_width;
            let y = rect.top() + r as f32 * row_height;
            let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::new(char_width, row_height));
            if focused {
                // Dead keys and input methods (⌥N then a space for ~ on a
                // Mac) compose at the cursor.
                crate::views::ime::allow(ui.ctx(), rect, cell);
                painter.rect_filled(cell, 0.0, app.palette.cursor.gamma_multiply(0.8));
                if let Some(ch) = snap.cells.get(r).and_then(|row| row.get(c)).map(|cc| cc.c) {
                    painter.text(cell.center(), egui::Align2::CENTER_CENTER, ch, font.clone(), app.palette.panel_bg);
                }
            } else {
                painter.rect_stroke(cell, 0.0, Stroke::new(1.0_f32, app.palette.cursor), egui::StrokeKind::Inside);
            }
        }
    }
    if snap.display_offset > 0 {
        painter.text(
            rect.right_top() + Vec2::new(-6.0, 2.0),
            egui::Align2::RIGHT_TOP,
            format!("↑ {}", snap.display_offset),
            egui::FontId::proportional(11.0),
            app.palette.text_dim,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn shorten_keeps_the_end() {
        assert_eq!(shorten("bash", 10), "bash");
        assert_eq!(shorten("user@host: /home/me/project", 10), "…e/project");
        assert_eq!(shorten("user@host: /home/me/project", 10).chars().count(), 10);
    }
}
