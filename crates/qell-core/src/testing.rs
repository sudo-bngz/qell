//! Tests in a file and what a test run printed: finding the test functions
//! of Go, Rust, JavaScript/TypeScript (Jest, Vitest) and Python (pytest),
//! and reading their runners' output into passes, failures and where each
//! failure happened.

/// The tools that run tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Runner {
    Go,
    Cargo,
    Jest,
    Vitest,
    Pytest,
}

/// A test (or a group of them) defined in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    /// What the runner is given to select it: `TestAdd`, `adds`, `greet
    /// says hi` (a JS test inside a `describe`), `TestMath::test_add`.
    pub name: String,
    /// What it is called in the file (the function, the `it` title).
    pub label: String,
    /// 0-based line it is defined on.
    pub line: usize,
    /// A `describe` block or a test class: runs the tests inside.
    pub group: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Passed,
    Failed,
    Skipped,
}

/// One test's result as the runner printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// As the runner names it (`TestSub/negative`, `tests::subs`,
    /// `greet › repeats`, `test_math.py::test_sub`); `""` for a build
    /// error that stopped the run.
    pub name: String,
    pub status: Status,
    /// Where it failed: a path as printed (relative to where the runner
    /// ran) and a 1-based line.
    pub location: Option<(String, usize)>,
    /// The first lines saying why.
    pub message: Vec<String>,
}

impl Outcome {
    /// The name of the test function or title it is about, to match the
    /// file's tests: `TestSub/negative` → `TestSub`, `tests::subs` →
    /// `subs`, `greet › repeats` → `repeats`, `a.py::T::t` → `t`.
    pub fn short_name(&self) -> &str {
        let n = self.name.as_str();
        let n = n.split('/').next().unwrap_or(n);
        let n = n.rsplit("::").next().unwrap_or(n);
        let n = n.rsplit(" › ").next().unwrap_or(n);
        n.rsplit(" > ").next().unwrap_or(n).trim()
    }
}

/// Which runner a file's tests are for, by its name. JavaScript needs the
/// project's choice (`vitest` in package.json), given as `vitest`.
pub fn runner_for(file_name: &str, vitest: bool) -> Option<Runner> {
    let ext = file_name.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "go" if file_name.ends_with("_test.go") => Some(Runner::Go),
        "rs" => Some(Runner::Cargo),
        "js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs" | "mts" | "cts" if is_js_test(file_name) => {
            Some(if vitest { Runner::Vitest } else { Runner::Jest })
        }
        "py" if file_name.starts_with("test_") || file_name.ends_with("_test.py") => Some(Runner::Pytest),
        _ => None,
    }
}

fn is_js_test(name: &str) -> bool {
    [".test.", ".spec."].iter().any(|m| name.contains(m))
}

/// The tests a file defines, in order.
pub fn find(runner: Runner, text: &str) -> Vec<TestFn> {
    match runner {
        Runner::Go => find_go(text),
        Runner::Cargo => find_rust(text),
        Runner::Jest | Runner::Vitest => find_js(text),
        Runner::Pytest => find_python(text),
    }
}

fn ident(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..end]
}

fn find_go(text: &str) -> Vec<TestFn> {
    text.lines()
        .enumerate()
        .filter_map(|(line, l)| {
            let name = ident(l.strip_prefix("func ")?);
            let rest = &l["func ".len() + name.len()..];
            let is_test = ["Test", "Benchmark", "Example", "Fuzz"].iter().any(|p| {
                // `TestX` or `Test_x` or `Test`, not `Testing`.
                name.strip_prefix(p).is_some_and(|r| r.is_empty() || !r.starts_with(|c: char| c.is_lowercase()))
            });
            (is_test && rest.starts_with('(')).then(|| TestFn {
                name: name.to_string(),
                label: name.to_string(),
                line,
                group: false,
            })
        })
        .collect()
}

