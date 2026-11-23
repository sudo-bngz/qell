//! Application state and the top-level frame layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, Key, Modifiers};
use qell_core::{Buffer, Cursor, LanguageRegistry, Position};
use qell_git::{RepoStatus, Watcher as GitWatcher, WorkspaceStatus};
use qell_lsp::convert::from_lsp_position;
use qell_lsp::lsp_types::{CompletionItem, Diagnostic, InsertTextFormat};
use qell_lsp::{convert, Event as LspEvent, Registry as LspRegistry, RequestId};
use qell_plugins::{Action as PluginAction, EditorContext, Host as PluginHost};
use qell_term::Terminal;

use crate::commands::Command;
use crate::config::{self, Config};
use crate::document::{DiskChange, Document};
use crate::theme::Palette;
use crate::views;

/// What the left sidebar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarView {
    Explorer,
    Search,
    SourceControl,
}

/// A go-to-definition or find-references request waiting for a server.
#[derive(Debug, Clone)]
pub enum NavRequest {
    Definition { word: String, lang: String },
    References { word: String },
}

/// One line of the Go to Symbol list.
#[derive(Debug, Clone)]
pub struct SymbolRow {
    pub name: String,
    pub kind: String,
    /// What it is defined in, when the language server says so.
    pub container: String,
    pub line: usize,
    pub col: usize,
    pub end_col: usize,
}

/// A manifest tab: which object it came from, and where it goes back to.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub object: qell_k8s::Object,
    /// The context it was read from, so saving can't hit another cluster.
    pub context: String,
}

/// Char range of `word` in `preview`, for highlighting.
fn preview_range(preview: &str, _chars: &str, word: &str) -> (usize, usize) {
    let last = word.rsplit('.').next().unwrap_or(word);
    match preview.find(last).or_else(|| preview.find(word)) {
        Some(b) => {
            let s = preview[..b].chars().count();
            (s, s + last.chars().count())
        }
        None => (0, 0),
    }
}

/// An inline text field in the file tree: renaming an entry, or naming a
/// new file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEditKind {
    Rename(PathBuf),
    NewFile(PathBuf),
    NewFolder(PathBuf),
}

#[derive(Debug, Clone)]
pub struct TreeEdit {
    pub kind: TreeEditKind,
    pub text: String,
    /// Not yet focused (first frame): focus it and select the name.
    pub fresh: bool,
}

/// Where the cursor was, for Go Back / Go Forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavPoint {
    pub path: PathBuf,
    pub pos: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Terminal,
    /// The cluster view (k9s-style) has the keyboard.
    K8s,
    /// The AWS view has the keyboard.
    Aws,
}

/// Completion popup state.
#[derive(Default)]
pub struct Completion {
    pub open: bool,
    pub request: Option<RequestId>,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    /// Buffer position where the completed word starts.
    pub word_start: Position,
    /// Screen position (bottom-left of the cursor) to anchor the popup.
    pub anchor: egui::Pos2,
    /// The server's list wasn't complete: typing must ask again rather than
    /// filter what we have.
    pub incomplete: bool,
    /// Pending `completionItem/resolve`: request id and item index.
    pub resolving: Option<(RequestId, usize)>,
    /// Items already resolved (or asked for).
    pub resolved: std::collections::HashSet<usize>,
}

/// Tab stops left to visit after inserting a snippet completion.
pub struct SnippetSession {
    pub doc: usize,
    /// Character ranges in the buffer, in visiting order.
    pub stops: Vec<(usize, usize)>,
    /// The stop being edited and its end when it was selected, to measure
    /// what the user typed into it.
    pub current: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,
    Files,
    /// Jumping to one of the things the open file defines.
    Symbols,
    GoToLine,
    /// Typing the new name for the symbol at the cursor (F2).
    Rename,
    /// Picking one of the server's quick fixes.
    CodeActions,
}

/// Command palette / quick open / go-to-line overlay.
pub struct CommandPalette {
    pub open: bool,
    pub mode: PaletteMode,
    pub query: String,
    pub selected: usize,
    pub files: Vec<PathBuf>,
    pub needs_focus: bool,
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self {
            open: false,
            mode: PaletteMode::Commands,
            query: String::new(),
            selected: 0,
            files: Vec::new(),
            needs_focus: false,
        }
    }
}

/// One shell in the terminal panel.
pub struct TerminalTab {
    pub term: Terminal,
    /// Stable number shown in the tab, VS Code style ("1: bash").
    pub number: usize,
    /// What Qell started here, when it wasn't a shell ("logs api-0").
    pub name: Option<String>,
}

impl TerminalTab {
    /// Tab label: the title the shell set, else the shell's name.
    pub fn label(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let title = self.term.title().trim();
        if !title.is_empty() {
            return title.to_string();
        }
        std::env::var("SHELL")
            .ok()
            .and_then(|s| std::path::Path::new(&s).file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "shell".into())
    }
}

pub struct Notification {
    pub text: String,
    pub at: Instant,
    pub is_error: bool,
}

pub struct QellApp {
    pub config: Config,
    pub palette: Palette,
    pub root: PathBuf,
    pub docs: Vec<Document>,
    pub active: usize,
    pub focus: Focus,
    pub languages: LanguageRegistry,
    pub lsp: LspRegistry,
    pub terminals: Vec<TerminalTab>,
    pub active_terminal: usize,
    next_terminal_number: usize,
    pub plugins: Option<PluginHost>,
    pub plugin_commands: Vec<String>,
    pub completion: Completion,
    /// Item indices of the completion list in the order shown (filtered
    /// and ranked), for arrow-key navigation.
    pub completion_order: Vec<usize>,
    pub command_palette: CommandPalette,
    pub notifications: Vec<Notification>,
    pub show_sidebar: bool,
    pub show_terminal: bool,
    pub font_size: f32,
    /// Cached directory listings for the file tree.
    pub tree_cache: HashMap<PathBuf, Vec<views::sidebar::Entry>>,
    pub pending_commands: Vec<Command>,
    pub pending_plugin_commands: Vec<String>,
    /// Background `git status` of every repository in the workspace;
    /// `git_status` is `None` when there is none.
    pub git: GitWatcher,
    pub git_status: Option<WorkspaceStatus>,
    was_focused: bool,
    /// The Settings window.
    pub settings: views::settings::SettingsState,
    pub sidebar_view: SidebarView,
    /// In-memory index of the workspace, for search and definitions.
    pub indexer: qell_index::Indexer,
    pub searcher: qell_index::Searcher,
    pub search: views::search::SearchState,
    pub pending_nav: Option<(RequestId, NavRequest)>,
    /// A rename waiting on the server, with the old and the new name.
    pending_rename: Option<(RequestId, String, String)>,
    /// A code-action request waiting on the server, and the actions it
    /// came back with (what the Quick Fix palette lists).
    pending_actions: Option<RequestId>,
    pub code_actions: Vec<qell_lsp::lsp_types::CodeActionOrCommand>,
    /// What the open file defines, for Go to Symbol.
    pub symbols: Vec<SymbolRow>,
    pending_symbols: Option<RequestId>,
    /// Reads definitions out of a file; built the first time it is needed
    /// (compiling the tree-sitter queries takes a moment).
    extractor: Option<qell_index::symbols::Extractor>,
    /// A chosen action whose edit the server is still filling in.
    pending_action_resolve: Option<RequestId>,
    /// The symbol F2 is renaming, shown in the prompt.
    pub rename_symbol: String,
    /// Git's version of open files, fetched off the UI thread for the
    /// gutter's change marks.
    git_base_tx: std::sync::mpsc::Sender<(PathBuf, Option<String>)>,
    git_base_rx: std::sync::mpsc::Receiver<(PathBuf, Option<String>)>,
    /// Find and replace in the open file.
    pub find: views::find::FindState,
    /// The tabs of each workspace, reopened on the next start.
    session: crate::session::Store,
    /// Paths and active tab the session was last written for.
    session_key: (Vec<PathBuf>, usize),
    /// Files opened in each workspace, for the file tree's shortcuts.
    pub recent: crate::recent::Recent,
    /// Expand the tree down to this path and scroll to it.
    pub reveal: Option<PathBuf>,
    /// Vim mode state, and what its commands asked the app to do.
    pub vim: crate::vim::Vim,
    pub pending_vim: Vec<crate::vim::Action>,
    /// A formatting request waiting on a language server, and whether the
    /// file is to be saved once its edits arrive.
    pending_format: Option<(RequestId, bool)>,
    /// Which pane had the keyboard last frame, to notice coming back to
    /// the editor from the terminal.
    last_focus: Focus,
    /// The cluster view (k9s-style), when it is open.
    pub k8s: views::k8s::K8sState,
    pub aws: views::aws::AwsState,
    /// Manifest tabs: the temp file each one is in, and the object it
    /// goes back to when saved.
    pub manifests: HashMap<PathBuf, Manifest>,
    /// A manifest that was just saved and is waiting to be applied.
    pending_apply: Option<PathBuf>,
    /// A tab that came from the cluster view was closed or escaped out
    /// of: show the list again where it was left.
    pub back_to_cluster: bool,
    /// A change mark in the gutter was clicked: its line and where its
    /// Revert / Stage menu goes.
    pub hunk_menu: Option<(usize, egui::Pos2)>,
    /// Rename / new file / new folder being typed in the file tree.
    pub tree_edit: Option<TreeEdit>,
    /// "Move to Trash?" asked for this path.
    pub confirm_delete: Option<PathBuf>,
    /// Go Back / Go Forward stacks.
    pub nav_back: Vec<NavPoint>,
    pub nav_forward: Vec<NavPoint>,
    pub snippet: Option<SnippetSession>,
    /// For waking the UI from background threads.
    pub egui_ctx: Context,
    /// The native menu bar, once installed (macOS only).
    pub native_menu: Option<crate::native_menu::NativeMenu>,
    pub scm: views::scm::ScmState,
    /// The config as last applied to the running editor; `config` is what
    /// the Settings window edits.
    applied_config: Config,
    /// Unsaved settings changes, saved shortly after the last edit.
    config_dirty_since: Option<Instant>,
}

impl QellApp {
    pub fn new(cc: &eframe::CreationContext<'_>, root: PathBuf, file: Option<PathBuf>) -> Self {
        let config = Config::load();
        let palette = Palette::dark();
        palette.apply(&cc.egui_ctx);
        crate::icons::install(&cc.egui_ctx);
        cc.egui_ctx.set_zoom_factor(config.ui_scale);

        // No plugins installed: don't start the WebAssembly runtime at all.
        let mut plugins = if !PluginHost::has_plugins(&config::plugins_dir()) {
            None
        } else {
            match PluginHost::new() {
                Ok(h) => Some(h),
                Err(e) => {
                    log::error!("plugin host unavailable: {e:#}");
                    None
                }
            }
        };
        let plugin_commands = plugins
            .as_mut()
            .map(|h| {
                let loaded = h.load_dir(&config::plugins_dir());
                for p in &loaded {
                    log::info!("loaded plugin {} ({} commands)", p.name, p.commands.len());
                }
                h.commands()
            })
            .unwrap_or_default();

        let lsp = new_lsp(&cc.egui_ctx, &root, &config);

        let git = GitWatcher::new(root.clone(), config.git_scan());
        let wake: qell_index::Waker = {
            let ctx = cc.egui_ctx.clone();
            std::sync::Arc::new(move || ctx.request_repaint())
        };
        let indexer = qell_index::Indexer::new(root.clone(), config.ignore.clone(), Some(wake.clone()));
        let searcher = qell_index::Searcher::new(indexer.index(), Some(wake));
        let (git_base_tx, git_base_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            applied_config: config.clone(),
            config_dirty_since: None,
            settings: Default::default(),
            sidebar_view: SidebarView::Explorer,
            indexer,
            searcher,
            search: Default::default(),
            pending_nav: None,
            pending_rename: None,
            pending_actions: None,
            code_actions: Vec::new(),
            symbols: Vec::new(),
            pending_symbols: None,
            extractor: None,
            pending_action_resolve: None,
            rename_symbol: String::new(),
            git_base_tx,
            git_base_rx,
            find: Default::default(),
            session: crate::session::Store::load(),
            session_key: (Vec::new(), 0),
            recent: crate::recent::Recent::load(),
            reveal: None,
            vim: crate::vim::Vim::new(),
            pending_vim: Vec::new(),
            pending_format: None,
            last_focus: Focus::Editor,
            k8s: views::k8s::K8sState::default(),
            aws: views::aws::AwsState::default(),
            manifests: HashMap::new(),
            pending_apply: None,
            back_to_cluster: false,
            hunk_menu: None,
            tree_edit: None,
            confirm_delete: None,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            snippet: None,
            egui_ctx: cc.egui_ctx.clone(),
            native_menu: None,
            scm: Default::default(),
            show_sidebar: config.show_sidebar,
            show_terminal: config.show_terminal,
            font_size: config.font_size,
            config,
            palette,
            languages: LanguageRegistry::new(),
            lsp,
            root,
            docs: Vec::new(),
            active: 0,
            focus: Focus::Editor,
            terminals: Vec::new(),
            active_terminal: 0,
            next_terminal_number: 1,
            plugins,
            plugin_commands,
            completion: Completion::default(),
            completion_order: Vec::new(),
            command_palette: CommandPalette::default(),
            notifications: Vec::new(),
            tree_cache: HashMap::new(),
            pending_commands: Vec::new(),
            pending_plugin_commands: Vec::new(),
            git,
            git_status: None,
            was_focused: true,
        };
        let restored = if app.config.restore_session { app.restore_session() } else { 0 };
        match file {
            Some(f) => app.open_file(f),
            // Only an empty workspace opens on a blank tab.
            None if restored == 0 => app.docs.push(Document::untitled()),
            None => {}
        }
        app.session_key = app.session_state().0;
        app
    }

