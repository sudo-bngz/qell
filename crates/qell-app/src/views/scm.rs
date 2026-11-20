//! The Source Control view (Ctrl+Shift+G): per repository, a commit box,
//! staged changes and changes, with stage / unstage buttons.
//!
//! Git commands run on a background thread: commit hooks (pre-commit,
//! linters, signing) can take seconds and must not freeze the editor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{Color32, CursorIcon, Key, Modifiers, RichText};
use qell_git::{FileStatus, RepoStatus};

use crate::app::QellApp;
use crate::icons;

/// A finished background git command.
pub struct JobResult {
    pub repo: PathBuf,
    pub kind: JobKind,
    pub result: Result<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Stage,
    Unstage,
    Commit,
}

pub struct ScmState {
    /// Commit message being written, per repository (workspace-relative).
    pub messages: HashMap<PathBuf, String>,
    /// Repositories with a git command running.
    pub busy: HashSet<PathBuf>,
    /// A commit message box has the keyboard: the editor must not type.
    pub typing: bool,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
}

impl Default for ScmState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { messages: HashMap::new(), busy: HashSet::new(), typing: false, tx, rx }
    }
}

impl ScmState {
    fn spawn(
        &mut self,
        root: &Path,
        repo: &Path,
        kind: JobKind,
        job: impl FnOnce(&Path) -> Result<String, String> + Send + 'static,
    ) {
        self.busy.insert(repo.to_path_buf());
        let tx = self.tx.clone();
        let (dir, repo) = (root.join(repo), repo.to_path_buf());
        std::thread::spawn(move || {
            let result = job(&dir);
            let _ = tx.send(JobResult { repo, kind, result });
        });
    }

    pub fn poll(&mut self) -> Vec<JobResult> {
        let done: Vec<JobResult> = self.rx.try_iter().collect();
        for r in &done {
            self.busy.remove(&r.repo);
        }
        done
    }
}

fn stage(app: &mut QellApp, repo: &Path, paths: Vec<PathBuf>) {
    let root = app.root.clone();
    app.scm.spawn(&root, repo, JobKind::Stage, move |dir| {
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        qell_git::stage(dir, &refs).map(|_| String::new())
    });
}

fn unstage(app: &mut QellApp, repo: &Path, paths: Vec<PathBuf>) {
    let root = app.root.clone();
    app.scm.spawn(&root, repo, JobKind::Unstage, move |dir| {
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        qell_git::unstage(dir, &refs).map(|_| String::new())
    });
}

fn commit(app: &mut QellApp, repo: &Path) {
    let message = app.scm.messages.get(repo).cloned().unwrap_or_default();
    if message.trim().is_empty() {
        return;
    }
    let root = app.root.clone();
    app.scm.spawn(&root, repo, JobKind::Commit, move |dir| qell_git::commit(dir, &message));
}

/// Display name of a repository: its folder, or the workspace's.
fn repo_name(app: &QellApp, rel: &Path) -> String {
    let path = if rel.as_os_str().is_empty() { app.root.as_path() } else { rel };
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Section header ("STAGED CHANGES 2") with one action button on the right.
fn section_header(ui: &mut egui::Ui, title: &str, count: usize, action: (char, &str), dim: Color32, enabled: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(11.0).strong().color(dim));
        ui.label(RichText::new(count.to_string()).size(11.0).color(dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(RichText::new(action.0.to_string()).font(icons::font(13.0)).color(dim)).frame(false);
            clicked = ui.add_enabled(enabled, b).on_hover_text(action.1).on_hover_cursor(CursorIcon::PointingHand).clicked();
        });
    });
    clicked
}

enum RowAction {
    Open,
    Toggle,
}

/// One changed file: icon, name, folder, then the (un)stage button and the
/// status letter on the right.
fn file_row(
    ui: &mut egui::Ui,
    app: &QellApp,
    path: &Path,
    status: FileStatus,
    action: (char, &str),
    enabled: bool,
) -> Option<RowAction> {
    let color = app.palette.git_color(status);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let icon = app.config.file_icons.then(|| icons::for_file(path));
        let mut job =
            super::sidebar::icon_label(icon, &name, if status == FileStatus::Deleted { app.palette.text_dim } else { color });
        if status == FileStatus::Deleted {
            // Struck through, like VS Code.
            if let Some(s) = job.sections.last_mut() {
                s.format.strikethrough = egui::Stroke::new(1.0_f32, app.palette.text_dim);
            }
        }
        if !dir.is_empty() {
            job.append(
                &dir,
                6.0,
                egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: app.palette.text_dim, ..Default::default() },
            );
        }
        let row = ui.add(egui::Label::new(job).sense(egui::Sense::click()).truncate());
        let row = row.on_hover_text(path.display().to_string()).on_hover_cursor(CursorIcon::PointingHand);
        if row.clicked() {
            out = Some(RowAction::Open);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(status.badge()).size(12.0).color(color));
            let b = egui::Button::new(RichText::new(action.0.to_string()).font(icons::font(13.0)).color(app.palette.text_dim))
                .frame(false);
            if ui.add_enabled(enabled, b).on_hover_text(action.1).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                out = Some(RowAction::Toggle);
            }
        });
    });
    out
}

