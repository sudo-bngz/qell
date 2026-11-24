//! The file tree.

use std::path::{Path, PathBuf};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, FontId, RichText};
use qell_git::FileStatus;

use crate::app::{Focus, QellApp, SidebarView, TreeEditKind};
use crate::icons;
use crate::recent::Order;

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
    // A new file or folder being named here: its field comes first.
    let new_here = app
        .tree_edit
        .as_ref()
        .is_some_and(|t| matches!(&t.kind, TreeEditKind::NewFile(d) | TreeEditKind::NewFolder(d) if d.as_path() == dir));
    if new_here {
        let folder = app.tree_edit.as_ref().is_some_and(|t| matches!(t.kind, TreeEditKind::NewFolder(_)));
        let icon = if folder { (icons::FOLDER, app.palette.text_dim) } else { icons::for_file(Path::new("new")) };
        tree_edit_row(app, ui, icon);
    }
    let active_path = app.docs.get(app.active).and_then(|d| d.buffer.path().map(Path::to_path_buf));

    let default_color = app.palette.text;
    let folder_color = app.palette.text_dim;
    let show_icons = app.config.file_icons;
    for e in entries {
        // Being renamed: a text field instead of the row.
        if app.tree_edit.as_ref().is_some_and(|t| t.kind == TreeEditKind::Rename(e.path.clone())) {
            let icon = if e.is_dir { (icons::FOLDER, app.palette.text_dim) } else { icons::for_file(&e.path) };
            tree_edit_row(app, ui, icon);
            continue;
        }
        let git = app.git_status_of(&e.path);
        let color = git.map(|s| app.palette.git_color(s));
        let name_color = color.unwrap_or(default_color);
        let tooltip = git.filter(|s| *s != FileStatus::Ignored).map(|s| s.description());

        if e.is_dir {
            let id = ui.make_persistent_id(&e.path);
            // Revealing a file opens every folder on the way to it.
            if app.reveal.as_ref().is_some_and(|r| r.starts_with(&e.path)) {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
                state.set_open(true);
                state.store(ui.ctx());
            }
            let is_open = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false).is_open();
            let icon = if is_open { icons::FOLDER_OPEN } else { icons::FOLDER };
            let label = icon_label(show_icons.then_some((icon, folder_color)), &e.name, name_color);
            let (_, header, _) = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    // Not selectable text: a folder name is a button, so the
                    // pointer is a hand, not a text cursor.
                    let name = ui
                        .add(egui::Label::new(label).sense(egui::Sense::click()).selectable(false))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    // Folders get a dot rather than a letter, like VS Code.
                    if let (Some(c), Some(_)) = (color, tooltip) {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(6.0);
                            // Monospace: the dot only exists in the bundled Hack font.
                            ui.label(RichText::new("●").monospace().size(10.0).color(c));
                        });
                    }
                    name
                })
                .body_unindented(|ui| {
                    ui.indent(id.with("body"), |ui| show_dir(app, ui, &e.path));
                });
            // Clicking the name opens/closes the folder, not just the arrow.
            if header.inner.clicked() {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
                state.toggle(ui);
                state.store(ui.ctx());
            }
            // Revealing the folder itself: show it and stop forcing it open,
            // so it can be collapsed again.
            if app.reveal.as_deref() == Some(e.path.as_path()) {
                header.inner.scroll_to_me(Some(egui::Align::Center));
                app.reveal = None;
            }
            // The name takes the clicks (it sits above the row).
            header.inner.context_menu(|ui| entry_menu(app, ui, &e.path, Some(id)));
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
            let mut resp = ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand);
            if let Some(t) = tooltip {
                resp = resp.on_hover_text(t);
            }
            if resp.clicked() {
                app.open_file(e.path.clone());
            }
            if app.reveal.as_deref() == Some(e.path.as_path()) {
                resp.scroll_to_me(Some(egui::Align::Center));
                app.reveal = None;
            }
            resp.context_menu(|ui| entry_menu(app, ui, &e.path, None));
        }
    }
}

