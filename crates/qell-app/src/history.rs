//! Local history: a copy of a file each time Qell saves it, kept on this
//! machine, to look back at or restore an earlier version — git or not.
//!
//! Each file has a folder under `<data>/qell/history/<fnv(path)>/` holding
//! a `path` file (which file this is) and one `<unix ms>.snap` per version.
//! The version on disk before Qell first wrote over it is kept too, so the
//! history starts where your edits did. Identical versions aren't repeated.
//! Folders are pruned by age and count; nothing ever leaves the machine.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::HistoryConfig;

pub fn dir() -> PathBuf {
    crate::recovery::data().join("history")
}

fn file_dir(root: &Path, path: &Path) -> PathBuf {
    root.join(crate::recovery::fnv(&path.to_string_lossy()))
}

/// One kept version of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// When it was written (or, for the version found on disk, when that
    /// was last changed).
    pub time: SystemTime,
    /// Where the copy is.
    pub snapshot: PathBuf,
}

impl Version {
    pub fn read(&self) -> std::io::Result<String> {
        std::fs::read_to_string(&self.snapshot)
    }
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The versions kept of `path`, newest first.
pub fn versions(path: &Path) -> Vec<Version> {
    versions_in(&dir(), path)
}

fn versions_in(root: &Path, path: &Path) -> Vec<Version> {
    let Ok(rd) = std::fs::read_dir(file_dir(root, path)) else { return Vec::new() };
    let mut out: Vec<Version> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ms: u64 = p.file_name()?.to_str()?.strip_suffix(".snap")?.parse().ok()?;
            Some(Version { time: UNIX_EPOCH + Duration::from_millis(ms), snapshot: p })
        })
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v.time));
    out
}

/// The file as it is on disk, read just before Qell writes over it, if it
/// is small enough to keep.
pub fn before_write(path: &Path, cfg: &HistoryConfig) -> Option<(String, SystemTime)> {
    if !cfg.enabled {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cfg.max_file_kb.saturating_mul(1024) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some((text, meta.modified().unwrap_or_else(|_| SystemTime::now())))
}

/// Keep `text`, just saved to `path`, and the version it replaced when the
/// history doesn't have that one yet. Runs on a worker: the caller hands
/// over owned values.
pub fn record(path: PathBuf, before: Option<(String, SystemTime)>, text: String, cfg: HistoryConfig) {
    if !cfg.enabled || text.len() as u64 > cfg.max_file_kb.saturating_mul(1024) {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = record_in(&dir(), &path, before, &text, &cfg, SystemTime::now()) {
            log::warn!("local history for {}: {e}", path.display());
        }
    });
}

fn record_in(
    root: &Path,
    path: &Path,
    before: Option<(String, SystemTime)>,
    text: &str,
    cfg: &HistoryConfig,
    now: SystemTime,
) -> std::io::Result<()> {
    let folder = file_dir(root, path);
    std::fs::create_dir_all(&folder)?;
    private(&folder);
    let id = folder.join("path");
    if !id.exists() {
        std::fs::write(&id, path.to_string_lossy().as_bytes())?;
    }
    let mut latest = versions_in(root, path).first().and_then(|v| v.read().ok());
    if let Some((old, at)) = before {
        // The version on disk, unless it's the one we have (or the one
        // about to be kept): the start of this round of edits.
        if latest.as_deref() != Some(old.as_str()) && old != text {
            write_new(&folder, at.min(now), &old)?;
        }
        latest = Some(old);
    }
    if latest.as_deref() != Some(text) {
        write_new(&folder, now, text)?;
    }
    prune_folder(&folder, cfg, now);
    Ok(())
}

/// Write a version at `at`, a millisecond later if that name is taken.
fn write_new(folder: &Path, at: SystemTime, text: &str) -> std::io::Result<()> {
    let mut ms = millis(at);
    loop {
        let p = folder.join(format!("{ms}.snap"));
        if !p.exists() {
            std::fs::write(&p, text)?;
            private(&p);
            return Ok(());
        }
        ms += 1;
    }
}

/// Only you can read what you wrote.
fn private(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if p.is_dir() { 0o700 } else { 0o600 };
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = p;
}