    // ---- session ---------------------------------------------------------

    /// The tabs to remember: saved files, in order, and which was in front.
    fn session_state(&self) -> ((Vec<PathBuf>, usize), crate::session::Workspace) {
        let mut ws = crate::session::Workspace::default();
        let mut paths = Vec::new();
        for (i, doc) in self.docs.iter().enumerate() {
            // Untitled tabs and read-only diffs have nothing to reopen,
            // and a manifest is a scratch copy of something in a cluster:
            // it is fetched again by `e`, not restored from a temp file.
            let Some(path) = doc.buffer.path().filter(|_| !doc.read_only && !doc.from_cluster) else { continue };
            if i == self.active {
                ws.active = ws.tabs.len();
            }
            let head = doc.buffer.cursor.head();
            ws.tabs.push(crate::session::Tab { path: path.to_path_buf(), line: head.line, col: head.col });
            paths.push(path.to_path_buf());
        }
        ((paths, ws.active), ws)
    }

    /// Write the session (tabs opened or closed, or Qell closing).
    fn save_session(&mut self) {
        if !self.config.restore_session {
            return;
        }
        let (key, ws) = self.session_state();
        self.session_key = key;
        let root = self.root.clone();
        self.session.save(&root, ws);
    }

    /// Reopen the tabs of this workspace. Returns how many opened.
    fn restore_session(&mut self) -> usize {
        let ws = self.session.take(&self.root);
        for tab in &ws.tabs {
            match Document::open(tab.path.clone()) {
                Ok(mut doc) => {
                    let p = doc.buffer.clamp(Position::new(tab.line, tab.col));
                    doc.buffer.set_cursor(p, false);
                    doc.scroll_to_cursor = true;
                    self.docs.push(doc);
                }
                Err(e) => log::warn!("cannot reopen {}: {e}", tab.path.display()),
            }
        }
        if !self.docs.is_empty() {
            self.active = ws.active.min(self.docs.len() - 1);
        }
        self.docs.len()
    }

    // ---- documents -------------------------------------------------------

    pub fn active_doc(&mut self) -> Option<&mut Document> {
        self.docs.get_mut(self.active)
    }

    /// Bring the editor forward. The cluster view stands in its place
    /// rather than beside it, so showing one puts the other away.
    pub fn show_editor_area(&mut self) {
        self.close_places();
        self.focus = Focus::Editor;
    }

    /// Whether the cluster or the AWS view is standing where the editor
    /// would be.
    pub fn in_a_place(&self) -> bool {
        self.k8s.open || self.aws.open
    }

    /// Put the cluster and AWS views away; says whether one was up.
    pub fn close_places(&mut self) -> bool {
        let was = self.in_a_place();
        if self.k8s.open {
            views::k8s::close(self);
        }
        if self.aws.open {
            views::aws::close(self);
        }
        was
    }

    /// Who gets the keyboard when the terminal lets go of it.
    fn focus_behind_terminal(&self) -> Focus {
        if self.k8s.open {
            Focus::K8s
        } else if self.aws.open {
            Focus::Aws
        } else {
            Focus::Editor
        }
    }

