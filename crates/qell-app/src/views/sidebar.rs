//! The file tree.

use std::path::{Path, PathBuf};

use egui::{Context, RichText};

use crate::app::QellApp;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

fn read_dir(path: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(path) else { return Vec::new() };
    let mut entries: Vec<Entry> = rd
        .flatten()
        .map(|e| {
            let path = e.path();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Entry { name: e.file_name().to_string_lossy().into_owned(), path, is_dir }
        })
        .collect();
    // Directories first, then case-insensitive by name — like every file explorer.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries
}

fn show_dir(app: &mut QellApp, ui: &mut egui::Ui, dir: &Path) {
    if !app.tree_cache.contains_key(dir) {
        let entries = read_dir(dir);
        app.tree_cache.insert(dir.to_path_buf(), entries);
    }
    let entries = app.tree_cache.get(dir).cloned().unwrap_or_default();
    let active_path = app.docs.get(app.active).and_then(|d| d.buffer.path().map(Path::to_path_buf));

    for e in entries {
        if e.is_dir {
            let id = ui.make_persistent_id(&e.path);
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    ui.label(RichText::new(&e.name).size(13.0));
                })
                .body_unindented(|ui| {
                    ui.indent(id.with("body"), |ui| show_dir(app, ui, &e.path));
                });
        } else {
            let selected = active_path.as_deref() == Some(e.path.as_path());
            let text = RichText::new(&e.name).size(13.0);
            let resp = ui.add(egui::Button::selectable(selected, text));
            if resp.clicked() {
                app.open_file(e.path.clone());
            }
        }
    }
}

pub fn show(app: &mut QellApp, ctx: &Context) {
    let bg = app.palette.panel_bg;
    egui::SidePanel::left("sidebar")
        .default_width(240.0)
        .width_range(160.0..=600.0)
        .frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(4, 6)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let title = app.root.file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_default();
                ui.label(RichText::new(title).size(11.0).color(app.palette.text_dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                        app.tree_cache.clear();
                        app.command_palette.files.clear();
                    }
                });
            });
            ui.add_space(4.0);
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let root = app.root.clone();
                show_dir(app, ui, &root);
            });
        });
}
