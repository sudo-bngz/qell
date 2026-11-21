//! Commenting lines out, and bringing them back.

/// How a language writes comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// What starts a comment that runs to the end of the line.
    pub line: Option<&'static str>,
    /// What wraps a comment, for languages without line comments.
    pub block: Option<(&'static str, &'static str)>,
}

/// The comment markers of a language.
pub fn style_for(language: &str) -> Style {
    let line = |l| Style { line: Some(l), block: None };
    match language {
        "rust" | "c" | "go" | "javascript" | "typescript" | "typescriptreact" | "vcl" | "alea" => line("//"),
        "css" => Style { line: None, block: Some(("/*", "*/")) },
        "terraform" | "toml" | "yaml" | "ini" | "shellscript" | "makefile" | "nginx" | "dockerfile" | "conf" => line("#"),
        "html" | "xml" | "markdown" | "markdown_inline" => Style { line: None, block: Some(("<!--", "-->")) },
        // JSON has no comments of its own, but every tool that reads it by
        // hand takes `//`, and it is what VS Code inserts.
        "json" => line("//"),
        _ => Style { line: None, block: None },
    }
}

/// What commenting the given lines should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Put `marker` at column `col` of each line listed.
    Comment { lines: Vec<usize>, col: usize, marker: String },
    /// Take `len` characters away at `col` of each `(line, col)`.
    Uncomment { at: Vec<(usize, usize)>, len: usize },
    /// Wrap a range in a block comment (open at the start, close at the end).
    Wrap { open: &'static str, close: &'static str },
    /// The language has no comments, or there is nothing to do.
    Nothing,
}

/// Decide what `⌘/` does to the given lines (their number and text, in
/// order). Lines that are entirely blank are left alone, and if every
/// other line is already commented the comments come off.
pub fn toggle(style: Style, lines: &[(usize, String)]) -> Action {
    let Some(marker) = style.line else {
        return match style.block {
            Some((open, close)) => Action::Wrap { open, close },
            None => Action::Nothing,
        };
    };
    let used: Vec<&(usize, String)> = lines.iter().filter(|(_, l)| !l.trim().is_empty()).collect();
    if used.is_empty() {
        return Action::Nothing;
    }
    let all_commented = used.iter().all(|(_, l)| l.trim_start().starts_with(marker));
    if all_commented {
        let at = used.iter().map(|(n, l)| (*n, l.chars().take_while(|c| c.is_whitespace()).count())).collect();
        // The space after the marker goes too, when it is there.
        let len = marker.chars().count() + usize::from(used.iter().all(|(_, l)| l.trim_start()[marker.len()..].starts_with(' ')));
        return Action::Uncomment { at, len };
    }
    // All the markers line up with the least indented line.
    let col = used.iter().map(|(_, l)| l.chars().take_while(|c| c.is_whitespace()).count()).min().unwrap_or(0);
    Action::Comment { lines: used.iter().map(|(n, _)| *n).collect(), col, marker: format!("{marker} ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numbered lines, starting at `first`.
    fn from(first: usize, text: &str) -> Vec<(usize, String)> {
        text.lines().enumerate().map(|(i, l)| (first + i, l.to_string())).collect()
    }

    #[test]
    fn markers_per_language() {
        assert_eq!(style_for("rust").line, Some("//"));
        assert_eq!(style_for("terraform").line, Some("#"));
        assert_eq!(style_for("html").block, Some(("<!--", "-->")));
        assert_eq!(style_for("plaintext"), Style { line: None, block: None });
    }

    #[test]
    fn commenting_lines_up_with_the_least_indented() {
        let a = toggle(style_for("rust"), &from(3, "    let a = 1;\n  let b = 2;"));
        assert_eq!(a, Action::Comment { lines: vec![3, 4], col: 2, marker: "// ".into() });
    }

    #[test]
    fn only_the_lines_given_are_touched() {
        // Two cursors, two far-apart lines: the ones between stay bare.
        let a = toggle(style_for("rust"), &[(0, "a".into()), (5, "b".into())]);
        assert_eq!(a, Action::Comment { lines: vec![0, 5], col: 0, marker: "// ".into() });
    }

    #[test]
    fn blank_lines_are_left_alone() {
        let a = toggle(style_for("rust"), &from(0, "a\n\nb"));
        assert_eq!(a, Action::Comment { lines: vec![0, 2], col: 0, marker: "// ".into() });
        assert_eq!(toggle(style_for("rust"), &from(0, "\n   ")), Action::Nothing);
    }

    #[test]
    fn already_commented_lines_come_back() {
        let a = toggle(style_for("terraform"), &from(1, "# a = 1\n  # b = 2"));
        assert_eq!(a, Action::Uncomment { at: vec![(1, 0), (2, 2)], len: 2 });
        // Without the space after the marker, only the marker goes.
        let a = toggle(style_for("terraform"), &from(0, "#a = 1\n#b = 2"));
        assert_eq!(a, Action::Uncomment { at: vec![(0, 0), (1, 0)], len: 1 });
        // One line still bare: everything gets commented instead.
        let a = toggle(style_for("terraform"), &from(0, "# a\nb"));
        assert!(matches!(a, Action::Comment { .. }));
    }

    #[test]
    fn languages_without_line_comments_wrap() {
        assert_eq!(toggle(style_for("css"), &from(0, "a {}")), Action::Wrap { open: "/*", close: "*/" });
        assert_eq!(toggle(style_for("plaintext"), &from(0, "x")), Action::Nothing);
    }
}
