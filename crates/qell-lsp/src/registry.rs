//! Lazily-started language servers: one per language *and* project.
//!
//! A folder of projects (`~/go/src`, a monorepo) gets one server per
//! project you actually open a file in, each rooted where its own
//! `go.mod`, `Cargo.toml` or repository is — see [`crate::roots`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crossbeam_channel::{unbounded, Receiver, Sender};
use lsp_types::{TextDocumentContentChangeEvent, Uri};
use qell_core::buffer::Change;
use qell_core::{Buffer, LanguageRegistry};

use crate::client::{Client, Event, RequestId, Waker};
use crate::convert::{path_to_uri, to_lsp_position};

/// A server is one per language and project root, so two Go modules in
/// the same folder get a gopls each, rooted where their module is.
type Key = (String, PathBuf);

pub struct Registry {
    root: PathBuf,
    /// User-configured server commands, overriding the built-in ones.
    overrides: HashMap<String, Vec<String>>,
    clients: HashMap<Key, Client>,
    /// Servers that failed to start; we don't retry every keystroke.
    failed: HashSet<Key>,
    /// Servers whose program isn't installed, with that program.
    missing: HashMap<Key, String>,
    /// The project each language was last used in, for the calls that
    /// carry only a language id.
    current: HashMap<String, PathBuf>,
    /// Wakes the UI when a server sends something.
    waker: Option<Waker>,
    /// Documents already sent with `didOpen`, keyed by uri.
    open: HashSet<Uri>,
    events_tx: Sender<Event>,
    events_rx: Receiver<Event>,
}

