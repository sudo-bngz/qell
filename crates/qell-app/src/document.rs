//! An open tab: a buffer plus the UI-side caches that go with it
//! (highlight spans, diagnostics, scroll position).

use std::path::{Path, PathBuf};

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
    /// Opened from the cluster view (describe, YAML, a manifest), so Esc
    /// and closing the tab go back to the list.
    pub from_cluster: bool,
    /// A Markdown file shown rendered beside its source.
    pub preview: bool,
    /// A picture, a video or a sound file: looked at, not edited.
    pub media: Option<MediaTab>,
    /// Bracket pairs, for colours and matching.
    pub brackets: Brackets,
    /// Indentation step of the file (guides), e.g. 2 or 4; 1 for tabs.
    pub indent_unit: usize,
    /// (buffer version, tab size) the two above were computed for.
    structure_key: Option<(i32, usize)>,
    /// The file as git has it, to mark what changed since (`None`: not
    /// tracked, or not fetched yet).
    pub git_base: Option<std::sync::Arc<str>>,
    pub git_base_state: BaseState,
    /// Per line: how it differs from `git_base`.
    pub changes: Vec<LineChange>,
    changes_key: Option<i32>,
    /// Size and time of the file when it was last read or written.
    pub disk: Option<DiskState>,
    /// It changed (or went) on disk since, and you have unsaved edits:
    /// waiting for you to pick a side.
    pub conflict: Option<DiskChange>,
    /// Rows on screen (word wrap), for the width last drawn at.
    pub wrap: WrapLayout,
    /// (buffer version, wrap width) `wrap` was computed for.
    wrap_key: Option<(i32, usize, u64)>,
    /// Folded regions, by header line.
    pub folds: std::collections::BTreeSet<usize>,
    /// What can be folded: header line → last line of its region.
    pub fold_regions: std::collections::BTreeMap<usize, usize>,
    /// The text the regions were found in, and its line count.
    fold_key: Option<(i32, usize)>,
    /// Bumped when `folds` changes, so the layout is redone.
    folds_gen: u64,
    /// Screen column Up/Down aim for across wrapped rows of different
    /// lengths; reset by any other move.
    pub goal_col: Option<usize>,
    /// The miniature beside the editor.
    pub minimap: crate::views::minimap::Minimap,
}

/// What happened to a file while it was open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskChange {
    Modified,
    Deleted,
}

/// What the file looked like on disk, to notice changes without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskState {
    len: u64,
    mtime: Option<std::time::SystemTime>,
}

impl DiskState {
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self { len: m.len(), mtime: m.modified().ok() })
    }
}

/// Whether git's version of a file has been asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseState {
    /// Not asked yet, or out of date (git noticed changes).
    Unknown,
    Loading,
    Loaded,
}

/// How a line differs from git's version of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineChange {
    pub added: bool,
    pub modified: bool,
    /// Lines were deleted just above this one / just below it (the end).
    pub deleted_above: bool,
    pub deleted_below: bool,
}

/// A run of lines that differ from git's version: which lines it replaces
/// there (`base`) and which lines it is here (`new`). Either can be empty,
/// for lines only added or only deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub base: std::ops::Range<usize>,
    pub new: std::ops::Range<usize>,
}

impl Hunk {
    /// Whether `line` is in the hunk; a deletion (no lines of its own) is
    /// reached from the line it sits above, or the last line of the file.
    pub fn covers(&self, line: usize) -> bool {
        if self.new.is_empty() {
            self.new.start == line || (self.new.start > 0 && self.new.start - 1 == line)
        } else {
            self.new.contains(&line)
        }
    }
}

/// The runs of lines of `text` that differ from `base`, in order.
pub fn hunks(base: &str, text: &str) -> Vec<Hunk> {
    use similar::DiffOp;
    let diff = similar::TextDiff::configure().timeout(std::time::Duration::from_millis(200)).diff_lines(base, text);
    let mut out: Vec<Hunk> = Vec::new();
    for op in diff.ops() {
        if matches!(op, DiffOp::Equal { .. }) {
            continue;
        }
        let (old, new) = (op.old_range(), op.new_range());
        // Changes that touch join into one hunk, as git's own diff does.
        match out.last_mut() {
            Some(h) if h.base.end == old.start && h.new.end == new.start => {
                h.base.end = old.end;
                h.new.end = new.end;
            }
            _ => out.push(Hunk { base: old, new }),
        }
    }
    out
}

