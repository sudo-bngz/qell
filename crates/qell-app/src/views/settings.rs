//! The Settings window (Ctrl+, / ⌘,).
//!
//! A searchable list of every option in `config.toml`, grouped by section,
//! in the spirit of VS Code's settings editor. Checkboxes, sliders and
//! pickers apply as you change them; text fields apply on Enter or when
//! they lose focus. The app notices the config changed, applies it to the
//! running editor and saves `config.toml` (see `QellApp::apply_config`).

use std::collections::HashMap;

use egui::{Color32, Context, Id, RichText, Stroke};

use crate::app::QellApp;
use crate::commands::Command;
use crate::config::{self, Config};

#[derive(Default)]
pub struct SettingsState {
    pub open: bool,
    pub query: String,
    focus_search: bool,
    /// Text being typed in a list / command field, keyed by setting.
    /// Committed on Enter or focus loss so half-typed values never apply.
    drafts: HashMap<String, String>,
}

impl SettingsState {
    pub fn toggle(&mut self, _config: &Config) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.drafts.clear();
            self.focus_search = true;
        }
    }
}

/// One row of the settings list.
struct Def {
    key: String,
    section: &'static str,
    title: String,
    desc: String,
}

fn def(key: &str, section: &'static str, title: &str, desc: &str) -> Def {
    Def { key: key.into(), section, title: title.into(), desc: desc.into() }
}

const SECTIONS: &[&str] = &["Editor", "Appearance", "Layout", "Terminal", "Files", "Language Servers"];

fn defs() -> Vec<Def> {
    let mut v = vec![
        def("font_size", "Editor", "Font Size", "Editor font size in points."),
        def("tab_size", "Editor", "Tab Size", "Spaces inserted by the Tab key."),
        def("auto_complete", "Editor", "Auto Complete", "Suggest completions while typing, not only on Ctrl+Space."),
        def(
            "auto_close",
            "Editor",
            "Close Brackets and Quotes",
            "Add the closing half as you type, wrap a selection when you type a bracket or quote around it, and take both away on backspace.",
        ),
        def(
            "format_on_save",
            "Editor",
            "Format on Save",
            "Run the language's formatter (gofmt, terraform fmt, rustfmt…) when you save, if it is installed.",
        ),
        def(
            "vim_mode",
            "Editor",
            "Vim Mode",
            "Vim keys in the editor: Normal, Insert and Visual modes, and a command line for : commands and / search.",
        ),
        def(
            "word_wrap",
            "Editor",
            "Word Wrap",
            "Wrap long lines at the window's width instead of scrolling sideways. Toggle with Alt+Z.",
        ),
        def(
            "bracket_pair_colors",
            "Editor",
            "Bracket Pair Colours",
            "Colour matching brackets by nesting depth; unmatched brackets show in red.",
        ),
        def(
            "indent_guides",
            "Editor",
            "Indent Guides",
            "Vertical lines at each indentation level; the block around the cursor is highlighted.",
        ),
        def("outline", "Editor", "Outline", "An Outline section under the file tree lists what the open file defines; click to jump."),
        def("sticky_scroll", "Editor", "Sticky Scroll", "The lines that open the blocks you are scrolled into (a function, an if) stay at the top of the editor."),
        def("inlay_hints", "Editor", "Inlay Hints", "Types and parameter names from the language server, shown dimmed inside the code (they are not part of the file)."),
        def(
            "minimap",
            "Appearance",
            "Minimap",
            "The whole file in miniature on the right of the editor; click or drag it to scroll.",
        ),
        def("ui_scale", "Appearance", "UI Scale", "Size of the whole interface."),
        def("file_icons", "Appearance", "File Icons", "Show file-type icons in the file tree and tabs."),
        def(
            "inline_problems",
            "Editor",
            "Problems at Line End",
            "Write an error's or warning's message after the end of its line.",
        ),
        def(
            "tree_branches",
            "Appearance",
            "Branches in File Tree",
            "Show each repository's current branch beside its folder in the file tree.",
        ),
        def(
            "git_status",
            "Appearance",
            "Git Status Colours",
            "Colour files by git status (modified, untracked…) and show the branch.",
        ),
        def(
            "git_scan_depth",
            "Appearance",
            "Repository Scan Depth",
            "Folder levels searched for git repositories when the workspace holds several. 0 only uses the workspace's own.",
        ),
        def("restore_session", "Layout", "Restore Session", "Reopen the files that were open when Qell last closed, per folder."),
        def(
            "shortcuts",
            "Layout",
            "File Tree Shortcuts",
            "How many rows of shortcuts to show above the file tree, half of them folders. 0 hides the section.",
        ),
        def("show_sidebar", "Layout", "Show File Tree", "Show the file tree (also on startup)."),
        def("show_terminal", "Layout", "Show Terminal", "Show the terminal panel (also on startup)."),
        def(
            "shell",
            "Terminal",
            "Shell",
            "Command for new terminals, e.g. zsh -l. Empty uses $SHELL. Restart a terminal to apply.",
        ),
        def("terminal_font_size", "Terminal", "Font Size", "Terminal font size in points. 0 follows the editor."),
        def(
            "kubectl",
            "Kubernetes",
            "kubectl Command",
            "What the cluster view runs to read your cluster, e.g. kubectl or /opt/homebrew/bin/kubectl.",
        ),
        def(
            "aws",
            "AWS",
            "aws Command",
            "What the AWS view runs to read your account, e.g. aws or /opt/homebrew/bin/aws.",
        ),
        def("tree_hidden", "Files", "Hidden in File Tree", "Comma-separated names the file tree doesn't show."),
        def(
            "ignore",
            "Files",
            "Quick Open Ignore",
            "Comma-separated directory and file names Go to File and the search index skip.",
        ),
    ];
    for lang in qell_core::syntax::builtin_languages() {
        let Some(cmd) = lang.lsp else { continue };
        v.push(Def {
            key: format!("lsp.{}", lang.id),
            section: "Language Servers",
            title: lang.name.to_string(),
            desc: format!("Server command for {}. Empty uses the default: {}", lang.name, cmd.join(" ")),
        });
    }
    v
}

