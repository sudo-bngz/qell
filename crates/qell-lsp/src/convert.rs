//! Conversions between Qell positions (char columns) and LSP positions
//! (UTF-16 code-unit columns by default).

use std::str::FromStr;

use lsp_types::Uri;
use qell_core::Position;

/// Qell column → LSP UTF-16 column, given the line's text.
pub fn to_lsp_position(line_text: &str, p: Position) -> lsp_types::Position {
    let col16: usize = line_text.chars().take(p.col).map(char::len_utf16).sum();
    lsp_types::Position { line: p.line as u32, character: col16 as u32 }
}

/// LSP UTF-16 column → Qell column, given the line's text.
pub fn from_lsp_position(line_text: &str, p: lsp_types::Position) -> Position {
    let mut units = 0usize;
    let mut col = 0usize;
    for c in line_text.chars() {
        if units >= p.character as usize {
            break;
        }
        units += c.len_utf16();
        col += 1;
    }
    Position { line: p.line as usize, col }
}

pub fn path_to_uri(path: &std::path::Path) -> Option<Uri> {
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().ok()?.join(path) };
    let url = url::Url::from_file_path(abs).ok()?;
    Uri::from_str(url.as_str()).ok()
}

pub fn uri_to_path(uri: &Uri) -> Option<std::path::PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip() {
        let p = std::path::Path::new("/tmp/some dir/x.rs");
        let u = path_to_uri(p).unwrap();
        assert!(u.as_str().starts_with("file:///tmp/some%20dir/"));
        assert_eq!(uri_to_path(&u).unwrap(), p);
    }

    #[test]
    fn utf16_roundtrip() {
        let line = "a😀b"; // 😀 is 2 UTF-16 units
        let p = Position::new(0, 2); // after the emoji
        let l = to_lsp_position(line, p);
        assert_eq!(l.character, 3);
        assert_eq!(from_lsp_position(line, l), p);
    }
}