impl Registry {
    pub fn new(root: PathBuf) -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            root,
            overrides: HashMap::new(),
            clients: HashMap::new(),
            failed: HashSet::new(),
            missing: HashMap::new(),
            current: HashMap::new(),
            waker: None,
            open: HashSet::new(),
            events_tx,
            events_rx,
        }
    }

    /// Replace the command used for a language (from the user's config).
    /// Called after every message from any server (typically
    /// `egui::Context::request_repaint`), so responses show up immediately.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn set_override(&mut self, lang_id: impl Into<String>, command: Vec<String>) {
        self.overrides.insert(lang_id.into(), command);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// For the memory report: events not drained yet, and per running
    /// server (language, project) the messages waiting to be sent to it.
    pub fn queues(&self) -> (usize, Vec<(String, PathBuf, usize)>) {
        let servers = self.clients.iter().map(|((lang, root), c)| (lang.clone(), root.clone(), c.queued())).collect();
        (self.events_rx.len(), servers)
    }

    /// Drain pending events. Call once per frame.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out: Vec<Event> = self.events_rx.try_iter().collect();
        for ev in &out {
            match ev {
                Event::Exited { language, reason } => {
                    log::warn!("language server for {language} exited: {reason}");
                    // The event doesn't say which project it was, so the
                    // one in front is the one that gives up.
                    let key = self.key_of(language);
                    self.clients.remove(&key);
                    self.failed.insert(key);
                }
                // A proxy (rustup) started but the real server isn't there.
                Event::NotInstalled { language, program } => {
                    let key = self.key_of(language);
                    self.clients.remove(&key);
                    self.failed.insert(key.clone());
                    self.missing.insert(key, program.clone());
                }
                _ => {}
            }
        }
        out.retain(|e| !matches!(e, Event::Error { id: None, .. }));
        out
    }

    /// Where the server for this file belongs: its own project, not
    /// necessarily the folder you opened.
    pub fn project_root(&self, lang_id: &str, file: Option<&Path>) -> PathBuf {
        crate::roots::project_root(file, &self.root, lang_id)
    }

    /// The key of the project a language was last used in.
    fn key_of(&self, lang_id: &str) -> Key {
        let root = self.current.get(lang_id).cloned().unwrap_or_else(|| self.root.clone());
        (lang_id.to_string(), root)
    }

    /// The server of the project last worked in, if it is running.
    fn client(&self, lang_id: &str) -> Option<&Client> {
        self.clients.get(&self.key_of(lang_id))
    }

    /// The server of the project this file belongs to. Saving or closing
    /// a file of another project must reach that project's server, not
    /// whichever one the cursor happens to be in.
    fn client_at(&self, lang_id: &str, file: Option<&Path>) -> Option<&Client> {
        self.clients.get(&(lang_id.to_string(), self.project_root(lang_id, file)))
    }

    /// The client for a language, starting it if needed and possible.
    /// `file` decides which project it is rooted at.
    pub fn client_for(&mut self, languages: &LanguageRegistry, lang_id: &str, file: Option<&Path>) -> Option<&Client> {
        let root = self.project_root(lang_id, file);
        self.current.insert(lang_id.to_string(), root.clone());
        let key: Key = (lang_id.to_string(), root.clone());
        if !self.clients.contains_key(&key) && !self.failed.contains(&key) {
            let owned: Vec<String> = match self.overrides.get(lang_id) {
                Some(c) if !c.is_empty() => c.clone(),
                _ => languages.get(lang_id)?.lsp?.iter().map(|s| s.to_string()).collect(),
            };
            let cmd: Vec<&str> = owned.iter().map(String::as_str).collect();
            log::debug!("starting {} for {lang_id} in {}", cmd.first().copied().unwrap_or_default(), root.display());
            match Client::start(lang_id, &cmd, &root, self.events_tx.clone(), self.waker.clone()) {
                Ok(c) => {
                    self.clients.insert(key.clone(), c);
                }
                Err(e) if is_not_found(&e) => {
                    let program = cmd.first().copied().unwrap_or_default().to_string();
                    log::debug!("{program} not installed: no language server for {lang_id}");
                    self.failed.insert(key.clone());
                    self.missing.insert(key, program.clone());
                    let _ = self.events_tx.send(Event::NotInstalled { language: lang_id.to_string(), program });
                    return None;
                }
                Err(e) => {
                    log::warn!("{e:#}");
                    self.failed.insert(key);
                    let _ = self.events_tx.send(Event::Exited { language: lang_id.to_string(), reason: format!("{e:#}") });
                    return None;
                }
            }
        }
        self.clients.get(&key)
    }

    /// Keep the server's copy of `buffer` in sync: sends `didOpen` the first
    /// time and `didChange` with the buffer's pending changes after that.
    /// Call this every frame for the active buffer; it is cheap when there
    /// is nothing to send.
    pub fn sync(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer) {
        let Some(lang) = buffer.language().map(str::to_string) else { return };
        let Some(uri) = buffer.path().and_then(path_to_uri) else { return };
        let changes = buffer.take_changes();

        // Make sure the server for this file's project exists.
        let path = buffer.path().map(Path::to_path_buf);
        if self.client_for(languages, &lang, path.as_deref()).is_none() {
            return;
        }
        let already_open = self.open.contains(&uri);
        let Some(client) = self.client(&lang) else { return };
        if !client.is_ready() {
            // Dropping the changes is fine: didOpen sends the full text.
            return;
        }

        if !already_open {
            client.did_open(uri.clone(), &lang, buffer.version(), buffer.text());
            self.open.insert(uri);
        } else if !changes.is_empty() {
            let events = changes.iter().map(to_change_event).collect();
            client.did_change(uri, buffer.version(), events);
        }
    }

    pub fn did_save(&mut self, buffer: &Buffer) {
        if let (Some(lang), Some(uri)) = (buffer.language(), buffer.path().and_then(path_to_uri)) {
            if let Some(c) = self.client_at(lang, buffer.path()) {
                c.did_save(uri);
            }
        }
    }

    pub fn did_close(&mut self, buffer: &Buffer) {
        if let (Some(lang), Some(uri)) = (buffer.language(), buffer.path().and_then(path_to_uri)) {
            if let Some(c) = self.client_at(lang, buffer.path()) {
                c.did_close(uri.clone());
            }
            self.open.remove(&uri);
        }
    }

    /// Ask for completions at the cursor. The buffer's pending edits are
    /// sent first: the request must describe the text the user sees, not
    /// the text as of the previous frame.
    pub fn request_completion(
        &mut self,
        languages: &LanguageRegistry,
        buffer: &mut Buffer,
        trigger: Option<char>,
        refresh_incomplete: bool,
    ) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client(lang).filter(|c| c.is_ready())?;
        let p = buffer.cursor.head();
        Some(client.completion(uri, to_lsp_position(&buffer.line(p.line), p), trigger, refresh_incomplete))
    }

    /// Fetch the documentation / edits a server left out of a completion
    /// item. `None` when the server doesn't do that.
    pub fn resolve_completion(&self, lang_id: &str, item: &lsp_types::CompletionItem) -> Option<RequestId> {
        let client = self.client(lang_id).filter(|c| c.is_ready() && c.resolves_completions())?;
        Some(client.resolve_completion(item))
    }

    /// Ask the server to format the whole file. `None` when no ready
    /// server for the language can do it.
    pub fn request_formatting(
        &mut self,
        languages: &LanguageRegistry,
        buffer: &mut Buffer,
        tab_size: usize,
    ) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client(lang).filter(|c| c.is_ready())?;
        client.supports_formatting().then(|| client.formatting(uri, tab_size as u32, true))
    }

    /// Ask where the symbol at the cursor is defined. `None` when no ready
    /// server for the language can answer.
    pub fn request_definition(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer) -> Option<RequestId> {
        self.request_locations(languages, buffer, false)
    }

    /// Ask for every reference to the symbol at the cursor.
    pub fn request_references(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer) -> Option<RequestId> {
        self.request_locations(languages, buffer, true)
    }

    fn request_locations(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer, references: bool) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client(lang).filter(|c| c.is_ready())?;
        let p = buffer.cursor.head();
        let pos = to_lsp_position(&buffer.line(p.line), p);
        if references {
            client.supports_references().then(|| client.references(uri, pos))
        } else {
            client.supports_definition().then(|| client.definition(uri, pos))
        }
    }

    /// Ask the server to rename the symbol at the cursor. `None` when no
    /// ready server for the language can do it.
    pub fn request_rename(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer, new_name: &str) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client(lang).filter(|c| c.is_ready())?;
        let p = buffer.cursor.head();
        let pos = to_lsp_position(&buffer.line(p.line), p);
        client.supports_rename().then(|| client.rename(uri, pos, new_name.to_string()))
    }

    /// Whether a ready server can rename symbols for this buffer.
    pub fn can_rename(&mut self, languages: &LanguageRegistry, buffer: &Buffer) -> bool {
        let Some(lang) = buffer.language().map(str::to_string) else { return false };
        let path = buffer.path().map(Path::to_path_buf);
        self.client_for(languages, &lang, path.as_deref()).is_some_and(|c| c.is_ready() && c.supports_rename())
    }

    /// Ask what the server can do over the cursor's line or selection.
    /// `None` when no ready server for the language offers code actions.
    pub fn request_code_actions(
        &mut self,
        languages: &LanguageRegistry,
        buffer: &mut Buffer,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client(lang).filter(|c| c.is_ready())?;
        let (s, e) = buffer.cursor.selection.ordered();
        let range =
            lsp_types::Range { start: to_lsp_position(&buffer.line(s.line), s), end: to_lsp_position(&buffer.line(e.line), e) };
        client.supports_code_actions().then(|| client.code_actions(uri, range, diagnostics))
    }

    /// Ask what the file defines. `None` when no ready server for the
    /// language can say.
    pub fn request_symbols(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client(lang).filter(|c| c.is_ready())?;
        client.supports_document_symbols().then(|| client.document_symbols(uri))
    }

    /// Fill in the edit of an action the server sent without one.
    pub fn resolve_code_action(&self, lang_id: &str, action: &lsp_types::CodeAction) -> Option<RequestId> {
        let client = self.client(lang_id).filter(|c| c.is_ready())?;
        Some(client.resolve_code_action(action))
    }

    /// Run a command an action asked for.
    pub fn execute_command(&self, lang_id: &str, command: &lsp_types::Command) -> Option<RequestId> {
        let client = self.client(lang_id).filter(|c| c.is_ready())?;
        Some(client.execute_command(command))
    }

    /// Characters that open the completion list for a language.
    pub fn completion_triggers(&self, lang_id: &str) -> Vec<String> {
        self.client(lang_id).map(|c| c.completion_triggers()).unwrap_or_default()
    }

    /// Ask for the signature of the call the cursor is in. `None` when no
    /// ready server offers it.
    pub fn request_signature_help(
        &mut self,
        languages: &LanguageRegistry,
        buffer: &mut Buffer,
        retrigger: bool,
    ) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client_at(lang, buffer.path()).filter(|c| c.is_ready() && c.supports_signature_help())?;
        let p = buffer.cursor.head();
        Some(client.signature_help(uri, to_lsp_position(&buffer.line(p.line), p), retrigger))
    }

    /// Characters that open the signature help in this language (`(` and
    /// `,` when the server doesn't say).
    pub fn signature_triggers(&self, lang_id: &str) -> Vec<String> {
        let t = self.client(lang_id).map(|c| c.signature_triggers()).unwrap_or_default();
        if t.is_empty() {
            vec!["(".into(), ",".into()]
        } else {
            t
        }
    }

    /// Ask for the inlay hints of the whole file.
    pub fn request_inlay_hints(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer) -> Option<RequestId> {
        self.sync(languages, buffer);
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client_at(lang, buffer.path()).filter(|c| c.is_ready() && c.supports_inlay_hints())?;
        let last = buffer.line_count().saturating_sub(1);
        let end = to_lsp_position(&buffer.line(last), qell_core::Position::new(last, buffer.line_len(last)));
        let range = lsp_types::Range { start: lsp_types::Position::new(0, 0), end };
        Some(client.inlay_hints(uri, range))
    }

    pub fn request_hover(&mut self, buffer: &Buffer) -> Option<RequestId> {
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.client_at(lang, buffer.path()).filter(|c| c.is_ready())?;
        let p = buffer.cursor.head();
        Some(client.hover(uri, to_lsp_position(&buffer.line(p.line), p)))
    }

    pub fn status(&self, lang_id: &str) -> &'static str {
        let key = self.key_of(lang_id);
        match self.clients.get(&key) {
            Some(c) if c.is_ready() => "ready",
            Some(_) => "starting",
            None if self.missing.contains_key(&key) => "not installed",
            None if self.failed.contains(&key) => "unavailable",
            None => "off",
        }
    }

    /// The server program of a language, when it isn't installed.
    pub fn missing_program(&self, lang_id: &str) -> Option<&str> {
        self.missing.get(&self.key_of(lang_id)).map(String::as_str)
    }

    /// Where the server for this language is rooted, for the status bar.
    pub fn server_root(&self, lang_id: &str) -> Option<&Path> {
        self.current.get(lang_id).map(PathBuf::as_path)
    }

    pub fn shutdown_all(&mut self) {
        for (_, mut c) in self.clients.drain() {
            c.shutdown();
        }
    }
}

