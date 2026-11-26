//! `git blame`: who last changed each line, and in which commit.

use std::collections::HashMap;
use std::path::Path;

/// A commit as blame shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    /// Seconds since the epoch.
    pub time: i64,
    pub summary: String,
}

impl Commit {
    /// Changes in the editor or the working tree that no commit has yet.
    pub fn uncommitted(&self) -> bool {
        self.sha.bytes().all(|b| b == b'0')
    }

    pub fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
}

/// The commit of each line of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blame {
    /// One entry per line: an index into `commits`.
    pub lines: Vec<usize>,
    pub commits: Vec<Commit>,
}

impl Blame {
    pub fn commit_of(&self, line: usize) -> Option<&Commit> {
        self.commits.get(*self.lines.get(line)?)
    }
}

/// Blame `rel` (as git spells it, from the repository's root) in the
/// repository at `root`. With `contents`, the lines are those of the text
/// in the editor, unsaved changes included, so they match what is shown.
pub fn blame(root: &Path, rel: &str, contents: Option<&str>) -> Result<Blame, String> {
    let mut args = vec!["blame", "--porcelain"];
    if contents.is_some() {
        args.extend(["--contents", "-"]);
    }
    args.extend(["--", rel]);
    let out = crate::run(root, &args, contents)?;
    Ok(parse(&out))
}

/// Read `git blame --porcelain`.
pub fn parse(out: &str) -> Blame {
    let mut blame = Blame::default();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut current: Option<usize> = None;
    let mut target_line = 0usize;
    for line in out.lines() {
        if let Some(_text) = line.strip_prefix('\t') {
            // The line itself ends a group.
            if let Some(c) = current {
                if blame.lines.len() <= target_line {
                    blame.lines.resize(target_line + 1, c);
                }
                blame.lines[target_line] = c;
            }
            continue;
        }
        let mut words = line.split(' ');
        let first = words.next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            // "<sha> <original line> <final line> [<lines in group>]"
            let final_line: usize = words.nth(1).and_then(|n| n.parse().ok()).unwrap_or(1);
            target_line = final_line.saturating_sub(1);
            let i = *index.entry(first.to_string()).or_insert_with(|| {
                blame.commits.push(Commit { sha: first.to_string(), author: String::new(), time: 0, summary: String::new() });
                blame.commits.len() - 1
            });
            current = Some(i);
            continue;
        }
        let Some(c) = current.and_then(|i| blame.commits.get_mut(i)) else { continue };
        let rest = line.split_once(' ').map_or("", |(_, r)| r);
        match first {
            "author" => c.author = rest.to_string(),
            "author-time" => c.time = rest.parse().unwrap_or(0),
            "summary" => c.summary = rest.to_string(),
            _ => {}
        }
    }
    blame
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "\
1111111111111111111111111111111111111111 1 1 2
author Nicolas
author-time 1795244645
summary feat: first
filename main.go
\tpackage main
1111111111111111111111111111111111111111 2 2
\t
0000000000000000000000000000000000000000 3 3 1
author Not Committed Yet
author-time 1795300000
summary Version of main.go from main.go
filename main.go
\tfunc main() {}
";

    #[test]
    fn each_line_gets_its_commit() {
        let b = parse(OUT);
        assert_eq!(b.lines, vec![0, 0, 1]);
        let first = b.commit_of(0).unwrap();
        assert_eq!((first.author.as_str(), first.time, first.summary.as_str()), ("Nicolas", 1_795_244_645, "feat: first"));
        assert_eq!(first.short(), "1111111");
        assert!(!first.uncommitted());
        assert!(b.commit_of(2).unwrap().uncommitted());
        assert!(b.commit_of(3).is_none());
    }
}
