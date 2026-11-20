//! Commit history and its graph layout, for the Source Control "Graph"
//! section.
//!
//! [`log`] reads commits in topological order; [`layout`] assigns each
//! commit a lane (column) and works out the line segments to draw in each
//! row, so the UI only paints what it is given.

use std::path::Path;

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
    }
}
