//! The status bar: cursor position, language, LSP state, notifications.

use egui::{Context, RichText};
use qell_lsp::lsp_types::DiagnosticSeverity;

use crate::app::QellApp;

pub fn show(app: &mut QellApp, ctx: &Context) {
    let palette_text_dim = app.palette.text_dim;
    let error_color = app.palette.error;
    let warning_color = app.palette.warning;
    let accent = app.palette.accent;

    egui::TopBottomPanel::bottom("status_bar").exact_height(22.0).show(ctx, |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let small = |t: String| RichText::new(t).size(12.0);

            // Left: workspace + diagnostics.
            let root = app.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(small(root).color(palette_text_dim));

            if let Some(git) = app.git_status.as_ref().filter(|_| app.config.git_status) {
                // The Powerline branch glyph lives in the bundled monospace font.
                ui.add_space(-8.0);
                ui.label(RichText::new("\u{e0a0}").monospace().size(12.0).color(palette_text_dim));
                ui.add_space(-10.0);
                let mut text = git.branch.as_deref().unwrap_or("detached").to_string();
                if git.ahead > 0 {
                    text.push_str(&format!(" ↑{}", git.ahead));
                }
                if git.behind > 0 {
                    text.push_str(&format!(" ↓{}", git.behind));
                }
                let changed = git.changed_files();
                let resp = ui.label(small(text).color(palette_text_dim));
                if changed > 0 {
                    resp.on_hover_text(format!("{changed} changed file{}", if changed == 1 { "" } else { "s" }));
                }
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
                    let lsp = app.lsp.status(lang_id);
                    let lsp_color = match lsp {
                        "ready" => accent,
                        "unavailable" => error_color,
                        _ => palette_text_dim,
                    };
                    ui.label(small(format!("LSP {lsp}")).color(lsp_color));
                    ui.label(small(lang_name.to_string()));
                    ui.label(small(format!("Ln {}, Col {}", p.line + 1, p.col + 1)));
                }
            });
        });
    });
}
