//! Branches and stashes from the palette: Git: Switch Branch…, Create,
//! Delete, Stash, Pop… Each runs in the background like the Source Control
//! view's commands; git's refusals come back as what to do about them.

use std::path::{Path, PathBuf};

use qell_git::refs::{self, Branch, Stash};

use crate::app::{GitPick, PaletteMode, QellApp, StashAction};
use crate::views::scm::JobKind;

impl QellApp {
    /// The repository branch and stash commands work on: the one of the
    /// file being edited, else the workspace's.
    fn git_repo(&self) -> Option<PathBuf> {
        if !self.config.git_status {
            return None;
        }
        self.active_repo().map(|(root, _)| root.to_path_buf())
    }

    /// Open the palette on the branches or stashes of the active repository.
    pub fn pick_git(&mut self, mode: PaletteMode) {
        let Some(repo) = self.git_repo() else {
            self.notify("No git repository here: open a file inside one, or a folder that is one.");
            return;
        };
        self.pick_git_in(repo, mode);
    }

    /// The same, for a given repository (the Source Control view's).
    pub fn pick_git_in(&mut self, repo: PathBuf, mode: PaletteMode) {
        let dir = self.root.join(&repo);
        let needs_branches = matches!(mode, PaletteMode::Branches | PaletteMode::DeleteBranch { .. });
        let needs_stashes = matches!(mode, PaletteMode::Stashes(_));
        let mut pick = GitPick { repo, ..Default::default() };
        if needs_branches {
            match refs::branches(&dir) {
                Ok(b) => pick.branches = b,
                Err(e) => return self.notify_error(format!("Cannot list the branches: {e}")),
            }
            if matches!(mode, PaletteMode::DeleteBranch { .. }) {
                pick.branches.retain(|b| !b.remote && !b.current);
                if pick.branches.is_empty() {
                    return self.notify("No other local branch to delete.");
                }
            }
        }
        if needs_stashes {
            match refs::stashes(&dir) {
                Ok(s) if s.is_empty() => return self.notify("No stashes in this repository."),
                Ok(s) => pick.stashes = s,
                Err(e) => return self.notify_error(format!("Cannot list the stashes: {e}")),
            }
        }
        self.git_pick = pick;
        self.open_palette(mode);
    }

    fn run_git_job(&mut self, kind: JobKind, job: impl FnOnce(&Path) -> Result<String, String> + Send + 'static) {
        let root = self.root.clone();
        let repo = self.git_pick.repo.clone();
        self.scm.spawn(&root, &repo, kind, job);
    }

    /// Switch to the picked branch.
    pub fn switch_branch(&mut self, i: usize) {
        let Some(branch) = self.git_pick.branches.get(i).cloned() else { return };
        if branch.current {
            return self.notify(format!("Already on {}.", branch.name));
        }
        let locals: Vec<Branch> = self.git_pick.branches.iter().filter(|b| !b.remote).cloned().collect();
        self.notify(format!("Switching to {}…", branch.name));
        self.run_git_job(JobKind::Switch, move |dir| {
            refs::switch(dir, &branch, &locals).map(|_| format!("Switched to {}", branch.name))
        });
    }

    /// Make a branch where you are and switch to it.
    pub fn create_branch(&mut self, name: &str) {
        let name = name.trim().to_string();
        let Some(repo) = self.git_repo() else { return };
        self.git_pick.repo = repo;
        let dir = self.root.join(&self.git_pick.repo);
        if !refs::valid_branch_name(&dir, &name) {
            return self.notify_error(format!(
                "\"{name}\" can't be a branch name: no spaces, no `..`, `~`, `^`, `:`, `?`, `*`, `[`, and it can't end with `/` or `.lock`."
            ));
        }
        self.run_git_job(JobKind::CreateBranch, move |dir| {
            refs::create(dir, &name).map(|_| format!("Created and switched to {name}"))
        });
    }

    pub fn delete_branch(&mut self, i: usize, force: bool) {
        let Some(branch) = self.git_pick.branches.get(i).cloned() else { return };
        self.run_git_job(JobKind::DeleteBranch, move |dir| {
            refs::delete(dir, &branch.name, force).map(|_| format!("Deleted branch {}", branch.name))
        });
    }