fn find_rust(text: &str) -> Vec<TestFn> {
    let mut out = Vec::new();
    let mut pending = false;
    // Inline modules the test sits in: (indent, name), for its path
    // (`tests::adds`).
    let mut mods: Vec<(usize, String)> = Vec::new();
    for (line, l) in text.lines().enumerate() {
        let t = l.trim();
        let indent = l.len() - l.trim_start().len();
        if t.starts_with('}') {
            while mods.last().is_some_and(|(i, _)| *i >= indent) {
                mods.pop();
            }
        }
        let item = t.trim_start_matches("pub(crate) ").trim_start_matches("pub ");
        if let Some(rest) = item.strip_prefix("mod ") {
            let name = ident(rest);
            if rest[name.len()..].trim_start().starts_with('{') {
                mods.push((indent, name.to_string()));
            }
            continue;
        }
        if t.starts_with("#[test]") || t.starts_with("#[tokio::test") || t.starts_with("#[async_std::test") {
            pending = true;
            continue;
        }
        if !pending || t.starts_with("#[") || t.starts_with("//") || t.is_empty() {
            continue;
        }
        pending = false;
        let after = t.trim_start_matches("pub ").trim_start_matches("async ").trim_start_matches("unsafe ");
        if let Some(rest) = after.strip_prefix("fn ") {
            let name = ident(rest);
            if !name.is_empty() {
                let mut path: Vec<&str> = mods.iter().map(|(_, m)| m.as_str()).collect();
                path.push(name);
                out.push(TestFn { name: path.join("::"), label: name.to_string(), line, group: false });
            }
        }
    }
    out
}

/// The title of `it('…')`, `test("…")`, `describe(`…`)` (and `.only`,
/// `.skip`, `.each(…)`) at the start of a line: (is a describe, title).
fn js_call(line: &str) -> Option<(bool, String)> {
    let t = line.trim_start();
    let (group, rest) = ["describe", "it", "test"].iter().find_map(|k| Some((*k == "describe", t.strip_prefix(k)?)))?;
    let mut rest = rest;
    // Modifiers: .only, .skip, .concurrent, .each(table).
    while let Some(r) = rest.strip_prefix('.') {
        let m = ident(r);
        rest = &r[m.len()..];
        if m == "each" {
            let mut depth = 0;
            let end = rest.char_indices().find(|&(_, c)| {
                depth += match c {
                    '(' | '[' => 1,
                    ')' | ']' => -1,
                    _ => 0,
                };
                depth == 0
            })?;
            rest = &rest[end.0 + 1..];
        }
    }
    let rest = rest.strip_prefix('(')?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"' | '`'))?;
    let body = &rest[1..];
    let end = body.find(quote)?;
    Some((group, body[..end].to_string()))
}

fn find_js(text: &str) -> Vec<TestFn> {
    let mut out = Vec::new();
    // Open describe blocks: (indent, title).
    let mut groups: Vec<(usize, String)> = Vec::new();
    for (line, l) in text.lines().enumerate() {
        let indent = l.len() - l.trim_start().len();
        if l.trim().is_empty() {
            continue;
        }
        while groups.last().is_some_and(|(i, _)| *i >= indent) {
            groups.pop();
        }
        let Some((group, title)) = js_call(l) else { continue };
        let mut full: Vec<&str> = groups.iter().map(|(_, t)| t.as_str()).collect();
        full.push(&title);
        out.push(TestFn { name: full.join(" "), label: title.clone(), line, group });
        if group {
            groups.push((indent, title));
        }
    }
    out
}

fn find_python(text: &str) -> Vec<TestFn> {
    let mut out = Vec::new();
    let mut class: Option<(usize, String)> = None;
    for (line, l) in text.lines().enumerate() {
        let indent = l.len() - l.trim_start().len();
        let t = l.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if class.as_ref().is_some_and(|(i, _)| indent <= *i) {
            class = None;
        }
        if let Some(rest) = t.strip_prefix("class ") {
            let name = ident(rest);
            if name.starts_with("Test") {
                out.push(TestFn { name: name.to_string(), label: name.to_string(), line, group: true });
                class = Some((indent, name.to_string()));
            }
            continue;
        }
        let def = t.strip_prefix("def ").or_else(|| t.strip_prefix("async def "));
        if let Some(name) = def.map(ident).filter(|n| n.starts_with("test")) {
            let full = match &class {
                Some((_, c)) => format!("{c}::{name}"),
                None => name.to_string(),
            };
            out.push(TestFn { name: full, label: name.to_string(), line, group: false });
        }
    }
    out
}

/// Remove terminal colour codes.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// `path/to/file.ext:12` or `…:12:5` somewhere in `s`: (path, line).
fn location_in(s: &str, ext: &[&str]) -> Option<(String, usize)> {
    for word in s.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '\'' | '"' | ',')) {
        let mut parts = word.splitn(3, ':');
        let (Some(path), Some(line)) = (parts.next(), parts.next()) else { continue };
        let path = path.trim_start_matches("file://");
        if !ext.iter().any(|e| path.ends_with(e)) {
            continue;
        }
        if let Ok(n) = line.parse::<usize>() {
            return Some((path.to_string(), n));
        }
    }
    None
}

