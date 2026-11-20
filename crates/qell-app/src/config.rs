//! User configuration, read from `~/.config/qell/config.toml`.
//!
//! Every field has a default, so an absent or partial file is fine. The
//! file is written out with the defaults the first time Qell runs so users
//! have something to edit. The Settings screen saves back into the same
//! file with [`Config::save`], editing values in place so the user's own
//! comments and ordering survive.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Editor font size in points.
    pub font_size: f32,
    /// Spaces inserted by the Tab key.
    pub tab_size: usize,
    /// Ask language servers for completions while typing (not only on Ctrl+Space).
    pub auto_complete: bool,
    /// Colour bracket pairs by nesting depth, unmatched ones in red.
    pub bracket_pair_colors: bool,
    /// Vertical lines at each indentation level, the current block's brighter.
    pub indent_guides: bool,
    /// Scale of the whole interface (1.0 = 100%).
    pub ui_scale: f32,
    /// Show file-type icons in the file tree and tabs.
    pub file_icons: bool,
    /// Colour the file tree and tabs by git status.
    pub git_status: bool,
    /// Folder levels searched for git repositories below the workspace
    /// (for a folder holding several repos). 0: only the workspace's own.
    pub git_scan_depth: usize,
    /// Show the file tree on startup.
    pub show_sidebar: bool,
    /// Show the terminal panel on startup.
    pub show_terminal: bool,
    /// Shell command for new terminals, e.g. `zsh -l`. Empty: `$SHELL`.
    pub shell: String,
    /// Terminal font size in points. 0: same as the editor.
    pub terminal_font_size: f32,
    /// Names hidden from the file tree.
    pub tree_hidden: Vec<String>,
    /// Directories and names the quick-open file list skips.
    pub ignore: Vec<String>,
    /// Extra language-server commands, overriding the built-ins, keyed by
    /// language id. Example: `rust = ["rust-analyzer"]`.
    pub lsp: BTreeMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            tab_size: 4,
            auto_complete: true,
            bracket_pair_colors: true,
            indent_guides: true,
            ui_scale: 1.0,
            file_icons: true,
            git_status: true,
            git_scan_depth: 3,
            show_sidebar: true,
            show_terminal: true,
            shell: String::new(),
            terminal_font_size: 0.0,
            tree_hidden: vec![".git".into(), ".DS_Store".into()],
            ignore: vec![".git".into(), "target".into(), "node_modules".into(), ".terraform".into(), "dist".into()],
            lsp: Default::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("qell")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

impl Config {
    /// Where to look for git repositories, from these settings.
    pub fn git_scan(&self) -> qell_git::ScanOptions {
        let mut skip: Vec<String> = self.ignore.iter().filter(|n| *n != ".git").cloned().collect();
        skip.extend(qell_git::ScanOptions::default().skip);
        skip.dedup();
        qell_git::ScanOptions { depth: self.git_scan_depth, skip }
    }

