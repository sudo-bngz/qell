//! Git integration.
//!
//! Qell shells out to the `git` binary instead of linking libgit2: it is
//! always the user's own git, with their config, hooks and credential
//! helpers, and it keeps the build free of C dependencies. Status runs on a
//! background thread ([`Watcher`]) so a slow repository never stalls a frame.
//!
//! A workspace can be inside one repository, or be a plain folder holding
//! several (`~/code/{api,web,…}`), or both (a repo with other repos cloned
//! inside it). [`discover`] finds the repositories, [`RepoStatus`] is one
//! repository's `git status`, and [`WorkspaceStatus`] merges them so every
//! path takes its state from the repository it belongs to. All paths are
//! relative to the *workspace* root.

pub mod graph;

pub mod blame;
pub mod refs;

use std::collections::{HashMap, HashSet};
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

/// A changed file as the Source Control view shows it: its state in the
/// index (staged) and in the working tree (not staged yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to where `git status` was run (see [`RepoStatus::parse`]).
    pub path: PathBuf,
    pub staged: Option<FileStatus>,
    pub unstaged: Option<FileStatus>,
}

/// Index (`X`) side of a porcelain code.
fn staged_status(x: u8) -> Option<FileStatus> {
    Some(match x {
        b'M' | b'T' => FileStatus::Modified,
        b'A' => FileStatus::Added,
        b'D' => FileStatus::Deleted,
        b'R' | b'C' => FileStatus::Renamed,
        _ => return None,
    })
}

/// Working-tree (`Y`) side of a porcelain code.
fn unstaged_status(y: u8) -> Option<FileStatus> {
    Some(match y {
        b'M' | b'T' => FileStatus::Modified,
        b'D' => FileStatus::Deleted,
        b'A' => FileStatus::Added,
        _ => return None,
    })
}

/// One repository's `git status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Current branch, or a short commit hash when detached.
    pub branch: Option<String>,
    /// Commits ahead of / behind the upstream, when there is one.
    pub ahead: u32,
    pub behind: u32,
    /// Changed files, relative to where `git status` was asked from.
    files: Vec<(PathBuf, FileStatus)>,
    /// The same files split into staged / unstaged, for Source Control
    /// (ignored files left out).
    pub changes: Vec<Change>,
}

impl RepoStatus {
    pub fn staged(&self) -> impl Iterator<Item = (&Path, FileStatus)> {
        self.changes.iter().filter_map(|c| c.staged.map(|s| (c.path.as_path(), s)))
    }

    pub fn unstaged(&self) -> impl Iterator<Item = (&Path, FileStatus)> {
        self.changes.iter().filter_map(|c| c.unstaged.map(|s| (c.path.as_path(), s)))
    }

    /// Number of changed files (excluding ignored ones).
    pub fn changed_files(&self) -> usize {
        self.files.iter().filter(|(_, s)| *s != FileStatus::Ignored).count()
    }

    /// Parse `git status --porcelain=v1 -z --branch --ignored` output.
    ///
    /// `prefix` is the path git was run from inside the repository (from
    /// `git rev-parse --show-prefix`, e.g. `"sub/dir/"`); entries outside it
    /// are dropped and the rest are made relative to it.
    pub fn parse(output: &[u8], prefix: &str) -> Self {
        let prefix = Path::new(prefix.trim_end_matches('/'));
        let mut status = RepoStatus::default();

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
            status.files.push((rel.to_path_buf(), st));
            let change = match st {
                FileStatus::Ignored => None,
                FileStatus::Untracked => Some((None, Some(FileStatus::Untracked))),
                FileStatus::Conflict => Some((None, Some(FileStatus::Conflict))),
                _ => Some((staged_status(x), unstaged_status(y))),
            };
            if let Some((staged, unstaged)) = change {
                status.changes.push(Change { path: rel.to_path_buf(), staged, unstaged });
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

/// The git state of a whole workspace: every repository in it, merged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceStatus {
    /// Repositories by workspace-relative root, outermost first. An empty
    /// path is a repository containing the workspace.
    pub repos: Vec<(PathBuf, RepoStatus)>,
    /// Files and directories with a status. Directories hold the strongest
    /// status found beneath them.
    entries: HashMap<PathBuf, FileStatus>,
    /// Folders git itself reported as untracked or ignored (`?? dir/`,
    /// `!! dir/`): everything inside them shares that state. Folders only
    /// coloured because of a file inside them are not in here.
    inherited: HashMap<PathBuf, FileStatus>,
    /// Roots of repositories nested in the workspace (not the empty path).
    nested_roots: HashSet<PathBuf>,
    /// More repositories were found than are followed.
    pub truncated: bool,
}

impl WorkspaceStatus {
    /// Paths with a status (for the memory report).
    pub fn entry_count(&self) -> usize {
        self.entries.len() + self.inherited.len()
    }
}

impl WorkspaceStatus {
    /// Merge per-repository statuses. Each status's paths are relative to
    /// its own root, given alongside it relative to the workspace.
    pub fn merge(mut repos: Vec<(PathBuf, RepoStatus)>) -> Self {
        repos.sort_by_key(|(root, _)| root.components().count());
        let nested_roots: HashSet<PathBuf> = repos.iter().map(|(r, _)| r.clone()).filter(|r| !r.as_os_str().is_empty()).collect();

        // Every changed file in workspace coordinates. A repository sees a
        // repository nested in it as an untracked (or ignored) folder; that
        // entry is dropped, the nested repository speaks for its own files.
        let mut files: Vec<(PathBuf, FileStatus)> = Vec::new();
        for (root, st) in &repos {
            for (p, s) in &st.files {
                let path = root.join(p);
                let inside_nested = path
                    .ancestors()
                    .take_while(|a| a.starts_with(root) && *a != root.as_path())
                    .any(|a| nested_roots.contains(a));
                if !inside_nested {
                    files.push((path, *s));
                }
            }
        }

        let inherited: HashMap<PathBuf, FileStatus> = files.iter().filter(|(_, s)| s.is_inherited()).cloned().collect();
        let mut entries: HashMap<PathBuf, FileStatus> = HashMap::new();
        for (path, st) in files {
            entries.entry(path.clone()).and_modify(|e| *e = (*e).max(st)).or_insert(st);
            // Ignored files don't make their folder look changed.
            if st == FileStatus::Ignored {
                continue;
            }
            // Folders up to the workspace root show it, across repositories:
            // a folder of repos lights up when one of them has changes.
            for dir in path.ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()) {
                // A deleted file is gone from the tree; show its folder as modified.
                let dir_st = if st == FileStatus::Deleted { FileStatus::Modified } else { st };
                let e = entries.entry(dir.to_path_buf()).or_insert(dir_st);
                if *e == FileStatus::Ignored || dir_st > *e {
                    *e = dir_st;
                }
            }
        }
        Self { repos, entries, inherited, nested_roots, truncated: false }
    }

    /// Status of a workspace-relative path, including states inherited from
    /// an untracked or ignored ancestor folder of the same repository.
    pub fn status_of(&self, rel: &Path) -> Option<FileStatus> {
        if let Some(s) = self.entries.get(rel) {
            return Some(*s);
        }
        for a in rel.ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()) {
            if let Some(s) = self.inherited.get(a).copied() {
                return Some(s);
            }
            // Don't inherit across a repository boundary: a clean nested
            // repo inside an ignored folder is not itself ignored.
            if self.nested_roots.contains(a) {
                return None;
            }
        }
        None
    }

