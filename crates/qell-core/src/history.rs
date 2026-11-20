//! Undo / redo.
//!
//! Every edit is recorded as an [`Edit`] (a char range that was replaced,
//! with the old and new text). Consecutive single-character insertions are
//! merged into one undo step so that undo feels like "undo the word", not
//! "undo one keystroke", matching VSCode's behaviour.

use crate::Position;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Char offset where the edit started.
    pub start: usize,
    /// Text that was there before.
    pub old: String,
    /// Text that is there now.
    pub new: String,
    /// Cursor position before the edit, restored on undo.
    pub cursor_before: Position,
    /// Cursor position after the edit, restored on redo.
    pub cursor_after: Position,
}

impl Edit {
    pub fn inverse(&self) -> Edit {
        Edit {
            start: self.start,
            old: self.new.clone(),
            new: self.old.clone(),
            cursor_before: self.cursor_after,
            cursor_after: self.cursor_before,
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Vec<Edit>>,
    redo: Vec<Vec<Edit>>,
    /// When true the next recorded edit starts a new group.
    boundary: bool,
    /// An explicit group is open (Vim's insert sessions, `:s`): `Some(false)`
    /// until its first edit, then `Some(true)`; every edit joins it.
    grouping: Option<bool>,
}

impl History {
    pub fn new() -> Self {
        Self { boundary: true, ..Default::default() }
    }

    /// Force the next edit into a fresh undo group (called on cursor moves,
    /// whitespace, pastes, etc.).
    pub fn commit(&mut self) {
        if self.grouping.is_none() {
            self.boundary = true;
        }
    }

    /// Start a group that every following edit joins, until `end_group`:
    /// one undo step for the lot.
    pub fn begin_group(&mut self) {
        if self.grouping.is_none() {
            self.grouping = Some(false);
        }
    }

    pub fn end_group(&mut self) {
        self.grouping = None;
        self.boundary = true;
    }

    pub fn record(&mut self, edit: Edit) {
        self.redo.clear();
        match self.grouping {
            Some(true) => {
                if let Some(group) = self.undo.last_mut() {
                    group.push(edit);
                    return;
                }
            }
            Some(false) => {
                self.grouping = Some(true);
                self.undo.push(vec![edit]);
                return;
            }
            None => {}
        }
        let mergeable =
            !self.boundary && edit.old.is_empty() && edit.new.chars().count() == 1 && !edit.new.chars().all(char::is_whitespace);
        match self.undo.last_mut() {
            Some(group) if mergeable => group.push(edit),
            _ => self.undo.push(vec![edit]),
        }
        self.boundary = false;
    }

    /// Pops the last undo group; the caller applies each returned edit in
    /// order and nothing else. Returns `None` when there is nothing to undo.
    pub fn pop_undo(&mut self) -> Option<Vec<Edit>> {
        let group = self.undo.pop()?;
        let inverse: Vec<Edit> = group.iter().rev().map(Edit::inverse).collect();
        self.redo.push(group);
        self.boundary = true;
        Some(inverse)
    }

    pub fn pop_redo(&mut self) -> Option<Vec<Edit>> {
        let group = self.redo.pop()?;
        self.undo.push(group.clone());
        self.boundary = true;
        Some(group)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
