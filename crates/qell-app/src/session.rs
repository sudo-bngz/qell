//! What was open last time: the tabs of each workspace, so reopening Qell
//! picks up where you left off.
//!
//! Kept in `~/.config/qell/session.json`, per workspace root, written when
//! the tabs change and when Qell closes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One remembered tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub path: PathBuf,
    /// Where the cursor was.
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub col: usize,
}

/// One remembered shell: where it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shell {
    pub cwd: PathBuf,
}

/// The tabs of one workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub tabs: Vec<Tab>,
    /// Which tab was in front.
    #[serde(default)]
    pub active: usize,
    /// The shells of the terminal panel, each started again in its folder.
    #[serde(default)]
    pub shells: Vec<Shell>,
    /// Which shell was in front.
    #[serde(default)]
    pub active_shell: usize,
    /// Whether the terminal panel was showing.
    #[serde(default)]
    pub panel: bool,
}

/// Shells remembered per workspace, at most.
const MAX_SHELLS: usize = 12;

/// Tabs remembered per workspace, at most this many each.
const MAX_TABS: usize = 60;
/// Workspaces remembered; the least recently saved is dropped.
const MAX_ROOTS: usize = 40;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    roots: HashMap<PathBuf, Workspace>,
    /// Roots in the order they were last saved, oldest first.
    #[serde(default)]
    order: Vec<PathBuf>,
}

impl Store {
    pub fn path() -> PathBuf {
        crate::config::config_dir().join("session.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// The tabs of a workspace, skipping files that have gone.
    pub fn take(&self, root: &Path) -> Workspace {
        let Some(ws) = self.roots.get(root) else { return Workspace::default() };
        let mut out = Workspace::default();
        for (i, tab) in ws.tabs.iter().enumerate() {
            if tab.path.is_file() {
                if i == ws.active {
                    out.active = out.tabs.len();
                }
                out.tabs.push(tab.clone());
            }
        }
        // A folder that has gone starts nowhere; the others keep their order.
        for (i, shell) in ws.shells.iter().enumerate() {
            if shell.cwd.is_dir() {
                if i == ws.active_shell {
                    out.active_shell = out.shells.len();
                }
                out.shells.push(shell.clone());
            }
        }
        out.panel = ws.panel && !out.shells.is_empty();
        out
    }

    /// Remember a workspace's tabs, and write the file.
    pub fn save(&mut self, root: &Path, mut ws: Workspace) {
        ws.tabs.truncate(MAX_TABS);
        ws.shells.truncate(MAX_SHELLS);
        ws.active_shell = ws.active_shell.min(ws.shells.len().saturating_sub(1));
        ws.active = ws.active.min(ws.tabs.len().saturating_sub(1));
        self.order.retain(|p| p != root);
        self.order.push(root.to_path_buf());
        self.roots.insert(root.to_path_buf(), ws);
        while self.order.len() > MAX_ROOTS {
            let old = self.order.remove(0);
            self.roots.remove(&old);
        }
        self.write();
    }

    fn write(&self) {
        let path = Self::path();
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        match serde_json::to_string(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    log::warn!("cannot write {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("cannot encode the session: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(path: &Path) -> Tab {
        Tab { path: path.to_path_buf(), line: 3, col: 7 }
    }

    #[test]
    fn shells_come_back_where_they_were_unless_the_folder_is_gone() {
        let dir = std::env::temp_dir().join(format!("qell-shells-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut store = Store::default();
        let shells = [a.clone(), dir.join("gone"), b.clone()].map(|cwd| Shell { cwd }).to_vec();
        store.roots.insert(dir.clone(), Workspace { shells, active_shell: 2, panel: true, ..Default::default() });
        let ws = store.take(&dir);
        assert_eq!(ws.shells, vec![Shell { cwd: a }, Shell { cwd: b }]);
        // The one in front is still the one in front, after the gap closes.
        assert_eq!(ws.active_shell, 1);
        assert!(ws.panel);
        std::fs::remove_dir_all(&dir).unwrap();
        // No shells to show: the panel stays shut.
        let none = store.take(&dir);
        assert!(none.shells.is_empty() && !none.panel);
    }

    #[test]
    fn keeps_tabs_per_workspace_and_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let (root, other) = (dir.path().join("ws"), dir.path().join("other"));
        std::fs::create_dir_all(&root).unwrap();
        let (a, b) = (root.join("a.rs"), root.join("b.rs"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();

        let mut s = Store::default();
        s.save(&root, Workspace { tabs: vec![tab(&a), tab(&b)], active: 1, ..Default::default() });
        s.save(&other, Workspace { tabs: vec![tab(&a)], active: 0, ..Default::default() });
        assert_eq!(s.take(&root).tabs.len(), 2);
        assert_eq!(s.take(&root).active, 1);
        assert_eq!(s.take(&other).tabs.len(), 1);
        assert!(s.take(Path::new("/never/seen")).tabs.is_empty());

        // A file deleted since: dropped, and the active tab follows.
        std::fs::remove_file(&a).unwrap();
        let ws = s.take(&root);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].path, b);
        assert_eq!((ws.active, ws.tabs[0].line, ws.tabs[0].col), (0, 3, 7));
    }

    #[test]
    fn written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("a.rs");
        std::fs::write(&a, "").unwrap();
        let mut s = Store::default();
        s.save(root, Workspace { tabs: vec![tab(&a)], active: 0, ..Default::default() });
        let text = serde_json::to_string(&s).unwrap();
        let back: Store = serde_json::from_str(&text).unwrap();
        assert_eq!(back.take(root).tabs, vec![tab(&a)]);
        // A file from an older version, without cursor fields.
        let old: Store =
            serde_json::from_str(&format!(r#"{{"roots":{{"{}":{{"tabs":[{{"path":"{}"}}]}}}}}}"#, root.display(), a.display()))
                .unwrap();
        assert_eq!(old.take(root).tabs[0].line, 0);
    }

    #[test]
    fn forgets_the_oldest_workspaces() {
        let mut s = Store::default();
        for i in 0..MAX_ROOTS + 5 {
            s.roots.insert(PathBuf::from(format!("/ws{i}")), Workspace::default());
            s.order.push(PathBuf::from(format!("/ws{i}")));
        }
        s.save(Path::new("/ws-new"), Workspace::default());
        assert_eq!(s.roots.len(), MAX_ROOTS);
        assert!(s.roots.contains_key(Path::new("/ws-new")));
        assert!(!s.roots.contains_key(Path::new("/ws0")));
    }
}
