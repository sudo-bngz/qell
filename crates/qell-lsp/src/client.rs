//! A single language-server connection over stdio.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Sender};
use lsp_types::*;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

pub type RequestId = i64;

/// Called after each message from a server, so the UI can redraw at once
/// instead of on its next timer tick.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// What a client tells the rest of the editor.
#[derive(Debug)]
pub enum Event {
    /// The server finished initializing and is ready for documents.
    Ready {
        language: String,
    },
    Completion {
        id: RequestId,
        items: Vec<CompletionItem>,
        /// The server didn't send everything: typing more must ask again
        /// instead of filtering this list.
        incomplete: bool,
    },
    /// A completion item with its lazily computed fields (documentation,
    /// detail, additional edits) filled in.
    CompletionResolved {
        id: RequestId,
        item: Box<CompletionItem>,
    },
    /// Answer to a definition or references request (empty: none found).
    Locations {
        id: RequestId,
        locations: Vec<Location>,
    },
    /// Answer to a formatting request: the edits to apply.
    Edits {
        id: RequestId,
        edits: Vec<TextEdit>,
    },
    Hover {
        id: RequestId,
        hover: Option<Hover>,
    },
    Diagnostics {
        uri: Uri,
        diagnostics: Vec<Diagnostic>,
    },
    /// A request failed on the server side.
    Error {
        id: Option<RequestId>,
        message: String,
    },
    /// The process exited (or we could not start it).
    Exited {
        language: String,
        reason: String,
    },
    /// The server program isn't installed (not found on `PATH`). Not an
    /// error: the editor works without it, with fewer features.
    NotInstalled {
        language: String,
        program: String,
    },
}

/// Which kind of response a pending request id expects.
#[derive(Debug, Clone, Copy)]
enum Pending {
    Completion,
    Resolve,
    Locations,
    Formatting,
    Hover,
    Other,
}

pub struct Client {
    language: String,
    child: Child,
    outgoing: Sender<Value>,
    next_id: AtomicI64,
    pending: Arc<Mutex<HashMap<RequestId, Pending>>>,
    pub capabilities: Arc<Mutex<Option<ServerCapabilities>>>,
    /// Set once `initialized` has been sent.
    ready: Arc<Mutex<bool>>,
}