/// Marks for each line of `text` compared with `base`.
pub fn line_changes(base: &str, text: &str) -> Vec<LineChange> {
    use similar::DiffOp;
    let diff = similar::TextDiff::configure().timeout(std::time::Duration::from_millis(40)).diff_lines(base, text);
    let mut marks: Vec<LineChange> = Vec::new();
    fn at(marks: &mut Vec<LineChange>, i: usize) -> &mut LineChange {
        if marks.len() <= i {
            marks.resize(i + 1, LineChange::default());
        }
        &mut marks[i]
    }
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert { new_index, new_len, .. } => {
                (new_index..new_index + new_len).for_each(|l| at(&mut marks, l).added = true)
            }
            DiffOp::Replace { new_index, new_len, .. } => {
                (new_index..new_index + new_len).for_each(|l| at(&mut marks, l).modified = true)
            }
            DiffOp::Delete { new_index, .. } => {
                // Past the last line: mark the end of the one before.
                if new_index >= text.lines().count() && new_index > 0 {
                    at(&mut marks, new_index - 1).deleted_below = true;
                } else {
                    at(&mut marks, new_index).deleted_above = true;
                }
            }
        }
    }
    marks
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

/// A tab showing a picture, a video or a sound file.
pub struct MediaTab {
    pub kind: crate::preview::Media,
    pub size: u64,
    /// The picture on screen: the file itself, or a frame of the video.
    pub texture: Option<egui::TextureHandle>,
    pub pixels: (usize, usize),
    /// What ffprobe said, once it has said it.
    pub info: Option<crate::preview::Info>,
    /// Where in the video the frame came from, and where the slider is.
    pub at: f64,
    pub wanted: f64,
    /// What went wrong, if anything: a decoder error, no ffmpeg.
    pub error: Option<String>,
    /// Nothing has been read yet.
    pub pending: bool,
    /// Show it at its own size rather than fitted to the window.
    pub actual_size: bool,
}

