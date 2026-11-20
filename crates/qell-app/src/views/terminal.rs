//! The integrated terminal panel.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, Event, Key, Pos2, Rect, Sense, Stroke, Vec2};
use qell_term::{Key as TermKey, Snapshot};

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
            if ui.interact(panel, ui.id().with("terminal_panel_bg"), Sense::click()).clicked() {
                app.focus = Focus::Terminal;
            }
            show_header(app, ui);
            ui.add_space(2.0);
            show_grid(app, ui);
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
            restart = ui.small_button("⟳").on_hover_text("Restart this shell").clicked();
            let new_tip = format!("New terminal ({})", Command::NewTerminal.shortcut_text(ui.ctx()));
            new = ui.small_button("+").on_hover_text(new_tip).clicked();

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                egui::ScrollArea::horizontal().id_salt("terminal_tabs").auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let mut switch_to = None;
                        let mut kill = None;
                        for (i, tab) in app.terminals.iter().enumerate() {
                            let active = i == app.active_terminal;
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
                            if active && multiple {
                                let r = resp.rect;
                                ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.5_f32, accent));
                            }
                        }
                        if let Some(i) = switch_to {
                            app.active_terminal = i;
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

    // ---- input ----
    if focused {
        let events = ui.input(|i| i.events.clone());
        let mut used = Vec::new();
        for (idx, ev) in events.iter().enumerate() {
            match ev {
                Event::Text(t) => {
                    let mods = ui.input(|i| i.modifiers);
                    if mods.ctrl && !mods.alt {
                        continue;
                    }
                    term.send_text(t);
                    term.scroll_to_bottom();
                }
                Event::Paste(t) => term.send_text(t),
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
            let bg = to_color(cell.bg);
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