/// Drop versions older than `keep_days`, then all but the newest
/// `max_per_file`; the folder goes with its last version.
fn prune_folder(folder: &Path, cfg: &HistoryConfig, now: SystemTime) {
    let Ok(rd) = std::fs::read_dir(folder) else { return };
    let mut snaps: Vec<(u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ms = p.file_name()?.to_str()?.strip_suffix(".snap")?.parse().ok()?;
            Some((ms, p))
        })
        .collect();
    snaps.sort_by_key(|(ms, _)| std::cmp::Reverse(*ms));
    let cutoff = millis(now).saturating_sub(cfg.keep_days.max(1) as u64 * 86_400_000);
    let keep = cfg.max_per_file.max(1);
    let mut kept = 0;
    for (ms, p) in snaps {
        if ms < cutoff || kept >= keep {
            let _ = std::fs::remove_file(p);
        } else {
            kept += 1;
        }
    }
    if kept == 0 {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// Prune every file's history (at startup, on a worker).
pub fn prune(cfg: &HistoryConfig) {
    let Ok(rd) = std::fs::read_dir(dir()) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        if e.path().is_dir() {
            prune_folder(&e.path(), cfg, now);
        }
    }
}

/// "Today 10:42:07", "Yesterday 18:03:12", "Mon 24 Nov 09:15:00", in the
/// local time `offset` seconds from UTC.
pub fn when(t: SystemTime, now: SystemTime, offset: i64) -> String {
    let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (t, now) = (secs(t) + offset, secs(now) + offset);
    let (day, today) = (t.div_euclid(86_400), now.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    let clock = format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60);
    if day == today {
        format!("Today {clock}")
    } else if day == today - 1 {
        format!("Yesterday {clock}")
    } else {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let (_, m, d, weekday) = crate::journal::civil(day);
        format!("{} {d} {} {clock}", &weekday[..3], MONTHS[m as usize - 1])
    }
}

