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
    ToggleWordWrap,
    ToggleMinimap,
    ToggleVimMode,
    FormatDocument,
    ToggleComment,
    Fold,
    Unfold,
    FoldAll,
    UnfoldAll,
    AddNextOccurrence,
    SelectAllOccurrences,
    AddCursorAbove,
    AddCursorBelow,
    ShowExplorer,
    ShowSearch,
    FindInFile,
    ReplaceInFile,
    ReplaceInFiles,
    FindNext,
    FindPrevious,
    ShowSourceControl,
    StageHunk,
    GitSwitchBranch,
    GitCreateBranch,
    GitDeleteBranch,
    GitForceDeleteBranch,
    GitStash,
    GitStashUntracked,
    GitStashPopLatest,
    GitStashPop,
    GitStashApply,
    GitStashDrop,
    MergeAcceptCurrent,
    MergeAcceptIncoming,
    MergeAcceptBoth,
    MergeAcceptAllCurrent,
    MergeAcceptAllIncoming,
    MergeNextConflict,
    MergePrevConflict,
    RevertHunk,
    ToggleBlame,
    GoToDefinition,
    FindReferences,
    RenameSymbol,
    QuickFix,
    GoToSymbol,
    ParameterHints,
    GoBack,
    GoForward,
    ToggleCluster,
    ToggleAws,
    ToggleTerminal,
    ShowProblems,
    JournalToday,
    JournalPrevious,
    JournalFolder,
    TestRunAtCursor,
    TestRunFile,
    TestRerun,
    ShowTests,
    MarkdownPreview,
    SplitEditor,
    FocusLeftEditor,
    FocusRightEditor,
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
    ZoomReset,
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
        Command::ToggleWordWrap,
        Command::ToggleMinimap,
        Command::ToggleVimMode,
        Command::FormatDocument,
        Command::ToggleComment,
        Command::Fold,
        Command::Unfold,
        Command::FoldAll,
        Command::UnfoldAll,
        Command::AddNextOccurrence,
        Command::SelectAllOccurrences,
        Command::AddCursorAbove,
        Command::AddCursorBelow,
        Command::ShowExplorer,
        Command::ShowSearch,
        Command::FindInFile,
        Command::ReplaceInFile,
        Command::ReplaceInFiles,
        Command::FindNext,
        Command::FindPrevious,
        Command::ShowSourceControl,
        Command::StageHunk,
        Command::GitSwitchBranch,
        Command::GitCreateBranch,
        Command::GitDeleteBranch,
        Command::GitForceDeleteBranch,
        Command::GitStash,
        Command::GitStashUntracked,
        Command::GitStashPopLatest,
        Command::GitStashPop,
        Command::GitStashApply,
        Command::GitStashDrop,
        Command::MergeAcceptCurrent,
        Command::MergeAcceptIncoming,
        Command::MergeAcceptBoth,
        Command::MergeAcceptAllCurrent,
        Command::MergeAcceptAllIncoming,
        Command::MergeNextConflict,
        Command::MergePrevConflict,
        Command::RevertHunk,
        Command::ToggleBlame,
        Command::GoToDefinition,
        Command::FindReferences,
        Command::RenameSymbol,
        Command::QuickFix,
        Command::GoToSymbol,
        Command::ParameterHints,
        Command::GoBack,
        Command::GoForward,
        Command::ToggleCluster,
        Command::ToggleAws,
        Command::ToggleTerminal,
        Command::ShowProblems,
        Command::JournalToday,
        Command::JournalPrevious,
        Command::JournalFolder,
        Command::TestRunAtCursor,
        Command::TestRunFile,
        Command::TestRerun,
        Command::ShowTests,
        Command::MarkdownPreview,
        Command::SplitEditor,
        Command::FocusLeftEditor,
        Command::FocusRightEditor,
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
        Command::ZoomReset,
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
            Command::ToggleWordWrap => "View: Toggle Word Wrap",
            Command::ToggleMinimap => "View: Toggle Minimap",
            Command::ToggleVimMode => "View: Toggle Vim Mode",
            Command::FormatDocument => "Format Document",
            Command::ToggleComment => "Toggle Line Comment",
            Command::Fold => "Fold",
            Command::Unfold => "Unfold",
            Command::FoldAll => "Fold All",
            Command::UnfoldAll => "Unfold All",
            Command::AddNextOccurrence => "Selection: Add Next Occurrence",
            Command::SelectAllOccurrences => "Selection: Select All Occurrences",
            Command::AddCursorAbove => "Selection: Add Cursor Above",
            Command::AddCursorBelow => "Selection: Add Cursor Below",
            Command::ShowExplorer => "View: Show Explorer",
            Command::ShowSearch => "View: Search in Files",
            Command::FindInFile => "Find",
            Command::ReplaceInFile => "Replace",
            Command::ReplaceInFiles => "Replace in Files",
            Command::FindNext => "Find Next",
            Command::FindPrevious => "Find Previous",
            Command::GoToDefinition => "Go: Go to Definition",
            Command::FindReferences => "Go: Find All References",
            Command::RenameSymbol => "Rename Symbol",
            Command::QuickFix => "Quick Fix",
            Command::GoToSymbol => "Go: Go to Symbol in File",
            Command::ParameterHints => "Trigger Parameter Hints",
            Command::GoBack => "Go: Back",
            Command::GoForward => "Go: Forward",
            Command::ShowSourceControl => "View: Show Source Control",
            Command::StageHunk => "Git: Stage This Change",
            Command::GitSwitchBranch => "Git: Switch Branch…",
            Command::GitCreateBranch => "Git: Create Branch…",
            Command::GitDeleteBranch => "Git: Delete Branch…",
            Command::GitForceDeleteBranch => "Git: Force Delete Branch…",
            Command::GitStash => "Git: Stash",
            Command::GitStashUntracked => "Git: Stash (Include Untracked)",
            Command::GitStashPopLatest => "Git: Pop Latest Stash",
            Command::GitStashPop => "Git: Pop Stash…",
            Command::GitStashApply => "Git: Apply Stash…",
            Command::GitStashDrop => "Git: Drop Stash…",
            Command::MergeAcceptCurrent => "Merge: Accept Current Change",
            Command::MergeAcceptIncoming => "Merge: Accept Incoming Change",
            Command::MergeAcceptBoth => "Merge: Accept Both Changes",
            Command::MergeAcceptAllCurrent => "Merge: Accept All Current",
            Command::MergeAcceptAllIncoming => "Merge: Accept All Incoming",
            Command::MergeNextConflict => "Merge: Next Conflict",
            Command::MergePrevConflict => "Merge: Previous Conflict",
            Command::RevertHunk => "Git: Revert This Change",
            Command::ToggleBlame => "Git: Toggle Blame",
            Command::ToggleCluster => "View: Toggle Kubernetes Cluster",
            Command::ToggleAws => "View: Toggle AWS",
            Command::ToggleTerminal => "View: Toggle Terminal",
            Command::ShowProblems => "View: Show Problems",
            Command::JournalToday => "Journal: Write Today's Log",
            Command::JournalPrevious => "Journal: Write Log for Last Working Day",
            Command::JournalFolder => "Journal: Open Logs Folder",
            Command::TestRunAtCursor => "Test: Run Test at Cursor",
            Command::TestRunFile => "Test: Run Tests in File",
            Command::TestRerun => "Test: Run Last Again",
            Command::ShowTests => "View: Show Tests",
            Command::MarkdownPreview => "Markdown: Toggle Preview",
            Command::SplitEditor => "View: Split Editor",
            Command::FocusLeftEditor => "View: Focus Left Editor",
            Command::FocusRightEditor => "View: Focus Right Editor",
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
            Command::ZoomReset => "View: Reset Zoom",
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
            // Ctrl/Cmd+Shift+O goes to a symbol, as everywhere else.
            Command::OpenFolder => s(cmd | Modifiers::ALT, Key::O),
            Command::Save => s(cmd, Key::S),
            Command::SaveAs => s(cmd_shift, Key::S),
            Command::CloseTab => s(cmd, Key::W),
            Command::NextTab => s(Modifiers::CTRL, Key::Tab),
            Command::PrevTab => s(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab),
            Command::Undo => s(cmd, Key::Z),
            // macOS convention is ⇧⌘Z; elsewhere Ctrl+Y.
            Command::Redo if cfg!(target_os = "macos") => s(cmd_shift, Key::Z),
            Command::Redo => s(cmd, Key::Y),
            Command::SelectAll => s(cmd, Key::A),
            Command::TriggerCompletion => s(Modifiers::CTRL, Key::Space),
            Command::ToggleSidebar => s(cmd, Key::B),
            // Alt+Z everywhere (⌥Z on macOS), as in VS Code.
            Command::ToggleWordWrap => s(Modifiers::ALT, Key::Z),
            // No default shortcut, like VS Code (it's in View and the palette).
            Command::ToggleMinimap => None,
            Command::FoldAll | Command::UnfoldAll => None,
            Command::ToggleVimMode => None,
            // VS Code's ⇧⌥F / Shift+Alt+F.
            Command::FormatDocument => s(Modifiers::SHIFT | Modifiers::ALT, Key::F),
            Command::ToggleComment => s(cmd, Key::Slash),
            // As in VS Code on macOS; Ctrl+Shift+[ elsewhere would clash
            // with keyboards that need Shift for brackets.
            Command::Fold => s(cmd | Modifiers::ALT, Key::OpenBracket),
            Command::Unfold => s(cmd | Modifiers::ALT, Key::CloseBracket),
            Command::AddNextOccurrence => s(cmd, Key::D),
            Command::SelectAllOccurrences => s(cmd_shift, Key::L),
            // VS Code: ⌥⌘↑/↓ on macOS, Ctrl+Alt+↑/↓ elsewhere.
            Command::AddCursorAbove => s(add_cursor_mods(), Key::ArrowUp),
            Command::AddCursorBelow => s(add_cursor_mods(), Key::ArrowDown),
            Command::ShowExplorer => s(Modifiers::CTRL | Modifiers::SHIFT, Key::E),
            Command::ShowSearch => s(cmd_shift, Key::F),
            Command::FindInFile => s(cmd, Key::F),
            // VS Code: ⌥⌘F on macOS, Ctrl+H elsewhere.
            Command::ReplaceInFile => {
                if cfg!(target_os = "macos") {
                    s(Modifiers::COMMAND | Modifiers::ALT, Key::F)
                } else {
                    s(cmd, Key::H)
                }
            }
            Command::ReplaceInFiles => s(cmd_shift, Key::H),
            Command::FindNext => s(Modifiers::NONE, Key::F3),
            Command::FindPrevious => s(Modifiers::SHIFT, Key::F3),
            Command::GoToDefinition => s(Modifiers::NONE, Key::F12),
            Command::FindReferences => s(Modifiers::SHIFT, Key::F12),
            Command::RenameSymbol => s(Modifiers::NONE, Key::F2),
            Command::QuickFix => s(cmd, Key::Period),
            Command::GoToSymbol => s(cmd | Modifiers::SHIFT, Key::O),
            Command::ParameterHints => s(cmd | Modifiers::SHIFT, Key::Space),
            // VS Code: ⌃- / ⌃⇧- on macOS, Alt+Left / Alt+Right elsewhere.
            Command::GoBack if cfg!(target_os = "macos") => s(Modifiers::CTRL, Key::Minus),
            Command::GoBack => s(Modifiers::ALT, Key::ArrowLeft),
            Command::GoForward if cfg!(target_os = "macos") => s(Modifiers::CTRL | Modifiers::SHIFT, Key::Minus),
            Command::GoForward => s(Modifiers::ALT, Key::ArrowRight),
            Command::ShowSourceControl => s(Modifiers::CTRL | Modifiers::SHIFT, Key::G),
            // No default keys: from the gutter, the menu or the palette.
            Command::StageHunk | Command::RevertHunk => None,
            Command::GitSwitchBranch
            | Command::GitCreateBranch
            | Command::GitDeleteBranch
            | Command::GitForceDeleteBranch
            | Command::GitStash
            | Command::GitStashUntracked
            | Command::GitStashPopLatest
            | Command::GitStashPop
            | Command::GitStashApply
            | Command::GitStashDrop => None,
            Command::MergeAcceptCurrent
            | Command::MergeAcceptIncoming
            | Command::MergeAcceptBoth
            | Command::MergeAcceptAllCurrent
            | Command::MergeAcceptAllIncoming
            | Command::MergeNextConflict
            | Command::MergePrevConflict => None,
            Command::ToggleBlame => s(cmd | Modifiers::ALT, Key::B),
            Command::ToggleCluster => s(cmd_shift, Key::K),
            Command::ToggleAws => s(cmd_shift, Key::A),
            Command::ToggleTerminal => s(Modifiers::CTRL, Key::Backtick),
            Command::ShowProblems => s(cmd_shift, Key::M),
            Command::TestRunAtCursor => s(cmd | Modifiers::ALT, Key::T),
            Command::TestRunFile | Command::TestRerun | Command::ShowTests => None,
            Command::JournalToday | Command::JournalPrevious | Command::JournalFolder => None,
            // Not ⇧V: that is Paste (as plain text) on every system.
            Command::MarkdownPreview => s(cmd | Modifiers::ALT, Key::V),
            Command::SplitEditor => s(cmd, Key::Backslash),
            Command::FocusLeftEditor => s(cmd, Key::Num1),
            Command::FocusRightEditor => s(cmd, Key::Num2),
            // ⌘T on macOS, as in its terminals. Elsewhere Ctrl+T belongs
            // to the shell (it swaps two characters), so the VS Code
            // chord stays.
            Command::NewTerminal if cfg!(target_os = "macos") => s(Modifiers::MAC_CMD, Key::T),
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
            Command::ZoomReset => s(cmd, Key::Num0),
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