/// The handful of entries at the top of the tree: the folders you work in
/// most (the repositories, in a workspace of several), then the files you
/// open most, plus anything you pinned.
fn show_shortcuts(app: &mut QellApp, ui: &mut egui::Ui) {
    let limit = app.config.shortcuts;
    if limit == 0 {
        return;
    }
    let order = if app.config.shortcuts_recent { Order::Recent } else { Order::MostUsed };
    let shortcuts = app.recent.shortcuts(&app.root.clone(), order, limit);
    let (dim, text) = (app.palette.text_dim, app.palette.text);
    let show_icons = app.config.file_icons;

    ui.horizontal(|ui| {
        let title = if app.config.shortcuts_recent { "RECENT" } else { "MOST USED" };
        if ui
            .add(egui::Label::new(RichText::new(title).size(11.0).color(dim)).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Show what you opened last instead")
            .clicked()
        {
            app.config.shortcuts_recent = !app.config.shortcuts_recent;
            app.save_config();
        }
    });
    if shortcuts.folders.is_empty() && shortcuts.files.is_empty() {
        ui.label(RichText::new("Files you open show up here.").size(11.0).italics().color(dim));
    }
    let active = app.docs.get(app.active).and_then(|d| d.buffer.path().map(Path::to_path_buf));
    let root = app.root.clone();
    for e in shortcuts.folders.into_iter().chain(shortcuts.files) {
        let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = e.path.parent().and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.to_string_lossy().into_owned());
        let icon = show_icons.then(|| if e.is_dir { (icons::FOLDER, dim) } else { icons::for_file(&e.path) });
        let label = icon_label(icon, &name, text);
        let mut button = egui::Button::selectable(!e.is_dir && active.as_deref() == Some(e.path.as_path()), label)
            .min_size(egui::vec2(ui.available_width(), 0.0));
        // Where it is, and a star when it's pinned. A folder below the
        // workspace needs no "where": its own name says it.
        let parent = parent.filter(|_| !e.is_dir);
        let right = match (e.pinned, parent.as_deref()) {
            (true, Some(p)) if !p.is_empty() => format!("★ {p}"),
            (true, _) => "★".into(),
            (false, Some(p)) => p.to_string(),
            (false, None) => String::new(),
        };
        if !right.is_empty() {
            button = button.right_text(RichText::new(shorten_dir(&right)).size(11.0).color(dim));
        }
        let hover = if e.is_dir { format!("{} — open it in the tree", e.path.display()) } else { e.path.display().to_string() };
        let resp = ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hover);
        if resp.clicked() {
            // A folder opens in the tree; a file opens in the editor.
            if !e.is_dir {
                app.open_file(e.path.clone());
            }
            app.reveal = Some(e.path.clone());
        }
        resp.context_menu(|ui| {
            let pinned = app.recent.is_pinned(&root, &e.path);
            if ui.button(if pinned { "Unpin" } else { "Pin to top" }).clicked() {
                app.recent.toggle_pin(&root, &e.path);
                ui.close();
            }
            if ui.button("Reveal in File Tree").clicked() {
                app.reveal = Some(e.path.clone());
                ui.close();
            }
            ui.separator();
            if ui.button("Remove from list").clicked() {
                app.recent.forget(&root, &e.path);
                ui.close();
            }
            if ui.button("Clear the list").clicked() {
                app.recent.clear(&root);
                ui.close();
            }
            ui.separator();
            entry_menu(app, ui, &e.path.clone(), e.is_dir.then(|| ui.id().with(&e.path)));
        });
    }
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(2.0);
}

/// Keep a folder path short enough for the narrow column ("…/deep/dir").
fn shorten_dir(path: &str) -> String {
    const MAX: usize = 22;
    if path.chars().count() <= MAX {
        return path.to_string();
    }
    let tail: String = path.chars().skip(path.chars().count() - (MAX - 1)).collect();
    match tail.split_once('/') {
        Some((_, rest)) => format!("…/{rest}"),
        None => format!("…{tail}"),
    }
}

