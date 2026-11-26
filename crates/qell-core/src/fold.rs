//! Code folding: which lines can be folded away, by indentation.
//!
//! A line opens a region when the lines after it are indented further;
//! the region ends at the last of those lines, so a closing `}` at the
//! header's own indentation stays visible, as in VS Code's indentation
//! folding. Blank lines inside a region belong to it but don't end it.
//! Works the same for every language, servers or not.

use std::collections::BTreeMap;

/// Columns of leading whitespace, a tab counting `tab` columns; `None`
/// for a blank line.
fn indent(line: &str, tab: usize) -> Option<usize> {
    let mut col = 0;
    for c in line.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += tab - col % tab.max(1),
            '\n' | '\r' => return None,
            _ => return Some(col),
        }
    }
    None
}

/// Every foldable region: header line → last line folded under it.
pub fn regions<S: AsRef<str>>(lines: impl Iterator<Item = S>, tab: usize) -> BTreeMap<usize, usize> {
    let mut out = BTreeMap::new();
    // Headers still open, with their indentation, outermost first.
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_text: Option<usize> = None;
    for (i, line) in lines.enumerate() {
        let Some(d) = indent(line.as_ref(), tab.max(1)) else { continue };
        while let Some(&(hd, h)) = open.last() {
            if hd < d {
                break;
            }
            open.pop();
            if let Some(end) = last_text.filter(|&e| e > h) {
                out.insert(h, end);
            }
        }
        open.push((d, i));
        last_text = Some(i);
    }
    for (_, h) in open {
        if let Some(end) = last_text.filter(|&e| e > h) {
            out.insert(h, end);
        }
    }
    out
}

/// The innermost region holding `line` (its header may be `line` itself).
pub fn around(regions: &BTreeMap<usize, usize>, line: usize) -> Option<(usize, usize)> {
    regions.range(..=line).rev().find(|(_, &end)| end >= line).map(|(&h, &e)| (h, e))
}

/// Sticky scroll: the lines opening the blocks around line `top`, the
/// outermost first, at most `max`. They cover the first rows on screen, so
/// a block is kept only while it still holds the first line visible below
/// them.
pub fn sticky(regions: &BTreeMap<usize, usize>, top: usize, max: usize) -> Vec<usize> {
    let mut lines: Vec<(usize, usize)> =
        regions.range(..top).filter(|(_, end)| **end >= top).map(|(h, e)| (*h, *e)).take(max).collect();
    while lines.last().is_some_and(|(_, end)| *end < top + lines.len()) {
        lines.pop();
    }
    lines.into_iter().map(|(h, _)| h).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_lines_are_the_open_blocks() {
        let code = "fn a() {\n    if x {\n        one();\n        two();\n        three();\n    }\n}\n";
        let r = regions(code.lines(), 4);
        assert_eq!(sticky(&r, 0, 3), Vec::<usize>::new());
        // Scrolled to `one();`: fn and if stay on top, covering two rows,
        // so the blocks are those around `three();` — the same.
        assert_eq!(sticky(&r, 2, 3), vec![0, 1]);
        assert_eq!(sticky(&r, 2, 1), vec![0]);
        // At `three();`: the if would cover its own last line, so only fn.
        assert_eq!(sticky(&r, 4, 3), vec![0]);
        // At the if's `}`: fn's last line is hidden under it, so nothing.
        assert_eq!(sticky(&r, 5, 3), Vec::<usize>::new());
    }

    const CODE: &str = "\
fn main() {
    let a = 1;
    if a > 0 {
        go();

        stop();
    }
}
fn other() {}
";

    #[test]
    fn regions_follow_indentation() {
        let r = regions(CODE.lines(), 4);
        // main's body (not its closing brace), and the if's body.
        assert_eq!(r.get(&0), Some(&6));
        assert_eq!(r.get(&2), Some(&5));
        // One-liners and leaves don't fold.
        assert_eq!(r.get(&8), None);
        assert_eq!(r.get(&1), None);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn the_innermost_region_wins() {
        let r = regions(CODE.lines(), 4);
        assert_eq!(around(&r, 3), Some((2, 5)));
        assert_eq!(around(&r, 4), Some((2, 5)), "a blank line inside counts");
        assert_eq!(around(&r, 1), Some((0, 6)));
        assert_eq!(around(&r, 2), Some((2, 5)), "a header is in its own region");
        assert_eq!(around(&r, 8), None);
    }

    #[test]
    fn yaml_and_python_fold_without_braces() {
        let yaml = "a:\n  b: 1\n  c:\n    - x\nd: 2\n";
        let r = regions(yaml.lines(), 2);
        assert_eq!((r.get(&0), r.get(&2)), (Some(&3), Some(&3)));
        let py = "def f():\n\tif x:\n\t\treturn 1\n\treturn 2\n";
        let r = regions(py.lines(), 4);
        assert_eq!((r.get(&0), r.get(&1)), (Some(&3), Some(&2)));
    }
}