    /// The repository a workspace-relative path belongs to (the innermost
    /// one containing it).
    pub fn repo_for(&self, rel: &Path) -> Option<&(PathBuf, RepoStatus)> {
        self.repos.iter().rev().find(|(root, _)| rel.starts_with(root))
    }

    /// Changed files across all repositories (excluding ignored ones).
    pub fn changed_files(&self) -> usize {
        self.repos.iter().map(|(_, s)| s.changed_files()).sum()
    }

    /// What git reports as changed, workspace-relative: files, and the
    /// untracked folders it names as a whole. Ignored paths are left out.
    pub fn changed_paths(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.repos
            .iter()
            .flat_map(|(root, st)| st.files.iter().filter(|(_, s)| *s != FileStatus::Ignored).map(move |(p, _)| root.join(p)))
    }
}

/// Run a git command in `dir` (stdin from `input` when given). Returns
/// stdout, or git's own error message.
pub fn run(dir: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd.stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
        Err(err.trim().to_string())
    }
}

/// The repository holding `path`, and `path` relative to its root (as
/// git spells it, with forward slashes). `None` outside a repository.
pub fn repo_of(path: &Path) -> Option<(PathBuf, String)> {
    // Both sides are resolved first: git prints the real path, and on
    // macOS a temporary folder is reached through a symlink.
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = path.parent()?;
    let root = run(dir, &["rev-parse", "--show-toplevel"], None).ok()?;
    let root = PathBuf::from(root.trim());
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let rel = path.strip_prefix(&root).ok()?;
    let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
    (!rel.is_empty()).then_some((root, rel))
}