/// The inline field for renaming or naming a new entry. Enter or clicking
/// away applies it, Escape cancels.
fn tree_edit_row(app: &mut QellApp, ui: &mut egui::Ui, icon: (char, Color32)) {
    let text_color = app.palette.text;
    let Some(edit) = app.tree_edit.as_mut() else { return };
    let mut done = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon.0.to_string()).font(icons::font(14.0)).color(icon.1));
        let id = ui.id().with("tree_edit");
        if edit.fresh {
            // Focus it with the name selected, minus its extension (VS Code).
            edit.fresh = false;
            let stem = match edit.text.rfind('.') {
                Some(i) if i > 0 => edit.text[..i].chars().count(),
                _ => edit.text.chars().count(),
            };
            let mut st = egui::text_edit::TextEditState::default();
            st.cursor
                .set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem))));
            st.store(ui.ctx(), id);
            ui.memory_mut(|m| m.request_focus(id));
        }
        let resp = ui.add(
            egui::TextEdit::singleline(&mut edit.text)
                .id(id)
                .desired_width(f32::INFINITY)
                .text_color(text_color)
                .margin(egui::Margin::symmetric(2, 1)),
        );
        if resp.lost_focus() {
            done = Some(!ui.input(|i| i.key_pressed(egui::Key::Escape)));
            // The key that ended it isn't for the editor (which gets the
            // focus back this same frame and would type a newline).
            ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
        }
    });
    match done {
        Some(true) => app.commit_tree_edit(),
        Some(false) => app.tree_edit = None,
        None => {}
    }
}

/// "Reveal in Finder" and its equivalents, by platform (VS Code's wording).
pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(windows) {
        "Reveal in File Explorer"
    } else {
        "Open Containing Folder"
    }
}

/// Right-click menu of a file (`folder_id: None`) or a folder in the tree.
fn entry_menu(app: &mut QellApp, ui: &mut egui::Ui, path: &Path, folder_id: Option<egui::Id>) {
    let dir = if folder_id.is_some() { path.to_path_buf() } else { path.parent().unwrap_or(path).to_path_buf() };
    if let Some(id) = folder_id {
        let new = |kind: TreeEditKind, app: &mut QellApp, ui: &mut egui::Ui| {
            // Open the folder so the field shows up inside it.
            let mut st = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            st.set_open(true);
            st.store(ui.ctx());
            app.start_tree_edit(kind);
            ui.close();
        };
        if ui.button("New File…").clicked() {
            new(TreeEditKind::NewFile(dir.clone()), app, ui);
        }
        if ui.button("New Folder…").clicked() {
            new(TreeEditKind::NewFolder(dir.clone()), app, ui);
        }
    } else if ui.button("Open").clicked() {
        app.open_file(path.to_path_buf());
        ui.close();
    }
    ui.separator();
    if ui.button(reveal_label()).clicked() {
        app.reveal_in_file_manager(path);
        ui.close();
    }
    if ui.button("Open in Integrated Terminal").clicked() {
        app.new_terminal_in(dir);
        app.show_terminal = true;
        app.focus = Focus::Terminal;
        ui.close();
    }
    ui.separator();
    if ui.button("Copy Path").clicked() {
        ui.ctx().copy_text(path.display().to_string());
        ui.close();
    }
    if ui.button("Copy Relative Path").clicked() {
        ui.ctx().copy_text(app.relative_path(path));
        ui.close();
    }
    ui.separator();
    if ui.button("Rename…").clicked() {
        app.start_tree_edit(TreeEditKind::Rename(path.to_path_buf()));
        ui.close();
    }
    if ui.button("Delete").clicked() {
        app.confirm_delete = Some(path.to_path_buf());
        ui.close();
    }
}

