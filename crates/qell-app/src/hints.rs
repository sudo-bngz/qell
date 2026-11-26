//! What the language server says about the code as it is typed: the
//! signature of the call the cursor is in, and inlay hints (types and
//! parameter names shown inside the code).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use qell_core::Position;
use qell_lsp::convert::from_lsp_position;
use qell_lsp::lsp_types::{InlayHint, InlayHintLabel, SignatureHelp};
use qell_lsp::RequestId;

use crate::app::{Focus, QellApp};
use crate::document::Document;

/// Ask for inlay hints this long after the text stops changing.
const INLAY_SETTLE: Duration = Duration::from_millis(350);
/// With no server ready yet, try again this often.
const INLAY_RETRY: Duration = Duration::from_secs(2);

/// The signature help popup.
#[derive(Default)]
pub struct Signature {
    /// Shown while `Some`.
    pub help: Option<SignatureHelp>,
    request: Option<RequestId>,
    /// File, text version and cursor it was last asked for, to ask again
    /// when they change while it shows.
    asked: Option<(PathBuf, i32, Position)>,
}

/// One inlay hint, in the document's char columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inlay {
    pub line: usize,
    /// The hint is drawn before this char (after the line's end when it
    /// is the line's length).
    pub col: usize,
    /// The text drawn, padding included.
    pub label: String,
}

/// Inlay hints of the active tab.
#[derive(Default)]
pub struct InlayState {
    /// A request in flight: its file and text version.
    pending: Option<(RequestId, PathBuf, i32)>,
    /// When the active text was seen changed, or a request was last tried.
    since: Option<(PathBuf, i32, Instant)>,
    tried: Option<Instant>,
}

impl QellApp {
    /// Open the signature help at the cursor (a trigger was typed, or
    /// Ctrl/⌘+Shift+Space).
    pub fn request_signature(&mut self, explicit: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let retrigger = self.signature.help.is_some();
        let lang = doc.buffer.language().unwrap_or("").to_string();
        let id = self.lsp.request_signature_help(&self.languages, &mut doc.buffer, retrigger);
        let asked = doc.buffer.path().map(|p| (p.to_path_buf(), doc.buffer.version(), doc.buffer.cursor.head()));
        match id {
            Some(id) => {
                self.signature.request = Some(id);
                self.signature.asked = asked;
            }
            None if explicit => {
                let hint = self.no_server_hint(&lang, "Parameter hints need");
                self.notify(hint);
            }
            None => {}
        }
    }

    pub fn close_signature(&mut self) {
        self.signature = Signature::default();
    }

    pub fn handle_signature(&mut self, id: RequestId, help: Option<SignatureHelp>) {
        if self.signature.request != Some(id) {
            return;
        }
        self.signature.request = None;
        self.signature.help = help.filter(|h| !h.signatures.is_empty());
    }

    pub fn handle_inlay_hints(&mut self, id: RequestId, hints: Vec<InlayHint>) {
        let Some((pending, path, version)) = self.inlay.pending.take() else { return };
        if pending != id {
            self.inlay.pending = Some((pending, path, version));
            return;
        }
        let Some(doc) = self.docs.iter_mut().find(|d| d.buffer.path() == Some(path.as_path())) else { return };
        doc.set_inlays(convert_hints(doc, hints), version);
    }

    /// Every frame: keep the signature help on the call the cursor is in,
    /// and the inlay hints of the active tab up to date.
    pub fn update_hints(&mut self, ctx: &egui::Context) {
        // Signature help: asked again as the cursor moves or the text
        // changes; gone with the focus or the tab.
        if self.signature.help.is_some() || self.signature.request.is_some() {
            let here = self
                .docs
                .get(self.active)
                .and_then(|d| d.buffer.path().map(|p| (p.to_path_buf(), d.buffer.version(), d.buffer.cursor.head())));
            let same_file = here.as_ref().map(|h| &h.0) == self.signature.asked.as_ref().map(|a| &a.0);
            if self.focus != Focus::Editor || !same_file || self.modal_open() {
                self.close_signature();
            } else if self.signature.request.is_none() && here != self.signature.asked {
                self.request_signature(false);
            }
        }

        // Inlay hints.
        let on = self.config.inlay_hints;
        for d in &mut self.docs {
            d.follow_inlays();
            if !on {
                d.inlays.clear();
            }
        }
        if !on || self.inlay.pending.is_some() {
            return;
        }
        let Some(doc) = self.docs.get(self.active) else { return };
        let Some(path) = doc.buffer.path().map(|p| p.to_path_buf()) else { return };
        let version = doc.buffer.version();
        if doc.inlays_version() == Some(version) {
            return;
        }
        // Let the typing pause first.
        let since = match &self.inlay.since {
            Some((p, v, at)) if *p == path && *v == version => *at,
            _ => {
                let now = Instant::now();
                self.inlay.since = Some((path.clone(), version, now));
                now
            }
        };
        let fresh_tab = doc.inlays_version().is_none();
        if !fresh_tab && since.elapsed() < INLAY_SETTLE {
            ctx.request_repaint_after(INLAY_SETTLE - since.elapsed());
            return;
        }
        // No server (yet, or one without hints): try again now and then.
        if let Some(t) = self.inlay.tried.filter(|t| t.elapsed() < INLAY_RETRY) {
            ctx.request_repaint_after(INLAY_RETRY - t.elapsed());
            return;
        }
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        match self.lsp.request_inlay_hints(&self.languages, &mut doc.buffer) {
            Some(id) => {
                self.inlay.pending = Some((id, path, version));
                self.inlay.tried = None;
            }
            None => self.inlay.tried = Some(Instant::now()),
        }
    }
}

