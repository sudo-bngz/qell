//! The text buffer.
//!
//! A [`Buffer`] is one open file: a `ropey` rope holding the text, a cursor,
//! the undo history, a language id for highlighting/LSP, and a `version`
//! counter that increments on every change (language servers need it).

use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::history::{Edit, History};
use crate::position::{Cursor, Position};

/// Something that changed in the buffer, in a form a language server can
/// consume (`textDocument/didChange` wants exactly this).
#[derive(Debug, Clone)]
pub struct Change {
    pub start: Position,
    pub end: Position,
    pub text: String,
    /// `start` / `end` columns in UTF-16 code units, measured on the text
    /// as it was when the change happened. Language servers count columns
    /// that way, and a later change in the same batch can move lines, so
    /// they can't be recomputed from the final text.
    pub start_utf16: usize,
    pub end_utf16: usize,
}

#[derive(Debug)]
pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    language: Option<String>,
    pub cursor: Cursor,
    /// Cursors besides the main one, in document order. Empty nearly
    /// always; typing, deleting and moving happen at every one of them.
    extra: Vec<Cursor>,
    history: History,
    version: i32,
    dirty: bool,
    /// Changes since the last call to [`Buffer::take_changes`]. The LSP
    /// client drains this; nobody else should touch it.
    pending_changes: Vec<Change>,
}

impl Buffer {
    pub fn empty() -> Self {
        Self::from_text(String::new(), None)
    }

