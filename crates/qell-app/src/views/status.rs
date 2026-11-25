//! The status bar: cursor position, language, LSP state, notifications.

use egui::{Context, RichText};
use qell_lsp::lsp_types::DiagnosticSeverity;

use crate::app::QellApp;

pub fn show(app: &mut QellApp, ctx: &Context) {
    let palette_text_dim = app.palette.text_dim;
    let error_color = app.palette.error;
    let warning_color = app.palette.warning;
    let accent = app.palette.accent;
    let mut copied = false;

    egui::TopBottomPanel::bottom("status_bar").exact_height(22.0).show(ctx, |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let small = |t: String| RichText::new(t).size(12.0);

            // Left: workspace + diagnostics.
            let root = app.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(small(root).color(palette_text_dim));

            if let Some(ws) = app.git_status.as_ref().filter(|_| app.config.git_status) {
                let repos = ws.repos.len();
                let label = match app.active_repo() {
                    Some((root, git)) => {
                        let mut text = git.branch.as_deref().unwrap_or("detached").to_string();
                        // Several repositories: say which one the branch is of.
                        if repos > 1 && !root.as_os_str().is_empty() {
                            text = format!("{}: {text}", root.display());
                        }
                        if git.ahead > 0 {
                            text.push_str(&format!(" ↑{}", git.ahead));
                        }
                        if git.behind > 0 {
                            text.push_str(&format!(" ↓{}", git.behind));
                        }
                        text
                    }
                    None if ws.truncated => format!("{repos}+ repositories"),
                    None => format!("{repos} repositories"),
                };
                // The Powerline branch glyph lives in the bundled monospace font.
                ui.add_space(-8.0);
                ui.label(RichText::new("\u{e0a0}").monospace().size(12.0).color(palette_text_dim));
                ui.add_space(-10.0);
                let changed = ws.changed_files();
                let resp = ui.label(small(label).color(palette_text_dim));
                let mut tip = format!("{changed} changed file{}", if changed == 1 { "" } else { "s" });
                if repos > 1 {
                    tip.push_str(&format!(" in {repos} repositories"));
                }
                if ws.truncated {
                    tip.push_str(&format!(
                        "\nThere are more below this folder; only the first {repos} are followed. Open a narrower folder to see the others."
                    ));
                }
                resp.on_hover_text(tip);
            }

            if let Some(doc) = app.docs.get(app.active) {
                let errors = doc.diagnostics.iter().filter(|d| d.severity == Some(DiagnosticSeverity::ERROR)).count();
                let warnings = doc.diagnostics.iter().filter(|d| d.severity == Some(DiagnosticSeverity::WARNING)).count();
                if errors + warnings > 0 {
                    ui.label(small(format!("✖ {errors}")).color(error_color));
                    ui.label(small(format!("⚠ {warnings}")).color(warning_color));
                }
            }

            // Middle: most recent notification.
            if let Some(n) = app.notifications.last() {
                let color = if n.is_error { error_color } else { palette_text_dim };
                ui.label(small(n.text.clone()).color(color));
            }

            // Right: position, language, LSP.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if let Some(doc) = app.docs.get(app.active) {
                    let p = doc.buffer.cursor.head();
                    let lang_id = doc.buffer.language().unwrap_or("plaintext");
                    let lang_name = app.languages.get(lang_id).map(|l| l.name).unwrap_or("Plain Text");
                    // A picture has no language, no server and no cursor;
                    // what it is, is written above it instead.
                    if doc.media.is_some() {
                        return;
                    }
                    let lsp = app.lsp.status(lang_id);
                    let lsp_color = match lsp {
                        "ready" => accent,
                        "unavailable" => error_color,
                        _ => palette_text_dim,
                    };
                    match app.lsp.missing_program(lang_id) {
                        Some(program) => {
                            let hint = qell_lsp::install_hint(program);
                            let mut tip = format!(
                                "{lang_name} completion and diagnostics come from the {program} language server, which isn't installed.\n\
                                 Highlighting works without it, and completion falls back to words from the file.\n\n"
                            );
                            match hint {
                                Some(h) => {
                                    tip.push_str(&format!("To install it, run:\n    {}\n", h.command));
                                    if let Some(note) = h.note {
                                        tip.push_str(&format!("{note}\n"));
                                    }
                                    tip.push_str("Then restart Qell.");
                                }
                                None => tip.push_str(&format!("Install {program} and restart Qell.")),
                            }
                            tip.push_str("\n\nAlready installed elsewhere? Set its path in Settings > Language Servers.");
                            if hint.is_some() {
                                tip.push_str("\n\nClick to copy the install command.");
                            }
                            let resp = ui
                                .add(egui::Label::new(small("LSP not installed".into()).color(palette_text_dim)).sense(egui::Sense::click()))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(tip);
                            if let (true, Some(h)) = (resp.clicked(), hint) {
                                ui.ctx().copy_text(h.command.to_string());
                                copied = true;
                            }
                        }
                        None => {
                            // Which project the server was started in,
                            // which isn't always the folder you opened.
                            let tip = match app.lsp.server_root(lang_id) {
                                Some(root) if root != app.lsp.root() => {
                                    format!("{lang_name} language server, rooted at {}", root.display())
                                }
                                _ => format!("{lang_name} language server"),
                            };
                            ui.label(small(format!("LSP {lsp}")).color(lsp_color)).on_hover_text(tip);
                        }
                    }
                    ui.label(small(lang_name.to_string()));
                    let extra = doc.buffer.extra_cursors().len();
                    if extra > 0 {
                        ui.label(small(format!("{} cursors", extra + 1)));
                    }
                    ui.label(small(format!("Ln {}, Col {}", p.line + 1, p.col + 1)));
                }
            });
        });
    });
    if copied {
        app.notify("Install command copied: paste it in a terminal.");
    }
}
