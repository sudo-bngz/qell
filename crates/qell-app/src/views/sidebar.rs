//! The file tree.

use std::path::{Path, PathBuf};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, FontId, RichText};
use qell_git::FileStatus;

use crate::app::QellApp;
use crate::icons;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

fn read_dir(path: &Path, hidden: &[String]) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(path) else { return Vec::new() };
    let mut entries: Vec<Entry> = rd
        .flatten()
        .filter(|e| !hidden.iter().any(|h| *h == *e.file_name().to_string_lossy()))
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

/// Icon glyph followed by the name, in one label.
/// `None` for the icon (icons turned off in Settings) gives the bare name.
pub fn icon_label(icon: Option<(char, Color32)>, name: &str, name_color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut gap = 0.0;
    if let Some((icon, icon_color)) = icon {
        job.append(&icon.to_string(), 0.0, TextFormat { font_id: icons::font(14.0), color: icon_color, ..Default::default() });
        gap = 6.0;
    }
    job.append(name, gap, TextFormat { font_id: FontId::proportional(13.0), color: name_color, ..Default::default() });
    job
}

fn show_dir(app: &mut QellApp, ui: &mut egui::Ui, dir: &Path) {
    if !app.tree_cache.contains_key(dir) {
        let entries = read_dir(dir, &app.config.tree_hidden);
        app.tree_cache.insert(dir.to_path_buf(), entries);
    }
    let entries = app.tree_cache.get(dir).cloned().unwrap_or_default();
    let active_path = app.docs.get(app.active).and_then(|d| d.buffer.path().map(Path::to_path_buf));

    let default_color = app.palette.text;
    let folder_color = app.palette.text_dim;
    let show_icons = app.config.file_icons;
    for e in entries {
        let git = app.git_status_of(&e.path);
        let color = git.map(|s| app.palette.git_color(s));
        let name_color = color.unwrap_or(default_color);
        let tooltip = git.filter(|s| *s != FileStatus::Ignored).map(|s| s.description());

        if e.is_dir {
            let id = ui.make_persistent_id(&e.path);
            let is_open = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false).is_open();
            let icon = if is_open { icons::FOLDER_OPEN } else { icons::FOLDER };
            let label = icon_label(show_icons.then_some((icon, folder_color)), &e.name, name_color);
            let (_, header, _) = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    let clicked = ui.add(egui::Label::new(label).sense(egui::Sense::click())).clicked();
                    // Folders get a dot rather than a letter, like VS Code.
                    if let (Some(c), Some(_)) = (color, tooltip) {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(6.0);
                            // Monospace: the dot only exists in the bundled Hack font.
                            ui.label(RichText::new("●").monospace().size(10.0).color(c));
                        });
                    }
                    clicked
                })
                .body_unindented(|ui| {
                    ui.indent(id.with("body"), |ui| show_dir(app, ui, &e.path));
                });
            // Clicking the name opens/closes the folder, not just the arrow.
            if header.inner {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
                state.toggle(ui);
                state.store(ui.ctx());
            }
            if let Some(t) = tooltip {
                header.response.on_hover_text(t);
            }
        } else {
            let selected = active_path.as_deref() == Some(e.path.as_path());
            let label = icon_label(show_icons.then(|| icons::for_file(&e.path)), &e.name, name_color);
            let mut button = egui::Button::selectable(selected, label).min_size(egui::vec2(ui.available_width(), 0.0));
            if let (Some(s), Some(c)) = (git, color) {
                if !s.badge().is_empty() {
                    button = button.right_text(RichText::new(s.badge()).size(12.0).color(c));
                }
            }
            let mut resp = ui.add(button);
            if let Some(t) = tooltip {
                resp = resp.on_hover_text(t);
            }
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
                        app.git.refresh();
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