/// A `Change` was recorded *after* the edit, so the buffer's current line
/// text may not match the pre-edit line. For the start position the line is
/// still valid in practice (edits don't change text before their start on
/// the same line), and we approximate the end the same way. Since the
/// editor only emits single-cursor edits this is exact for ASCII and
/// correct for BMP text, which is what matters day to day.
/// An LSP change event. Columns come from the change itself, measured
/// when it happened: converting with the buffer's current lines would be
/// wrong as soon as another change in the batch moved them.
fn to_change_event(c: &Change) -> TextDocumentContentChangeEvent {
    let pos = |line: usize, col: usize| lsp_types::Position { line: line as u32, character: col as u32 };
    TextDocumentContentChangeEvent {
        range: Some(lsp_types::Range { start: pos(c.start.line, c.start_utf16), end: pos(c.end.line, c.end_utf16) }),
        range_length: None,
        text: c.text.clone(),
    }
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound))
}

/// How to install a language server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallHint {
    /// A command the user can paste in a terminal.
    pub command: &'static str,
    /// Anything else they need to know.
    pub note: Option<&'static str>,
}

/// How to install a language server, for the user to run themselves. Qell
/// never installs anything on its own.
pub fn install_hint(program: &str) -> Option<InstallHint> {
    let mac = cfg!(target_os = "macos");
    let hint = |command, note| Some(InstallHint { command, note });
    match program {
        "rust-analyzer" => hint("rustup component add rust-analyzer rust-src", None),
        "gopls" => hint("go install golang.org/x/tools/gopls@latest", Some("Make sure $(go env GOPATH)/bin is on your PATH.")),
        "typescript-language-server" => hint("npm install -g typescript-language-server typescript", None),
        "terraform-ls" if mac => hint("brew install terraform-ls", None),
        "terraform-ls" => hint(
            "brew install terraform-ls",
            Some("Without Homebrew, download it from https://releases.hashicorp.com/terraform-ls."),
        ),
        "clangd" if mac => hint("brew install llvm", Some("Then put $(brew --prefix llvm)/bin on your PATH.")),
        "clangd" => hint("sudo apt install clangd", Some("Or your distribution's clangd package.")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_reported_as_not_installed() {
        let langs = qell_core::LanguageRegistry::new();
        let mut reg = Registry::new(std::env::temp_dir());
        reg.set_override("go", vec!["qell-no-such-server-xyz".into()]);
        assert!(reg.client_for(&langs, "go", None).is_none());
        assert_eq!(reg.status("go"), "not installed");
        assert_eq!(reg.missing_program("go"), Some("qell-no-such-server-xyz"));
        let events = reg.poll();
        assert!(events.iter().any(|e| matches!(e, Event::NotInstalled { language, .. } if language == "go")));
        assert!(!events.iter().any(|e| matches!(e, Event::Exited { .. })));
    }

    #[test]
    #[cfg(unix)]
    fn rustup_proxy_without_component_is_not_installed() {
        // A stand-in for rustup's proxy: prints its error and exits.
        let dir = tempfile_dir();
        let fake = dir.join("fake-ra");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"error: Unknown binary 'rust-analyzer' in official toolchain 'stable'\" >&2\nexit 1\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let langs = qell_core::LanguageRegistry::new();
        let mut reg = Registry::new(dir.clone());
        reg.set_override("rust", vec![fake.to_string_lossy().into_owned()]);
        let _ = reg.client_for(&langs, "rust", None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while reg.status("rust") != "not installed" {
            let _ = reg.poll();
            assert!(std::time::Instant::now() < deadline, "status {}", reg.status("rust"));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("qell-lsp-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_builtin_server_has_an_install_hint() {
        for lang in qell_core::syntax::builtin_languages() {
            if let Some(cmd) = lang.lsp {
                assert!(install_hint(cmd[0]).is_some(), "no install hint for {}", cmd[0]);
            }
        }
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use qell_core::Position;

    /// Apply LSP change events to a plain string, as a server would.
    fn apply_lsp(text: &str, ev: &TextDocumentContentChangeEvent) -> String {
        let r = ev.range.unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        // Byte offset of an LSP position (line, UTF-16 column).
        let off = |p: lsp_types::Position| -> usize {
            let mut o = 0;
            for l in &lines[..p.line as usize] {
                o += l.len() + 1;
            }
            let mut units = 0;
            for (i, ch) in lines[p.line as usize].char_indices() {
                if units >= p.character as usize {
                    return o + i;
                }
                units += ch.len_utf16();
            }
            o + lines[p.line as usize].len()
        };
        let (s, e) = (off(r.start), off(r.end));
        format!("{}{}{}", &text[..s], ev.text, &text[e..])
    }

    #[test]
    fn several_edits_in_one_batch_replay_to_the_same_text() {
        let initial = "// Entry point\n\nfunction main() {\n  const s = formatD\n}\n";
        let mut b = Buffer::from_text(initial, None);
        b.take_changes();
        // Accepting a completion with an auto-import: the word, then a line
        // inserted above it.
        b.replace_range(Position::new(3, 12), Position::new(3, 19), "formatDate");
        b.replace_range(Position::new(1, 0), Position::new(1, 0), "\nimport { formatDate } from \"./utils\"\n");
        b.set_cursor(Position::new(5, 22), false);
        b.insert("(");
        b.insert("x");
        let mut server = initial.to_string();
        for c in b.take_changes() {
            server = apply_lsp(&server, &to_change_event(&c));
        }
        assert_eq!(server, b.text());
    }

    #[test]
    fn non_ascii_columns_survive_line_shifts() {
        let initial = "let é = \"😀\"; x\nend\n";
        let mut b = Buffer::from_text(initial, None);
        b.take_changes();
        // Edit after the emoji (UTF-16 column != char column), then insert
        // lines above it in the same batch.
        b.replace_range(Position::new(0, 13), Position::new(0, 14), "y");
        b.replace_range(Position::new(0, 0), Position::new(0, 0), "// ä\n// ö\n");
        let mut server = initial.to_string();
        for c in b.take_changes() {
            server = apply_lsp(&server, &to_change_event(&c));
        }
        assert_eq!(server, b.text());
    }
}
