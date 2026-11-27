//! Tasks: the commands you run for a project — `make dev`, `npm run
//! build`, `terraform plan -var-file=staging.tfvars` — picked from the
//! palette and run in a terminal tab of their own.
//!
//! They come from `.qell/tasks.toml` files (in the workspace and in the
//! folders above the open file), and from the nearest Makefile,
//! package.json scripts and justfile. Those are read when you open the
//! list, never watched, and nothing runs unless you pick it.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where tasks of your own are written, in a project's folder.
pub const CONFIG: &str = ".qell/tasks.toml";

/// A command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    pub command: String,
    /// Where it runs.
    pub cwd: PathBuf,
    /// Extra environment, set for the command only.
    pub env: Vec<(String, String)>,
    pub source: Source,
    /// What it does, when the file says.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Config,
    Make,
    Npm,
    Just,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Source::Config => "tasks.toml",
            Source::Make => "make",
            Source::Npm => "package.json",
            Source::Just => "just",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    task: Vec<ConfigTask>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigTask {
    name: String,
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    description: String,
}

/// The folders to look in: the open file's, up to the workspace's (or
/// the workspace's alone when the file is outside it). Nearest first.
fn folders(root: &Path, from: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(mut dir) = from.and_then(Path::parent).filter(|d| d.starts_with(root)) {
        loop {
            out.push(dir.to_path_buf());
            if dir == root {
                break;
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
    }
    if out.is_empty() {
        out.push(root.to_path_buf());
    }
    out
}

/// Every task for a file open in `from` (or the workspace), and what
/// couldn't be read.
pub fn discover(root: &Path, from: Option<&Path>) -> (Vec<Task>, Vec<String>) {
    let dirs = folders(root, from);
    let mut tasks = Vec::new();
    let mut errors = Vec::new();
    for dir in &dirs {
        let file = dir.join(CONFIG);
        if let Ok(text) = std::fs::read_to_string(&file) {
            match from_config(&text, dir) {
                Ok(t) => tasks.extend(t),
                Err(e) => errors.push(format!("{}: {e}", file.strip_prefix(root).unwrap_or(&file).display())),
            }
        }
    }
    let nearest = |names: &[&str]| -> Option<(PathBuf, String)> {
        dirs.iter().find_map(|d| names.iter().find_map(|n| std::fs::read_to_string(d.join(n)).ok().map(|t| (d.clone(), t))))
    };
    if let Some((dir, text)) = nearest(&["GNUmakefile", "makefile", "Makefile"]) {
        tasks.extend(make_targets(&text).into_iter().map(|(name, detail)| Task {
            command: format!("make {name}"),
            name,
            cwd: dir.clone(),
            env: Vec::new(),
            source: Source::Make,
            detail,
        }));
    }
    if let Some((dir, text)) = nearest(&["package.json"]) {
        let runner = package_runner(&dir, root);
        match npm_scripts(&text) {
            Ok(scripts) => tasks.extend(scripts.into_iter().map(|(name, script)| Task {
                command: format!("{runner} {name}"),
                name,
                cwd: dir.clone(),
                env: Vec::new(),
                source: Source::Npm,
                detail: script,
            })),
            Err(e) => errors.push(format!("package.json: {e}")),
        }
    }
    if let Some((dir, text)) = nearest(&["justfile", "Justfile", ".justfile"]) {
        tasks.extend(just_recipes(&text).into_iter().map(|(name, detail)| Task {
            command: format!("just {name}"),
            name,
            cwd: dir.clone(),
            env: Vec::new(),
            source: Source::Just,
            detail,
        }));
    }
    (tasks, errors)
}

/// The tasks of a `.qell/tasks.toml` in `dir`.
fn from_config(text: &str, dir: &Path) -> Result<Vec<Task>, String> {
    let file: ConfigFile = toml::from_str(text).map_err(|e| e.message().to_string() + &line_of(text, e.span()))?;
    Ok(file
        .task
        .into_iter()
        .map(|t| Task {
            cwd: match t.cwd.as_deref() {
                Some(c) if !c.is_empty() => dir.join(c),
                _ => dir.to_path_buf(),
            },
            name: t.name,
            env: t.env.into_iter().collect(),
            detail: if t.description.is_empty() { t.command.clone() } else { t.description },
            command: t.command,
            source: Source::Config,
        })
        .collect())
}

fn line_of(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    span.map(|s| format!(" (line {})", text[..s.start.min(text.len())].lines().count().max(1))).unwrap_or_default()
}

/// The targets of a Makefile you'd run by name, with their description
/// (`target: ## what it does`, or a `#` comment just above).
pub fn make_targets(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut comment = String::new();
    let mut in_define = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if in_define {
            in_define = trimmed != "endef";
            continue;
        }
        if trimmed.starts_with("define ") || trimmed == "define" {
            in_define = true;
            continue;
        }
        if line.starts_with('\t') {
            continue; // a recipe line
        }
        if let Some(c) = trimmed.strip_prefix('#') {
            comment = c.trim_start_matches('#').trim().to_string();
            continue;
        }
        let Some(colon) = line.find(':') else {
            comment.clear();
            continue;
        };
        let (head, rest) = (&line[..colon], &line[colon + 1..]);
        // `X := 1`, `X ::= 1` and `x: y = 1` (target variables) aren't rules.
        if rest.starts_with('=')
            || rest.starts_with(":=")
            || head.contains('=')
            || rest.split("##").next().unwrap_or("").contains('=')
        {
            comment.clear();
            continue;
        }
        let detail = match rest.split_once("##") {
            Some((_, d)) => d.trim().to_string(),
            None => std::mem::take(&mut comment),
        };
        for name in head.split_whitespace() {
            let runnable = !name.starts_with('.')
                && !name.contains(['%', '$', '/', '(', ')'])
                && !name.ends_with(".o")
                && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if runnable && !out.iter().any(|(n, _)| n == name) {
                out.push((name.to_string(), detail.clone()));
            }
        }
        comment.clear();
    }
    out
}

/// The scripts of a package.json, in order, without the `pre`/`post`
/// hooks npm runs on its own.
pub fn npm_scripts(text: &str) -> Result<Vec<(String, String)>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) else { return Ok(Vec::new()) };
    let names: Vec<&String> = scripts.keys().collect();
    Ok(scripts
        .iter()
        .filter(|(name, _)| {
            let hook = |p: &str| name.strip_prefix(p).is_some_and(|rest| names.iter().any(|n| n.as_str() == rest));
            !hook("pre") && !hook("post")
        })
        .map(|(name, s)| (name.clone(), s.as_str().unwrap_or_default().to_string()))
        .collect())
}