const MAX_MESSAGE: usize = 6;

/// Results so far, from everything the runner printed (colours removed).
pub fn parse(runner: Runner, out: &str) -> Vec<Outcome> {
    let out = strip_ansi(out);
    match runner {
        Runner::Go => parse_go(&out),
        Runner::Cargo => parse_cargo(&out),
        Runner::Jest => parse_jest(&out),
        Runner::Vitest => parse_vitest(&out),
        Runner::Pytest => parse_pytest(&out),
    }
}

fn upsert(list: &mut Vec<Outcome>, name: &str) -> usize {
    if let Some(i) = list.iter().position(|o| o.name == name) {
        return i;
    }
    list.push(Outcome { name: name.to_string(), status: Status::Running, location: None, message: Vec::new() });
    list.len() - 1
}

fn note(o: &mut Outcome, line: &str) {
    if o.message.len() < MAX_MESSAGE && !line.trim().is_empty() {
        o.message.push(line.trim().to_string());
    }
}

fn parse_go(out: &str) -> Vec<Outcome> {
    let mut list = Vec::new();
    let mut current: Option<usize> = None;
    for l in out.lines() {
        let t = l.trim();
        if let Some(name) = t.strip_prefix("=== RUN") {
            current = Some(upsert(&mut list, name.trim()));
        } else if let Some(rest) = t.strip_prefix("=== CONT").or_else(|| t.strip_prefix("=== PAUSE")) {
            current = list.iter().position(|o: &Outcome| o.name == rest.trim());
        } else if let Some(rest) = ["--- PASS:", "--- FAIL:", "--- SKIP:"].iter().find_map(|p| Some((*p, t.strip_prefix(p)?))) {
            let name = rest.1.trim().split(" (").next().unwrap_or("").to_string();
            let i = upsert(&mut list, &name);
            list[i].status = match rest.0 {
                "--- PASS:" => Status::Passed,
                "--- FAIL:" => Status::Failed,
                _ => Status::Skipped,
            };
        } else if let Some(i) = current {
            // `    file_test.go:12: message`, or a panic's trace.
            if let Some(loc) = location_in(t, &["_test.go"]) {
                let o = &mut list[i];
                if o.location.is_none() {
                    o.location = Some(loc);
                }
                let msg = t.split_once(": ").map_or(t, |(_, m)| m);
                note(o, msg);
            } else if t.starts_with("panic:") {
                note(&mut list[i], t);
            }
        } else if t.starts_with("./") || t.contains(".go:") {
            // A build error before any test ran.
            if let Some(loc) = location_in(t, &[".go"]) {
                let i = upsert(&mut list, "");
                list[i].status = Status::Failed;
                if list[i].location.is_none() {
                    list[i].location = Some(loc);
                }
                note(&mut list[i], t);
            }
        }
    }
    // A test whose subtests all ran keeps its own line; a parent that only
    // has children is fine as it is.
    list
}

fn parse_cargo(out: &str) -> Vec<Outcome> {
    let mut list = Vec::new();
    let mut failing: Option<usize> = None;
    let mut build: Option<String> = None;
    for l in out.lines() {
        let t = l.trim();
        if let Some(rest) = l.strip_prefix("test ") {
            if let Some((name, result)) = rest.rsplit_once(" ... ") {
                let i = upsert(&mut list, name.trim());
                list[i].status = match result.trim() {
                    "ok" => Status::Passed,
                    r if r.starts_with("ignored") => Status::Skipped,
                    _ => Status::Failed,
                };
                failing = None;
                continue;
            }
        }
        if let Some(name) = t.strip_prefix("---- ").and_then(|r| r.strip_suffix(" stdout ----")) {
            failing = Some(upsert(&mut list, name));
            continue;
        }
        if let Some(i) = failing {
            if t == "failures:" || t.starts_with("test result:") {
                failing = None;
                continue;
            }
            // `thread 'tests::subs' panicked at src/lib.rs:12:9:` (newer
            // Rust puts the thread id in: `'tests::subs' (3964) panicked`).
            if let Some((_, at)) = t.strip_prefix("thread '").and_then(|r| r.split_once(" panicked at ")) {
                list[i].location = location_in(at.trim_end_matches(':'), &[".rs"]);
                continue;
            }
            if t.starts_with("stack backtrace:") {
                // The backtrace says nothing the location doesn't.
                failing = None;
                continue;
            }
            if !t.starts_with("note:") {
                note(&mut list[i], t);
            }
            continue;
        }
        // A compile error: `error[E0425]: …` then `--> src/lib.rs:5:9`.
        if t.starts_with("error") {
            build = Some(t.to_string());
        } else if let (Some(msg), Some(rest)) = (&build, t.strip_prefix("--> ")) {
            let i = upsert(&mut list, "");
            list[i].status = Status::Failed;
            if list[i].location.is_none() {
                list[i].location = location_in(rest, &[".rs"]);
            }
            let msg = msg.clone();
            note(&mut list[i], &msg);
            build = None;
        }
    }
    list
}