pub fn show_contents(app: &mut QellApp, ui: &mut egui::Ui) {
    let dim = app.palette.text_dim;
    app.scm.typing = false;

    ui.horizontal(|ui| {
        ui.label(RichText::new("SOURCE CONTROL").size(11.0).color(dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(RichText::new(icons::REFRESH.to_string()).font(icons::font(13.0)).color(dim)).frame(false);
            if ui.add(b).on_hover_text("Refresh").on_hover_cursor(CursorIcon::PointingHand).clicked() {
                app.git.refresh();
            }
        });
    });
    ui.add_space(4.0);

    let Some(ws) = app.git_status.clone() else {
        ui.add_space(8.0);
        ui.label(RichText::new("No git repository in this folder.").color(dim));
        return;
    };
    // With many repositories, list those with changes, plus the one of the
    // file being edited.
    let active = app.active_repo().map(|(root, _)| root.to_path_buf());
    let repos: Vec<&(PathBuf, RepoStatus)> = ws
        .repos
        .iter()
        .filter(|(root, st)| ws.repos.len() == 1 || !st.changes.is_empty() || Some(root) == active.as_ref())
        .collect();

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if repos.is_empty() {
            ui.label(RichText::new(format!("No changes in {} repositories.", ws.repos.len())).color(dim));
        }
        for (rel, st) in repos {
            show_repo(app, ui, rel, st);
            ui.add_space(10.0);
        }
    });
}

fn show_repo(app: &mut QellApp, ui: &mut egui::Ui, rel: &Path, st: &RepoStatus) {
    let dim = app.palette.text_dim;
    let busy = app.scm.busy.contains(rel);
    let branch = st.branch.clone().unwrap_or_else(|| "detached".into());

    ui.horizontal(|ui| {
        ui.label(RichText::new(icons::SOURCE_CONTROL.to_string()).font(icons::font(13.0)).color(dim));
        ui.label(RichText::new(repo_name(app, rel)).size(13.0).strong());
        ui.label(RichText::new(&branch).size(12.0).color(dim));
        if busy {
            ui.add(egui::Spinner::new().size(12.0));
        }
    });

    // Commit box.
    let staged: Vec<(PathBuf, FileStatus)> = st.staged().map(|(p, s)| (p.to_path_buf(), s)).collect();
    let unstaged: Vec<(PathBuf, FileStatus)> = st.unstaged().map(|(p, s)| (p.to_path_buf(), s)).collect();
    let id = egui::Id::new(("qell-commit-message", rel));
    let message = app.scm.messages.entry(rel.to_path_buf()).or_default();
    let resp = egui::TextEdit::multiline(message)
        .id(id)
        .hint_text(format!("Message (Ctrl+Enter to commit on \"{branch}\")"))
        .desired_rows(2)
        .desired_width(f32::INFINITY)
        .show(ui)
        .response;
    let mut do_commit = false;
    if resp.has_focus() {
        app.scm.typing = true;
        let ctrl_enter =
            ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter) || i.consume_key(Modifiers::CTRL, Key::Enter));
        do_commit |= ctrl_enter;
    }
    let has_message = !app.scm.messages.get(rel).is_none_or(|m| m.trim().is_empty());
    let can_commit = !busy && !staged.is_empty() && has_message;
    let mut label = egui::text::LayoutJob::default();
    let white = if can_commit { Color32::WHITE } else { dim };
    label.append(
        &icons::CHECK.to_string(),
        0.0,
        egui::TextFormat { font_id: icons::font(13.0), color: white, ..Default::default() },
    );
    label.append(
        "Commit",
        6.0,
        egui::TextFormat { font_id: egui::FontId::proportional(13.0), color: white, ..Default::default() },
    );
    let button = egui::Button::new(label)
        .fill(if can_commit { app.palette.accent } else { app.palette.hover })
        .min_size(egui::vec2(ui.available_width(), 26.0));
    let why = if staged.is_empty() {
        "Stage changes first (+)"
    } else if !has_message {
        "Write a commit message"
    } else {
        "Commit staged changes"
    };
    if ui.add_enabled(can_commit, button).on_hover_text(why).on_disabled_hover_text(why).clicked() {
        do_commit = true;
    }
    if do_commit && can_commit {
        commit(app, rel);
    }
    ui.add_space(6.0);

    let mut to_stage = Vec::new();
    let mut to_unstage = Vec::new();
    let mut to_open = None;
    if !staged.is_empty() {
        if section_header(ui, "STAGED CHANGES", staged.len(), (icons::REMOVE, "Unstage all"), dim, !busy) {
            to_unstage.extend(staged.iter().map(|(p, _)| p.clone()));
        }
        for (p, s) in &staged {
            match file_row(ui, app, p, *s, (icons::REMOVE, "Unstage"), !busy) {
                Some(RowAction::Toggle) => to_unstage.push(p.clone()),
                Some(RowAction::Open) => to_open = Some((p.clone(), *s)),
                None => {}
            }
        }
        ui.add_space(4.0);
    }
    // An empty "Changes" section only says something when nothing is staged.
    if (!unstaged.is_empty() || staged.is_empty())
        && section_header(ui, "CHANGES", unstaged.len(), (icons::ADD, "Stage all"), dim, !busy && !unstaged.is_empty())
    {
        to_stage.extend(unstaged.iter().map(|(p, _)| p.clone()));
    }
    for (p, s) in &unstaged {
        match file_row(ui, app, p, *s, (icons::ADD, "Stage"), !busy) {
            Some(RowAction::Toggle) => to_stage.push(p.clone()),
            Some(RowAction::Open) => to_open = Some((p.clone(), *s)),
            None => {}
        }
    }
    if unstaged.is_empty() && staged.is_empty() {
        ui.label(RichText::new("No changes.").size(12.0).color(dim));
    }

    if !to_stage.is_empty() {
        stage(app, rel, to_stage);
    }
    if !to_unstage.is_empty() {
        unstage(app, rel, to_unstage);
    }
    if let Some((p, s)) = to_open {
        if s == FileStatus::Deleted {
            app.notify(format!("{} was deleted", p.display()));
        } else {
            app.open_file(app.root.join(rel).join(p));
        }
    }
}