    pub fn from_text(text: impl AsRef<str>, path: Option<PathBuf>) -> Self {
        let language = path.as_deref().and_then(crate::syntax::language_for_path);
        Self {
            rope: Rope::from_str(text.as_ref()),
            path,
            language,
            cursor: Cursor::default(),
            extra: Vec::new(),
            history: History::new(),
            version: 0,
            dirty: false,
            pending_changes: Vec::new(),
        }
    }

    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_text(text, Some(path.to_path_buf())))
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "buffer has no path"));
        };
        self.rope.write_to(std::io::BufWriter::new(std::fs::File::create(path)?))?;
        self.dirty = false;
        Ok(())
    }

    /// Point the buffer at another file (after a rename on disk), keeping
    /// its text; the language follows the new name.
    pub fn set_path(&mut self, path: PathBuf) {
        self.language = crate::syntax::language_for_path(&path);
        self.path = Some(path);
    }

    pub fn save_as(&mut self, path: PathBuf) -> std::io::Result<()> {
        self.language = crate::syntax::language_for_path(&path);
        self.path = Some(path);
        self.save()
    }

    // ---- accessors -------------------------------------------------------

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string())
    }

    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    pub fn set_language(&mut self, id: Option<String>) {
        self.language = id;
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    /// Treat the text as matching the file on disk (after a reload).
    pub fn set_clean(&mut self) {
        self.dirty = false;
    }

    /// The very end of the text.
    pub fn end(&self) -> Position {
        let last = self.line_count().saturating_sub(1);
        Position::new(last, self.line_len(last))
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line text without its trailing newline.
    pub fn line(&self, idx: usize) -> String {
        let s = self.rope.line(idx).to_string();
        s.trim_end_matches(['\n', '\r']).to_string()
    }

    pub fn line_len(&self, idx: usize) -> usize {
        self.line(idx).chars().count()
    }

    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.pending_changes)
    }

    // ---- position math ---------------------------------------------------

    pub fn pos_to_char(&self, p: Position) -> usize {
        let line = p.line.min(self.line_count().saturating_sub(1));
        let col = p.col.min(self.line_len(line));
        self.rope.line_to_char(line) + col
    }

    pub fn char_to_pos(&self, idx: usize) -> Position {
        let idx = idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        Position { line, col: idx - self.rope.line_to_char(line) }
    }

    pub fn clamp(&self, p: Position) -> Position {
        let line = p.line.min(self.line_count().saturating_sub(1));
        Position { line, col: p.col.min(self.line_len(line)) }
    }

    // ---- editing ---------------------------------------------------------

    /// The one place text actually changes. Everything else routes here so
    /// that history, versioning and LSP change tracking stay consistent.
    fn apply(&mut self, start: usize, old_len: usize, new: &str, record: bool) {
        let old: String = self.rope.slice(start..start + old_len).to_string();
        let start_pos = self.char_to_pos(start);
        let end_pos = self.char_to_pos(start + old_len);
        let utf16_col = |p: Position| -> usize {
            let line_start = self.rope.line_to_char(p.line);
            self.rope.slice(line_start..line_start + p.col).chars().map(char::len_utf16).sum()
        };
        let (start_utf16, end_utf16) = (utf16_col(start_pos), utf16_col(end_pos));
        let cursor_before = self.cursor.head();

        self.rope.remove(start..start + old_len);
        self.rope.insert(start, new);

        let after = self.char_to_pos(start + new.chars().count());
        self.cursor = Cursor::at(after);
        self.version += 1;
        self.dirty = true;
        self.pending_changes.push(Change { start: start_pos, end: end_pos, text: new.to_string(), start_utf16, end_utf16 });

        if record {
            self.history.record(Edit { start, old, new: new.to_string(), cursor_before, cursor_after: after });
        }
    }

    // ---- several cursors at once -----------------------------------------

    /// Every cursor, the main one included, in document order.
    pub fn cursors(&self) -> Vec<Cursor> {
        let mut all = self.extra.clone();
        all.push(self.cursor);
        all.sort_by_key(|c| c.selection.ordered().0);
        all
    }

    pub fn extra_cursors(&self) -> &[Cursor] {
        &self.extra
    }

    pub fn has_extra_cursors(&self) -> bool {
        !self.extra.is_empty()
    }

    /// Add a cursor, or remove the one already there (⌥-click twice).
    /// The last cursor left standing becomes the main one.
    pub fn toggle_cursor(&mut self, c: Cursor) {
        let head = c.head();
        if self.cursor.head() == head {
            // Removing the main one: another takes over.
            if let Some(next) = self.extra.pop() {
                self.cursor = next;
            }
            return;
        }
        match self.extra.iter().position(|e| e.head() == head) {
            Some(i) => {
                self.extra.remove(i);
            }
            None => self.extra.push(std::mem::replace(&mut self.cursor, c)),
        }
    }

    /// Replace the whole set; the last one given leads.
    pub fn set_cursors(&mut self, mut cursors: Vec<Cursor>) {
        // Two cursors in the same place are one cursor.
        cursors.dedup_by(|a, b| a.selection == b.selection);
        match cursors.pop() {
            Some(main) => {
                self.cursor = main;
                self.extra = cursors;
            }
            None => self.extra.clear(),
        }
    }

    /// Back to one cursor (Escape, or a plain click).
    pub fn clear_extra_cursors(&mut self) {
        self.extra.clear();
    }

    /// Do something at every cursor: `make` says what each one replaces
    /// and with what, in char offsets. The edits are applied from the end
    /// of the file backwards, so the offsets of the ones still to come
    /// stay valid, and the cursors end up after their own new text.
    fn edit_each(&mut self, mut make: impl FnMut(&Buffer, &Cursor) -> Option<(usize, usize, String)>) -> bool {
        let cursors = self.cursors();
        let mut edits: Vec<(usize, usize, String)> = cursors.iter().filter_map(|c| make(self, c)).collect();
        edits.sort_by_key(|(s, e, _)| (*s, *e));
        // Cursors whose edits overlap act as one.
        edits.dedup_by(|b, a| b.0 < a.1 || (b.0 == a.0 && b.1 == a.1));
        if edits.is_empty() {
            return false;
        }
        // One keystroke at many cursors is one undo step.
        let merge = edits.iter().all(|(s, e, t)| s == e && t.chars().count() == 1 && !t.chars().all(char::is_whitespace));
        self.history.begin_keystroke(merge);
        for (s, e, text) in edits.iter().rev() {
            self.apply(*s, e - s, text, true);
        }
        self.history.end_keystroke(merge);
        let mut shift: isize = 0;
        let mut cursors = Vec::with_capacity(edits.len());
        for (s, e, text) in &edits {
            let len = text.chars().count();
            let at = (*s as isize + shift) as usize + len;
            cursors.push(Cursor::at(self.char_to_pos(at)));
            shift += len as isize - (*e - *s) as isize;
        }
        self.set_cursors(cursors);
        true
    }

    /// Move every cursor the same way, with the single-cursor code.
    fn move_each(&mut self, mut step: impl FnMut(&mut Self)) {
        if self.extra.is_empty() {
            step(self);
            return;
        }
        let cursors = self.cursors();
        self.extra.clear();
        let mut moved = Vec::with_capacity(cursors.len());
        for c in cursors {
            self.cursor = c;
            step(self);
            moved.push(self.cursor);
        }
        moved.sort_by_key(|c| c.selection.ordered().0);
        self.set_cursors(moved);
    }

    /// Insert text at every cursor, replacing any selections.
    pub fn insert(&mut self, text: &str) {
        if self.extra.is_empty() {
            let (s, e) = self.cursor.selection.ordered();
            let start = self.pos_to_char(s);
            let end = self.pos_to_char(e);
            self.apply(start, end - start, text, true);
            return;
        }
        let text = text.to_string();
        self.edit_each(|buf, c| {
            let (s, e) = c.selection.ordered();
            Some((buf.pos_to_char(s), buf.pos_to_char(e), text.clone()))
        });
    }

    /// Paste at every cursor: as many lines as cursors gives each cursor
    /// its own line, as in VS Code; otherwise they all get the lot.
    pub fn insert_pasted(&mut self, text: &str) {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let count = self.extra.len() + 1;
        if count < 2 || lines.len() != count {
            self.insert(text);
            return;
        }
        let mut i = 0;
        let parts: Vec<String> = lines.iter().map(|l| l.trim_end_matches('\n').to_string()).collect();
        self.edit_each(move |buf, c| {
            let (s, e) = c.selection.ordered();
            let text = parts.get(i).cloned().unwrap_or_default();
            i += 1;
            Some((buf.pos_to_char(s), buf.pos_to_char(e), text))
        });
    }

    /// Insert a newline, copying the current line's leading whitespace.
    pub fn insert_newline(&mut self) {
        self.history.commit();
        if self.extra.is_empty() {
            let line = self.line(self.cursor.head().line);
            let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            self.insert(&format!("\n{indent}"));
        } else {
            // Each cursor keeps the indentation of its own line.
            self.edit_each(|buf, c| {
                let (s, e) = c.selection.ordered();
                let line = buf.line(s.line);
                let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                Some((buf.pos_to_char(s), buf.pos_to_char(e), format!("\n{indent}")))
            });
        }
        self.history.commit();
    }

    pub fn delete_selection(&mut self) -> bool {
        if !self.extra.is_empty() {
            if self.cursors().iter().all(|c| c.selection.is_empty()) {
                return false;
            }
            self.history.commit();
            return self.edit_each(|buf, c| {
                let (s, e) = c.selection.ordered();
                (s != e).then(|| (buf.pos_to_char(s), buf.pos_to_char(e), String::new()))
            });
        }
        if self.cursor.selection.is_empty() {
            return false;
        }
        let (s, e) = self.cursor.selection.ordered();
        let start = self.pos_to_char(s);
        let end = self.pos_to_char(e);
        self.history.commit();
        self.apply(start, end - start, "", true);
        true
    }

    pub fn backspace(&mut self) {
        if !self.extra.is_empty() {
            if self.delete_selection() {
                return;
            }
            self.history.commit();
            self.edit_each(|buf, c| {
                let at = buf.pos_to_char(c.head());
                (at > 0).then(|| (at - 1, at, String::new()))
            });
            return;
        }
        if self.delete_selection() {
            return;
        }
        let idx = self.pos_to_char(self.cursor.head());
        if idx == 0 {
            return;
        }
        self.history.commit();
        self.apply(idx - 1, 1, "", true);
    }

    pub fn delete_forward(&mut self) {
        if !self.extra.is_empty() {
            if self.delete_selection() {
                return;
            }
            self.history.commit();
            let len = self.rope.len_chars();
            self.edit_each(move |buf, c| {
                let at = buf.pos_to_char(c.head());
                (at < len).then(|| (at, at + 1, String::new()))
            });
            return;
        }
        if self.delete_selection() {
            return;
        }
        let idx = self.pos_to_char(self.cursor.head());
        if idx >= self.rope.len_chars() {
            return;
        }
        self.history.commit();
        self.apply(idx, 1, "", true);
    }

    pub fn replace_range(&mut self, start: Position, end: Position, text: &str) {
        let s = self.pos_to_char(start);
        let e = self.pos_to_char(end);
        self.history.commit();
        self.apply(s, e.saturating_sub(s), text, true);
        self.history.commit();
    }

    /// The selected text; with several cursors, one line per selection.
    pub fn selected_text(&self) -> String {
        if self.extra.is_empty() {
            let (s, e) = self.cursor.selection.ordered();
            return self.rope.slice(self.pos_to_char(s)..self.pos_to_char(e)).to_string();
        }
        self.cursors()
            .iter()
            .map(|c| {
                let (s, e) = c.selection.ordered();
                self.rope.slice(self.pos_to_char(s)..self.pos_to_char(e)).to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn select_all(&mut self) {
        self.cursor.selection.anchor = Position::new(0, 0);
        self.cursor.selection.head = self.char_to_pos(self.rope.len_chars());
    }

    /// Make the edits until `end_undo_group` a single undo step.
    pub fn begin_undo_group(&mut self) {
        self.history.begin_group();
    }

    pub fn end_undo_group(&mut self) {
        self.history.end_group();
    }

    pub fn undo(&mut self) -> bool {
        // The old cursors belong to the text as it was.
        self.extra.clear();
        let Some(edits) = self.history.pop_undo() else { return false };
        for e in edits {
            self.apply(e.start, e.old.chars().count(), &e.new, false);
            self.cursor = Cursor::at(e.cursor_after);
        }
        true
    }

    pub fn redo(&mut self) -> bool {
        // The old cursors belong to the text as it was.
        self.extra.clear();
        let Some(edits) = self.history.pop_redo() else { return false };
        for e in edits {
            self.apply(e.start, e.old.chars().count(), &e.new, false);
            self.cursor = Cursor::at(e.cursor_after);
        }
        true
    }

    // ---- cursor movement -------------------------------------------------

    pub fn move_left(&mut self, extend: bool) {
        self.move_each(|b| b.move_left_one(extend));
    }

    fn move_left_one(&mut self, extend: bool) {
        self.history.commit();
        let p = self.cursor.head();
        let np = if p.col > 0 {
            Position::new(p.line, p.col - 1)
        } else if p.line > 0 {
            Position::new(p.line - 1, self.line_len(p.line - 1))
        } else {
            p
        };
        self.cursor.set_head(np, extend);
        self.cursor.goal_col = None;
    }

    pub fn move_right(&mut self, extend: bool) {
        self.move_each(|b| b.move_right_one(extend));
    }

    fn move_right_one(&mut self, extend: bool) {
        self.history.commit();
        let p = self.cursor.head();
        let np = if p.col < self.line_len(p.line) {
            Position::new(p.line, p.col + 1)
        } else if p.line + 1 < self.line_count() {
            Position::new(p.line + 1, 0)
        } else {
            p
        };
        self.cursor.set_head(np, extend);
        self.cursor.goal_col = None;
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        self.move_each(|b| b.move_vertical_one(delta, extend));
    }

    fn move_vertical_one(&mut self, delta: isize, extend: bool) {
        self.history.commit();
        let p = self.cursor.head();
        let goal = self.cursor.goal_col.unwrap_or(p.col);
        let line = (p.line as isize + delta).clamp(0, self.line_count() as isize - 1) as usize;
        let np = Position::new(line, goal.min(self.line_len(line)));
        self.cursor.set_head(np, extend);
        self.cursor.goal_col = Some(goal);
    }

    /// Home: jump to first non-blank, or column 0 if already there.
    pub fn move_home(&mut self, extend: bool) {
        self.move_each(|b| b.move_home_one(extend));
    }

    fn move_home_one(&mut self, extend: bool) {
        self.history.commit();
        let p = self.cursor.head();
        let first_non_blank = self.line(p.line).chars().take_while(|c| c.is_whitespace()).count();
        let col = if p.col == first_non_blank { 0 } else { first_non_blank };
        self.cursor.set_head(Position::new(p.line, col), extend);
        self.cursor.goal_col = None;
    }

    pub fn move_end(&mut self, extend: bool) {
        self.move_each(|b| b.move_end_one(extend));
    }

    fn move_end_one(&mut self, extend: bool) {
        self.history.commit();
        let p = self.cursor.head();
        self.cursor.set_head(Position::new(p.line, self.line_len(p.line)), extend);
        self.cursor.goal_col = None;
    }

    pub fn move_word(&mut self, forward: bool, extend: bool) {
        self.move_each(|b| b.move_word_one(forward, extend));
    }

    fn move_word_one(&mut self, forward: bool, extend: bool) {
        self.history.commit();
        let mut idx = self.pos_to_char(self.cursor.head());
        let len = self.rope.len_chars();
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                1
            } else if c.is_whitespace() {
                0
            } else {
                2
            }
        };
        if forward {
            if idx < len {
                let start_class = class(self.rope.char(idx));
                while idx < len && class(self.rope.char(idx)) == start_class {
                    idx += 1;
                }
                while idx < len && class(self.rope.char(idx)) == 0 {
                    idx += 1;
                }
            }
        } else if idx > 0 {
            idx -= 1;
            while idx > 0 && class(self.rope.char(idx)) == 0 {
                idx -= 1;
            }
            let start_class = class(self.rope.char(idx));
            while idx > 0 && class(self.rope.char(idx - 1)) == start_class {
                idx -= 1;
            }
        }
        let np = self.char_to_pos(idx);
        self.cursor.set_head(np, extend);
        self.cursor.goal_col = None;
    }

    /// Comment the lines the cursors are on, or uncomment them when they
    /// are all commented already (⌘/).
    pub fn toggle_comment(&mut self) {
        let Some(language) = self.language.clone() else { return };
        let style = crate::comment::style_for(&language);
        // Every line touched by a cursor or its selection.
        let mut lines: Vec<usize> = Vec::new();
        for c in self.cursors() {
            let (s, e) = c.selection.ordered();
            // A selection ending at column 0 doesn't reach into that line.
            let last = if e.line > s.line && e.col == 0 { e.line - 1 } else { e.line };
            lines.extend(s.line..=last);
        }
        lines.sort_unstable();
        lines.dedup();
        let Some(&first) = lines.first() else { return };
        let Some(&last) = lines.last() else { return };
        let text: Vec<(usize, String)> = lines.iter().map(|l| (*l, self.line(*l))).collect();
        // Put the cursors back where they were, allowing for the markers.
        let before = self.cursors();
        self.history.commit();
        let action = crate::comment::toggle(style, &text);
        let shift: Box<dyn Fn(Position) -> Position> = match &action {
            crate::comment::Action::Comment { lines, col, marker } => {
                let (lines, col, len) = (lines.clone(), *col, marker.chars().count());
                Box::new(
                    move |p: Position| {
                        if lines.contains(&p.line) && p.col >= col {
                            Position::new(p.line, p.col + len)
                        } else {
                            p
                        }
                    },
                )
            }
            crate::comment::Action::Uncomment { at, len } => {
                let (at, len) = (at.clone(), *len);
                Box::new(move |p: Position| match at.iter().find(|(l, _)| *l == p.line) {
                    Some((_, col)) if p.col > *col => Position::new(p.line, p.col.saturating_sub(len).max(*col)),
                    _ => p,
                })
            }
            _ => Box::new(|p| p),
        };
        match action {
            crate::comment::Action::Nothing => {}
            crate::comment::Action::Comment { lines, col, marker } => {
                self.history.begin_group();
                for line in lines.iter().rev() {
                    let at = Position::new(*line, col.min(self.line_len(*line)));
                    let start = self.pos_to_char(at);
                    self.apply(start, 0, &marker, true);
                }
                self.history.end_group();
            }
            crate::comment::Action::Uncomment { at, len } => {
                self.history.begin_group();
                for (line, col) in at.iter().rev() {
                    let start = self.pos_to_char(Position::new(*line, *col));
                    let len = len.min(self.line_len(*line).saturating_sub(*col));
                    self.apply(start, len, "", true);
                }
                self.history.end_group();
            }
            crate::comment::Action::Wrap { open, close } => {
                let (s, e) = self.cursor.selection.ordered();
                let (s, e) = if s == e { (Position::new(first, 0), Position::new(last, self.line_len(last))) } else { (s, e) };
                let inside = {
                    let (a, b) = (self.pos_to_char(s), self.pos_to_char(e));
                    self.rope.slice(a..b).to_string()
                };
                let trimmed = inside.trim();
                self.history.begin_group();
                // Already wrapped: take the markers off again.
                let text = match trimmed.strip_prefix(open).and_then(|t| t.strip_suffix(close)) {
                    Some(bare) => bare.trim().to_string(),
                    None => format!("{open} {trimmed} {close}"),
                };
                let (a, b) = (self.pos_to_char(s), self.pos_to_char(e));
                self.apply(a, b - a, &text, true);
                self.history.end_group();
            }
        }
        self.history.commit();
        let kept: Vec<Cursor> = before
            .iter()
            .map(|c| {
                let mut moved = *c;
                moved.selection.anchor = self.clamp(shift(c.selection.anchor));
                moved.selection.head = self.clamp(shift(c.selection.head));
                moved.goal_col = None;
                moved
            })
            .collect();
        self.set_cursors(kept);
    }

    /// Put the one cursor here (any others are dropped).
    pub fn set_cursor(&mut self, p: Position, extend: bool) {
        self.extra.clear();
        self.history.commit();
        let p = self.clamp(p);
        self.cursor.set_head(p, extend);
        self.cursor.goal_col = None;
    }

    /// The identifier-ish word ending at the cursor, used as the completion
    /// prefix for filtering LSP results client-side.
    pub fn word_before_cursor(&self) -> String {
        let p = self.cursor.head();
        let line = self.line(p.line);
        line.chars()
            .take(p.col)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_undo() {
        let mut b = Buffer::from_text("hello", None);
        b.set_cursor(Position::new(0, 5), false);
        b.insert(" world");
        assert_eq!(b.text(), "hello world");
        assert!(b.undo());
        assert_eq!(b.text(), "hello");
        assert!(b.redo());
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn typing_merges_into_one_undo_step() {
        let mut b = Buffer::empty();
        for c in "abc".chars() {
            b.insert(&c.to_string());
        }
        assert_eq!(b.text(), "abc");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn newline_keeps_indent() {
        let mut b = Buffer::from_text("    x", None);
        b.set_cursor(Position::new(0, 5), false);
        b.insert_newline();
        assert_eq!(b.text(), "    x\n    ");
        assert_eq!(b.cursor.head(), Position::new(1, 4));
    }

    #[test]
    fn backspace_across_lines() {
        let mut b = Buffer::from_text("a\nb", None);
        b.set_cursor(Position::new(1, 0), false);
        b.backspace();
        assert_eq!(b.text(), "ab");
        assert_eq!(b.cursor.head(), Position::new(0, 1));
    }

    #[test]
    fn word_movement() {
        let mut b = Buffer::from_text("foo_bar baz", None);
        b.move_word(true, false);
        assert_eq!(b.cursor.head(), Position::new(0, 8));
        b.move_word(false, false);
        assert_eq!(b.cursor.head(), Position::new(0, 0));
    }

    #[test]
    fn changes_are_tracked_for_lsp() {
        let mut b = Buffer::from_text("ab", None);
        b.set_cursor(Position::new(0, 1), false);
        b.insert("X");
        let ch = b.take_changes();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].start, Position::new(0, 1));
        assert_eq!(ch[0].text, "X");
        assert!(b.take_changes().is_empty());
    }
}

#[cfg(test)]
mod multi_cursor_tests {
    use super::*;

    /// A buffer with a cursor at each given (line, col).
    fn with_cursors(text: &str, at: &[(usize, usize)]) -> Buffer {
        let mut b = Buffer::from_text(text, None);
        let cursors: Vec<Cursor> = at.iter().map(|&(l, c)| Cursor::at(Position::new(l, c))).collect();
        b.set_cursors(cursors);
        b
    }

    fn heads(b: &Buffer) -> Vec<(usize, usize)> {
        b.cursors().iter().map(|c| (c.head().line, c.head().col)).collect()
    }

    #[test]
    fn typing_happens_at_every_cursor() {
        let mut b = with_cursors("a\nb\nc\n", &[(0, 1), (1, 1), (2, 1)]);
        b.insert("!");
        assert_eq!(b.text(), "a!\nb!\nc!\n");
        assert_eq!(heads(&b), vec![(0, 2), (1, 2), (2, 2)]);
        // One undo takes back the whole keystroke.
        assert!(b.undo());
        assert_eq!(b.text(), "a\nb\nc\n");
    }

    #[test]
    fn a_word_typed_at_several_cursors_undoes_in_one_go() {
        let mut b = with_cursors("a\nb\n", &[(0, 1), (1, 1)]);
        for c in "xyz".chars() {
            b.insert(&c.to_string());
        }
        assert_eq!(b.text(), "axyz\nbxyz\n");
        assert!(b.undo());
        assert_eq!(b.text(), "a\nb\n");
        assert!(b.redo());
        assert_eq!(b.text(), "axyz\nbxyz\n");
    }

    #[test]
    fn replacing_selections_then_typing_undoes_cleanly() {
        let mut b = Buffer::from_text("foo 1\nfoo 2\n", None);
        let sel = |l: usize| {
            let mut c = Cursor::at(Position::new(l, 0));
            c.set_head(Position::new(l, 3), true);
            c
        };
        b.set_cursors(vec![sel(0), sel(1)]);
        for c in "bar".chars() {
            b.insert(&c.to_string());
        }
        assert_eq!(b.text(), "bar 1\nbar 2\n");
        // The typing, then the replacement: two steps, nothing half-done.
        assert!(b.undo());
        assert!(b.undo());
        assert_eq!(b.text(), "foo 1\nfoo 2\n");
    }

    #[test]
    fn backspace_and_delete_at_every_cursor() {
        let mut b = with_cursors("one\ntwo\n", &[(0, 3), (1, 3)]);
        b.backspace();
        assert_eq!(b.text(), "on\ntw\n");
        b.delete_forward();
        assert_eq!(b.text(), "ontw");
        // At the very start there is nothing to delete.
        let mut b = with_cursors("ab\n", &[(0, 0)]);
        b.backspace();
        assert_eq!(b.text(), "ab\n");
    }

    #[test]
    fn selections_are_replaced_at_every_cursor() {
        let mut b = Buffer::from_text("foo = 1\nfoo = 2\n", None);
        let sel = |l: usize| {
            let mut c = Cursor::at(Position::new(l, 0));
            c.set_head(Position::new(l, 3), true);
            c
        };
        b.set_cursors(vec![sel(0), sel(1)]);
        assert_eq!(b.selected_text(), "foo\nfoo");
        b.insert("bar");
        assert_eq!(b.text(), "bar = 1\nbar = 2\n");
    }

    #[test]
    fn newline_keeps_each_line_indentation() {
        let mut b = with_cursors("    a\n  b\n", &[(0, 5), (1, 3)]);
        b.insert_newline();
        assert_eq!(b.text(), "    a\n    \n  b\n  \n");
    }

    #[test]
    fn moving_moves_every_cursor_and_merges_them() {
        let mut b = with_cursors("abc\ndef\n", &[(0, 1), (1, 1)]);
        b.move_right(false);
        assert_eq!(heads(&b), vec![(0, 2), (1, 2)]);
        b.move_home(false);
        assert_eq!(heads(&b), vec![(0, 0), (1, 0)]);
        // Both cursors on one line collapse into one.
        let mut b = with_cursors("abc\n", &[(0, 0), (0, 1)]);
        b.move_end(false);
        assert_eq!(heads(&b), vec![(0, 3)]);
        assert!(!b.has_extra_cursors());
    }

    #[test]
    fn adding_and_removing_cursors() {
        let mut b = Buffer::from_text("abc\n", None);
        assert!(!b.has_extra_cursors());
        b.toggle_cursor(Cursor::at(Position::new(0, 2)));
        assert_eq!(heads(&b), vec![(0, 0), (0, 2)]);
        // ⌥-clicking the same spot again takes it away.
        b.toggle_cursor(Cursor::at(Position::new(0, 2)));
        assert_eq!(heads(&b), vec![(0, 0)]);
        b.toggle_cursor(Cursor::at(Position::new(0, 3)));
        b.clear_extra_cursors();
        assert_eq!(b.cursors().len(), 1);
    }

    #[test]
    fn paste_spreads_matching_lines_over_the_cursors() {
        let mut b = with_cursors("a: \nb: \n", &[(0, 3), (1, 3)]);
        b.insert_pasted("one\ntwo\n");
        assert_eq!(b.text(), "a: one\nb: two\n");
        // A different number of lines goes in whole at each cursor.
        let mut b = with_cursors("a\nb\n", &[(0, 1), (1, 1)]);
        b.insert_pasted("x\ny\nz\n");
        assert_eq!(b.text(), "ax\ny\nz\n\nbx\ny\nz\n\n");
    }

    #[test]
    fn overlapping_cursors_edit_once() {
        // Two cursors next to each other both deleting the same character.
        let b = with_cursors("abc\n", &[(0, 1), (0, 1)]);
        assert_eq!(b.cursors().len(), 1);
        let mut b = with_cursors("abcd\n", &[(0, 2), (0, 3)]);
        b.backspace();
        assert_eq!(b.text(), "ad\n");
    }
}

#[cfg(test)]
mod comment_tests {
    use super::*;

    fn tf(text: &str) -> Buffer {
        Buffer::from_text(text, Some(PathBuf::from("main.tf")))
    }

    #[test]
    fn comments_and_uncomments_the_cursor_line() {
        let mut b = tf("a = 1\nb = 2\n");
        b.toggle_comment();
        assert_eq!(b.text(), "# a = 1\nb = 2\n");
        b.toggle_comment();
        assert_eq!(b.text(), "a = 1\nb = 2\n");
        // And it is one undo step either way.
        assert!(b.undo());
        assert_eq!(b.text(), "# a = 1\nb = 2\n");
    }

    #[test]
    fn comments_every_line_of_a_selection() {
        let mut b = tf("locals {\n  a = 1\n  b = 2\n}\n");
        b.set_cursor(Position::new(1, 0), false);
        b.set_cursor(Position::new(2, 5), true);
        b.toggle_comment();
        assert_eq!(b.text(), "locals {\n  # a = 1\n  # b = 2\n}\n");
        b.toggle_comment();
        assert_eq!(b.text(), "locals {\n  a = 1\n  b = 2\n}\n");
    }

    #[test]
    fn comments_the_lines_of_every_cursor() {
        let mut b = tf("a\nb\nc\n");
        b.set_cursors(vec![Cursor::at(Position::new(0, 0)), Cursor::at(Position::new(2, 0))]);
        b.toggle_comment();
        assert_eq!(b.text(), "# a\nb\n# c\n");
        // Both cursors are still there, moved along by the markers.
        assert_eq!(b.cursors().len(), 2);
        b.toggle_comment();
        assert_eq!(b.text(), "a\nb\nc\n");
    }

    #[test]
    fn a_language_without_comments_is_left_alone() {
        let mut b = Buffer::from_text("x\n", Some(PathBuf::from("notes.txt")));
        b.toggle_comment();
        assert_eq!(b.text(), "x\n");
    }
}
