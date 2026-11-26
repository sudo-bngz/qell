//! Word wrap and folding: how text lines map to the rows drawn on screen.
//!
//! Without wrapping or folds, row `n` is line `n`. With wrapping, a long
//! line is split into several rows at word boundaries; continuation rows
//! are indented like the line itself (VS Code's `wrappingIndent: "same"`).
//! Lines inside a folded region have no rows at all.

use crate::Position;

/// One row on screen: the chars `start..end` of `line`, drawn `indent`
/// columns in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    pub line: usize,
    pub start: usize,
    /// Exclusive. `usize::MAX` without wrapping (the rest of the line).
    pub end: usize,
    pub indent: usize,
}

/// Rows of a whole text.
#[derive(Debug, Clone, Default)]
pub struct WrapLayout {
    /// Columns per row; 0 when not wrapping.
    width: usize,
    /// Rows are listed (wrapping or folds); otherwise row `n` is line `n`.
    explicit: bool,
    lines: usize,
    rows: Vec<VisualRow>,
    /// Index of the first row of each line, plus one past the last.
    first: Vec<usize>,
}

/// Narrowest wrap width, so a tiny window doesn't make one row per char.
pub const MIN_WRAP_WIDTH: usize = 20;

impl WrapLayout {
    /// One row per line.
    pub fn plain(lines: usize) -> Self {
        Self { width: 0, explicit: false, lines: lines.max(1), rows: Vec::new(), first: Vec::new() }
    }

    /// Wrap `lines` (without their line breaks) to `width` columns.
    pub fn wrapped<S: AsRef<str>>(lines: impl Iterator<Item = S>, width: usize) -> Self {
        Self::layout(lines, width.max(MIN_WRAP_WIDTH), |_| false)
    }

    /// Lay `lines` out: wrapped to `width` columns (0: not wrapped), with
    /// the lines for which `hidden` is true given no row (they are folded
    /// away under the line before them).
    pub fn layout<S: AsRef<str>>(lines: impl Iterator<Item = S>, width: usize, hidden: impl Fn(usize) -> bool) -> Self {
        let width = if width == 0 { 0 } else { width.max(MIN_WRAP_WIDTH) };
        let mut rows = Vec::new();
        let mut first = Vec::new();
        for (line, text) in lines.enumerate() {
            first.push(rows.len());
            if line > 0 && hidden(line) {
                continue;
            }
            if width == 0 {
                rows.push(VisualRow { line, start: 0, end: usize::MAX, indent: 0 });
                continue;
            }
            let chars: Vec<char> = text.as_ref().trim_end_matches(['\n', '\r']).chars().collect();
            for (start, end, indent) in wrap_line(&chars, width) {
                rows.push(VisualRow { line, start, end, indent });
            }
        }
        if first.is_empty() {
            first.push(0);
            rows.push(VisualRow { line: 0, start: 0, end: 0, indent: 0 });
        }
        let lines = first.len();
        first.push(rows.len());
        Self { width, explicit: true, lines, rows, first }
    }

    pub fn is_wrapped(&self) -> bool {
        self.width > 0
    }

