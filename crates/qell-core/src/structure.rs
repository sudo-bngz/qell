//! Code structure the editor draws: bracket pairs (nesting depth, partner,
//! unmatched brackets) and indentation guides.
//!
//! Both are computed from plain text plus the highlight spans, so they work
//! for every language Qell highlights, with no language server.

use crate::{HighlightSpan, TokenKind};

/// One bracket in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bracket {
    /// Byte offset in the whole text.
    pub byte: usize,
    pub line: usize,
    /// Char column.
    pub col: usize,
    pub ch: char,
    pub open: bool,
    /// Nesting depth, from 0. Both brackets of a pair share it.
    pub depth: usize,
    /// Index of the matching bracket in [`Brackets::list`]; `None` when it
    /// is unmatched (shown in red).
    pub partner: Option<usize>,
}

/// Every bracket of a text, in order.
#[derive(Debug, Clone, Default)]
pub struct Brackets {
    pub list: Vec<Bracket>,
}

fn closer_of(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

impl Brackets {
    /// Find the brackets of `text`, skipping those inside strings and
    /// comments (per `spans`, sorted by start).
    pub fn compute(text: &str, spans: &[HighlightSpan]) -> Self {
        let mut list: Vec<Bracket> = Vec::new();
        // Indices of open brackets waiting for their closer.
        let mut stack: Vec<usize> = Vec::new();
        let mut span_i = 0;
        let (mut line, mut col) = (0usize, 0usize);
        for (byte, ch) in text.char_indices() {
            if ch == '\n' {
                line += 1;
                col = 0;
                continue;
            }
            let this_col = col;
            col += 1;
            if !matches!(ch, '(' | ')' | '[' | ']' | '{' | '}') {
                continue;
            }
            while span_i < spans.len() && spans[span_i].end <= byte {
                span_i += 1;
            }
            let in_literal = spans[span_i..]
                .iter()
                .take_while(|s| s.start <= byte)
                .any(|s| byte < s.end && matches!(s.kind, TokenKind::String | TokenKind::Comment));
            if in_literal {
                continue;
            }
            let open = matches!(ch, '(' | '[' | '{');
            let idx = list.len();
            let mut b = Bracket { byte, line, col: this_col, ch, open, depth: stack.len(), partner: None };
            if open {
                stack.push(idx);
            } else if let Some(pos) = stack.iter().rposition(|&o| closer_of(list[o].ch) == ch) {
                // Openers above the match were never closed: they stay
                // unmatched, and the pair resumes at the right depth.
                let o = stack[pos];
                stack.truncate(pos);
                list[o].partner = Some(idx);
                b.partner = Some(o);
                b.depth = list[o].depth;
            } else {
                b.depth = stack.len();
            }
            list.push(b);
        }
        Self { list }
    }

    /// Brackets on one line.
    pub fn on_line(&self, line: usize) -> &[Bracket] {
        let s = self.list.partition_point(|b| b.line < line);
        let e = s + self.list[s..].partition_point(|b| b.line <= line);
        &self.list[s..e]
    }

    /// The pair touching the cursor: the bracket right of it, else the one
    /// left of it (like VS Code). `None` for an unmatched bracket.
    pub fn pair_at(&self, line: usize, col: usize) -> Option<(Bracket, Bracket)> {
        let on = self.on_line(line);
        let b = on.iter().find(|b| b.col == col).or_else(|| on.iter().find(|b| col > 0 && b.col == col - 1))?;
        let p = self.list[b.partner?];
        Some(if b.open { (*b, p) } else { (p, *b) })
    }

    /// The innermost pair around `byte` (an open bracket before it whose
    /// closer is at or after it).
    pub fn enclosing(&self, byte: usize) -> Option<(Bracket, Bracket)> {
        let mut i = self.list.partition_point(|b| b.byte < byte);
        while i > 0 {
            i -= 1;
            let b = self.list[i];
            match (b.open, b.partner) {
                (true, Some(p)) if self.list[p].byte >= byte => return Some((b, self.list[p])),
                // A closed pair before `byte`: skip over it.
                (false, Some(p)) => i = p,
                _ => {}
            }
        }
        None
    }
}

// ---- indentation ------------------------------------------------------------

/// Leading whitespace of a line in columns (a tab counts as one, as the
/// editor draws it); `None` for a blank line.
pub fn indent_of(line: &str) -> Option<usize> {
    let n = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    (line.chars().nth(n).is_some_and(|c| c != '\n' && c != '\r')).then_some(n)
}

/// The file's indentation step: 1 for tabs, otherwise the most common
/// increase between consecutive lines (2, 4…), `fallback` when unclear.
pub fn detect_indent_unit<'a>(lines: impl Iterator<Item = &'a str>, fallback: usize) -> usize {
    let mut counts = [0usize; 9];
    let (mut tabs, mut spaces) = (0usize, 0usize);
    let mut prev = 0usize;
    for line in lines.take(5000) {
        let Some(ind) = indent_of(line) else { continue };
        if line.starts_with('\t') {
            tabs += 1;
        } else if ind > 0 {
            spaces += 1;
        }
        if ind > prev && ind - prev <= 8 {
            counts[ind - prev] += 1;
        }
        prev = ind;
    }
    if tabs > spaces {
        return 1;
    }
    match (2..=8).max_by_key(|&n| (counts[n], std::cmp::Reverse(n))) {
        Some(n) if counts[n] > 0 => n,
        _ => fallback.max(1),
    }
}

/// How far blank lines and blocks are looked through, so a huge file
/// can't stall a frame.
const SCAN_LIMIT: usize = 3000;

