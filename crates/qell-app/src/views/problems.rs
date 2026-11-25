//! The Problems tab of the bottom panel: every error and warning the
//! language servers report, by file, each a click away from its line.

use std::path::{Path, PathBuf};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, Sense};
use qell_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

use crate::app::QellApp;
use crate::icons;

/// Files with errors first, then those with only warnings; by path within.
pub fn ordered(problems: &std::collections::HashMap<PathBuf, Vec<Diagnostic>>) -> Vec<(&PathBuf, &Vec<Diagnostic>)> {
    let mut files: Vec<_> = problems.iter().collect();
    let has_error = |v: &Vec<Diagnostic>| v.iter().any(|d| d.severity == Some(DiagnosticSeverity::ERROR));
    files.sort_by(|a, b| has_error(b.1).cmp(&has_error(a.1)).then_with(|| a.0.cmp(b.0)));
    files
}

fn source_of(d: &Diagnostic) -> String {
    let code = match &d.code {
        Some(NumberOrString::Number(n)) => Some(n.to_string()),
        Some(NumberOrString::String(s)) => Some(s.clone()),
        None => None,
    };
    match (d.source.as_deref(), code) {
        (Some(src), Some(code)) => format!("{src}({code})"),
        (Some(src), None) => src.to_string(),
        (None, Some(code)) => code,
        (None, None) => String::new(),
    }
}

pub fn show(app: &mut QellApp, ui: &mut egui::Ui) {
    let (text, dim, error, warning) = (app.palette.text, app.palette.text_dim, app.palette.error, app.palette.warning);
    let size = app.font_size.min(13.0);
    if app.problems.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("No problems reported.").size(size).color(dim)).on_hover_text(
            "Errors and warnings appear here as language servers report them. Some check the whole project, \
             others only the files you have open.",
        );
        return;
    }
    let root = app.root.clone();
    let mut go: Option<(PathBuf, Diagnostic)> = None;
    let files: Vec<(PathBuf, Vec<Diagnostic>)> =
        ordered(&app.problems).into_iter().map(|(p, v)| (p.clone(), v.clone())).collect();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for (path, problems) in &files {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let dir = rel.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.display().to_string());
            let (icon, icon_color) = icons::for_file(path);
            let id = ui.make_persistent_id(("problems", path));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    let mut job = LayoutJob::default();
                    let f = |font_id: FontId, color: Color32| TextFormat { font_id, color, ..Default::default() };
                    job.append(&icon.to_string(), 0.0, f(icons::font(size + 1.0), icon_color));
                    job.append(&name, 6.0, f(FontId::proportional(size), text));
                    if let Some(dir) = &dir {
                        job.append(dir, 8.0, f(FontId::proportional(size - 1.0), dim));
                    }
                    job.append(&problems.len().to_string(), 8.0, f(FontId::proportional(size - 2.0), dim));
                    ui.label(job);
                })
                .body(|ui| {
                    for d in problems {
                        let (mark, color) = match d.severity {
                            Some(DiagnosticSeverity::ERROR) => ("✖", error),
                            Some(DiagnosticSeverity::WARNING) => ("⚠", warning),
                            _ => ("ℹ", dim),
                        };
                        let what = d.message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
                        let mut job = LayoutJob::default();
                        let f =
                            |color: Color32, s: f32| TextFormat { font_id: FontId::proportional(s), color, ..Default::default() };
                        job.append(mark, 0.0, f(color, size));
                        job.append(what, 8.0, f(text, size));
                        let src = source_of(d);
                        if !src.is_empty() {
                            job.append(&src, 8.0, f(dim, size - 1.0));
                        }
                        job.append(
                            &format!("[Ln {}, Col {}]", d.range.start.line + 1, d.range.start.character + 1),
                            8.0,
                            f(dim, size - 1.0),
                        );
                        let resp = ui
                            .add(egui::Label::new(job).truncate().sense(Sense::click()).selectable(false))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(d.message.as_str());
                        if resp.clicked() {
                            go = Some((path.clone(), d.clone()));
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Copy Message").clicked() {
                                ui.ctx().copy_text(d.message.clone());
                                ui.close();
                            }
                        });
                    }
                });
        }
    });
    if let Some((path, d)) = go {
        app.go_to_problem(Path::new(&path), &d, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qell_lsp::lsp_types::{Position, Range};
    use std::collections::HashMap;

    fn diag(severity: DiagnosticSeverity) -> Diagnostic {
        Diagnostic { range: Range::new(Position::new(0, 0), Position::new(0, 1)), severity: Some(severity), ..Default::default() }
    }

    #[test]
    fn files_with_errors_come_first() {
        let problems = HashMap::from([
            (PathBuf::from("/ws/a.go"), vec![diag(DiagnosticSeverity::WARNING)]),
            (PathBuf::from("/ws/b.go"), vec![diag(DiagnosticSeverity::ERROR)]),
            (PathBuf::from("/ws/c.go"), vec![diag(DiagnosticSeverity::WARNING), diag(DiagnosticSeverity::ERROR)]),
        ]);
        let order: Vec<_> =
            ordered(&problems).into_iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(order, ["b.go", "c.go", "a.go"]);
    }

    #[test]
    fn the_source_names_the_tool_and_code() {
        let mut d = diag(DiagnosticSeverity::ERROR);
        assert_eq!(source_of(&d), "");
        d.source = Some("compiler".into());
        d.code = Some(NumberOrString::String("UndeclaredName".into()));
        assert_eq!(source_of(&d), "compiler(UndeclaredName)");
    }
}