/// A unified diff turning the `base` lines into the `new` ones, for one
/// run of changed lines. No context lines, so it is applied with
/// `--unidiff-zero`.
pub fn unified_patch(
    rel: &str,
    base: &str,
    new: &str,
    base_range: std::ops::Range<usize>,
    new_range: std::ops::Range<usize>,
) -> String {
    let b: Vec<&str> = base.split_inclusive('\n').collect();
    let n: Vec<&str> = new.split_inclusive('\n').collect();
    let (blen, nlen) = (base_range.len(), new_range.len());
    // An empty side points at the line it comes after, not at a line.
    let bs = if blen == 0 { base_range.start } else { base_range.start + 1 };
    let ns = if nlen == 0 { new_range.start } else { new_range.start + 1 };
    let mut out = format!("--- a/{rel}\n+++ b/{rel}\n@@ -{bs},{blen} +{ns},{nlen} @@\n");
    let mut push = |sign: char, lines: &[&str]| {
        for line in lines {
            out.push(sign);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    };
    push('-', b.get(base_range).unwrap_or_default());
    push('+', n.get(new_range).unwrap_or_default());
    out
}

/// Put `patch` into the index, leaving the working tree alone.
pub fn apply_cached(root: &Path, patch: &str) -> Result<(), String> {
    run(root, &["apply", "--cached", "--unidiff-zero", "-"], Some(patch)).map(|_| ())
}

/// `git add` these paths (relative to `dir`).
pub fn stage(dir: &Path, paths: &[&Path]) -> Result<(), String> {
    let mut args = vec!["add", "--all", "--"];
    let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    args.extend(paths.iter().map(String::as_str));
    run(dir, &args, None).map(drop)
}

/// Take these paths back out of the index, keeping the working tree.
/// `git reset` works before the first commit too, unlike `restore --staged`.
pub fn unstage(dir: &Path, paths: &[&Path]) -> Result<(), String> {
    let mut args = vec!["reset", "-q", "--"];
    let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    args.extend(paths.iter().map(String::as_str));
    run(dir, &args, None).map(drop)
}

/// Commit what is staged, with the user's hooks and signing config.
pub fn commit(dir: &Path, message: &str) -> Result<String, String> {
    run(dir, &["commit", "--file", "-"], Some(message))
}

/// A file's text as git has it (the index: the last commit plus anything
/// staged), to show what changed since. `None` when the file isn't in a
/// repository, isn't tracked, or isn't text.
pub fn index_text(path: &Path) -> Option<String> {
    let (dir, name) = (path.parent()?, path.file_name()?.to_str()?);
    let out = git(dir).args(["show", &format!(":./{name}")]).stderr(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
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

/// Run `git status` from `dir`. `None` when it isn't inside a repository
/// or git isn't installed.
pub fn status(dir: &Path) -> Option<RepoStatus> {
    let out = git(dir).args(["rev-parse", "--show-prefix"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let out = git(dir)
        .args(["status", "--porcelain=v1", "-z", "--branch", "--ignored=matching", "--untracked-files=normal"])
        .output()
        .ok()?;
    if !out.status.success() {
        log::warn!("git status failed in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr).trim());
        return None;
    }
    let mut st = RepoStatus::parse(&out.stdout, &prefix);
    if st.branch.is_none() {
        // Detached HEAD: show the short hash instead.
        if let Ok(o) = git(dir).args(["rev-parse", "--short", "HEAD"]).output() {
            if o.status.success() {
                st.branch = Some(String::from_utf8_lossy(&o.stdout).trim().to_string());
            }
        }
    }
    Some(st)
}

/// How far below the workspace root to look for repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// Folder levels to search (0: only the workspace's own repository).
    pub depth: usize,
    /// Folder names never searched (`node_modules`, `target`…).
    pub skip: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { depth: 3, skip: vec!["node_modules".into(), "target".into(), ".terraform".into(), "vendor".into()] }
    }
}

/// Most repositories followed at once. Past a handful they are refreshed
/// a few at a time in turn (see [`Watcher`]), so this only bounds memory
/// and the length of a sweep.
const MAX_REPOS: usize = 512;

/// Folders of a home directory that hold caches, downloaded dependencies
/// and app data rather than anybody's code. When a workspace contains
/// the home directory, they are not searched for repositories nor
/// indexed; they still show in the file tree.
const HOME_NOISE: &[&str] = &[
    "Library",
    "Applications",
    "Movies",
    "Music",
    "Pictures",
    ".Trash",
    ".cache",
    ".local",
    ".npm",
    ".nvm",
    ".pnpm-store",
    ".yarn",
    ".bun",
    ".cargo",
    ".rustup",
    ".gradle",
    ".m2",
    ".docker",
    ".orbstack",
    ".colima",
    ".vscode",
    ".vscode-oss",
    ".cursor",
    ".oh-my-zsh",
    ".terraform.d",
    ".kube/cache",
    ".minikube",
    ".pyenv",
    ".rbenv",
    ".sdkman",
    ".tfenv",
    ".asdf",
    "go/pkg",
    "snap",
];

/// Whether `path` is one of the home directory's [`HOME_NOISE`] folders.
pub fn is_home_noise(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return false };
    is_noise_of(&home, path)
}

fn is_noise_of(home: &Path, path: &Path) -> bool {
    path.strip_prefix(home).is_ok_and(|rel| HOME_NOISE.iter().any(|n| rel == Path::new(n)))
}

/// Find the repositories of a workspace: the one containing `root` (as an
/// empty relative path), then repositories in folders below it, as paths
/// relative to `root`. A folder with a `.git` entry (directory, or file for
/// worktrees and submodules) is a repository.
pub fn discover(root: &Path, opts: &ScanOptions) -> Vec<PathBuf> {
    discover_capped(root, opts).0
}

/// [`discover`], and whether it stopped at [`MAX_REPOS`].
fn discover_capped(root: &Path, opts: &ScanOptions) -> (Vec<PathBuf>, bool) {
    let mut found = Vec::new();
    let enclosing = git(root).args(["rev-parse", "--is-inside-work-tree"]).output();
    if enclosing.is_ok_and(|o| o.status.success()) {
        found.push(PathBuf::new());
    }
    let mut frontier = vec![PathBuf::new()];
    for _ in 0..opts.depth {
        let mut next = Vec::new();
        for rel in frontier {
            let Ok(rd) = std::fs::read_dir(root.join(&rel)) else { continue };
            let mut dirs: Vec<_> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).collect();
            dirs.sort_by_key(|e| e.file_name());
            for e in dirs {
                let name = e.file_name().to_string_lossy().into_owned();
                if name == ".git" || opts.skip.contains(&name) || is_home_noise(&e.path()) {
                    continue;
                }
                let child = rel.join(&name);
                if e.path().join(".git").exists() {
                    if found.len() >= MAX_REPOS {
                        return (found, true);
                    }
                    found.push(child.clone());
                }
                // Keep going inside repos too: clones nested in a repo exist.
                next.push(child);
            }
        }
        frontier = next;
    }
    (found, false)
}

/// Most `git status` processes run at once.
const PARALLEL: usize = 4;

/// `git status` for every repository, a few at a time. `None` when there
/// is no repository at all.
pub fn workspace_status(root: &Path, repos: &[PathBuf]) -> Option<WorkspaceStatus> {
    let results: Vec<(PathBuf, RepoStatus)> =
        statuses(root, repos).into_iter().filter_map(|(rel, st)| Some((rel, st?))).collect();
    (!results.is_empty()).then(|| WorkspaceStatus::merge(results))
}

/// `git status` of each repository, a few at a time; `None` where git
/// failed (a broken or half-cloned repository).
fn statuses(root: &Path, repos: &[PathBuf]) -> Vec<(PathBuf, Option<RepoStatus>)> {
    if repos.is_empty() {
        return Vec::new();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    thread::scope(|s| {
        let workers: Vec<_> = (0..PARALLEL.min(repos.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(rel) = repos.get(i) else { break };
                        out.push((rel.clone(), status(&root.join(rel))));
                    }
                    out
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    })
}

/// Up to this many repositories, every one is refreshed each time.
const ALL_AT_ONCE: usize = 32;
/// Past that, this many in turn each time, plus the ones with changes
/// and the ones asked about.
const PER_ROUND: usize = 16;

/// Which repositories to ask `git status` this time: all of a small
/// workspace; in a large one, those asked about (`hot`), those with
/// changes (`dirty`, where work is going on), those never looked at yet
/// (not in `seen`: the first look covers everything) and the next few in
/// turn from `cursor`, which moves on.
fn round(
    repos: &[PathBuf],
    hot: &HashSet<PathBuf>,
    dirty: &HashSet<PathBuf>,
    seen: &dyn Fn(&PathBuf) -> bool,
    cursor: &mut usize,
) -> Vec<PathBuf> {
    if repos.len() <= ALL_AT_ONCE {
        return repos.to_vec();
    }
    let mut pick: Vec<PathBuf> = repos.iter().filter(|r| hot.contains(*r) || dirty.contains(*r) || !seen(r)).cloned().collect();
    let start = *cursor % repos.len();
    for r in repos.iter().cycle().skip(start).take(PER_ROUND) {
        if !pick.contains(r) {
            pick.push(r.clone());
        }
    }
    *cursor = (start + PER_ROUND) % repos.len();
    pick
}

/// The repository of a workspace-relative path: the deepest root holding it.
fn repo_of_rel<'a>(repos: &'a [PathBuf], rel: &Path) -> Option<&'a PathBuf> {
    repos.iter().filter(|r| rel.starts_with(r)).max_by_key(|r| r.components().count())
}

/// Time between refreshes: 3 s for a handful of repositories, longer for
/// many, so a folder of 60 clones doesn't run git non-stop.
fn interval_for(repos: usize) -> Duration {
    let rounds = repos.div_ceil(PARALLEL * 2).max(1) as u32;
    (INTERVAL * rounds).min(MAX_INTERVAL)
}

enum Request {
    /// Look again; with a workspace-relative path, that file's
    /// repository is refreshed for sure.
    Refresh(Option<PathBuf>),
    SetRoot(PathBuf),
    SetOptions(ScanOptions),
}

/// Keeps a [`WorkspaceStatus`] up to date on a background thread: refreshes
/// on demand and every few seconds, and hands results back through [`poll`].
///
/// A workspace of many repositories isn't asked about all of them every
/// time: the ones with changes, the ones a save or a refresh pointed at,
/// and a few others in turn, so each is looked at within a minute or so
/// while git never runs more than a handful of statuses at once.
///
/// [`poll`]: Watcher::poll
pub struct Watcher {
    tx: Sender<Request>,
    rx: Receiver<Option<WorkspaceStatus>>,
}

/// How often the status is refreshed when nothing asks for it.
const INTERVAL: Duration = Duration::from_secs(3);
const MAX_INTERVAL: Duration = Duration::from_secs(20);
/// Repositories are looked for again every this many refreshes (and on
/// every explicit refresh), so a new clone shows up on its own.
const REDISCOVER_EVERY: u32 = 10;

impl Watcher {
    pub fn new(root: PathBuf, opts: ScanOptions) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (res_tx, res_rx) = mpsc::channel();
        thread::Builder::new()
            .name("qell-git".into())
            .spawn(move || {
                let (mut root, mut opts) = (root, opts);
                let mut last: Option<Option<WorkspaceStatus>> = None;
                let mut repos: Option<Vec<PathBuf>> = None;
                let mut truncated = false;
                // Said once per workspace, not on every look.
                let mut warned = false;
                let mut known: HashMap<PathBuf, RepoStatus> = HashMap::new();
                let mut failed: HashSet<PathBuf> = HashSet::new();
                let mut hot: HashSet<PathBuf> = HashSet::new();
                let mut hot_paths: Vec<PathBuf> = Vec::new();
                let mut cursor = 0usize;
                let mut cycles = 0u32;
                loop {
                    if repos.is_none() || cycles.is_multiple_of(REDISCOVER_EVERY) {
                        let (found, more) = discover_capped(&root, &opts);
                        if more && !warned {
                            log::warn!(
                                "more than {MAX_REPOS} git repositories under {}; following the first ones",
                                root.display()
                            );
                            warned = true;
                        }
                        known.retain(|r, _| found.contains(r));
                        failed.retain(|r| found.contains(r));
                        repos = Some(found);
                        truncated = more;
                    }
                    cycles = cycles.wrapping_add(1);
                    let all = repos.as_deref().unwrap_or_default();
                    hot.extend(hot_paths.drain(..).filter_map(|p| repo_of_rel(all, &p).cloned()));
                    let dirty: HashSet<PathBuf> =
                        known.iter().filter(|(_, s)| s.changed_files() > 0).map(|(r, _)| r.clone()).collect();
                    let batch = round(all, &hot, &dirty, &|r| known.contains_key(r) || failed.contains(r), &mut cursor);
                    hot.clear();
                    for (rel, st) in statuses(&root, &batch) {
                        match st {
                            Some(st) => {
                                failed.remove(&rel);
                                known.insert(rel, st);
                            }
                            // Looked at, but git failed: not asked again
                            // until its turn comes.
                            None => {
                                known.remove(&rel);
                                failed.insert(rel);
                            }
                        }
                    }
                    let merged: Vec<(PathBuf, RepoStatus)> =
                        all.iter().filter_map(|r| Some((r.clone(), known.get(r)?.clone()))).collect();
                    let st = (!merged.is_empty()).then(|| WorkspaceStatus { truncated, ..WorkspaceStatus::merge(merged) });
                    // Only wake the UI when something changed.
                    if last.as_ref() != Some(&st) {
                        if res_tx.send(st.clone()).is_err() {
                            return;
                        }
                        last = Some(st);
                    }
                    let wait = interval_for(batch.len());
                    let first = match req_rx.recv_timeout(wait) {
                        Ok(r) => Some(r),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    // Coalesce a burst of requests into one status run.
                    for req in first.into_iter().chain(req_rx.try_iter()) {
                        match req {
                            Request::Refresh(path) => {
                                repos = None;
                                hot_paths.extend(path);
                            }
                            Request::SetRoot(r) => {
                                root = r;
                                repos = None;
                                last = None;
                                known.clear();
                                failed.clear();
                                warned = false;
                                cursor = 0;
                            }
                            Request::SetOptions(o) => {
                                opts = o;
                                repos = None;
                            }
                        }
                    }
                }
            })
            .expect("spawn git thread");
        Self { tx: req_tx, rx: res_rx }
    }

    /// Ask for a status run now (after a refresh, regaining focus).
    pub fn refresh(&self) {
        let _ = self.tx.send(Request::Refresh(None));
    }

    /// Ask for a status run now that surely covers the repository of
    /// `rel` (workspace-relative), as after saving that file.
    pub fn refresh_path(&self, rel: PathBuf) {
        let _ = self.tx.send(Request::Refresh(Some(rel)));
    }

    /// Switch to another workspace.
    pub fn set_root(&self, root: PathBuf) {
        let _ = self.tx.send(Request::SetRoot(root));
    }

    /// Change how deep repositories are looked for.
    pub fn set_options(&self, opts: ScanOptions) {
        let _ = self.tx.send(Request::SetOptions(opts));
    }

    /// The newest status, if one arrived since the last call. The outer
    /// `Option` is "is there news", the inner one "is there any repository".
    pub fn poll(&self) -> Option<Option<WorkspaceStatus>> {
        self.rx.try_iter().last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// A single repository at the workspace root.
    fn single(out: &[u8], prefix: &str) -> WorkspaceStatus {
        WorkspaceStatus::merge(vec![(PathBuf::new(), RepoStatus::parse(out, prefix))])
    }

    #[test]
    fn parses_files_and_propagates_to_dirs() {
        let out =
            b"## main...origin/main [ahead 2, behind 1]\0 M src/app.rs\0?? notes/\0A  src/new.rs\0!! target/\0 D old/gone.rs\0";
        let ws = single(out, "");
        let st = &ws.repos[0].1;
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(ws.status_of(&p("src/app.rs")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("src/new.rs")), Some(FileStatus::Added));
        // Strongest child wins.
        assert_eq!(ws.status_of(&p("src")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("old")), Some(FileStatus::Modified));
        // Inherited from the untracked / ignored directory.
        assert_eq!(ws.status_of(&p("notes/todo.md")), Some(FileStatus::Untracked));
        assert_eq!(ws.status_of(&p("target/debug/qell")), Some(FileStatus::Ignored));
        assert_eq!(ws.status_of(&p("src/lib.rs")), None);
        assert_eq!(ws.status_of(&p("README.md")), None);
    }

    #[test]
    fn renames_skip_the_original_path() {
        let ws = single(b"## main\0R  b.rs\0a.rs\0 M c.rs\0", "");
        assert_eq!(ws.status_of(&p("b.rs")), Some(FileStatus::Renamed));
        assert_eq!(ws.status_of(&p("a.rs")), None);
        assert_eq!(ws.status_of(&p("c.rs")), Some(FileStatus::Modified));
    }

    #[test]
    fn conflicts() {
        for code in ["UU", "AA", "DD", "AU", "UD"] {
            let out = format!("{code} f.rs\0");
            assert_eq!(single(out.as_bytes(), "").status_of(&p("f.rs")), Some(FileStatus::Conflict), "{code}");
        }
    }

    #[test]
    fn workspace_prefix_filters_and_strips() {
        let ws = single(b"## main\0 M sub/a.rs\0 M other/b.rs\0", "sub/");
        assert_eq!(ws.status_of(&p("a.rs")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("other/b.rs")), None);
        assert_eq!(ws.changed_files(), 1);
    }

    #[test]
    fn clean_files_next_to_a_new_file_stay_clean() {
        // src/ is green because of new.rs, but lib.rs is still tracked and clean.
        let ws = single(b"?? src/new.rs\0", "");
        assert_eq!(ws.status_of(&p("src")), Some(FileStatus::Untracked));
        assert_eq!(ws.status_of(&p("src/new.rs")), Some(FileStatus::Untracked));
        assert_eq!(ws.status_of(&p("src/lib.rs")), None);
    }

    #[test]
    fn ignored_files_do_not_mark_parent() {
        let ws = single(b"!! src/gen.rs\0", "");
        assert_eq!(ws.status_of(&p("src/gen.rs")), Some(FileStatus::Ignored));
        assert_eq!(ws.status_of(&p("src")), None);
    }

    #[test]
    fn staged_and_unstaged_are_split() {
        let st = RepoStatus::parse(b"## main\0MM both.rs\0M  staged.rs\0 M work.rs\0A  new.rs\0?? untracked.rs\0UU conflict.rs\0!! ignored.rs\0 D gone.rs\0", "");
        let staged: Vec<_> = st.staged().map(|(p, s)| (p.to_str().unwrap(), s)).collect();
        let unstaged: Vec<_> = st.unstaged().map(|(p, s)| (p.to_str().unwrap(), s)).collect();
        use FileStatus::*;
        assert_eq!(staged, vec![("both.rs", Modified), ("staged.rs", Modified), ("new.rs", Added)]);
        assert_eq!(
            unstaged,
            vec![
                ("both.rs", Modified),
                ("work.rs", Modified),
                ("untracked.rs", Untracked),
                ("conflict.rs", Conflict),
                ("gone.rs", Deleted)
            ]
        );
    }

    #[test]
    fn real_stage_unstage_commit() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::write(root.join("a.txt"), "a").unwrap();
        // Before the first commit: stage, unstage, stage again.
        stage(root, &[Path::new("a.txt")]).unwrap();
        assert_eq!(status(root).unwrap().staged().count(), 1);
        unstage(root, &[Path::new("a.txt")]).unwrap();
        assert_eq!(status(root).unwrap().staged().count(), 0);
        stage(root, &[Path::new("a.txt")]).unwrap();
        commit(root, "first\n\nwith a body").unwrap();
        let st = status(root).unwrap();
        assert!(st.changes.is_empty());
        let log = run(root, &["log", "--format=%B", "-1"], None).unwrap();
        assert_eq!(log.trim(), "first\n\nwith a body");
        // A deleted file stages as a deletion.
        std::fs::remove_file(root.join("a.txt")).unwrap();
        stage(root, &[Path::new("a.txt")]).unwrap();
        assert_eq!(status(root).unwrap().staged().next().map(|(_, s)| s), Some(FileStatus::Deleted));
        // Errors come back as git's message.
        assert!(commit(root, "").unwrap_err().to_lowercase().contains("message"));
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
    fn refresh_slows_down_with_many_repositories() {
        assert_eq!(interval_for(0), INTERVAL);
        assert_eq!(interval_for(5), INTERVAL);
        assert!(interval_for(30) > INTERVAL);
        assert_eq!(interval_for(64), MAX_INTERVAL);
    }

    #[test]
    fn folder_of_repositories() {
        // Workspace is not a repo; api/ and tools/cli/ are.
        let api = RepoStatus::parse(b"## main\0 M README.md\0", "");
        let cli = RepoStatus::parse(b"## dev\0?? new.go\0", "");
        let ws = WorkspaceStatus::merge(vec![(p("tools/cli"), cli), (p("api"), api)]);
        assert_eq!(ws.status_of(&p("api/README.md")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("api")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("tools/cli/new.go")), Some(FileStatus::Untracked));
        // Folders above a repository light up too.
        assert_eq!(ws.status_of(&p("tools")), Some(FileStatus::Untracked));
        assert_eq!(ws.status_of(&p("api/src/main.go")), None);
        assert_eq!(ws.repo_for(&p("tools/cli/new.go")).and_then(|(_, r)| r.branch.as_deref()), Some("dev"));
        assert_eq!(ws.repo_for(&p("api/x")).map(|(r, _)| r.as_path()), Some(Path::new("api")));
        assert!(ws.repo_for(&p("elsewhere")).is_none());
        assert_eq!(ws.changed_files(), 2);
    }

    #[test]
    fn repository_inside_a_repository() {
        // The outer repo sees the clone as an untracked (or ignored) folder;
        // the clone's own status must win.
        let outer = RepoStatus::parse(b"## main\0?? vendor/lib/\0!! clones/\0 M top.txt\0", "");
        let lib = RepoStatus::parse(b"## main\0 M src/a.rs\0", "");
        let tool = RepoStatus::parse(b"## main\0", "");
        let ws = WorkspaceStatus::merge(vec![(p(""), outer), (p("vendor/lib"), lib), (p("clones/tool"), tool)]);
        assert_eq!(ws.status_of(&p("vendor/lib/src/a.rs")), Some(FileStatus::Modified));
        // Clean file of the nested repo: not "untracked" from the outer one.
        assert_eq!(ws.status_of(&p("vendor/lib/README.md")), None);
        assert_eq!(ws.status_of(&p("vendor/lib")), Some(FileStatus::Modified));
        // A clean repo inside an ignored folder isn't greyed out.
        assert_eq!(ws.status_of(&p("clones/tool/main.go")), None);
        assert_eq!(ws.status_of(&p("clones/other.txt")), Some(FileStatus::Ignored));
        assert_eq!(ws.status_of(&p("top.txt")), Some(FileStatus::Modified));
    }

    #[test]
    fn staging_one_hunk_leaves_the_rest_alone() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        commit_all(tmp.path());
        let new = "ONE\ntwo\nTHREE\n";
        std::fs::write(&file, new).unwrap();
        let (root, rel) = repo_of(&file).unwrap();
        assert_eq!(rel, "a.txt");
        // Only the first line.
        let patch = unified_patch(&rel, "one\ntwo\nthree\n", new, 0..1, 0..1);
        apply_cached(&root, &patch).unwrap();
        assert_eq!(index_text(&file).as_deref(), Some("ONE\ntwo\nthree\n"));
    }

    #[test]
    fn patches_for_added_and_deleted_lines() {
        // Two lines added after the first.
        let p = unified_patch("a.txt", "one\n", "one\nx\ny\n", 1..1, 1..3);
        assert!(p.contains("@@ -1,0 +2,2 @@\n+x\n+y\n"), "{p}");
        // The second line deleted.
        let p = unified_patch("a.txt", "one\ntwo\n", "one\n", 1..2, 1..1);
        assert!(p.contains("@@ -2,1 +1,0 @@\n-two\n"), "{p}");
        // A last line without its newline is marked as such.
        let p = unified_patch("a.txt", "one\n", "two", 0..1, 0..1);
        assert!(p.ends_with("+two\n\\ No newline at end of file\n"), "{p}");
    }

    #[test]
    fn index_text_is_the_committed_version() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sub");
        init_repo(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        commit_all(tmp.path());
        std::fs::write(dir.join("a.txt"), "one\nchanged\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        assert_eq!(index_text(&dir.join("a.txt")).as_deref(), Some("one\ntwo\n"));
        assert_eq!(index_text(&dir.join("new.txt")), None); // untracked
        assert_eq!(index_text(Path::new("/nonexistent/x")), None);
    }

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
            assert!(Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success());
        }
    }

    fn commit_all(dir: &Path) {
        for args in [&["add", "."][..], &["commit", "-q", "-m", "init"]] {
            assert!(Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success());
        }
    }

    #[test]
    fn branches_and_stashes_in_a_real_repository() {
        use crate::refs;
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        commit_all(root);
        assert!(refs::valid_branch_name(root, "feat/x"));
        assert!(!refs::valid_branch_name(root, "bad..name"));
        refs::create(root, "feat/x").unwrap();
        let b = refs::branches(root).unwrap();
        assert_eq!(b.iter().find(|b| b.current).map(|b| b.name.as_str()), Some("feat/x"));
        // Stash a change, see it listed, bring it back.
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        refs::stash(root, "half done", false).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
        let s = refs::stashes(root).unwrap();
        assert!(s[0].message.contains("half done"));
        refs::unstash(root, &s[0], true).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "two\n");
        assert!(refs::stashes(root).unwrap().is_empty());
        // Back to main, then the branch goes.
        let main = b.iter().find(|b| b.name == "main").unwrap().clone();
        refs::stash(root, "", false).unwrap();
        refs::switch(root, &main, &b).unwrap();
        refs::delete(root, "feat/x", false).unwrap();
        assert!(refs::branches(root).unwrap().iter().all(|b| b.name != "feat/x"));
    }

    #[test]
    fn todays_commits() {
        use crate::refs;
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        commit_all(root);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let c = refs::my_commits(root, now - 3600, now + 60);
        assert_eq!(c.len(), 1);
        assert!(c[0].ends_with("init (main)"), "{c:?}");
        assert!(refs::my_commits(root, now - 7200, now - 3600).is_empty());
        assert_eq!(refs::current_branch(root).as_deref(), Some("main"));
        assert!(refs::toplevel(&root.join(".")).is_some());
    }

    #[test]
    fn blame_follows_unsaved_text() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        commit_all(root);
        let b = crate::blame::blame(root, "a.txt", Some("one\nnew\ntwo\n")).unwrap();
        assert_eq!(b.lines.len(), 3);
        let c = |l| b.commit_of(l).unwrap();
        assert_eq!((c(0).author.as_str(), c(0).summary.as_str()), ("t", "init"));
        assert!(c(1).uncommitted());
        assert_eq!(c(2).sha, c(0).sha);
        assert!(crate::blame::blame(root, "missing.txt", None).is_err());
    }

    #[test]
    fn real_repository() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        std::fs::write(root.join("tracked.txt"), "a").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        commit_all(root);
        std::fs::write(root.join("tracked.txt"), "b").unwrap();
        std::fs::write(root.join("new.txt"), "n").unwrap();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build/out"), "o").unwrap();

        let repos = discover(root, &ScanOptions::default());
        assert_eq!(repos, vec![PathBuf::new()]);
        let ws = workspace_status(root, &repos).expect("is a repo");
        assert_eq!(ws.repos[0].1.branch.as_deref(), Some("main"));
        assert_eq!(ws.status_of(&p("tracked.txt")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("new.txt")), Some(FileStatus::Untracked));
        assert_eq!(ws.status_of(&p("build/out")), Some(FileStatus::Ignored));
        assert_eq!(ws.status_of(&p(".gitignore")), None);

        let not_repo = tempfile::tempdir().unwrap();
        assert!(discover(not_repo.path(), &ScanOptions::default()).is_empty());
        assert!(workspace_status(not_repo.path(), &[]).is_none());
    }

    #[test]
    fn a_home_directory_s_caches_are_noise_but_not_what_is_in_them_by_name() {
        let home = Path::new("/Users/nico");
        assert!(is_noise_of(home, &home.join("Library")));
        assert!(is_noise_of(home, &home.join("go/pkg")));
        assert!(is_noise_of(home, &home.join(".cargo")));
        // Only those very folders: a project's own Library or pkg is code.
        assert!(!is_noise_of(home, &home.join("REPOS/app/Library")));
        assert!(!is_noise_of(home, &home.join("go/src")));
        assert!(!is_noise_of(home, &home.join("go")));
        assert!(!is_noise_of(home, Path::new("/Library")));
    }

    #[test]
    fn many_repositories_are_looked_at_in_turn() {
        let repos: Vec<PathBuf> = (0..100).map(|i| p(&format!("r{i:03}"))).collect();
        let none = HashSet::new();
        let all_seen = |_: &PathBuf| true;
        // A small workspace: everything, every time.
        let mut cursor = 0;
        assert_eq!(round(&repos[..20], &none, &none, &all_seen, &mut cursor).len(), 20);
        // The first look at a large one covers every repository too.
        assert_eq!(round(&repos, &none, &none, &|_| false, &mut cursor).len(), repos.len());
        // A large one: a slice at a time, moving on, wrapping around.
        let mut seen = HashSet::new();
        let mut cursor = 0;
        for _ in 0..repos.len().div_ceil(PER_ROUND) {
            let batch = round(&repos, &none, &none, &all_seen, &mut cursor);
            assert_eq!(batch.len(), PER_ROUND);
            seen.extend(batch);
        }
        assert_eq!(seen.len(), repos.len(), "every repository is reached within one sweep");
        // The ones with changes and the one just saved in come every time.
        let dirty = HashSet::from([p("r050")]);
        let hot = HashSet::from([p("r099")]);
        let batch = round(&repos, &hot, &dirty, &all_seen, &mut cursor);
        assert!(batch.contains(&p("r050")) && batch.contains(&p("r099")));
        assert_eq!(batch.len(), PER_ROUND + 2);
        // A file is credited to the deepest repository holding it.
        let nested = [p("api"), p("api/vendor/lib")];
        assert_eq!(repo_of_rel(&nested, Path::new("api/vendor/lib/x.go")), Some(&p("api/vendor/lib")));
        assert_eq!(repo_of_rel(&nested, Path::new("api/main.go")), Some(&p("api")));
        assert_eq!(repo_of_rel(&nested, Path::new("web/x")), None);
    }

    #[test]
    fn real_folder_of_repositories() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for r in ["api", "web", "tools/cli", "node_modules/pkg"] {
            init_repo(&root.join(r));
            std::fs::write(root.join(r).join("README.md"), "a").unwrap();
            commit_all(&root.join(r));
        }
        std::fs::write(root.join("api/README.md"), "changed").unwrap();
        std::fs::write(root.join("web/new.txt"), "n").unwrap();

        let repos = discover(root, &ScanOptions::default());
        assert_eq!(repos, vec![p("api"), p("web"), p("tools/cli")], "node_modules is skipped");
        let shallow = discover(root, &ScanOptions { depth: 1, ..Default::default() });
        assert_eq!(shallow, vec![p("api"), p("web")]);

        let ws = workspace_status(root, &repos).expect("has repos");
        assert_eq!(ws.status_of(&p("api/README.md")), Some(FileStatus::Modified));
        assert_eq!(ws.status_of(&p("web/new.txt")), Some(FileStatus::Untracked));
        assert_eq!(ws.status_of(&p("web/README.md")), None);
        assert_eq!(ws.status_of(&p("api")), Some(FileStatus::Modified));
    }
}