    pub fn stash(&mut self, message: &str, untracked: bool) {
        let message = message.to_string();
        self.run_git_job(JobKind::Stash, move |dir| {
            let out = refs::stash(dir, &message, untracked)?;
            if out.contains("No local changes to save") {
                Ok("Nothing to stash: no changes.".to_string())
            } else {
                Ok("Changes stashed. Git: Pop Latest Stash brings them back.".to_string())
            }
        });
    }

    pub fn stash_action(&mut self, i: usize, action: StashAction) {
        let Some(stash) = self.git_pick.stashes.get(i).cloned() else { return };
        self.run_stash(stash, action);
    }

    fn run_stash(&mut self, stash: Stash, action: StashAction) {
        let what = short_stash(&stash.message);
        match action {
            StashAction::Drop => self.run_git_job(JobKind::Unstash, move |dir| {
                refs::drop_stash(dir, &stash).map(|_| format!("Dropped stash: {what}"))
            }),
            StashAction::Apply | StashAction::Pop => {
                let pop = action == StashAction::Pop;
                self.run_git_job(JobKind::Unstash, move |dir| {
                    refs::unstash(dir, &stash, pop).map(|_| format!("{} stash: {what}", if pop { "Popped" } else { "Applied" }))
                })
            }
        }
    }

    /// Git: Pop Latest Stash.
    pub fn pop_latest_stash(&mut self) {
        let Some(repo) = self.git_repo() else {
            return self.notify("No git repository here: open a file inside one, or a folder that is one.");
        };
        match refs::stashes(&self.root.join(&repo)) {
            Ok(s) if s.is_empty() => self.notify("No stashes in this repository."),
            Ok(mut s) => {
                self.git_pick.repo = repo;
                self.run_stash(s.remove(0), StashAction::Pop);
            }
            Err(e) => self.notify_error(format!("Cannot list the stashes: {e}")),
        }
    }

    /// The Source Control view's buttons on one stash.
    pub fn stash_button(&mut self, repo: &Path, stash: Stash, action: StashAction) {
        self.git_pick.repo = repo.to_path_buf();
        self.run_stash(stash, action);
    }
}

/// "On main: wip" → "wip"; "WIP on main: 1a2b3c4 subject" stays readable.
pub fn short_stash(message: &str) -> String {
    match message.strip_prefix("On ").and_then(|m| m.split_once(": ")) {
        Some((_, m)) => m.to_string(),
        None => message.to_string(),
    }
}

/// What to do when git refused a branch or stash command.
pub fn explain(kind: JobKind, err: &str) -> String {
    // git lists the files concerned indented by a tab.
    let files: Vec<&str> = err.lines().filter(|l| l.starts_with('\t')).map(str::trim).take(4).collect();
    let files = if files.is_empty() { String::new() } else { format!(" ({})", files.join(", ")) };
    if err.contains("would be overwritten") {
        return match kind {
            JobKind::Unstash => format!("The stash changes files you have changed too{files}. Commit or stash those first."),
            _ => format!(
                "Switching would lose your changes{files}. Git: Stash them, switch, then Git: Pop Latest Stash to bring them back."
            ),
        };
    }
    if err.contains("not fully merged") {
        return "That branch has commits that aren't merged anywhere. Git: Force Delete Branch… removes it anyway.".to_string();
    }
    if err.contains("already exists") {
        return "A branch with that name already exists: Git: Switch Branch… to go to it.".to_string();
    }
    if err.contains("CONFLICT") || err.contains("conflict") {
        return "The stash conflicts with your files: they now hold conflict markers to resolve. The stash stays in the list until you drop it.".to_string();
    }
    let last = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or(err);
    format!("git: {}", last.trim().trim_start_matches("error: ").trim_start_matches("fatal: "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_say_what_to_do() {
        let dirty = "error: Your local changes to the following files would be overwritten by checkout:\n\tsrc/main.rs\nPlease commit your changes or stash them before you switch branches.\nAborting";
        let e = explain(JobKind::Switch, dirty);
        assert!(e.contains("Git: Stash") && e.contains("(src/main.rs)"), "{e}");
        assert!(explain(JobKind::DeleteBranch, "error: the branch 'x' is not fully merged.").contains("Force Delete"));
        assert_eq!(explain(JobKind::Switch, "fatal: invalid reference: nope"), "git: invalid reference: nope");
        assert_eq!(short_stash("On main: half done"), "half done");
        assert_eq!(short_stash("WIP on main: 1a2b3c4 subject"), "WIP on main: 1a2b3c4 subject");
    }
}
