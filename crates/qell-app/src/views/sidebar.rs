//! The file tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, FontId, RichText};
use qell_git::FileStatus;
use qell_lsp::lsp_types::{Diagnostic, NumberOrString};

use crate::app::{Focus, QellApp, SidebarView, TreeEditKind};
use crate::config::TreeSort;
use crate::icons;
use crate::recent::Order;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// When it last changed on disk, for a file; for a folder, when an
    /// entry was added to or removed from it.
    pub modified: Option<SystemTime>,
}

fn read_dir(path: &Path, hidden: &[String]) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(path) else { return Vec::new() };
    let mut entries: Vec<Entry> = rd
        .flatten()
        .filter(|e| !hidden.iter().any(|h| *h == *e.file_name().to_string_lossy()))
        .map(|e| {
            let path = e.path();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let modified = e.metadata().and_then(|m| m.modified()).ok();
            Entry { name: e.file_name().to_string_lossy().into_owned(), path, is_dir, modified }
        })
        .collect();
    // Directories first, then case-insensitive by name — like every file explorer.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries
}

/// Put a folder's entries in the tree's order: folders first, then
/// `sort` within each group, name breaking ties. `newest` is when anything
/// inside a folder last changed (from the index) and `used` how much each
/// path is used.
fn order(entries: &mut [Entry], sort: TreeSort, newest: &dyn Fn(&Entry) -> Option<SystemTime>, used: &HashMap<PathBuf, f64>) {
    let name = |a: &Entry, b: &Entry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    match sort {
        TreeSort::Name => entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| name(a, b))),
        TreeSort::Modified => {
            let when = |e: &Entry| newest(e).max(e.modified);
            entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| when(b).cmp(&when(a))).then_with(|| name(a, b)));
        }
        TreeSort::Used => {
            let score = |e: &Entry| used.get(&e.path).copied().unwrap_or(0.0);
            entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| score(b).total_cmp(&score(a))).then_with(|| name(a, b)));
        }
    }
}

/// The most errors in any one file below `dir`, if a file there has any.
fn errors_under<T>(errors: &HashMap<PathBuf, Vec<T>>, dir: &Path) -> Option<usize> {
    errors.iter().filter(|(p, _)| p.starts_with(dir)).map(|(_, v)| v.len()).max()
}

/// A problem as one line: where, what, and which tool says so.
pub fn problem_line(d: &Diagnostic) -> String {
    let code = match &d.code {
        Some(NumberOrString::Number(n)) => Some(n.to_string()),
        Some(NumberOrString::String(s)) => Some(s.clone()),
        None => None,
    };
    let from = match (d.source.as_deref(), code) {
        (Some(src), Some(code)) => format!("  {src}({code})"),
        (Some(src), None) => format!("  {src}"),
        (None, Some(code)) => format!("  {code}"),
        (None, None) => String::new(),
    };
    let what = d.message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    format!("Ln {}  {what}{from}", d.range.start.line + 1)
}

/// The errors of a file, at the top of its menu: each one goes there,
/// and Quick Fix asks its server how to fix the first.
fn problems_menu(app: &mut QellApp, ui: &mut egui::Ui, path: &Path) {
    let Some(errors) = app.errors.get(path).cloned() else { return };
    let dim = app.palette.text_dim;
    ui.label(RichText::new(errors_text(errors.len(), "in this file")).size(11.0).color(app.palette.error));
    for d in errors.iter().take(8) {
        let line = problem_line(d);
        let short = if line.chars().count() > 70 { format!("{}…", line.chars().take(69).collect::<String>()) } else { line };
        if ui.button(short).on_hover_text(d.message.as_str()).clicked() {
            app.go_to_problem(path, d, false);
            ui.close();
        }
    }
    if errors.len() > 8 {
        ui.label(RichText::new(format!("and {} more", errors.len() - 8)).size(11.0).color(dim));
    }
    let shortcut = crate::commands::Command::QuickFix.shortcut_text(ui.ctx());
    if ui
        .add(egui::Button::new("Quick Fix…").shortcut_text(shortcut))
        .on_hover_text("Ask the language server how to fix the first error")
        .clicked()
    {
        app.go_to_problem(path, &errors[0], true);
        ui.close();
    }
    if ui.button("Copy Errors").clicked() {
        let text: Vec<String> = errors.iter().map(problem_line).collect();
        ui.ctx().copy_text(text.join("\n"));
        ui.close();
    }
    ui.separator();
}

