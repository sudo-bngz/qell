//! LSP snippets (`fn ${1:name}($2) {\n\t$0\n}`): expanded to plain text
//! plus the tab stops to visit, in order.
//!
//! Supported: `$1`, `${1}`, `${1:placeholder}` (nested), `${1|a,b|}`
//! (first choice), variables `$NAME` / `${NAME:default}` (default or
//! empty), and `\$`, `\}`, `\\` escapes. A repeated tab stop keeps its
//! first occurrence. `$0`, the final cursor position, is visited last.

/// A tab stop: a character range of the expanded text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop {
    pub index: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// In visiting order: 1, 2, …, then 0.
    pub stops: Vec<Stop>,
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    out: String,
    /// Length of `out` in chars (offsets are in chars, like the buffer).
    len: usize,
    stops: Vec<Stop>,
    indent: &'a str,
}

impl Parser<'_> {
    fn push(&mut self, c: char) {
        self.out.push(c);
        self.len += 1;
        // Continuation lines keep the indentation of the line the snippet
        // is inserted on, like VS Code.
        if c == '\n' {
            self.out.push_str(self.indent);
            self.len += self.indent.chars().count();
        }
    }

    /// Parse until `stop` (a closing `}` or `|`) or the end.
    fn parse(&mut self, until: Option<char>) {
        while let Some(&c) = self.chars.peek() {
            if Some(c) == until {
                return;
            }
            self.chars.next();
            match c {
                '\\' => {
                    if let Some(n) = self.chars.next() {
                        if !matches!(n, '$' | '}' | '\\' | ',' | '|') {
                            self.push('\\');
                        }
                        self.push(n);
                    }
                }
                '$' => self.dollar(),
                // A tab in a snippet means one indentation level; keep it.
                c => self.push(c),
            }
        }
    }

    fn number(&mut self) -> Option<u32> {
        let mut digits = String::new();
        while let Some(d) = self.chars.peek().filter(|d| d.is_ascii_digit()) {
            digits.push(*d);
            self.chars.next();
        }
        digits.parse().ok()
    }

    fn name(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.chars.peek().filter(|c| c.is_ascii_alphanumeric() || **c == '_') {
            name.push(*c);
            self.chars.next();
        }
        name
    }

    fn add_stop(&mut self, index: u32, start: usize) {
        if !self.stops.iter().any(|s| s.index == index) {
            self.stops.push(Stop { index, start, end: self.len });
        }
    }

    fn dollar(&mut self) {
        match self.chars.peek().copied() {
            Some(d) if d.is_ascii_digit() => {
                let n = self.number().unwrap_or(0);
                self.add_stop(n, self.len);
            }
            Some('{') => {
                self.chars.next();
                let start = self.len;
                if self.chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    let n = self.number().unwrap_or(0);
                    match self.chars.next() {
                        Some(':') => {
                            self.parse(Some('}'));
                            self.chars.next();
                        }
                        Some('|') => {
                            // Choice: keep the first option.
                            let mut first = String::new();
                            let mut in_first = true;
                            while let Some(c) = self.chars.next() {
                                match c {
                                    '|' if self.chars.peek() == Some(&'}') => {
                                        self.chars.next();
                                        break;
                                    }
                                    ',' => in_first = false,
                                    '\\' => {
                                        if let (true, Some(n)) = (in_first, self.chars.next()) {
                                            first.push(n);
                                        }
                                    }
                                    c if in_first => first.push(c),
                                    _ => {}
                                }
                            }
                            for c in first.chars() {
                                self.push(c);
                            }
                        }
                        _ => {} // `${1}`
                    }
                    self.add_stop(n, start);
                } else {
                    // Variable: `${NAME}` or `${NAME:default}`.
                    let _ = self.name();
                    match self.chars.next() {
                        Some(':') => {
                            self.parse(Some('}'));
                            self.chars.next();
                        }
                        Some('}') | None => {}
                        Some(_) => {
                            // Transform or other syntax we don't run: skip it.
                            let mut depth = 1;
                            for c in self.chars.by_ref() {
                                match c {
                                    '{' => depth += 1,
                                    '}' => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let _ = self.name(); // `$NAME`: no value.
            }
            _ => self.push('$'),
        }
    }
}

/// Expand `snippet`, indenting continuation lines with `indent`.
pub fn expand(snippet: &str, indent: &str) -> Expanded {
    let mut p = Parser { chars: snippet.chars().peekable(), out: String::new(), len: 0, stops: Vec::new(), indent };
    p.parse(None);
    let mut stops = p.stops;
    // Visit 1, 2, … then 0 (the exit point).
    stops.sort_by_key(|s| if s.index == 0 { u32::MAX } else { s.index });
    Expanded { text: p.out, stops }
}

/// After the user typed into a tab stop ending at `old_end` (the cursor is
/// now at `cursor`), move the later stops by the same amount.
pub fn shift_stops(stops: &mut [(usize, usize)], old_end: usize, cursor: usize) {
    let delta = cursor as isize - old_end as isize;
    for (s, e) in stops.iter_mut() {
        if *s >= old_end {
            *s = (*s as isize + delta).max(0) as usize;
            *e = (*e as isize + delta).max(0) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stops(e: &Expanded) -> Vec<(u32, &str)> {
        let chars: Vec<char> = e.text.chars().collect();
        e.stops
            .iter()
            .map(|s| {
                (
                    s.index,
                    &e.text[chars[..s.start].iter().map(|c| c.len_utf8()).sum()..]
                        [..chars[s.start..s.end].iter().map(|c| c.len_utf8()).sum()],
                )
            })
            .collect()
    }

    #[test]
    fn placeholders_and_order() {
        let e = expand("foo(${1:a}, ${2:b})$0", "");
        assert_eq!(e.text, "foo(a, b)");
        assert_eq!(stops(&e), vec![(1, "a"), (2, "b"), (0, "")]);
    }

    #[test]
    fn plain_stops_and_nested_placeholder() {
        let e = expand("let ${1:x${2:y}} = $3;", "");
        assert_eq!(e.text, "let xy = ;");
        assert_eq!(stops(&e), vec![(1, "xy"), (2, "y"), (3, "")]);
    }

    #[test]
    fn choices_variables_and_escapes() {
        let e = expand("${1|pub,pub(crate)|} \\$x ${TM_FILENAME:file} $UNKNOWN!", "");
        assert_eq!(e.text, "pub $x file !");
        assert_eq!(stops(&e), vec![(1, "pub")]);
    }

    #[test]
    fn continuation_lines_are_indented() {
        let e = expand("if $1 {\n\t$0\n}", "    ");
        assert_eq!(e.text, "if  {\n    \t\n    }");
        // $0 sits after the indentation and the tab.
        let zero = e.stops.iter().find(|s| s.index == 0).unwrap();
        assert_eq!(e.text.chars().nth(zero.start - 1), Some('\t'));
    }

    #[test]
    fn repeated_stop_keeps_first() {
        let e = expand("${1:a} + ${1:a}", "");
        assert_eq!(e.text, "a + a");
        assert_eq!(e.stops.len(), 1);
    }

    #[test]
    fn plain_text_has_no_stops() {
        let e = expand("println!", "");
        assert_eq!(e.text, "println!");
        assert!(e.stops.is_empty());
    }

    #[test]
    fn offsets_are_in_chars() {
        let e = expand("é${1:ü}", "");
        assert_eq!(e.stops[0], Stop { index: 1, start: 1, end: 2 });
    }

    #[test]
    fn typing_into_a_stop_moves_the_next_ones() {
        // "foo(a, b)" inserted at 10: stop 1 = "a" (14..15), stop 2 = "b" (17..18), $0 at 19.
        let mut rest = vec![(17, 18), (19, 19)];
        // The user replaced "a" with "count": cursor at 14 + 5 = 19.
        shift_stops(&mut rest, 15, 19);
        assert_eq!(rest, vec![(21, 22), (23, 23)]);
        // Deleting the placeholder entirely moves them back.
        let mut rest = vec![(17, 18)];
        shift_stops(&mut rest, 15, 14);
        assert_eq!(rest, vec![(16, 17)]);
    }
}
