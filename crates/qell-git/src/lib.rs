//! Git integration.
//!
//! Qell shells out to the `git` binary instead of linking libgit2: it is
//! always the user's own git, with their config, hooks and credential
//! helpers, and it keeps the build free of C dependencies. Status runs on a
//! background thread ([`Watcher`]) so a slow repository never stalls a frame.
//!
//! Paths in a [`RepoStatus`] are relative to the *workspace* root, which may
//! be a subdirectory of the repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

/// The state of one file (or, propagated, one directory) in the working tree.
///
/// Ordered by how loudly it should show up: when a directory contains files
/// in several states it takes the highest one, like VS Code does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileStatus {
    Ignored,
    Renamed,
    Added,
    Untracked,
    Modified,
    Deleted,
    Conflict,
}

impl FileStatus {
    /// The one-letter badge VS Code shows next to the name.
    pub fn badge(self) -> &'static str {
        match self {
            FileStatus::Ignored => "",
            FileStatus::Renamed => "R",
            FileStatus::Added => "A",
            FileStatus::Untracked => "U",
            FileStatus::Modified => "M",
            FileStatus::Deleted => "D",
            FileStatus::Conflict => "!",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            FileStatus::Ignored => "Ignored",
            FileStatus::Renamed => "Renamed",
            FileStatus::Added => "Added",
            FileStatus::Untracked => "Untracked",
            FileStatus::Modified => "Modified",
            FileStatus::Deleted => "Deleted",
            FileStatus::Conflict => "Conflict",
        }
    }

    /// Statuses a whole directory can have that apply to everything inside
    /// it (git reports `?? dir/` and `!! dir/` without listing the contents).
    fn is_inherited(self) -> bool {
        matches!(self, FileStatus::Untracked | FileStatus::Ignored)
    }
}

/// Classify a porcelain v1 `XY` code.
fn classify(x: u8, y: u8) -> Option<FileStatus> {
    Some(match (x, y) {
        (b'?', b'?') => FileStatus::Untracked,
        (b'!', b'!') => FileStatus::Ignored,
        (b'D', b'D') | (b'A', b'A') | (b'U', _) | (_, b'U') => FileStatus::Conflict,
        (_, b'D') | (b'D', _) => FileStatus::Deleted,
        (b'A', _) => FileStatus::Added,
        (b'R', _) | (b'C', _) => FileStatus::Renamed,
        (b'M' | b'T', _) | (_, b'M' | b'T') => FileStatus::Modified,
        _ => return None,
    })
}

/// A snapshot of `git status` for a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Current branch, or a short commit hash when detached.
    pub branch: Option<String>,
    /// Commits ahead of / behind the upstream, when there is one.
    pub ahead: u32,
    pub behind: u32,
    /// Files and directories with an explicit status. Directories hold the
    /// strongest status found beneath them.
    entries: HashMap<PathBuf, FileStatus>,
    /// Number of changed files (not directories, not ignored).
    changed: usize,
}

impl RepoStatus {
    /// Status of a path relative to the workspace root, including states
    /// inherited from an untracked or ignored ancestor directory.
    pub fn status_of(&self, rel: &Path) -> Option<FileStatus> {
        if let Some(s) = self.entries.get(rel) {
            return Some(*s);
        }
        rel.ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
            .find_map(|a| self.entries.get(a).copied().filter(|s| s.is_inherited()))
    }

    /// Number of changed files (excluding ignored ones), for the status bar.
    pub fn changed_files(&self) -> usize {
        self.changed
    }

    /// Parse `git status --porcelain=v1 -z --branch --ignored` output.
    ///
    /// `prefix` is the workspace's path inside the repository (from
    /// `git rev-parse --show-prefix`, e.g. `"sub/dir/"`); entries outside it
    /// are dropped and the rest are made relative to it.
    pub fn parse(output: &[u8], prefix: &str) -> Self {
        let prefix = Path::new(prefix.trim_end_matches('/'));
        let mut status = RepoStatus::default();
        let mut files: Vec<(PathBuf, FileStatus)> = Vec::new();

        let mut records = output.split(|b| *b == 0).filter(|r| !r.is_empty());
        while let Some(rec) = records.next() {
            if let Some(branch) = rec.strip_prefix(b"## ") {
                status.parse_branch(&String::from_utf8_lossy(branch));
                continue;
            }
            if rec.len() < 4 {
                continue;
            }
            let (x, y) = (rec[0], rec[1]);
            // Renames and copies are followed by the original path.
            if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
                records.next();
            }
            let Some(st) = classify(x, y) else { continue };
            let path = String::from_utf8_lossy(&rec[3..]);
            let path = Path::new(path.trim_end_matches('/'));
            let Ok(rel) = path.strip_prefix(prefix) else { continue };
            if rel.as_os_str().is_empty() {
                // The workspace root itself (e.g. the whole folder is untracked).
                continue;
            }
            files.push((rel.to_path_buf(), st));
        }

