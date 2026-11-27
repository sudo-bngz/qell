//! Branches and stashes: listing them, and switching, creating, deleting,
//! stashing and restoring.

use std::path::Path;

use crate::run;

/// A local or remote branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote one.
    pub name: String,
    pub remote: bool,
    /// The branch checked out.
    pub current: bool,
    /// What it tracks (`origin/main`), for a local branch.
    pub upstream: Option<String>,
    /// Date of its last commit, seconds since the epoch.
    pub time: i64,
}

/// One entry of `git stash list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// Position in the list: `stash@{index}`.
    pub index: usize,
    /// "WIP on main: 1a2b3c4 subject", or the message given.
    pub message: String,
    pub time: i64,
}

impl Stash {
    pub fn reference(&self) -> String {
        format!("stash@{{{}}}", self.index)
    }
}

/// Local branches (newest first), then remote ones (newest first), without
/// remotes' `HEAD`.
pub fn branches(dir: &Path) -> Result<Vec<Branch>, String> {
    let out = run(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(committerdate:unix)",
            "refs/heads",
            "refs/remotes",
        ],
        None,
    )?;
    Ok(parse_branches(&out))
}

pub fn parse_branches(out: &str) -> Vec<Branch> {
    let mut list: Vec<Branch> = out
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\0');
            let (full, short, head, upstream, time) = (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
            let remote = full.starts_with("refs/remotes/");
            if remote && full.ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                name: short.to_string(),
                remote,
                current: head == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                time: time.trim().parse().unwrap_or(0),
            })
        })
        .collect();
    list.sort_by(|a, b| a.remote.cmp(&b.remote).then(b.time.cmp(&a.time)).then_with(|| a.name.cmp(&b.name)));
    list
}

/// Whether `name` can be a branch name (`git check-ref-format`).
pub fn valid_branch_name(dir: &Path, name: &str) -> bool {
    !name.trim().is_empty() && run(dir, &["check-ref-format", "--branch", name], None).is_ok()
}

/// Check out a branch. A remote one (`origin/feature`) is checked out as
/// the local branch tracking it, made if needed.
pub fn switch(dir: &Path, branch: &Branch, locals: &[Branch]) -> Result<String, String> {
    if !branch.remote {
        return run(dir, &["switch", &branch.name], None);
    }
    let local = branch.name.split_once('/').map_or(branch.name.as_str(), |(_, rest)| rest);
    if locals.iter().any(|b| !b.remote && b.name == local) {
        run(dir, &["switch", local], None)
    } else {
        run(dir, &["switch", "--track", &branch.name], None)
    }
}

/// Make a branch from where you are and check it out.
pub fn create(dir: &Path, name: &str) -> Result<String, String> {
    run(dir, &["switch", "-c", name], None)
}

/// Delete a local branch; `force` even when it isn't merged.
pub fn delete(dir: &Path, name: &str, force: bool) -> Result<String, String> {
    run(dir, &["branch", if force { "-D" } else { "-d" }, name], None)
}

pub fn stashes(dir: &Path) -> Result<Vec<Stash>, String> {
    let out = run(dir, &["stash", "list", "--format=%gd%x00%s%x00%ct"], None)?;
    Ok(parse_stashes(&out))
}

pub fn parse_stashes(out: &str) -> Vec<Stash> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\0');
            let (reference, message, time) = (f.next()?, f.next()?, f.next()?);
            let index = reference.strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()?;
            Some(Stash { index, message: message.to_string(), time: time.trim().parse().unwrap_or(0) })
        })
        .collect()
}

/// Put the changes away. `untracked`: new files too.
pub fn stash(dir: &Path, message: &str, untracked: bool) -> Result<String, String> {
    let mut args = vec!["stash", "push"];
    if untracked {
        args.push("--include-untracked");
    }
    if !message.trim().is_empty() {
        args.extend(["--message", message.trim()]);
    }
    run(dir, &args, None)
}

/// Bring a stash back; `pop` also removes it from the list.
pub fn unstash(dir: &Path, stash: &Stash, pop: bool) -> Result<String, String> {
    run(dir, &["stash", if pop { "pop" } else { "apply" }, &stash.reference()], None)
}

pub fn drop_stash(dir: &Path, stash: &Stash) -> Result<String, String> {
    run(dir, &["stash", "drop", &stash.reference()], None)
}

/// The repository holding `dir` (its top folder).
pub fn toplevel(dir: &Path) -> Option<std::path::PathBuf> {
    let out = run(dir, &["rev-parse", "--show-toplevel"], None).ok()?;
    let root = std::path::PathBuf::from(out.trim());
    Some(std::fs::canonicalize(&root).unwrap_or(root))
}

/// The branch checked out (`None`: detached).
pub fn current_branch(dir: &Path) -> Option<String> {
    let out = run(dir, &["branch", "--show-current"], None).ok()?;
    let b = out.trim();
    (!b.is_empty()).then(|| b.to_string())
}

/// Your commits (by `user.email`) made between two times, on any branch:
/// `1a2b3c4 subject (branch)`, oldest first.
pub fn my_commits(dir: &Path, since: i64, until: i64) -> Vec<String> {
    let email = run(dir, &["config", "user.email"], None).map(|e| e.trim().to_string()).unwrap_or_default();
    let mut args = vec![
        "log".to_string(),
        "--all".into(),
        "--no-merges".into(),
        "--reverse".into(),
        format!("--since=@{since}"),
        format!("--until=@{until}"),
        "--pretty=format:%h %s%x00%D".into(),
    ];
    if !email.is_empty() {
        args.push(format!("--author={email}"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(dir, &args, None)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let (subject, refs) = l.split_once('\0').unwrap_or((l, ""));
            let branch = refs.split(", ").map(|r| r.trim_start_matches("HEAD -> ")).find(|r| !r.is_empty() && !r.contains('/'));
            match branch {
                Some(b) => format!("{subject} ({b})"),
                None => subject.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_list() {
        let out = "refs/heads/main\x00main\x00*\x00origin/main\x001700000000\n\
                   refs/heads/feat\x00feat\x00 \x00\x001700000500\n\
                   refs/remotes/origin/HEAD\x00origin\x00 \x00\x001700000000\n\
                   refs/remotes/origin/main\x00origin/main\x00 \x00\x001700000000\n";
        let b = parse_branches(out);
        let names: Vec<&str> = b.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["feat", "main", "origin/main"]);
        assert!(b[1].current && !b[0].current);
        assert_eq!(b[1].upstream.as_deref(), Some("origin/main"));
        assert!(b[2].remote);
    }

    #[test]
    fn stash_list() {
        let s = parse_stashes("stash@{0}\x00On main: wip\x001700000000\nstash@{1}\x00WIP on feat: abc x\x001600000000\n");
        assert_eq!(s.len(), 2);
        assert_eq!((s[1].index, s[1].reference()), (1, "stash@{1}".to_string()));
        assert_eq!(s[0].message, "On main: wip");
    }
}
