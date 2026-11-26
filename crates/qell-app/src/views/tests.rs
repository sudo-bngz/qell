//! The Tests tab of the bottom panel: the last run's results, failures
//! first with why, each a click away from where it failed.

use egui::{Color32, RichText, Sense};
use qell_core::testing::Status;

use crate::app::QellApp;
use crate::commands::Command;

/// Green of a passed test (the git "added" green).
pub const PASS: Color32 = Color32::from_rgb(0x9d, 0xb8, 0x82);

pub fn show(app: &mut QellApp, ui: &mut egui::Ui) {
    let (text, dim, error, accent) = (app.palette.text, app.palette.text_dim, app.palette.error, app.palette.accent);
    let size = app.font_size.min(13.0);
    let Some(run) = &app.tests.run else {
        ui.label(RichText::new("No tests have run yet.").size(size).color(dim));
        return;
    };
    // Header: what ran, how it went, and what to do next.
    let mut rerun = false;
    let mut output = false;
    ui.horizontal(|ui| {
        let state = match run.exit {
            None => RichText::new(format!("Running… {:.1}s", run.started.elapsed().as_secs_f32())).color(accent),
            Some(0) => RichText::new("✓ Passed").color(PASS),
            Some(-1) => RichText::new("Stopped").color(dim),
            Some(_) => RichText::new("✗ Failed").color(error),
        };
        ui.label(state.size(size).strong());
        if let (Some(end), Some(_)) = (run.finished, run.exit) {
            ui.label(RichText::new(format!("{:.1}s", (end - run.started).as_secs_f32())).size(size).color(dim));
        }
        ui.label(RichText::new(&run.command).size(size * 0.9).monospace().color(dim))
            .on_hover_text(run.cwd.display().to_string());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            rerun = ui.small_button("Run again").clicked();
            output = ui.small_button("Output").on_hover_text("The runner's own output, in its terminal tab").clicked();
        });
    });
    ui.add_space(2.0);
    // Failures (with their message) first, then the rest in run order.
    let mut order: Vec<usize> = (0..run.outcomes.len()).collect();
    order.sort_by_key(|&i| {
        let o = &run.outcomes[i];
        match o.status {
            Status::Failed => 0,
            Status::Running => 1,
            Status::Passed => 2,
            Status::Skipped => 3,
        }
    });
    let mut open = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if run.outcomes.is_empty() {
            let what = if run.exit.is_some() {
                "Nothing reported. The runner's output is in its terminal tab."
            } else {
                "Waiting for the first results…"
            };
            ui.label(RichText::new(what).size(size).color(dim));
        }
        for i in order {
            let o = &run.outcomes[i];
            let (mark, color) = match o.status {
                Status::Passed => ("✓", PASS),
                Status::Failed => ("✗", error),
                Status::Skipped => ("○", dim),
                Status::Running => ("…", accent),
            };
            let name = if o.name.is_empty() { "Build failed" } else { o.name.as_str() };
            let mut job = egui::text::LayoutJob::default();
            let fmt =
                |c: Color32, s: f32| egui::TextFormat { font_id: egui::FontId::proportional(s), color: c, ..Default::default() };
            job.append(mark, 0.0, fmt(color, size));
            job.append(name, 8.0, fmt(text, size));
            if let Some((path, line)) = &o.location {
                job.append(&format!("{path}:{line}"), 10.0, fmt(dim, size * 0.9));
            }
            let resp = ui.add(egui::Label::new(job).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                open = Some(i);
            }
            if o.status == Status::Failed || o.name.is_empty() {
                for m in &o.message {
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.label(RichText::new(m).size(size * 0.9).monospace().color(dim));
                    });
                }
            }
        }
    });
    if rerun {
        app.pending_commands.push(Command::TestRerun);
    }
    if output {
        let n = run.terminal;
        if let Some(i) = app.terminals.iter().position(|t| t.number == n) {
            app.active_terminal = i;
            app.tests_open = false;
        }
    }
    if let Some(i) = open {
        app.open_test_outcome(i);
    }
}
