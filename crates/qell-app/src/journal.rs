//! The daily log: Journal: Write Today's Log gathers the day's activity
//! (activity.rs) by repository: your commits, the files you saved, the
//! tests you ran, the git and terminal commands. It asks an LLM to write
//! a stand-up style summary of it, and saves both as Markdown, one file
//! per day, opened in a tab.
//!
//! The LLM is the command in `[journal] command` (fed the activity on
//! stdin: `claude -p`, `ollama run …`, `llm`), else the Anthropic API with
//! `[journal] api_key` or `ANTHROPIC_API_KEY`, else none: the log then
//! holds the activity only. It runs only when asked.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::activity::{self, Command, Event};
use crate::app::QellApp;
use crate::config::JournalConfig;

/// How long the LLM may take.
const TIMEOUT: Duration = Duration::from_secs(180);

/// Commands that say nothing about the work.
const TRIVIAL: &[&str] = &[
    "ls",
    "ll",
    "la",
    "l",
    "cd",
    "pwd",
    "clear",
    "exit",
    "history",
    "cls",
    "z",
    "j",
    "which",
    "echo",
    "cat",
    "less",
    "man",
    "git status",
    "git st",
    "git diff",
    "git log",
    "git branch",
    "gst",
    "gd",
    "top",
    "htop",
];

/// The day's activity, ready to read or to hand to the LLM.
pub struct Day {
    /// `2026-11-27`.
    pub date: String,
    /// `Friday`.
    pub weekday: String,
    /// The activity as Markdown.
    pub digest: String,
    /// Anything was done.
    pub any: bool,
}

/// Your UTC offset in seconds, as the system says (`date +%z`).
pub fn utc_offset() -> i64 {
    let out = Process::new("date").arg("+%z").output().ok();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    parse_offset(&text)
}

fn parse_offset(s: &str) -> i64 {
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => return 0,
    };
    if digits.len() != 4 {
        return 0;
    }
    let h: i64 = digits[..2].parse().unwrap_or(0);
    let m: i64 = digits[2..].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

/// Days since the epoch → (year, month, day), and the weekday.
pub fn civil(days: i64) -> (i64, u32, u32, &'static str) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    const NAMES: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];
    (y, m, d, NAMES[days.rem_euclid(7) as usize])
}

/// The local day `days` (since the epoch): its start and end, in UTC
/// seconds.
fn bounds(days: i64, offset: i64) -> (i64, i64) {
    let start = days * 86_400 - offset;
    (start, start + 86_400)
}

/// The local day number of a time.
fn day_of(t: i64, offset: i64) -> i64 {
    (t + offset).div_euclid(86_400)
}

fn clock(t: i64, offset: i64) -> String {
    let s = (t + offset).rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

fn trivial(line: &str) -> bool {
    let l = line.trim();
    TRIVIAL.iter().any(|t| {
        l == *t
            || l.starts_with(&format!("{t} "))
                && matches!(*t, "cd" | "ls" | "ll" | "la" | "cat" | "less" | "man" | "which" | "echo" | "z" | "j")
    })
}

/// Repositories met, cached: folder → top folder.
struct Repos(BTreeMap<PathBuf, Option<PathBuf>>);

impl Repos {
    fn of(&mut self, path: &Path) -> Option<PathBuf> {
        let dir = if path.is_dir() { path.to_path_buf() } else { path.parent()?.to_path_buf() };
        self.0.entry(dir.clone()).or_insert_with(|| qell_git::refs::toplevel(&dir)).clone()
    }
}

#[derive(Default)]
struct RepoDay {
    saves: BTreeMap<String, usize>,
    tests: Vec<String>,
    git: Vec<String>,
    tasks: Vec<String>,
    commands: Vec<(i64, Command)>,
}

fn rel(root: &Path, path: &Path) -> String {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    p.strip_prefix(root).map_or_else(|_| path.display().to_string(), |r| r.display().to_string())
}

/// Commands as lines: time, folder (when not the repository's top),
/// failures and long ones marked, repeats folded.
fn command_lines(cmds: &[(i64, Command)], root: Option<&Path>, offset: i64) -> Vec<String> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for (_, c) in cmds {
        if trivial(&c.line) {
            continue;
        }
        let mut s = format!("{} `{}`", clock(c.start, offset), c.line);
        if let Some(root) = root {
            let r = rel(root, &c.dir);
            if !r.is_empty() && r != "." {
                s.push_str(&format!(" (in {r})"));
            }
        } else {
            s.push_str(&format!(" (in {})", c.dir.display()));
        }
        if c.status != 0 {
            s.push_str(&format!(" → failed ({})", c.status));
        }
        if c.end - c.start >= 30 {
            s.push_str(&format!(" · {}m{:02}s", (c.end - c.start) / 60, (c.end - c.start) % 60));
        }
        // The same command again (the time aside): counted once.
        let key = s.split_once(' ').map_or(s.as_str(), |(_, k)| k).to_string();
        match out.last_mut() {
            Some((prev, n)) if prev.split_once(' ').map(|(_, k)| k) == Some(key.as_str()) => *n += 1,
            _ => out.push((s, 1)),
        }
    }
    out.into_iter().map(|(s, n)| if n > 1 { format!("{s} ×{n}") } else { s }).collect()
}

