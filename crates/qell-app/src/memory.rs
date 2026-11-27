//! The memory guard: Qell keeps an eye on how much memory it uses, and
//! when that passes `memory_warning_mb` it writes a report of what holds it
//! — the file index, each terminal's scrollback, open files, diagnostics,
//! language-server queues, git status — and tells you, before the machine
//! runs out.
//!
//! A thread reads the process's own footprint every few seconds (one
//! system call); everything else happens only when the limit is crossed,
//! and again each time it grows by half. Nothing leaves the machine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EVERY: Duration = Duration::from_secs(5);
const MB: u64 = 1024 * 1024;

/// The process's memory as the system counts it (Activity Monitor's
/// "Memory" on macOS, resident memory on Linux), in bytes.
pub fn footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: rusage_info_v2 is plain data, filled by the kernel.
        unsafe {
            let mut info: libc::rusage_info_v2 = std::mem::zeroed();
            let r = libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V2,
                &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
            );
            (r == 0).then_some(info.ri_phys_footprint)
        }
    }
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

pub fn dir() -> PathBuf {
    crate::recovery::data().join("memory")
}

/// The latest reading, kept fresh by a thread.
pub struct Guard {
    latest: Arc<AtomicU64>,
    /// The footprint of the last warning: the next is at half as much again.
    warned_at: u64,
    started: Instant,
    /// The last report written, shown in the status bar while memory stays high.
    pub report: Option<PathBuf>,
}

impl Guard {
    pub fn start(waker: impl Fn() + Send + 'static) -> Self {
        let latest = Arc::new(AtomicU64::new(footprint().unwrap_or(0)));
        let l = latest.clone();
        let spawned = std::thread::Builder::new().name("qell-memory".into()).spawn(move || loop {
            std::thread::sleep(EVERY);
            let Some(now) = footprint() else { return };
            let before = l.swap(now, Ordering::Relaxed);
            // Wake the UI only on a big change: it checks the limit.
            if now > before + 64 * MB {
                waker();
            }
        });
        if let Err(e) = spawned {
            log::warn!("memory guard not started: {e}");
        }
        Self { latest, warned_at: 0, started: Instant::now(), report: None }
    }

    pub fn latest(&self) -> u64 {
        self.latest.load(Ordering::Relaxed)
    }

    /// Whether it's time to warn, given the limit in MB (0: never).
    pub fn should_warn(&mut self, limit_mb: u64) -> Option<u64> {
        let now = self.latest();
        let due = limit_mb > 0 && now >= limit_mb * MB && now >= self.warned_at + self.warned_at / 2;
        if due {
            self.warned_at = now;
        }
        due.then_some(now)
    }

    /// The report to point at: while memory is still over the limit.
    pub fn alert(&self, limit_mb: u64) -> Option<(u64, PathBuf)> {
        let now = self.latest();
        (limit_mb > 0 && now >= limit_mb * MB).then(|| self.report.clone().map(|p| (now, p))).flatten()
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }
}

/// One measured part of what Qell holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub bytes: u64,
    pub detail: String,
}

pub fn mb(bytes: u64) -> String {
    if bytes < MB {
        format!("{} KB", bytes.div_ceil(1024))
    } else if bytes >= 1024 * MB {
        format!("{:.1} GB", bytes as f64 / (1024 * MB) as f64)
    } else {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    }
}

/// The report, as Markdown.
pub fn report(footprint: u64, uptime: Duration, parts: &[Part], notes: &[String]) -> String {
    let mut s = format!(
        "# Qell memory report\n\nQell {} on {} — using **{}**, {} min after start.\n\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        mb(footprint),
        uptime.as_secs() / 60
    );
    let known: u64 = parts.iter().map(|p| p.bytes).sum();
    s.push_str("| Part | Size | Detail |\n|---|---:|---|\n");
    let mut sorted: Vec<&Part> = parts.iter().collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    for p in sorted {
        s.push_str(&format!("| {} | {} | {} |\n", p.name, mb(p.bytes), p.detail.replace('|', "\\|")));
    }
    s.push_str(&format!("| **Measured** | **{}** | of {} in all |\n", mb(known), mb(footprint)));
    if !notes.is_empty() {
        s.push('\n');
        for n in notes {
            s.push_str(&format!("- {n}\n"));
        }
    }
    s.push_str(
        "\nWhat isn't measured above is the rest of the program (fonts, graphics, \
         libraries, about 300 MB) and anything growing where Qell doesn't look. \
         Please attach this report to an issue: https://github.com/sudo-bngz/qell/issues\n",
    );
    s
}

/// What to tell the user: the biggest part, if it explains much.
pub fn headline(footprint: u64, parts: &[Part]) -> String {
    let top = parts.iter().max_by_key(|p| p.bytes);
    match top {
        Some(p) if p.bytes * 3 >= footprint => {
            format!("Qell is using {} — {} holds {}.", mb(footprint), p.name, mb(p.bytes))
        }
        _ => format!("Qell is using {}, more than its parts account for.", mb(footprint)),
    }
}

/// Write a report; returns its path.
pub fn write(text: &str) -> std::io::Result<PathBuf> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = d.join(format!("memory-{secs}.md"));
    std::fs::write(&path, text)?;
    Ok(path)
}