/// `npm run`, or the package manager whose lock file is there (or in a
/// folder above, for a workspace), up to `root`.
fn package_runner(dir: &Path, root: &Path) -> &'static str {
    let mut d = Some(dir);
    while let Some(here) = d {
        for (lock, runner) in [
            ("pnpm-lock.yaml", "pnpm run"),
            ("yarn.lock", "yarn run"),
            ("bun.lockb", "bun run"),
            ("bun.lock", "bun run"),
            ("package-lock.json", "npm run"),
        ] {
            if here.join(lock).exists() {
                return runner;
            }
        }
        if here == root {
            break;
        }
        d = here.parent();
    }
    "npm run"
}

/// The public recipes of a justfile, with the comment above each.
pub fn just_recipes(text: &str) -> Vec<(String, String)> {
    const KEYWORDS: [&str; 6] = ["set", "alias", "export", "import", "mod", "unexport"];
    let mut out = Vec::new();
    let mut comment = String::new();
    let mut private = false;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) || line.trim().is_empty() {
            if line.trim().is_empty() {
                comment.clear();
                private = false;
            }
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            if !c.starts_with('!') {
                comment = c.trim().to_string();
            }
            continue;
        }
        if line.starts_with('[') {
            private |= line.contains("private");
            continue;
        }
        let head = line.trim_start_matches('@');
        let name: String = head.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
        let after = head[name.len()..].trim_start();
        // `name params: deps`, not `name := value`.
        let is_recipe = !name.is_empty()
            && !KEYWORDS.contains(&name.as_str())
            && after.find(':').is_some_and(|i| !after[i + 1..].starts_with('='));
        if is_recipe && !name.starts_with('_') && !private {
            out.push((name, std::mem::take(&mut comment)));
        }
        comment.clear();
        private = false;
    }
    out
}

/// What `.qell/tasks.toml` starts as.
pub fn template(found: &[Task]) -> String {
    let mut s = String::from(
        "# Tasks for this project: Tasks: Run Task… lists them, and runs the one you\n\
         # pick in a terminal tab of its own (with bash, from the folder holding .qell/,\n\
         # or `cwd` below it).\n\
         #\n\
         # [[task]]\n\
         # name = \"api\"\n\
         # command = \"go run ./cmd/api\"\n\
         # cwd = \"api\"                         # optional\n\
         # env = { PORT = \"8080\" }             # optional\n\
         # description = \"The API on :8080\"    # optional\n",
    );
    if !found.is_empty() {
        s.push_str("#\n# Found already (no need to repeat them here):\n");
        for t in found.iter().take(12) {
            s.push_str(&format!("#   {}  ({})\n", t.command, t.source.tag()));
        }
    }
    s.push_str("\n[[task]]\nname = \"hello\"\ncommand = \"echo hello from Qell\"\n");
    s
}

