//! Qell — a minimalist code editor.
//!
//! `qell [PATH]` opens a file, or a folder as the workspace. With no
//! argument the current directory is the workspace.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod config;
mod document;
mod format;
mod icons;
mod native_menu;
mod recent;
mod session;
mod snippet;
mod theme;
mod views;
mod vim;

use std::path::PathBuf;

/// Walk up from `dir` to the nearest directory that looks like a project
/// root, so language servers see the whole workspace. Falls back to `dir`.
fn project_root(dir: &std::path::Path) -> PathBuf {
    const MARKERS: &[&str] = &[".git", "Cargo.toml", "go.mod", "package.json", "compile_commands.json", ".terraform", "Makefile"];
    let start = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut cur = Some(start.as_path());
    while let Some(d) = cur {
        if MARKERS.iter().any(|m| d.join(m).exists()) {
            return d.to_path_buf();
        }
        cur = d.parent();
    }
    start
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("qell=info,warn")).init();

    let arg = std::env::args().nth(1).map(PathBuf::from);
    let (root, file) = match arg {
        Some(p) if p.is_dir() => (p, None),
        Some(p) => {
            let parent = p.parent().filter(|d| !d.as_os_str().is_empty()).map(PathBuf::from).unwrap_or_else(|| ".".into());
            (project_root(&parent), Some(p))
        }
        None => (".".into(), None),
    };
    let root = std::fs::canonicalize(&root).unwrap_or(root);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Qell")
            .with_app_id("qell")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native("Qell", options, Box::new(move |cc| Ok(Box::new(app::QellApp::new(cc, root, file)))))
}