/// For a folder: the files inside it with errors, each going to its first.
fn folder_problems_menu(app: &mut QellApp, ui: &mut egui::Ui, dir: &Path) {
    let mut files: Vec<(PathBuf, Diagnostic)> =
        app.errors.iter().filter(|(p, _)| p.starts_with(dir)).map(|(p, v)| (p.clone(), v[0].clone())).collect();
    if files.is_empty() {
        return;
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let n = files.len();
    ui.label(RichText::new(format!("{n} file{} with errors", if n == 1 { "" } else { "s" })).size(11.0).color(app.palette.error));
    for (path, first) in files.iter().take(8) {
        let rel = path.strip_prefix(dir).unwrap_or(path).display().to_string();
        if ui.button(rel).on_hover_text(problem_line(first)).clicked() {
            app.go_to_problem(path, first, false);
            ui.close();
        }
    }
    if n > 8 {
        ui.label(RichText::new(format!("and {} more", n - 8)).size(11.0).color(app.palette.text_dim));
    }
    ui.separator();
}

fn errors_text(n: usize, where_: &str) -> String {
    if n == 1 {
        format!("1 error {where_}")
    } else {
        format!("{n} errors {where_}")
    }
}

/// A curly red line under a tree row's name (not its icon), starting from
/// where the label starts.
fn underline_name(ui: &egui::Ui, left: f32, bottom: f32, name: &str, size: f32, icon: bool, color: Color32) {
    let measure = |text: &str, font: FontId| ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, color).size().x);
    // Where the name starts: after the icon and the gap icon_label leaves.
    let start = if icon { left + measure("\u{f15b}", icons::font(size + 1.0)) + 6.0 } else { left };
    let width = measure(name, FontId::proportional(size));
    super::wavy_line(ui.painter(), start, start + width, bottom - 1.0, color);
}

/// A branch name short enough to sit beside a folder's: long ticket
/// branches keep their start, which is the part people recognise.
fn elide_branch(branch: &str) -> String {
    const MAX: usize = 28;
    if branch.chars().count() <= MAX {
        branch.to_string()
    } else {
        format!("{}…", branch.chars().take(MAX - 1).collect::<String>())
    }
}