fn matches(d: &Def, query: &str) -> bool {
    let hay = format!("{} {} {} {}", d.section, d.title, d.desc, d.key).to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Whether a setting differs from its default.
fn is_modified(key: &str, c: &Config) -> bool {
    let d = Config::default();
    match key {
        "font_size" => c.font_size != d.font_size,
        "tab_size" => c.tab_size != d.tab_size,
        "auto_complete" => c.auto_complete != d.auto_complete,
        "auto_close" => c.auto_close != d.auto_close,
        "format_on_save" => c.format_on_save != d.format_on_save,
        "vim_mode" => c.vim_mode != d.vim_mode,
        "word_wrap" => c.word_wrap != d.word_wrap,
        "minimap" => c.minimap != d.minimap,
        "bracket_pair_colors" => c.bracket_pair_colors != d.bracket_pair_colors,
        "indent_guides" => c.indent_guides != d.indent_guides,
        "outline" => c.outline != d.outline,
        "sticky_scroll" => c.sticky_scroll != d.sticky_scroll,
        "inlay_hints" => c.inlay_hints != d.inlay_hints,
        "ui_scale" => c.ui_scale != d.ui_scale,
        "file_icons" => c.file_icons != d.file_icons,
        "inline_problems" => c.inline_problems != d.inline_problems,
        "tree_branches" => c.tree_branches != d.tree_branches,
        "git_status" => c.git_status != d.git_status,
        "git_scan_depth" => c.git_scan_depth != d.git_scan_depth,
        "restore_session" => c.restore_session != d.restore_session,
        "shortcuts" => c.shortcuts != d.shortcuts,
        "show_sidebar" => c.show_sidebar != d.show_sidebar,
        "show_terminal" => c.show_terminal != d.show_terminal,
        "shell" => c.shell != d.shell,
        "kubectl" => c.kubectl != d.kubectl,
        "aws" => c.aws != d.aws,
        "terminal_font_size" => c.terminal_font_size != d.terminal_font_size,
        "tree_hidden" => c.tree_hidden != d.tree_hidden,
        "ignore" => c.ignore != d.ignore,
        k => k.strip_prefix("lsp.").is_some_and(|lang| c.lsp.contains_key(lang)),
    }
}

fn reset(key: &str, c: &mut Config) {
    let d = Config::default();
    match key {
        "font_size" => c.font_size = d.font_size,
        "tab_size" => c.tab_size = d.tab_size,
        "auto_complete" => c.auto_complete = d.auto_complete,
        "auto_close" => c.auto_close = d.auto_close,
        "format_on_save" => c.format_on_save = d.format_on_save,
        "vim_mode" => c.vim_mode = d.vim_mode,
        "word_wrap" => c.word_wrap = d.word_wrap,
        "minimap" => c.minimap = d.minimap,
        "bracket_pair_colors" => c.bracket_pair_colors = d.bracket_pair_colors,
        "indent_guides" => c.indent_guides = d.indent_guides,
        "outline" => c.outline = d.outline,
        "sticky_scroll" => c.sticky_scroll = d.sticky_scroll,
        "inlay_hints" => c.inlay_hints = d.inlay_hints,
        "ui_scale" => c.ui_scale = d.ui_scale,
        "file_icons" => c.file_icons = d.file_icons,
        "inline_problems" => c.inline_problems = d.inline_problems,
        "tree_branches" => c.tree_branches = d.tree_branches,
        "git_status" => c.git_status = d.git_status,
        "git_scan_depth" => c.git_scan_depth = d.git_scan_depth,
        "restore_session" => c.restore_session = d.restore_session,
        "shortcuts" => c.shortcuts = d.shortcuts,
        "show_sidebar" => c.show_sidebar = d.show_sidebar,
        "show_terminal" => c.show_terminal = d.show_terminal,
        "shell" => c.shell = d.shell,
        "kubectl" => c.kubectl = d.kubectl,
        "aws" => c.aws = d.aws,
        "terminal_font_size" => c.terminal_font_size = d.terminal_font_size,
        "tree_hidden" => c.tree_hidden = d.tree_hidden,
        "ignore" => c.ignore = d.ignore,
        k => {
            if let Some(lang) = k.strip_prefix("lsp.") {
                c.lsp.remove(lang);
            }
        }
    }
}

/// A text field that commits on Enter / focus loss. Returns the new text
/// when committed and different from `current`.
fn text_field(ui: &mut egui::Ui, drafts: &mut HashMap<String, String>, key: &str, current: &str, hint: &str) -> Option<String> {
    let id = Id::new(("qell-setting", key));
    let focused = ui.memory(|m| m.has_focus(id));
    let mut text = if focused {
        drafts.entry(key.to_string()).or_insert_with(|| current.to_string()).clone()
    } else {
        drafts.remove(key);
        current.to_string()
    };
    let resp = egui::TextEdit::singleline(&mut text)
        .id(id)
        .hint_text(hint)
        .desired_width(f32::INFINITY)
        .margin(egui::vec2(6.0, 4.0))
        .show(ui)
        .response;
    // The theme draws unfocused fields without a border; settings need to
    // look editable, and a value must not look like the default hint.
    if !resp.has_focus() {
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter().rect_stroke(resp.rect, 0.0, stroke, egui::StrokeKind::Inside);
    }
    if resp.has_focus() {
        drafts.insert(key.to_string(), text.clone());
    }
    (resp.lost_focus() && text.trim() != current).then(|| text.trim().to_string())
}

/// Draw the control for one setting, editing `c` in place.
fn control(ui: &mut egui::Ui, key: &str, c: &mut Config, drafts: &mut HashMap<String, String>) {
    match key {
        "font_size" => {
            ui.add(egui::Slider::new(&mut c.font_size, 8.0..=32.0).step_by(1.0).suffix(" pt"));
        }
        "tab_size" => {
            ui.add(egui::Slider::new(&mut c.tab_size, 1..=8));
        }
        "terminal_font_size" => {
            ui.add(egui::Slider::new(&mut c.terminal_font_size, 0.0..=32.0).step_by(1.0).custom_formatter(|v, _| {
                if v == 0.0 {
                    "editor".into()
                } else {
                    format!("{v} pt")
                }
            }));
        }
        "ui_scale" => {
            // A picker rather than a slider: rescaling while dragging moves
            // the slider out from under the pointer.
            let label = |v: f32| format!("{:.0}%", v * 100.0);
            egui::ComboBox::from_id_salt("ui_scale").selected_text(label(c.ui_scale)).show_ui(ui, |ui| {
                for v in [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0] {
                    ui.selectable_value(&mut c.ui_scale, v, label(v));
                }
            });
        }
        "auto_complete" => {
            ui.checkbox(&mut c.auto_complete, "Enabled");
        }
        "auto_close" => {
            ui.checkbox(&mut c.auto_close, "Enabled");
        }
        "format_on_save" => {
            ui.checkbox(&mut c.format_on_save, "Enabled");
        }
        "vim_mode" => {
            ui.checkbox(&mut c.vim_mode, "Enabled");
        }
        "word_wrap" => {
            ui.checkbox(&mut c.word_wrap, "Enabled");
        }
        "minimap" => {
            ui.checkbox(&mut c.minimap, "Visible");
        }
        "bracket_pair_colors" => {
            ui.checkbox(&mut c.bracket_pair_colors, "Enabled");
        }
        "indent_guides" => {
            ui.checkbox(&mut c.indent_guides, "Enabled");
        }
        "outline" => {
            ui.checkbox(&mut c.outline, "Visible");
        }
        "sticky_scroll" => {
            ui.checkbox(&mut c.sticky_scroll, "Enabled");
        }
        "inlay_hints" => {
            ui.checkbox(&mut c.inlay_hints, "Enabled");
        }
        "file_icons" => {
            ui.checkbox(&mut c.file_icons, "Enabled");
        }
        "inline_problems" => {
            ui.checkbox(&mut c.inline_problems, "Enabled");
        }
        "tree_branches" => {
            ui.checkbox(&mut c.tree_branches, "Enabled");
        }
        "git_status" => {
            ui.checkbox(&mut c.git_status, "Enabled");
        }
        "git_scan_depth" => {
            ui.add(egui::Slider::new(&mut c.git_scan_depth, 0..=6));
        }
        "restore_session" => {
            ui.checkbox(&mut c.restore_session, "Enabled");
        }
        "shortcuts" => {
            ui.add(egui::Slider::new(&mut c.shortcuts, 0..=15));
        }
        "show_sidebar" => {
            ui.checkbox(&mut c.show_sidebar, "Visible");
        }
        "show_terminal" => {
            ui.checkbox(&mut c.show_terminal, "Visible");
        }
        "shell" => {
            let shell_env = std::env::var("SHELL").unwrap_or_else(|_| "sh".into());
            if let Some(v) = text_field(ui, drafts, key, &c.shell, &shell_env) {
                c.shell = v;
            }
        }
        "aws" => {
            if let Some(v) = text_field(ui, drafts, key, &c.aws, "aws") {
                c.aws = v;
            }
        }
        "kubectl" => {
            if let Some(v) = text_field(ui, drafts, key, &c.kubectl, "kubectl") {
                c.kubectl = v;
            }
        }
        "tree_hidden" => {
            if let Some(v) = text_field(ui, drafts, key, &c.tree_hidden.join(", "), "e.g. .git, .DS_Store") {
                c.tree_hidden = split_list(&v);
            }
        }
        "ignore" => {
            if let Some(v) = text_field(ui, drafts, key, &c.ignore.join(", "), "e.g. target, node_modules") {
                c.ignore = split_list(&v);
            }
        }
        k => {
            let Some(lang) = k.strip_prefix("lsp.") else { return };
            let current = c.lsp.get(lang).map(|v| v.join(" ")).unwrap_or_default();
            let default = qell_core::syntax::builtin_languages()
                .into_iter()
                .find(|l| l.id == lang)
                .and_then(|l| l.lsp)
                .map(|cmd| cmd.join(" "))
                .unwrap_or_default();
            if let Some(v) = text_field(ui, drafts, k, &current, &default) {
                let words: Vec<String> = v.split_whitespace().map(String::from).collect();
                if words.is_empty() {
                    c.lsp.remove(lang);
                } else {
                    c.lsp.insert(lang.to_string(), words);
                }
            }
        }
    }
}

pub fn show(app: &mut QellApp, ctx: &Context) {
    if !app.settings.open {
        return;
    }
    app.command_palette.open = false;
    let accent = app.palette.accent;
    let text_dim = app.palette.text_dim;
    let border = app.palette.border;
    let screen = ctx.content_rect();
    let width = 640.0_f32.min(screen.width() - 40.0);
    let height = 560.0_f32.min(screen.height() - 80.0);

    let mut open = true;
    let mut open_file = false;
    egui::Window::new("Settings")
        .id(Id::new("qell-settings"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fixed_size([width, height])
        .anchor(egui::Align2::CENTER_TOP, [0.0, 50.0])
        .frame(egui::Frame::window(&ctx.style()).fill(app.palette.editor_bg).stroke(Stroke::new(1.0_f32, border)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let search = egui::TextEdit::singleline(&mut app.settings.query)
                    .hint_text("Search settings")
                    .desired_width(width - 150.0)
                    .show(ui)
                    .response;
                if app.settings.focus_search {
                    search.request_focus();
                    app.settings.focus_search = false;
                }
                open_file = ui
                    .link(RichText::new("Edit config.toml").size(12.0))
                    .on_hover_text(config::config_path().display().to_string())
                    .clicked();
            });
            ui.add_space(6.0);
            ui.separator();

            let query = app.settings.query.clone();
            let defs = defs();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(width - 16.0);
                let mut shown = 0;
                for section in SECTIONS {
                    let rows: Vec<&Def> = defs.iter().filter(|d| d.section == *section && matches(d, &query)).collect();
                    if rows.is_empty() {
                        continue;
                    }
                    ui.add_space(10.0);
                    ui.label(RichText::new(*section).size(16.0).strong());
                    ui.add_space(4.0);
                    for d in rows {
                        shown += 1;
                        show_row(ui, d, &mut app.config, &mut app.settings.drafts, accent, text_dim);
                    }
                }
                if shown == 0 {
                    ui.add_space(20.0);
                    ui.label(RichText::new("No settings found.").color(text_dim));
                }
                ui.add_space(12.0);
            });
        });
    if !open {
        app.settings.open = false;
    }
    if open_file {
        app.pending_commands.push(Command::OpenSettingsFile);
    }
}

