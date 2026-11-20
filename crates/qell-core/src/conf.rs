//! A tolerant highlighter for generic configuration files (`*.conf`,
//! `*.cnf`, `sshd_config`…).
//!
//! These files share no grammar: some are INI-like (`key = value`,
//! `[section]`), some Apache-like (`Directive value`, `<Block>`), some
//! nginx-like (`name value;`, `{ }`). Rather than guess one grammar and
//! mis-colour the others, this lexer recognises the pieces they all have in
//! common, line by line: comments, section headers, the leading key or
//! directive, strings, numbers (with units), booleans and variables.

use crate::syntax::{HighlightSpan, TokenKind};

/// Highlight a whole config file.
pub fn highlight(source: &str) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        highlight_line(line.trim_end_matches(['\n', '\r']), offset, &mut spans);
        offset += line.len();
    }
    spans
}

fn push(spans: &mut Vec<HighlightSpan>, base: usize, start: usize, end: usize, kind: TokenKind) {
    if end > start {
        spans.push(HighlightSpan { start: base + start, end: base + end, kind });
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b'@' | b'*' | b'~' | b'+')
}

fn highlight_line(line: &str, base: usize, spans: &mut Vec<HighlightSpan>) {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
    let rest = &line[indent..];
    if rest.is_empty() {
        return;
    }

    // Whole-line comments: #, ;, //.
    if rest.starts_with('#') || rest.starts_with(';') || rest.starts_with("//") {
        push(spans, base, indent, line.len(), TokenKind::Comment);
        return;
    }

    // [section] / [[array]] headers.
    if rest.starts_with('[') {
        if let Some(close) = rest.rfind(']') {
            let (open_len, close_len) = if rest.starts_with("[[") && rest[..=close].ends_with("]]") { (2, 2) } else { (1, 1) };
            push(spans, base, indent, indent + open_len, TokenKind::Punctuation);
            push(spans, base, indent + open_len, indent + close + 1 - close_len, TokenKind::Type);
            push(spans, base, indent + close + 1 - close_len, indent + close + 1, TokenKind::Punctuation);
            values(line, base, indent + close + 1, spans);
            return;
        }
    }

    // <Directory /var/www> … </Directory> (Apache, Tomcat, …).
    if let Some(tag) = rest.strip_prefix('<').filter(|_| rest.contains('>')) {
        let name_start = if tag.starts_with('/') { 2 } else { 1 };
        let name_len = rest[name_start..].bytes().take_while(|b| is_word_byte(*b)).count();
        push(spans, base, indent, indent + name_start, TokenKind::Punctuation);
        push(spans, base, indent + name_start, indent + name_start + name_len, TokenKind::Tag);
        values(line, base, indent + name_start + name_len, spans);
        return;
    }

    // Leading key / directive.
    let key_len = rest.bytes().take_while(|b| is_word_byte(*b)).count();
    if key_len == 0 {
        values(line, base, indent, spans);
        return;
    }
    push(spans, base, indent, indent + key_len, TokenKind::Property);
    let mut pos = indent + key_len;
    // Optional separator: `=`, `:`, `+=`, `:=`.
    let ws = line[pos..].bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    let after = &line[pos + ws..];
    for sep in [":=", "+=", "=", ":"] {
        if after.starts_with(sep) {
            push(spans, base, pos + ws, pos + ws + sep.len(), TokenKind::Operator);
            pos += ws + sep.len();
            break;
        }
    }
    values(line, base, pos, spans);
}

