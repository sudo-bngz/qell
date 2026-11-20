//! Application state and the top-level frame layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, Key, Modifiers};
use qell_core::{LanguageRegistry, Position};
use qell_git::{RepoStatus, Watcher as GitWatcher, WorkspaceStatus};
use qell_lsp::convert::from_lsp_position;
use qell_lsp::lsp_types::{CompletionItem, Diagnostic, InsertTextFormat};
use qell_lsp::{convert, Event as LspEvent, Registry as LspRegistry, RequestId};
use qell_plugins::{Action as PluginAction, EditorContext, Host as PluginHost};
use qell_term::Terminal;

use crate::commands::Command;
use crate::config::{self, Config};
use crate::document::Document;
use crate::theme::Palette;
use crate::views;

/// What the left sidebar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarView {
    Explorer,
    SourceControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Terminal,
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
    GoToLine,
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
}

impl TerminalTab {
    /// Tab label: the title the shell set, else the shell's name.
    pub fn label(&self) -> String {
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
        let mut app = Self {
            applied_config: config.clone(),
            config_dirty_since: None,
            settings: Default::default(),
            sidebar_view: SidebarView::Explorer,
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
        match file {
            Some(f) => app.open_file(f),
            None => app.docs.push(Document::untitled()),
        }
        app
    }

    // ---- documents -------------------------------------------------------

    pub fn active_doc(&mut self) -> Option<&mut Document> {
        self.docs.get_mut(self.active)
    }

    pub fn open_file(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if let Some(i) = self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path())) {
            self.active = i;
            self.focus = Focus::Editor;
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
                self.focus = Focus::Editor;
            }
            Err(e) => self.notify_error(format!("Cannot open {}: {e}", path.display())),
        }
    }

    pub fn open_folder(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.lsp.shutdown_all();
        self.lsp = new_lsp(&self.egui_ctx, &path, &self.config);
        self.git.set_root(path.clone());
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
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if doc.buffer.path().is_none() || force_dialog {
            let Some(path) = rfd::FileDialog::new().set_directory(&self.root).save_file() else { return };
            if let Err(e) = doc.buffer.save_as(path) {
                self.notify_error(format!("Save failed: {e}"));
                return;
            }
        } else if let Err(e) = doc.buffer.save() {
            self.notify_error(format!("Save failed: {e}"));
            return;
        }
        self.lsp.did_save(&doc.buffer);
        self.git.refresh();
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
        self.lsp.did_close(&doc.buffer);
        if self.docs.is_empty() {
            self.docs.push(Document::untitled());
        }
        self.active = self.active.min(self.docs.len() - 1);
        self.completion.open = false;
    }

    /// Show a sidebar view; asking for the one already shown hides the
    /// sidebar, like clicking its icon in VS Code.
    pub fn show_sidebar_view(&mut self, view: SidebarView) {
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

    // ---- settings --------------------------------------------------------

    /// The Settings window or the palette has the keyboard.
    pub fn modal_open(&self) -> bool {
        self.command_palette.open || self.settings.open || self.scm.typing
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

    fn save_config(&mut self) {
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
        }
        self.was_focused = focused;
        if let Some(st) = self.git.poll() {
            // Something changed on disk; new or removed files must show up
            // in the tree too.
            self.tree_cache.clear();
            self.git_status = st;
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
                self.focus = Focus::Editor;
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
            Command::ToggleSidebar => self.show_sidebar = !self.show_sidebar,
            Command::ShowExplorer => self.show_sidebar_view(SidebarView::Explorer),
            Command::ShowSourceControl => self.show_sidebar_view(SidebarView::SourceControl),
            Command::ToggleTerminal => {
                self.show_terminal = !self.show_terminal;
                self.focus = if self.show_terminal { Focus::Terminal } else { Focus::Editor };
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
                LspEvent::CompletionResolved { id, item } => {
                    if let Some((rid, i)) = self.completion.resolving {
                        if rid == id && i < self.completion.items.len() {
                            self.completion.items[i] = *item;
                            self.completion.resolving = None;
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
        match Terminal::new(80, 24, Some(self.root.clone()), Some(self.config.shell.as_str())) {
            Ok(term) => {
                let number = self.next_terminal_number;
                self.next_terminal_number += 1;
                self.terminals.push(TerminalTab { term, number });
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
            self.focus = Focus::Editor;
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
        // In Settings or the commit box, text fields own the keyboard: only
        // Escape, the settings shortcut itself and Quit go through.
        if self.settings.open || self.scm.typing {
            if self.settings.open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.settings.open = false;
            }
            for cmd in [Command::OpenSettings, Command::Quit] {
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
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                self.pending_commands.push(cmd);
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

        for cmd in std::mem::take(&mut self.pending_commands) {
            self.execute(cmd, ctx);
        }
        for name in std::mem::take(&mut self.pending_plugin_commands) {
            self.run_plugin_command(&name);
        }
        self.handle_config(ctx);

        // Language servers and the shell produce output without user input,
        // so keep polling at a modest rate.
        ctx.request_repaint_after(Duration::from_millis(if self.show_terminal { 33 } else { 100 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.config_dirty_since.is_some() {
            self.save_config();
        }
        self.lsp.shutdown_all();
    }
}

/// Whether two paths name the same file (resolving symlinks and `..`).
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