        status.changed = files.iter().filter(|(_, s)| *s != FileStatus::Ignored).count();
        for (path, st) in files {
            status.entries.entry(path.clone()).and_modify(|e| *e = (*e).max(st)).or_insert(st);
            // Ignored files don't make their folder look changed.
            if st == FileStatus::Ignored {
                continue;
            }
            for dir in path.ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()) {
                // A deleted file is gone from the tree; show its folder as modified.
                let dir_st = if st == FileStatus::Deleted { FileStatus::Modified } else { st };
                let e = status.entries.entry(dir.to_path_buf()).or_insert(dir_st);
                if *e == FileStatus::Ignored || dir_st > *e {
                    *e = dir_st;
                }
            }
        }
        status
    }

    fn parse_branch(&mut self, line: &str) {
        // Forms: "main", "main...origin/main [ahead 1, behind 2]",
        // "No commits yet on main", "HEAD (no branch)".
        let (head, tracking) = match line.split_once(" [") {
            Some((h, t)) => (h, Some(t.trim_end_matches(']'))),
            None => (line, None),
        };
        let name = head.split("...").next().unwrap_or(head);
        let name = name.strip_prefix("No commits yet on ").or_else(|| name.strip_prefix("Initial commit on ")).unwrap_or(name);
        if name != "HEAD (no branch)" {
            self.branch = Some(name.to_string());
        }
        for part in tracking.into_iter().flat_map(|t| t.split(", ")) {
            if let Some(n) = part.strip_prefix("ahead ") {
                self.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                self.behind = n.parse().unwrap_or(0);
            }
        }
    }
}

fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root);
    // Don't take index.lock for a read: a background status must never
    // make the user's own `git commit` in the terminal fail.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.stdin(std::process::Stdio::null());
    cmd
}