/// `✓ title (3 ms)` → title.
fn strip_duration(s: &str) -> &str {
    let s = s.trim();
    if let Some(i) = s.rfind(" (") {
        if s.ends_with("ms)") || s.ends_with(" s)") {
            return s[..i].trim();
        }
    }
    // Vitest: `title 12ms`.
    match s.rsplit_once(' ') {
        Some((head, tail)) if tail.ends_with("ms") && tail[..tail.len() - 2].chars().all(|c| c.is_ascii_digit()) => head.trim(),
        _ => s,
    }
}

fn parse_jest(out: &str) -> Vec<Outcome> {
    let mut list = Vec::new();
    let mut failing: Option<usize> = None;
    for l in out.lines() {
        let t = l.trim();
        let mark = |m: &str| t.strip_prefix(m).map(|r| strip_duration(r).to_string());
        if let Some(name) = mark("✓").or_else(|| mark("√")) {
            let i = upsert(&mut list, &name);
            list[i].status = Status::Passed;
        } else if let Some(name) = mark("✕").or_else(|| mark("×")) {
            let i = upsert(&mut list, &name);
            list[i].status = Status::Failed;
        } else if let Some(name) = mark("○ skipped").or_else(|| mark("○ todo")) {
            let i = upsert(&mut list, &name);
            list[i].status = Status::Skipped;
        } else if let Some(path) = t.strip_prefix("● ") {
            // `● describe › title`: the details of a failure.
            let title = path.rsplit(" › ").next().unwrap_or(path).trim().to_string();
            failing = list.iter().position(|o: &Outcome| o.name == title).or_else(|| {
                let i = upsert(&mut list, &title);
                list[i].status = Status::Failed;
                Some(i)
            });
        } else if let Some(i) = failing {
            if list[i].location.is_none() {
                if let Some(loc) = location_in(t, &[".js", ".ts", ".jsx", ".tsx", ".mjs", ".cjs"]) {
                    if !loc.0.contains("node_modules") {
                        list[i].location = Some(loc);
                        continue;
                    }
                }
            }
            if list[i].location.is_none() && !t.starts_with("at ") {
                note(&mut list[i], t);
            }
        }
    }
    list
}

fn parse_vitest(out: &str) -> Vec<Outcome> {
    let mut list = Vec::new();
    let mut failing: Option<usize> = None;
    let title = |path: &str| strip_duration(path.rsplit(" > ").next().unwrap_or(path)).to_string();
    for l in out.lines() {
        let t = l.trim();
        if let Some(path) = t.strip_prefix("✓ ").or_else(|| t.strip_prefix("√ ")) {
            if path.contains(" > ") {
                let i = upsert(&mut list, &title(path));
                list[i].status = Status::Passed;
            }
        } else if let Some(path) = t.strip_prefix("× ").or_else(|| t.strip_prefix("✗ ")) {
            if path.contains(" > ") {
                let i = upsert(&mut list, &title(path));
                list[i].status = Status::Failed;
            }
        } else if let Some(path) = t.strip_prefix("↓ ") {
            if path.contains(" > ") {
                let i = upsert(&mut list, &title(path));
                list[i].status = Status::Skipped;
            }
        } else if let Some(path) = t.strip_prefix("FAIL ") {
            if path.contains(" > ") {
                let i = upsert(&mut list, &title(path));
                list[i].status = Status::Failed;
                failing = Some(i);
            }
        } else if let Some(i) = failing {
            if let Some(rest) = t.strip_prefix("❯ ").or_else(|| t.strip_prefix("> ")) {
                if list[i].location.is_none() {
                    list[i].location = location_in(rest, &[".js", ".ts", ".jsx", ".tsx", ".mjs", ".cjs"]);
                }
            } else if list[i].location.is_none() {
                note(&mut list[i], t);
            }
        }
    }
    list
}