/// Like [`fuzzy_score`], also returning which characters of `haystack`
/// matched (char indices), to highlight them.
pub fn fuzzy_match(needle: &str, haystack: &str) -> Option<(usize, Vec<usize>)> {
    let hay: Vec<char> = haystack.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut score = 0;
    let mut pos = 0;
    let mut last = None;
    let mut hits = Vec::new();
    for n in needle.chars().map(|c| c.to_ascii_lowercase()) {
        let found = hay[pos..].iter().position(|&h| h == n)? + pos;
        score += match last {
            Some(l) => found - l - 1,
            None => found,
        };
        hits.push(found);
        last = Some(found);
        pos = found + 1;
    }
    Some((score, hits))
}

/// ⌥⌘ on macOS, Ctrl+Alt elsewhere: the modifiers for adding a cursor.
fn add_cursor_mods() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::COMMAND | Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::ALT
    }
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
        // On macOS a new terminal is ⌘T, which shares no key with the
        // toggle, so there is no order between them to keep.
        if !cfg!(target_os = "macos") {
            assert!(pos(Command::NewTerminal) < pos(Command::ToggleTerminal));
        }
        assert!(pos(Command::SaveAs) < pos(Command::Save));
    }

    #[test]
    fn fuzzy_match_positions() {
        assert_eq!(fuzzy_match("pl", "println"), Some((4, vec![0, 5])));
        assert_eq!(fuzzy_match("", "x"), Some((0, vec![])));
        assert!(fuzzy_match("zz", "println").is_none());
    }

    #[test]
    fn every_command_has_a_label() {
        for c in Command::ALL {
            assert!(!c.label().is_empty());
        }
    }
}