/// Run `git status` for `root`. `None` when it isn't inside a repository or
/// git isn't installed.
pub fn status(root: &Path) -> Option<RepoStatus> {
    let out = git(root).args(["rev-parse", "--show-prefix"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let out = git(root)
        .args(["status", "--porcelain=v1", "-z", "--branch", "--ignored=matching", "--untracked-files=normal"])
        .output()
        .ok()?;
    if !out.status.success() {
        log::warn!("git status failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        return None;
    }
    let mut st = RepoStatus::parse(&out.stdout, &prefix);
    if st.branch.is_none() {
        // Detached HEAD: show the short hash instead.
        if let Ok(o) = git(root).args(["rev-parse", "--short", "HEAD"]).output() {
            if o.status.success() {
                st.branch = Some(String::from_utf8_lossy(&o.stdout).trim().to_string());
            }
        }
    }
    Some(st)
}

enum Request {
    Refresh,
    SetRoot(PathBuf),
}

/// Keeps a [`RepoStatus`] up to date on a background thread: refreshes on
/// demand and every few seconds, and hands results back through [`poll`].
///
/// [`poll`]: Watcher::poll
pub struct Watcher {
    tx: Sender<Request>,
    rx: Receiver<Option<RepoStatus>>,
}

/// How often the status is refreshed when nothing asks for it.
const INTERVAL: Duration = Duration::from_secs(3);

impl Watcher {
    pub fn new(root: PathBuf) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (res_tx, res_rx) = mpsc::channel();
        thread::Builder::new()
            .name("qell-git".into())
            .spawn(move || {
                let mut root = root;
                let mut last: Option<Option<RepoStatus>> = None;
                loop {
                    let st = status(&root);
                    // Only wake the UI when something changed.
                    if last.as_ref() != Some(&st) {
                        if res_tx.send(st.clone()).is_err() {
                            return;
                        }
                        last = Some(st);
                    }
                    match req_rx.recv_timeout(INTERVAL) {
                        Ok(Request::Refresh) | Err(RecvTimeoutError::Timeout) => {}
                        Ok(Request::SetRoot(r)) => {
                            root = r;
                            last = None;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    // Coalesce a burst of requests into one status run.
                    while let Ok(req) = req_rx.try_recv() {
                        if let Request::SetRoot(r) = req {
                            root = r;
                            last = None;
                        }
                    }
                }
            })
            .expect("spawn git thread");
        Self { tx: req_tx, rx: res_rx }
    }

    /// Ask for a status run now (after a save, a refresh, regaining focus).
    pub fn refresh(&self) {
        let _ = self.tx.send(Request::Refresh);
    }

    /// Switch to another workspace.
    pub fn set_root(&self, root: PathBuf) {
        let _ = self.tx.send(Request::SetRoot(root));
    }

    /// The newest status, if one arrived since the last call. The outer
    /// `Option` is "is there news", the inner one "is this a repository".
    pub fn poll(&self) -> Option<Option<RepoStatus>> {
        self.rx.try_iter().last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn parses_files_and_propagates_to_dirs() {
        let out =
            b"## main...origin/main [ahead 2, behind 1]\0 M src/app.rs\0?? notes/\0A  src/new.rs\0!! target/\0 D old/gone.rs\0";
        let st = RepoStatus::parse(out, "");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.status_of(&p("src/app.rs")), Some(FileStatus::Modified));
        assert_eq!(st.status_of(&p("src/new.rs")), Some(FileStatus::Added));
        // Strongest child wins.
        assert_eq!(st.status_of(&p("src")), Some(FileStatus::Modified));
        assert_eq!(st.status_of(&p("old")), Some(FileStatus::Modified));
        // Inherited from the untracked / ignored directory.
        assert_eq!(st.status_of(&p("notes/todo.md")), Some(FileStatus::Untracked));
        assert_eq!(st.status_of(&p("target/debug/qell")), Some(FileStatus::Ignored));
        assert_eq!(st.status_of(&p("src/lib.rs")), None);
        assert_eq!(st.status_of(&p("README.md")), None);
    }

    #[test]
    fn renames_skip_the_original_path() {
        let out = b"## main\0R  b.rs\0a.rs\0 M c.rs\0";
        let st = RepoStatus::parse(out, "");
        assert_eq!(st.status_of(&p("b.rs")), Some(FileStatus::Renamed));
        assert_eq!(st.status_of(&p("a.rs")), None);
        assert_eq!(st.status_of(&p("c.rs")), Some(FileStatus::Modified));
    }

    #[test]
    fn conflicts() {
        for code in ["UU", "AA", "DD", "AU", "UD"] {
            let out = format!("{code} f.rs\0");
            assert_eq!(RepoStatus::parse(out.as_bytes(), "").status_of(&p("f.rs")), Some(FileStatus::Conflict), "{code}");
        }
    }

    #[test]
    fn workspace_prefix_filters_and_strips() {
        let out = b"## main\0 M sub/a.rs\0 M other/b.rs\0";
        let st = RepoStatus::parse(out, "sub/");
        assert_eq!(st.status_of(&p("a.rs")), Some(FileStatus::Modified));
        assert_eq!(st.status_of(&p("other/b.rs")), None);
        assert_eq!(st.changed_files(), 1);
    }

    #[test]
    fn ignored_files_do_not_mark_parent() {
        let out = b"!! src/gen.rs\0";
        let st = RepoStatus::parse(out, "");
        assert_eq!(st.status_of(&p("src/gen.rs")), Some(FileStatus::Ignored));
        assert_eq!(st.status_of(&p("src")), None);
    }

    #[test]
    fn branch_forms() {
        let st = RepoStatus::parse(b"## No commits yet on main\0", "");
        assert_eq!(st.branch.as_deref(), Some("main"));
        let st = RepoStatus::parse(b"## HEAD (no branch)\0", "");
        assert_eq!(st.branch, None);
        let st = RepoStatus::parse(b"## feat/x...origin/feat/x\0", "");
        assert_eq!(st.branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn real_repository() {
        let has_git = Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success());
        if !has_git {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(root).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("tracked.txt"), "a").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("tracked.txt"), "b").unwrap();
        std::fs::write(root.join("new.txt"), "n").unwrap();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build/out"), "o").unwrap();

        let st = status(root).expect("is a repo");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.status_of(&p("tracked.txt")), Some(FileStatus::Modified));
        assert_eq!(st.status_of(&p("new.txt")), Some(FileStatus::Untracked));
        assert_eq!(st.status_of(&p("build/out")), Some(FileStatus::Ignored));
        assert_eq!(st.status_of(&p(".gitignore")), None);

        let not_repo = tempfile::tempdir().unwrap();
        assert!(status(not_repo.path()).is_none());
    }
}