/// Colour the value part of a line, from byte `from`.
fn values(line: &str, base: usize, from: usize, spans: &mut Vec<HighlightSpan>) {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' => i += 1,
            // Trailing comment (`# …`, INI's `; …`), only after whitespace so
            // `a#b`, URL fragments and nginx's `listen 80;` stay values.
            b'#' | b';' if i > 0 && bytes[i - 1].is_ascii_whitespace() => {
                push(spans, base, i, bytes.len(), TokenKind::Comment);
                return;
            }
            b'"' | b'\'' | b'`' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(bytes.len());
                push(spans, base, i, end, TokenKind::String);
                i = end;
            }
            b'$' | b'%' if i + 1 < bytes.len() && (bytes[i + 1] == b'{' || bytes[i + 1] == b'(') => {
                let close = if bytes[i + 1] == b'{' { b'}' } else { b')' };
                let end = bytes[i..].iter().position(|c| *c == close).map(|p| i + p + 1).unwrap_or(bytes.len());
                push(spans, base, i, end, TokenKind::Variable);
                i = end;
            }
            b'$' if i + 1 < bytes.len() && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_') => {
                let len = 1 + bytes[i + 1..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
                push(spans, base, i, i + len, TokenKind::Variable);
                i += len;
            }
            b'{' | b'}' | b'(' | b')' | b',' | b';' => {
                push(spans, base, i, i + 1, TokenKind::Punctuation);
                i += 1;
            }
            b'=' => {
                push(spans, base, i, i + 1, TokenKind::Operator);
                i += 1;
            }
            _ => {
                let len = bytes[i..]
                    .iter()
                    .take_while(|c| {
                        !c.is_ascii_whitespace() && !matches!(c, b'"' | b'\'' | b'{' | b'}' | b'(' | b')' | b',' | b';' | b'=')
                    })
                    .count()
                    .max(1);
                let word = &line[i..i + len];
                if let Some(kind) = classify_word(word) {
                    push(spans, base, i, i + len, kind);
                }
                i += len;
            }
        }
    }
}

fn classify_word(word: &str) -> Option<TokenKind> {
    let lower = word.to_ascii_lowercase();
    if matches!(lower.as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "enabled" | "disabled" | "none" | "null") {
        return Some(TokenKind::Constant);
    }
    // 42, -1, 3.14, 0x1f, 10s, 512M, 80%, 1.2.3.4 (IPs read as numbers too).
    let digits = word.trim_start_matches(['-', '+']);
    if digits.starts_with(|c: char| c.is_ascii_digit()) {
        let numeric = digits.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
        if !numeric.is_empty() && numeric.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ':' || c == 'x') {
            return Some(TokenKind::Number);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(&str, TokenKind)> {
        highlight(src).into_iter().map(|s| (&src[s.start..s.end], s.kind)).collect()
    }

    #[test]
    fn ini_style() {
        let k = kinds("[server]\nport = 8080 ; main\nname = \"qell\"\n");
        assert!(k.contains(&("server", TokenKind::Type)));
        assert!(k.contains(&("port", TokenKind::Property)));
        assert!(k.contains(&("=", TokenKind::Operator)));
        assert!(k.contains(&("8080", TokenKind::Number)));
        assert!(k.contains(&("; main", TokenKind::Comment)));
        assert!(k.contains(&("\"qell\"", TokenKind::String)));
    }

    #[test]
    fn directive_style() {
        let k = kinds("# sshd\nPermitRootLogin no\nMaxSessions 10\n<Directory /var/www>\n  Require all granted\n</Directory>\n");
        assert!(k.contains(&("# sshd", TokenKind::Comment)));
        assert!(k.contains(&("PermitRootLogin", TokenKind::Property)));
        assert!(k.contains(&("no", TokenKind::Constant)));
        assert!(k.contains(&("10", TokenKind::Number)));
        assert!(k.contains(&("Directory", TokenKind::Tag)));
        assert!(k.contains(&("Require", TokenKind::Property)));
    }

    #[test]
    fn block_style_and_variables() {
        let k = kinds("upstream app {\n    timeout 30s;\n    path ${HOME}/x $USER;\n}\n");
        assert!(k.contains(&("upstream", TokenKind::Property)));
        assert!(k.contains(&("{", TokenKind::Punctuation)));
        assert!(k.contains(&("30s", TokenKind::Number)));
        assert!(k.contains(&("${HOME}", TokenKind::Variable)));
        assert!(k.contains(&("$USER", TokenKind::Variable)));
    }

    #[test]
    fn hash_inside_value_is_not_a_comment() {
        let k = kinds("url = http://x/a#frag\n");
        assert!(!k.iter().any(|(_, kind)| *kind == TokenKind::Comment));
    }

    #[test]
    fn spans_are_ordered_and_in_bounds() {
        let src = "a = \"unterminated\nb { c 1; } # x\n\t\n[[t]]\n<x\n";
        let spans = highlight(src);
        for w in spans.windows(2) {
            assert!(w[0].end <= w[1].start);
        }
        assert!(spans.iter().all(|s| s.end <= src.len() && s.start < s.end));
    }
}
