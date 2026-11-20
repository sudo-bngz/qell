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

    /// Insert text at the cursor, replacing the selection if there is one.
    pub fn insert(&mut self, text: &str) {
        let (s, e) = self.cursor.selection.ordered();
        let start = self.pos_to_char(s);
        let end = self.pos_to_char(e);
        self.apply(start, end - start, text, true);
    }

    /// Insert a newline, copying the current line's leading whitespace.
    pub fn insert_newline(&mut self) {
        let line = self.line(self.cursor.head().line);
        let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        self.history.commit();
        self.insert(&format!("\n{indent}"));
        self.history.commit();
    }

    pub fn delete_selection(&mut self) -> bool {
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

    pub fn selected_text(&self) -> String {
        let (s, e) = self.cursor.selection.ordered();
        self.rope.slice(self.pos_to_char(s)..self.pos_to_char(e)).to_string()
    }

    pub fn select_all(&mut self) {
        self.cursor.selection.anchor = Position::new(0, 0);
        self.cursor.selection.head = self.char_to_pos(self.rope.len_chars());
    }

    pub fn undo(&mut self) -> bool {
        let Some(edits) = self.history.pop_undo() else { return false };
        for e in edits {
            self.apply(e.start, e.old.chars().count(), &e.new, false);
            self.cursor = Cursor::at(e.cursor_after);
        }
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edits) = self.history.pop_redo() else { return false };
        for e in edits {
            self.apply(e.start, e.old.chars().count(), &e.new, false);
            self.cursor = Cursor::at(e.cursor_after);
        }
        true
    }

    // ---- cursor movement -------------------------------------------------

    pub fn move_left(&mut self, extend: bool) {
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
        self.history.commit();
        let p = self.cursor.head();
        let first_non_blank = self.line(p.line).chars().take_while(|c| c.is_whitespace()).count();
        let col = if p.col == first_non_blank { 0 } else { first_non_blank };
        self.cursor.set_head(Position::new(p.line, col), extend);
        self.cursor.goal_col = None;
    }

    pub fn move_end(&mut self, extend: bool) {
        self.history.commit();
        let p = self.cursor.head();
        self.cursor.set_head(Position::new(p.line, self.line_len(p.line)), extend);
        self.cursor.goal_col = None;
    }

    pub fn move_word(&mut self, forward: bool, extend: bool) {
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

    pub fn set_cursor(&mut self, p: Position, extend: bool) {
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