/// `'…'` for sh, whatever is inside.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The bash script a task runs as: show the command, run it with its
/// environment, say how it ended, then stay as a shell in that folder.
pub fn script(task: &Task) -> String {
    let mut s = format!("printf '\\033[2m$ %s\\033[0m\\n\\n' {}; ", quote(&task.command));
    for (k, v) in &task.env {
        if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.is_empty() {
            s.push_str(&format!("export {k}={}; ", quote(v)));
        }
    }
    s.push_str(&task.command);
    s.push_str(
        "\ns=$?; if [ $s -eq 0 ]; then printf '\\n\\033[2m✓ done\\033[0m\\n'; \
         else printf '\\n\\033[31m✗ exited with %s\\033[0m\\n' \"$s\"; fi; exec \"${SHELL:-bash}\"",
    );
    s
}

/// The tasks found, being picked from in the palette.
#[derive(Debug, Clone, Default)]
pub struct Pick {
    pub tasks: Vec<Task>,
}

impl crate::app::QellApp {
    /// The open file, for finding the tasks nearest to it.
    fn task_anchor(&self) -> Option<PathBuf> {
        self.docs.get(self.active).and_then(|d| d.buffer.path()).map(Path::to_path_buf)
    }

    /// List the tasks in the palette.
    pub fn pick_task(&mut self) {
        let (tasks, errors) = discover(&self.root, self.task_anchor().as_deref());
        if let Some(e) = errors.first() {
            self.notify_error(format!("Cannot read {e}"));
        }
        if tasks.is_empty() {
            if errors.is_empty() {
                self.notify("No tasks here: no Makefile, package.json or justfile. Tasks: Configure Tasks adds your own.");
            }
            return;
        }
        self.task_pick = Pick { tasks };
        self.open_palette(crate::app::PaletteMode::Tasks);
    }

    pub fn run_task_at(&mut self, i: usize) {
        if let Some(t) = self.task_pick.tasks.get(i).cloned() {
            self.run_task(t);
        }
    }

    /// Run a task in its own terminal tab; running it again replaces it.
    pub fn run_task(&mut self, task: Task) {
        if !task.cwd.is_dir() {
            return self.notify_error(format!("Task {}: no folder {}", task.name, task.cwd.display()));
        }
        let label = format!("task {}", task.name);
        if let Some(i) = self.terminals.iter().position(|t| t.name.as_deref() == Some(label.as_str())) {
            self.kill_terminal(i);
        }
        // The terminal gets the keyboard: Ctrl+C stops a server.
        if self.run_in_terminal_at(label, task.cwd.clone(), "bash", vec!["-c".into(), script(&task)]).is_none() {
            return;
        }
        if self.config.journal.enabled {
            crate::activity::record(&crate::activity::Event::Task {
                t: crate::activity::now(),
                cwd: task.cwd.clone(),
                name: task.name.clone(),
                command: task.command.clone(),
            });
        }
        self.notify(format!("Running {}", task.command));
        self.last_task = Some(task);
    }

    pub fn rerun_task(&mut self) {
        match self.last_task.clone() {
            Some(t) => self.run_task(t),
            None => self.pick_task(),
        }
    }