/// The server's hints in the document's columns, sorted.
fn convert_hints(doc: &Document, hints: Vec<InlayHint>) -> Vec<Inlay> {
    let mut out: Vec<Inlay> = hints
        .into_iter()
        .filter_map(|h| {
            let line = h.position.line as usize;
            if line >= doc.buffer.line_count() {
                return None;
            }
            let text = doc.buffer.line(line);
            let col = from_lsp_position(&text, h.position).col.min(doc.buffer.line_len(line));
            let mut label = match h.label {
                InlayHintLabel::String(s) => s,
                InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|p| p.value).collect(),
            };
            // One line, and not too long.
            label = label.replace(['\n', '\r', '\t'], " ");
            if label.chars().count() > 40 {
                label = format!("{}…", label.chars().take(39).collect::<String>());
            }
            // A type after a name reads `total: number[]`, without a gap.
            if h.padding_left == Some(true) && !label.starts_with(':') {
                label.insert(0, ' ');
            }
            if h.padding_right == Some(true) {
                label.push(' ');
            }
            Some(Inlay { line, col, label })
        })
        .collect();
    out.sort_by_key(|h| (h.line, h.col));
    out
}

/// The parts of a signature's label: before the active parameter, the
/// parameter, after it.
pub fn split_label(help: &SignatureHelp) -> Option<(String, String, String, usize, usize)> {
    let n = help.signatures.len();
    let active = (help.active_signature.unwrap_or(0) as usize).min(n.checked_sub(1)?);
    let sig = &help.signatures[active];
    let label = &sig.label;
    let param = sig.active_parameter.or(help.active_parameter).map(|p| p as usize);
    let range = param.and_then(|p| sig.parameters.as_ref()?.get(p)).and_then(|p| match &p.label {
        qell_lsp::lsp_types::ParameterLabel::Simple(s) => label.find(s.as_str()).map(|i| (i, i + s.len())),
        qell_lsp::lsp_types::ParameterLabel::LabelOffsets([a, b]) => {
            // UTF-16 offsets into the label.
            let byte = |u: u32| {
                let mut units = 0;
                for (i, c) in label.char_indices() {
                    if units >= u {
                        return i;
                    }
                    units += c.len_utf16() as u32;
                }
                label.len()
            };
            Some((byte(*a), byte(*b)))
        }
    });
    let (a, b) = range.unwrap_or((label.len(), label.len()));
    Some((label[..a].to_string(), label[a..b].to_string(), label[b..].to_string(), active, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qell_lsp::lsp_types::{ParameterInformation, ParameterLabel, SignatureInformation};

    fn help(params: Vec<ParameterLabel>, active: u32) -> SignatureHelp {
        SignatureHelp {
            signatures: vec![SignatureInformation {
                label: "func Println(a ...any) (n int, err error)".into(),
                documentation: None,
                parameters: Some(params.into_iter().map(|label| ParameterInformation { label, documentation: None }).collect()),
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter: Some(active),
        }
    }

    #[test]
    fn the_active_parameter_is_cut_out() {
        let by_name = help(vec![ParameterLabel::Simple("a ...any".into())], 0);
        let (pre, p, post, i, n) = split_label(&by_name).unwrap();
        assert_eq!((pre.as_str(), p.as_str(), post.as_str(), i, n), ("func Println(", "a ...any", ") (n int, err error)", 0, 1));
        let by_offsets = help(vec![ParameterLabel::LabelOffsets([13, 21])], 0);
        assert_eq!(split_label(&by_offsets).unwrap().1, "a ...any");
        // No such parameter: nothing stands out.
        let none = help(vec![], 3);
        assert_eq!(split_label(&none).unwrap().1, "");
    }
}