impl MediaTab {
    fn new(kind: crate::preview::Media, size: u64) -> Self {
        Self {
            kind,
            size,
            texture: None,
            pixels: (0, 0),
            info: None,
            at: 0.0,
            wanted: 0.0,
            error: None,
            pending: true,
            actual_size: false,
        }
    }
}

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
            from_cluster: false,
            preview: false,
            media: None,
            brackets: Brackets::default(),
            indent_unit: 4,
            structure_key: None,
            git_base: None,
            git_base_state: BaseState::Unknown,
            changes: Vec::new(),
            changes_key: None,
            disk: None,
            conflict: None,
            wrap: WrapLayout::plain(1),
            wrap_key: None,
            folds: Default::default(),
            fold_regions: Default::default(),
            fold_key: None,
            folds_gen: 0,
            goal_col: None,
            minimap: Default::default(),
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
        // A picture or a video isn't text to be read into a buffer.
        if let Some(kind) = crate::preview::Media::of(&path) {
            return Ok(Self::media(path, kind));
        }
        let mut doc = Self::new(Buffer::open(&path)?);
        doc.disk = DiskState::of(&path);
        Ok(doc)
    }

    /// A tab that shows a file rather than holding its text.
    fn media(path: PathBuf, kind: crate::preview::Media) -> Self {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // The buffer keeps the path, so tabs, the file tree and the
        // session treat it like any other open file.
        let mut buffer = Buffer::from_text("", Some(path));
        buffer.set_clean();
        let mut doc = Self::new(buffer);
        doc.read_only = true;
        doc.media = Some(MediaTab::new(kind, size));
        doc
    }

    /// Note the file as it is now: just written, or its changes accepted.
    pub fn note_disk(&mut self) {
        self.disk = self.buffer.path().and_then(DiskState::of);
        self.conflict = None;
    }

    /// How the file differs from what was last read or written. `None`
    /// for an untitled buffer, a diff view, or a file nobody touched.
    pub fn disk_change(&self) -> Option<DiskChange> {
        let path = self.buffer.path().filter(|_| !self.read_only)?;
        let known = self.disk.as_ref()?;
        match DiskState::of(path) {
            Some(now) if now == *known => None,
            Some(_) => Some(DiskChange::Modified),
            // A rename or a checkout can make it blink out of existence;
            // only call it deleted once it stays gone.
            None if path.exists() => None,
            None => Some(DiskChange::Deleted),
        }
    }

    /// Take the file's text as it is on disk, keeping the cursor where it
    /// was. The swap is a normal edit, so undo brings your version back.
    pub fn reload(&mut self) -> std::io::Result<()> {
        let path = self
            .buffer
            .path()
            .map(Path::to_path_buf)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "buffer has no path"))?;
        let text = std::fs::read_to_string(&path)?;
        let head = self.buffer.cursor.head();
        let end = self.buffer.end();
        // Its own undo step, so one undo gives your version back whole.
        self.buffer.begin_undo_group();
        self.buffer.replace_range(qell_core::Position::new(0, 0), end, &text);
        self.buffer.end_undo_group();
        let p = self.buffer.clamp(head);
        self.buffer.set_cursor(p, false);
        self.buffer.set_clean();
        self.scroll_to_cursor = true;
        // Everything derived from the old text is out of date.
        self.spans_version = None;
        self.structure_key = None;
        self.changes_key = None;
        self.git_base_state = BaseState::Unknown;
        self.note_disk();
        Ok(())
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

    /// Git's version of the file arrived (or there is none).
    pub fn set_git_base(&mut self, base: Option<String>) {
        self.git_base = base.map(Into::into);
        self.git_base_state = BaseState::Loaded;
        self.changes_key = None;
    }

    /// Recompute the gutter's change marks if the text changed.
    pub fn update_changes(&mut self) {
        let version = self.buffer.version();
        if self.changes_key == Some(version) {
            return;
        }
        self.changes_key = Some(version);
        self.changes = match &self.git_base {
            Some(base) if self.buffer.rope().len_bytes() <= HIGHLIGHT_LIMIT_BYTES => line_changes(base, &self.buffer.text()),
            _ => Vec::new(),
        };
    }

    /// Lay the text out in rows of `width` columns (0: no wrapping), if the
    /// text or the width changed.
    pub fn update_wrap(&mut self, width: usize) {
        self.update_folds();
        let key = (self.buffer.version(), width, self.folds_gen);
        if self.wrap_key == Some(key) {
            return;
        }
        self.wrap_key = Some(key);
        self.wrap = if width == 0 && self.folds.is_empty() {
            WrapLayout::plain(self.buffer.line_count())
        } else {
            let mut hidden = vec![false; self.buffer.line_count()];
            for (h, end) in self.folds.iter().filter_map(|h| Some((*h, *self.fold_regions.get(h)?))) {
                for slot in hidden.iter_mut().take(end + 1).skip(h + 1) {
                    *slot = true;
                }
            }
            WrapLayout::layout(self.buffer.rope().lines().map(|l| l.to_string()), width, |l| {
                hidden.get(l).copied().unwrap_or(false)
            })
        };
    }

    /// Keep the folds right as the text changes: find the regions again,
    /// move folds below an edit with it, drop those whose region is gone,
    /// and open any the cursor has gone into (a search, a jump).
    fn update_folds(&mut self) {
        let lines = self.buffer.line_count();
        let version = self.buffer.version();
        let mut changed = false;
        if self.fold_key.map(|(v, _)| v) != Some(version) {
            let before = self.fold_key.map_or(lines, |(_, n)| n);
            self.fold_key = Some((version, lines));
            self.fold_regions = qell_core::fold::regions(self.buffer.rope().lines().map(|l| l.to_string()), 4);
            if !self.folds.is_empty() {
                let delta = lines as isize - before as isize;
                let at = self.buffer.cursor.head().line as isize;
                let moved: std::collections::BTreeSet<usize> = self
                    .folds
                    .iter()
                    .filter_map(|&h| {
                        let h = h as isize;
                        // Lines went in above (the cursor ends after them)
                        // or came out above (the cursor ends where they were).
                        let h = if delta > 0 && h > at - delta {
                            h + delta
                        } else if delta < 0 && h > at {
                            let n = h + delta;
                            (n > at).then_some(n)?
                        } else {
                            h
                        };
                        Some(h as usize)
                    })
                    .filter(|h| self.fold_regions.contains_key(h))
                    .collect();
                changed = moved != self.folds;
                self.folds = moved;
            }
        }
        let line = self.buffer.cursor.head().line;
        let inside: Vec<usize> =
            self.folds.iter().copied().filter(|h| self.fold_regions.get(h).is_some_and(|&e| line > *h && line <= e)).collect();
        if !inside.is_empty() {
            for h in inside {
                self.folds.remove(&h);
            }
            changed = true;
        }
        if changed {
            self.folds_gen += 1;
        }
    }

    /// Fold the innermost open region around `line`. Says whether one was.
    pub fn fold_at(&mut self, line: usize) -> bool {
        let open: std::collections::BTreeMap<usize, usize> =
            self.fold_regions.iter().filter(|(h, _)| !self.folds.contains(h)).map(|(h, e)| (*h, *e)).collect();
        let Some((h, _)) = qell_core::fold::around(&open, line) else { return false };
        self.set_fold(h, true);
        true
    }

    /// Open the fold at `line`, or the innermost one around it.
    pub fn unfold_at(&mut self, line: usize) -> bool {
        if self.folds.contains(&line) {
            self.set_fold(line, false);
            return true;
        }
        let folded: std::collections::BTreeMap<usize, usize> =
            self.fold_regions.iter().filter(|(h, _)| self.folds.contains(h)).map(|(h, e)| (*h, *e)).collect();
        let Some((h, _)) = qell_core::fold::around(&folded, line) else { return false };
        self.set_fold(h, false);
        true
    }

    /// Fold or open the region headed by `line`.
    pub fn set_fold(&mut self, line: usize, folded: bool) {
        let changed = if folded && self.fold_regions.contains_key(&line) {
            // The cursor can't stay inside: it moves to the header.
            let end = self.fold_regions[&line];
            let head = self.buffer.cursor.head();
            if head.line > line && head.line <= end {
                let p = qell_core::Position::new(line, self.buffer.line_len(line));
                self.buffer.set_cursor(p, false);
            }
            self.folds.insert(line)
        } else {
            self.folds.remove(&line)
        };
        if changed {
            self.folds_gen += 1;
        }
    }

    /// Fold every region (`true`) or open them all.
    pub fn fold_all(&mut self, folded: bool) {
        if folded {
            let head = self.buffer.cursor.head().line;
            // The cursor goes to the top-level header it was under.
            if let Some((h, _)) = self.fold_regions.iter().find(|(h, e)| **h < head && **e >= head) {
                let p = qell_core::Position::new(*h, self.buffer.line_len(*h));
                self.buffer.set_cursor(p, false);
            }
            self.folds = self.fold_regions.keys().copied().collect();
        } else {
            self.folds.clear();
        }
        self.folds_gen += 1;
    }

    /// Whether `wrap` matches the current text (it is computed when drawing,
    /// so edits earlier in the frame make it stale).
    pub fn wrap_is_current(&self) -> bool {
        self.wrap_key.is_some_and(|(v, _, _)| v == self.buffer.version())
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

#[cfg(test)]
mod change_tests {
    use super::*;

    fn kinds(base: &str, text: &str) -> String {
        let marks = line_changes(base, text);
        (0..text.lines().count().max(marks.len()))
            .map(|i| {
                let m = marks.get(i).copied().unwrap_or_default();
                let c = if m.added {
                    'A'
                } else if m.modified {
                    'M'
                } else {
                    '.'
                };
                format!("{}{c}{}", if m.deleted_above { "^" } else { "" }, if m.deleted_below { "v" } else { "" })
            })
            .collect()
    }

    #[test]
    fn added_modified_and_deleted_lines() {
        let base = "a\nb\nc\nd\n";
        assert_eq!(kinds(base, base), "....");
        assert_eq!(kinds(base, "a\nb\nNEW\nc\nd\n"), "..A..");
        assert_eq!(kinds(base, "a\nB\nc\nd\n"), ".M..");
        assert_eq!(kinds(base, "a\nc\nd\n"), ".^..");
        assert_eq!(kinds(base, "a\nb\nc\n"), "...v");
        assert_eq!(kinds(base, "b\nc\nd\n"), "^...");
        assert_eq!(kinds("", "x\ny\n"), "AA");
    }
}

#[cfg(test)]
mod disk_tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        // Enough of a pause that the file's timestamp really moves.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn notices_changes_made_outside_qell() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.tf");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        assert_eq!(doc.disk_change(), None);

        // Changed on disk, same length: caught by the timestamp.
        write(&path, "one\nTWO\n");
        assert_eq!(doc.disk_change(), Some(DiskChange::Modified));
        doc.reload().unwrap();
        assert_eq!(doc.buffer.text(), "one\nTWO\n");
        assert_eq!(doc.disk_change(), None);
        assert!(!doc.buffer.is_dirty());

        // Gone.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(doc.disk_change(), Some(DiskChange::Deleted));
    }

    #[test]
    fn reload_keeps_the_cursor_and_can_be_undone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        doc.buffer.set_cursor(qell_core::Position::new(2, 5), false);
        doc.buffer.insert("X");
        assert!(doc.buffer.is_dirty());

        write(&path, "fn a() {}\n");
        assert_eq!(doc.disk_change(), Some(DiskChange::Modified));
        doc.reload().unwrap();
        assert_eq!(doc.buffer.text(), "fn a() {}\n");
        // The cursor was past the new end: pulled back inside it.
        let head = doc.buffer.cursor.head();
        assert!(head.line < doc.buffer.line_count() && head.col <= doc.buffer.line_len(head.line));
        // The edited version is one undo away.
        assert!(doc.buffer.undo());
        assert!(doc.buffer.text().contains("fn c(X) {}"), "{}", doc.buffer.text());
    }

    #[test]
    fn a_saved_file_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let mut doc = Document::open(path.clone()).unwrap();
        doc.buffer.insert("y");
        std::thread::sleep(std::time::Duration::from_millis(20));
        doc.buffer.save().unwrap();
        doc.note_disk();
        assert_eq!(doc.disk_change(), None);
    }
}

