//! Commit history and its graph layout, for the Source Control "Graph"
//! section.
//!
//! [`log`] reads commits in topological order; [`layout`] assigns each
//! commit a lane (column) and works out the line segments to draw in each
//! row, so the UI only paints what it is given.

use std::path::{Path, PathBuf};

/// One commit from `git log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    /// Ref names pointing here: `HEAD -> main`, `origin/main`, `tag: v1`.
    pub refs: Vec<String>,
    pub subject: String,
}

impl Commit {
    pub fn short_hash(&self) -> &str {
        &self.hash[..self.hash.len().min(7)]
    }
}

const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';
const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s%x1e";

/// Parse the output of `git log` with [`FORMAT`].
pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split(RECORD)
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let mut f = rec.split(FIELD);
            let hash = f.next().filter(|h| !h.is_empty())?.to_string();
            let parents = f.next()?.split_whitespace().map(String::from).collect();
            let author = f.next()?.to_string();
            let time = f.next()?.trim().parse().unwrap_or(0);
            let refs = f.next()?.split(", ").filter(|r| !r.is_empty()).map(String::from).collect();
            let subject = f.next().unwrap_or("").trim_end().to_string();
            Some(Commit { hash, parents, author, time, refs, subject })
        })
        .collect()
}

/// The last `limit` commits of HEAD and its upstream (so commits to pull
/// show too), newest first.
pub fn log(dir: &Path, limit: usize) -> Result<Vec<Commit>, String> {
    let n = format!("-n{limit}");
    let with_upstream = crate::run(dir, &["log", "--topo-order", &n, FORMAT, "HEAD", "@{upstream}"], None);
    let out = match with_upstream {
        Ok(out) => out,
        // No upstream (or no commits yet): HEAD alone.
        Err(_) => crate::run(dir, &["log", "--topo-order", &n, FORMAT, "HEAD"], None)?,
    };
    Ok(parse_log(&out))
}

/// A file changed by a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: crate::FileStatus,
    /// Relative to the repository's top level.
    pub path: PathBuf,
    /// The previous path, for renames and copies.
    pub old_path: Option<PathBuf>,
}

/// Parse `git diff-tree -z --name-status` output.
fn parse_name_status(out: &str) -> Vec<ChangedFile> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut files = Vec::new();
    while let Some(code) = fields.next() {
        let status = match code.chars().next() {
            Some('A') => crate::FileStatus::Added,
            Some('D') => crate::FileStatus::Deleted,
            Some('R') | Some('C') => crate::FileStatus::Renamed,
            Some('U') => crate::FileStatus::Conflict,
            _ => crate::FileStatus::Modified, // M, T
        };
        let first = fields.next().map(PathBuf::from);
        let (path, old_path) =
            if status == crate::FileStatus::Renamed { (fields.next().map(PathBuf::from), first) } else { (first, None) };
        if let Some(path) = path {
            files.push(ChangedFile { status, path, old_path });
        }
    }
    files
}

/// Files a commit changed: against its first parent (merges too, like
/// VS Code), or against nothing for a root commit.
pub fn changed_files(dir: &Path, commit: &Commit) -> Result<Vec<ChangedFile>, String> {
    let out = match commit.parents.first() {
        Some(parent) => crate::run(dir, &["diff-tree", "-r", "-M", "--name-status", "-z", parent, &commit.hash], None)?,
        None => crate::run(dir, &["diff-tree", "--root", "-r", "--no-commit-id", "--name-status", "-z", &commit.hash], None)?,
    };
    Ok(parse_name_status(&out))
}

/// What a line of an inline diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// A whole file with its changes inline: every line of the new version
/// plus the removed lines where they were, each with its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub text: String,
    pub kinds: Vec<LineKind>,
    pub added: usize,
    pub removed: usize,
}

/// Parse a unified diff with full context into an inline diff.
fn parse_full_diff(out: &str) -> Result<FileDiff, String> {
    let mut lines = Vec::new();
    let mut kinds = Vec::new();
    let mut in_hunk = false;
    for line in out.lines() {
        if !in_hunk {
            if line.starts_with("Binary files") {
                return Err("Binary file: no text diff.".into());
            }
            in_hunk = line.starts_with("@@");
            continue;
        }
        if line.starts_with("@@") || line.starts_with('\\') {
            continue; // another hunk header, "\ No newline at end of file"
        }
        let (kind, rest) = match line.chars().next() {
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some(' ') => (LineKind::Context, &line[1..]),
            _ => (LineKind::Context, line),
        };
        lines.push(rest);
        kinds.push(kind);
    }
    let added = kinds.iter().filter(|k| **k == LineKind::Added).count();
    let removed = kinds.iter().filter(|k| **k == LineKind::Removed).count();
    Ok(FileDiff { text: lines.join("\n"), kinds, added, removed })
}

