//! Editor commands and their default key bindings.
//!
//! Every user-visible action is a [`Command`], so the command palette, the
//! menus and the keymap all pull from one list. Plugin commands are
//! separate (see `qell_plugins`) and show up in the palette by name.

use egui::{Key, KeyboardShortcut, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)]
pub enum Command {
    NewFile,
    OpenFile,
    OpenFolder,
    Save,
    SaveAs,
    CloseTab,
    NextTab,
    PrevTab,
    Undo,
    Redo,
    SelectAll,
    TriggerCompletion,
    ToggleSidebar,
    ShowExplorer,
    ShowSourceControl,
    ToggleTerminal,
    NewTerminal,
    KillTerminal,
    NextTerminal,
    PrevTerminal,
    FocusTerminal,
    FocusEditor,
    CommandPalette,
    QuickOpen,
    GoToLine,
    ZoomIn,
    ZoomOut,
    OpenSettings,
    OpenSettingsFile,
    Quit,
}

impl Command {
    pub const ALL: &'static [Command] = &[
        Command::NewFile,
        Command::OpenFile,
        Command::OpenFolder,
        Command::Save,
        Command::SaveAs,
        Command::CloseTab,
        Command::NextTab,
        Command::PrevTab,
        Command::Undo,
        Command::Redo,
        Command::SelectAll,
        Command::TriggerCompletion,
        Command::ToggleSidebar,
        Command::ShowExplorer,
        Command::ShowSourceControl,
        Command::ToggleTerminal,
        Command::NewTerminal,
        Command::KillTerminal,
        Command::NextTerminal,
        Command::PrevTerminal,
        Command::FocusTerminal,
        Command::FocusEditor,
        Command::CommandPalette,
        Command::QuickOpen,
        Command::GoToLine,
        Command::ZoomIn,
        Command::ZoomOut,
        Command::OpenSettings,
        Command::OpenSettingsFile,
        Command::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::NewFile => "File: New File",
            Command::OpenFile => "File: Open File…",
            Command::OpenFolder => "File: Open Folder…",
            Command::Save => "File: Save",
            Command::SaveAs => "File: Save As…",
            Command::CloseTab => "File: Close Tab",
            Command::NextTab => "View: Next Tab",
            Command::PrevTab => "View: Previous Tab",
            Command::Undo => "Edit: Undo",
            Command::Redo => "Edit: Redo",
            Command::SelectAll => "Edit: Select All",
            Command::TriggerCompletion => "Edit: Trigger Completion",
            Command::ToggleSidebar => "View: Toggle Sidebar",
            Command::ShowExplorer => "View: Show Explorer",
            Command::ShowSourceControl => "View: Show Source Control",
            Command::ToggleTerminal => "View: Toggle Terminal",
            Command::NewTerminal => "Terminal: New Terminal",
            Command::KillTerminal => "Terminal: Kill Active Terminal",
            Command::NextTerminal => "Terminal: Focus Next Terminal",
            Command::PrevTerminal => "Terminal: Focus Previous Terminal",
            Command::FocusTerminal => "View: Focus Terminal",
            Command::FocusEditor => "View: Focus Editor",
            Command::CommandPalette => "Command Palette",
            Command::QuickOpen => "Go to File…",
            Command::GoToLine => "Go to Line…",
            Command::ZoomIn => "View: Zoom In",
            Command::ZoomOut => "View: Zoom Out",
            Command::OpenSettings => "Preferences: Open Settings",
            Command::OpenSettingsFile => "Preferences: Open Settings File (TOML)",
            Command::Quit => "Quit",
        }
    }

    /// Default keybinding. `Modifiers::COMMAND` is Ctrl on Linux/Windows
    /// and ⌘ on macOS.
    pub fn shortcut(self) -> Option<KeyboardShortcut> {
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let s = |m, k| Some(KeyboardShortcut::new(m, k));
        match self {
            Command::NewFile => s(cmd, Key::N),
            Command::OpenFile => s(cmd, Key::O),
            Command::OpenFolder => s(cmd_shift, Key::O),
            Command::Save => s(cmd, Key::S),
            Command::SaveAs => s(cmd_shift, Key::S),
            Command::CloseTab => s(cmd, Key::W),
            Command::NextTab => s(Modifiers::CTRL, Key::Tab),
            Command::PrevTab => s(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab),
            Command::Undo => s(cmd, Key::Z),
            Command::Redo => s(cmd, Key::Y),
            Command::SelectAll => s(cmd, Key::A),
            Command::TriggerCompletion => s(Modifiers::CTRL, Key::Space),
            Command::ToggleSidebar => s(cmd, Key::B),
            Command::ShowExplorer => s(Modifiers::CTRL | Modifiers::SHIFT, Key::E),
            Command::ShowSourceControl => s(Modifiers::CTRL | Modifiers::SHIFT, Key::G),
            Command::ToggleTerminal => s(Modifiers::CTRL, Key::Backtick),
            Command::NewTerminal => s(Modifiers::CTRL | Modifiers::SHIFT, Key::Backtick),
            Command::KillTerminal => None,
            Command::NextTerminal => s(Modifiers::CTRL, Key::PageDown),
            Command::PrevTerminal => s(Modifiers::CTRL, Key::PageUp),
            Command::FocusTerminal => None,
            Command::FocusEditor => None,
            Command::CommandPalette => s(cmd_shift, Key::P),
            Command::QuickOpen => s(cmd, Key::P),
            Command::GoToLine => s(cmd, Key::G),
            Command::ZoomIn => s(cmd, Key::Equals),
            Command::ZoomOut => s(cmd, Key::Minus),
            Command::OpenSettings => s(cmd, Key::Comma),
            Command::OpenSettingsFile => None,
            Command::Quit => s(cmd, Key::Q),
        }
    }

    /// Commands with a shortcut, most specific first. egui matches
    /// shortcuts "logically" (an extra Shift still matches), so Ctrl+Tab
    /// would otherwise swallow Ctrl+Shift+Tab.
    pub fn by_shortcut_specificity() -> Vec<(Command, KeyboardShortcut)> {
        let mut v: Vec<_> = Self::ALL.iter().filter_map(|c| c.shortcut().map(|s| (*c, s))).collect();
        let weight = |m: Modifiers| m.shift as u8 + m.alt as u8 + (m.ctrl || m.command || m.mac_cmd) as u8;
        v.sort_by_key(|(_, s)| std::cmp::Reverse(weight(s.modifiers)));
        v
    }

    pub fn shortcut_text(self, ctx: &egui::Context) -> String {
        self.shortcut().map(|s| ctx.format_shortcut(&s)).unwrap_or_default()
    }
}