/// "Move to Trash?" before deleting from the tree.
pub fn show_confirm_delete(app: &mut QellApp, ctx: &Context) {
    let Some(path) = app.confirm_delete.clone() else { return };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bin = if cfg!(windows) { "Recycle Bin" } else { "Trash" };
    let what = if path.is_dir() { "folder" } else { "file" };
    let mut choice = None;
    let modal = egui::Modal::new(egui::Id::new("confirm_delete")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(format!("Move the {what} '{name}' to the {bin}?")).strong());
        ui.add_space(4.0);
        ui.label(RichText::new(format!("You can restore it from the {bin}.")).color(app.palette.text_dim));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(format!("Move to {bin}")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                choice = Some(true);
            }
            if ui.button("Cancel").clicked() {
                choice = Some(false);
            }
        });
    });
    if modal.should_close() && choice.is_none() {
        choice = Some(false);
    }
    match choice {
        Some(true) => {
            app.confirm_delete = None;
            app.trash_path(&path);
        }
        Some(false) => app.confirm_delete = None,
        None => {}
    }
}

pub fn show(app: &mut QellApp, ctx: &Context) {
    let bg = app.palette.panel_bg;
    egui::SidePanel::left("sidebar")
        .default_width(240.0)
        .width_range(160.0..=600.0)
        .frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(4, 6)))
        .show(ctx, |ui| {
            // A rename or new entry only lives in the Explorer.
            if app.sidebar_view != SidebarView::Explorer {
                app.tree_edit = None;
            }
            match app.sidebar_view {
                SidebarView::SourceControl => return super::scm::show_contents(app, ui),
                SidebarView::Search => return super::search::show_contents(app, ui),
                SidebarView::Explorer => {}
            }
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
                show_shortcuts(app, ui);
                let root = app.root.clone();
                show_dir(app, ui, &root);
                // The empty space below the tree: menu for the workspace root.
                let h = ui.available_height();
                let rest = ui.allocate_response(
                    egui::vec2(ui.available_width(), if h.is_finite() { h.max(40.0) } else { 40.0 }),
                    egui::Sense::click(),
                );
                rest.context_menu(|ui| {
                    if ui.button("New File…").clicked() {
                        app.start_tree_edit(TreeEditKind::NewFile(root.clone()));
                        ui.close();
                    }
                    if ui.button("New Folder…").clicked() {
                        app.start_tree_edit(TreeEditKind::NewFolder(root.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(reveal_label()).clicked() {
                        app.reveal_in_file_manager(&root);
                        ui.close();
                    }
                    if ui.button("Open in Integrated Terminal").clicked() {
                        app.new_terminal_in(root.clone());
                        app.show_terminal = true;
                        app.focus = Focus::Terminal;
                        ui.close();
                    }
                });
            });
        });
}

/// The thin bar on the far left: Explorer and Source Control, VS Code style.
pub fn show_activity_bar(app: &mut QellApp, ctx: &Context) {
    let changes = app.git_status.as_ref().filter(|_| app.config.git_status).map_or(0, |ws| ws.changed_files());
    egui::SidePanel::left("activity_bar")
        .exact_width(44.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(app.palette.chrome_bg).inner_margin(egui::Margin::symmetric(0, 6)))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                for (view, icon, tip) in [
                    (SidebarView::Explorer, icons::EXPLORER, "Explorer (Ctrl+Shift+E)"),
                    (SidebarView::Search, icons::SEARCH, "Search (Ctrl+Shift+F)"),
                    (SidebarView::SourceControl, icons::SOURCE_CONTROL, "Source Control (Ctrl+Shift+G)"),
                ] {
                    let active = app.show_sidebar && app.sidebar_view == view;
                    let color = if active { app.palette.text } else { app.palette.text_dim };
                    let button = egui::Button::new(RichText::new(icon.to_string()).font(icons::font(22.0)).color(color))
                        .frame(false)
                        .min_size(egui::vec2(44.0, 44.0));
                    let resp = ui.add(button).on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if view == SidebarView::SourceControl && changes > 0 {
                        // Badge with the number of changed files.
                        let text = if changes > 99 { "99+".to_string() } else { changes.to_string() };
                        let center = resp.rect.center() + egui::vec2(10.0, 9.0);
                        let galley = ui.painter().layout_no_wrap(text, FontId::proportional(9.0), app.palette.editor_bg);
                        let w = (galley.size().x + 7.0).max(15.0);
                        let rect = egui::Rect::from_center_size(center, egui::vec2(w, 15.0));
                        ui.painter().rect_filled(rect, 7.5, app.palette.accent);
                        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, app.palette.editor_bg);
                    }
                    if resp.clicked() {
                        app.show_sidebar_view(view);
                    }
                }
                cluster_button(app, ui);
                aws_button(app, ui);
            });
        });
}

