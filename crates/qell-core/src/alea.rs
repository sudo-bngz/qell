//! A tolerant highlighter for [alea](https://github.com/sudo-bngz/alea), a
//! language for generative art.
//!
//! alea has no tree-sitter grammar, and its own lexer stops at the first
//! error, which is the normal state of a file being typed. This one never
//! fails: it walks the text once and colours what it recognises (comments,
//! strings, numbers, keywords, calls, named arguments and the `~` draw),
//! leaving the rest as plain text.

use crate::syntax::{HighlightSpan, TokenKind};

const KEYWORDS: &[&str] = &[
    "work",
    "let",
    "field",
    "pattern",
    "constraint",
    "render",
    "output",
    "choose",
    "repeat",
    "synth",
    "event",
    "use",
    "grow",
    "then",
    "for",
    "in",
    "over",
    "and",
    "or",
];

/// What a `render` produces: `render image(...)`.
const RENDER_KINDS: &[&str] = &["image", "sound", "video", "sculpture", "midi"];

/// Names alea binds for you inside a work.
const BUILTIN_VARS: &[&str] = &["x", "y", "t", "i", "n"];
const CONSTANTS: &[&str] = &["pi", "tau"];

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Highlight a whole alea source file.
pub fn highlight(source: &str) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let mut push = |start: usize, end: usize, kind: TokenKind| {
        if end > start {
            spans.push(HighlightSpan { start, end, kind });
        }
    };
    let bytes = source.as_bytes();
    let mut chars = source.char_indices().peekable();
    // The keyword just before the identifier being read: `work Rosace`
    // names a type, `let petal` a binding, `render image` a kind.
    let mut after: Option<&str> = None;

    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }

        // Comments run to the end of the line: `// …` and `# …`.
        if c == '#' || (c == '/' && bytes.get(start + 1) == Some(&b'/')) {
            let end = source[start..].find('\n').map_or(source.len(), |n| start + n);
            while chars.peek().is_some_and(|(i, _)| *i < end) {
                chars.next();
            }
            push(start, end, TokenKind::Comment);
            continue;
        }

        // Strings close at the quote, or at the end of the line while the
        // closing quote is still to be typed.
        if c == '"' {
            let mut end = source.len();
            while let Some(&(i, ch)) = chars.peek() {
                if ch == '\n' {
                    end = i;
                    break;
                }
                chars.next();
                if ch == '"' {
                    end = i + 1;
                    break;
                }
            }
            push(start, end, TokenKind::String);
            after = None;
            continue;
        }

        if c.is_ascii_digit() {
            let mut end = start + 1;
            while let Some(&(i, ch)) = chars.peek() {
                let fraction = ch == '.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
                if ch.is_ascii_digit() || ch == '_' || fraction {
                    chars.next();
                    end = i + 1;
                } else {
                    break;
                }
            }
            push(start, end, TokenKind::Number);
            after = None;
            continue;
        }

        if is_ident_start(c) {
            let mut end = start + c.len_utf8();
            while let Some(&(i, ch)) = chars.peek() {
                if !is_ident_part(ch) {
                    break;
                }
                chars.next();
                end = i + ch.len_utf8();
            }
            let word = &source[start..end];
            let next = source[end..].trim_start_matches([' ', '\t']).chars().next();
            let previous = after.take();

            let kind = if previous == Some("work") {
                Some(TokenKind::Type)
            } else if matches!(previous, Some("let" | "field" | "pattern")) {
                Some(TokenKind::Variable)
            } else if previous == Some("render") && RENDER_KINDS.contains(&word) {
                Some(TokenKind::Type)
            } else if KEYWORDS.contains(&word) {
                after = KEYWORDS.iter().copied().find(|k| *k == word);
                Some(TokenKind::Keyword)
            } else if next == Some('(') {
                // `Rosace(...)` calls another work, `blob(...)` a function.
                Some(if word.chars().next().is_some_and(char::is_uppercase) { TokenKind::Type } else { TokenKind::Function })
            } else if next == Some(':') {
                Some(TokenKind::Parameter)
            } else if CONSTANTS.contains(&word) {
                Some(TokenKind::Constant)
            } else if BUILTIN_VARS.contains(&word) {
                Some(TokenKind::Variable)
            } else {
                None
            };
            if let Some(kind) = kind {
                push(start, end, kind);
            }
            continue;
        }

        after = None;
        match c {
            // The draw. `~` picks once per edition, `~~` at every point; it
            // is what makes a work non-deterministic, so it reads as a
            // keyword rather than as arithmetic.
            '~' => {
                let end = if chars.peek().is_some_and(|(_, ch)| *ch == '~') {
                    chars.next();
                    start + 2
                } else {
                    start + 1
                };
                push(start, end, TokenKind::Keyword);
            }
            '(' | ')' | '{' | '}' | '[' | ']' | ',' | ':' | '.' => push(start, start + 1, TokenKind::Punctuation),
            '+' | '-' | '*' | '/' | '%' | '^' | '|' | '<' | '>' | '=' | '!' => {
                let end = if chars.peek().is_some_and(|(_, ch)| *ch == '=') && matches!(c, '<' | '>' | '=' | '!') {
                    chars.next();
                    start + 2
                } else {
                    start + 1
                };
                push(start, end, TokenKind::Operator);
            }
            _ => {}
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    fn kinds(src: &str) -> Vec<(&str, TokenKind)> {
        highlight(src).into_iter().map(|s| (&src[s.start..s.end], s.kind)).collect()
    }

    fn kind_of(src: &str, token: &str) -> Option<TokenKind> {
        kinds(src).into_iter().find(|(t, _)| *t == token).map(|(_, k)| k)
    }

    #[test]
    fn work_and_bindings() {
        let src = "work Rosace {\n  let petal = blob(x: 0.72, size: 0.22)\n}";
        assert_eq!(kind_of(src, "work"), Some(Keyword));
        assert_eq!(kind_of(src, "Rosace"), Some(Type));
        assert_eq!(kind_of(src, "let"), Some(Keyword));
        assert_eq!(kind_of(src, "petal"), Some(Variable));
        assert_eq!(kind_of(src, "blob"), Some(Function));
        assert_eq!(kind_of(src, "size"), Some(Parameter));
        assert_eq!(kind_of(src, "0.72"), Some(Number));
    }

    #[test]
    fn draws_and_choices() {
        let src = "let sw = ~between(0.54, 0.60)\nlet g = ~~normal(0, 1)\nlet s = choose { 5 | 6 | \"ember\" }";
        assert_eq!(kind_of(src, "~"), Some(Keyword));
        assert_eq!(kind_of(src, "~~"), Some(Keyword));
        assert_eq!(kind_of(src, "between"), Some(Function));
        assert_eq!(kind_of(src, "choose"), Some(Keyword));
        assert_eq!(kind_of(src, "|"), Some(Operator));
        assert_eq!(kind_of(src, "\"ember\""), Some(String));
    }

    #[test]
    fn render_kinds_and_comments() {
        let src = "// a rose window\nrender image(output, palette: \"night\") # done\n";
        assert_eq!(kind_of(src, "// a rose window"), Some(Comment));
        assert_eq!(kind_of(src, "render"), Some(Keyword));
        assert_eq!(kind_of(src, "image"), Some(Type));
        assert_eq!(kind_of(src, "output"), Some(Keyword));
        assert_eq!(kind_of(src, "# done"), Some(Comment));
    }

    #[test]
    fn division_is_not_a_comment() {
        let src = "let k = prog.key / 4";
        assert_eq!(kind_of(src, "/"), Some(Operator));
        assert_eq!(kind_of(src, "4"), Some(Number));
    }

    #[test]
    fn half_typed_code_still_highlights() {
        let src = "work Été {\n  let name = \"unterminated\n  let n = 1_000 ~";
        assert_eq!(kind_of(src, "Été"), Some(Type));
        assert_eq!(kind_of(src, "\"unterminated"), Some(String));
        assert_eq!(kind_of(src, "1_000"), Some(Number));
        assert_eq!(kind_of(src, "n"), Some(Variable));
    }

    #[test]
    fn spans_are_ordered_and_in_bounds() {
        let src = "work V {\n  let o = [~between(0.5, 0.8), x * tau]\n  render sculpture(o, side: 80)\n}\n";
        let spans = highlight(src);
        let mut last = 0;
        for s in &spans {
            assert!(s.start >= last && s.end <= src.len() && s.start < s.end);
            assert!(src.is_char_boundary(s.start) && src.is_char_boundary(s.end));
            last = s.end;
        }
        assert_eq!(kind_of(src, "tau"), Some(Constant));
        assert_eq!(kind_of(src, "x"), Some(Variable));
    }
}
