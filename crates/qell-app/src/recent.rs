//! Which files you open, so the file tree can offer a few shortcuts at the
//! top: handy when the workspace holds dozens of repositories.
//!
//! Kept in `~/.config/qell/recent.json`, per workspace, and saved after a
//! change (a few entries of JSON, written at most once a second).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
}

/// How many files are remembered per workspace.
const MAX_PER_ROOT: usize = 200;
/// An open counts half as much after this long.
const HALF_LIFE_DAYS: f64 = 10.0;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Recent {
    /// Entries by workspace root.
    #[serde(default)]
    roots: HashMap<PathBuf, Vec<Entry>>,
    #[serde(skip)]
    dirty: bool,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Opens, with older ones counting for less.
fn score(e: &Entry, now: u64) -> f64 {
    let age_days = now.saturating_sub(e.last) as f64 / 86_400.0;
    e.opens.max(1) as f64 * 0.5_f64.powf(age_days / HALF_LIFE_DAYS)
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

    /// Note that `path` was opened in `root`.
    pub fn record(&mut self, root: &Path, path: &Path) {
        if !path.starts_with(root) || path == root {
            return;
        }
        let now = now();
        let entries = self.entries_mut(root);
        match entries.iter_mut().find(|e| e.path == path) {
            Some(e) => {
                e.opens = e.opens.saturating_add(1);
                e.last = now;
            }
            None => entries.push(Entry { path: path.to_path_buf(), opens: 1, last: now, pinned: false }),
        }
        // Forget the least interesting when the list grows too long.
        if entries.len() > MAX_PER_ROOT {
            entries.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(score(b, now).total_cmp(&score(a, now))));
            entries.truncate(MAX_PER_ROOT);
        }
        self.dirty = true;
    }

    /// The shortcuts to show: pinned first, then by `order`.
    pub fn top(&self, root: &Path, order: Order, limit: usize) -> Vec<Entry> {
        let Some(entries) = self.roots.get(root) else { return Vec::new() };
        let now = now();
        let mut v: Vec<Entry> = entries.iter().filter(|e| e.path.exists()).cloned().collect();
        v.sort_by(|a, b| {
            b.pinned.cmp(&a.pinned).then(match order {
                Order::MostUsed => score(b, now).total_cmp(&score(a, now)),
                Order::Recent => b.last.cmp(&a.last),
            })
        });
        v.truncate(limit);
        v
    }

    pub fn is_pinned(&self, root: &Path, path: &Path) -> bool {
        self.roots.get(root).is_some_and(|v| v.iter().any(|e| e.path == path && e.pinned))
    }

    pub fn toggle_pin(&mut self, root: &Path, path: &Path) {
        if let Some(e) = self.entries_mut(root).iter_mut().find(|e| e.path == path) {
            e.pinned = !e.pinned;
            self.dirty = true;
        }
    }

    /// Drop one entry (and any trace of a file that has gone).
    pub fn forget(&mut self, root: &Path, path: &Path) {
        self.entries_mut(root).retain(|e| e.path != path);
        self.dirty = true;
    }

    pub fn clear(&mut self, root: &Path) {
        self.entries_mut(root).clear();
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, opens: u32, days_ago: u64) -> Entry {
        Entry { path: PathBuf::from(path), opens, last: now() - days_ago * 86_400, pinned: false }
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

        let used: Vec<PathBuf> = r.top(root, Order::MostUsed, 5).into_iter().map(|e| e.path).collect();
        assert_eq!(used, vec![b.clone(), a.clone()]);
        // Pinned entries come first whatever the order.
        r.toggle_pin(root, &a);
        assert!(r.is_pinned(root, &a));
        assert_eq!(r.top(root, Order::MostUsed, 5)[0].path, a);
        r.toggle_pin(root, &a);
        // A file that no longer exists isn't offered.
        std::fs::remove_file(&b).unwrap();
        assert_eq!(r.top(root, Order::Recent, 5).len(), 1);
        r.forget(root, &a);
        assert!(r.top(root, Order::Recent, 5).is_empty());
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
