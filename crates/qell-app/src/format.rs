//! Formatting a file: the language server when it offers it, otherwise the
//! language's own tool — `terraform fmt`, `gofmt`, `rustfmt` and friends.
//!
//! Nothing is downloaded or installed: a tool is used when it is already on
//! your PATH, and `[format]` in the config says which to use.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use qell_core::Position;

/// The tool for a language when the config names none. `{path}` stands for
/// the file being formatted; every one of these reads the text on stdin and
/// writes the result on stdout.
const BUILT_IN: &[(&str, &[&str])] = &[
    ("rust", &["rustfmt", "--emit", "stdout", "--quiet"]),
    ("go", &["gofmt"]),
    ("terraform", &["terraform", "fmt", "-"]),
    ("python", &["black", "--quiet", "-"]),
    ("c", &["clang-format", "--assume-filename={path}"]),
    ("cpp", &["clang-format", "--assume-filename={path}"]),
    ("sh", &["shfmt"]),
    ("toml", &["taplo", "format", "-"]),
    ("javascript", &["prettier", "--stdin-filepath", "{path}"]),
    ("typescript", &["prettier", "--stdin-filepath", "{path}"]),
    ("json", &["prettier", "--stdin-filepath", "{path}"]),
    ("css", &["prettier", "--stdin-filepath", "{path}"]),
    ("html", &["prettier", "--stdin-filepath", "{path}"]),
    ("yaml", &["prettier", "--stdin-filepath", "{path}"]),
    ("markdown", &["prettier", "--stdin-filepath", "{path}"]),
];

/// A formatter gives up after this long, so a stuck tool can't freeze Qell.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The command to format `language` with: the config's entry, else the
/// built-in one.
pub fn command_for(language: &str, configured: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    if let Some(cmd) = configured.get(language) {
        return (!cmd.is_empty()).then(|| cmd.clone());
    }
    BUILT_IN.iter().find(|(l, _)| *l == language).map(|(_, cmd)| cmd.iter().map(|s| (*s).to_string()).collect())
}

/// What to tell someone whose formatter isn't installed.
pub fn install_hint(program: &str) -> String {
    let how = match program {
        "rustfmt" => "rustup component add rustfmt",
        "gofmt" => "it comes with Go: https://go.dev/dl/",
        "terraform" => "https://developer.hashicorp.com/terraform/install",
        "black" => "pip install black",
        "clang-format" => "your package manager's clang-format (or llvm) package",
        "shfmt" => "https://github.com/mvdan/sh",
        "taplo" => "cargo install taplo-cli",
        "prettier" => "npm install -g prettier",
        _ => return format!("{program} isn't installed, or isn't on your PATH."),
    };
    format!("{program} isn't installed. Install it with: {how}")
}

/// Why formatting didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The tool isn't on the PATH.
    NotInstalled(String),
    /// It ran and complained (a syntax error, usually).
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled(program) => write!(f, "{}", install_hint(program)),
            Error::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// Run a formatter over `text`, returning what it printed.
pub fn run(cmd: &[String], text: &str, path: &Path, cwd: &Path) -> Result<String, Error> {
    let (program, args) = cmd.split_first().ok_or_else(|| Error::Failed("empty format command".into()))?;
    let args: Vec<String> = args.iter().map(|a| a.replace("{path}", &path.to_string_lossy())).collect();
    let mut child = Command::new(program)
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotInstalled(program.clone()),
            _ => Error::Failed(format!("{program}: {e}")),
        })?;
    // The tool reads everything before it answers, so write on a thread.
    let mut stdin = child.stdin.take().ok_or_else(|| Error::Failed("no stdin".into()))?;
    let owned = text.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(owned.as_bytes()));

    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                return Err(Error::Failed(format!("{program} took more than {}s", TIMEOUT.as_secs())));
            }
            Err(e) => return Err(Error::Failed(format!("{program}: {e}"))),
        }
    }
    let _ = writer.join();
    let out = child.wait_with_output().map_err(|e| Error::Failed(format!("{program}: {e}")))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let first = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("it reported an error");
        return Err(Error::Failed(format!("{program}: {}", first.trim())));
    }
    String::from_utf8(out.stdout).map_err(|_| Error::Failed(format!("{program} returned text that isn't UTF-8")))
}

