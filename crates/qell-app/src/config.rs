//! User configuration, read from `~/.config/qell/config.toml`.
//!
//! Every field has a default, so an absent or partial file is fine. The
//! file is written out with the defaults the first time Qell runs so users
//! have something to edit.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Editor and terminal font size in points.
    pub font_size: f32,
    /// Spaces inserted by the Tab key.
    pub tab_size: usize,
    /// Show the file tree on startup.
    pub show_sidebar: bool,
    /// Show the terminal panel on startup.
    pub show_terminal: bool,
    /// Ask language servers for completions while typing (not only on Ctrl+Space).
    pub auto_complete: bool,
    /// Extra language-server commands, overriding the built-ins, keyed by
    /// language id. Example: `rust = ["rust-analyzer"]`.
    pub lsp: std::collections::BTreeMap<String, Vec<String>>,
    /// Directories and names the quick-open file list skips.
    pub ignore: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            tab_size: 4,
            show_sidebar: true,
            show_terminal: true,
            auto_complete: true,
            lsp: Default::default(),
            ignore: vec![".git".into(), "target".into(), "node_modules".into(), ".terraform".into(), "dist".into()],
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("qell")
}

pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str(&s) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("{}: {e}; using defaults", path.display());
                    Self::default()
                }
            },
            Err(_) => {
                let cfg = Self::default();
                if std::fs::create_dir_all(config_dir()).is_ok() {
                    let _ = std::fs::create_dir_all(plugins_dir());
                    if let Ok(s) = toml::to_string_pretty(&cfg) {
                        let _ = std::fs::write(&path, format!("# Qell configuration\n\n{s}"));
                    }
                }
                cfg
            }
        }
    }
}