    pub fn open_file(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.recent.record(&self.root, &path);
        if let Some(i) = self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path())) {
            self.active = i;
            self.show_editor_area();
            return;
        }
        match Document::open(path.clone()) {
            Ok(doc) => {
                // Replace a pristine untitled tab instead of piling up.
                if self.docs.len() == 1 && self.docs[0].buffer.path().is_none() && !self.docs[0].buffer.is_dirty() {
                    self.docs.clear();
                }
                self.docs.push(doc);
                self.active = self.docs.len() - 1;
                self.show_editor_area();
            }
            Err(e) => self.notify_error(format!("Cannot open {}: {e}", path.display())),
        }
    }

    /// Open (or switch to) a read-only inline diff of one file in a commit.
    pub fn open_commit_diff(&mut self, repo: &Path, commit: &qell_git::graph::Commit, file: &qell_git::graph::ChangedFile) {
        let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let label = format!("{name} ({})", commit.short_hash());
        if let Some(i) = self.docs.iter().position(|d| d.label.as_deref() == Some(label.as_str())) {
            self.active = i;
            self.show_editor_area();
            return;
        }
        match qell_git::graph::file_diff(&self.root.join(repo), commit, file) {
            Ok(diff) => {
                use qell_git::graph::LineKind;
                let marks = diff
                    .kinds
                    .iter()
                    .map(|k| match k {
                        LineKind::Added => crate::document::LineMark::Added,
                        LineKind::Removed => crate::document::LineMark::Removed,
                        LineKind::Context => crate::document::LineMark::Context,
                    })
                    .collect();
                // Replace a pristine untitled tab instead of piling up.
                if self.docs.len() == 1
                    && self.docs[0].buffer.path().is_none()
                    && self.docs[0].label.is_none()
                    && !self.docs[0].buffer.is_dirty()
                {
                    self.docs.clear();
                }
                self.docs.push(Document::diff(label, &file.path, &diff.text, marks));
                self.active = self.docs.len() - 1;
                self.show_editor_area();
                self.notify(format!("{}: +{} −{} in {}", file.path.display(), diff.added, diff.removed, commit.short_hash()));
            }
            Err(e) => self.notify_error(format!("Cannot show the diff: {e}")),
        }
    }

    pub fn open_folder(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        // Keep the tabs of the folder being left.
        self.save_session();
        self.lsp.shutdown_all();
        self.lsp = new_lsp(&self.egui_ctx, &path, &self.config);
        self.git.set_root(path.clone());
        self.indexer.set_root(path.clone());
        self.search = Default::default();
        self.nav_back.clear();
        self.nav_forward.clear();
        self.git_status = None;
        self.root = path;
        self.tree_cache.clear();
        self.command_palette.files.clear();
        // Shells restart in the new directory.
        self.terminals.clear();
        self.next_terminal_number = 1;
        self.notify(format!("Opened {}", self.root.display()));
    }

    fn save_active(&mut self, force_dialog: bool) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if doc.read_only {
            self.notify("This is a read-only view.");
            return;
        }
        // Format first, then write (the formatter may answer later).
        if self.config.format_on_save && !force_dialog && doc.buffer.path().is_some() {
            self.format_active(true);
        } else {
            self.write_active_or_ask(force_dialog);
        }
    }

    /// Write the file, after formatting if that was asked for.
    fn write_active(&mut self) {
        self.write_active_or_ask(false);
    }

    fn write_active_or_ask(&mut self, force_dialog: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if doc.buffer.path().is_none() || force_dialog {
            let Some(path) = rfd::FileDialog::new().set_directory(&self.root).save_file() else { return };
            if let Err(e) = doc.buffer.save_as(path) {
                self.notify_error(format!("Save failed: {e}"));
                return;
            }
        } else {
            // Never write over a change made outside Qell without asking.
            if let Some(change) = doc.disk_change() {
                if change == DiskChange::Modified {
                    doc.conflict = Some(change);
                    let name = doc.buffer.file_name();
                    self.notify_error(format!("{name} changed on disk: reload it or keep yours first"));
                    return;
                }
            }
            if let Err(e) = doc.buffer.save() {
                self.notify_error(format!("Save failed: {e}"));
                return;
            }
        }
        doc.note_disk();
        // A manifest tab goes back to the cluster when it is saved,
        // which is what `e` in the cluster view is for. The apply runs
        // on a worker, so it is started from the frame loop.
        if let Some(path) = doc.buffer.path().map(Path::to_path_buf) {
            if self.manifests.contains_key(&path) {
                self.pending_apply = Some(path);
            }
        }
        self.lsp.did_save(&doc.buffer);
        self.git.refresh();
        self.indexer.refresh();
        let name = doc.buffer.file_name();
        let saved_config = doc.buffer.path().is_some_and(|p| same_file(p, &config::config_path()));
        self.notify(format!("Saved {name}"));
        if saved_config {
            self.reload_config_file();
        }
    }

    fn close_active(&mut self) {
        if self.docs.is_empty() {
            return;
        }
        let doc = self.docs.remove(self.active);
        self.back_to_cluster |= doc.from_cluster;
        self.lsp.did_close(&doc.buffer);
        if self.docs.is_empty() {
            self.docs.push(Document::untitled());
        }
        self.active = self.active.min(self.docs.len() - 1);
        self.completion.open = false;
    }

    /// Close the tabs whose index matches (tab menu: others, to the right,
    /// all). Closing everything leaves an empty untitled tab.
    pub fn close_tabs_where(&mut self, close: impl Fn(usize) -> bool) {
        let mut kept = Vec::new();
        let mut active = None;
        let mut closed: Vec<bool> = Vec::new();
        for (i, doc) in std::mem::take(&mut self.docs).into_iter().enumerate() {
            if close(i) {
                closed.push(doc.from_cluster);
                self.lsp.did_close(&doc.buffer);
            } else {
                if i == self.active {
                    active = Some(kept.len());
                }
                kept.push(doc);
            }
        }
        if kept.is_empty() {
            kept.push(Document::untitled());
        }
        // The active tab closed: the nearest one to its left takes over.
        let fallback = self.active.min(kept.len()).saturating_sub(1);
        self.active = active.unwrap_or(fallback).min(kept.len() - 1);
        self.docs = kept;
        // Closing the one cluster tab goes back to the list; closing a
        // whole row of tabs is housekeeping, and shouldn't open a view.
        self.back_to_cluster |= closed.as_slice() == [true];
        self.completion.open = false;
    }

    /// Show a sidebar view; asking for the one already shown hides the
    /// sidebar, like clicking its icon in VS Code.
    pub fn show_sidebar_view(&mut self, view: SidebarView) {
        // Explorer, search and git are about files: asking for one of
        // them is asking for the editor back.
        if self.close_places() {
            self.sidebar_view = view;
            self.show_sidebar = true;
            return;
        }
        if self.show_sidebar && self.sidebar_view == view {
            self.show_sidebar = false;
        } else {
            self.sidebar_view = view;
            self.show_sidebar = true;
        }
    }

    /// Install the native menu on the first frame (the application must be
    /// running for AppKit to take it), then turn chosen items into actions.
    fn handle_native_menu(&mut self, ctx: &Context) {
        #[cfg(target_os = "macos")]
        if self.native_menu.is_none() {
            match crate::native_menu::NativeMenu::install(ctx) {
                Ok(m) => self.native_menu = Some(m),
                Err(e) => log::error!("native menu unavailable: {e}"),
            }
        }
        let Some(menu) = &self.native_menu else { return };
        for action in menu.poll() {
            match action {
                crate::native_menu::Action::Command(cmd) => self.pending_commands.push(cmd),
                crate::native_menu::Action::Edit(e) => {
                    // Replayed as input so it acts on whatever has focus.
                    if let Some(ev) = crate::native_menu::NativeMenu::edit_event(e) {
                        ctx.input_mut(|i| i.events.push(ev));
                    }
                }
            }
        }
    }

    fn handle_scm_jobs(&mut self) {
        for job in self.scm.poll() {
            self.git.refresh();
            match (job.kind, job.result) {
                (views::scm::JobKind::Commit, Ok(out)) => {
                    self.scm.messages.remove(&job.repo);
                    // "[main 1a2b3c4] subject" → "1a2b3c4 subject"
                    let first = out.lines().next().unwrap_or("").trim();
                    let summary = first
                        .split_once("] ")
                        .map(|(h, s)| format!("{} {s}", h.rsplit(' ').next().unwrap_or("")))
                        .unwrap_or(first.to_string());
                    self.notify(format!("Committed {summary}"));
                }
                (_, Ok(_)) => {}
                (kind, Err(e)) => {
                    log::error!("git {kind:?} failed: {e}");
                    // Hooks can print a lot: show the last lines, which say why.
                    let tail: Vec<&str> = e.lines().filter(|l| !l.trim().is_empty()).collect();
                    let msg = tail[tail.len().saturating_sub(2)..].join(" — ");
                    self.notify_error(format!("git {} failed: {msg}", format!("{kind:?}").to_lowercase()));
                }
            }
        }
    }

    // ---- go to definition / references ------------------------------------

    /// Language and symbol (word, or Terraform address) at the cursor.
    fn symbol_at_cursor(&self) -> Option<(String, String)> {
        let doc = self.docs.get(self.active)?;
        let lang = doc.buffer.language().unwrap_or("").to_string();
        let head = doc.buffer.cursor.head();
        let line = doc.buffer.line(head.line);
        let (s, e) = views::editor::word_range(&line, head.col, lang == "terraform")?;
        Some((lang, line.chars().skip(s).take(e - s).collect()))
    }

    /// F12 / Cmd+click: jump to where the symbol at the cursor is defined.
    /// Asks the language server when there is one; otherwise (or when it
    /// finds nothing) uses Qell's own index.
    pub fn go_to_definition(&mut self) {
        let Some((lang, word)) = self.symbol_at_cursor() else { return };
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match self.lsp.request_definition(&self.languages, &mut doc.buffer) {
            Some(id) => self.pending_nav = Some((id, NavRequest::Definition { word, lang })),
            None => self.builtin_definition(&word, &lang),
        }
    }

    /// Shift+F12: every reference to the symbol at the cursor.
    pub fn find_references(&mut self) {
        let Some((_, word)) = self.symbol_at_cursor() else { return };
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match self.lsp.request_references(&self.languages, &mut doc.buffer) {
            Some(id) => self.pending_nav = Some((id, NavRequest::References { word })),
            None => self.text_search_for(&word),
        }
    }

    /// Without a server: a whole-word search for `word`, in the Search view.
    fn text_search_for(&mut self, word: &str) {
        self.show_editor_area();
        self.sidebar_view = SidebarView::Search;
        self.show_sidebar = true;
        self.search.whole_word = !word.contains('.');
        self.search.regex = false;
        self.search.case_sensitive = true;
        self.search.start(Some(word.to_string()));
    }

    /// Definitions from the index: tree-sitter tags, Terraform blocks.
    fn builtin_definition(&mut self, word: &str, lang: &str) {
        let current = self.docs.get(self.active).and_then(|d| d.buffer.path()).map(Path::to_path_buf);
        let terraform = lang == "terraform";
        let index = self.indexer.index();
        let index = index.read().unwrap();
        let names = if terraform { qell_index::terraform_candidates(word) } else { vec![word.to_string()] };
        let mut defs = names.iter().map(|n| index.definitions(n)).find(|d| !d.is_empty()).unwrap_or_default();
        drop(index);
        if defs.is_empty() {
            if self.indexer.status().indexing {
                self.notify(format!("Still indexing the workspace: try again in a moment to find {word}."));
            } else {
                self.notify(format!("No definition found for {word}: showing where it appears."));
                self.text_search_for(word);
            }
            return;
        }
        // Closest first: same file, same folder, then by path.
        let dir = current.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf);
        defs.sort_by_key(|d| (Some(&d.path) != current.as_ref(), d.path.parent() != dir.as_deref(), d.rel.clone(), d.line));
        // Terraform resolves names within a module, i.e. a folder.
        if terraform && defs.iter().any(|d| d.path.parent() == dir.as_deref()) {
            defs.retain(|d| d.path.parent() == dir.as_deref());
        }
        let items: Vec<views::search::ListItem> = defs
            .into_iter()
            .map(|d| views::search::ListItem {
                preview_range: preview_range(&d.preview, &d.preview.chars().collect::<String>(), word),
                path: d.path,
                rel: d.rel,
                line: d.line,
                col: d.col,
                end_col: d.end_col,
                preview: d.preview,
            })
            .collect();
        self.show_locations(format!("Definitions of {word}"), items, true);
    }

    /// One place: go there. Several: list them in the Search view (and go
    /// to the first when `jump_first`).
    fn show_locations(&mut self, title: String, items: Vec<views::search::ListItem>, jump_first: bool) {
        match items.as_slice() {
            [] => {}
            [one] => self.open_location(&one.path.clone(), one.line, one.col, one.end_col),
            [first, ..] => {
                if jump_first {
                    self.open_location(&first.path.clone(), first.line, first.col, first.end_col);
                }
                self.sidebar_view = SidebarView::Search;
                self.show_sidebar = true;
                self.search.list = Some(views::search::LocationList { title: format!("{title} ({})", items.len()), items });
            }
        }
    }

    /// Text of a file: the open buffer, the index, or the disk.
    fn file_text(&self, path: &Path) -> Option<String> {
        if let Some(d) = self.docs.iter().find(|d| d.buffer.path() == Some(path)) {
            return Some(d.buffer.text());
        }
        if let Some(t) = self.indexer.index().read().unwrap().text(path) {
            return Some(t.to_string());
        }
        std::fs::read_to_string(path).ok()
    }

    /// A language server's locations, as list items (UTF-16 → chars).
    fn lsp_items(&self, locations: Vec<qell_lsp::lsp_types::Location>) -> Vec<views::search::ListItem> {
        let mut texts: HashMap<PathBuf, Option<String>> = HashMap::new();
        locations
            .into_iter()
            .filter_map(|l| {
                let path = convert::uri_to_path(&l.uri)?;
                let text = texts.entry(path.clone()).or_insert_with(|| self.file_text(&path)).clone()?;
                let line_text = text.lines().nth(l.range.start.line as usize).unwrap_or("");
                let start = from_lsp_position(line_text, l.range.start);
                let end = if l.range.end.line == l.range.start.line { from_lsp_position(line_text, l.range.end) } else { start };
                let lead = line_text.chars().take_while(|c| c.is_whitespace()).count();
                let preview: String = line_text.trim().chars().take(200).collect();
                let rel = path.strip_prefix(&self.root).unwrap_or(&path).to_path_buf();
                Some(views::search::ListItem {
                    preview_range: (start.col.saturating_sub(lead), end.col.max(start.col).saturating_sub(lead)),
                    path,
                    rel,
                    line: start.line,
                    col: start.col,
                    end_col: end.col.max(start.col),
                    preview,
                })
            })
            .collect()
    }

    fn handle_locations(&mut self, id: RequestId, locations: Vec<qell_lsp::lsp_types::Location>) {
        let Some((pending, req)) = self.pending_nav.take() else { return };
        if pending != id {
            self.pending_nav = Some((pending, req));
            return;
        }
        let items = self.lsp_items(locations);
        match req {
            // The server found nothing: Qell's own index may.
            NavRequest::Definition { word, lang } if items.is_empty() => self.builtin_definition(&word, &lang),
            NavRequest::Definition { word, .. } => self.show_locations(format!("Definitions of {word}"), items, true),
            NavRequest::References { word } if items.is_empty() => self.notify(format!("No references to {word}.")),
            NavRequest::References { word } => self.show_locations(format!("References to {word}"), items, false),
        }
    }

    // ---- text from the cluster --------------------------------------------

    /// Open an object's manifest in a tab you can edit. It is a real file
    /// in a temp folder, so saving works as everywhere else — and because
    /// it is a manifest, saving also sends it back to the cluster.
    pub fn open_manifest_tab(&mut self, object: qell_k8s::Object, context: String, text: &str) -> std::io::Result<()> {
        let dir = std::env::temp_dir().join("qell-manifests");
        std::fs::create_dir_all(&dir)?;
        let safe = |s: &str| s.replace(['/', '\\', ' ', ':'], "-");
        let name = format!(
            "{}{}-{}.yaml",
            object.namespace.as_ref().map(|n| format!("{}-", safe(n))).unwrap_or_default(),
            safe(&object.kind),
            safe(&object.name)
        );
        let path = dir.join(name);
        // Already open: don't write over edits that haven't been applied,
        // and don't pile up a second tab for the same object.
        let label = format!("{} (manifest)", object.slash());
        if let Some(i) = self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path())) {
            self.active = i;
            self.show_editor_area();
            if self.docs[i].buffer.is_dirty() {
                self.notify("This manifest is already open, with changes you haven't applied.");
            } else {
                // Not touched since: show what the cluster has now.
                std::fs::write(&path, text)?;
                let mut doc = Document::open(path.clone())?;
                doc.label = Some(label);
                doc.from_cluster = true;
                self.docs[i] = doc;
                self.manifests.insert(path, Manifest { object, context });
            }
            return Ok(());
        }
        std::fs::write(&path, text)?;
        self.manifests.insert(path.clone(), Manifest { object, context });
        self.drop_pristine_tab();
        let mut doc = Document::open(path)?;
        doc.label = Some(label);
        doc.from_cluster = true;
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        self.show_editor_area();
        Ok(())
    }

    /// The manifest the active tab holds, if it holds one.
    pub fn active_manifest(&self) -> Option<&Manifest> {
        let path = self.docs.get(self.active)?.buffer.path()?;
        self.manifests.get(path)
    }

    /// Send a saved manifest back to the cluster, off the UI thread.
    fn apply_manifest(&mut self, path: PathBuf, ctx: &Context) {
        let Some(manifest) = self.manifests.get(&path) else { return };
        let mut client = self.k8s.client.clone();
        client.context = Some(manifest.context.clone()).filter(|c| !c.is_empty());
        let what = manifest.object.slash();
        self.k8s.apply(client, path, what, ctx);
    }

    /// Replace a tab that holds nothing rather than piling up.
    fn drop_pristine_tab(&mut self) {
        if self.docs.len() == 1
            && self.docs[0].buffer.path().is_none()
            && self.docs[0].label.is_none()
            && !self.docs[0].buffer.is_dirty()
        {
            self.docs.clear();
        }
    }

    // ---- git hunks from the gutter ----------------------------------------

    /// The change `line` sits in, with git's text for the file.
    fn hunk_at(&self, line: usize) -> Option<(crate::document::Hunk, std::sync::Arc<str>)> {
        let doc = self.docs.get(self.active)?;
        let base = doc.git_base.clone()?;
        let hunk = crate::document::hunks(&base, &doc.buffer.text()).into_iter().find(|h| h.covers(line))?;
        Some((hunk, base))
    }

    pub fn stage_hunk_at_cursor(&mut self) {
        let line = self.docs.get(self.active).map(|d| d.buffer.cursor.head().line).unwrap_or(0);
        self.stage_hunk(line);
    }

    pub fn revert_hunk_at_cursor(&mut self) {
        let line = self.docs.get(self.active).map(|d| d.buffer.cursor.head().line).unwrap_or(0);
        self.revert_hunk(line);
    }

    /// Put one change into the index, leaving the rest of the file alone.
    /// The file must be saved first: git stages what is on disk.
    pub fn stage_hunk(&mut self, line: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let Some(path) = doc.buffer.path().map(Path::to_path_buf) else { return };
        if doc.buffer.is_dirty() {
            self.notify("Save the file first: git stages what is on disk.");
            return;
        }
        let Some((hunk, base)) = self.hunk_at(line) else {
            self.notify("No change here to stage.");
            return;
        };
        let Some((root, rel)) = qell_git::repo_of(&path) else {
            self.notify("This file isn't in a git repository.");
            return;
        };
        let text = self.docs.get(self.active).map(|d| d.buffer.text()).unwrap_or_default();
        let patch = qell_git::unified_patch(&rel, &base, &text, hunk.base.clone(), hunk.new.clone());
        match qell_git::apply_cached(&root, &patch) {
            Ok(()) => {
                let n = hunk.new.len().max(hunk.base.len());
                self.notify(format!("Staged {n} line{} of {rel}.", if n == 1 { "" } else { "s" }));
                // The gutter compares with the index, which just moved.
                if let Some(doc) = self.docs.get_mut(self.active) {
                    doc.git_base_state = crate::document::BaseState::Unknown;
                }
                self.git.refresh();
            }
            Err(e) => self.notify_error(format!("git apply failed: {}", e.lines().next().unwrap_or(&e))),
        }
    }

    /// Put one change back to git's version, in the editor and unsaved.
    pub fn revert_hunk(&mut self, line: usize) {
        let Some((hunk, base)) = self.hunk_at(line) else {
            self.notify("No change here to revert.");
            return;
        };
        let Some(doc) = self.docs.get_mut(self.active).filter(|d| !d.read_only) else { return };
        let was: Vec<&str> = base.split_inclusive('\n').collect();
        let text: String = was.get(hunk.base.clone()).unwrap_or_default().concat();
        // The whole run of lines: from the start of the first to the start
        // of the one after it (at the end of the file, to its very end).
        let start = Position::new(hunk.new.start, 0);
        let end = if doc.buffer.line_count() > hunk.new.end { Position::new(hunk.new.end, 0) } else { doc.buffer.end() };
        doc.buffer.begin_undo_group();
        doc.buffer.replace_range(start, end, &text);
        doc.buffer.end_undo_group();
        let at = doc.buffer.clamp(Position::new(hunk.new.start, 0));
        doc.buffer.set_cursor(at, false);
        doc.scroll_to_cursor = true;
        let n = hunk.base.len().max(hunk.new.len());
        self.notify(format!("Put {n} line{} back to git's version, unsaved.", if n == 1 { "" } else { "s" }));
    }

    // ---- replace across files ---------------------------------------------

    /// Put the Search view's replacement in place of every match in the
    /// workspace. Files open in a tab are changed in the editor, unsaved,
    /// so you can read them over and undo; the others are written to disk,
    /// which is what the confirmation is for.
    pub fn replace_in_files(&mut self) {
        let query = self.search.query();
        let re = match query.regex() {
            Ok(re) => re,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let Some(Ok(results)) = &self.search.results else { return };
        let paths: Vec<PathBuf> = results.files.iter().map(|f| f.path.clone()).collect();
        let truncated = results.truncated;
        let with = self.search.replace.clone();
        // `$1` in the replacement means a capture group only when the
        // pattern is a regular expression; otherwise it is just text.
        let swap = |text: &str| -> String {
            if query.is_regex {
                re.replace_all(text, with.as_str()).into_owned()
            } else {
                re.replace_all(text, regex::NoExpand(with.as_str())).into_owned()
            }
        };
        let (mut places, mut in_tabs, mut on_disk, mut failed) = (0usize, 0usize, 0usize, 0usize);
        for path in paths {
            let open = self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path()));
            match open {
                Some(i) => {
                    let Some(doc) = self.docs.get_mut(i) else { continue };
                    if doc.read_only {
                        failed += 1;
                        continue;
                    }
                    let old = doc.buffer.text();
                    let hits = re.find_iter(&old).count();
                    let new = swap(&old);
                    if hits == 0 || new == old {
                        continue;
                    }
                    apply_text(doc, &old, &new);
                    places += hits;
                    in_tabs += 1;
                }
                None => {
                    // Read the file again rather than trust the index.
                    let Ok(old) = std::fs::read_to_string(&path) else {
                        failed += 1;
                        continue;
                    };
                    let hits = re.find_iter(&old).count();
                    let new = swap(&old);
                    if hits == 0 || new == old {
                        continue;
                    }
                    match std::fs::write(&path, &new) {
                        Ok(()) => {
                            places += hits;
                            on_disk += 1;
                        }
                        Err(e) => {
                            log::warn!("replace in {}: {e}", path.display());
                            failed += 1;
                        }
                    }
                }
            }
        }
        let plural = |n: usize, s: &str| if n == 1 { format!("1 {s}") } else { format!("{n} {s}s") };
        if places == 0 {
            self.notify("Nothing to replace: the files no longer match.");
        } else {
            let mut msg = format!("Replaced {}", plural(places, "match"));
            match (on_disk, in_tabs) {
                (0, t) => msg.push_str(&format!(" in {}, unsaved", plural(t, "open file"))),
                (d, 0) => msg.push_str(&format!(" in {}, written to disk", plural(d, "file"))),
                (d, t) => msg.push_str(&format!(
                    " in {} written to disk and {} left unsaved",
                    plural(d, "file"),
                    plural(t, "open file")
                )),
            }
            msg.push('.');
            if truncated {
                msg.push_str(" The result list was cut short, so search again to see if any are left.");
            }
            self.notify(msg);
        }
        if failed > 0 {
            self.notify_error(format!("{} could not be changed.", plural(failed, "file")));
        }
        // The index and the results are now out of date.
        self.indexer.refresh();
        self.search.search_again();
    }

    // ---- go to symbol -----------------------------------------------------

    /// Ctrl/Cmd+Shift+O: list what the open file defines and jump to one.
    /// Qell reads the file itself for the languages it can; for the others
    /// it asks the language server.
    pub fn go_to_symbol(&mut self) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let lang = doc.buffer.language().unwrap_or("").to_string();
        let text = doc.buffer.text();
        let extractor = self.extractor.get_or_insert_with(qell_index::symbols::Extractor::new);
        if extractor.supports(&lang) {
            let mut rows: Vec<SymbolRow> = extractor
                .extract(&lang, &text)
                .into_iter()
                .map(|s| SymbolRow {
                    name: s.name,
                    kind: s.kind,
                    container: String::new(),
                    line: s.line,
                    col: s.col,
                    end_col: s.end_col,
                })
                .collect();
            rows.sort_by_key(|s| (s.line, s.col));
            if rows.is_empty() {
                self.notify("Nothing defined in this file.");
                return;
            }
            self.symbols = rows;
            self.open_palette(PaletteMode::Symbols);
            return;
        }
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match self.lsp.request_symbols(&self.languages, &mut doc.buffer) {
            Some(id) => {
                self.pending_symbols = Some(id);
                self.notify("Reading the symbols in this file…");
            }
            None => self.notify(self.no_server_hint(&lang, "Listing what a file defines needs")),
        }
    }

    fn handle_symbols(&mut self, id: RequestId, symbols: Vec<qell_lsp::FileSymbol>) {
        if self.pending_symbols != Some(id) {
            return;
        }
        self.pending_symbols = None;
        let Some(doc) = self.docs.get(self.active) else { return };
        let mut rows: Vec<SymbolRow> = symbols
            .into_iter()
            .map(|s| {
                let line = s.range.start.line as usize;
                let text = doc.buffer.line(line);
                let start = from_lsp_position(&text, s.range.start);
                let end = from_lsp_position(&text, s.range.end);
                SymbolRow {
                    name: s.name,
                    kind: s.kind,
                    container: s.container,
                    line,
                    col: start.col,
                    end_col: end.col.max(start.col),
                }
            })
            .collect();
        rows.sort_by_key(|s| (s.line, s.col));
        if rows.is_empty() {
            self.notify("The language server found nothing defined in this file.");
            return;
        }
        self.symbols = rows;
        self.open_palette(PaletteMode::Symbols);
    }

    /// Jump to the symbol the palette picked.
    pub fn go_to_symbol_row(&mut self, index: usize) {
        let Some(row) = self.symbols.get(index).cloned() else { return };
        let Some(path) = self.docs.get(self.active).and_then(|d| d.buffer.path()).map(Path::to_path_buf) else {
            // An untitled buffer has no path to navigate to: just move.
            if let Some(doc) = self.docs.get_mut(self.active) {
                doc.buffer.set_cursor(Position::new(row.line, row.col), false);
                doc.scroll_to_cursor = true;
            }
            return;
        };
        self.open_location(&path, row.line, row.col, row.end_col);
    }

    // ---- rename and quick fixes -------------------------------------------

    /// F2: ask for the new name of the symbol at the cursor. The rename
    /// itself is the language server's work, so say so when there is none.
    pub fn start_rename(&mut self) {
        let Some((_, word)) = self.symbol_at_cursor() else {
            self.notify("Put the cursor on a symbol to rename it.");
            return;
        };
        let Self { docs, active, lsp, languages, .. } = self;
        let Some(doc) = docs.get(*active).filter(|d| !d.read_only) else { return };
        let lang = doc.buffer.language().unwrap_or("").to_string();
        let can = lsp.can_rename(languages, &doc.buffer);
        if !can {
            self.notify(self.no_server_hint(&lang, "Renaming a symbol needs"));
            return;
        }
        self.rename_symbol = word.clone();
        self.open_palette(PaletteMode::Rename);
        self.command_palette.query = word;
    }

    /// Send the rename the prompt collected.
    pub fn rename_to(&mut self, new_name: &str) {
        let new_name = new_name.trim().to_string();
        let old = std::mem::take(&mut self.rename_symbol);
        if new_name.is_empty() || new_name == old {
            return;
        }
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match self.lsp.request_rename(&self.languages, &mut doc.buffer, &new_name) {
            Some(id) => self.pending_rename = Some((id, old, new_name)),
            None => self.notify("The language server stopped: nothing was renamed."),
        }
    }

    /// Ctrl/Cmd+. : what the server can do at the cursor.
    pub fn quick_fix(&mut self) {
        let Some(doc) = self.docs.get(self.active).filter(|d| !d.read_only) else { return };
        let lang = doc.buffer.language().unwrap_or("").to_string();
        // Only the diagnostics the cursor or selection touches.
        let (s, e) = doc.buffer.cursor.selection.ordered();
        let (first, last) = (s.line as u32, e.line as u32);
        let here: Vec<Diagnostic> =
            doc.diagnostics.iter().filter(|d| d.range.start.line <= last && d.range.end.line >= first).cloned().collect();
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match self.lsp.request_code_actions(&self.languages, &mut doc.buffer, here) {
            Some(id) => {
                self.pending_actions = Some(id);
                self.notify("Looking for quick fixes…");
            }
            None => self.notify(self.no_server_hint(&lang, "Quick fixes need")),
        }
    }

    /// Why a language-server feature isn't available for `lang`. `needs`
    /// opens the sentence, e.g. "Quick fixes need".
    fn no_server_hint(&self, lang: &str, needs: &str) -> String {
        let name = self.languages.get(lang).map(|l| l.name).unwrap_or(lang);
        match self.lsp.status(lang) {
            "ready" => format!("The {name} language server doesn't do that."),
            "starting" => format!("The {name} language server is still starting: try again in a moment."),
            "not installed" => {
                format!("{needs} a language server for {name}. Hover \"LSP\" in the status bar to see how to add one.")
            }
            _ => format!("{needs} a language server for {name}, and there is none running."),
        }
    }

    fn handle_code_actions(&mut self, id: RequestId, actions: Vec<qell_lsp::lsp_types::CodeActionOrCommand>) {
        if self.pending_actions != Some(id) {
            return;
        }
        self.pending_actions = None;
        if actions.is_empty() {
            self.notify("No quick fixes here.");
            return;
        }
        self.code_actions = actions;
        self.open_palette(PaletteMode::CodeActions);
    }

    /// Run the action the Quick Fix palette picked: apply its edit, ask
    /// the server to fill one in, or run its command.
    pub fn run_code_action(&mut self, index: usize) {
        use qell_lsp::lsp_types::CodeActionOrCommand;
        let Some(action) = self.code_actions.get(index).cloned() else { return };
        let lang = self.docs.get(self.active).and_then(|d| d.buffer.language()).unwrap_or("").to_string();
        match action {
            CodeActionOrCommand::Command(cmd) => {
                if self.lsp.execute_command(&lang, &cmd).is_none() {
                    self.notify("The language server stopped: nothing was changed.");
                }
            }
            CodeActionOrCommand::CodeAction(a) => match a.edit.clone() {
                Some(edit) => self.handle_workspace_edit(0, edit),
                None if a.command.is_some() => {
                    let cmd = a.command.clone().expect("checked");
                    if self.lsp.execute_command(&lang, &cmd).is_none() {
                        self.notify("The language server stopped: nothing was changed.");
                    }
                }
                None => match self.lsp.resolve_code_action(&lang, &a) {
                    Some(id) => self.pending_action_resolve = Some(id),
                    None => self.notify(format!("{} cannot be applied here.", a.title)),
                },
            },
        }
    }

    /// A rename's answer, or an edit the server asked us to apply. `id` is
    /// 0 for the latter, which nothing is waiting on.
    fn handle_workspace_edit(&mut self, id: RequestId, edit: qell_lsp::lsp_types::WorkspaceEdit) {
        let rename = match self.pending_rename.take() {
            Some((pending, old, new)) if pending == id => Some((old, new)),
            other => {
                self.pending_rename = other;
                None
            }
        };
        let (files, places, skipped) = self.apply_workspace_edit(edit);
        let what = |n: usize, one: &str, many: &str| if n == 1 { one.to_string() } else { format!("{n} {many}") };
        if places == 0 {
            match rename {
                Some((old, _)) => self.notify(format!("The language server found nothing to rename for {old}.")),
                None => self.notify("Nothing to change."),
            }
        } else {
            let where_ = format!("{} in {}", what(places, "one place", "places"), what(files, "this file", "files"));
            match rename {
                Some((old, new)) => self.notify(format!("Renamed {old} to {new}: {where_}. Not saved yet.")),
                None => self.notify(format!("Changed {where_}. Not saved yet.")),
            }
        }
        if skipped > 0 {
            self.notify_error(format!(
                "{} the server wanted to create, delete or rename: do that yourself.",
                what(skipped, "One file", "files")
            ));
        }
    }

    /// Apply an edit that may touch several files. Files that aren't open
    /// are opened as tabs rather than written behind your back; nothing
    /// touches the disk until you save. Returns how many files and places
    /// changed, and how many file operations were left out.
    fn apply_workspace_edit(&mut self, edit: qell_lsp::lsp_types::WorkspaceEdit) -> (usize, usize, usize) {
        use qell_lsp::lsp_types::{DocumentChangeOperation, DocumentChanges};
        let mut per_file: Vec<(PathBuf, Vec<qell_lsp::lsp_types::TextEdit>)> = Vec::new();
        let mut skipped = 0usize;
        let mut add = |uri: &qell_lsp::lsp_types::Uri, edits: Vec<qell_lsp::lsp_types::TextEdit>| {
            let Some(path) = convert::uri_to_path(uri) else { return };
            match per_file.iter_mut().find(|(p, _)| p == &path) {
                Some((_, have)) => have.extend(edits),
                None => per_file.push((path, edits)),
            }
        };
        match edit.document_changes {
            Some(DocumentChanges::Edits(edits)) => {
                for e in edits {
                    let plain = e.edits.into_iter().map(|e| match e {
                        qell_lsp::lsp_types::OneOf::Left(edit) => edit,
                        // An annotated edit carries the same change plus a label.
                        qell_lsp::lsp_types::OneOf::Right(a) => a.text_edit,
                    });
                    add(&e.text_document.uri, plain.collect());
                }
            }
            Some(DocumentChanges::Operations(ops)) => {
                for op in ops {
                    match op {
                        DocumentChangeOperation::Edit(e) => {
                            let plain = e.edits.into_iter().map(|e| match e {
                                qell_lsp::lsp_types::OneOf::Left(edit) => edit,
                                qell_lsp::lsp_types::OneOf::Right(a) => a.text_edit,
                            });
                            add(&e.text_document.uri, plain.collect());
                        }
                        DocumentChangeOperation::Op(_) => skipped += 1,
                    }
                }
            }
            None => {
                for (uri, edits) in edit.changes.unwrap_or_default() {
                    add(&uri, edits);
                }
            }
        }
        per_file.retain(|(_, e)| !e.is_empty());
        let was_active = self.docs.get(self.active).and_then(|d| d.buffer.path()).map(Path::to_path_buf);
        let (mut files, mut places) = (0, 0);
        for (path, edits) in per_file {
            let open = self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path()));
            let index = match open {
                Some(i) => i,
                None => {
                    self.open_file(path.clone());
                    match self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path())) {
                        Some(i) => i,
                        None => continue,
                    }
                }
            };
            if let Some(doc) = self.docs.get_mut(index) {
                places += edits.len();
                files += 1;
                apply_lsp_edits(doc, edits);
            }
        }
        // Stay where the user was.
        if let Some(p) = was_active {
            if let Some(i) = self.docs.iter().position(|d| d.buffer.path() == Some(p.as_path())) {
                self.active = i;
            }
        }
        self.show_editor_area();
        (files, places, skipped)
    }

    // ---- settings --------------------------------------------------------

    /// The Settings window or the palette has the keyboard.
    pub fn modal_open(&self) -> bool {
        self.command_palette.open
            || self.settings.open
            || self.scm.typing
            || self.search.typing
            || self.find.typing
            || self.tree_edit.is_some()
            || self.confirm_delete.is_some()
    }

    // ---- Vim commands that need the app ---------------------------------------

    fn vim_error(&mut self, text: &str) {
        self.vim.set_message(text, true);
    }

    fn any_unsaved(&self) -> bool {
        self.docs.iter().any(|d| d.buffer.is_dirty() && !d.read_only)
    }

    /// `:wa`: save every modified file that has a name.
    fn save_all(&mut self) {
        let active = self.active;
        for i in 0..self.docs.len() {
            if self.docs[i].buffer.is_dirty() && self.docs[i].buffer.path().is_some() && !self.docs[i].read_only {
                self.active = i;
                self.save_active(false);
            }
        }
        self.active = active.min(self.docs.len().saturating_sub(1));
    }

    fn run_vim_action(&mut self, action: crate::vim::Action, ctx: &Context) {
        use crate::vim::Action;
        const UNSAVED: &str = "E37: No write since last change (add ! to override)";
        let dirty = self.docs.get(self.active).is_some_and(|d| d.buffer.is_dirty() && !d.read_only);
        match action {
            Action::Save(None) => self.save_active(false),
            Action::Save(Some(name)) => {
                let path = if Path::new(&name).is_absolute() { PathBuf::from(&name) } else { self.root.join(&name) };
                let Some(doc) = self.docs.get_mut(self.active) else { return };
                match doc.buffer.save_as(path) {
                    Ok(()) => {
                        self.lsp.did_save(&doc.buffer);
                        self.files_changed();
                        self.vim.set_message(format!("\"{name}\" written"), false);
                    }
                    Err(e) => self.vim.set_message(format!("E212: Can't open file for writing: {e}"), true),
                }
            }
            Action::Close { force } => {
                if dirty && !force {
                    self.vim_error(UNSAVED);
                } else {
                    self.close_active();
                }
            }
            Action::SaveAndClose => {
                if dirty {
                    self.save_active(false);
                }
                // Still modified: the save was cancelled or failed.
                if !self.docs.get(self.active).is_some_and(|d| d.buffer.is_dirty() && !d.read_only) {
                    self.close_active();
                }
            }
            Action::SaveAll => self.save_all(),
            Action::QuitAll { force } => {
                if self.any_unsaved() && !force {
                    self.vim_error(UNSAVED);
                } else {
                    self.execute(Command::Quit, ctx);
                }
            }
            Action::SaveAllAndQuit => {
                self.save_all();
                if self.any_unsaved() {
                    self.vim_error("E141: Some files have no name: use :w name");
                } else {
                    self.execute(Command::Quit, ctx);
                }
            }
            Action::Edit(name) => {
                let path = if Path::new(&name).is_absolute() { PathBuf::from(&name) } else { self.root.join(&name) };
                if path.is_dir() {
                    self.vim_error(&format!("\"{name}\" is a directory"));
                } else if path.exists() {
                    self.open_file(path);
                } else {
                    // A new file: it exists once written, as in Vim.
                    self.docs.push(Document::new(qell_core::Buffer::from_text("", Some(path))));
                    self.active = self.docs.len() - 1;
                    self.vim.set_message(format!("\"{name}\" [New]"), false);
                }
            }
            Action::NextTab => self.execute(Command::NextTab, ctx),
            Action::PrevTab => self.execute(Command::PrevTab, ctx),
            Action::GoToDefinition => self.go_to_definition(),
            Action::GoBack => self.navigate(true),
            Action::Copy(text) => ctx.copy_text(text),
            Action::SetWrap(on) => {
                self.config.word_wrap = on;
                self.save_config();
            }
            Action::Terminal => self.execute(Command::FocusTerminal, ctx),
            Action::Explorer => {
                self.sidebar_view = SidebarView::Explorer;
                self.show_sidebar = true;
            }
        }
    }

    // ---- several cursors --------------------------------------------------

    /// ⌘D: select the word under the cursor, then one more occurrence of
    /// it each time. `all` takes every occurrence at once (⌘⇧L).
    fn add_occurrence(&mut self, all: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let buffer = &mut doc.buffer;
        // Nothing selected yet: start from the word each cursor sits on.
        if buffer.cursors().iter().all(|c| c.selection.is_empty()) {
            let words: Vec<Cursor> = buffer
                .cursors()
                .iter()
                .filter_map(|c| {
                    let head = c.head();
                    let (s, e) = views::editor::word_range(&buffer.line(head.line), head.col, false)?;
                    let mut c = Cursor::at(Position::new(head.line, s));
                    c.set_head(Position::new(head.line, e), true);
                    Some(c)
                })
                .collect();
            if !words.is_empty() {
                buffer.set_cursors(words);
                if !all {
                    return;
                }
            }
        }
        let (ps, pe) = buffer.cursor.selection.ordered();
        let (pstart, pend) = (buffer.pos_to_char(ps), buffer.pos_to_char(pe));
        let needle: String = buffer.rope().slice(pstart..pend).to_string();
        if needle.is_empty() || needle.contains('\n') {
            return;
        }
        // Where the cursors already are, so the same place isn't taken twice.
        let mut taken: Vec<(usize, usize)> = buffer
            .cursors()
            .iter()
            .map(|c| {
                let (s, e) = c.selection.ordered();
                (buffer.pos_to_char(s), buffer.pos_to_char(e))
            })
            .collect();
        let text = buffer.text();
        let rope = buffer.rope();
        let found: Vec<(usize, usize)> =
            text.match_indices(&needle).map(|(b, m)| (rope.byte_to_char(b), rope.byte_to_char(b + m.len()))).collect();
        let mut added: Vec<Cursor> = Vec::new();
        let take = |at: &(usize, usize), buffer: &Buffer, added: &mut Vec<Cursor>| {
            let mut c = Cursor::at(buffer.char_to_pos(at.0));
            c.set_head(buffer.char_to_pos(at.1), true);
            added.push(c);
        };
        if all {
            for at in found.iter().filter(|f| !taken.contains(f)) {
                take(at, buffer, &mut added);
            }
        } else {
            let after = taken.iter().map(|(_, e)| *e).max().unwrap_or(pend);
            let next =
                found.iter().find(|f| f.0 >= after && !taken.contains(f)).or_else(|| found.iter().find(|f| !taken.contains(f)));
            match next {
                Some(at) => take(at, buffer, &mut added),
                // Every one of them is already selected.
                None => return,
            }
        }
        if added.is_empty() {
            return;
        }
        taken.extend(added.iter().map(|c| {
            let (s, e) = c.selection.ordered();
            (buffer.pos_to_char(s), buffer.pos_to_char(e))
        }));
        let mut cursors = buffer.cursors();
        cursors.extend(added);
        cursors.sort_by_key(|c| c.selection.ordered().0);
        buffer.set_cursors(cursors);
        doc.scroll_to_cursor = true;
    }

    /// Add a cursor on the line above or below the outermost one.
    fn add_cursor_line(&mut self, delta: isize) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let buffer = &mut doc.buffer;
        let cursors = buffer.cursors();
        let edge = if delta < 0 { cursors.first() } else { cursors.last() };
        let Some(from) = edge.map(|c| c.head()) else { return };
        let line = from.line as isize + delta;
        if line < 0 || line as usize >= buffer.line_count() {
            return;
        }
        let line = line as usize;
        let col = doc.goal_col.unwrap_or(from.col);
        doc.goal_col = Some(col);
        let at = Position::new(line, col.min(buffer.line_len(line)));
        buffer.toggle_cursor(Cursor::at(at));
        doc.scroll_to_cursor = true;
    }

    // ---- formatting -----------------------------------------------------------

    /// Format the open file: the language server when it offers it, else
    /// the language's own tool. `then_save` writes the file afterwards.
    fn format_active(&mut self, then_save: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if doc.read_only {
            if !then_save {
                self.notify("This is a read-only view.");
            }
            return;
        }
        let Some(lang) = doc.buffer.language().map(str::to_string) else {
            if !then_save {
                self.notify("Qell doesn't know this file's language, so it can't format it.");
            }
            return;
        };
        let configured = crate::format::command_for(&lang, &self.config.format);
        // A command set by hand wins; otherwise the server, then the
        // language's usual tool.
        if self.config.format.contains_key(&lang) {
            if let Some(cmd) = configured {
                self.format_with_command(&cmd, then_save);
            } else if !then_save {
                self.notify(format!("Formatting is turned off for {lang} in your settings."));
            }
            return;
        }
        let doc = &mut self.docs[self.active];
        if let Some(id) = self.lsp.request_formatting(&self.languages, &mut doc.buffer, self.config.tab_size) {
            self.pending_format = Some((id, then_save));
            return;
        }
        match configured {
            Some(cmd) => self.format_with_command(&cmd, then_save),
            None if !then_save => self.notify(format!("No formatter for {lang}: set one with [format] in the settings.")),
            None => {}
        }
    }

    /// Run an outside formatter over the buffer and take in what it prints.
    fn format_with_command(&mut self, cmd: &[String], then_save: bool) {
        let root = self.root.clone();
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let text = doc.buffer.text();
        let path = doc.buffer.path().map(Path::to_path_buf).unwrap_or_else(|| root.join(doc.buffer.file_name()));
        let cwd = path.parent().filter(|p| p.is_dir()).unwrap_or(&root).to_path_buf();
        match crate::format::run(cmd, &text, &path, &cwd) {
            Ok(formatted) => {
                let count = apply_text(doc, &text, &formatted);
                if !then_save && count == 0 {
                    self.notify("Already formatted.");
                }
            }
            // On save, a tool that isn't installed stays quiet: saving must
            // not nag. Everything else is worth saying once.
            Err(crate::format::Error::NotInstalled(_)) if then_save => {}
            Err(e) => self.notify_error(e.to_string()),
        }
        if then_save {
            self.write_active();
        }
    }

    /// Edits from the language server's formatter.
    fn handle_format_edits(&mut self, id: RequestId, edits: Vec<qell_lsp::lsp_types::TextEdit>) {
        let Some((pending, then_save)) = self.pending_format else { return };
        if pending != id {
            return;
        }
        self.pending_format = None;
        if let Some(doc) = self.docs.get_mut(self.active) {
            let before = doc.buffer.text();
            apply_lsp_edits(doc, edits);
            if !then_save && doc.buffer.text() == before {
                self.notify("Already formatted.");
            }
        }
        if then_save {
            self.write_active();
        }
    }

    // ---- files changed outside Qell -----------------------------------------

    /// Notice files that changed on disk since Qell read them. Untouched
    /// ones are quietly brought up to date; where you have unsaved edits,
    /// the editor asks which version to keep.
    fn check_disk(&mut self) {
        let mut reloaded: Vec<String> = Vec::new();
        for doc in &mut self.docs {
            if doc.conflict.is_some() {
                continue;
            }
            let Some(change) = doc.disk_change() else { continue };
            if doc.buffer.is_dirty() || change == DiskChange::Deleted {
                doc.conflict = Some(change);
                continue;
            }
            let name = doc.buffer.file_name();
            match doc.reload() {
                Ok(()) => reloaded.push(name),
                // Unreadable right now (a half-written file): ask instead.
                Err(_) => doc.conflict = Some(DiskChange::Modified),
            }
        }
        match reloaded.len() {
            0 => {}
            1 => self.notify(format!("{} changed on disk: reloaded", reloaded[0])),
            n => self.notify(format!("{n} files changed on disk: reloaded")),
        }
    }

    /// Take the file as it is on disk, dropping the unsaved edits (undo
    /// brings them back).
    pub fn reload_active(&mut self) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let name = doc.buffer.file_name();
        match doc.reload() {
            Ok(()) => self.notify(format!("Reloaded {name}")),
            Err(e) => self.notify_error(format!("Cannot read {name}: {e}")),
        }
    }

    /// Keep the version in the editor; the next save overwrites the file.
    pub fn keep_mine(&mut self) {
        if let Some(doc) = self.docs.get_mut(self.active) {
            doc.note_disk();
        }
    }

    // ---- file operations (file tree menu) -----------------------------------

    /// Something changed on disk: refresh the tree, file list, git and index.
    fn files_changed(&mut self) {
        self.tree_cache.clear();
        self.command_palette.files.clear();
        self.git.refresh();
        self.indexer.refresh();
    }

    /// Start naming a new file or folder in `dir`, or renaming `path`.
    pub fn start_tree_edit(&mut self, kind: TreeEditKind) {
        let text = match &kind {
            TreeEditKind::Rename(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            _ => String::new(),
        };
        self.tree_edit = Some(TreeEdit { kind, text, fresh: true });
        self.show_sidebar = true;
        self.sidebar_view = SidebarView::Explorer;
    }

    /// Apply the typed name (Enter, or clicking away).
    pub fn commit_tree_edit(&mut self) {
        let Some(edit) = self.tree_edit.take() else { return };
        let name = edit.text.trim();
        if name.is_empty() || name == "." || name == ".." {
            return;
        }
        let result = match &edit.kind {
            TreeEditKind::Rename(from) => {
                let to = from.with_file_name(name);
                if to == *from {
                    return;
                }
                if to.exists() {
                    Err(format!("{name} already exists"))
                } else {
                    std::fs::rename(from, &to).map_err(|e| e.to_string()).map(|_| {
                        self.retarget_docs(from, &to);
                        None
                    })
                }
            }
            // "a/b.txt" makes the folders on the way, as in VS Code.
            TreeEditKind::NewFile(dir) => {
                let path = dir.join(name);
                let made = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(|_| ()));
                made.map_err(|e| format!("Can't create {name}: {e}")).map(|_| Some(path))
            }
            TreeEditKind::NewFolder(dir) => {
                let path = dir.join(name);
                if path.exists() {
                    Err(format!("{name} already exists"))
                } else {
                    std::fs::create_dir_all(&path).map_err(|e| format!("Can't create {name}: {e}")).map(|_| None)
                }
            }
        };
        match result {
            Ok(open) => {
                self.files_changed();
                if let Some(p) = open {
                    self.open_file(p);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// Open tabs on a renamed file (or inside a renamed folder) follow it.
    fn retarget_docs(&mut self, from: &Path, to: &Path) {
        for i in 0..self.docs.len() {
            let Some(old) = self.docs[i].buffer.path().map(Path::to_path_buf) else { continue };
            let Ok(rest) = old.strip_prefix(from) else { continue };
            let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            self.lsp.did_close(&self.docs[i].buffer);
            self.docs[i].buffer.set_path(new);
            self.docs[i].git_base_state = crate::document::BaseState::Unknown;
        }
    }

    /// Move a file or folder to the Trash (after confirmation).
    pub fn trash_path(&mut self, path: &Path) {
        match trash::delete(path) {
            Ok(()) => self.files_changed(),
            Err(e) => self.notify_error(format!("Couldn't move {} to the Trash: {e}", path.display())),
        }
    }

    /// Show a file or folder in Finder / Explorer / the file manager.
    pub fn reveal_in_file_manager(&mut self, path: &Path) {
        let spawned = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg("-R").arg(path).spawn()
        } else if cfg!(windows) {
            std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
        } else {
            // No portable "select this file": open the folder holding it.
            let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
            std::process::Command::new("xdg-open").arg(dir).spawn()
        };
        if let Err(e) = spawned {
            self.notify_error(format!("Couldn't open the file manager: {e}"));
        }
    }

    /// The path relative to the workspace, as shown in the tree.
    pub fn relative_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().into_owned()
    }

    /// Where the cursor is now, for navigation history.
    fn here(&self) -> Option<NavPoint> {
        let doc = self.docs.get(self.active)?;
        Some(NavPoint { path: doc.buffer.path()?.to_path_buf(), pos: doc.buffer.cursor.head() })
    }

    /// Open `path` with `line:col..end_col` selected (0-based, chars),
    /// remembering where we were for Go Back.
    pub fn open_location(&mut self, path: &Path, line: usize, col: usize, end_col: usize) {
        let from = self.here();
        self.open_file(path.to_path_buf());
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if doc.buffer.path().map(|p| std::fs::canonicalize(p).unwrap_or(p.to_path_buf()))
            != Some(std::fs::canonicalize(path).unwrap_or(path.to_path_buf()))
        {
            return; // didn't open
        }
        let start = doc.buffer.clamp(Position::new(line, col));
        let end = doc.buffer.clamp(Position::new(line, end_col.max(col)));
        doc.buffer.set_cursor(start, false);
        doc.buffer.set_cursor(end, true);
        doc.scroll_to_cursor = true;
        self.show_editor_area();
        if let Some(from) = from.filter(|f| f.path != path || f.pos.line != line) {
            self.nav_back.push(from);
            self.nav_back.truncate(100);
            self.nav_forward.clear();
        }
    }

    fn navigate(&mut self, back: bool) {
        let target = if back { self.nav_back.pop() } else { self.nav_forward.pop() };
        let Some(target) = target else { return };
        if let Some(here) = self.here() {
            if back {
                self.nav_forward.push(here)
            } else {
                self.nav_back.push(here)
            }
        }
        self.open_file(target.path.clone());
        if let Some(doc) = self.docs.get_mut(self.active) {
            let p = doc.buffer.clamp(target.pos);
            doc.buffer.set_cursor(p, false);
            doc.scroll_to_cursor = true;
        }
        self.show_editor_area();
    }

    /// Re-read config.toml after the user edited it by hand.
    fn reload_config_file(&mut self) {
        let path = config::config_path();
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| Config::parse(&s).map_err(|e| e.to_string()))
        {
            Ok(c) => {
                self.config = c;
                self.notify("Settings reloaded");
            }
            Err(e) => self.notify_error(format!("Settings not applied, {}: {e}", path.display())),
        }
    }

    /// Bring the running editor in line with `config` after it changed
    /// (Settings window or config.toml), and schedule a save.
    fn apply_config(&mut self, ctx: &Context) {
        let old = std::mem::replace(&mut self.applied_config, self.config.clone());
        let new = &self.config;
        if new.font_size != old.font_size {
            self.font_size = new.font_size;
        }
        if new.ui_scale != old.ui_scale {
            ctx.set_zoom_factor(new.ui_scale);
        }
        if new.show_sidebar != old.show_sidebar {
            self.show_sidebar = new.show_sidebar;
        }
        if new.show_terminal != old.show_terminal {
            self.show_terminal = new.show_terminal;
        }
        if new.tree_hidden != old.tree_hidden {
            self.tree_cache.clear();
        }
        if new.ignore != old.ignore {
            self.command_palette.files.clear();
            self.indexer.set_ignore(new.ignore.clone());
        }
        if new.git_scan() != old.git_scan() {
            self.git.set_options(new.git_scan());
        }
        if new.lsp != old.lsp {
            // Restart servers so the new commands are used; documents are
            // re-opened with them on the next sync.
            self.lsp.shutdown_all();
            self.lsp = new_lsp(&self.egui_ctx, &self.root, &self.config);
        }
        self.config_dirty_since = Some(Instant::now());
    }

    pub fn save_config(&mut self) {
        self.config_dirty_since = None;
        if let Err(e) = self.config.save() {
            self.notify_error(format!("Cannot save settings to {}: {e}", config::config_path().display()));
        }
    }

    fn handle_config(&mut self, ctx: &Context) {
        if self.config != self.applied_config {
            self.apply_config(ctx);
        }
        // Debounce: sliders and typing change the config many times a second.
        if self.config_dirty_since.is_some_and(|t| t.elapsed() > Duration::from_millis(400)) {
            self.save_config();
        }
    }

    // ---- git -------------------------------------------------------------

    /// Git status of a path, if it is inside the workspace and has one.
    pub fn git_status_of(&self, path: &Path) -> Option<qell_git::FileStatus> {
        if !self.config.git_status {
            return None;
        }
        let st = self.git_status.as_ref()?;
        st.status_of(path.strip_prefix(&self.root).ok()?)
    }

    /// The repository of the active document, else the workspace's own,
    /// with its workspace-relative root.
    pub fn active_repo(&self) -> Option<(&Path, &RepoStatus)> {
        let ws = self.git_status.as_ref()?;
        let doc_rel = self.docs.get(self.active).and_then(|d| d.buffer.path()).and_then(|p| p.strip_prefix(&self.root).ok());
        doc_rel.and_then(|rel| ws.repo_for(rel)).or_else(|| ws.repo_for(Path::new(""))).map(|(root, st)| (root.as_path(), st))
    }

    fn handle_git(&mut self, ctx: &Context) {
        // Regaining window focus usually means files changed elsewhere.
        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused {
            self.git.refresh();
            self.check_disk();
        }
        self.was_focused = focused;
        // Coming back to the editor after running something in the terminal.
        if self.focus == Focus::Editor && self.last_focus != Focus::Editor {
            self.check_disk();
        }
        self.last_focus = self.focus;
        if let Some(st) = self.git.poll() {
            // Files changed on disk (git noticed): bring the index up to date.
            self.indexer.refresh();
            // A checkout or a rebase rewrites the files you have open.
            self.check_disk();
            // Something changed on disk; new or removed files must show up
            // in the tree too.
            self.tree_cache.clear();
            self.git_status = st;
            // A commit, a checkout, staging…: what files are compared with
            // may have changed. Keep the marks until the new version is in.
            for d in &mut self.docs {
                d.git_base_state = crate::document::BaseState::Unknown;
            }
        }
        self.handle_git_base(ctx);
    }

    /// Fetch git's version of the active file when it isn't known (one
    /// `git show`, in a thread), and take in the ones that arrived.
    fn handle_git_base(&mut self, ctx: &Context) {
        use crate::document::BaseState;
        for (path, base) in self.git_base_rx.try_iter() {
            for d in self.docs.iter_mut().filter(|d| d.buffer.path() == Some(path.as_path())) {
                d.set_git_base(base.clone());
            }
        }
        let on = self.config.git_status;
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if !on || doc.read_only {
            doc.git_base = None;
            doc.changes.clear();
            return;
        }
        if doc.git_base_state != BaseState::Unknown {
            return;
        }
        let Some(path) = doc.buffer.path().map(Path::to_path_buf) else { return };
        doc.git_base_state = BaseState::Loading;
        let (tx, ctx) = (self.git_base_tx.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("qell-git-base".into()).spawn(move || {
            let base = qell_git::index_text(&path);
            let _ = tx.send((path, base));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            doc.git_base_state = BaseState::Loaded;
        }
    }

    // ---- notifications ---------------------------------------------------

    pub fn notify(&mut self, text: impl Into<String>) {
        self.notifications.push(Notification { text: text.into(), at: Instant::now(), is_error: false });
    }

    pub fn notify_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::error!("{text}");
        self.notifications.push(Notification { text, at: Instant::now(), is_error: true });
    }

    // ---- commands --------------------------------------------------------

    pub fn execute(&mut self, cmd: Command, ctx: &Context) {
        self.completion.open = false;
        match cmd {
            Command::NewFile => {
                self.docs.push(Document::untitled());
                self.active = self.docs.len() - 1;
                self.show_editor_area();
            }
            Command::OpenFile => {
                if let Some(p) = rfd::FileDialog::new().set_directory(&self.root).pick_file() {
                    self.open_file(p);
                }
            }
            Command::OpenFolder => {
                if let Some(p) = rfd::FileDialog::new().set_directory(&self.root).pick_folder() {
                    self.open_folder(p);
                }
            }
            Command::Save => self.save_active(false),
            Command::SaveAs => self.save_active(true),
            Command::CloseTab => self.close_active(),
            Command::NextTab => {
                if !self.docs.is_empty() {
                    self.active = (self.active + 1) % self.docs.len();
                }
            }
            Command::PrevTab => {
                if !self.docs.is_empty() {
                    self.active = (self.active + self.docs.len() - 1) % self.docs.len();
                }
            }
            Command::Undo => {
                if let Some(d) = self.active_doc() {
                    d.buffer.undo();
                    d.scroll_to_cursor = true;
                }
            }
            Command::Redo => {
                if let Some(d) = self.active_doc() {
                    d.buffer.redo();
                    d.scroll_to_cursor = true;
                }
            }
            Command::SelectAll => {
                if let Some(d) = self.active_doc() {
                    d.buffer.select_all();
                }
            }
            Command::TriggerCompletion => {
                self.completion.open = false;
                self.request_completion(None);
            }
            Command::ToggleSidebar => {
                if self.close_places() {
                    self.show_sidebar = true;
                } else {
                    self.show_sidebar = !self.show_sidebar;
                }
            }
            Command::FormatDocument => self.format_active(false),
            Command::ToggleComment => {
                if let Some(doc) = self.docs.get_mut(self.active).filter(|d| !d.read_only) {
                    doc.buffer.toggle_comment();
                    doc.scroll_to_cursor = true;
                }
            }
            Command::AddNextOccurrence => self.add_occurrence(false),
            Command::SelectAllOccurrences => self.add_occurrence(true),
            Command::AddCursorAbove => self.add_cursor_line(-1),
            Command::AddCursorBelow => self.add_cursor_line(1),
            Command::ToggleVimMode => {
                self.config.vim_mode = !self.config.vim_mode;
                self.save_config();
                self.vim = crate::vim::Vim::new();
                let on = if self.config.vim_mode { "on: Esc for Normal mode, : for commands" } else { "off" };
                self.notify(format!("Vim mode {on}"));
            }
            Command::ToggleMinimap => {
                self.config.minimap = !self.config.minimap;
                self.save_config();
            }
            Command::ToggleWordWrap => {
                self.config.word_wrap = !self.config.word_wrap;
                self.save_config();
                if let Some(d) = self.docs.get_mut(self.active) {
                    d.scroll_to_cursor = true;
                }
            }
            Command::ShowExplorer => self.show_sidebar_view(SidebarView::Explorer),
            Command::StageHunk => self.stage_hunk_at_cursor(),
            Command::RevertHunk => self.revert_hunk_at_cursor(),
            Command::ShowSearch => {
                // Always shows (and focuses) Search, seeded with the selection.
                let selection = self.docs.get(self.active).map(|d| d.buffer.selected_text());
                self.sidebar_view = SidebarView::Search;
                self.show_sidebar = true;
                self.search.start(selection);
            }
            Command::ReplaceInFiles => {
                let selection = self.docs.get(self.active).map(|d| d.buffer.selected_text());
                self.sidebar_view = SidebarView::Search;
                self.show_sidebar = true;
                self.search.start_replace(selection);
            }
            Command::FindInFile | Command::ReplaceInFile => {
                let selection = self.docs.get(self.active).map(|d| d.buffer.selected_text());
                self.find.start(selection, cmd == Command::ReplaceInFile);
                self.focus = Focus::Editor;
            }
            Command::FindNext => views::find::go(self, true),
            Command::FindPrevious => views::find::go(self, false),
            Command::GoToDefinition => self.go_to_definition(),
            Command::FindReferences => self.find_references(),
            Command::GoToSymbol => self.go_to_symbol(),
            Command::RenameSymbol => self.start_rename(),
            Command::QuickFix => self.quick_fix(),
            Command::GoBack => self.navigate(true),
            Command::GoForward => self.navigate(false),
            Command::ShowSourceControl => self.show_sidebar_view(SidebarView::SourceControl),
            Command::ToggleCluster => match (self.k8s.open, self.focus == Focus::K8s) {
                // Open but behind (you were in the terminal): bring it
                // back rather than closing what you were looking at.
                (true, false) => self.focus = Focus::K8s,
                (true, true) => views::k8s::close(self),
                (false, _) => views::k8s::open(self, ctx),
            },
            Command::ToggleAws => match (self.aws.open, self.focus == Focus::Aws) {
                (true, false) => self.focus = Focus::Aws,
                (true, true) => views::aws::close(self),
                (false, _) => views::aws::open(self, ctx),
            },
            Command::ToggleTerminal => {
                self.show_terminal = !self.show_terminal;
                // Closing it hands the keyboard back to whatever is behind.
                self.focus = if self.show_terminal { Focus::Terminal } else { self.focus_behind_terminal() };
            }
            Command::NewTerminal => {
                self.show_terminal = true;
                self.new_terminal();
                self.focus = Focus::Terminal;
            }
            Command::KillTerminal => self.kill_terminal(self.active_terminal),
            Command::NextTerminal | Command::PrevTerminal => {
                let n = self.terminals.len();
                if n > 0 {
                    let step = if cmd == Command::NextTerminal { 1 } else { n - 1 };
                    self.active_terminal = (self.active_terminal + step) % n;
                    self.show_terminal = true;
                    self.focus = Focus::Terminal;
                }
            }
            Command::FocusTerminal => {
                self.show_terminal = true;
                self.focus = Focus::Terminal;
            }
            Command::FocusEditor => self.focus = Focus::Editor,
            Command::CommandPalette => self.open_palette(PaletteMode::Commands),
            Command::QuickOpen => self.open_palette(PaletteMode::Files),
            Command::GoToLine => self.open_palette(PaletteMode::GoToLine),
            Command::ZoomIn => self.font_size = (self.font_size + 1.0).min(40.0),
            Command::ZoomOut => self.font_size = (self.font_size - 1.0).max(6.0),
            Command::OpenSettings => self.settings.toggle(&self.config),
            Command::OpenSettingsFile => {
                // Make sure the file exists (and holds the current values).
                self.save_config();
                self.settings.open = false;
                self.open_file(config::config_path());
            }
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn open_palette(&mut self, mode: PaletteMode) {
        let p = &mut self.command_palette;
        p.open = true;
        p.mode = mode;
        p.query.clear();
        p.selected = 0;
        p.needs_focus = true;
        if mode == PaletteMode::Files && p.files.is_empty() {
            p.files = views::palette::collect_files(&self.root, &self.config.ignore);
        }
    }

    pub fn run_plugin_command(&mut self, name: &str) {
        let Some(host) = self.plugins.as_mut() else { return };
        let selection = self.docs.get(self.active).map(|d| d.buffer.selected_text()).unwrap_or_default();
        match host.run(name, "", &EditorContext { selection }) {
            Ok(actions) => {
                for a in actions {
                    match a {
                        PluginAction::Notify(m) => self.notify(m),
                        PluginAction::InsertText(t) => {
                            if let Some(d) = self.docs.get_mut(self.active) {
                                d.buffer.insert(&t);
                                d.scroll_to_cursor = true;
                            }
                        }
                    }
                }
            }
            Err(e) => self.notify_error(format!("{e:#}")),
        }
    }

    // ---- LSP -------------------------------------------------------------

    /// Open or update the completion list at the cursor. `trigger` is the
    /// character that asked for it (`.`, `:`), `None` while typing a word.
    pub fn request_completion(&mut self, trigger: Option<char>) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let word = doc.buffer.word_before_cursor();
        let head = doc.buffer.cursor.head();
        let word_start = Position::new(head.line, head.col.saturating_sub(word.chars().count()));
        // Still typing the same word and the server sent everything: just
        // narrow the list we have, like VS Code. No round trip, no flicker.
        let c = &mut self.completion;
        if trigger.is_none() && c.open && !c.incomplete && c.word_start == word_start && !c.items.is_empty() {
            return;
        }
        let refresh = c.open && c.incomplete && c.word_start == word_start;
        c.word_start = word_start;
        c.request = self.lsp.request_completion(&self.languages, &mut doc.buffer, trigger, refresh);
        if c.request.is_none() {
            // No server: fall back to words from the buffer so completion
            // still does something useful in plain-text files.
            c.items = views::editor::buffer_word_completions(&doc.buffer, &word);
            c.incomplete = true; // recompute on every keystroke
            c.selected = 0;
            c.resolved.clear();
            c.open = !c.items.is_empty();
        }
    }

    /// Fetch the documentation of the selected item if the server left it
    /// out of the list.
    pub fn resolve_selected_completion(&mut self) {
        let i = self.completion.selected;
        let Some(item) = self.completion.items.get(i) else { return };
        if item.documentation.is_some() || self.completion.resolved.contains(&i) {
            return;
        }
        let Some(lang) = self.docs.get(self.active).and_then(|d| d.buffer.language()) else { return };
        self.completion.resolved.insert(i);
        if let Some(id) = self.lsp.resolve_completion(lang, item) {
            self.completion.resolving = Some((id, i));
        }
    }

    pub fn accept_completion(&mut self) {
        let Some(item) = self.completion.items.get(self.completion.selected).cloned() else { return };
        self.completion.open = false;
        let doc_idx = self.active;
        let Some(doc) = self.docs.get_mut(doc_idx) else { return };
        let b = &mut doc.buffer;

        let (start, end, raw) = views::editor::completion_edit(b, &item, self.completion.word_start);
        let indent: String = b.line(start.line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let expanded = if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
            crate::snippet::expand(&raw, &indent)
        } else {
            crate::snippet::Expanded { text: raw, stops: Vec::new() }
        };

        // The completion plus the server's extra edits (auto-imports), as
        // character ranges of the current text, applied last-first so the
        // ranges stay valid.
        let main = (b.pos_to_char(start), b.pos_to_char(end));
        let mut extra: Vec<(usize, usize, String)> = item
            .additional_text_edits
            .iter()
            .flatten()
            .map(|e| {
                let s = from_lsp_position(&b.line(e.range.start.line as usize), e.range.start);
                let en = from_lsp_position(&b.line(e.range.end.line as usize), e.range.end);
                (b.pos_to_char(s), b.pos_to_char(en), e.new_text.clone())
            })
            .filter(|(s, e, _)| *e <= main.0 || *s >= main.1) // never overlapping the main edit
            .collect();
        extra.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        let mut shift: isize = 0;
        let mut main_done = false;
        for (s, e, text) in &extra {
            if !main_done && *s < main.0 {
                b.replace_range(b.char_to_pos(main.0), b.char_to_pos(main.1), &expanded.text);
                main_done = true;
            }
            b.replace_range(b.char_to_pos(*s), b.char_to_pos(*e), text);
            if *s < main.0 {
                shift += text.chars().count() as isize - (*e - *s) as isize;
            }
        }
        if !main_done {
            b.replace_range(b.char_to_pos(main.0), b.char_to_pos(main.1), &expanded.text);
        }
        let inserted_at = (main.0 as isize + shift) as usize;
        let inserted_end = inserted_at + expanded.text.chars().count();

        // Snippet: select the first tab stop; Tab visits the next ones.
        let mut stops: Vec<(usize, usize)> =
            expanded.stops.iter().map(|s| (inserted_at + s.start, inserted_at + s.end)).collect();
        if stops.is_empty() {
            b.set_cursor(b.char_to_pos(inserted_end), false);
            self.snippet = None;
        } else {
            let (s, e) = stops.remove(0);
            b.set_cursor(b.char_to_pos(s), false);
            b.set_cursor(b.char_to_pos(e), true);
            // A lone `$0` is just where the cursor ends: no session needed.
            self.snippet = (!stops.is_empty()).then_some(SnippetSession { doc: doc_idx, stops, current: (s, e) });
        }
        doc.scroll_to_cursor = true;
    }

    /// Tab inside a snippet: move to the next tab stop. Returns `false`
    /// when there is no snippet session (Tab indents as usual).
    pub fn next_snippet_stop(&mut self) -> bool {
        let Some(session) = self.snippet.as_mut() else { return false };
        let Some(doc) = self.docs.get_mut(session.doc).filter(|_| session.doc == self.active) else {
            self.snippet = None;
            return false;
        };
        let b = &mut doc.buffer;
        // Whatever was typed into the current stop shifted the later ones.
        let cursor = b.pos_to_char(b.cursor.head());
        let (cur_start, cur_end) = session.current;
        if cursor < cur_start {
            self.snippet = None;
            return false;
        }
        crate::snippet::shift_stops(&mut session.stops, cur_end, cursor);
        let (s, e) = session.stops.remove(0);
        let len = b.rope().len_chars();
        let (s, e) = (s.min(len), e.min(len));
        b.set_cursor(b.char_to_pos(s), false);
        b.set_cursor(b.char_to_pos(e), true);
        doc.scroll_to_cursor = true;
        if session.stops.is_empty() {
            self.snippet = None;
        } else {
            session.current = (s, e);
        }
        true
    }

    fn handle_lsp_events(&mut self) {
        for ev in self.lsp.poll() {
            match ev {
                LspEvent::Ready { language } => self.notify(format!("{language} language server ready")),
                LspEvent::Completion { id, items, incomplete } => {
                    if self.completion.request == Some(id) {
                        let c = &mut self.completion;
                        c.items = items;
                        c.incomplete = incomplete;
                        c.selected = 0;
                        c.resolved.clear();
                        c.resolving = None;
                        c.open = !c.items.is_empty();
                    }
                }
                LspEvent::Locations { id, locations } => self.handle_locations(id, locations),
                LspEvent::Edits { id, edits } => self.handle_format_edits(id, edits),
                LspEvent::CompletionResolved { id, item } => {
                    if let Some((rid, i)) = self.completion.resolving {
                        if rid == id && i < self.completion.items.len() {
                            self.completion.items[i] = *item;
                            self.completion.resolving = None;
                        }
                    }
                }
                LspEvent::Symbols { id, symbols } => self.handle_symbols(id, symbols),
                LspEvent::WorkspaceEdit { id, edit } => self.handle_workspace_edit(id, *edit),
                LspEvent::CodeActions { id, actions } => self.handle_code_actions(id, actions),
                LspEvent::CodeActionResolved { id, action } => {
                    if self.pending_action_resolve == Some(id) {
                        self.pending_action_resolve = None;
                        match action.edit {
                            Some(edit) => self.handle_workspace_edit(0, edit),
                            None => self.notify(format!("{} has nothing to change here.", action.title)),
                        }
                    }
                }
                LspEvent::Hover { .. } => {}
                LspEvent::Diagnostics { uri, diagnostics } => {
                    if let Some(path) = convert::uri_to_path(&uri) {
                        self.set_diagnostics(&path, diagnostics);
                    }
                }
                LspEvent::Error { message, .. } => log::warn!("lsp: {message}"),
                LspEvent::Exited { language, reason } => {
                    self.notify_error(format!("{language} language server unavailable: {reason}"));
                }
                LspEvent::NotInstalled { language, program } => {
                    // Shown once per language and session (the registry
                    // doesn't retry), and not as an error.
                    let name = self.languages.get(&language).map(|l| l.name).unwrap_or(&language);
                    self.notify(format!("{program} isn't installed, so {name} has no completion. Hover \"LSP\" in the status bar to see how to add it."));
                }
            }
        }
        // Keep the active document in sync (didOpen / didChange).
        if let Some(doc) = self.docs.get_mut(self.active) {
            self.lsp.sync(&self.languages, &mut doc.buffer);
        }
    }

    fn set_diagnostics(&mut self, path: &Path, diagnostics: Vec<Diagnostic>) {
        if let Some(d) = self.docs.iter_mut().find(|d| d.buffer.path() == Some(path)) {
            d.diagnostics = diagnostics;
        }
    }

    // ---- terminal --------------------------------------------------------

    /// Open a new shell in the workspace root and make it active.
    pub fn new_terminal(&mut self) {
        self.new_terminal_in(self.root.clone());
    }

    /// A new shell starting in `dir` ("Open in Integrated Terminal").
    pub fn new_terminal_in(&mut self, dir: PathBuf) {
        match Terminal::new(80, 24, Some(dir), Some(self.config.shell.as_str())) {
            Ok(term) => {
                let number = self.next_terminal_number;
                self.next_terminal_number += 1;
                self.terminals.push(TerminalTab { term, number, name: None });
                self.active_terminal = self.terminals.len() - 1;
            }
            Err(e) => {
                self.notify_error(format!("Terminal unavailable: {e:#}"));
                if self.terminals.is_empty() {
                    self.show_terminal = false;
                }
            }
        }
    }

    /// Run one command in a terminal tab of its own: a shell in a pod,
    /// its logs, a port-forward. The process lives as long as the tab,
    /// so Ctrl+C or closing it is how you stop it — nothing of Qell's
    /// keeps running where you can't see it.
    pub fn run_in_terminal(&mut self, name: String, program: &str, args: Vec<String>) {
        match Terminal::command(80, 24, Some(self.root.clone()), program, args) {
            Ok(term) => {
                let number = self.next_terminal_number;
                self.next_terminal_number += 1;
                self.terminals.push(TerminalTab { term, number, name: Some(name) });
                self.active_terminal = self.terminals.len() - 1;
                self.show_terminal = true;
                self.focus = Focus::Terminal;
            }
            Err(e) => self.notify_error(format!("Cannot start {program}: {e:#}")),
        }
    }

    /// Close a shell. Closing the last one hides the panel, like VS Code.
    pub fn kill_terminal(&mut self, index: usize) {
        if index >= self.terminals.len() {
            return;
        }
        self.terminals.remove(index);
        self.after_terminal_removed(index);
    }

    /// Replace a shell with a fresh one in the same tab.
    pub fn restart_terminal(&mut self, index: usize) {
        let Some(tab) = self.terminals.get_mut(index) else { return };
        match Terminal::new(80, 24, Some(self.root.clone()), Some(self.config.shell.as_str())) {
            Ok(term) => tab.term = term,
            Err(e) => self.notify_error(format!("Terminal unavailable: {e:#}")),
        }
    }

    fn after_terminal_removed(&mut self, index: usize) {
        if self.terminals.is_empty() {
            self.show_terminal = false;
            self.focus = self.focus_behind_terminal();
            self.active_terminal = 0;
            self.next_terminal_number = 1;
        } else if self.active_terminal > index || self.active_terminal >= self.terminals.len() {
            self.active_terminal = self.active_terminal.saturating_sub(1);
        }
    }

    /// Drain every shell's events (background tabs keep running) and drop
    /// the ones whose shell exited.
    fn poll_terminals(&mut self) {
        if self.show_terminal && self.terminals.is_empty() {
            self.new_terminal();
        }
        let mut i = 0;
        while i < self.terminals.len() {
            self.terminals[i].term.poll();
            if self.terminals[i].term.has_exited() {
                self.terminals.remove(i);
                self.after_terminal_removed(i);
            } else {
                i += 1;
            }
        }
    }

    // ---- shortcuts -------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &Context) {
        // Inside the palette only Escape is global.
        if self.command_palette.open {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.command_palette.open = false;
            }
            return;
        }
        // In Settings, the commit box or the search box, text fields own the
        // keyboard: only Escape, the settings shortcut itself and Quit go through.
        if self.settings.open
            || self.scm.typing
            || self.search.typing
            || self.k8s.typing != views::k8s::Typing::No
            || self.aws.typing != views::aws::Typing::No
            || self.find.typing
            || self.tree_edit.is_some()
            || self.confirm_delete.is_some()
        {
            if self.settings.open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.settings.open = false;
            }
            // ⌘A, ⌘C, ⌘V… must edit the text being typed, not the document.
            let mut allowed = vec![Command::OpenSettings, Command::Quit];
            if self.find.typing {
                allowed.extend([Command::FindInFile, Command::ReplaceInFile, Command::FindNext, Command::FindPrevious]);
            }
            for cmd in allowed {
                if let Some(sc) = cmd.shortcut() {
                    if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                        self.pending_commands.push(cmd);
                    }
                }
            }
            return;
        }
        // The cluster view is driven by bare keys (j, k, r, 0, :), and by
        // Ctrl+D / Ctrl+U for paging: only the few view commands go past it.
        if (self.focus == Focus::K8s && self.k8s.open) || (self.focus == Focus::Aws && self.aws.open) {
            for cmd in [
                Command::ToggleCluster,
                Command::ToggleAws,
                Command::ToggleTerminal,
                Command::CommandPalette,
                Command::ToggleSidebar,
                // Asking for a file is asking for the editor back, and
                // these all bring it forward.
                Command::QuickOpen,
                Command::NewFile,
                Command::OpenFile,
                Command::OpenFolder,
                Command::ShowExplorer,
                Command::ShowSearch,
                Command::ShowSourceControl,
                Command::OpenSettings,
                Command::ZoomIn,
                Command::ZoomOut,
                Command::Quit,
            ] {
                if let Some(sc) = cmd.shortcut() {
                    if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                        self.pending_commands.push(cmd);
                    }
                }
            }
            return;
        }
        for (cmd, sc) in Command::by_shortcut_specificity() {
            let in_terminal = self.focus == Focus::Terminal && self.show_terminal;
            // Switching terminals only makes sense from the terminal.
            if matches!(cmd, Command::NextTerminal | Command::PrevTerminal) && !in_terminal {
                continue;
            }
            // In the terminal, Ctrl+<letter> belongs to the shell, except
            // the few view commands people expect to keep working.
            if in_terminal
                && !matches!(
                    cmd,
                    Command::ToggleTerminal
                        | Command::ToggleCluster
                        | Command::ToggleAws
                        | Command::NewTerminal
                        | Command::NextTerminal
                        | Command::PrevTerminal
                        | Command::CommandPalette
                        | Command::QuickOpen
                        | Command::ToggleSidebar
                        | Command::Quit
                )
            {
                continue;
            }
            // In Vim's Normal mode, Ctrl-D scrolls: leave it to Vim.
            if cmd == Command::AddNextOccurrence && self.config.vim_mode && self.focus == Focus::Editor && self.vim.captures() {
                continue;
            }
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                self.pending_commands.push(cmd);
                // On macOS ⌥<key> also types a character (⌥Z is "Ω"): the
                // shortcut used the key, so drop the text too.
                if sc.modifiers.alt {
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                }
            }
        }
    }
}

