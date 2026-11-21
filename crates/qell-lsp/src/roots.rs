//! Where a language server should be rooted.
//!
//! A folder you open is often a folder of projects — `~/go/src`, a
//! monorepo, a scratch directory. Starting one server at that folder and
//! pointing it at everything inside is how you get *cannot find package
//! in GOROOT or GOPATH*: gopls needs the module's own directory, the one
//! with `go.mod`, not the drawer it sits in. So the root is looked for
//! upwards from the file itself, and the server is started there.

use std::path::{Path, PathBuf};

/// The files and folders that mark the top of a project, per language.
/// Order matters: the first marker found walking up wins, and within one
/// directory the earlier marker wins.
fn markers(lang_id: &str) -> &'static [&'static str] {
    match lang_id {
        "go" => &["go.work", "go.mod"],
        "rust" => &["Cargo.toml"],
        "javascript" | "javascriptreact" | "typescript" | "typescriptreact" => {
            &["deno.json", "deno.jsonc", "tsconfig.json", "jsconfig.json", "package.json"]
        }
        "python" => &["pyproject.toml", "setup.py", "setup.cfg", "Pipfile", "requirements.txt"],
        "c" | "cpp" => &["compile_commands.json", "CMakeLists.txt", "Makefile"],
        "ruby" => &["Gemfile"],
        "php" => &["composer.json"],
        "java" => &["pom.xml", "build.gradle", "build.gradle.kts"],
        "terraform" => &[".terraform.lock.hcl", ".terraform"],
        "elixir" => &["mix.exs"],
        "haskell" => &["stack.yaml", "cabal.project"],
        "zig" => &["build.zig"],
        _ => &[],
    }
}

/// Markers that tie several projects together, so the server belongs at
/// the outer one: a Cargo workspace is one build, and `go.work` is what
/// gopls wants when it is there. A nested `go.mod`, by contrast, is its
/// own module and keeps its own server.
fn enclosing(lang_id: &str) -> &'static [&'static str] {
    match lang_id {
        "go" => &["go.work"],
        "rust" => &["Cargo.toml"],
        _ => &[],
    }
}

/// Every project marker, so a language Qell has no list for still lands
/// on something sensible.
const REPOSITORY: &[&str] = &[".git", ".hg", ".svn"];

/// Where to start the server for `file`, looking up from it as far as
/// `workspace` and no further: the nearest directory holding one of the
/// language's markers, else the nearest repository, else the workspace.
///
/// Qell doesn't go above the folder you opened. A server started outside
/// it would read files you didn't ask it to.
pub fn project_root(file: Option<&Path>, workspace: &Path, lang_id: &str) -> PathBuf {
    let Some(dir) = file.and_then(Path::parent) else { return workspace.to_path_buf() };
    // Only files inside the workspace get their own root.
    if !dir.starts_with(workspace) {
        return workspace.to_path_buf();
    }
    let language = markers(lang_id);
    let mut repository: Option<PathBuf> = None;
    let mut at = Some(dir);
    while let Some(here) = at {
        if language.iter().any(|m| here.join(m).exists()) {
            return outermost(here, workspace, enclosing(lang_id));
        }
        if repository.is_none() && REPOSITORY.iter().any(|m| here.join(m).exists()) {
            repository = Some(here.to_path_buf());
        }
        if here == workspace {
            break;
        }
        at = here.parent();
    }
    repository.unwrap_or_else(|| workspace.to_path_buf())
}

/// Keep walking up for a marker that encloses this project.
fn outermost(from: &Path, workspace: &Path, markers: &[&str]) -> PathBuf {
    if markers.is_empty() {
        return from.to_path_buf();
    }
    let mut best = from.to_path_buf();
    let mut at = from;
    while at != workspace {
        let Some(parent) = at.parent() else { break };
        if !parent.starts_with(workspace) {
            break;
        }
        if markers.iter().any(|m| parent.join(m).exists()) {
            best = parent.to_path_buf();
        }
        at = parent;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp dir");
        for f in files {
            let path = dir.path().join(f);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(&path, "").expect("write");
        }
        dir
    }

    #[test]
    fn a_go_module_under_a_folder_of_projects_gets_its_own_root() {
        // What `~/go/src` looks like: several modules side by side.
        let d = make(&["gohead/go.mod", "gohead/main.go", "gojelly/go.mod", "gojelly/main.go"]);
        let (ws, file) = (d.path(), d.path().join("gohead/main.go"));
        assert_eq!(project_root(Some(&file), ws, "go"), d.path().join("gohead"));
        let other = d.path().join("gojelly/main.go");
        assert_eq!(project_root(Some(&other), ws, "go"), d.path().join("gojelly"));
    }

    #[test]
    fn the_module_is_found_from_a_file_deep_inside_it() {
        let d = make(&["gohead/go.mod", "gohead/internal/api/server.go"]);
        let file = d.path().join("gohead/internal/api/server.go");
        assert_eq!(project_root(Some(&file), d.path(), "go"), d.path().join("gohead"));
    }

    #[test]
    fn a_go_work_above_the_module_is_the_root() {
        let d = make(&["ws/go.work", "ws/api/go.mod", "ws/api/main.go"]);
        let file = d.path().join("ws/api/main.go");
        // It ties the modules together, and gopls wants to see all of them.
        assert_eq!(project_root(Some(&file), d.path(), "go"), d.path().join("ws"));
    }

    #[test]
    fn a_module_inside_a_module_keeps_its_own_root() {
        // No `go.work`: the inner one is a module of its own.
        let d = make(&["outer/go.mod", "outer/tools/go.mod", "outer/tools/main.go"]);
        let file = d.path().join("outer/tools/main.go");
        assert_eq!(project_root(Some(&file), d.path(), "go"), d.path().join("outer/tools"));
    }

    #[test]
    fn a_crate_of_a_cargo_workspace_roots_at_the_workspace() {
        let d = make(&["repo/Cargo.toml", "repo/crates/core/Cargo.toml", "repo/crates/core/src/lib.rs"]);
        let file = d.path().join("repo/crates/core/src/lib.rs");
        assert_eq!(project_root(Some(&file), d.path(), "rust"), d.path().join("repo"));
    }

    #[test]
    fn without_a_language_marker_the_repository_is_the_root() {
        let d = make(&["site/.git/config", "site/index.html"]);
        let file = d.path().join("site/index.html");
        assert_eq!(project_root(Some(&file), d.path(), "html"), d.path().join("site"));
    }

    #[test]
    fn with_nothing_to_go_on_it_stays_where_you_opened() {
        let d = make(&["notes/main.go"]);
        let file = d.path().join("notes/main.go");
        assert_eq!(project_root(Some(&file), d.path(), "go"), d.path());
        // And a file outside the workspace doesn't drag the server out.
        let outside = PathBuf::from("/elsewhere/main.go");
        assert_eq!(project_root(Some(&outside), d.path(), "go"), d.path());
        assert_eq!(project_root(None, d.path(), "go"), d.path());
    }

    #[test]
    fn the_search_stops_at_the_folder_you_opened() {
        // The module is above the workspace: Qell doesn't reach for it.
        let d = make(&["gohead/go.mod", "gohead/internal/api/server.go"]);
        let ws = d.path().join("gohead/internal");
        let file = d.path().join("gohead/internal/api/server.go");
        assert_eq!(project_root(Some(&file), &ws, "go"), ws);
    }
}
