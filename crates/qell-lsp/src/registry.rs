//! Lazily-started language servers, one per language id.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crossbeam_channel::{unbounded, Receiver, Sender};
use lsp_types::{TextDocumentContentChangeEvent, Uri};
use qell_core::buffer::Change;
use qell_core::{Buffer, LanguageRegistry};

use crate::client::{Client, Event, RequestId};
use crate::convert::{path_to_uri, to_lsp_position};

pub struct Registry {
    root: PathBuf,
    /// User-configured server commands, overriding the built-in ones.
    overrides: HashMap<String, Vec<String>>,
    clients: HashMap<String, Client>,
    /// Languages whose server failed to start; we don't retry every keystroke.
    failed: HashSet<String>,
    /// Languages whose server program isn't installed, with that program.
    missing: HashMap<String, String>,
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
            open: HashSet::new(),
            events_tx,
            events_rx,
        }
    }

    /// Replace the command used for a language (from the user's config).
    pub fn set_override(&mut self, lang_id: impl Into<String>, command: Vec<String>) {
        self.overrides.insert(lang_id.into(), command);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Drain pending events. Call once per frame.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out: Vec<Event> = self.events_rx.try_iter().collect();
        for ev in &out {
            if let Event::Exited { language, reason } = ev {
                log::warn!("language server for {language} exited: {reason}");
                self.clients.remove(language);
                self.failed.insert(language.clone());
            }
        }
        out.retain(|e| !matches!(e, Event::Error { id: None, .. }));
        out
    }

    /// The client for a language, starting it if needed and possible.
    pub fn client_for(&mut self, languages: &LanguageRegistry, lang_id: &str) -> Option<&Client> {
        if !self.clients.contains_key(lang_id) && !self.failed.contains(lang_id) {
            let owned: Vec<String> = match self.overrides.get(lang_id) {
                Some(c) if !c.is_empty() => c.clone(),
                _ => languages.get(lang_id)?.lsp?.iter().map(|s| s.to_string()).collect(),
            };
            let cmd: Vec<&str> = owned.iter().map(String::as_str).collect();
            match Client::start(lang_id, &cmd, &self.root, self.events_tx.clone()) {
                Ok(c) => {
                    self.clients.insert(lang_id.to_string(), c);
                }
                Err(e) if is_not_found(&e) => {
                    let program = cmd.first().copied().unwrap_or_default().to_string();
                    log::debug!("{program} not installed: no language server for {lang_id}");
                    self.failed.insert(lang_id.to_string());
                    self.missing.insert(lang_id.to_string(), program.clone());
                    let _ = self.events_tx.send(Event::NotInstalled { language: lang_id.to_string(), program });
                    return None;
                }
                Err(e) => {
                    log::warn!("{e:#}");
                    self.failed.insert(lang_id.to_string());
                    let _ = self.events_tx.send(Event::Exited { language: lang_id.to_string(), reason: format!("{e:#}") });
                    return None;
                }
            }
        }
        self.clients.get(lang_id)
    }

    /// Keep the server's copy of `buffer` in sync: sends `didOpen` the first
    /// time and `didChange` with the buffer's pending changes after that.
    /// Call this every frame for the active buffer; it is cheap when there
    /// is nothing to send.
    pub fn sync(&mut self, languages: &LanguageRegistry, buffer: &mut Buffer) {
        let Some(lang) = buffer.language().map(str::to_string) else { return };
        let Some(uri) = buffer.path().and_then(path_to_uri) else { return };
        let changes = buffer.take_changes();

        // Make sure the server exists (this may start it).
        if self.client_for(languages, &lang).is_none() {
            return;
        }
        let already_open = self.open.contains(&uri);
        let Some(client) = self.clients.get(&lang) else { return };
        if !client.is_ready() {
            // Dropping the changes is fine: didOpen sends the full text.
            return;
        }

        if !already_open {
            client.did_open(uri.clone(), &lang, buffer.version(), buffer.text());
            self.open.insert(uri);
        } else if !changes.is_empty() {
            let events = changes.iter().map(|c| to_change_event(buffer, c)).collect();
            client.did_change(uri, buffer.version(), events);
        }
    }

    pub fn did_save(&mut self, buffer: &Buffer) {
        if let (Some(lang), Some(uri)) = (buffer.language(), buffer.path().and_then(path_to_uri)) {
            if let Some(c) = self.clients.get(lang) {
                c.did_save(uri);
            }
        }
    }

    pub fn did_close(&mut self, buffer: &Buffer) {
        if let (Some(lang), Some(uri)) = (buffer.language(), buffer.path().and_then(path_to_uri)) {
            if let Some(c) = self.clients.get(lang) {
                c.did_close(uri.clone());
            }
            self.open.remove(&uri);
        }
    }

    pub fn request_completion(&mut self, buffer: &Buffer) -> Option<RequestId> {
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.clients.get(lang).filter(|c| c.is_ready())?;
        let p = buffer.cursor.head();
        Some(client.completion(uri, to_lsp_position(&buffer.line(p.line), p)))
    }

    pub fn request_hover(&mut self, buffer: &Buffer) -> Option<RequestId> {
        let lang = buffer.language()?;
        let uri = buffer.path().and_then(path_to_uri)?;
        let client = self.clients.get(lang).filter(|c| c.is_ready())?;
        let p = buffer.cursor.head();
        Some(client.hover(uri, to_lsp_position(&buffer.line(p.line), p)))
    }

    pub fn status(&self, lang_id: &str) -> &'static str {
        match self.clients.get(lang_id) {
            Some(c) if c.is_ready() => "ready",
            Some(_) => "starting",
            None if self.missing.contains_key(lang_id) => "not installed",
            None if self.failed.contains(lang_id) => "unavailable",
            None => "off",
        }
    }

    /// The server program of a language, when it isn't installed.
    pub fn missing_program(&self, lang_id: &str) -> Option<&str> {
        self.missing.get(lang_id).map(String::as_str)
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
fn to_change_event(buffer: &Buffer, c: &Change) -> TextDocumentContentChangeEvent {
    let start_line = buffer.line(c.start.line.min(buffer.line_count().saturating_sub(1)));
    let end_line = buffer.line(c.end.line.min(buffer.line_count().saturating_sub(1)));
    TextDocumentContentChangeEvent {
        range: Some(lsp_types::Range { start: to_lsp_position(&start_line, c.start), end: to_lsp_position(&end_line, c.end) }),
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
        "rust-analyzer" => hint("rustup component add rust-analyzer", None),
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
        assert!(reg.client_for(&langs, "go").is_none());
        assert_eq!(reg.status("go"), "not installed");
        assert_eq!(reg.missing_program("go"), Some("qell-no-such-server-xyz"));
        let events = reg.poll();
        assert!(events.iter().any(|e| matches!(e, Event::NotInstalled { language, .. } if language == "go")));
        assert!(!events.iter().any(|e| matches!(e, Event::Exited { .. })));
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
