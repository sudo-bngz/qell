//! Positions, cursors and selections.
//!
//! All positions are `(line, column)` where the column counts **characters**
//! (Unicode scalar values), not bytes and not grapheme clusters. That is the
//! unit `ropey` works in and the one that keeps cursor arithmetic simple;
//! conversion to LSP's UTF-16 columns happens at the LSP boundary.

use serde::{Deserialize, Serialize};

/// A location in a buffer: zero-based line and zero-based character column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

impl Position {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// A range of text between two positions. `anchor` is where the selection
/// started, `head` is where the cursor currently is; `head` may be before
/// `anchor` when the user selected backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: Position,
    pub head: Position,
}

impl Selection {
    pub fn point(p: Position) -> Self {
        Self { anchor: p, head: p }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` in document order.
    pub fn ordered(&self) -> (Position, Position) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// The user's cursor. `goal_col` remembers the column the user *wants* to be
/// on while moving vertically through shorter lines, like every serious editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub selection: Selection,
    pub goal_col: Option<usize>,
}

impl Cursor {
    pub fn at(p: Position) -> Self {
        Self { selection: Selection::point(p), goal_col: None }
    }

    pub fn head(&self) -> Position {
        self.selection.head
    }

    pub fn set_head(&mut self, p: Position, extend: bool) {
        self.selection.head = p;
        if !extend {
            self.selection.anchor = p;
        }
    }
}
