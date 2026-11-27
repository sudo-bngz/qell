//! When something goes wrong: a crash report, and unsaved work that
//! survives.
//!
//! - A panic anywhere writes `<data>/qell/crash/crash-<time>.log` (what,
//!   where, the backtrace, Qell's version). Qell says so at the next start.
//! - A panic while drawing a frame doesn't close the window: the frame is
//!   dropped, you're told, and Qell goes on. If it keeps happening (three
//!   times in half a minute) it stops, after saving the backups below.
//! - Every few seconds, the text of each tab with unsaved changes is
//!   copied to `<data>/qell/backup/<workspace>/`. A normal quit removes the
//!   copies; after a crash, the next start puts them back in their tabs,
//!   still unsaved, for you to look at and save.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How often unsaved tabs are copied.
const BACKUP_EVERY: Duration = Duration::from_secs(5);

pub fn data() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("qell")
}

pub fn crash_dir() -> PathBuf {
    data().join("crash")
}

/// FNV-1a: a short, stable name for a path.
pub fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Where the backups of a workspace go.
pub fn backup_dir(root: &Path) -> PathBuf {
    data().join("backup").join(fnv(&root.to_string_lossy()))
}

/// The last panic's message (for the frame that caught it).
static LAST_PANIC: Mutex<Option<(String, PathBuf)>> = Mutex::new(None);

/// Write a crash report for every panic, then let the default handler
/// print it as usual.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let report = format!(
            "Qell {} on {} {}\nthread '{thread}' panicked at {at}:\n{what}\n\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::backtrace::Backtrace::force_capture(),
        );
        let dir = crash_dir();
        let path = dir.join(format!("crash-{}.log", crate::activity::now()));
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(&path, report);
        }
        if let Ok(mut last) = LAST_PANIC.lock() {
            *last = Some((format!("{what} ({at})"), path));
        }
        default(info);
    }));
}

/// Debug builds: a panic on demand, to try all this. With
/// `QELL_DEBUG_PANIC_FILE=/some/path`, creating that file makes the next
/// frame panic (and removes it).
pub fn debug_panic() {
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("QELL_DEBUG_PANIC_FILE") {
        if std::fs::remove_file(&p).is_ok() {
            panic!("debug panic");
        }
    }
}

/// The report of the last panic, taken.
pub fn take_last_panic() -> Option<(String, PathBuf)> {
    LAST_PANIC.lock().ok()?.take()
}

