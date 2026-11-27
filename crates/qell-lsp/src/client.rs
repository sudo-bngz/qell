//! A single language-server connection over stdio.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
// A reader thread that panics poisons the locks it held: the editor goes
// on with the data as it was rather than panicking in turn.
use std::sync::{Arc, Mutex, PoisonError};
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
    /// Answer to a rename request, or an edit the server asks us to apply
    /// (`workspace/applyEdit`), which may touch several files.
    WorkspaceEdit {
        id: RequestId,
        edit: Box<lsp_types::WorkspaceEdit>,
    },
    /// Answer to a document-symbol request: what the file defines, as
    /// (name, kind, line, start column, end column), outermost first.
    Symbols {
        id: RequestId,
        symbols: Vec<FileSymbol>,
    },
    /// Answer to a code-action request: what the server can do here.
    CodeActions {
        id: RequestId,
        actions: Vec<CodeActionOrCommand>,
    },
    /// A code action with its edit filled in (`codeAction/resolve`).
    CodeActionResolved {
        id: RequestId,
        action: Box<CodeAction>,
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
    /// Answer to `textDocument/signatureHelp`: the call the cursor is in.
    SignatureHelp {
        id: RequestId,
        help: Option<SignatureHelp>,
    },
    /// Answer to `textDocument/inlayHint`.
    InlayHints {
        id: RequestId,
        hints: Vec<InlayHint>,
    },
    /// The server's hints changed (it finished loading the project):
    /// ask for them again.
    RefreshInlayHints,
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
    SignatureHelp,
    InlayHints,
    Rename,
    Symbols,
    CodeActions,
    ResolveAction,
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
                    let mut h = head.lock().unwrap_or_else(PoisonError::into_inner);
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
                            let said = stderr_head.lock().unwrap_or_else(PoisonError::into_inner).clone();
                            let never_started = !*ready.lock().unwrap_or_else(PoisonError::into_inner);
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
        *self.ready.lock().unwrap_or_else(PoisonError::into_inner)
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
                workspace: Some(WorkspaceClientCapabilities {
                    apply_edit: Some(true),
                    workspace_edit: Some(WorkspaceEditClientCapabilities { document_changes: Some(true), ..Default::default() }),
                    ..Default::default()
                }),
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
                    rename: Some(RenameClientCapabilities { prepare_support: Some(false), ..Default::default() }),
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    code_action: Some(CodeActionClientCapabilities {
                        // Ask for whole code actions, not bare commands.
                        code_action_literal_support: Some(CodeActionLiteralSupport {
                            code_action_kind: CodeActionKindLiteralSupport {
                                value_set: [
                                    CodeActionKind::QUICKFIX,
                                    CodeActionKind::REFACTOR,
                                    CodeActionKind::REFACTOR_EXTRACT,
                                    CodeActionKind::REFACTOR_INLINE,
                                    CodeActionKind::REFACTOR_REWRITE,
                                    CodeActionKind::SOURCE,
                                ]
                                .iter()
                                .map(|k| k.as_str().to_string())
                                .collect(),
                            },
                        }),
                        // Servers may leave the edit out and fill it in on
                        // `codeAction/resolve` when one is picked.
                        resolve_support: Some(CodeActionCapabilityResolveSupport { properties: vec!["edit".into()] }),
                        data_support: Some(true),
                        ..Default::default()
                    }),
                    references: Some(DynamicRegistrationClientCapabilities { dynamic_registration: None }),
                    hover: Some(HoverClientCapabilities {
                        content_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    signature_help: Some(SignatureHelpClientCapabilities {
                        signature_information: Some(SignatureInformationSettings {
                            documentation_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                            parameter_information: Some(ParameterInformationSettings { label_offset_support: Some(true) }),
                            active_parameter_support: Some(true),
                        }),
                        context_support: Some(true),
                        ..Default::default()
                    }),
                    inlay_hint: Some(InlayHintClientCapabilities { dynamic_registration: None, resolve_support: None }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities::default()),
                    synchronization: Some(TextDocumentSyncClientCapabilities { did_save: Some(true), ..Default::default() }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            client_info: Some(ClientInfo { name: "qell".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
            initialization_options: initialization_options(&self.language),
            ..Default::default()
        };
        self.request("initialize", params, Pending::Other);
        Ok(())
    }

    fn request<P: serde::Serialize>(&self, method: &str, params: P, kind: Pending) -> RequestId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).insert(id, kind);
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
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|c| c.completion_provider.as_ref())
            .and_then(|p| p.resolve_provider)
            .unwrap_or(false)
    }

    /// Characters that should open the completion list, per the server.
    pub fn completion_triggers(&self) -> Vec<String> {
        self.capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|c| c.completion_provider.as_ref())
            .and_then(|p| p.trigger_characters.clone())
            .unwrap_or_default()
    }

    /// Whether the server answers go-to-definition / find-references.
    pub fn supports_definition(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        caps.as_ref().and_then(|c| c.definition_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    pub fn supports_references(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        caps.as_ref().and_then(|c| c.references_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    /// Whether the server can format a whole file.
    pub fn supports_formatting(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
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

    /// Whether the server can rename a symbol across the workspace.
    pub fn supports_rename(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        caps.as_ref().and_then(|c| c.rename_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    /// Whether the server offers quick fixes and refactorings.
    pub fn supports_code_actions(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        caps.as_ref()
            .and_then(|c| c.code_action_provider.as_ref())
            .is_some_and(|p| !matches!(p, CodeActionProviderCapability::Simple(false)))
    }

    /// Rename the symbol at `position` everywhere it is used.
    pub fn rename(&self, uri: Uri, position: Position, new_name: String) -> RequestId {
        self.request(
            "textDocument/rename",
            RenameParams {
                text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri }, position },
                new_name,
                work_done_progress_params: Default::default(),
            },
            Pending::Rename,
        )
    }

    /// Whether the server lists what a file defines.
    pub fn supports_document_symbols(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        caps.as_ref().and_then(|c| c.document_symbol_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    /// Everything the file defines.
    pub fn document_symbols(&self, uri: Uri) -> RequestId {
        self.request(
            "textDocument/documentSymbol",
            DocumentSymbolParams {
                text_document: TextDocumentIdentifier { uri },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
            Pending::Symbols,
        )
    }

    /// What the server can do over `range`, given the diagnostics there.
    pub fn code_actions(&self, uri: Uri, range: Range, diagnostics: Vec<Diagnostic>) -> RequestId {
        self.request(
            "textDocument/codeAction",
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri },
                range,
                context: CodeActionContext { diagnostics, only: None, trigger_kind: Some(CodeActionTriggerKind::INVOKED) },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
            Pending::CodeActions,
        )
    }

    /// Fill in the edit of an action the server sent without one.
    pub fn resolve_code_action(&self, action: &CodeAction) -> RequestId {
        self.request("codeAction/resolve", action, Pending::ResolveAction)
    }

    /// Run a command an action asked for; its edits come back as
    /// `workspace/applyEdit`.
    pub fn execute_command(&self, command: &lsp_types::Command) -> RequestId {
        self.request(
            "workspace/executeCommand",
            ExecuteCommandParams {
                command: command.command.clone(),
                arguments: command.arguments.clone().unwrap_or_default(),
                work_done_progress_params: Default::default(),
            },
            Pending::Other,
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

    /// Whether the server shows the parameters of the call being typed.
    pub fn supports_signature_help(&self) -> bool {
        self.capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|c| c.signature_help_provider.is_some())
    }

    /// Characters that open (and keep up to date) the signature help.
    pub fn signature_triggers(&self) -> Vec<String> {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(p) = caps.as_ref().and_then(|c| c.signature_help_provider.as_ref()) else { return Vec::new() };
        let mut out = p.trigger_characters.clone().unwrap_or_default();
        out.extend(p.retrigger_characters.clone().unwrap_or_default());
        out
    }

    pub fn signature_help(&self, uri: Uri, position: Position, retrigger: bool) -> RequestId {
        self.request(
            "textDocument/signatureHelp",
            SignatureHelpParams {
                context: Some(SignatureHelpContext {
                    trigger_kind: SignatureHelpTriggerKind::INVOKED,
                    trigger_character: None,
                    is_retrigger: retrigger,
                    active_signature_help: None,
                }),
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position,
                },
                work_done_progress_params: Default::default(),
            },
            Pending::SignatureHelp,
        )
    }

    /// Whether the server gives inlay hints (types, parameter names).
    pub fn supports_inlay_hints(&self) -> bool {
        let caps = self.capabilities.lock().unwrap_or_else(PoisonError::into_inner);
        caps.as_ref().and_then(|c| c.inlay_hint_provider.as_ref()).is_some_and(|p| !matches!(p, OneOf::Left(false)))
    }

    pub fn inlay_hints(&self, uri: Uri, range: Range) -> RequestId {
        self.request(
            "textDocument/inlayHint",
            InlayHintParams {
                text_document: TextDocumentIdentifier { uri },
                range,
                work_done_progress_params: Default::default(),
            },
            Pending::InlayHints,
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

/// One thing a file defines, as the Go to Symbol list shows it.
#[derive(Debug, Clone)]
pub struct FileSymbol {
    pub name: String,
    /// "function", "struct", "variable"…
    pub kind: String,
    /// What it sits in ("impl Buffer", a class), empty at the top level.
    pub container: String,
    /// Where the name is, in UTF-16 units.
    pub range: Range,
}

/// A symbol tree, depth first, each child naming its parent.
fn flatten_symbols(tree: &[DocumentSymbol], container: &str, out: &mut Vec<FileSymbol>) {
    for s in tree {
        out.push(FileSymbol {
            name: s.name.clone(),
            kind: kind_name(s.kind).to_string(),
            container: container.to_string(),
            range: s.selection_range,
        });
        if let Some(children) = &s.children {
            flatten_symbols(children, &s.name, out);
        }
    }
}

fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE | SymbolKind::NAMESPACE | SymbolKind::PACKAGE => "module",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY | SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM | SymbolKind::ENUM_MEMBER => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRUCT => "struct",
        SymbolKind::TYPE_PARAMETER => "type",
        _ => "symbol",
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

/// Settings a server asks for by section (`workspace/configuration`):
/// turn on the inlay hints of the servers that keep them off by default.
fn section_settings(section: &str) -> Value {
    match section {
        "gopls" => json!({ "hints": {
            "assignVariableTypes": true,
            "compositeLiteralFields": true,
            "constantValues": true,
            "functionTypeParameters": true,
            "parameterNames": true,
            "rangeVariableTypes": true
        } }),
        _ => Value::Null,
    }
}

/// Options sent with `initialize`, by language: inlay hints for the
/// TypeScript server (off unless asked for) and gopls.
fn initialization_options(language: &str) -> Option<Value> {
    match language {
        "typescript" | "javascript" | "tsx" | "jsx" | "typescriptreact" | "javascriptreact" => Some(json!({ "preferences": {
            "includeInlayParameterNameHints": "all",
            "includeInlayFunctionParameterTypeHints": true,
            "includeInlayVariableTypeHints": true,
            "includeInlayPropertyDeclarationTypeHints": true,
            "includeInlayFunctionLikeReturnTypeHints": true,
            "includeInlayEnumMemberValueHints": true
        } })),
        "go" => Some(section_settings("gopls")),
        _ => None,
    }
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
        if method == "workspace/applyEdit" {
            // The server asks us to change files (after a command, say).
            // Say yes, then hand the edit to the editor.
            let _ = outgoing.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "applied": true } }));
            if let Some(p) = parse::<ApplyWorkspaceEditParams>(msg["params"].clone()) {
                let _ = events.send(Event::WorkspaceEdit { id: 0, edit: Box::new(p.edit) });
            }
            return;
        }
        if method == "workspace/inlayHint/refresh" {
            let _ = events.send(Event::RefreshInlayHints);
        }
        let result = match method {
            "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" => Value::Null,
            "workspace/configuration" => {
                // Settings asked for by section; only inlay hints are given
                // (gopls reads them this way), the rest keeps its defaults.
                let items = msg["params"]["items"].as_array().cloned().unwrap_or_default();
                let n = items.len().max(1);
                Value::Array(
                    (0..n)
                        .map(|i| items.get(i).and_then(|it| it["section"].as_str()).map_or(Value::Null, section_settings))
                        .collect(),
                )
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
    let kind = pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
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
        Some(Pending::Rename) => {
            let edit = parse::<WorkspaceEdit>(result).unwrap_or_default();
            let _ = events.send(Event::WorkspaceEdit { id, edit: Box::new(edit) });
        }
        Some(Pending::Symbols) => {
            let symbols = match parse::<DocumentSymbolResponse>(result) {
                Some(DocumentSymbolResponse::Nested(tree)) => {
                    let mut out = Vec::new();
                    flatten_symbols(&tree, "", &mut out);
                    out
                }
                Some(DocumentSymbolResponse::Flat(flat)) => flat
                    .into_iter()
                    .map(|s| FileSymbol {
                        name: s.name,
                        kind: kind_name(s.kind).to_string(),
                        container: s.container_name.unwrap_or_default(),
                        range: s.location.range,
                    })
                    .collect(),
                None => Vec::new(),
            };
            let _ = events.send(Event::Symbols { id, symbols });
        }
        Some(Pending::CodeActions) => {
            let actions = parse::<Vec<CodeActionOrCommand>>(result).unwrap_or_default();
            let _ = events.send(Event::CodeActions { id, actions });
        }
        Some(Pending::ResolveAction) => {
            if let Some(action) = parse::<CodeAction>(result) {
                let _ = events.send(Event::CodeActionResolved { id, action: Box::new(action) });
            }
        }
        Some(Pending::Hover) => {
            let _ = events.send(Event::Hover { id, hover: parse(result) });
        }
        Some(Pending::SignatureHelp) => {
            let _ = events.send(Event::SignatureHelp { id, help: parse(result) });
        }
        Some(Pending::InlayHints) => {
            let hints = parse::<Vec<InlayHint>>(result).unwrap_or_default();
            let _ = events.send(Event::InlayHints { id, hints });
        }
        Some(Pending::Other) => {
            // The only "other" request we care about is initialize (id 1).
            if id == 1 {
                if let Some(init) = parse::<InitializeResult>(result) {
                    *capabilities.lock().unwrap_or_else(PoisonError::into_inner) = Some(init.capabilities);
                }
                let _ = outgoing.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                *ready.lock().unwrap_or_else(PoisonError::into_inner) = true;
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
