//! Qell — a minimalist code editor.
//!
//! `qell [PATH]` opens a file, or a folder as the workspace. With no
//! argument the current directory is the workspace.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activity;
mod app;
mod commands;
mod config;
mod document;
mod format;
mod gitops;
mod hints;
mod history;
mod icons;
mod journal;
mod memory;
mod merge;
mod native_menu;
mod preview;
mod recent;
mod recovery;
mod session;
mod snippet;
mod tasks;
mod testrun;
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

/// The window and Dock icon (a bundled app uses its .icns instead).
fn app_icon() -> egui::IconData {
    let png = include_bytes!("../assets/icon/qell-256.png");
    match image::load_from_memory(png) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData { rgba: rgba.into_raw(), width, height }
        }
        Err(_) => egui::IconData::default(),
    }
}

/// Started from the Finder or the Dock, a Mac app gets a bare `PATH`
/// (`/usr/bin:/bin:…`): language servers, formatters, `go`, `terraform`
/// and Homebrew's tools wouldn't be found. Take the `PATH` your login
/// shell sets, once, as a terminal would have given it. Started from a
/// terminal (`TERM` is set), the `PATH` is already yours.
fn shell_path() {
    if !cfg!(target_os = "macos") || std::env::var_os("TERM").is_some() {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let child = std::process::Command::new(&shell)
        .args(["-l", "-i", "-c", "printf '__QELL_PATH__%s' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return };
    // A shell that hangs (a prompt asking something) isn't waited for.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < std::time::Duration::from_secs(3) => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                log::warn!("{shell} took too long to tell its PATH; keeping the app's");
                return;
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout.read_to_string(&mut out);
    }
    if let Some((_, path)) = out.rsplit_once("__QELL_PATH__") {
        if !path.trim().is_empty() {
            // Before any thread starts: nothing else reads the environment yet.
            std::env::set_var("PATH", path.trim());
        }
    }
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("qell=info,warn")).init();
    recovery::install_panic_hook();

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

    // Before anything starts a process: a Dock-launched app's PATH.
    shell_path();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Qell")
            .with_app_id("qell")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("Qell", options, Box::new(move |cc| Ok(Box::new(app::QellApp::new(cc, root, file)))))
}