/// One file's change in a commit, as an inline diff over the whole file.
pub fn file_diff(dir: &Path, commit: &Commit, file: &ChangedFile) -> Result<FileDiff, String> {
    // `:(top)` makes paths relative to the repository's top level, which
    // is what diff-tree reported, whatever folder `dir` is.
    let new = format!(":(top){}", file.path.display());
    let old = file.old_path.as_ref().map(|p| format!(":(top){}", p.display()));
    let mut args: Vec<&str> = vec!["-U100000000", "--no-color", "--no-ext-diff", "-M"];
    let out = match commit.parents.first() {
        Some(parent) => {
            let mut a = vec!["diff"];
            a.append(&mut args);
            a.extend([parent.as_str(), commit.hash.as_str(), "--", &new]);
            if let Some(o) = &old {
                a.push(o);
            }
            crate::run(dir, &a, None)?
        }
        None => {
            let mut a = vec!["show", "--format="];
            a.append(&mut args);
            a.extend([commit.hash.as_str(), "--", &new]);
            crate::run(dir, &a, None)?
        }
    };
    parse_full_diff(&out)
}

/// Where a line segment of a row sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// From the top edge of the row to its middle.
    Top,
    /// From the middle of the row to its bottom edge.
    Bottom,
}

/// A line segment in a row: from lane `from` to lane `to` (equal for a
/// straight line). `color` is the lane whose colour it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub half: Half,
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

/// How to draw one commit's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The commit's lane (where its dot goes).
    pub lane: usize,
    pub segments: Vec<Segment>,
    /// Number of lanes used in this row (for the graph's width).
    pub width: usize,
}

