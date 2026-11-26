//! Screen columns of a line: a tab reaches the next tab stop, every other
//! char takes one column.

/// Screen column where each char of `line` starts, plus one past the end
/// (a trailing line break isn't counted).
pub fn display_cols(line: &str, tab: usize) -> Vec<usize> {
    let tab = tab.max(1);
    let mut cols = Vec::with_capacity(line.len() + 1);
    let mut x = 0;
    for c in line.chars() {
        if c == '\n' || c == '\r' {
            break;
        }
        cols.push(x);
        x = if c == '\t' { (x / tab + 1) * tab } else { x + 1 };
    }
    cols.push(x);
    cols
}

/// Screen column of char `col`, given `display_cols`. Past the end, one
/// column per char (virtual space).
pub fn disp(cols: &[usize], col: usize) -> usize {
    let last = cols.len() - 1;
    if col <= last {
        cols[col]
    } else {
        cols[last] + (col - last)
    }
}

/// The char column at screen offset `x` from char `start` (a fraction of a
/// column: where the mouse is). `round`: the nearest boundary between two
/// chars, for placing a cursor; otherwise the char under `x`.
pub fn col_at(cols: &[usize], start: usize, x: f32, round: bool) -> usize {
    let last = cols.len() - 1;
    let start = start.min(last);
    let base = cols[start] as f32;
    let mut c = start;
    while c < last {
        let (a, b) = (cols[c] as f32 - base, cols[c + 1] as f32 - base);
        let edge = if round { (a + b) / 2.0 } else { b };
        if x < edge {
            return c;
        }
        c += 1;
    }
    // Past the end: virtual columns.
    let past = x - (cols[last] as f32 - base);
    let extra = if round { past.round() } else { past.floor() };
    last + extra.max(0.0) as usize
}

/// `text`, which starts at screen column `start`, with each tab turned
/// into the spaces that reach its tab stop.
pub fn expand_tabs(text: &str, start: usize, tab: usize) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let tab = tab.max(1);
    let mut out = String::with_capacity(text.len() + 8);
    let mut x = start;
    for c in text.chars() {
        if c == '\t' {
            let next = (x / tab + 1) * tab;
            out.extend(std::iter::repeat_n(' ', next - x));
            x = next;
        } else {
            out.push(c);
            x += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_reach_the_next_stop() {
        assert_eq!(display_cols("\t\tx", 4), vec![0, 4, 8, 9]);
        assert_eq!(display_cols("ab\tc\n", 4), vec![0, 1, 2, 4, 5]);
        assert_eq!(display_cols("", 4), vec![0]);
        assert_eq!(disp(&[0, 4, 8, 9], 5), 11);
    }

    #[test]
    fn mouse_to_column() {
        let cols = display_cols("\t\tfmt", 4);
        // Inside the first tab: before it, or after it past its middle.
        assert_eq!(col_at(&cols, 0, 1.0, true), 0);
        assert_eq!(col_at(&cols, 0, 2.5, true), 1);
        assert_eq!(col_at(&cols, 0, 8.6, true), 3);
        assert_eq!(col_at(&cols, 0, 8.6, false), 2);
        assert_eq!(col_at(&cols, 0, 13.2, true), 7);
        // From a later start (a wrapped row).
        assert_eq!(col_at(&cols, 2, 0.6, true), 3);
    }

    #[test]
    fn expanding() {
        assert_eq!(expand_tabs("\tx", 0, 4), "    x");
        assert_eq!(expand_tabs("ab\tc", 0, 4), "ab  c");
        assert_eq!(expand_tabs("\tc", 6, 4), "  c");
    }
}
