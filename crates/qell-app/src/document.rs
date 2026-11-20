//! An open tab: a buffer plus the UI-side caches that go with it
//! (highlight spans, diagnostics, scroll position).

use std::path::PathBuf;

use qell_core::structure::{detect_indent_unit, Brackets};
use qell_core::wrap::WrapLayout;
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
    /// Bracket pairs, for colours and matching.
    pub brackets: Brackets,
    /// Indentation step of the file (guides), e.g. 2 or 4; 1 for tabs.
    pub indent_unit: usize,
    /// (buffer version, tab size) the two above were computed for.
    structure_key: Option<(i32, usize)>,
    /// Rows on screen (word wrap), for the width last drawn at.
    pub wrap: WrapLayout,
    /// (buffer version, wrap width) `wrap` was computed for.
    wrap_key: Option<(i32, usize)>,
    /// Screen column Up/Down aim for across wrapped rows of different
    /// lengths; reset by any other move.
    pub goal_col: Option<usize>,
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
            brackets: Brackets::default(),
            indent_unit: 4,
            structure_key: None,
            wrap: WrapLayout::plain(1),
            wrap_key: None,
            goal_col: None,
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

    /// Lay the text out in rows of `width` columns (0: no wrapping), if the
    /// text or the width changed.
    pub fn update_wrap(&mut self, width: usize) {
        let key = (self.buffer.version(), width);
        if self.wrap_key == Some(key) {
            return;
        }
        self.wrap_key = Some(key);
        self.wrap = if width == 0 {
            WrapLayout::plain(self.buffer.line_count())
        } else {
            WrapLayout::wrapped(self.buffer.rope().lines().map(|l| l.to_string()), width)
        };
    }

    /// Whether `wrap` matches the current text (it is computed when drawing,
    /// so edits earlier in the frame make it stale).
    pub fn wrap_is_current(&self) -> bool {
        self.wrap_key.is_some_and(|(v, _)| v == self.buffer.version())
    }

    /// Up/Down over wrapped rows: `delta` rows, keeping the screen column.
    pub fn move_rows(&mut self, delta: isize, extend: bool) {
        let w = &self.wrap;
        let head = self.buffer.cursor.head();
        let row = w.row_of(head);
        let x = self.goal_col.unwrap_or_else(|| w.screen_col(row, head.col));
        let target = (row as isize + delta).clamp(0, w.row_count() as isize - 1) as usize;
        let b = &self.buffer;
        let pos = w.position_at(target, x, |l| b.line_len(l));
        self.buffer.set_cursor(pos, extend);
        self.goal_col = Some(x);
    }

    /// Recompute brackets and the indentation step if the text changed.
    pub fn update_structure(&mut self, languages: &mut LanguageRegistry, tab_size: usize) {
        let key = (self.buffer.version(), tab_size);
        if self.structure_key == Some(key) {
            return;
        }
        self.spans(languages);
        self.structure_key = Some(key);
        let text = self.buffer.text();
        // Prose has brackets too, but colouring them only adds noise.
        let code = !matches!(self.buffer.language(), None | Some("markdown"));
        self.brackets =
            if code && text.len() <= HIGHLIGHT_LIMIT_BYTES { Brackets::compute(&text, &self.spans) } else { Brackets::default() };
        self.indent_unit = detect_indent_unit(text.lines(), tab_size);
    }
}
