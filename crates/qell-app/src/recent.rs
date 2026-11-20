//! Which files you open, so the file tree can offer a few shortcuts at the
//! top: handy when the workspace holds dozens of repositories.
//!
//! Kept in `~/.config/qell/recent.json`, per workspace, and saved after a
//! change (a few entries of JSON, written at most once a second).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What the shortcuts are sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Opened most, recent opens counting for more.
    MostUsed,
    /// Opened last.
    Recent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    /// How many times it was opened.
    #[serde(default)]
    pub opens: u32,
    /// When it was last opened (seconds since the epoch).
    #[serde(default)]
    pub last: u64,
    /// Kept at the top, whatever the order.
    #[serde(default)]
    pub pinned: bool,
    /// A folder rather than a file; worked out when the list is built.
    #[serde(skip)]
    pub is_dir: bool,
}

/// The shortcuts of one workspace: the folders you work in, then the files
/// you open.
#[derive(Debug, Clone, Default)]
pub struct Shortcuts {
    pub folders: Vec<Entry>,
    pub files: Vec<Entry>,
}

/// How many files are remembered per workspace.
const MAX_PER_ROOT: usize = 200;
/// An open counts half as much after this long.
const HALF_LIFE_DAYS: f64 = 10.0;

/// Whether a path is still there, and what it is, is looked up at most
/// this often: the sidebar asks for the list on every frame.
const RECHECK: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Recent {
    /// Entries by workspace root.
    #[serde(default)]
    roots: HashMap<PathBuf, Vec<Entry>>,
    #[serde(skip)]
    dirty: bool,
    /// The last list handed out, and what it was built from.
    #[serde(skip)]
    cache: Option<(PathBuf, Order, usize, Instant, Shortcuts)>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Opens, with older ones counting for less.
fn score(e: &Entry, now: u64) -> f64 {
    let age_days = now.saturating_sub(e.last) as f64 / 86_400.0;
    e.opens.max(1) as f64 * 0.5_f64.powf(age_days / HALF_LIFE_DAYS)
}

/// The folder directly below `root` that holds `path` (the repository, in
/// a workspace of several). `None` when the file sits in the workspace.
fn top_folder(root: &Path, path: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(root).ok()?;
    let first = rest.components().next()?;
    let folder = root.join(first.as_os_str());
    (folder != path).then_some(folder)
}

impl Recent {
    pub fn path() -> PathBuf {
        crate::config::config_dir().join("recent.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Write the file when something changed (called once a frame; cheap).
    pub fn save_if_dirty(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let path = Self::path();
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        match serde_json::to_string(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    log::warn!("cannot write {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("cannot encode recent files: {e}"),
        }
    }

    fn entries_mut(&mut self, root: &Path) -> &mut Vec<Entry> {
        self.roots.entry(root.to_path_buf()).or_default()
    }

    /// Note that `path` was opened in `root`. The folder it lives in
    /// directly below the workspace — the repository, when the workspace
    /// holds several — is credited too, so folders can be ranked as well.
    pub fn record(&mut self, root: &Path, path: &Path) {
        if !path.starts_with(root) || path == root {
            return;
        }
        if let Some(folder) = top_folder(root, path) {
            self.bump(root, &folder);
        }
        self.bump(root, path);
        self.cache = None;
    }

    fn bump(&mut self, root: &Path, path: &Path) {
        let now = now();
        let entries = self.entries_mut(root);
        match entries.iter_mut().find(|e| e.path == path) {
            Some(e) => {
                e.opens = e.opens.saturating_add(1);
                e.last = now;
            }
            None => entries.push(Entry { path: path.to_path_buf(), opens: 1, last: now, pinned: false, is_dir: false }),
        }
        // Forget the least interesting when the list grows too long.
        if entries.len() > MAX_PER_ROOT {
            entries.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(score(b, now).total_cmp(&score(a, now))));
            entries.truncate(MAX_PER_ROOT);
        }
        self.dirty = true;
    }

    /// The shortcuts to show, folders first: pinned entries, then `order`.
    /// `total` rows at most, with up to half of them folders.
    pub fn shortcuts(&mut self, root: &Path, order: Order, total: usize) -> Shortcuts {
        match &self.cache {
            Some((r, o, t, at, s)) if r == root && *o == order && *t == total && at.elapsed() < RECHECK => return s.clone(),
            _ => {}
        }
        let built = self.build(root, order, total);
        self.cache = Some((root.to_path_buf(), order, total, Instant::now(), built.clone()));
        built
    }

    fn build(&self, root: &Path, order: Order, total: usize) -> Shortcuts {
        let Some(entries) = self.roots.get(root) else { return Shortcuts::default() };
        let now = now();
        // One look at the disk per entry: gone, a file, or a folder.
        let mut v: Vec<Entry> = entries
            .iter()
            .filter_map(|e| {
                let dir = std::fs::metadata(&e.path).ok()?.is_dir();
                Some(Entry { is_dir: dir, ..e.clone() })
            })
            .collect();
        v.sort_by(|a, b| {
            b.pinned.cmp(&a.pinned).then(match order {
                Order::MostUsed => score(b, now).total_cmp(&score(a, now)),
                Order::Recent => b.last.cmp(&a.last),
            })
        });
        let (mut folders, mut files): (Vec<Entry>, Vec<Entry>) = v.into_iter().partition(|e| e.is_dir);
        // Folders first, since they are what you jump to; files take the rest.
        folders.truncate(total.div_ceil(2));
        files.truncate(total.saturating_sub(folders.len()));
        Shortcuts { folders, files }
    }

    pub fn is_pinned(&self, root: &Path, path: &Path) -> bool {
        self.roots.get(root).is_some_and(|v| v.iter().any(|e| e.path == path && e.pinned))
    }

    pub fn toggle_pin(&mut self, root: &Path, path: &Path) {
        if let Some(e) = self.entries_mut(root).iter_mut().find(|e| e.path == path) {
            e.pinned = !e.pinned;
            self.dirty = true;
            self.cache = None;
        }
    }

    /// Drop one entry (and any trace of a file that has gone).
    pub fn forget(&mut self, root: &Path, path: &Path) {
        self.entries_mut(root).retain(|e| e.path != path);
        self.dirty = true;
        self.cache = None;
    }

    pub fn clear(&mut self, root: &Path) {
        self.entries_mut(root).clear();
        self.dirty = true;
        self.cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, opens: u32, days_ago: u64) -> Entry {
        Entry { path: PathBuf::from(path), opens, last: now() - days_ago * 86_400, pinned: false, is_dir: false }
    }

    #[test]
    fn the_repository_folder_is_credited_too() {
        let root = Path::new("/ws");
        assert_eq!(top_folder(root, Path::new("/ws/repo/src/main.rs")), Some(PathBuf::from("/ws/repo")));
        assert_eq!(top_folder(root, Path::new("/ws/repo/main.tf")), Some(PathBuf::from("/ws/repo")));
        // A file sitting in the workspace itself has no folder of its own.
        assert_eq!(top_folder(root, Path::new("/ws/README.md")), None);
        assert_eq!(top_folder(root, Path::new("/elsewhere/x.rs")), None);
    }

    #[test]
    fn folders_come_first_and_share_the_rows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let (a, b) = (repo.join("src/main.rs"), repo.join("main.tf"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let mut r = Recent::default();
        r.record(root, &a);
        r.record(root, &b);

        let s = r.shortcuts(root, Order::MostUsed, 4);
        assert_eq!(s.folders.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![repo.clone()]);
        assert_eq!(s.files.len(), 2);
        assert!(s.folders[0].is_dir && !s.files[0].is_dir);
        // The folder was opened twice: once per file inside it.
        assert_eq!(s.folders[0].opens, 2);
        // Half the rows at most go to folders, and files take the rest.
        assert_eq!(r.shortcuts(root, Order::MostUsed, 2).files.len(), 1);
        // A folder that has gone is dropped.
        std::fs::remove_dir_all(&repo).unwrap();
        let s = r.shortcuts(root, Order::MostUsed, 4);
        assert!(s.folders.is_empty() && s.files.is_empty());
    }

    #[test]
    fn most_used_prefers_often_and_lately() {
        let now = now();
        // Opened twice today beats ten times a month ago.
        assert!(score(&entry("a", 2, 0), now) > score(&entry("b", 10, 30), now));
        // Same age: more opens wins.
        assert!(score(&entry("a", 5, 3), now) > score(&entry("b", 2, 3), now));
        // Same count: the newer one wins.
        assert!(score(&entry("a", 3, 1), now) > score(&entry("b", 3, 9), now));
    }

    #[test]
    fn records_opens_and_orders_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (a, b) = (root.join("a.rs"), root.join("b.rs"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let mut r = Recent::default();
        r.record(root, &a);
        r.record(root, &b);
        r.record(root, &b);
        // Outside the workspace, and the workspace itself: not recorded.
        r.record(root, Path::new("/elsewhere/x.rs"));
        r.record(root, root);

        let used: Vec<PathBuf> = r.shortcuts(root, Order::MostUsed, 5).files.into_iter().map(|e| e.path).collect();
        assert_eq!(used, vec![b.clone(), a.clone()]);
        // Pinned entries come first whatever the order.
        r.toggle_pin(root, &a);
        assert!(r.is_pinned(root, &a));
        assert_eq!(r.shortcuts(root, Order::MostUsed, 5).files[0].path, a);
        r.toggle_pin(root, &a);
        // A file that no longer exists isn't offered.
        std::fs::remove_file(&b).unwrap();
        assert_eq!(r.shortcuts(root, Order::Recent, 5).files.len(), 1);
        r.forget(root, &a);
        assert!(r.shortcuts(root, Order::Recent, 5).files.is_empty());
    }

    #[test]
    fn survives_a_round_trip() {
        let mut r = Recent::default();
        let root = Path::new("/ws");
        r.record(root, Path::new("/ws/a.rs"));
        let text = serde_json::to_string(&r).unwrap();
        let back: Recent = serde_json::from_str(&text).unwrap();
        assert_eq!(back.roots[root][0].opens, 1);
        // Missing fields and unknown roots don't break loading.
        let old: Recent = serde_json::from_str(r#"{"roots":{"/ws":[{"path":"/ws/x.rs"}]}}"#).unwrap();
        assert_eq!(old.roots[root][0].opens, 0);
    }
}
