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
    /// A view that can't be edited (a diff).
    pub read_only: bool,
    /// Per-line marks of an inline diff; empty for normal files.
    pub line_marks: Vec<LineMark>,
    /// Tab title instead of the file name (diffs: "main.tf (f71c98)").
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMark {
    Context,
    Added,
    Removed,
}

/// Files above this size are shown without highlighting; re-highlighting
/// the whole text on every keystroke would stutter.
const HIGHLIGHT_LIMIT_BYTES: usize = 2 * 1024 * 1024;

impl Document {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            spans: Vec::new(),
            spans_version: None,
            diagnostics: Vec::new(),
            scroll_to_cursor: true,
            read_only: false,
            line_marks: Vec::new(),
            label: None,
        }
    }

    /// A read-only inline diff of `path` (used for highlighting only).
    pub fn diff(label: String, path: &std::path::Path, text: &str, marks: Vec<LineMark>) -> Self {
        let mut buffer = Buffer::from_text(text, None);
        buffer.set_language(qell_core::syntax::language_for_path(path));
        // Open on the first change rather than the top of the file.
        let first = marks.iter().position(|m| *m != LineMark::Context).unwrap_or(0);
        buffer.set_cursor(qell_core::Position::new(first, 0), false);
        let mut doc = Self::new(buffer);
        doc.read_only = true;
        doc.line_marks = marks;
        doc.label = Some(label);
        doc
    }

    pub fn untitled() -> Self {
        Self::new(Buffer::empty())
    }

    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        Ok(Self::new(Buffer::open(path)?))
    }

    pub fn title(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
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