impl Client {
    /// Spawn `command` and start the reader/writer threads. Events go to
    /// `events`. `initialize` is sent immediately; `Event::Ready` arrives
    /// when the server answers.
    pub fn start(language: &str, command: &[&str], root: &Path, events: Sender<Event>, waker: Option<Waker>) -> Result<Self> {
        let (program, args) = command.split_first().ok_or_else(|| anyhow!("empty LSP command"))?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting language server `{program}` for {language}"))?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        // Keep the start of stderr: when a server dies before answering,
        // it usually says why there (and a rustup proxy says the component
        // isn't installed). The rest is drained so the pipe never blocks.
        let stderr_head = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let head = stderr_head.clone();
            thread::Builder::new().name(format!("lsp-stderr-{language}")).spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(std::result::Result::ok) {
                    let mut h = head.lock().unwrap();
                    if h.len() < 4096 {
                        h.push_str(&line);
                        h.push('\n');
                    }
                }
            })?;
        }

        let (outgoing, outgoing_rx) = unbounded::<Value>();
        let pending: Arc<Mutex<HashMap<RequestId, Pending>>> = Arc::default();
        let capabilities: Arc<Mutex<Option<ServerCapabilities>>> = Arc::default();
        let ready = Arc::new(Mutex::new(false));

        // Writer thread: frames every outgoing message.
        {
            let lang = language.to_string();
            thread::Builder::new().name(format!("lsp-writer-{lang}")).spawn(move || {
                let mut stdin = stdin;
                for msg in outgoing_rx {
                    let body = msg.to_string();
                    if write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).and_then(|_| stdin.flush()).is_err() {
                        log::warn!("[{lang}] writer: pipe closed");
                        break;
                    }
                }
            })?;
        }

        // Reader thread: parses frames and dispatches.
        {
            let lang = language.to_string();
            let pending = pending.clone();
            let capabilities = capabilities.clone();
            let ready = ready.clone();
            let outgoing = outgoing.clone();
            let program = program.to_string();
            thread::Builder::new().name(format!("lsp-reader-{lang}")).spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    match read_message(&mut reader) {
                        Ok(msg) => dispatch(&lang, msg, &pending, &capabilities, &ready, &outgoing, &events),
                        Err(e) => {
                            // Give the stderr thread a moment to read the last words.
                            thread::sleep(std::time::Duration::from_millis(50));
                            let said = stderr_head.lock().unwrap().clone();
                            let never_started = !*ready.lock().unwrap();
                            let event = if never_started && is_missing_component(&said) {
                                Event::NotInstalled { language: lang.clone(), program: program.clone() }
                            } else {
                                let why = said.lines().find(|l| !l.trim().is_empty()).map(str::trim).unwrap_or("");
                                let reason = if why.is_empty() { e.to_string() } else { format!("{e}: {why}") };
                                Event::Exited { language: lang.clone(), reason }
                            };
                            let _ = events.send(event);
                            if let Some(w) = &waker {
                                w();
                            }
                            break;
                        }
                    }
                    if let Some(w) = &waker {
                        w();
                    }
                }
            })?;
        }

        let client =
            Self { language: language.to_string(), child, outgoing, next_id: AtomicI64::new(1), pending, capabilities, ready };
        client.initialize(root)?;
        Ok(client)
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn is_ready(&self) -> bool {
        *self.ready.lock().unwrap()
    }

    fn initialize(&self, root: &Path) -> Result<()> {
        let root_uri = crate::convert::path_to_uri(root).ok_or_else(|| anyhow!("root is not an absolute path"))?;
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.clone()),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            }]),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    completion: Some(CompletionClientCapabilities {
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(true),
                            documentation_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                            label_details_support: Some(true),
                            // Servers may leave these out of the list and send
                            // them on `completionItem/resolve`, which keeps the
                            // list fast.
                            resolve_support: Some(CompletionItemCapabilityResolveSupport {
                                properties: vec!["documentation".into(), "detail".into(), "additionalTextEdits".into()],
                            }),
                            insert_replace_support: Some(false),
                            ..Default::default()
                        }),
                        context_support: Some(true),
                        ..Default::default()
                    }),
                    definition: Some(GotoCapability { dynamic_registration: None, link_support: Some(true) }),
                    references: Some(DynamicRegistrationClientCapabilities { dynamic_registration: None }),
                    hover: Some(HoverClientCapabilities {
                        content_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities::default()),
                    synchronization: Some(TextDocumentSyncClientCapabilities { did_save: Some(true), ..Default::default() }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            client_info: Some(ClientInfo { name: "qell".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
            ..Default::default()
        };
        self.request("initialize", params, Pending::Other);
        Ok(())
    }

    fn request<P: serde::Serialize>(&self, method: &str, params: P, kind: Pending) -> RequestId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap().insert(id, kind);
        let _ = self.outgoing.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    fn notify<P: serde::Serialize>(&self, method: &str, params: P) {
        let _ = self.outgoing.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    // ---- document sync ---------------------------------------------------

    pub fn did_open(&self, uri: Uri, language_id: &str, version: i32, text: String) {
        self.notify(
            "textDocument/didOpen",
            DidOpenTextDocumentParams { text_document: TextDocumentItem { uri, language_id: language_id.into(), version, text } },
        );
    }

    pub fn did_change(&self, uri: Uri, version: i32, changes: Vec<TextDocumentContentChangeEvent>) {
        self.notify(
            "textDocument/didChange",
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier { uri, version },
                content_changes: changes,
            },
        );
    }

    pub fn did_save(&self, uri: Uri) {
        self.notify(
            "textDocument/didSave",
            DidSaveTextDocumentParams { text_document: TextDocumentIdentifier { uri }, text: None },
        );
    }

    pub fn did_close(&self, uri: Uri) {
        self.notify("textDocument/didClose", DidCloseTextDocumentParams { text_document: TextDocumentIdentifier { uri } });
    }

    // ---- requests --------------------------------------------------------

    /// Ask for completions at `position`. `trigger` is the character that
    /// opened the list (`.`, `:`), `None` when typing or invoked by hand.
    pub fn completion(&self, uri: Uri, position: Position, trigger: Option<char>, refresh_incomplete: bool) -> RequestId {
        let context = match trigger {
            Some(c) => CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(c.to_string()),
            },
            None if refresh_incomplete => CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS,
                trigger_character: None,
            },
            None => CompletionContext { trigger_kind: CompletionTriggerKind::INVOKED, trigger_character: None },
        };
        self.request(
            "textDocument/completion",
            CompletionParams {
                text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri }, position },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: Some(context),
            },
            Pending::Completion,
        )
    }

    /// Whether the server fills in items lazily (`completionItem/resolve`).
    pub fn resolves_completions(&self) -> bool {
        self.capabilities
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.completion_provider.as_ref())
            .and_then(|p| p.resolve_provider)
            .unwrap_or(false)
    }

    /// Characters that should open the completion list, per the server.
    pub fn completion_triggers(&self) -> Vec<String> {
        self.capabilities
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.completion_provider.as_ref())
            .and_then(|p| p.trigger_characters.clone())
            .unwrap_or_default()
    }

    /// Whether the server answers go-to-definition / find-references.
    pub fn supports_definition(&self) -> bool {
        let caps = self.capabilities.lock().unwrap();
        caps.as_ref().and_then(|c| c.definition_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    pub fn supports_references(&self) -> bool {
        let caps = self.capabilities.lock().unwrap();
        caps.as_ref().and_then(|c| c.references_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    /// Whether the server can format a whole file.
    pub fn supports_formatting(&self) -> bool {
        let caps = self.capabilities.lock().unwrap();
        caps.as_ref().and_then(|c| c.document_formatting_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    pub fn formatting(&self, uri: Uri, tab_size: u32, insert_spaces: bool) -> RequestId {
        self.request(
            "textDocument/formatting",
            DocumentFormattingParams {
                text_document: TextDocumentIdentifier { uri },
                options: FormattingOptions {
                    tab_size,
                    insert_spaces,
                    trim_trailing_whitespace: Some(true),
                    insert_final_newline: Some(true),
                    ..Default::default()
                },
                work_done_progress_params: Default::default(),
            },
            Pending::Formatting,
        )
    }

    pub fn definition(&self, uri: Uri, position: Position) -> RequestId {
        self.request(
            "textDocument/definition",
            GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
            Pending::Locations,
        )
    }

    pub fn references(&self, uri: Uri, position: Position) -> RequestId {
        self.request(
            "textDocument/references",
            ReferenceParams {
                text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri }, position },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: ReferenceContext { include_declaration: true },
            },
            Pending::Locations,
        )
    }

    pub fn resolve_completion(&self, item: &CompletionItem) -> RequestId {
        self.request("completionItem/resolve", item, Pending::Resolve)
    }

    pub fn hover(&self, uri: Uri, position: Position) -> RequestId {
        self.request(
            "textDocument/hover",
            HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: Default::default(),
            },
            Pending::Hover,
        )
    }

    pub fn shutdown(&mut self) {
        self.request("shutdown", Value::Null, Pending::Other);
        self.notify("exit", Value::Null);
        let _ = self.child.wait();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

// ---- wire format -----------------------------------------------------------

fn read_message<R: BufRead>(reader: &mut R) -> Result<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(anyhow!("language server closed its stdout"));
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_length = Some(v.trim().parse()?);
        }
    }
    let len = content_length.ok_or_else(|| anyhow!("missing Content-Length header"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&buf)?)
}

/// What a rustup proxy prints when the component behind it isn't
/// installed (`rust-analyzer` without `rustup component add`).
fn is_missing_component(stderr: &str) -> bool {
    stderr.contains("Unknown binary") || stderr.contains("is not installed") || stderr.contains("not applicable")
}

fn parse<T: DeserializeOwned>(v: Value) -> Option<T> {
    serde_json::from_value(v).ok()
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    lang: &str,
    msg: Value,
    pending: &Mutex<HashMap<RequestId, Pending>>,
    capabilities: &Mutex<Option<ServerCapabilities>>,
    ready: &Mutex<bool>,
    outgoing: &Sender<Value>,
    events: &Sender<Event>,
) {
    // Server → client request (we answer the few we must, with null).
    if let (Some(id), Some(method)) = (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
        let result = match method {
            "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" => Value::Null,
            "workspace/configuration" => {
                let n = msg["params"]["items"].as_array().map(Vec::len).unwrap_or(1);
                Value::Array(vec![Value::Null; n])
            }
            _ => Value::Null,
        };
        let _ = outgoing.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        return;
    }

    // Notification.
    if let Some(method) = msg.get("method").and_then(Value::as_str) {
        if method == "textDocument/publishDiagnostics" {
            if let Some(p) = parse::<PublishDiagnosticsParams>(msg["params"].clone()) {
                let _ = events.send(Event::Diagnostics { uri: p.uri, diagnostics: p.diagnostics });
            }
        } else if method == "window/logMessage" || method == "window/showMessage" {
            log::debug!("[{lang}] {}", msg["params"]["message"].as_str().unwrap_or(""));
        }
        return;
    }

    // Response.
    let Some(id) = msg.get("id").and_then(Value::as_i64) else { return };
    let kind = pending.lock().unwrap().remove(&id);
    if let Some(err) = msg.get("error") {
        let _ = events.send(Event::Error { id: Some(id), message: err["message"].as_str().unwrap_or("unknown error").into() });
        return;
    }
    let result = msg.get("result").cloned().unwrap_or(Value::Null);
    match kind {
        Some(Pending::Completion) => {
            let (items, incomplete) = match parse::<CompletionResponse>(result) {
                Some(CompletionResponse::Array(items)) => (items, false),
                Some(CompletionResponse::List(list)) => (list.items, list.is_incomplete),
                None => (Vec::new(), false),
            };
            let _ = events.send(Event::Completion { id, items, incomplete });
        }
        Some(Pending::Locations) => {
            // Location | Location[] | LocationLink[] | null
            let locations = match parse::<GotoDefinitionResponse>(result) {
                Some(GotoDefinitionResponse::Scalar(l)) => vec![l],
                Some(GotoDefinitionResponse::Array(ls)) => ls,
                Some(GotoDefinitionResponse::Link(links)) => {
                    links.into_iter().map(|l| Location { uri: l.target_uri, range: l.target_selection_range }).collect()
                }
                None => Vec::new(),
            };
            let _ = events.send(Event::Locations { id, locations });
        }
        Some(Pending::Formatting) => {
            let edits = parse::<Vec<TextEdit>>(result).unwrap_or_default();
            let _ = events.send(Event::Edits { id, edits });
        }
        Some(Pending::Resolve) => {
            if let Some(item) = parse::<CompletionItem>(result) {
                let _ = events.send(Event::CompletionResolved { id, item: Box::new(item) });
            }
        }
        Some(Pending::Hover) => {
            let _ = events.send(Event::Hover { id, hover: parse(result) });
        }
        Some(Pending::Other) => {
            // The only "other" request we care about is initialize (id 1).
            if id == 1 {
                if let Some(init) = parse::<InitializeResult>(result) {
                    *capabilities.lock().unwrap() = Some(init.capabilities);
                }
                let _ = outgoing.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                *ready.lock().unwrap() = true;
                let _ = events.send(Event::Ready { language: lang.to_string() });
            }
        }
        None => log::debug!("[{lang}] response to unknown request {id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_message() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let frame = format!("Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{}", body.len(), body);
        let mut r = std::io::Cursor::new(frame.into_bytes());
        let v = read_message(&mut r).unwrap();
        assert_eq!(v["id"], 1);
    }
}
