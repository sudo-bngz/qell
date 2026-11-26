//! Running tests: the ▶ beside each test in the editor, Test: Run… in
//! the palette. The runner's own command runs in a terminal tab (you see
//! its real output, and the tab stays as a shell in that folder after);
//! a copy of that output is read as it comes to show passes and failures
//! in the gutter and the Tests tab. Nothing runs unless asked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qell_core::testing::{self, Outcome, Runner, Status, TestFn};

use crate::app::{Focus, QellApp};

/// How often the output is read while tests run.
const POLL: Duration = Duration::from_millis(250);
/// What the run's script writes last, with the exit status.
const EXIT_MARK: &str = "__QELL_EXIT__";

/// A test run: what was asked, and what came out so far.
pub struct TestRun {
    pub runner: Runner,
    pub file: PathBuf,
    /// Where the command ran; failure paths are relative to it.
    pub cwd: PathBuf,
    pub command: String,
    /// The tests asked for (their labels); all of the file when it was run.
    pub asked: Vec<String>,
    pub outcomes: Vec<Outcome>,
    /// The command's exit status, once done.
    pub exit: Option<i32>,
    pub started: Instant,
    pub finished: Option<Instant>,
    log: PathBuf,
    read_at: Instant,
    /// Terminal tab number it runs in.
    pub terminal: usize,
}

#[derive(Default)]
pub struct Tests {
    pub run: Option<TestRun>,
    /// Last known result of each test, by file and label.
    pub results: HashMap<PathBuf, HashMap<String, Status>>,
    /// What runner each file's tests are for (`None`: not a test file).
    runners: HashMap<PathBuf, Option<Runner>>,
    /// The last run, to run again: file and the tests asked for.
    last: Option<(PathBuf, Vec<TestFn>)>,
}

impl Tests {
    /// The status shown beside a test of `file`.
    pub fn status(&self, file: &Path, label: &str) -> Option<Status> {
        self.results.get(file)?.get(label).copied()
    }
}

/// `'…'` for sh, whatever is inside.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A regex matching exactly these names (for `-run`, `-t`).
fn exact(names: &[String], prefix_only: bool) -> String {
    let esc: Vec<String> = names.iter().map(|n| regex::escape(n)).collect();
    let body = if esc.len() == 1 { esc[0].clone() } else { format!("({})", esc.join("|")) };
    if prefix_only {
        format!("^{body}")
    } else {
        format!("^{body}$")
    }
}

/// The nearest folder at or above `from` holding `marker`.
fn up_to(from: &Path, marker: &str) -> Option<PathBuf> {
    from.ancestors().find(|d| d.join(marker).is_file()).map(Path::to_path_buf)
}