fn show_row(
    ui: &mut egui::Ui,
    d: &Def,
    config: &mut Config,
    drafts: &mut HashMap<String, String>,
    accent: Color32,
    text_dim: Color32,
) {
    let modified = is_modified(&d.key, config);
    let resp = egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 4, top: 6, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&d.title).size(13.0).strong());
            if modified
                && ui
                    .add(egui::Button::new(RichText::new("↺").size(12.0).color(text_dim)).frame(false))
                    .on_hover_text("Reset to default")
                    .clicked()
            {
                reset(&d.key, config);
            }
        });
        ui.label(RichText::new(&d.desc).size(12.0).color(text_dim));
        ui.add_space(2.0);
        control(ui, &d.key, config, drafts);
    });
    // A bar on the left marks settings changed from the default, like VS Code.
    if modified {
        let r = resp.response.rect;
        ui.painter().vline(r.left() + 2.0, r.y_range(), Stroke::new(2.0_f32, accent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_control_and_reset() {
        // Changing then resetting each key must return to the default, and
        // the key must be recognised by is_modified (no silent typos).
        for d in defs() {
            let mut c = Config::default();
            assert!(!is_modified(&d.key, &c), "{} modified by default", d.key);
            match d.key.as_str() {
                "font_size" => c.font_size = 20.0,
                "tab_size" => c.tab_size = 2,
                "auto_complete" => c.auto_complete = false,
                "auto_close" => c.auto_close = false,
                "format_on_save" => c.format_on_save = false,
                "vim_mode" => c.vim_mode = true,
                "word_wrap" => c.word_wrap = true,
                "minimap" => c.minimap = false,
                "bracket_pair_colors" => c.bracket_pair_colors = false,
                "indent_guides" => c.indent_guides = false,
                "outline" => c.outline = false,
                "sticky_scroll" => c.sticky_scroll = false,
                "inlay_hints" => c.inlay_hints = false,
                "ui_scale" => c.ui_scale = 1.5,
                "file_icons" => c.file_icons = false,
                "inline_problems" => c.inline_problems = false,
                "tree_branches" => c.tree_branches = false,
                "git_status" => c.git_status = false,
                "git_scan_depth" => c.git_scan_depth = 1,
                "restore_session" => c.restore_session = false,
                "shortcuts" => c.shortcuts = 10,
                "show_sidebar" => c.show_sidebar = false,
                "show_terminal" => c.show_terminal = false,
                "shell" => c.shell = "zsh".into(),
                "kubectl" => c.kubectl = "/opt/homebrew/bin/kubectl".into(),
                "aws" => c.aws = "/opt/homebrew/bin/aws".into(),
                "terminal_font_size" => c.terminal_font_size = 12.0,
                "tree_hidden" => c.tree_hidden.clear(),
                "ignore" => c.ignore.clear(),
                k => {
                    let lang = k.strip_prefix("lsp.").expect("unknown setting key");
                    c.lsp.insert(lang.into(), vec!["x".into()]);
                }
            }
            assert!(is_modified(&d.key, &c), "{} change not detected", d.key);
            reset(&d.key, &mut c);
            assert_eq!(c, Config::default(), "{} reset", d.key);
        }
    }

    #[test]
    fn search_matches_all_words_anywhere() {
        let d = def("shell", "Terminal", "Shell", "Command for new terminals");
        assert!(matches(&d, "terminal shell"));
        assert!(matches(&d, "COMMAND"));
        assert!(!matches(&d, "font"));
    }

    #[test]
    fn lists_are_comma_separated() {
        assert_eq!(split_list(" .git, target,, node_modules "), vec![".git", "target", "node_modules"]);
    }
}
