//! Resolving merge conflicts in the editor: accept the current side, the
//! incoming one or both, for one block or all of them, and move between
//! blocks. The editor draws the blocks and puts the same actions on each
//! `<<<<<<<` line.

use qell_core::conflict::Choice;
use qell_core::Position;

use crate::app::QellApp;

impl QellApp {
    /// Resolve conflict `i` of the active tab.
    pub fn resolve_conflict(&mut self, i: usize, choice: Choice) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if doc.read_only {
            return self.notify("This is a read-only view.");
        }
        let left = doc.resolve_conflict(i, choice);
        self.after_resolving(left);
    }

    /// The conflict around the cursor.
    pub fn resolve_conflict_at_cursor(&mut self, choice: Choice) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let line = doc.buffer.cursor.head().line;
        let found = doc.conflicts().iter().position(|c| (c.start..=c.end).contains(&line));
        match found {
            Some(i) => self.resolve_conflict(i, choice),
            None if doc.conflicts().is_empty() => self.notify("No merge conflicts in this file."),
            None => self.notify("Put the cursor inside a conflict (between <<<<<<< and >>>>>>>), or use Merge: Next Conflict."),
        }
    }

    /// Every conflict of the file, the same way (bottom up, so the line
    /// numbers of the others don't move).
    pub fn resolve_all_conflicts(&mut self, choice: Choice) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if doc.read_only {
            return self.notify("This is a read-only view.");
        }
        let n = doc.conflicts().len();
        if n == 0 {
            return self.notify("No merge conflicts in this file.");
        }
        let mut left = n;
        for i in (0..n).rev() {
            left = doc.resolve_conflict(i, choice);
        }
        self.after_resolving(left);
    }

    fn after_resolving(&mut self, left: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let name = doc.buffer.file_name();
        if left == 0 {
            self.notify(format!("No conflicts left in {name}. Save it, then stage it in Source Control to mark it resolved."));
        } else {
            self.notify(format!("{left} conflict{} left in {name}.", if left == 1 { "" } else { "s" }));
        }
    }

    /// Move to the next (or previous) conflict, round the end of the file.
    pub fn go_to_conflict(&mut self, forward: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let line = doc.buffer.cursor.head().line;
        let starts: Vec<usize> = doc.conflicts().iter().map(|c| c.start).collect();
        if starts.is_empty() {
            return self.notify("No merge conflicts in this file.");
        }
        let target = if forward {
            starts.iter().find(|&&s| s > line).or(starts.first())
        } else {
            starts.iter().rev().find(|&&s| s < line).or(starts.last())
        };
        if let Some(&start) = target {
            doc.buffer.set_cursor(Position::new(start, 0), false);
            doc.scroll_to_cursor = true;
        }
    }
}