    /// Open the workspace's `.qell/tasks.toml`, made from a template the
    /// first time.
    pub fn configure_tasks(&mut self) {
        let file = self.root.join(CONFIG);
        if !file.exists() {
            let (found, _) = discover(&self.root, None);
            let made =
                file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&file, template(&found)));
            if let Err(e) = made {
                return self.notify_error(format!("Cannot create {CONFIG}: {e}"));
            }
            self.files_changed();
        }
        self.open_file(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makefile_targets_with_descriptions() {
        let mk = "\
.PHONY: build test
APP := shop
VERSION ?= 1.0

# Build the binary
build: deps
\tgo build -o $(APP) ./cmd/api

test: ## Run the tests
\tgo test ./...

deps lint:
\t@true

%.o: %.c
\tcc -c $<

bin/app: build
\tcp $(APP) bin/app

dev: export STAGE = local
dev:
\tgo run ./cmd/api

define HELP
fake: target
endef
";
        let t = make_targets(mk);
        let names: Vec<&str> = t.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["build", "test", "deps", "lint", "dev"]);
        assert_eq!(t[0].1, "Build the binary");
        assert_eq!(t[1].1, "Run the tests");
        assert_eq!(t[2].1, "");
    }

    #[test]
    fn npm_scripts_without_hooks() {
        let pkg = r#"{"name":"web","scripts":{"prebuild":"rm -rf dist","build":"vite build","dev":"vite","postinstall":"x","pretty":"prettier -w ."}}"#;
        let s = npm_scripts(pkg).unwrap();
        let names: Vec<&str> = s.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["build", "dev", "postinstall", "pretty"]);
        assert_eq!(npm_scripts(r#"{"name":"x"}"#).unwrap(), []);
        assert!(npm_scripts("{").is_err());
    }

    #[test]
    fn just_recipes_public_only() {
        let j = "\
set dotenv-load
alias b := build
version := \"1.0\"

# Build everything
build:
    cargo build

[private]
helper:
    echo hi

_hidden:
    echo no

@deploy env='staging': build
    ./deploy {{env}}
";
        let r = just_recipes(j);
        assert_eq!(r, [("build".to_string(), "Build everything".to_string()), ("deploy".to_string(), String::new())]);
    }

    #[test]
    fn config_tasks() {
        let dir = Path::new("/w/shop");
        let t = from_config(
            "[[task]]\nname = \"api\"\ncommand = \"go run ./cmd/api\"\ncwd = \"api\"\nenv = { PORT = \"8080\" }\n\n[[task]]\nname = \"plan\"\ncommand = \"terraform plan\"\ndescription = \"Plan staging\"\n",
            dir,
        )
        .unwrap();
        assert_eq!(t[0].cwd, Path::new("/w/shop/api"));
        assert_eq!(t[0].env, [("PORT".to_string(), "8080".to_string())]);
        assert_eq!(t[0].detail, "go run ./cmd/api");
        assert_eq!(t[1].cwd, dir);
        assert_eq!(t[1].detail, "Plan staging");
        let e = from_config("[[task]]\nname = \"x\"\n", dir).unwrap_err();
        assert!(e.contains("command") && e.contains("line"), "{e}");
        assert!(from_config("[[task]]\nname = \"x\"\ncommand = \"y\"\ncmd = 1\n", dir).is_err());
    }

    #[test]
    fn discovers_nearest_files_and_every_config() {
        let root = std::env::temp_dir().join(format!("qell-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let api = root.join("api");
        std::fs::create_dir_all(api.join(".qell")).unwrap();
        std::fs::create_dir_all(root.join(".qell")).unwrap();
        std::fs::write(root.join("Makefile"), "all:\n\ttrue\n").unwrap();
        std::fs::write(api.join("Makefile"), "run:\n\ttrue\n").unwrap();
        std::fs::write(root.join("package.json"), r#"{"scripts":{"lint":"eslint ."}}"#).unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(api.join(".qell/tasks.toml"), "[[task]]\nname = \"a\"\ncommand = \"true\"\n").unwrap();
        std::fs::write(root.join(".qell/tasks.toml"), "[[task]]\nname = \"b\"\ncommand = \"true\"\n").unwrap();
        let (t, errors) = discover(&root, Some(&api.join("main.go")));
        assert!(errors.is_empty());
        let cmds: Vec<&str> = t.iter().map(|t| t.command.as_str()).collect();
        assert_eq!(cmds, ["true", "true", "make run", "pnpm run lint"]);
        assert_eq!(t[0].name, "a");
        assert_eq!(t[2].cwd, api);
        // From the workspace alone: its own Makefile.
        let (t, _) = discover(&root, None);
        assert!(t.iter().any(|t| t.command == "make all"));
        std::fs::write(root.join(".qell/tasks.toml"), "[[task]\n").unwrap();
        let (_, errors) = discover(&root, None);
        assert!(errors[0].starts_with(".qell/tasks.toml:"), "{errors:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn script_shows_runs_and_stays() {
        let t = Task {
            name: "api".into(),
            command: "go run ./cmd/api".into(),
            cwd: "/w".into(),
            env: vec![("PORT".into(), "80'80".into()), ("bad name".into(), "x".into())],
            source: Source::Config,
            detail: String::new(),
        };
        let s = script(&t);
        assert!(s.starts_with("printf '\\033[2m$ %s\\033[0m\\n\\n' 'go run ./cmd/api'; "));
        assert!(s.contains("export PORT='80'\\''80'; go run ./cmd/api\n"));
        assert!(!s.contains("bad name"));
        assert!(s.ends_with("exec \"${SHELL:-bash}\""));
        let out = std::process::Command::new("bash")
            .args([
                "-c",
                &script(&Task { command: "echo $PORT".into(), env: vec![("PORT".into(), "8080".into())], ..t.clone() })
                    .replace("exec \"${SHELL:-bash}\"", "true"),
            ])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("8080") && text.contains("done"), "{text}");
    }

    #[test]
    fn template_parses_and_lists_what_was_found() {
        let found = [Task {
            name: "build".into(),
            command: "make build".into(),
            cwd: "/w".into(),
            env: vec![],
            source: Source::Make,
            detail: String::new(),
        }];
        let t = template(&found);
        assert!(t.contains("#   make build  (make)"));
        assert_eq!(from_config(&t, Path::new("/w")).unwrap()[0].name, "hello");
    }
}