/// Everything done on local day `days`.
pub fn gather(days: i64, offset: i64) -> Day {
    let (from, to) = bounds(days, offset);
    let (y, m, d, weekday) = civil(days);
    let events = activity::events(from, to);
    let commands = activity::commands(from, to);
    let mut repos = Repos(BTreeMap::new());
    let mut by_repo: BTreeMap<Option<PathBuf>, RepoDay> = BTreeMap::new();
    let mut first = i64::MAX;
    let mut last = i64::MIN;
    for e in &events {
        first = first.min(e.time());
        last = last.max(e.time());
        match e {
            Event::Save { path, .. } => {
                let root = repos.of(path);
                let name = root.as_deref().map_or_else(|| path.display().to_string(), |r| rel(r, path));
                *by_repo.entry(root).or_default().saves.entry(name).or_default() += 1;
            }
            Event::Tests { file, passed, failed, ok, command, .. } => {
                let root = repos.of(file);
                let what = if *failed > 0 || !ok {
                    format!("`{command}` → {failed} failed, {passed} passed")
                } else {
                    format!("`{command}` → {passed} passed")
                };
                by_repo.entry(root).or_default().tests.push(what);
            }
            Event::Git { repo, what, .. } => {
                let root = repos.of(&repo.join(".")).or(Some(repo.clone()));
                by_repo.entry(root).or_default().git.push(what.clone());
            }
            Event::Task { cwd, command, .. } => {
                let root = repos.of(&cwd.join("."));
                by_repo.entry(root).or_default().tasks.push(format!("`{command}`"));
            }
        }
    }
    for c in commands {
        first = first.min(c.start);
        last = last.max(c.end);
        let root = repos.of(&c.dir);
        by_repo.entry(root).or_default().commands.push((c.start, c));
    }
    let mut md = String::new();
    let any = first != i64::MAX;
    if any {
        md.push_str(&format!("Active from {} to {}.\n", clock(first, offset), clock(last, offset)));
    }
    for (root, day) in &by_repo {
        let commits = root.as_deref().map(|r| qell_git::refs::my_commits(r, from, to)).unwrap_or_default();
        match root {
            Some(r) => {
                let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let branch = qell_git::refs::current_branch(r).map(|b| format!(" — on `{b}`")).unwrap_or_default();
                md.push_str(&format!("\n### {name}{branch}\n_{}_\n", r.display()));
            }
            None => md.push_str("\n### Outside repositories\n"),
        }
        if !commits.is_empty() {
            md.push_str("\nCommits:\n");
            for c in &commits {
                md.push_str(&format!("- {c}\n"));
            }
        }
        if !day.saves.is_empty() {
            let mut files: Vec<(&String, &usize)> = day.saves.iter().collect();
            files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            md.push_str("\nFiles edited (saves):\n");
            for (f, n) in files.iter().take(25) {
                md.push_str(&format!("- {f} ({n})\n"));
            }
            if files.len() > 25 {
                md.push_str(&format!("- … and {} more\n", files.len() - 25));
            }
        }
        if !day.tests.is_empty() {
            md.push_str("\nTests:\n");
            for t in &day.tests {
                md.push_str(&format!("- {t}\n"));
            }
        }
        if !day.git.is_empty() {
            md.push_str("\nGit (from Qell):\n");
            for g in &day.git {
                md.push_str(&format!("- {g}\n"));
            }
        }
        if !day.tasks.is_empty() {
            md.push_str("\nTasks run:\n");
            let mut seen: Vec<&String> = Vec::new();
            for t in &day.tasks {
                if !seen.contains(&t) {
                    let n = day.tasks.iter().filter(|x| *x == t).count();
                    md.push_str(&format!("- {t}{}\n", if n > 1 { format!(" ×{n}") } else { String::new() }));
                    seen.push(t);
                }
            }
        }
        let lines = command_lines(&day.commands, root.as_deref(), offset);
        if !lines.is_empty() {
            md.push_str("\nTerminal:\n");
            for l in lines.iter().take(120) {
                md.push_str(&format!("- {l}\n"));
            }
            if lines.len() > 120 {
                md.push_str(&format!("- … and {} more\n", lines.len() - 120));
            }
        }
    }
    Day { date: format!("{y:04}-{m:02}-{d:02}"), weekday: weekday.to_string(), digest: md, any }
}

