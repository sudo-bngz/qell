//! Merge conflicts: the `<<<<<<<` / `=======` / `>>>>>>>` blocks git leaves
//! in a file, and what each choice puts in their place.

/// One conflict block, by line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The `<<<<<<<` line.
    pub start: usize,
    /// The `|||||||` line (common ancestor, with `merge.conflictStyle=diff3`).
    pub base: Option<usize>,
    /// The `=======` line.
    pub mid: usize,
    /// The `>>>>>>>` line.
    pub end: usize,
    /// What follows the markers: `HEAD`, `Updated upstream`, a branch…
    pub ours: String,
    pub theirs: String,
}

/// What replaces a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Ours: the side above `=======` (what you had).
    Current,
    /// Theirs: the side below (what came in).
    Incoming,
    /// Ours then theirs.
    Both,
}

impl Conflict {
    /// Lines of our side.
    pub fn current(&self) -> std::ops::Range<usize> {
        self.start + 1..self.base.unwrap_or(self.mid)
    }

    /// Lines of their side.
    pub fn incoming(&self) -> std::ops::Range<usize> {
        self.mid + 1..self.end
    }

    /// The lines that replace the block (each without its line break).
    pub fn resolve<'a>(&self, lines: &[&'a str], choice: Choice) -> Vec<&'a str> {
        let pick = |r: std::ops::Range<usize>| lines[r].to_vec();
        match choice {
            Choice::Current => pick(self.current()),
            Choice::Incoming => pick(self.incoming()),
            Choice::Both => {
                let mut v = pick(self.current());
                v.extend(pick(self.incoming()));
                v
            }
        }
    }
}

fn marker<'a>(line: &'a str, m: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(m)?;
    // Exactly seven: an eighth `<` is something else.
    if rest.starts_with(&m[..1]) {
        return None;
    }
    Some(rest.trim())
}

/// The conflicts in a text, in order. Half-written blocks (no `=======` or
/// no `>>>>>>>`) are left out.
pub fn conflicts<S: AsRef<str>>(lines: impl Iterator<Item = S>) -> Vec<Conflict> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String, Option<usize>, Option<usize>)> = None;
    for (i, line) in lines.enumerate() {
        let line = line.as_ref().trim_end_matches(['\n', '\r']);
        if let Some(label) = marker(line, "<<<<<<<") {
            open = Some((i, label.to_string(), None, None));
        } else if let Some((_, _, base, mid)) = open.as_mut() {
            if mid.is_none() && base.is_none() && marker(line, "|||||||").is_some() {
                *base = Some(i);
            } else if mid.is_none() && line == "=======" {
                *mid = Some(i);
            } else if let Some(label) = marker(line, ">>>>>>>").filter(|_| mid.is_some()) {
                let (start, ours, base, mid) = open.take().expect("open");
                out.push(Conflict { start, base, mid: mid.expect("mid"), end: i, ours, theirs: label.to_string() });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> feat/cli\nb\n<<<<<<< Updated upstream\n||||||| base\nold\n=======\nnew\n>>>>>>> Stashed changes\n";

    #[test]
    fn finds_blocks_and_resolves_them() {
        let lines: Vec<&str> = TEXT.lines().collect();
        let c = conflicts(lines.iter());
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].start, c[0].mid, c[0].end, c[0].ours.as_str(), c[0].theirs.as_str()), (1, 4, 6, "HEAD", "feat/cli"));
        assert_eq!(c[0].resolve(&lines, Choice::Current), ["ours 1", "ours 2"]);
        assert_eq!(c[0].resolve(&lines, Choice::Incoming), ["theirs"]);
        assert_eq!(c[0].resolve(&lines, Choice::Both), ["ours 1", "ours 2", "theirs"]);
        // diff3: the base section is neither side.
        assert_eq!(c[1].base, Some(9));
        assert_eq!(c[1].resolve(&lines, Choice::Current), Vec::<&str>::new());
        assert_eq!(c[1].resolve(&lines, Choice::Incoming), ["new"]);
    }

    #[test]
    fn half_blocks_and_lookalikes_are_ignored() {
        assert!(conflicts("<<<<<<< HEAD\nx\n".lines()).is_empty());
        assert!(conflicts("<<<<<<<< not\n=======\n>>>>>>> x\n".lines()).is_empty());
        // A Markdown setext rule alone is not a conflict.
        assert!(conflicts("Title\n=======\n".lines()).is_empty());
    }
}
