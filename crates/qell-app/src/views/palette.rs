//! Command palette (Ctrl+Shift+P), quick open (Ctrl+P), go to line (Ctrl+G).

use std::path::{Path, PathBuf};

use egui::{Context, Key, Modifiers, Sense, Vec2};
use qell_core::Position;

use crate::app::{Focus, PaletteMode, QellApp};
use crate::commands::{fuzzy_score, Command};

/// Walk the workspace for quick open. Bounded so a huge monorepo doesn't
/// freeze the UI the first time the palette opens.
pub fn collect_files(root: &Path, ignore: &[String]) -> Vec<PathBuf> {
    const LIMIT: usize = 20_000;
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if ignore.iter().any(|i| i == &name) {
                continue;
            }
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(p);
            } else {
                out.push(p);
                if out.len() >= LIMIT {
                    return out;
                }
            }
        }
    }
    out
}

enum Choice {
    /// Index into `app.code_actions`.
    Action(usize),
    /// Index into `app.symbols`.
    Symbol(usize),
    Command(Command),
    Plugin(String),
    File(PathBuf),
}

pub fn show(app: &mut QellApp, ctx: &Context) {
    if !app.command_palette.open {
        return;
    }
    let mode = app.command_palette.mode;
    let query = app.command_palette.query.clone();
    let root = app.root.clone();

    // Build the candidate list for the current query.
    let mut choices: Vec<(String, String, Choice)> = Vec::new(); // (label, hint, choice)
    match mode {
        PaletteMode::Commands => {
            let mut scored: Vec<(usize, String, String, Choice)> = Command::ALL
                .iter()
                .filter_map(|c| {
                    fuzzy_score(&query, c.label()).map(|s| (s, c.label().to_string(), c.shortcut_text(ctx), Choice::Command(*c)))
                })
                .chain(app.plugin_commands.iter().filter_map(|n| {
                    let label = format!("Plugin: {n}");
                    fuzzy_score(&query, &label).map(|s| (s, label, String::new(), Choice::Plugin(n.clone())))
                }))
                .collect();
            scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            choices.extend(scored.into_iter().map(|(_, l, h, c)| (l, h, c)));
        }
        PaletteMode::Files => {
            let mut scored: Vec<(usize, PathBuf)> = app
                .command_palette
                .files
                .iter()
                .filter_map(|p| {
                    let rel = p.strip_prefix(&root).unwrap_or(p).to_string_lossy();
                    fuzzy_score(&query, &rel).map(|s| (s, p.clone()))
                })
                .collect();
            scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            for (_, p) in scored.into_iter().take(200) {
                let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().into_owned();
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                choices.push((name, rel, Choice::File(p)));
            }
        }
        PaletteMode::Symbols => {
            for (i, s) in app.symbols.iter().enumerate() {
                let hint = match (s.container.is_empty(), s.kind.is_empty()) {
                    (true, _) => s.kind.clone(),
                    (false, true) => s.container.clone(),
                    (false, false) => format!("{} · {}", s.container, s.kind),
                };
                if fuzzy_score(&query, &s.name).is_some() {
                    choices.push((s.name.clone(), hint, Choice::Symbol(i)));
                }
            }
        }
        PaletteMode::CodeActions => {
            use qell_lsp::lsp_types::CodeActionOrCommand as Ac;
            for (i, a) in app.code_actions.iter().enumerate() {
                let (title, kind) = match a {
                    Ac::Command(c) => (c.title.clone(), String::new()),
                    Ac::CodeAction(a) => (
                        a.title.clone(),
                        a.kind.as_ref().map(|k| k.as_str().split('.').next_back().unwrap_or("").to_string()).unwrap_or_default(),
                    ),
                };
                if fuzzy_score(&query, &title).is_some() {
                    choices.push((title, kind, Choice::Action(i)));
                }
            }
        }
        PaletteMode::GoToLine | PaletteMode::Rename => {}
    }
    if choices.is_empty() {
        app.command_palette.selected = 0;
    } else {
        app.command_palette.selected = app.command_palette.selected.min(choices.len() - 1);
    }

    // Keyboard navigation, before the text field consumes keys.
    let (up, down, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    if up {
        app.command_palette.selected = app.command_palette.selected.saturating_sub(1);
    }
    if down && !choices.is_empty() {
        app.command_palette.selected = (app.command_palette.selected + 1).min(choices.len() - 1);
    }

    let placeholder = match mode {
        PaletteMode::Commands => "Type a command…",
        PaletteMode::Files => "Search files by name…",
        PaletteMode::GoToLine => "Line number (or line:column)",
        PaletteMode::Symbols => "Go to symbol in this file…",
        PaletteMode::Rename => "New name",
        PaletteMode::CodeActions => "Quick fix…",
    };
    let text_dim = app.palette.text_dim;
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.5).clamp(360.0, 720.0);
    let mut chosen: Option<usize> = None;
    let mut close = false;

    egui::Area::new(egui::Id::new("command_palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::new(screen.center().x - width / 2.0, screen.top() + 48.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(width);
                let edit = egui::TextEdit::singleline(&mut app.command_palette.query)
                    .hint_text(placeholder)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace);
                let resp = ui.add(edit);
                if app.command_palette.needs_focus {
                    resp.request_focus();
                    app.command_palette.needs_focus = false;
                }
                if resp.changed() {
                    app.command_palette.selected = 0;
                }
                // Clicking outside closes.
                if resp.lost_focus() && ctx.input(|i| i.pointer.any_click()) && !ui.ui_contains_pointer() {
                    close = true;
                }

                ui.add_space(4.0);
                if mode == PaletteMode::GoToLine {
                    ui.label(egui::RichText::new("Enter to jump, Esc to cancel").size(11.0).color(text_dim));
                    return;
                }
                if mode == PaletteMode::Rename {
                    let hint = format!("Renaming {}, everywhere the language server finds it", app.rename_symbol);
                    ui.label(egui::RichText::new(hint).size(11.0).color(text_dim));
                    return;
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, (label, hint, _)) in choices.iter().enumerate() {
                        let is_sel = i == app.command_palette.selected;
                        let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
                        if is_sel {
                            ui.painter().rect_filled(rect, 0.0, app.palette.selection);
                        } else if r.hovered() {
                            ui.painter().rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
                        }
                        let font = egui::FontId::proportional(13.0);
                        ui.painter().text(
                            rect.left_center() + Vec2::new(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            font.clone(),
                            ui.visuals().text_color(),
                        );
                        if !hint.is_empty() {
                            ui.painter().text(
                                rect.right_center() - Vec2::new(8.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                hint,
                                egui::FontId::proportional(11.0),
                                text_dim,
                            );
                        }
                        if r.clicked() {
                            chosen = Some(i);
                        }
                        if is_sel && (up || down) {
                            r.scroll_to_me(None);
                        }
                    }
                    if choices.is_empty() {
                        ui.label(egui::RichText::new("No matches").color(text_dim));
                    }
                });
            });
        });

    if enter {
        match mode {
            PaletteMode::GoToLine => {
                let q = app.command_palette.query.trim().to_string();
                let (l, c) = q.split_once(':').map(|(l, c)| (l.trim(), c.trim())).unwrap_or((q.as_str(), "1"));
                if let Ok(line) = l.parse::<usize>() {
                    let col = c.parse::<usize>().unwrap_or(1);
                    if let Some(doc) = app.docs.get_mut(app.active) {
                        doc.buffer.set_cursor(Position::new(line.saturating_sub(1), col.saturating_sub(1)), false);
                        doc.scroll_to_cursor = true;
                    }
                }
                close = true;
            }
            PaletteMode::Rename => {
                let name = app.command_palette.query.trim().to_string();
                app.rename_to(&name);
                close = true;
            }
            _ => chosen = chosen.or(Some(app.command_palette.selected)),
        }
    }
    if let Some(i) = chosen {
        if let Some((_, _, choice)) = choices.into_iter().nth(i) {
            match choice {
                Choice::Command(c) => app.pending_commands.push(c),
                Choice::Plugin(n) => app.pending_plugin_commands.push(n),
                Choice::File(p) => app.open_file(p),
                Choice::Action(i) => app.run_code_action(i),
                Choice::Symbol(i) => app.go_to_symbol_row(i),
            }
        }
        close = true;
    }
    if close {
        app.command_palette.open = false;
        app.code_actions.clear();
        app.symbols.clear();
        if app.focus != Focus::Terminal {
            app.focus = Focus::Editor;
        }
    }
}