/// How long ago, as briefly as `ls` would put it: `now`, `5m`, `3h`,
/// `2d`, `4mo`, `1y`.
fn ago(when: SystemTime, now: SystemTime) -> String {
    let s = now.duration_since(when).map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        86_400..2_592_000 => format!("{}d", s / 86_400),
        2_592_000..31_536_000 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// Icon glyph followed by the name, in one label.
/// `None` for the icon (icons turned off in Settings) gives the bare name.
/// Names are set at the editor's font size, so the tree, the tabs and
/// the code read as one text.
pub fn icon_label(icon: Option<(char, Color32)>, name: &str, name_color: Color32, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut gap = 0.0;
    if let Some((icon, icon_color)) = icon {
        job.append(
            &icon.to_string(),
            0.0,
            TextFormat { font_id: icons::font(size + 1.0), color: icon_color, ..Default::default() },
        );
        gap = 6.0;
    }
    job.append(name, gap, TextFormat { font_id: FontId::proportional(size), color: name_color, ..Default::default() });
    job
}

fn show_dir(app: &mut QellApp, ui: &mut egui::Ui, dir: &Path) {
    if !app.tree_cache.contains_key(dir) {
        let entries = read_dir(dir, &app.config.tree_hidden);
        app.tree_cache.insert(dir.to_path_buf(), entries);
    }
    let mut entries = app.tree_cache.get(dir).cloned().unwrap_or_default();
    let sort = app.config.tree_sort;
    let font_size = app.font_size;
    let error_color = app.palette.error;
    // The branch of each repository in this folder, by absolute path.
    let branches: HashMap<PathBuf, String> =
        match app.git_status.as_ref().filter(|_| app.config.tree_branches && app.config.git_status) {
            Some(ws) => ws
                .repos
                .iter()
                .filter(|(rel, _)| !rel.as_os_str().is_empty())
                .map(|(rel, st)| (app.root.join(rel), st.branch.clone().unwrap_or_else(|| "detached".into())))
                .filter(|(path, _)| path.parent() == Some(dir))
                .collect(),
            None => HashMap::new(),
        };
    // Only what git sees as changed, and the folders that hold it.
    if app.config.tree_changed {
        entries.retain(|e| app.git_status_of(&e.path).is_some_and(|s| s != FileStatus::Ignored));
    }
    let index = app.indexer.index();
    let index = index.read().ok();
    let root = app.root.clone();
    let newest = |e: &Entry| -> Option<SystemTime> {
        let index = index.as_ref()?;
        e.is_dir.then(|| index.newest_in(e.path.strip_prefix(&root).ok()?)).flatten()
    };
    let used = if sort == TreeSort::Used { app.recent.usage(&root) } else { HashMap::new() };
    order(&mut entries, sort, &newest, &used);
    // The age of each row, when sorted by it.
    let now = SystemTime::now();
    let ages: HashMap<PathBuf, String> = if sort == TreeSort::Modified {
        entries.iter().filter_map(|e| Some((e.path.clone(), ago(newest(e).max(e.modified)?, now)))).collect()
    } else {
        HashMap::new()
    };
    drop(index);
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
            let mut label = icon_label(show_icons.then_some((icon, folder_color)), &e.name, name_color, font_size);
            // A repository says which branch it is on, after its name.
            if let Some(branch) = branches.get(&e.path) {
                let fmt = |font_id: FontId| TextFormat { font_id, color: folder_color, ..Default::default() };
                label.append("\u{e0a0}", 10.0, fmt(FontId::monospace(font_size - 2.0)));
                label.append(&elide_branch(branch), 3.0, fmt(FontId::proportional(font_size - 2.0)));
            }
            let (_, header, _) = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    // Not selectable text: a folder name is a button, so the
                    // pointer is a hand, not a text cursor.
                    let name = ui
                        .add(egui::Label::new(label).sense(egui::Sense::click()).selectable(false))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    // Something inside has errors, however deep.
                    if errors_under(&app.errors, &e.path).is_some() {
                        // All of them, across the files inside.
                        let n: usize = app.errors.iter().filter(|(p, _)| p.starts_with(&e.path)).map(|(_, v)| v.len()).sum();
                        underline_name(ui, name.rect.left(), name.rect.bottom(), &e.name, font_size, show_icons, error_color);
                        if tooltip.is_none() {
                            name.clone().on_hover_text(errors_text(n, "inside"));
                        }
                    }
                    // Folders get a dot rather than a letter, like VS Code.
                    let dot = color.zip(tooltip).map(|(c, _)| c);
                    let age = ages.get(&e.path);
                    if dot.is_some() || age.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(6.0);
                            // Monospace: the dot only exists in the bundled Hack font.
                            if let Some(c) = dot {
                                ui.label(RichText::new("●").monospace().size(10.0).color(c));
                            }
                            if let Some(age) = age {
                                ui.label(RichText::new(age).size(11.0).color(folder_color));
                            }
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
            let label = icon_label(show_icons.then(|| icons::for_file(&e.path)), &e.name, name_color, font_size);
            let mut button = egui::Button::selectable(selected, label).min_size(egui::vec2(ui.available_width(), 0.0));
            // On the right: the age when sorted by it, then the git letter.
            let mut right = LayoutJob::default();
            if let Some(age) = ages.get(&e.path) {
                right.append(
                    age,
                    0.0,
                    TextFormat { font_id: FontId::proportional(11.0), color: folder_color, ..Default::default() },
                );
            }
            if let (Some(s), Some(c)) = (git, color) {
                if !s.badge().is_empty() {
                    let gap = if right.is_empty() { 0.0 } else { 6.0 };
                    right.append(
                        s.badge(),
                        gap,
                        TextFormat { font_id: FontId::proportional(12.0), color: c, ..Default::default() },
                    );
                }
            }
            if !right.is_empty() {
                button = button.right_text(right);
            }
            let mut resp = ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand);
            let errors = app.errors.get(&e.path).map(Vec::len);
            if let Some(n) = errors {
                let left = resp.rect.left() + ui.spacing().button_padding.x;
                underline_name(ui, left, resp.rect.bottom() - 2.0, &e.name, font_size, show_icons, error_color);
                resp = resp.on_hover_text(errors_text(n, "in this file"));
            }
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
        let label = icon_label(icon, &name, text, app.font_size);
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
        let errors = if e.is_dir { errors_under(&app.errors, &e.path) } else { app.errors.get(&e.path).map(Vec::len) };
        if errors.is_some() {
            let left = resp.rect.left() + ui.spacing().button_padding.x;
            underline_name(ui, left, resp.rect.bottom() - 2.0, &name, app.font_size, show_icons, app.palette.error);
        }
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

/// The line under the workspace's name: how the tree is ordered, and
/// whether it shows only what changed. Plain words, the one in use bright,
/// like flags you would give `ls`.
fn tree_options(app: &mut QellApp, ui: &mut egui::Ui) {
    let (dim, on) = (app.palette.text_dim, app.palette.text);
    let word = |ui: &mut egui::Ui, text: &str, active: bool, hover: &str| {
        let color = if active { on } else { dim.gamma_multiply(0.75) };
        ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).sense(egui::Sense::click()).selectable(false))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(hover)
            .clicked()
    };
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (sort, text, hover) in [
            (TreeSort::Name, "name", "Folders, then files, by name"),
            (TreeSort::Modified, "modified", "Last changed first, with how long ago"),
            (TreeSort::Used, "used", "Opened most first; a folder counts what was opened in it"),
        ] {
            if word(ui, text, app.config.tree_sort == sort, hover) && app.config.tree_sort != sort {
                app.config.tree_sort = sort;
                changed = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if word(ui, "changed", app.config.tree_changed, "Only what git sees as changed") {
                app.config.tree_changed = !app.config.tree_changed;
                changed = true;
            }
        });
    });
    if changed {
        app.save_config();
    }
}