/// The last day before `today` with any activity, within two weeks.
pub fn previous_day(today: i64, offset: i64) -> Option<i64> {
    let (from, _) = bounds(today - 14, offset);
    let (to, _) = bounds(today, offset);
    let mut days: Vec<i64> = activity::events(from, to).iter().map(|e| day_of(e.time(), offset)).collect();
    days.extend(activity::commands(from, to).iter().map(|c| day_of(c.start, offset)));
    days.into_iter().max()
}

fn prompt(day: &Day, language: &str) -> String {
    format!(
        "You write a developer's daily stand-up log from the activity their code editor recorded. \
Write in {language}. Output Markdown only, no preamble, no title.\n\
- Start with one sentence summing up the day.\n\
- `## Done`: bullet points grouped by project (bold project name), saying what was worked on in plain words — features, fixes, refactors, infrastructure changes, deployments, investigations. Use commits, edited files, tests and commands as evidence; merge related items; don't list commands one by one.\n\
- `## Issues`: failures worth telling the team (failing tests, failed deploys or commands), only if there are some.\n\
- `## Next`: only what the activity clearly leaves open (unfinished branch, failing tests).\n\
Be factual and short. Never invent work the activity doesn't show. Keep ticket ids (like OPS-5324) found in branches or commits.\n\n\
Activity of {weekday} {date}:\n\n{digest}",
        weekday = day.weekday,
        date = day.date,
        digest = day.digest,
    )
}

/// Run `command` (through sh) with `input` on stdin, within the timeout.
fn run_command(command: &str, input: &str) -> Result<String, String> {
    let mut child = Process::new("sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `{command}`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            return Err(format!("`{command}` took more than {}s", TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let last = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim();
        return Err(format!(
            "`{command}` failed ({status}){}",
            if last.is_empty() { String::new() } else { format!(": {last}") }
        ));
    }
    Ok(out.trim().to_string())
}

