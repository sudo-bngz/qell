//! An open tab: a buffer plus the UI-side caches that go with it
//! (highlight spans, diagnostics, scroll position).

use std::path::PathBuf;

use qell_core::{Buffer, HighlightSpan, LanguageRegistry};
use qell_lsp::lsp_types::Diagnostic;

pub struct Document {
    pub buffer: Buffer,
    spans: Vec<HighlightSpan>,
    /// Buffer version the spans were computed for.
    spans_version: Option<i32>,
    pub diagnostics: Vec<Diagnostic>,
    /// Line the view is scrolled to; the editor keeps the cursor visible.
    pub scroll_to_cursor: bool,
}

/// Files above this size are shown without highlighting; re-highlighting
/// the whole text on every keystroke would stutter.
const HIGHLIGHT_LIMIT_BYTES: usize = 2 * 1024 * 1024;

impl Document {
    pub fn new(buffer: Buffer) -> Self {
        Self { buffer, spans: Vec::new(), spans_version: None, diagnostics: Vec::new(), scroll_to_cursor: true }
    }

    pub fn untitled() -> Self {
        Self::new(Buffer::empty())
    }

    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        Ok(Self::new(Buffer::open(path)?))
    }

    pub fn title(&self) -> String {
        let mut t = self.buffer.file_name();
        if self.buffer.is_dirty() {
            t.push_str(" •");
        }
        t
    }

    /// Highlight spans for the current text, recomputed only when the
    /// buffer changed since the last call.
    pub fn spans(&mut self, languages: &mut LanguageRegistry) -> &[HighlightSpan] {
        let version = self.buffer.version();
        if self.spans_version != Some(version) {
            self.spans_version = Some(version);
            self.spans = match self.buffer.language() {
                Some(lang) if self.buffer.rope().len_bytes() <= HIGHLIGHT_LIMIT_BYTES => {
                    languages.highlight(lang, &self.buffer.text())
                }
                _ => Vec::new(),
            };
        }
        &self.spans
    }
}