/// Indentation of a line for guides: blank lines take the smaller indent of
/// the surrounding non-blank lines, so guides run through them.
pub fn guide_indent(indent: &dyn Fn(usize) -> Option<usize>, line_count: usize, row: usize) -> usize {
    if let Some(i) = indent(row) {
        return i;
    }
    let up = (row.saturating_sub(SCAN_LIMIT)..row).rev().find_map(indent);
    let down = (row + 1..line_count.min(row + SCAN_LIMIT)).find_map(indent);
    match (up, down) {
        (Some(a), Some(b)) => a.min(b),
        _ => 0,
    }
}

/// The guide of the block the cursor is in: `(column, first row, last
/// row)`. On a line that opens a block (the next line is indented more),
/// that block's guide.
pub fn active_guide(
    indent: &dyn Fn(usize) -> Option<usize>,
    line_count: usize,
    row: usize,
    unit: usize,
) -> Option<(usize, usize, usize)> {
    let unit = unit.max(1);
    let gi = |r: usize| guide_indent(indent, line_count, r);
    let here = gi(row);
    let next = (row + 1..line_count.min(row + SCAN_LIMIT)).find_map(indent);
    let (col, mut start) = match next {
        Some(n) if indent(row).is_some() && n > here => (here, row + 1),
        _ if here == 0 => return None,
        _ => ((here - 1) / unit * unit, row),
    };
    let inside = |r: usize| gi(r) > col;
    while start > 0 && row.saturating_sub(start) < SCAN_LIMIT && inside(start - 1) {
        start -= 1;
    }
    let mut end = start;
    while end + 1 < line_count && end - row.min(end) < SCAN_LIMIT && inside(end + 1) {
        end += 1;
    }
    // Blank lines closing the block don't belong to it.
    while end > start && indent(end).is_none() {
        end -= 1;
    }
    Some((col, start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brackets(text: &str) -> Vec<(char, usize, Option<usize>)> {
        Brackets::compute(text, &[]).list.iter().map(|b| (b.ch, b.depth, b.partner)).collect()
    }

    #[test]
    fn pairs_and_depths() {
        assert_eq!(
            brackets("f(a[1], {x})"),
            vec![
                ('(', 0, Some(5)),
                ('[', 1, Some(2)),
                (']', 1, Some(1)),
                ('{', 1, Some(4)),
                ('}', 1, Some(3)),
                (')', 0, Some(0)),
            ]
        );
    }

    #[test]
    fn unmatched_brackets_have_no_partner() {
        // A stray closer, and an opener never closed inside a pair.
        let b = brackets(") { ( }");
        assert_eq!(b[0], (')', 0, None));
        assert_eq!(b[1], ('{', 0, Some(3)));
        assert_eq!(b[2], ('(', 1, None));
        assert_eq!(b[3], ('}', 0, Some(1)));
    }

    #[test]
    fn strings_and_comments_are_skipped() {
        let text = "f(\")\") // (\n";
        let spans = vec![
            HighlightSpan { start: 2, end: 5, kind: TokenKind::String },
            HighlightSpan { start: 7, end: 12, kind: TokenKind::Comment },
        ];
        let b = Brackets::compute(text, &spans);
        assert_eq!(b.list.len(), 2);
        assert_eq!(b.list[1].byte, 5);
        assert_eq!(b.list[0].partner, Some(1));
    }

    #[test]
    fn pair_at_cursor_and_enclosing() {
        let text = "fn a() {\n    b(1);\n}\n";
        let b = Brackets::compute(text, &[]);
        // Cursor right before `{`.
        let (o, c) = b.pair_at(0, 7).unwrap();
        assert_eq!((o.line, o.col, c.line, c.col), (0, 7, 2, 0));
        // Cursor right after `)` of b(1).
        let (o, c) = b.pair_at(1, 8).unwrap();
        assert_eq!((o.col, c.col), (5, 7));
        assert!(b.pair_at(1, 1).is_none());
        // Inside the body but outside b(1): the braces.
        let (o, _) = b.enclosing(text.find("b(").unwrap()).unwrap();
        assert_eq!(o.ch, '{');
        let (o, _) = b.enclosing(text.find('1').unwrap()).unwrap();
        assert_eq!(o.ch, '(');
    }

    #[test]
    fn indent_unit_detection() {
        let two = "a:\n  b:\n    c: 1\n  d: 2\n";
        assert_eq!(detect_indent_unit(two.lines(), 4), 2);
        let tabs = "fn x() {\n\tif y {\n\t\tz();\n\t}\n}\n";
        assert_eq!(detect_indent_unit(tabs.lines(), 4), 1);
        assert_eq!(detect_indent_unit("flat\ntext\n".lines(), 4), 4);
    }

    #[test]
    fn active_block() {
        let text = "fn a() {\n    if x {\n        y();\n\n        z();\n    }\n}\n";
        let lines: Vec<&str> = text.lines().collect();
        let ind = |r: usize| lines.get(r).and_then(|l| indent_of(l));
        let n = lines.len();
        // On `y();`: the `if` block's guide at column 4, rows 2..=4.
        assert_eq!(active_guide(&ind, n, 2, 4), Some((4, 2, 4)));
        // On the blank line inside it: the same.
        assert_eq!(active_guide(&ind, n, 3, 4), Some((4, 2, 4)));
        // On `if x {`, which opens the block: same block.
        assert_eq!(active_guide(&ind, n, 1, 4), Some((4, 2, 4)));
        // On `fn a() {`: the function body.
        assert_eq!(active_guide(&ind, n, 0, 4), Some((0, 1, 5)));
        assert_eq!(guide_indent(&ind, n, 3), 8);
    }
}
