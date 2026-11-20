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
    /// Repository whose history the Graph shows (picked by the user).
    pub graph_repo: Option<PathBuf>,
    graphs: HashMap<PathBuf, GraphState>,
    #[allow(clippy::type_complexity)]
    graph_tx: Sender<(PathBuf, String, Result<Vec<qell_git::graph::Commit>, String>)>,
    #[allow(clippy::type_complexity)]
    graph_rx: Receiver<(PathBuf, String, Result<Vec<qell_git::graph::Commit>, String>)>,
}

impl Default for ScmState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let (graph_tx, graph_rx) = mpsc::channel();
        Self {
            messages: HashMap::new(),
            busy: HashSet::new(),
            typing: false,
            tx,
            rx,
            graph_repo: None,
            graphs: HashMap::new(),
            graph_tx,
            graph_rx,
        }
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
    app.scm.poll_graphs();

    ui.horizontal(|ui| {
        ui.label(RichText::new("SOURCE CONTROL").size(11.0).color(dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(RichText::new(icons::REFRESH.to_string()).font(icons::font(13.0)).color(dim)).frame(false);
            if ui.add(b).on_hover_text("Refresh").on_hover_cursor(CursorIcon::PointingHand).clicked() {
                app.git.refresh();
                app.scm.graphs.clear();
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
        show_graph(app, ui, &ws.repos);
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

// ---- graph -------------------------------------------------------------

/// A repository's loaded history.
pub struct GraphState {
    /// What the history was loaded for (branch, position, changes); a
    /// different key means it is stale.
    key: String,
    commits: Vec<qell_git::graph::Commit>,
    rows: Vec<qell_git::graph::Row>,
    error: Option<String>,
    loading: bool,
}

/// VS Code's graph lane colours (scmGraph.foreground1-5).
const LANE_COLORS: [Color32; 5] = [
    Color32::from_rgb(0xff, 0xb0, 0x00),
    Color32::from_rgb(0xdc, 0x26, 0x7f),
    Color32::from_rgb(0x99, 0x4f, 0x00),
    Color32::from_rgb(0x40, 0xb0, 0xa6),
    Color32::from_rgb(0xb6, 0x6d, 0xff),
];
const GRAPH_ROW: f32 = 22.0;
const LANE_W: f32 = 11.0;
const MAX_LANES: usize = 12;
const HISTORY: usize = 300;

fn graph_key(st: &RepoStatus) -> String {
    format!("{:?}/{}/{}/{}", st.branch, st.ahead, st.behind, st.changes.len())
}

fn time_ago(t: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(t);
    let s = (now - t).max(0);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86_400 => (s / 3600, "hour"),
        86_400..2_592_000 => (s / 86_400, "day"),
        2_592_000..31_536_000 => (s / 2_592_000, "month"),
        _ => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

impl ScmState {
    /// Take finished history loads.
    fn poll_graphs(&mut self) {
        for (repo, key, result) in self.graph_rx.try_iter() {
            let g = self.graphs.entry(repo).or_insert_with(|| GraphState {
                key: String::new(),
                commits: Vec::new(),
                rows: Vec::new(),
                error: None,
                loading: false,
            });
            g.loading = false;
            g.key = key;
            match result {
                Ok(commits) => {
                    g.rows = qell_git::graph::layout(&commits);
                    g.commits = commits;
                    g.error = None;
                }
                Err(e) => {
                    g.commits.clear();
                    g.rows.clear();
                    g.error = Some(e);
                }
            }
        }
    }

    /// Load the history of `repo` in the background if it is missing or stale.
    fn ensure_graph(&mut self, root: &Path, repo: &Path, key: String, ctx: &egui::Context) {
        let g = self.graphs.get(repo);
        if g.is_some_and(|g| g.loading || g.key == key) {
            return;
        }
        let entry = self.graphs.entry(repo.to_path_buf()).or_insert_with(|| GraphState {
            key: String::new(),
            commits: Vec::new(),
            rows: Vec::new(),
            error: None,
            loading: false,
        });
        entry.loading = true;
        let (tx, dir, repo, ctx) = (self.graph_tx.clone(), root.join(repo), repo.to_path_buf(), ctx.clone());
        std::thread::spawn(move || {
            let result = qell_git::graph::log(&dir, HISTORY);
            let _ = tx.send((repo, key, result));
            ctx.request_repaint();
        });
    }
}

/// Paint one row of the graph: its lines and the commit's dot.
fn paint_graph_row(p: &egui::Painter, rect: egui::Rect, row: &qell_git::graph::Row, merge: bool, head: bool, bg: Color32) {
    use qell_git::graph::Half;
    let x = |lane: usize| rect.left() + 7.0 + lane.min(MAX_LANES) as f32 * LANE_W;
    let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
    for s in &row.segments {
        let color = LANE_COLORS[s.color % LANE_COLORS.len()];
        let stroke = egui::Stroke::new(1.6_f32, color);
        let (y0, y1) = if s.half == Half::Top { (top, mid) } else { (mid, bottom) };
        let (x0, x1) = (x(s.from), x(s.to));
        if s.from == s.to {
            p.line_segment([egui::pos2(x0, y0), egui::pos2(x1, y1)], stroke);
        } else {
            // A smooth curve between lanes.
            let ym = (y0 + y1) / 2.0;
            let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                [egui::pos2(x0, y0), egui::pos2(x0, ym), egui::pos2(x1, ym), egui::pos2(x1, y1)],
                false,
                Color32::TRANSPARENT,
                stroke,
            );
            p.add(curve);
        }
    }
    let color = LANE_COLORS[row.lane % LANE_COLORS.len()];
    let c = egui::pos2(x(row.lane), mid);
    if head {
        // The checked-out commit: a ring.
        p.circle_filled(c, 5.0, bg);
        p.circle_stroke(c, 4.5, egui::Stroke::new(2.0_f32, color));
    } else if merge {
        p.circle_filled(c, 4.0, bg);
        p.circle_stroke(c, 3.5, egui::Stroke::new(1.6_f32, color));
    } else {
        p.circle_filled(c, 3.8, color);
    }
}

/// Branch / tag badges before a commit's subject.
fn ref_badges(job: &mut egui::text::LayoutJob, refs: &[String], accent: Color32, dim: Color32) {
    for r in refs {
        let (icon, name, color) = if let Some(b) = r.strip_prefix("HEAD -> ") {
            (icons::SOURCE_CONTROL, b, accent)
        } else if let Some(t) = r.strip_prefix("tag: ") {
            ('\u{ea66}', t, dim)
        } else if r == "HEAD" {
            (icons::SOURCE_CONTROL, "HEAD", accent)
        } else if r.contains('/') {
            ('\u{ebaa}', r.as_str(), dim) // remote branch
        } else {
            (icons::SOURCE_CONTROL, r.as_str(), dim)
        };
        job.append(&icon.to_string(), 0.0, egui::TextFormat { font_id: icons::font(11.0), color, ..Default::default() });
        job.append(
            name,
            2.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(11.0),
                color,
                background: Color32::TRANSPARENT,
                ..Default::default()
            },
        );
        job.append(" ", 0.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), ..Default::default() });
    }
}

fn show_graph(app: &mut QellApp, ui: &mut egui::Ui, repos: &[(PathBuf, RepoStatus)]) {
    let dim = app.palette.text_dim;
    if repos.is_empty() {
        return;
    }
    // Which repository: the one picked, else the active file's, else the first.
    let picked = app.scm.graph_repo.clone().filter(|r| repos.iter().any(|(p, _)| p == r));
    let repo = picked.or_else(|| app.active_repo().map(|(r, _)| r.to_path_buf())).unwrap_or_else(|| repos[0].0.clone());
    let Some(st) = repos.iter().find(|(p, _)| *p == repo).map(|(_, s)| s) else { return };

    ui.add_space(6.0);
    let mut refresh = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("GRAPH").size(11.0).strong().color(dim));
        if repos.len() > 1 {
            egui::ComboBox::from_id_salt("graph_repo").selected_text(RichText::new(repo_name(app, &repo)).size(11.0)).show_ui(
                ui,
                |ui| {
                    for (r, _) in repos {
                        let name = repo_name(app, r);
                        if ui.selectable_label(*r == repo, name).clicked() {
                            app.scm.graph_repo = Some(r.clone());
                        }
                    }
                },
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(RichText::new(icons::REFRESH.to_string()).font(icons::font(12.0)).color(dim)).frame(false);
            refresh = ui.add(b).on_hover_text("Reload history").on_hover_cursor(CursorIcon::PointingHand).clicked();
        });
    });
    if refresh {
        app.scm.graphs.remove(&repo);
    }
    let root = app.root.clone();
    let ctx = ui.ctx().clone();
    app.scm.ensure_graph(&root, &repo, graph_key(st), &ctx);

    let Some(g) = app.scm.graphs.get(&repo) else { return };
    if let Some(e) = &g.error {
        let msg = if e.contains("does not have any commits") || e.contains("unknown revision") {
            "No commits yet."
        } else {
            e.as_str()
        };
        ui.label(RichText::new(msg).size(12.0).color(dim));
        return;
    }
    if g.commits.is_empty() {
        if g.loading {
            ui.add(egui::Spinner::new().size(14.0));
        }
        return;
    }

    let accent = app.palette.accent;
    let text = app.palette.text;
    let bg = app.palette.panel_bg;
    let hover_bg = app.palette.hover;
    let head_hash =
        g.commits.iter().find(|c| c.refs.iter().any(|r| r == "HEAD" || r.starts_with("HEAD -> "))).map(|c| c.hash.clone());
    let lanes = g.rows.iter().map(|r| r.width).max().unwrap_or(1).min(MAX_LANES + 1);
    let graph_w = 10.0 + lanes as f32 * LANE_W;
    let mut copied = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    for (c, row) in g.commits.iter().zip(&g.rows) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), GRAPH_ROW), egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let p = ui.painter_at(rect);
        if resp.hovered() {
            p.rect_filled(rect, 0.0, hover_bg);
        }
        let is_head = head_hash.as_deref() == Some(c.hash.as_str());
        paint_graph_row(&p, rect, row, c.parents.len() > 1, is_head, if resp.hovered() { hover_bg } else { bg });

        let mut job = egui::text::LayoutJob::default();
        ref_badges(&mut job, &c.refs, accent, dim);
        job.append(
            &c.subject,
            0.0,
            egui::TextFormat { font_id: egui::FontId::proportional(12.5), color: text, ..Default::default() },
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - graph_w - 4.0);
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        p.galley(egui::pos2(rect.left() + graph_w, rect.center().y - galley.size().y / 2.0), galley, text);

        let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_ui(|ui| {
            ui.label(RichText::new(&c.subject).strong());
            ui.label(RichText::new(format!("{} · {} · {}", c.short_hash(), c.author, time_ago(c.time))).color(dim));
            if !c.refs.is_empty() {
                ui.label(RichText::new(c.refs.join(", ")).size(11.0).color(dim));
            }
            ui.label(RichText::new("Click to copy the commit hash").size(11.0).color(dim));
        });
        if resp.clicked() {
            copied = Some(c.short_hash().to_string());
        }
    }
    if let Some(h) = copied {
        ui.ctx().copy_text(h.clone());
        app.notify(format!("Copied {h}"));
    }
}