/// Ask the Anthropic API (through curl; the key goes on curl's stdin, not
/// its command line).
fn run_api(key: &str, model: &str, input: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1500,
        "messages": [{ "role": "user", "content": input }],
    });
    let dir = activity::dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("request-{}.json", std::process::id()));
    std::fs::write(&file, body.to_string()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    let curl = format!(
        "curl -sS --max-time {} https://api.anthropic.com/v1/messages -H 'content-type: application/json' -H 'anthropic-version: 2023-06-01' -K - --data-binary @'{}'",
        TIMEOUT.as_secs(),
        file.display()
    );
    let result = run_command(&curl, &format!("header = \"x-api-key: {key}\"\n"));
    let _ = std::fs::remove_file(&file);
    let out = result?;
    let v: serde_json::Value = serde_json::from_str(&out)
        .map_err(|_| format!("unexpected answer from the API: {}", out.chars().take(200).collect::<String>()))?;
    if let Some(msg) = v["error"]["message"].as_str() {
        return Err(format!("the API said: {msg}"));
    }
    let text: String = v["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
    if text.trim().is_empty() {
        return Err("the API answered nothing".into());
    }
    Ok(text)
}

/// Who writes the summary.
fn writer(cfg: &JournalConfig) -> Option<String> {
    if !cfg.command.trim().is_empty() {
        return Some(format!("`{}`", cfg.command.trim()));
    }
    api_key(cfg).map(|_| format!("the Anthropic API ({})", cfg.model))
}

fn api_key(cfg: &JournalConfig) -> Option<String> {
    Some(cfg.api_key.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()))
}

/// The summary, or why there is none.
fn summarize(cfg: &JournalConfig, day: &Day) -> Result<Option<String>, String> {
    let input = prompt(day, &cfg.language);
    if !cfg.command.trim().is_empty() {
        return run_command(cfg.command.trim(), &input).map(Some);
    }
    match api_key(cfg) {
        Some(key) => run_api(&key, &cfg.model, &input).map(Some),
        None => Ok(None),
    }
}

/// The log file's text.
fn document(day: &Day, summary: Option<&str>, problem: Option<&str>) -> String {
    let mut md = format!("# {} — {}\n\n", day.date, day.weekday);
    match (summary, problem) {
        (Some(s), _) => md.push_str(&format!("{}\n", s.trim())),
        (None, Some(p)) => md.push_str(&format!("> No summary: {p}\n")),
        (None, None) => md.push_str(
            "> No summary: set `command` (e.g. `claude -p`, `ollama run llama3.1`) or `api_key` under `[journal]` in config.toml, then Journal: Write Today's Log again.\n",
        ),
    }
    md.push_str("\n---\n\n## Activity\n\n");
    md.push_str(&day.digest);
    md
}

/// Where the logs go.
pub fn logs_dir(cfg: &JournalConfig) -> PathBuf {
    if cfg.dir.trim().is_empty() {
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("qell").join("journal")
    } else {
        let d = cfg.dir.trim();
        match d.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
            None => PathBuf::from(d),
        }
    }
}

/// A log being written, in the background.
pub struct Writing {
    pub rx: mpsc::Receiver<Result<PathBuf, (PathBuf, String)>>,
}