/// Lines added and removed going from `old` to `new`.
pub fn counts(old: &str, new: &str) -> (usize, usize) {
    let diff = similar::TextDiff::configure().timeout(Duration::from_millis(50)).diff_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

/// `old` and `new` as one text with every line of both, and per line
/// whether it was removed, added or kept — for an inline diff view.
pub fn inline(old: &str, new: &str) -> (String, Vec<crate::document::LineMark>) {
    use crate::document::LineMark;
    let diff = similar::TextDiff::configure().timeout(Duration::from_millis(200)).diff_lines(old, new);
    let mut lines = Vec::new();
    let mut marks = Vec::new();
    for change in diff.iter_all_changes() {
        let line = change.value();
        lines.push(line.strip_suffix('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).unwrap_or(line).to_string());
        marks.push(match change.tag() {
            similar::ChangeTag::Insert => LineMark::Added,
            similar::ChangeTag::Delete => LineMark::Removed,
            similar::ChangeTag::Equal => LineMark::Context,
        });
    }
    (lines.join("\n"), marks)
}

/// The one edit that turns `old` into `new`: the char range of `old` to
/// replace, and the text to put there (common start and end kept, so
/// folds, marks and the cursor around them stay put).
pub fn edit(old: &str, new: &str) -> (usize, usize, String) {
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    let mut start = 0;
    while start < o.len() && start < n.len() && o[start] == n[start] {
        start += 1;
    }
    let mut end = 0;
    while end < o.len() - start && end < n.len() - start && o[o.len() - 1 - end] == n[n.len() - 1 - end] {
        end += 1;
    }
    (start, o.len() - end, n[start..n.len() - end].iter().collect())
}

/// The versions of a file listed in the palette.
#[derive(Debug, Clone, Default)]
pub struct Pick {
    pub file: PathBuf,
    pub versions: Vec<Version>,
    /// (label, hint) per version: when, and how it differs from now.
    pub rows: Vec<(String, String)>,
}

impl crate::app::QellApp {
    /// The file the history commands are about: the active tab's, or the
    /// file a history tab compares with.
    fn history_file(&self) -> Option<PathBuf> {
        let doc = self.docs.get(self.active)?;
        if let Some((file, _)) = &doc.history_of {
            return Some(file.clone());
        }
        doc.buffer.path().filter(|_| !doc.read_only && doc.media.is_none()).map(Path::to_path_buf)
    }

    /// The text of `file` now: its tab's, unsaved edits included, or the disk's.
    fn current_text(&self, file: &Path) -> String {
        match self.docs.iter().find(|d| d.buffer.path() == Some(file)) {
            Some(d) => d.buffer.text(),
            None => std::fs::read_to_string(file).unwrap_or_default(),
        }
    }

    /// List the kept versions of the active file in the palette.
    pub fn pick_history(&mut self, restore: bool) {
        if !self.config.history.enabled {
            return self.notify("Local history is off: set enabled = true under [history] in the settings file.");
        }
        let Some(file) = self.history_file() else {
            return self.notify("Open a file to see its local history.");
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let versions = versions(&file);
        if versions.is_empty() {
            return self.notify(format!("No local history for {name} yet: a version is kept each time you save it."));
        }
        let now_text = self.current_text(&file);
        let now = SystemTime::now();
        let offset = crate::journal::utc_offset();
        let rows = versions
            .iter()
            .map(|v| {
                let ago = crate::views::sidebar::ago(v.time, now);
                let diff = match v.read() {
                    Ok(old) => match counts(&old, &now_text) {
                        (0, 0) => "same as now".to_string(),
                        (a, r) => format!("+{a} −{r} since then"),
                    },
                    Err(_) => "unreadable".to_string(),
                };
                (when(v.time, now, offset), format!("{ago} · {diff}"))
            })
            .collect();
        self.history_pick = Pick { file, versions, rows };
        self.open_palette(crate::app::PaletteMode::History { restore });
    }

    /// Open version `i` beside the file as it is now, as an inline diff:
    /// red what the version had, green what the file has now.
    pub fn compare_version(&mut self, i: usize) {
        let Some(v) = self.history_pick.versions.get(i).cloned() else { return };
        let file = self.history_pick.file.clone();
        let old = match v.read() {
            Ok(t) => t,
            Err(e) => return self.notify_error(format!("Cannot read that version: {e}")),
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let at = self.history_pick.rows.get(i).map(|r| r.0.clone()).unwrap_or_default();
        let label = format!("{name} ({at})");
        if let Some(i) = self.docs.iter().position(|d| d.label.as_deref() == Some(label.as_str())) {
            self.active = i;
            self.show_editor_area();
            return;
        }
        let (text, marks) = inline(&old, &self.current_text(&file));
        let mut doc = crate::document::Document::diff(label, &file, &text, marks);
        doc.history_of = Some((file, v.snapshot));
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        self.show_editor_area();
        self.notify(format!(
            "{name} as it was {}: red lines are what it had, green what it has now. Local History: Restore Version brings it back.",
            at.to_lowercase()
        ));
    }

    /// Restore: the version shown in this history tab, or else pick one.
    pub fn restore_history(&mut self) {
        let shown = self.docs.get(self.active).and_then(|d| d.history_of.clone());
        match shown {
            Some((file, snapshot)) => {
                // The tab is "cart.go (Today 10:42:07)".
                let label = self.docs[self.active].label.clone().unwrap_or_default();
                let at = label.rsplit_once(" (").map(|(_, r)| r.trim_end_matches(')').to_string()).unwrap_or_default();
                self.restore_snapshot(file, &snapshot, &at);
            }
            None => self.pick_history(true),
        }
    }

    pub fn restore_version(&mut self, i: usize) {
        let Some(v) = self.history_pick.versions.get(i).cloned() else { return };
        let file = self.history_pick.file.clone();
        let at = self.history_pick.rows.get(i).map(|r| r.0.clone()).unwrap_or_default();
        self.restore_snapshot(file, &v.snapshot, &at);
    }

    /// Put the file's text back to a kept version, in the editor, as one
    /// undo step and unsaved — so you can look it over before saving.
    fn restore_snapshot(&mut self, file: PathBuf, snapshot: &Path, at: &str) {
        let text = match std::fs::read_to_string(snapshot) {
            Ok(t) => t,
            Err(e) => return self.notify_error(format!("Cannot read that version: {e}")),
        };
        // Close the history tab first: it compares with what's being changed.
        if self.docs.get(self.active).is_some_and(|d| d.history_of.is_some()) {
            self.docs.remove(self.active);
            self.active = self.active.min(self.docs.len().saturating_sub(1));
        }
        self.open_file(file.clone());
        let Some(idx) = self.docs.iter().position(|d| d.buffer.path() == Some(file.as_path())) else {
            return self.notify_error(format!("Cannot open {} to restore it.", file.display()));
        };
        self.active = idx;
        let doc = &mut self.docs[idx];
        let name = doc.buffer.file_name();
        let (start, end, new) = edit(&doc.buffer.text(), &text);
        if start == end && new.is_empty() {
            return self.notify(format!("{name} is already that version."));
        }
        let (s, e) = (doc.buffer.char_to_pos(start), doc.buffer.char_to_pos(end));
        doc.buffer.replace_range(s, e, &new);
        doc.buffer.set_cursor(s, false);
        doc.scroll_to_cursor = true;
        self.notify(format!("Restored {name} as it was {} — not saved yet; Undo takes it back.", at.to_lowercase()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HistoryConfig {
        HistoryConfig { enabled: true, keep_days: 30, max_per_file: 3, max_file_kb: 1024 }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qell-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn texts(root: &Path, path: &Path) -> Vec<String> {
        versions_in(root, path).iter().map(|v| v.read().unwrap()).collect()
    }

    #[test]
    fn keeps_the_version_it_replaced_then_each_save() {
        let root = tmp("keep");
        let path = Path::new("/w/cart.go");
        let t0 = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let t1 = t0 + Duration::from_secs(60);
        record_in(&root, path, Some(("a\n".into(), t0)), "b\n", &cfg(), t1).unwrap();
        assert_eq!(texts(&root, path), ["b\n", "a\n"]);
        // Saving again: the disk has "b", already kept.
        record_in(&root, path, Some(("b\n".into(), t1)), "c\n", &cfg(), t1 + Duration::from_secs(5)).unwrap();
        assert_eq!(texts(&root, path), ["c\n", "b\n", "a\n"]);
        // Nothing changed: nothing new.
        record_in(&root, path, Some(("c\n".into(), t1)), "c\n", &cfg(), t1 + Duration::from_secs(9)).unwrap();
        assert_eq!(texts(&root, path).len(), 3);
        assert_eq!(std::fs::read_to_string(file_dir(&root, path).join("path")).unwrap(), "/w/cart.go");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_changed_elsewhere_is_kept_before_being_overwritten() {
        let root = tmp("outside");
        let path = Path::new("/w/main.tf");
        let t = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        record_in(&root, path, None, "one\n", &cfg(), t).unwrap();
        // A git checkout changed it on disk, then Qell saved over it.
        record_in(
            &root,
            path,
            Some(("two\n".into(), t + Duration::from_secs(10))),
            "three\n",
            &cfg(),
            t + Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(texts(&root, path), ["three\n", "two\n", "one\n"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prunes_by_count_and_age() {
        let root = tmp("prune");
        let path = Path::new("/w/a.rs");
        let t = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        for i in 0..5 {
            record_in(&root, path, None, &format!("v{i}"), &cfg(), t + Duration::from_secs(i)).unwrap();
        }
        assert_eq!(texts(&root, path), ["v4", "v3", "v2"]);
        // Forty days later, everything has expired.
        prune_folder(&file_dir(&root, path), &cfg(), t + Duration::from_secs(40 * 86_400));
        assert!(!file_dir(&root, path).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn same_millisecond_gets_its_own_name() {
        let root = tmp("ms");
        let path = Path::new("/w/x");
        let t = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        record_in(&root, path, Some(("old".into(), t)), "new", &cfg(), t).unwrap();
        assert_eq!(texts(&root, path), ["new", "old"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn too_big_or_disabled_is_not_read() {
        let root = tmp("big");
        std::fs::create_dir_all(&root).unwrap();
        let f = root.join("big.txt");
        std::fs::write(&f, "x".repeat(3000)).unwrap();
        let small = HistoryConfig { max_file_kb: 2, ..cfg() };
        assert!(before_write(&f, &small).is_none());
        assert!(before_write(&f, &HistoryConfig { enabled: false, ..cfg() }).is_none());
        assert_eq!(before_write(&f, &cfg()).unwrap().0.len(), 3000);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn when_reads_like_a_clock() {
        // 2026-11-27 10:42:07 in Paris (UTC+1).
        let t = UNIX_EPOCH + Duration::from_secs(1_795_772_527);
        let now = t + Duration::from_secs(3600);
        assert_eq!(when(t, now, 3600), "Today 10:42:07");
        assert_eq!(when(t, now + Duration::from_secs(86_400), 3600), "Yesterday 10:42:07");
        assert_eq!(when(t, now + Duration::from_secs(5 * 86_400), 3600), "Fri 27 Nov 10:42:07");
    }

    #[test]
    fn inline_diff_and_counts() {
        use crate::document::LineMark::*;
        let (text, marks) = inline("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(text, "a\nb\nB\nc\nd");
        assert_eq!(marks, [Context, Removed, Added, Context, Added]);
        assert_eq!(counts("a\nb\nc\n", "a\nB\nc\nd\n"), (2, 1));
    }

    #[test]
    fn edit_keeps_the_common_ends() {
        assert_eq!(edit("let a = 1;\nok", "let a = 22;\nok"), (8, 9, "22".into()));
        assert_eq!(edit("same", "same"), (4, 4, String::new()));
        assert_eq!(edit("", "new"), (0, 0, "new".into()));
        assert_eq!(edit("aaa", "aa"), (2, 3, String::new()));
        assert_eq!(edit("é€x", "é€y"), (2, 3, "y".into()));
    }
}