impl crate::app::QellApp {
    /// Warn, with a report, when memory passes the limit.
    pub(crate) fn check_memory(&mut self) {
        let Some(now) = self.memory.should_warn(self.config.memory_warning_mb) else { return };
        let (parts, notes) = self.memory_parts();
        let text = report(now, self.memory.uptime(), &parts, &notes);
        let headline = headline(now, &parts);
        log::warn!("{headline}");
        match write(&text) {
            Ok(path) => {
                self.notify_error(format!("{headline} A report of what holds it is in {}.", path.display()));
                self.memory.report = Some(path);
            }
            Err(e) => self.notify_error(format!("{headline} (The report couldn't be written: {e}.)")),
        }
    }

    /// What Qell can measure of what it holds.
    pub(crate) fn memory_parts(&self) -> (Vec<Part>, Vec<String>) {
        let mut parts = Vec::new();
        let mut notes = Vec::new();
        {
            let index = self.indexer.index();
            let index = index.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            let (files, text, trigrams, symbols) = index.memory();
            parts.push(Part {
                name: "Search index".into(),
                bytes: (text + trigrams) as u64,
                detail: format!(
                    "{files} files: {} of text, {} of trigrams, {symbols} definitions",
                    mb(text as u64),
                    mb(trigrams as u64)
                ),
            });
        }
        for tab in &self.terminals {
            let (lines, cols, bytes) = tab.term.memory();
            let name = tab.name.clone().unwrap_or_else(|| "shell".into());
            parts.push(Part {
                name: format!("Terminal {}: {name}", tab.number),
                bytes: bytes as u64,
                detail: format!("{lines} lines × {cols} columns"),
            });
        }
        let mut docs: Vec<(String, usize)> = self
            .docs
            .iter()
            .map(|d| (d.label.clone().unwrap_or_else(|| d.buffer.file_name()), d.buffer.rope().len_bytes()))
            .collect();
        docs.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
        let largest: Vec<String> = docs.iter().take(3).map(|(n, b)| format!("{n} {}", mb(*b as u64))).collect();
        parts.push(Part {
            name: "Open files".into(),
            bytes: docs.iter().map(|(_, b)| *b as u64).sum(),
            detail: format!("{} tab{}; largest: {}", docs.len(), if docs.len() == 1 { "" } else { "s" }, largest.join(", ")),
        });
        let diagnostics: Vec<&qell_lsp::lsp_types::Diagnostic> =
            self.errors.values().chain(self.problems.values()).flatten().collect();
        parts.push(Part {
            name: "Diagnostics".into(),
            bytes: diagnostics.iter().map(|d| d.message.len() as u64 + 160).sum(),
            detail: format!("{} in {} files (estimated)", diagnostics.len(), self.errors.len().max(self.problems.len())),
        });
        if let Some(st) = &self.git_status {
            let n = st.entry_count();
            parts.push(Part {
                name: "Git status".into(),
                bytes: n as u64 * 160,
                detail: format!("{} repositories, {n} paths with a status (estimated)", st.repos.len()),
            });
        }
        let (events, servers) = self.lsp.queues();
        if events > 0 {
            notes.push(format!("{events} language-server events waiting to be read"));
        }
        for (lang, root, queued) in servers {
            notes.push(format!("Language server {lang} for {}: {queued} messages waiting to be sent", root.display()));
        }
        notes.push(format!("Workspace: {} (ignore: {})", self.root.display(), self.config.ignore.join(", ")));
        (parts, notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_own_footprint() {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(footprint().unwrap() > MB);
    }

    #[test]
    fn warns_at_the_limit_then_at_each_half_more() {
        let mut g = Guard { latest: Arc::new(AtomicU64::new(500 * MB)), warned_at: 0, started: Instant::now(), report: None };
        assert_eq!(g.should_warn(2048), None);
        assert_eq!(g.should_warn(0), None);
        g.latest.store(2100 * MB, Ordering::Relaxed);
        assert_eq!(g.should_warn(2048), Some(2100 * MB));
        assert_eq!(g.should_warn(2048), None);
        g.latest.store(3000 * MB, Ordering::Relaxed);
        assert_eq!(g.should_warn(2048), None);
        g.latest.store(3200 * MB, Ordering::Relaxed);
        assert_eq!(g.should_warn(2048), Some(3200 * MB));
    }

    #[test]
    fn report_and_headline() {
        let parts = vec![
            Part { name: "Search index".into(), bytes: 300 * MB, detail: "21630 files".into() },
            Part { name: "Terminal 1: zsh".into(), bytes: 2400 * MB, detail: "10000 lines".into() },
        ];
        let r = report(3000 * MB, Duration::from_secs(600), &parts, &["a note".into()]);
        assert!(r.contains("using **2.9 GB**, 10 min after start"));
        // Biggest first.
        assert!(r.find("Terminal 1").unwrap() < r.find("Search index").unwrap());
        assert!(r.contains("| **Measured** | **2.6 GB** | of 2.9 GB in all |"));
        assert!(r.contains("- a note"));
        assert_eq!(headline(3000 * MB, &parts), "Qell is using 2.9 GB — Terminal 1: zsh holds 2.3 GB.");
        assert_eq!(headline(9000 * MB, &parts), "Qell is using 8.8 GB, more than its parts account for.");
    }
}