/// Whether git has looked and sees nothing changed in the workspace.
fn nothing_changed(app: &QellApp) -> bool {
    app.git_status.as_ref().is_some_and(|s| s.changed_files() == 0)
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
    if folder_id.is_some() {
        folder_problems_menu(app, ui, path);
    } else {
        problems_menu(app, ui, path);
    }
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
            tree_options(app, ui);
            ui.add_space(4.0);
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                show_shortcuts(app, ui);
                let root = app.root.clone();
                if app.config.tree_changed {
                    let why = if !app.config.git_status {
                        Some("Git status is off in Settings.")
                    } else if nothing_changed(app) {
                        Some("Nothing changed.")
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        ui.label(RichText::new(why).size(11.0).italics().color(app.palette.text_dim));
                    }
                }
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

/// The AWS mark, under the helm: opens and closes the AWS view. The glyph
/// is Material Design Icons' `md-aws`, from the bundled icon font.
fn aws_button(app: &mut QellApp, ui: &mut egui::Ui) {
    const AWS: char = '\u{f0e0f}';
    let open = app.aws.open;
    let color = if open { app.palette.text } else { app.palette.text_dim };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
    let resp = resp.on_hover_text("AWS (Ctrl+Shift+A)").on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter_at(rect).text(rect.center(), egui::Align2::CENTER_CENTER, AWS, icons::font(24.0), color);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn entry(name: &str, is_dir: bool, secs: u64) -> Entry {
        let modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        Entry { path: PathBuf::from("/ws").join(name), name: name.into(), is_dir, modified }
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_come_first_in_every_order() {
        let none = |_: &Entry| None;
        let mut v = vec![entry("b.rs", false, 30), entry("api", true, 10), entry("A.md", false, 20), entry("web", true, 40)];
        order(&mut v, TreeSort::Name, &none, &HashMap::new());
        assert_eq!(names(&v), ["api", "web", "A.md", "b.rs"]);
        order(&mut v, TreeSort::Modified, &none, &HashMap::new());
        assert_eq!(names(&v), ["web", "api", "b.rs", "A.md"]);
    }

    #[test]
    fn a_folder_is_as_recent_as_what_changed_inside_it() {
        let mut v = vec![entry("api", true, 10), entry("web", true, 40)];
        // Something deep in api changed after web's own entries did.
        let newest = |e: &Entry| (e.name == "api").then(|| SystemTime::UNIX_EPOCH + Duration::from_secs(99));
        order(&mut v, TreeSort::Modified, &newest, &HashMap::new());
        assert_eq!(names(&v), ["api", "web"]);
    }

    #[test]
    fn used_puts_what_you_open_first_and_the_rest_by_name() {
        let none = |_: &Entry| None;
        let mut v = vec![entry("c.rs", false, 0), entry("b.rs", false, 0), entry("a.rs", false, 0), entry("web", true, 0)];
        let used = HashMap::from([(PathBuf::from("/ws/c.rs"), 5.0), (PathBuf::from("/ws/b.rs"), 1.0)]);
        order(&mut v, TreeSort::Used, &none, &used);
        assert_eq!(names(&v), ["web", "c.rs", "b.rs", "a.rs"]);
    }

    #[test]
    fn a_folder_shows_the_worst_file_inside_it() {
        let errors =
            HashMap::from([(PathBuf::from("/ws/api/src/main.go"), vec![(); 2]), (PathBuf::from("/ws/api/x.go"), vec![(); 5])]);
        assert_eq!(errors_under(&errors, Path::new("/ws/api")), Some(5));
        assert_eq!(errors_under(&errors, Path::new("/ws/api/src")), Some(2));
        assert_eq!(errors_under(&errors, Path::new("/ws/web")), None);
        // A sibling whose name merely starts the same isn't inside.
        assert_eq!(errors_under(&errors, Path::new("/ws/ap")), None);
        assert_eq!(errors_text(1, "in this file"), "1 error in this file");
        assert_eq!(errors_text(3, "inside"), "3 errors inside");
    }

    #[test]
    fn a_problem_reads_as_one_line_with_its_source() {
        use qell_lsp::lsp_types::{Position, Range};
        let mut d = Diagnostic {
            range: Range::new(Position::new(48, 2), Position::new(48, 3)),
            message: "Argument or block definition required\n\nmore detail".into(),
            ..Default::default()
        };
        assert_eq!(problem_line(&d), "Ln 49  Argument or block definition required");
        d.source = Some("ts".into());
        d.code = Some(NumberOrString::Number(2304));
        assert_eq!(problem_line(&d), "Ln 49  Argument or block definition required  ts(2304)");
    }

    #[test]
    fn long_branch_names_keep_their_start() {
        assert_eq!(elide_branch("main"), "main");
        let long = elide_branch("OPS-5335-a-rather-long-ticket-branch-name");
        assert_eq!(long.chars().count(), 28);
        assert!(long.starts_with("OPS-5335") && long.ends_with('…'));
    }

    #[test]
    fn ages_read_like_ls() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000_000);
        let ago_s = |s: u64| ago(now - Duration::from_secs(s), now);
        assert_eq!(ago_s(5), "now");
        assert_eq!(ago_s(300), "5m");
        assert_eq!(ago_s(3 * 3_600), "3h");
        assert_eq!(ago_s(2 * 86_400), "2d");
        assert_eq!(ago_s(120 * 86_400), "4mo");
        assert_eq!(ago_s(800 * 86_400), "2y");
        // A clock that went backwards is "now", not a panic.
        assert_eq!(ago(now + Duration::from_secs(60), now), "now");
    }
}