/// Crash reports written since the last time Qell looked, newest first.
pub fn new_crash_reports() -> Vec<PathBuf> {
    let dir = crash_dir();
    let seen_file = dir.join("seen");
    let seen: i64 = std::fs::read_to_string(&seen_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut found: Vec<(i64, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let t: i64 = name.strip_prefix("crash-")?.strip_suffix(".log")?.parse().ok()?;
            (t > seen).then(|| (t, e.path()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    if let Some((t, _)) = found.first() {
        let _ = std::fs::write(&seen_file, t.to_string());
    }
    found.into_iter().map(|(_, p)| p).collect()
}

/// A tab's unsaved text, kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    /// The file (`None`: an untitled tab).
    pub path: Option<PathBuf>,
    pub text: String,
    pub time: i64,
}

/// Panics caught while drawing, to give up on a frame that always fails.
#[derive(Default)]
pub struct Guard {
    recent: Vec<Instant>,
    last_backup: Option<Instant>,
    /// Backup file name → buffer version written.
    written: std::collections::HashMap<String, i32>,
}

impl Guard {
    /// Note a caught panic; `true` when it keeps happening.
    pub fn caught(&mut self) -> bool {
        self.recent.retain(|t| t.elapsed() < Duration::from_secs(30));
        self.recent.push(Instant::now());
        self.recent.len() >= 3
    }
}

/// The backups of a workspace.
pub fn backups(root: &Path) -> Vec<(PathBuf, Backup)> {
    std::fs::read_dir(backup_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let b: Backup = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some((e.path(), b))
        })
        .collect()
}

impl crate::app::QellApp {
    /// Copy the unsaved tabs (every few seconds, or now with `force`), and
    /// remove the copies of tabs saved or closed since.
    pub fn backup_unsaved(&mut self, force: bool) {
        if !force && self.guard.last_backup.is_some_and(|t| t.elapsed() < BACKUP_EVERY) {
            return;
        }
        self.guard.last_backup = Some(Instant::now());
        let dir = backup_dir(&self.root);
        let mut keep = std::collections::HashSet::new();
        let mut untitled = 0;
        for doc in &self.docs {
            if !doc.buffer.is_dirty() || doc.read_only || doc.media.is_some() {
                continue;
            }
            let name = match doc.buffer.path() {
                Some(p) => format!("{}.json", fnv(&p.to_string_lossy())),
                None => {
                    untitled += 1;
                    format!("untitled-{untitled}.json")
                }
            };
            keep.insert(name.clone());
            let version = doc.buffer.version();
            if self.guard.written.get(&name) == Some(&version) {
                continue;
            }
            let b =
                Backup { path: doc.buffer.path().map(Path::to_path_buf), text: doc.buffer.text(), time: crate::activity::now() };
            if std::fs::create_dir_all(&dir).is_ok() {
                if let Ok(json) = serde_json::to_string(&b) {
                    let tmp = dir.join(format!("{name}.tmp"));
                    if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, dir.join(&name)).is_ok() {
                        self.guard.written.insert(name, version);
                    }
                }
            }
        }
        // Saved or closed since: their copy goes.
        let stale: Vec<String> = self.guard.written.keys().filter(|k| !keep.contains(*k)).cloned().collect();
        for name in stale {
            let _ = std::fs::remove_file(dir.join(&name));
            self.guard.written.remove(&name);
        }
    }

    /// A normal quit: nothing to recover.
    pub fn drop_backups(&mut self) {
        let _ = std::fs::remove_dir_all(backup_dir(&self.root));
        self.guard.written.clear();
    }

    /// At start: put back what a crash left unsaved, and say a crash
    /// happened.
    pub fn recover(&mut self) {
        let reports = new_crash_reports();
        let mut restored = 0;
        for (file, b) in backups(&self.root) {
            let _ = std::fs::remove_file(&file);
            if self.restore_backup(b) {
                restored += 1;
            }
        }
        match (reports.first(), restored) {
            (Some(r), 0) => self.notify_error(format!(
                "Qell crashed last time. The report is in {} — please send it with an issue.",
                r.display()
            )),
            (Some(r), n) => self.notify_error(format!(
                "Qell crashed last time: {n} unsaved file{} put back, still unsaved. The report is in {}.",
                if n == 1 { "" } else { "s" },
                r.display()
            )),
            (None, 0) => {}
            (None, n) => {
                self.notify(format!("{n} unsaved file{} from last time put back, still unsaved.", if n == 1 { "" } else { "s" }))
            }
        }
        if restored > 0 {
            // Copied again under the same names.
            self.backup_unsaved(true);
        }
    }

    /// One backup back in a tab: the file's tab (opened if needed) gets
    /// the text, as an unsaved change. `false` when there was nothing to
    /// put back.
    fn restore_backup(&mut self, b: Backup) -> bool {
        use qell_core::Position;
        let i = match &b.path {
            Some(path) => match self.docs.iter().position(|d| d.buffer.path() == Some(path.as_path())) {
                Some(i) => i,
                None => match crate::document::Document::open(path.clone()) {
                    Ok(doc) => {
                        self.docs.push(doc);
                        self.docs.len() - 1
                    }
                    // Deleted since: a tab with its name and the text.
                    Err(_) => {
                        let mut doc = crate::document::Document::untitled();
                        doc.buffer = qell_core::Buffer::from_text("", Some(path.clone()));
                        self.docs.push(doc);
                        self.docs.len() - 1
                    }
                },
            },
            None => {
                self.docs.push(crate::document::Document::untitled());
                self.docs.len() - 1
            }
        };
        let doc = &mut self.docs[i];
        if doc.buffer.text() == b.text {
            return false;
        }
        let last = doc.buffer.line_count().saturating_sub(1);
        let end = Position::new(last, doc.buffer.line_len(last));
        doc.buffer.replace_range(Position::new(0, 0), end, &b.text);
        doc.buffer.set_cursor(Position::new(0, 0), false);
        let mut i = i;
        // A single empty untitled tab from the start is replaced.
        let blank =
            |d: &crate::document::Document| d.buffer.path().is_none() && d.buffer.text().is_empty() && !d.buffer.is_dirty();
        if i > 0 && blank(&self.docs[0]) {
            self.docs.remove(0);
            i -= 1;
        }
        self.active = i;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stable() {
        assert_eq!(fnv("/src/qell"), fnv("/src/qell"));
        assert_ne!(fnv("/src/qell"), fnv("/src/qelm"));
        assert_eq!(fnv("").len(), 16);
    }

    #[test]
    fn three_panics_in_a_row_give_up() {
        let mut g = Guard::default();
        assert!(!g.caught());
        assert!(!g.caught());
        assert!(g.caught());
    }

    #[test]
    fn backups_round_trip() {
        let b = Backup { path: Some("/x/a.rs".into()), text: "fn main() {}\n".into(), time: 7 };
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<Backup>(&json).unwrap(), b);
    }
}