/// A cloud, under the helm: opens and closes the AWS view. Drawn, like
/// the helm, since the icon font has neither.
fn aws_button(app: &mut QellApp, ui: &mut egui::Ui) {
    let open = app.aws.open;
    let color = if open { app.palette.text } else { app.palette.text_dim };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
    let resp = resp.on_hover_text("AWS (Ctrl+Shift+A)").on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);
    let centre = rect.center() + egui::vec2(0.0, 1.0);
    // Three overlapping bumps on a flat base; the outline is found by
    // walking outwards from the middle in every direction.
    let bumps = [(egui::vec2(-5.5, 1.5), 5.0_f32), (egui::vec2(0.5, -2.5), 6.5), (egui::vec2(6.5, 2.0), 4.5)];
    let base = 6.5;
    let inside = |p: egui::Vec2| {
        p.y <= base && bumps.iter().any(|(c, r)| (p - *c).length() <= *r || (p.y > c.y && (p.x - c.x).abs() <= *r))
    };
    let points: Vec<egui::Pos2> = (0..72)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 72.0;
            let dir = egui::vec2(a.cos(), a.sin());
            let mut reach = 0.0;
            let mut t = 0.0;
            while t < 16.0 {
                if inside(egui::vec2(0.0, 2.0) + dir * t) {
                    reach = t;
                }
                t += 0.2;
            }
            centre + egui::vec2(0.0, 2.0) + dir * reach
        })
        .collect();
    let stroke = egui::Stroke::new(if open { 1.6_f32 } else { 1.3_f32 }, color);
    painter.add(egui::Shape::closed_line(points, stroke));
    if resp.clicked() {
        app.pending_commands.push(crate::commands::Command::ToggleAws);
    }
}

/// The Kubernetes helm, under the git icon: opens and closes the cluster
/// view. Drawn rather than taken from the icon font, which has no helm.
fn cluster_button(app: &mut QellApp, ui: &mut egui::Ui) {
    let open = app.k8s.open;
    let color = if open { app.palette.text } else { app.palette.text_dim };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
    let resp = resp.on_hover_text("Kubernetes Cluster (Ctrl+Shift+K)").on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);
    let centre = rect.center();
    let r = 11.0;
    // Seven corners, like the wheel on the Kubernetes logo.
    let corner = |i: usize, radius: f32| {
        let a = std::f32::consts::TAU * (i as f32 / 7.0) - std::f32::consts::FRAC_PI_2;
        centre + egui::vec2(a.cos(), a.sin()) * radius
    };
    let stroke = egui::Stroke::new(if open { 1.6_f32 } else { 1.3_f32 }, color);
    let points: Vec<egui::Pos2> = (0..7).map(|i| corner(i, r)).collect();
    painter.add(egui::Shape::closed_line(points, stroke));
    for i in 0..7 {
        painter.line_segment([corner(i, r * 0.42), corner(i, r * 0.88)], stroke);
    }
    painter.circle_stroke(centre, r * 0.42, stroke);
    if resp.clicked() {
        app.pending_commands.push(crate::commands::Command::ToggleCluster);
    }
}