    /// Load the config, writing a default file on first run. Errors fall
    /// back to defaults (logged); use [`Config::parse`] to surface them.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s).unwrap_or_else(|e| {
                log::error!("{}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => {
                let cfg = Self::default();
                if std::fs::create_dir_all(config_dir()).is_ok() {
                    let _ = std::fs::create_dir_all(plugins_dir());
                    if let Err(e) = cfg.save_to(&path) {
                        log::warn!("cannot write {}: {e}", path.display());
                    }
                }
                cfg
            }
        }
    }

    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        self.save_to(&config_path())
    }

    /// Write to `path`, keeping whatever the file already holds (comments,
    /// key order, unknown keys) and only touching values that differ.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let existing = std::fs::read_to_string(path).unwrap_or_else(|_| "# Qell configuration\n\n".into());
        let out = self.merge_into(&existing).map_err(std::io::Error::other)?;
        if out != existing {
            std::fs::write(path, out)?;
        }
        Ok(())
    }

    /// Merge into the text of an existing file. Fails when that text isn't
    /// valid TOML: the user is probably mid-edit and their work must not be
    /// overwritten.
    fn merge_into(&self, existing: &str) -> Result<String, String> {
        // Compact form: arrays stay on one line, like people write them.
        let fresh: toml_edit::DocumentMut =
            toml::to_string(self).expect("config serialises").parse().expect("serialised config parses");
        let mut doc = existing
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("it has a syntax error, fix it first ({})", e.to_string().lines().next().unwrap_or("")))?;
        for (key, item) in fresh.iter() {
            match (doc.get_mut(key), item) {
                (Some(toml_edit::Item::Value(old)), toml_edit::Item::Value(new)) => {
                    if old.to_string().trim() != new.to_string().trim() {
                        let decor = old.decor().clone();
                        *old = new.clone();
                        *old.decor_mut() = decor;
                    }
                }
                (Some(toml_edit::Item::Table(old)), toml_edit::Item::Table(new)) => {
                    // Keep the table (and its comments); sync its entries.
                    let stale: Vec<String> = old.iter().map(|(k, _)| k.to_string()).filter(|k| !new.contains_key(k)).collect();
                    for k in stale {
                        old.remove(&k);
                    }
                    for (k, v) in new.iter() {
                        match (old.get_mut(k), v) {
                            (Some(toml_edit::Item::Value(o)), toml_edit::Item::Value(n)) => {
                                if o.to_string().trim() != n.to_string().trim() {
                                    // Keep the comment and spacing around the value.
                                    let decor = o.decor().clone();
                                    *o = n.clone();
                                    *o.decor_mut() = decor;
                                }
                            }
                            _ => {
                                old.insert(k, v.clone());
                            }
                        }
                    }
                }
                _ => {
                    doc.insert(key, item.clone());
                }
            }
        }
        Ok(doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_uses_defaults() {
        let c = Config::parse("font_size = 18.0\n").unwrap();
        assert_eq!(c.font_size, 18.0);
        assert_eq!(c.tab_size, Config::default().tab_size);
    }

    #[test]
    fn save_keeps_comments_and_unknown_keys() {
        let existing = "# my settings\nfont_size = 14.0 # big screens: 16\ntab_size = 4\nfuture_option = \"x\"\n\n[lsp]\n# my go server\ngo = [\"gopls\"]\n";
        let mut c = Config::parse(existing).unwrap();
        c.font_size = 16.0;
        c.lsp.insert("rust".into(), vec!["rust-analyzer".into()]);
        let out = c.merge_into(existing).unwrap();
        assert!(out.contains("# my settings"));
        assert!(out.contains("font_size = 16.0 # big screens: 16"), "{out}");
        assert!(out.contains("future_option = \"x\""));
        assert!(out.contains("# my go server"));
        assert!(out.contains("rust = [\"rust-analyzer\"]"));
        // Changing an entry inside [lsp] keeps the comment above it.
        let mut c2 = c.clone();
        c2.lsp.insert("go".into(), vec!["gopls".into(), "-remote=auto".into()]);
        let out2 = c2.merge_into(&out).unwrap();
        assert!(out2.contains("# my go server\ngo = [\"gopls\", \"-remote=auto\"]"), "{out2}");
        assert_eq!(Config::parse(&out).unwrap(), c);
    }

    #[test]
    fn save_round_trips_from_nothing() {
        let c = Config { shell: "zsh -l".into(), ..Default::default() };
        let out = c.merge_into("").unwrap();
        assert_eq!(Config::parse(&out).unwrap(), c);
    }

    #[test]
    fn unchanged_config_is_byte_identical() {
        let c = Config::default();
        let first = c.merge_into("# hi\n").unwrap();
        assert_eq!(c.merge_into(&first).unwrap(), first);
    }

    #[test]
    fn broken_file_is_never_overwritten() {
        assert!(Config::default().merge_into("font_size = \n[[[").is_err());
    }

    #[test]
    fn removed_lsp_override_is_removed() {
        let existing = "[lsp]\ngo = [\"gopls\"]\n";
        let c = Config::default();
        assert!(!c.merge_into(existing).unwrap().contains("gopls"));
    }
}