impl eframe::App for QellApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.notifications.retain(|n| n.at.elapsed() < Duration::from_secs(if n.is_error { 8 } else { 4 }));
        self.handle_native_menu(ctx);
        self.handle_lsp_events();
        self.handle_git(ctx);
        self.handle_scm_jobs();
        views::search::update(self, ctx);
        views::find::handle_keys(self, ctx);
        self.poll_terminals();
        self.handle_shortcuts(ctx);

        views::menu::show(self, ctx);
        views::status::show(self, ctx);
        views::sidebar::show_activity_bar(self, ctx);
        if self.show_sidebar {
            views::sidebar::show(self, ctx);
        }
        if self.show_terminal {
            views::terminal::show(self, ctx);
        }
        views::editor::show_central(self, ctx);
        views::palette::show(self, ctx);
        views::settings::show(self, ctx);
        views::sidebar::show_confirm_delete(self, ctx);
        self.recent.save_if_dirty();

        for action in std::mem::take(&mut self.pending_vim) {
            self.run_vim_action(action, ctx);
        }
        for cmd in std::mem::take(&mut self.pending_commands) {
            self.execute(cmd, ctx);
        }
        for name in std::mem::take(&mut self.pending_plugin_commands) {
            self.run_plugin_command(&name);
        }
        if let Some(path) = self.pending_apply.take() {
            self.apply_manifest(path, ctx);
        }
        if std::mem::take(&mut self.back_to_cluster) {
            views::k8s::open(self, ctx);
        }
        // The cluster view's workers answer even when it is hidden (an
        // apply started from a manifest tab), so they are drained here
        // rather than while drawing the panel.
        views::k8s::update(self, ctx);
        views::aws::update(self, ctx);
        self.handle_config(ctx);
        // The tabs changed: remember them (cursors are saved again on exit).
        if self.session_state().0 != self.session_key {
            self.save_session();
        }

        // Language servers and the shell produce output without user input,
        // so keep polling at a modest rate.
        ctx.request_repaint_after(Duration::from_millis(if self.show_terminal { 33 } else { 100 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.config_dirty_since.is_some() {
            self.save_config();
        }
        // Again on the way out, for the cursor positions.
        self.save_session();
        self.lsp.shutdown_all();
    }
}

/// Whether two paths name the same file (resolving symlinks and `..`).
/// Put new text into the buffer, changing only the lines that moved, as
/// one undo step. Returns how many places changed.
fn apply_text(doc: &mut Document, old: &str, new: &str) -> usize {
    let edits = crate::format::edits(old, new);
    if edits.is_empty() {
        return 0;
    }
    let head = doc.buffer.cursor.head();
    doc.buffer.begin_undo_group();
    // Last first: an earlier edit would move the lines of the later ones.
    for (start, end, text) in edits.iter().rev() {
        doc.buffer.replace_range(*start, *end, text);
    }
    doc.buffer.end_undo_group();
    let p = doc.buffer.clamp(head);
    doc.buffer.set_cursor(p, false);
    doc.scroll_to_cursor = true;
    edits.len()
}

/// Apply a language server's edits (their positions count UTF-16 units),
/// last first so the earlier ones stay valid.
fn apply_lsp_edits(doc: &mut Document, mut edits: Vec<qell_lsp::lsp_types::TextEdit>) {
    if edits.is_empty() {
        return;
    }
    edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    let head = doc.buffer.cursor.head();
    doc.buffer.begin_undo_group();
    for edit in edits.iter().rev() {
        let line = |n: u32| doc.buffer.line(n as usize);
        let start = from_lsp_position(&line(edit.range.start.line), edit.range.start);
        let end = from_lsp_position(&line(edit.range.end.line), edit.range.end);
        doc.buffer.replace_range(start, end, &edit.new_text);
    }
    doc.buffer.end_undo_group();
    let p = doc.buffer.clamp(head);
    doc.buffer.set_cursor(p, false);
    doc.scroll_to_cursor = true;
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A language-server registry for `root` that wakes the UI whenever a
/// server answers, with the user's command overrides.
fn new_lsp(ctx: &Context, root: &Path, config: &Config) -> LspRegistry {
    let mut lsp = LspRegistry::new(root.to_path_buf());
    let ctx = ctx.clone();
    lsp.set_waker(std::sync::Arc::new(move || ctx.request_repaint()));
    for (lang, cmd) in &config.lsp {
        lsp.set_override(lang.clone(), cmd.clone());
    }
    lsp
}