#[cfg(test)]
mod hunk_tests {
    use super::*;

    #[test]
    fn one_hunk_per_run_of_changed_lines() {
        let base = "a\nb\nc\nd\ne\n";
        let text = "a\nB\nc\nd\ne\nf\n";
        let hs = hunks(base, text);
        assert_eq!(hs, vec![Hunk { base: 1..2, new: 1..2 }, Hunk { base: 5..5, new: 5..6 }]);
        assert!(hs[0].covers(1));
        assert!(!hs[0].covers(2));
    }

    #[test]
    fn deleted_lines_are_a_hunk_with_no_lines_here() {
        let hs = hunks("a\nb\nc\n", "a\nc\n");
        assert_eq!(hs, vec![Hunk { base: 1..2, new: 1..1 }]);
        // Reached from either side of where they were.
        assert!(hs[0].covers(0));
        assert!(hs[0].covers(1));
    }

    #[test]
    fn touching_changes_join() {
        // A line replaced and another added right after it.
        let hs = hunks("a\nb\n", "a\nB\nC\n");
        assert_eq!(hs, vec![Hunk { base: 1..2, new: 1..3 }]);
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use qell_core::Position;

    const CODE: &str = "fn a() {\n    one();\n    two();\n}\n\nfn b() {\n    three();\n}\n";

    fn doc() -> Document {
        let mut d = Document::new(Buffer::from_text(CODE, None));
        d.update_wrap(0);
        d
    }

    fn rows(d: &Document) -> Vec<usize> {
        (0..d.wrap.row_count()).map(|r| d.wrap.row(r).line).collect()
    }

    #[test]
    fn folding_hides_the_body_and_keeps_the_closing_line() {
        let mut d = doc();
        assert!(d.fold_at(1), "inside a's body folds a");
        d.update_wrap(0);
        assert_eq!(rows(&d)[..3], [0, 3, 4]);
        // The cursor was inside: it is moved to the header.
        assert_eq!(d.buffer.cursor.head().line, 0);
        assert!(d.unfold_at(0));
        d.update_wrap(0);
        assert_eq!(rows(&d).len(), d.buffer.line_count());
    }

    #[test]
    fn a_fold_moves_with_lines_added_above_it() {
        let mut d = doc();
        d.set_fold(5, true);
        d.update_wrap(0);
        d.buffer.set_cursor(Position::new(0, 0), false);
        d.buffer.insert("// one\n// two\n");
        d.update_wrap(0);
        assert!(d.folds.contains(&7), "{:?}", d.folds);
        assert!(!rows(&d).contains(&8), "b's body is still hidden");
    }

    #[test]
    fn going_into_a_fold_opens_it() {
        let mut d = doc();
        d.fold_all(true);
        d.update_wrap(0);
        assert_eq!(d.folds.len(), 2);
        d.buffer.set_cursor(Position::new(6, 2), false);
        d.update_wrap(0);
        assert!(!d.folds.contains(&5));
        assert!(d.folds.contains(&0));
    }
}
