//! Application state and the top-level frame layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Context, Key, Modifiers};
use qell_core::{LanguageRegistry, Position};
use qell_git::{RepoStatus, Watcher as GitWatcher};
use qell_lsp::lsp_types::{CompletionItem, Diagnostic};
use qell_lsp::{convert, Event as LspEvent, Registry as LspRegistry, RequestId};
use qell_plugins::{Action as PluginAction, EditorContext, Host as PluginHost};
use qell_term::Terminal;

use crate::commands::Command;
use crate::config::{self, Config};
use crate::document::Document;
use crate::theme::Palette;
use crate::views;

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
    pub command_palette: CommandPalette,
    pub notifications: Vec<Notification>,
    pub show_sidebar: bool,
    pub show_terminal: bool,
    pub font_size: f32,
    /// Cached directory listings for the file tree.
    pub tree_cache: HashMap<PathBuf, Vec<views::sidebar::Entry>>,
    pub pending_commands: Vec<Command>,
    pub pending_plugin_commands: Vec<String>,
    /// Background `git status`; `git_status` is `None` outside a repository.
    pub git: GitWatcher,
    pub git_status: Option<RepoStatus>,
    was_focused: bool,
}

impl QellApp {
    pub fn new(cc: &eframe::CreationContext<'_>, root: PathBuf, file: Option<PathBuf>) -> Self {
        let config = Config::load();
        let palette = Palette::dark();
        palette.apply(&cc.egui_ctx);
        crate::icons::install(&cc.egui_ctx);

        let mut plugins = match PluginHost::new() {
            Ok(h) => Some(h),
            Err(e) => {
                log::error!("plugin host unavailable: {e:#}");
                None
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

        let mut lsp = LspRegistry::new(root.clone());
        for (lang, cmd) in &config.lsp {
            lsp.set_override(lang.clone(), cmd.clone());
        }

        let git = GitWatcher::new(root.clone());
        let mut app = Self {
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
        self.lsp = LspRegistry::new(path.clone());
        for (lang, cmd) in &self.config.lsp {
            self.lsp.set_override(lang.clone(), cmd.clone());
        }
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
        self.notify(format!("Saved {name}"));
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

    // ---- git -------------------------------------------------------------

    /// Git status of a path, if it is inside the workspace and has one.
    pub fn git_status_of(&self, path: &Path) -> Option<qell_git::FileStatus> {
        let st = self.git_status.as_ref()?;
        st.status_of(path.strip_prefix(&self.root).ok()?)
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
            Command::TriggerCompletion => self.request_completion(),
            Command::ToggleSidebar => self.show_sidebar = !self.show_sidebar,
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

    pub fn request_completion(&mut self) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let word = doc.buffer.word_before_cursor();
        let head = doc.buffer.cursor.head();
        self.completion.word_start = Position::new(head.line, head.col.saturating_sub(word.chars().count()));
        self.completion.request = self.lsp.request_completion(&doc.buffer);
        if self.completion.request.is_none() {
            // No server: fall back to words from the buffer so completion
            // still does something useful in plain-text files.
            let items = views::editor::buffer_word_completions(&doc.buffer, &word);
            self.completion.items = items;
            self.completion.selected = 0;
            self.completion.open = !self.completion.items.is_empty();
        }
    }

    pub fn accept_completion(&mut self) {
        let Some(item) = self.completion.items.get(self.completion.selected).cloned() else { return };
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let (start, end, text) = views::editor::completion_edit(&doc.buffer, &item, self.completion.word_start);
        doc.buffer.replace_range(start, end, &text);
        doc.scroll_to_cursor = true;
        self.completion.open = false;
    }

    fn handle_lsp_events(&mut self) {
        for ev in self.lsp.poll() {
            match ev {
                LspEvent::Ready { language } => self.notify(format!("{language} language server ready")),
                LspEvent::Completion { id, items } => {
                    if self.completion.request == Some(id) {
                        self.completion.items = items;
                        self.completion.selected = 0;
                        self.completion.open = !self.completion.items.is_empty();
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
        match Terminal::new(80, 24, Some(self.root.clone())) {
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
        match Terminal::new(80, 24, Some(self.root.clone())) {
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
        self.handle_lsp_events();
        self.handle_git(ctx);
        self.poll_terminals();
        self.handle_shortcuts(ctx);

        views::menu::show(self, ctx);
        views::status::show(self, ctx);
        if self.show_sidebar {
            views::sidebar::show(self, ctx);
        }
        if self.show_terminal {
            views::terminal::show(self, ctx);
        }
        views::editor::show_central(self, ctx);
        views::palette::show(self, ctx);

        for cmd in std::mem::take(&mut self.pending_commands) {
            self.execute(cmd, ctx);
        }
        for name in std::mem::take(&mut self.pending_plugin_commands) {
            self.run_plugin_command(&name);
        }

        // Language servers and the shell produce output without user input,
        // so keep polling at a modest rate.
        ctx.request_repaint_after(Duration::from_millis(if self.show_terminal { 33 } else { 100 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.lsp.shutdown_all();
    }
}