/// The smallest set of line replacements turning `old` into `new`, last
/// first so earlier positions stay valid while they are applied.
///
/// Replacing the whole file would work, but this keeps the cursor, the
/// scroll position and the undo history tied to what actually changed.
pub fn edits(old: &str, new: &str) -> Vec<(Position, Position, String)> {
    use similar::DiffOp;
    if old == new {
        return Vec::new();
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let diff = similar::TextDiff::configure().timeout(std::time::Duration::from_millis(200)).diff_slices(&old_lines, &new_lines);
    let mut out = Vec::new();
    for op in diff.ops() {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match op {
            DiffOp::Equal { .. } => continue,
            _ => {
                let text: String = new_lines[new_range].concat();
                let start = Position::new(old_range.start, 0);
                // Up to the start of the line after the last one replaced;
                // at the end of the file, to the end of the last line.
                let end = match old_lines.get(old_range.end) {
                    Some(_) => Position::new(old_range.end, 0),
                    // Past the last line: the very end of the text. A text
                    // ending in a newline has an empty line after it.
                    None if old.ends_with('\n') => Position::new(old_lines.len(), 0),
                    None => {
                        let last = old_lines.len().saturating_sub(1);
                        Position::new(last, old_lines.get(last).map_or(0, |l| l.chars().count()))
                    }
                };
                out.push((start, end, text));
            }
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, new: &str) -> String {
        // Apply the edits to a buffer and check we really land on `new`.
        let mut buf = qell_core::Buffer::from_text(old, None);
        for (start, end, text) in edits(old, new) {
            buf.replace_range(start, end, &text);
        }
        buf.text()
    }

    #[test]
    fn edits_change_only_what_moved() {
        let old = "a = 1\nb   = 2\nc = 3\n";
        let new = "a = 1\nb = 2\nc = 3\n";
        let e = edits(old, new);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, Position::new(1, 0));
        assert_eq!(apply(old, new), new);
        assert!(edits(old, old).is_empty());
    }

    #[test]
    fn edits_handle_insertions_removals_and_the_last_line() {
        for (old, new) in [
            ("a\nb\n", "a\nx\nb\n"),             // inserted
            ("a\nb\nc\n", "a\nc\n"),             // removed
            ("a\nb", "a\nb\n"),                  // final newline added
            ("a\nb\n\n\n", "a\nb\n"),            // trailing blanks trimmed
            ("", "x\n"),                         // from empty
            ("one\ntwo\n", "ONE\nTWO\nthree\n"), // several hunks
        ] {
            assert_eq!(apply(old, new), new, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn built_in_commands_and_overrides() {
        let mut cfg = BTreeMap::new();
        assert_eq!(command_for("terraform", &cfg), Some(vec!["terraform".into(), "fmt".into(), "-".into()]));
        assert_eq!(command_for("cobol", &cfg), None);
        cfg.insert("terraform".to_string(), vec!["tofu".into(), "fmt".into(), "-".into()]);
        assert_eq!(command_for("terraform", &cfg).unwrap()[0], "tofu");
        // An empty list in the config turns formatting off for a language.
        cfg.insert("go".to_string(), Vec::new());
        assert_eq!(command_for("go", &cfg), None);
    }

    #[test]
    #[cfg(unix)]
    fn running_a_formatter() {
        let dir = std::env::temp_dir();
        let cmd = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // A formatter that upper-cases its input.
        let out = run(&cmd(&["tr", "a-z", "A-Z"]), "hello\n", Path::new("x.txt"), &dir).unwrap();
        assert_eq!(out, "HELLO\n");
        // The file's name can be passed along.
        let out = run(&cmd(&["echo", "{path}"]), "", Path::new("/tmp/a.tf"), &dir).unwrap();
        assert_eq!(out.trim(), "/tmp/a.tf");
        // Missing tools and failures are told apart.
        assert_eq!(
            run(&cmd(&["qell-no-such-formatter"]), "", Path::new("x"), &dir),
            Err(Error::NotInstalled("qell-no-such-formatter".into()))
        );
        let err = run(&cmd(&["sh", "-c", "echo 'line 3: bad syntax' >&2; exit 1"]), "", Path::new("x"), &dir).unwrap_err();
        assert!(matches!(err, Error::Failed(ref m) if m.contains("bad syntax")), "{err:?}");
        assert!(install_hint("rustfmt").contains("rustup component add"));
    }
}