/// `name = "…"` under `[package]` of a Cargo.toml.
fn cargo_package(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
        } else if in_package {
            if let Some(v) = t.strip_prefix("name").map(str::trim_start).and_then(|r| r.strip_prefix('=')) {
                return Some(v.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// The module path of a Rust file inside its crate, with `::` after it:
/// `src/net/http.rs` → `net::http::`; `src/lib.rs`, `src/main.rs` and an
/// integration test file → ``.
fn rust_module(root: &Path, file: &Path) -> String {
    let Ok(rel) = file.strip_prefix(root.join("src")) else { return String::new() };
    let mut parts: Vec<String> = rel.iter().map(|p| p.to_string_lossy().trim_end_matches(".rs").to_string()).collect();
    if parts.last().is_some_and(|l| l == "mod") {
        parts.pop();
    }
    if parts.len() == 1 && matches!(parts[0].as_str(), "lib" | "main") {
        return String::new();
    }
    // A binary under src/bin is its own crate.
    if parts.first().is_some_and(|p| p == "bin") {
        return String::new();
    }
    parts.iter().map(|p| format!("{p}::")).collect()
}

/// The command (for sh) running `tests` of `file`, and where to run it.
pub fn command(runner: Runner, file: &Path, tests: &[TestFn]) -> Result<(PathBuf, String), String> {
    let dir = file.parent().ok_or("no folder")?.to_path_buf();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let names: Vec<String> = tests.iter().map(|t| t.name.clone()).collect();
    match runner {
        Runner::Go => {
            let (bench, plain): (Vec<String>, Vec<String>) = names.into_iter().partition(|n| n.starts_with("Benchmark"));
            let mut cmd = String::from("go test -v -count=1");
            if plain.is_empty() {
                cmd.push_str(" -run '^$'");
            } else {
                cmd.push_str(&format!(" -run {}", quote(&exact(&plain, false))));
            }
            if !bench.is_empty() {
                cmd.push_str(&format!(" -bench {} -benchmem", quote(&exact(&bench, false))));
            }
            Ok((dir, format!("{cmd} .")))
        }
        Runner::Cargo => {
            let root = up_to(&dir, "Cargo.toml").ok_or("This file isn't in a Cargo project (no Cargo.toml above it).")?;
            let mut cmd = String::from("cargo test");
            if let Some(pkg) = cargo_package(&root.join("Cargo.toml")) {
                cmd.push_str(&format!(" -p {}", quote(&pkg)));
            }
            // An integration test: tests/<name>.rs is its own target.
            if file.parent() == Some(root.join("tests").as_path()) {
                let target = name.trim_end_matches(".rs");
                cmd.push_str(&format!(" --test {}", quote(target)));
            }
            // Test paths are those inside the file; the file's own module
            // comes first (src/net/http.rs → net::http::).
            let prefix = rust_module(&root, file);
            cmd.push_str(" -- --exact");
            for n in &names {
                cmd.push(' ');
                cmd.push_str(&quote(&format!("{prefix}{n}")));
            }
            Ok((root, cmd))
        }
        Runner::Jest | Runner::Vitest => {
            let root = up_to(&dir, "package.json").ok_or("This file isn't in a Node project (no package.json above it).")?;
            let rel = file.strip_prefix(&root).unwrap_or(file).to_string_lossy().into_owned();
            let groups = tests.iter().any(|t| t.group);
            let mut cmd = if runner == Runner::Vitest {
                format!("npx vitest run --reporter=verbose {}", quote(&rel))
            } else {
                format!("npx jest --colors --verbose {}", quote(&rel))
            };
            if !names.is_empty() {
                cmd.push_str(&format!(" -t {}", quote(&exact(&names, groups))));
            }
            Ok((root, cmd))
        }
        Runner::Pytest => {
            let mut cmd = String::from("python3 -m pytest -v --color=yes");
            if names.is_empty() {
                cmd.push_str(&format!(" {}", quote(&name)));
            }
            for n in &names {
                cmd.push_str(&format!(" {}", quote(&format!("{name}::{n}"))));
            }
            Ok((dir, cmd))
        }
    }
}

impl QellApp {
    /// The runner for a file's tests, found once per file (JavaScript
    /// looks at package.json for vitest).
    pub fn test_runner(&mut self, path: &Path) -> Option<Runner> {
        if let Some(r) = self.tests.runners.get(path) {
            return *r;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let vitest = path
            .parent()
            .and_then(|d| up_to(d, "package.json"))
            .and_then(|root| std::fs::read_to_string(root.join("package.json")).ok())
            .is_some_and(|p| p.contains("\"vitest\""));
        let r = testing::runner_for(&name, vitest);
        self.tests.runners.insert(path.to_path_buf(), r);
        r
    }

    /// The tests of the active tab, with its path and runner.
    fn active_tests(&mut self) -> Option<(PathBuf, Runner, Vec<TestFn>)> {
        let path = self.docs.get(self.active)?.buffer.path()?.to_path_buf();
        let runner = self.test_runner(&path)?;
        let text = self.docs.get(self.active)?.buffer.text();
        Some((path, runner, testing::find(runner, &text)))
    }

    /// Run the test defined on `line` of the active tab (the gutter's ▶).
    pub fn run_test_at_line(&mut self, line: usize) {
        let Some((path, runner, tests)) = self.active_tests() else { return };
        if let Some(t) = tests.into_iter().find(|t| t.line == line) {
            self.start_tests(path, runner, vec![t]);
        }
    }

    /// Test: Run Test at Cursor — the test the cursor is in (the last one
    /// starting at or above it).
    pub fn run_test_at_cursor(&mut self) {
        let Some((path, runner, tests)) = self.active_tests().filter(|(_, _, t)| !t.is_empty()) else {
            return self.notify("No tests in this file. Qell runs Go `TestX`, Rust `#[test]`, Jest/Vitest `it`/`test` and pytest `test_` functions.");
        };
        let line = self.docs.get(self.active).map(|d| d.buffer.cursor.head().line).unwrap_or(0);
        match tests.iter().rev().find(|t| t.line <= line && !t.group).cloned() {
            Some(t) => self.start_tests(path, runner, vec![t]),
            None => self.notify("Put the cursor in a test, or use Test: Run Tests in File."),
        }
    }

    /// Test: Run Tests in File.
    pub fn run_tests_in_file(&mut self) {
        match self.active_tests() {
            Some((path, runner, tests)) if !tests.is_empty() => {
                // The whole file: the runner selects by name what the file
                // defines (top-level ones; groups bring their own).
                let top: Vec<TestFn> = match runner {
                    Runner::Jest | Runner::Vitest => Vec::new(),
                    Runner::Pytest => Vec::new(),
                    _ => tests.into_iter().filter(|t| !t.group).collect(),
                };
                self.start_tests(path, runner, top);
            }
            _ => self.notify("No tests in this file. Qell runs Go `TestX`, Rust `#[test]`, Jest/Vitest `it`/`test` and pytest `test_` functions."),
        }
    }

    /// Test: Run Last Again.
    pub fn rerun_tests(&mut self) {
        let Some((path, tests)) = self.tests.last.clone() else {
            return self.notify("No tests have run yet.");
        };
        let Some(runner) = self.test_runner(&path) else { return };
        self.start_tests(path, runner, tests);
    }

    fn start_tests(&mut self, file: PathBuf, runner: Runner, tests: Vec<TestFn>) {
        // Unsaved edits wouldn't be tested: save the file first.
        if let Some(i) = self.docs.iter().position(|d| d.buffer.path() == Some(file.as_path()) && d.buffer.is_dirty()) {
            let active = self.active;
            self.active = i;
            self.write_active_or_ask(false);
            self.active = active;
        }
        let (cwd, cmd) = match command(runner, &file, &tests) {
            Ok(c) => c,
            Err(e) => return self.notify_error(e),
        };
        // One test tab at a time: a new run replaces the last one's.
        if let Some(old) = self.tests.run.take() {
            if let Some(i) = self.terminals.iter().position(|t| t.number == old.terminal) {
                self.kill_terminal(i);
            }
            let _ = std::fs::remove_file(&old.log);
        }
        static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let log = std::env::temp_dir().join(format!("qell-tests-{}-{n}.log", std::process::id()));
        let _ = std::fs::write(&log, "");
        let lg = quote(&log.to_string_lossy());
        let script = format!(
            "echo {shown}; echo; CARGO_TERM_COLOR=always FORCE_COLOR=1 {cmd} 2>&1 | tee {lg}; s=${{PIPESTATUS[0]}}; printf '\\n{EXIT_MARK} %s\\n' \"$s\" >> {lg}; exec \"${{SHELL:-bash}}\"",
            shown = quote(&format!("$ {cmd}")),
        );
        let asked: Vec<String> = tests.iter().map(|t| t.label.clone()).collect();
        let label = match tests.as_slice() {
            [one] => format!("test {}", one.label),
            _ => format!("test {}", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        };
        let focus = self.focus;
        let Some(number) = self.run_in_terminal_at(label, cwd.clone(), "bash", vec!["-c".into(), script]) else { return };
        // The editor keeps the keyboard; the output is in the panel.
        self.focus = if focus == Focus::Terminal { Focus::Terminal } else { Focus::Editor };
        let results = self.tests.results.entry(file.clone()).or_default();
        if asked.is_empty() {
            results.clear();
        }
        for a in &asked {
            results.insert(a.clone(), Status::Running);
        }
        self.tests.last = Some((file.clone(), tests));
        let now = Instant::now();
        self.tests.run = Some(TestRun {
            runner,
            file,
            cwd,
            command: cmd,
            asked,
            outcomes: Vec::new(),
            exit: None,
            started: now,
            finished: None,
            log,
            read_at: now,
            terminal: number,
        });
    }

    /// Every frame: read what the running tests printed.
    pub fn poll_tests(&mut self, ctx: &egui::Context) {
        let Some(run) = self.tests.run.as_mut() else { return };
        if run.exit.is_some() {
            return;
        }
        ctx.request_repaint_after(POLL);
        if run.read_at.elapsed() < POLL {
            return;
        }
        run.read_at = Instant::now();
        let out = std::fs::read_to_string(&run.log).unwrap_or_default();
        run.outcomes = testing::parse(run.runner, &out);
        let exit = out.lines().rev().find_map(|l| l.strip_prefix(EXIT_MARK)).map(|s| s.trim().parse().unwrap_or(1));
        // The tab was closed (or its process killed) before the end.
        let gone = !self.terminals.iter().any(|t| t.number == run.terminal && !t.term.has_exited());
        let exit = exit.or(gone.then_some(-1));
        // Results by label, for the gutter.
        let results = self.tests.results.entry(run.file.clone()).or_default();
        for o in run.outcomes.iter().filter(|o| !o.name.is_empty()) {
            let short = o.short_name().to_string();
            // A failing subtest fails its test; passing ones don't hide that.
            let keep_fail = o.status == Status::Passed && results.get(&short) == Some(&Status::Failed) && o.name.contains('/');
            if !keep_fail {
                results.insert(short, o.status);
            }
        }
        let Some(code) = exit else { return };
        run.exit = Some(code);
        run.finished = Some(Instant::now());
        // Asked for but never reported: the run failed before them (a
        // build error) or they don't exist any more.
        for a in &run.asked {
            if results.get(a) == Some(&Status::Running) {
                if code == 0 {
                    results.remove(a);
                } else {
                    results.insert(a.clone(), Status::Failed);
                }
            }
        }
        let passed = run.outcomes.iter().filter(|o| o.status == Status::Passed && !o.name.is_empty()).count();
        let failed = run.outcomes.iter().filter(|o| o.status == Status::Failed && !o.name.is_empty()).count();
        let build = run.outcomes.iter().any(|o| o.name.is_empty());
        let msg = if build {
            "Tests didn't build — see the Tests tab.".to_string()
        } else if code == -1 {
            "Test run stopped.".to_string()
        } else if passed + failed == 0 && code != 0 {
            "The test command failed — see its output in the terminal.".to_string()
        } else if failed > 0 {
            format!("Tests: {failed} failed, {passed} passed.")
        } else {
            format!("Tests: {passed} passed.")
        };
        if failed > 0 || build || code != 0 {
            self.notify_error(msg);
            // Show what failed (the output stays a tab away).
            self.show_terminal = true;
            self.tests_open = true;
            self.problems_open = false;
        } else {
            self.notify(msg);
        }
    }

    /// Open where a failure happened (paths are the runner's, from where
    /// it ran), or the test's own line.
    pub fn open_test_outcome(&mut self, i: usize) {
        let Some(run) = &self.tests.run else { return };
        let Some(o) = run.outcomes.get(i) else { return };
        if let Some((path, line)) = &o.location {
            let p = Path::new(path);
            let candidates = [run.cwd.join(p), run.file.parent().map(|d| d.join(p)).unwrap_or_default(), p.to_path_buf()];
            if let Some(found) = candidates.iter().find(|c| c.is_file()) {
                let found = found.clone();
                let line = line.saturating_sub(1);
                self.open_location(&found, line, 0, 0);
                return;
            }
        }
        // No location: the test's definition in the file that ran.
        let (file, runner, short) = (run.file.clone(), run.runner, o.short_name().to_string());
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        if let Some(t) = testing::find(runner, &text).into_iter().find(|t| t.label == short) {
            self.open_location(&file, t.line, 0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str) -> TestFn {
        TestFn { name: name.into(), label: name.into(), line: 0, group: false }
    }

    #[test]
    fn commands() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let go = root.join("pkg/math_test.go");
        let (cwd, cmd) = command(Runner::Go, &go, &[t("TestAdd"), t("BenchmarkAdd")]).unwrap();
        assert_eq!(cwd, root.join("pkg"));
        assert_eq!(cmd, "go test -v -count=1 -run '^TestAdd$' -bench '^BenchmarkAdd$' -benchmem .");

        std::fs::create_dir_all(root.join("crate/tests")).unwrap();
        std::fs::write(root.join("crate/Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let (cwd, cmd) = command(Runner::Cargo, &root.join("crate/tests/api.rs"), &[t("works")]).unwrap();
        assert_eq!(cwd, root.join("crate"));
        assert_eq!(cmd, "cargo test -p 'demo' --test 'api' -- --exact 'works'");
        let (_, cmd) = command(Runner::Cargo, &root.join("crate/src/net/http.rs"), &[t("tests::get")]).unwrap();
        assert_eq!(cmd, "cargo test -p 'demo' -- --exact 'net::http::tests::get'");

        std::fs::write(root.join("package.json"), "{}").unwrap();
        let (_, cmd) = command(Runner::Vitest, &root.join("src/a.test.ts"), &[t("greet says hi")]).unwrap();
        assert_eq!(cmd, "npx vitest run --reporter=verbose 'src/a.test.ts' -t '^greet says hi$'");
        let (_, cmd) = command(Runner::Pytest, &root.join("test_x.py"), &[t("TestA::test_b")]).unwrap();
        assert_eq!(cmd, "python3 -m pytest -v --color=yes 'test_x.py::TestA::test_b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