/// Lay out the graph of `commits` (newest first, topological order).
pub fn layout(commits: &[Commit]) -> Vec<Row> {
    // lanes[i]: the commit this lane is waiting for (going down).
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        let before = lanes.clone();
        let lane = match lanes.iter().position(|l| *l == Some(c.hash.as_str())) {
            Some(i) => i,
            None => match lanes.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };
        let mut segments = Vec::new();

        // Top half: lines arriving from the row above.
        for (j, waiting) in before.iter().enumerate() {
            match waiting {
                Some(h) if *h == c.hash => {
                    segments.push(Segment { half: Half::Top, from: j, to: lane, color: j });
                    lanes[j] = None; // several children may wait for one parent
                }
                Some(_) => segments.push(Segment { half: Half::Top, from: j, to: j, color: j }),
                None => {}
            }
        }

        // This lane now waits for the first parent; other parents get the
        // lane already waiting for them, or a new one.
        lanes[lane] = c.parents.first().map(String::as_str);
        let mut new_lanes = vec![lane];
        if !c.parents.is_empty() {
            segments.push(Segment { half: Half::Bottom, from: lane, to: lane, color: lane });
        }
        for p in c.parents.iter().skip(1) {
            let target = match lanes.iter().position(|l| *l == Some(p.as_str())) {
                Some(k) => k,
                None => {
                    let k = match lanes.iter().position(Option::is_none) {
                        Some(k) => k,
                        None => {
                            lanes.push(None);
                            lanes.len() - 1
                        }
                    };
                    lanes[k] = Some(p.as_str());
                    new_lanes.push(k);
                    k
                }
            };
            segments.push(Segment { half: Half::Bottom, from: lane, to: target, color: target });
        }

        // Bottom half: lines passing through to the row below.
        for (k, waiting) in lanes.iter().enumerate() {
            if waiting.is_some() && !new_lanes.contains(&k) {
                segments.push(Segment { half: Half::Bottom, from: k, to: k, color: k });
            }
        }

        let width = before.len().max(lanes.len()).max(lane + 1);
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        rows.push(Row { lane, segments, width });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: "a".into(),
            time: 0,
            refs: vec![],
            subject: hash.into(),
        }
    }

    fn has(row: &Row, half: Half, from: usize, to: usize) -> bool {
        row.segments.iter().any(|s| s.half == half && s.from == from && s.to == to)
    }

    #[test]
    fn linear_history_is_one_lane() {
        let commits = vec![c("c", &["b"]), c("b", &["a"]), c("a", &[])];
        let rows = layout(&commits);
        assert!(rows.iter().all(|r| r.lane == 0 && r.width == 1));
        assert!(!rows[0].segments.iter().any(|s| s.half == Half::Top), "tip has nothing above");
        assert!(has(&rows[1], Half::Top, 0, 0) && has(&rows[1], Half::Bottom, 0, 0));
        assert!(!rows[2].segments.iter().any(|s| s.half == Half::Bottom), "root has nothing below");
    }

    #[test]
    fn branch_and_merge() {
        //   m        merge of b2 into a2
        //   |\
        //   a2 b2
        //   |  b1
        //   | /
        //   a1
        let commits = vec![c("m", &["a2", "b2"]), c("b2", &["b1"]), c("b1", &["a1"]), c("a2", &["a1"]), c("a1", &[])];
        let rows = layout(&commits);
        assert_eq!(rows[0].lane, 0);
        assert!(has(&rows[0], Half::Bottom, 0, 0) && has(&rows[0], Half::Bottom, 0, 1), "merge forks into two lanes");
        assert_eq!(rows[1].lane, 1); // b2 on the second lane
        assert!(has(&rows[1], Half::Top, 0, 0), "a2's lane passes by");
        assert_eq!(rows[2].lane, 1); // b1
        assert_eq!(rows[3].lane, 0); // a2
        assert!(has(&rows[3], Half::Bottom, 1, 1), "b1's lane, waiting for a1, passes a2");
        // a1: both lanes join it.
        assert_eq!(rows[4].lane, 0);
        assert!(has(&rows[4], Half::Top, 0, 0) && has(&rows[4], Half::Top, 1, 0));
        assert_eq!(rows[4].width, 2);
    }

    #[test]
    fn parses_log_records() {
        let out = "aaaa\u{1f}bbbb cccc\u{1f}Nico\u{1f}1700000000\u{1f}HEAD -> main, origin/main, tag: v1\u{1f}feat: x\u{1e}\nbbbb\u{1f}\u{1f}Nico\u{1f}1690000000\u{1f}\u{1f}init\u{1e}\n";
        let log = parse_log(out);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents, vec!["bbbb", "cccc"]);
        assert_eq!(log[0].refs, vec!["HEAD -> main", "origin/main", "tag: v1"]);
        assert_eq!(log[0].subject, "feat: x");
        assert!(log[1].parents.is_empty() && log[1].refs.is_empty());
    }

    #[test]
    fn name_status_with_renames() {
        let out = "M\0src/a.rs\0R087\0old.txt\0new.txt\0A\0b.md\0D\0gone\0";
        let files = parse_name_status(out);
        use crate::FileStatus::*;
        assert_eq!(files.len(), 4);
        assert_eq!((files[0].status, files[0].path.to_str()), (Modified, Some("src/a.rs")));
        assert_eq!(
            (files[1].status, files[1].path.to_str(), files[1].old_path.as_deref().and_then(|p| p.to_str())),
            (Renamed, Some("new.txt"), Some("old.txt"))
        );
        assert_eq!(files[2].status, Added);
        assert_eq!(files[3].status, Deleted);
    }

    #[test]
    fn inline_diff_keeps_whole_file() {
        let out = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n\\ No newline at end of file\n";
        let d = parse_full_diff(out).unwrap();
        assert_eq!(d.text, "one\ntwo\nTWO\nthree");
        assert_eq!(d.kinds, vec![LineKind::Context, LineKind::Removed, LineKind::Added, LineKind::Context]);
        assert_eq!((d.added, d.removed), (1, 1));
        assert!(parse_full_diff("diff --git a/x b/x\nBinary files a/x and b/x differ\n").is_err());
    }

    #[test]
    fn real_log() {
        if std::process::Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let git = |args: &[&str]| assert!(crate::run(d, args, None).is_ok(), "{args:?}");
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "Tester"]);
        // No commits yet: an error, not a panic.
        assert!(log(d, 10).is_err());
        for i in 0..3 {
            std::fs::write(d.join("f"), i.to_string()).unwrap();
            git(&["add", "f"]);
            git(&["commit", "-q", "-m", &format!("commit {i}")]);
        }
        git(&["tag", "v1"]);
        let commits = log(d, 10).unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].subject, "commit 2");
        assert!(commits[0].refs.iter().any(|r| r == "HEAD -> main"));
        assert!(commits[0].refs.iter().any(|r| r == "tag: v1"));
        assert_eq!(commits[0].author, "Tester");
        assert_eq!(layout(&commits).iter().map(|r| r.lane).collect::<Vec<_>>(), vec![0, 0, 0]);

        // Files and diffs, including the root commit and a subfolder.
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/g.txt"), "x\ny\n").unwrap();
        std::fs::write(d.join("f"), "changed").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "more"]);
        let commits = log(d, 10).unwrap();
        let files = changed_files(d, &commits[0]).unwrap();
        assert_eq!(files.len(), 2);
        let g = files.iter().find(|f| f.path.to_str() == Some("sub/g.txt")).unwrap();
        assert_eq!(g.status, crate::FileStatus::Added);
        // Asked from the subfolder: paths are still repository-relative.
        let diff = file_diff(&d.join("sub"), &commits[0], g).unwrap();
        assert_eq!(diff.text, "x\ny");
        assert_eq!(diff.added, 2);
        let root = commits.last().unwrap();
        let root_files = changed_files(d, root).unwrap();
        assert_eq!(root_files[0].status, crate::FileStatus::Added);
        assert_eq!(file_diff(d, root, &root_files[0]).unwrap().text, "0");
    }
}