    /// Whether rows and lines differ (wrapping or folds), so moving up and
    /// down goes by rows.
    pub fn by_rows(&self) -> bool {
        self.explicit
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn row_count(&self) -> usize {
        if self.explicit {
            self.rows.len()
        } else {
            self.lines
        }
    }

    /// Row `i` (clamped to the last one).
    pub fn row(&self, i: usize) -> VisualRow {
        if self.explicit {
            self.rows[i.min(self.rows.len() - 1)]
        } else {
            VisualRow { line: i.min(self.lines - 1), start: 0, end: usize::MAX, indent: 0 }
        }
    }

    /// The rows of one line (none for a folded-away line).
    pub fn rows_of_line(&self, line: usize) -> std::ops::Range<usize> {
        if self.explicit {
            let l = line.min(self.lines - 1);
            self.first[l]..self.first[l + 1]
        } else {
            line..line + 1
        }
    }

    /// The row a position is drawn on. A column at a wrap point belongs to
    /// the row it starts.
    /// A folded-away line is on the row of the line it is folded under.
    pub fn row_of(&self, p: Position) -> usize {
        let rows = self.rows_of_line(p.line);
        if !self.explicit {
            return rows.start.min(self.lines - 1);
        }
        let (s, e) = (rows.start, rows.end);
        if s == e {
            return s.saturating_sub(1).min(self.rows.len() - 1);
        }
        s + self.rows[s..e].iter().rposition(|r| r.start <= p.col).unwrap_or(0)
    }

    /// Screen column of `col` (a char column of the row's line).
    pub fn screen_col(&self, row: usize, col: usize) -> usize {
        let r = self.row(row);
        r.indent + col.saturating_sub(r.start)
    }

    /// Text position at screen column `x` of `row`. Past the end of a
    /// wrapped row, the cursor stops before its last char (its end is the
    /// next row's start).
    pub fn position_at(&self, row: usize, x: usize, line_len: impl Fn(usize) -> usize) -> Position {
        let r = self.row(row);
        let len = line_len(r.line);
        let last_of_line = r.end >= len;
        let max = if last_of_line { len } else { r.end.saturating_sub(1).max(r.start) };
        Position::new(r.line, (r.start + x.saturating_sub(r.indent)).min(max))
    }
}

/// Split one line into `(start, end, indent)` rows of at most `width`
/// columns, breaking after whitespace when possible.
pub fn wrap_line(chars: &[char], width: usize) -> Vec<(usize, usize, usize)> {
    let n = chars.len();
    let width = width.max(1);
    if n <= width {
        return vec![(0, n, 0)];
    }
    let lead = chars.iter().take_while(|c| **c == ' ' || **c == '\t').count();
    // Continuation rows line up with the text, unless that leaves too little room.
    let cont = if lead <= width / 2 { lead } else { 0 };
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let indent = if start == 0 { 0 } else { cont };
        let room = (width - indent).max(1);
        if n - start <= room {
            out.push((start, n, indent));
            break;
        }
        let limit = start + room;
        // Don't break inside the leading indentation.
        let min = if start == 0 { lead + 1 } else { start + 1 };
        let brk = (min..limit).rev().find(|&i| chars[i].is_whitespace()).map_or(limit, |i| i + 1);
        out.push((start, brk, indent));
        start = brk;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(s: &str, w: usize) -> Vec<(String, usize)> {
        let chars: Vec<char> = s.chars().collect();
        wrap_line(&chars, w).into_iter().map(|(a, b, i)| (chars[a..b].iter().collect(), i)).collect()
    }

    #[test]
    fn breaks_after_spaces() {
        assert_eq!(
            rows("the quick brown fox jumps", 10),
            vec![("the quick ".into(), 0), ("brown fox ".into(), 0), ("jumps".into(), 0)]
        );
    }

    #[test]
    fn long_words_are_cut() {
        assert_eq!(rows("abcdefghijkl", 5), vec![("abcde".into(), 0), ("fghij".into(), 0), ("kl".into(), 0)]);
    }

    #[test]
    fn continuation_keeps_the_indent() {
        assert_eq!(
            rows("    let a = b + c + d;", 12),
            vec![("    let a = ".into(), 0), ("b + c + ".into(), 4), ("d;".into(), 4)]
        );
    }

    #[test]
    fn short_lines_are_one_row() {
        assert_eq!(rows("", 10), vec![("".into(), 0)]);
        assert_eq!(rows("exactly10!", 10), vec![("exactly10!".into(), 0)]);
    }

    #[test]
    fn layout_maps_positions_and_rows() {
        let text = ["short", "aaaa bbbb cccc dddd eeee ffff gggg", "x"];
        let w = WrapLayout::wrapped(text.iter(), 20);
        // "aaaa bbbb cccc dddd " (20) + "eeee ffff gggg"
        assert_eq!(w.row_count(), 4);
        assert_eq!(w.rows_of_line(1), 1..3);
        assert_eq!(w.row_of(Position::new(1, 3)), 1);
        assert_eq!(w.row_of(Position::new(1, 20)), 2); // wrap point: next row
        assert_eq!(w.row_of(Position::new(1, 34)), 2); // end of line
        assert_eq!(w.row_of(Position::new(2, 0)), 3);
        assert_eq!(w.screen_col(2, 22), 2);
        let len = |l: usize| text[l].chars().count();
        // Clicking past the end of row 1 stays on it.
        assert_eq!(w.position_at(1, 50, len), Position::new(1, 19));
        assert_eq!(w.position_at(2, 50, len), Position::new(1, 34));
        assert_eq!(w.position_at(2, 3, len), Position::new(1, 23));
    }

    #[test]
    fn folded_lines_have_no_rows() {
        let text = ["fn a() {", "  one", "  two", "}", "fn b() {}"];
        let w = WrapLayout::layout(text.iter(), 0, |l| l == 1 || l == 2);
        assert!(w.by_rows() && !w.is_wrapped());
        assert_eq!(w.row_count(), 3);
        assert_eq!((w.row(0).line, w.row(1).line, w.row(2).line), (0, 3, 4));
        assert!(w.rows_of_line(1).is_empty());
        // A position inside the fold is drawn on the header's row.
        assert_eq!(w.row_of(Position::new(2, 1)), 0);
        assert_eq!(w.row_of(Position::new(3, 0)), 1);
        // Down from the header lands after the fold.
        assert_eq!(w.position_at(1, 0, |l| text[l].len()), Position::new(3, 0));
    }

    #[test]
    fn plain_layout_is_one_row_per_line() {
        let w = WrapLayout::plain(3);
        assert_eq!(w.row_count(), 3);
        assert_eq!(w.row_of(Position::new(2, 99)), 2);
        assert_eq!(w.row(1).line, 1);
        assert_eq!(w.position_at(1, 7, |_| 4), Position::new(1, 4));
    }
}