fn parse_pytest(out: &str) -> Vec<Outcome> {
    let mut list = Vec::new();
    let mut section: Option<usize> = None;
    for l in out.lines() {
        let t = l.trim();
        // `test_math.py::test_add PASSED [ 50%]`
        if let Some((name, rest)) = t.split_once(' ').filter(|(n, _)| n.contains("::")) {
            let status = match rest.split_whitespace().next() {
                Some("PASSED") | Some("XFAIL") => Some(Status::Passed),
                Some("FAILED") | Some("ERROR") | Some("XPASS") => Some(Status::Failed),
                Some("SKIPPED") => Some(Status::Skipped),
                _ => None,
            };
            if let Some(status) = status {
                let i = upsert(&mut list, name);
                list[i].status = status;
                continue;
            }
        }
        // `____ test_sub ____` (or `____ TestMath.test_sub ____`) opens a failure.
        if t.starts_with("___") && t.ends_with("___") {
            let title = t.trim_matches('_').trim();
            let short = title.rsplit('.').next().unwrap_or(title);
            section = list.iter().position(|o: &Outcome| o.short_name() == short);
            continue;
        }
        if let Some(i) = section {
            if let Some(msg) = t.strip_prefix("E ") {
                note(&mut list[i], msg);
            } else if let Some(loc) = location_in(t, &[".py"]) {
                if t.starts_with(&loc.0) {
                    list[i].location = Some(loc);
                }
            }
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runners_by_name() {
        assert_eq!(runner_for("math_test.go", false), Some(Runner::Go));
        assert_eq!(runner_for("main.go", false), None);
        assert_eq!(runner_for("app.test.tsx", true), Some(Runner::Vitest));
        assert_eq!(runner_for("app.spec.js", false), Some(Runner::Jest));
        assert_eq!(runner_for("app.ts", false), None);
        assert_eq!(runner_for("test_math.py", false), Some(Runner::Pytest));
        assert_eq!(runner_for("lib.rs", false), Some(Runner::Cargo));
    }

    #[test]
    fn finds_tests() {
        let go =
            "package m\n\nfunc TestAdd(t *testing.T) {}\nfunc Testing() {}\nfunc BenchmarkX(b *testing.B) {}\nfunc helper() {}\n";
        let names: Vec<String> = find(Runner::Go, go).into_iter().map(|t| format!("{}@{}", t.name, t.line)).collect();
        assert_eq!(names, ["TestAdd@2", "BenchmarkX@4"]);

        let rs = "#[cfg(test)]\nmod tests {\n    #[test]\n    #[should_panic]\n    fn adds() {}\n    #[tokio::test]\n    async fn later() {}\n    fn helper() {}\n}\n";
        let names: Vec<String> = find(Runner::Cargo, rs).into_iter().map(|t| format!("{}@{}", t.name, t.line)).collect();
        assert_eq!(names, ["tests::adds@4", "tests::later@6"]);

        let js = "describe('greet', () => {\n  it('says hi', () => {})\n  test.each([[1, 2]])(\"doubles %i\", () => {})\n})\ntest(`top`, () => {})\n";
        let t = find(Runner::Jest, js);
        let names: Vec<(&str, &str, bool)> = t.iter().map(|t| (t.name.as_str(), t.label.as_str(), t.group)).collect();
        assert_eq!(
            names,
            [
                ("greet", "greet", true),
                ("greet says hi", "says hi", false),
                ("greet doubles %i", "doubles %i", false),
                ("top", "top", false)
            ]
        );

        let py = "import x\n\ndef test_add():\n    pass\n\nclass TestMath:\n    def test_sub(self):\n        pass\n\ndef helper():\n    pass\n";
        let names: Vec<String> = find(Runner::Pytest, py).into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["test_add", "TestMath", "TestMath::test_sub"]);
    }

    #[test]
    fn reads_go() {
        let out = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestSub\n    math_test.go:12: got 1, want 2\n--- FAIL: TestSub (0.00s)\n=== RUN   TestSub/neg\n    --- FAIL: TestSub/neg (0.00s)\nFAIL\n";
        let r = parse(Runner::Go, out);
        assert_eq!(r[0].status, Status::Passed);
        assert_eq!(r[1].status, Status::Failed);
        assert_eq!(r[1].location, Some(("math_test.go".into(), 12)));
        assert_eq!(r[1].message, ["got 1, want 2"]);
        assert_eq!(r[2].short_name(), "TestSub");
        let build = parse(Runner::Go, "# m\n./math_test.go:5:2: undefined: foo\nFAIL\tm [build failed]\n");
        assert_eq!((build[0].name.as_str(), build[0].location.clone()), ("", Some(("./math_test.go".into(), 5))));
    }

    #[test]
    fn reads_cargo() {
        let out = "running 2 tests\ntest tests::adds ... ok\ntest tests::subs ... FAILED\n\nfailures:\n\n---- tests::subs stdout ----\nthread 'tests::subs' panicked at src/lib.rs:12:9:\nassertion `left == right` failed\n  left: 1\n right: 2\nnote: run with `RUST_BACKTRACE=1`\n\nfailures:\n    tests::subs\n";
        let r = parse(Runner::Cargo, out);
        assert_eq!((r[0].status, r[1].status), (Status::Passed, Status::Failed));
        let newer = parse(Runner::Cargo, "test t::x ... FAILED\n---- t::x stdout ----\nthread 't::x' (3964) panicked at src/main.rs:20:9:\nboom\nstack backtrace:\n   0: __rustc::begin\n");
        assert_eq!(newer[0].location, Some(("src/main.rs".into(), 20)));
        assert_eq!(newer[0].message, ["boom"]);
        assert_eq!(r[1].location, Some(("src/lib.rs".into(), 12)));
        assert_eq!(r[1].message[0], "assertion `left == right` failed");
        assert_eq!(r[1].short_name(), "subs");
        let build = parse(Runner::Cargo, "error[E0425]: cannot find value `x`\n  --> src/lib.rs:5:9\n");
        assert_eq!(build[0].location, Some(("src/lib.rs".into(), 5)));
    }

    #[test]
    fn reads_jest_and_vitest() {
        let jest = "FAIL src/app.test.ts\n  greet\n    \u{1b}[32m✓\u{1b}[39m says hi (2 ms)\n    ✕ repeats (3 ms)\n\n  ● greet › repeats\n\n    expect(received).toBe(expected)\n\n      at Object.<anonymous> (src/app.test.ts:12:20)\n";
        let r = parse(Runner::Jest, jest);
        assert_eq!((r[0].name.as_str(), r[0].status), ("says hi", Status::Passed));
        assert_eq!((r[1].name.as_str(), r[1].status), ("repeats", Status::Failed));
        assert_eq!(r[1].location, Some(("src/app.test.ts".into(), 12)));
        assert_eq!(r[1].message, ["expect(received).toBe(expected)"]);

        let vitest = " ✓ src/app.test.ts > greet > says hi 1ms\n × src/app.test.ts > greet > repeats 2ms\n   → expected 'a' to be 'b'\n\n FAIL  src/app.test.ts > greet > repeats\nAssertionError: expected 'a' to be 'b'\n ❯ src/app.test.ts:12:20\n";
        let r = parse(Runner::Vitest, vitest);
        assert_eq!((r[0].name.as_str(), r[0].status), ("says hi", Status::Passed));
        assert_eq!((r[1].name.as_str(), r[1].status), ("repeats", Status::Failed));
        assert_eq!(r[1].location, Some(("src/app.test.ts".into(), 12)));
    }

    #[test]
    fn reads_pytest() {
        let out = "test_math.py::test_add PASSED      [ 50%]\ntest_math.py::TestMath::test_sub FAILED   [100%]\n\n=== FAILURES ===\n____ TestMath.test_sub ____\n\n    def test_sub(self):\n>       assert 1 == 2\nE       assert 1 == 2\n\ntest_math.py:7: AssertionError\n";
        let r = parse(Runner::Pytest, out);
        assert_eq!(r[0].status, Status::Passed);
        assert_eq!((r[1].short_name(), r[1].status), ("test_sub", Status::Failed));
        assert_eq!(r[1].location, Some(("test_math.py".into(), 7)));
        assert_eq!(r[1].message, ["assert 1 == 2"]);
    }
}