/// Case-insensitive subsequence match, returning a score (lower is better)
/// so palettes can sort. `None` when `needle` is not a subsequence.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut score = 0;
    let mut pos = 0;
    let mut last = None;
    for n in needle.chars().map(|c| c.to_ascii_lowercase()) {
        let found = hay[pos..].iter().position(|&h| h == n)? + pos;
        if let Some(l) = last {
            score += found - l - 1; // gaps between matched chars
        } else {
            score += found; // leading offset
        }
        last = Some(found);
        pos = found + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert!(fuzzy_score("sv", "File: Save").is_some());
        assert!(fuzzy_score("xyz", "File: Save").is_none());
        assert!(fuzzy_score("save", "File: Save").unwrap() < fuzzy_score("save", "View: Sidebar visible").unwrap());
    }

    #[test]
    fn shifted_shortcuts_are_checked_first() {
        let order = Command::by_shortcut_specificity();
        let pos = |c| order.iter().position(|(x, _)| *x == c).unwrap();
        assert!(pos(Command::PrevTab) < pos(Command::NextTab));
        assert!(pos(Command::NewTerminal) < pos(Command::ToggleTerminal));
        assert!(pos(Command::SaveAs) < pos(Command::Save));
    }

    #[test]
    fn every_command_has_a_label() {
        for c in Command::ALL {
            assert!(!c.label().is_empty());
        }
    }
}