impl QellApp {
    /// Journal: Write Today's Log (`previous`: the last working day before).
    pub fn write_journal(&mut self, previous: bool) {
        if !self.config.journal.enabled {
            return self.notify("The activity journal is off ([journal] enabled = false in config.toml).");
        }
        if self.journal.is_some() {
            return self.notify("Already writing a log…");
        }
        let offset = utc_offset();
        let today = day_of(activity::now(), offset);
        let days = if previous {
            match previous_day(today, offset) {
                Some(d) => d,
                None => return self.notify("No activity in the last two weeks before today."),
            }
        } else {
            today
        };
        let cfg = self.config.journal.clone();
        let who = writer(&cfg);
        self.notify(match &who {
            Some(w) => format!("Writing the log with {w}…"),
            None => "Gathering the day's activity…".to_string(),
        });
        let (tx, rx) = mpsc::channel();
        let ctx = Some(self.egui_ctx.clone());
        std::thread::spawn(move || {
            let day = gather(days, offset);
            let dir = logs_dir(&cfg);
            let path = dir.join(format!("{}.md", day.date));
            if !day.any {
                let _ = tx.send(Err((path, format!("Nothing recorded on {} {}.", day.weekday, day.date))));
                return;
            }
            let (summary, problem) = match summarize(&cfg, &day) {
                Ok(s) => (s, None),
                Err(e) => (None, Some(e)),
            };
            let text = document(&day, summary.as_deref(), problem.as_deref());
            let written = std::fs::create_dir_all(&dir).and_then(|_| {
                // A log written before (and maybe edited) is kept aside.
                if path.exists() {
                    let _ = std::fs::copy(&path, path.with_extension("md.bak"));
                }
                std::fs::write(&path, text)
            });
            let result = match (written, problem) {
                (Err(e), _) => Err((path.clone(), format!("Cannot write {}: {e}", path.display()))),
                (Ok(()), Some(p)) => Err((path, format!("The log has the activity but no summary: {p}"))),
                (Ok(()), None) => Ok(path),
            };
            let _ = tx.send(result);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.journal = Some(Writing { rx });
    }

    /// Every frame: open the log when it's written.
    pub fn poll_journal(&mut self) {
        let Some(w) = &self.journal else { return };
        let Ok(result) = w.rx.try_recv() else { return };
        self.journal = None;
        match result {
            Ok(path) => {
                self.open_file(path.clone());
                // Read it rendered; Ctrl/⌘+Alt+V goes back to the text.
                if let Some(doc) = self.docs.get_mut(self.active).filter(|d| d.buffer.path() == Some(path.as_path())) {
                    doc.preview = true;
                }
                self.notify(format!("Log written: {}", path.display()));
            }
            Err((path, msg)) => {
                if path.exists() {
                    self.open_file(path);
                }
                self.notify_error(msg);
            }
        }
    }

    /// Journal: Open Logs Folder (in the system's file manager).
    pub fn open_journal_folder(&mut self) {
        let dir = logs_dir(&self.config.journal);
        let _ = std::fs::create_dir_all(&dir);
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        if let Err(e) = Process::new(opener).arg(&dir).spawn() {
            self.notify_error(format!("Cannot open {}: {e}", dir.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_times() {
        assert_eq!(parse_offset("+0100"), 3600);
        assert_eq!(parse_offset("-0530"), -(5 * 3600 + 30 * 60));
        // 2026-11-27 is a Friday.
        let d = day_of(1_795_651_200 + 86_400, 0);
        assert_eq!(civil(d), (2026, 11, 27, "Friday"));
        // 23:30 UTC is already the next day in Paris.
        let late = 1_795_651_200 + 23 * 3600 + 1800;
        assert_eq!(day_of(late, 3600), day_of(late, 0) + 1);
        assert_eq!(clock(late, 3600), "00:30");
        let (start, end) = bounds(d, 3600);
        assert_eq!(end - start, 86_400);
        assert_eq!(clock(start, 3600), "00:00");
    }

    #[test]
    fn commands_are_trimmed_and_folded() {
        let c = |t: i64, line: &str, status: i32, dur: i64| {
            (t, Command { start: t, end: t + dur, status, dir: PathBuf::from("/r/infra"), line: line.into() })
        };
        let lines = command_lines(
            &[
                c(0, "ls -la", 0, 0),
                c(60, "terraform plan", 1, 45),
                c(120, "terraform plan", 1, 45),
                c(180, "git status", 0, 0),
                c(240, "kubectl get pods -n api", 0, 1),
            ],
            Some(Path::new("/r")),
            0,
        );
        assert_eq!(
            lines,
            ["00:01 `terraform plan` (in infra) → failed (1) · 0m45s ×2", "00:04 `kubectl get pods -n api` (in infra)"]
        );
    }

    #[test]
    fn log_without_summary_says_how() {
        let day = Day { date: "2026-11-27".into(), weekday: "Friday".into(), digest: "x\n".into(), any: true };
        let doc = document(&day, None, None);
        assert!(doc.starts_with("# 2026-11-27 — Friday"));
        assert!(doc.contains("[journal]") && doc.ends_with("x\n"));
        assert!(prompt(&day, "French").contains("Write in French"));
    }
}
