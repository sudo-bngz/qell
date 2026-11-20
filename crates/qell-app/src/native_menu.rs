//! The native menu bar (macOS).
//!
//! On macOS the menu belongs at the top of the screen, with the app menu
//! (About, Settings…, Hide, Quit) and ⌘ shortcuts shown next to each item.
//! [`NativeMenu`] builds that menu with `muda` from the same [`Command`]s
//! the palette and the in-window menu use. Other platforms keep the
//! in-window menu bar.
//!
//! Menu shortcuts are handled by AppKit before the window sees the key, so
//! every item's action arrives here. Commands run as commands; the Edit
//! items (undo, copy, paste…) are replayed into egui as the input they
//! stand for, so they act on whatever has focus: the editor, the terminal,
//! a text field in Settings or the commit box.
//!
//! The module builds and is tested on every platform; it is only
//! installed on macOS.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

use egui::{Event, Key, KeyboardShortcut, Modifiers};
use muda::accelerator::Accelerator;
use muda::{AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use crate::commands::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl EditAction {
    fn label(self) -> &'static str {
        match self {
            EditAction::Undo => "Undo",
            EditAction::Redo => "Redo",
            EditAction::Cut => "Cut",
            EditAction::Copy => "Copy",
            EditAction::Paste => "Paste",
            EditAction::SelectAll => "Select All",
        }
    }

    /// The standard macOS shortcut.
    fn shortcut(self) -> KeyboardShortcut {
        let cmd = Modifiers::COMMAND;
        let key = match self {
            EditAction::Undo | EditAction::Redo => Key::Z,
            EditAction::Cut => Key::X,
            EditAction::Copy => Key::C,
            EditAction::Paste => Key::V,
            EditAction::SelectAll => Key::A,
        };
        let mods = if self == EditAction::Redo { cmd | Modifiers::SHIFT } else { cmd };
        KeyboardShortcut::new(mods, key)
    }

    /// The egui input this menu item stands for.
    fn event(self) -> Option<Event> {
        let key = |sc: KeyboardShortcut| Event::Key {
            key: sc.logical_key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: sc.modifiers,
        };
        Some(match self {
            EditAction::Cut => Event::Cut,
            EditAction::Copy => Event::Copy,
            EditAction::Paste => Event::Paste(clipboard_text()?),
            other => key(other.shortcut()),
        })
    }
}

#[cfg(target_os = "macos")]
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

#[cfg(not(target_os = "macos"))]
fn clipboard_text() -> Option<String> {
    None
}

/// What choosing a menu item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Command(Command),
    Edit(EditAction),
}

/// One entry of the menu layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A Qell command, with an optional macOS-style label.
    Cmd(Command, Option<&'static str>),
    Edit(EditAction),
    Separator,
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    Fullscreen,
    BringAllToFront,
}

/// The menu bar, following the macOS Human Interface Guidelines: app menu,
/// File, Edit, View, Window.
pub fn layout() -> Vec<(&'static str, Vec<Entry>)> {
    use Command as C;
    use EditAction as E;
    use Entry::*;
    vec![
        (
            "Qell",
            vec![
                About,
                Separator,
                Cmd(C::OpenSettings, Some("Settings…")),
                Separator,
                Services,
                Separator,
                Hide,
                HideOthers,
                ShowAll,
                Separator,
                Cmd(C::Quit, Some("Quit Qell")),
            ],
        ),
        (
            "File",
            vec![
                Cmd(C::NewFile, None),
                Cmd(C::OpenFile, Some("Open…")),
                Cmd(C::OpenFolder, Some("Open Folder…")),
                Separator,
                Cmd(C::Save, None),
                Cmd(C::SaveAs, Some("Save As…")),
                Separator,
                Cmd(C::CloseTab, Some("Close Tab")),
            ],
        ),
        (
            "Edit",
            vec![
                Edit(E::Undo),
                Edit(E::Redo),
                Separator,
                Edit(E::Cut),
                Edit(E::Copy),
                Edit(E::Paste),
                Edit(E::SelectAll),
                Separator,
                Cmd(C::TriggerCompletion, None),
            ],
        ),
        (
            "View",
            vec![
                Cmd(C::CommandPalette, Some("Command Palette…")),
                Separator,
                Cmd(C::ShowExplorer, Some("Explorer")),
                Cmd(C::ShowSearch, Some("Search")),
                Cmd(C::ShowSourceControl, Some("Source Control")),
                Cmd(C::ToggleSidebar, Some("Toggle Sidebar")),
                Cmd(C::ToggleWordWrap, Some("Word Wrap")),
                Cmd(C::ToggleMinimap, Some("Minimap")),
                Cmd(C::ToggleVimMode, Some("Vim Mode")),
                Separator,
                Cmd(C::ToggleTerminal, Some("Toggle Terminal")),
                Cmd(C::NewTerminal, Some("New Terminal")),
                Separator,
                Cmd(C::ZoomIn, Some("Zoom In")),
                Cmd(C::ZoomOut, Some("Zoom Out")),
                Separator,
                Fullscreen,
            ],
        ),
        (
            "Go",
            vec![
                Cmd(C::GoBack, Some("Back")),
                Cmd(C::GoForward, Some("Forward")),
                Separator,
                Cmd(C::QuickOpen, Some("Go to File…")),
                Cmd(C::GoToLine, Some("Go to Line…")),
                Separator,
                Cmd(C::GoToDefinition, Some("Go to Definition")),
                Cmd(C::FindReferences, Some("Find All References")),
            ],
        ),
        ("Window", vec![Minimize, Zoom, Separator, BringAllToFront]),
    ]
}

/// A menu label from a command label: "File: Save As…" → "Save As…".
fn command_label(cmd: Command) -> &'static str {
    let l = cmd.label();
    l.split_once(": ").map(|(_, rest)| rest).unwrap_or(l)
}

/// muda accelerator syntax for an egui shortcut ("cmd+shift+KeyP"), or
/// `None` for keys the menu can't express.
pub fn accelerator(sc: &KeyboardShortcut) -> Option<String> {
    let m = sc.modifiers;
    let mut parts: Vec<&str> = Vec::new();
    if m.command || m.mac_cmd {
        parts.push("cmd");
    }
    if m.ctrl {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    let name = sc.logical_key.name();
    let code = match sc.logical_key {
        Key::Backtick => "Backquote".to_string(),
        Key::Equals => "Equal".to_string(),
        Key::Plus => "Equal".to_string(),
        Key::Comma | Key::Minus | Key::Period | Key::Space | Key::Tab | Key::Enter | Key::PageUp | Key::PageDown => {
            name.to_string()
        }
        Key::ArrowLeft => "ArrowLeft".to_string(),
        Key::ArrowRight => "ArrowRight".to_string(),
        Key::ArrowUp => "ArrowUp".to_string(),
        Key::ArrowDown => "ArrowDown".to_string(),
        _ if name.len() >= 2 && name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit()) => name.to_string(),
        _ if name.len() == 1 && name.chars().all(|c| c.is_ascii_uppercase()) => format!("Key{name}"),
        _ if name.len() == 1 && name.chars().all(|c| c.is_ascii_digit()) => format!("Digit{name}"),
        _ => return None,
    };
    parts.push(&code);
    Some(parts.join("+"))
}

pub struct NativeMenu {
    /// The menu must outlive its use by AppKit.
    _menu: Menu,
    actions: HashMap<String, Action>,
    rx: Receiver<String>,
}

impl NativeMenu {
    /// Build the menu. `ctx` is woken up when an item is chosen.
    pub fn build(ctx: &egui::Context) -> muda::Result<Self> {
        let menu = Menu::new();
        let mut actions = HashMap::new();
        for (title, entries) in layout() {
            let submenu = Submenu::new(title, true);
            for entry in entries {
                let item: Box<dyn IsMenuItem> = match entry {
                    Entry::Cmd(cmd, label) => {
                        let id = format!("cmd:{cmd:?}");
                        let accel = cmd.shortcut().as_ref().and_then(accelerator).and_then(|a| a.parse::<Accelerator>().ok());
                        actions.insert(id.clone(), Action::Command(cmd));
                        Box::new(MenuItem::with_id(id, label.unwrap_or(command_label(cmd)), true, accel))
                    }
                    Entry::Edit(e) => {
                        let id = format!("edit:{e:?}");
                        let accel = accelerator(&e.shortcut()).and_then(|a| a.parse::<Accelerator>().ok());
                        actions.insert(id.clone(), Action::Edit(e));
                        Box::new(MenuItem::with_id(id, e.label(), true, accel))
                    }
                    Entry::Separator => Box::new(PredefinedMenuItem::separator()),
                    Entry::About => Box::new(PredefinedMenuItem::about(
                        Some("About Qell"),
                        Some(AboutMetadata {
                            name: Some("Qell".into()),
                            version: Some(env!("CARGO_PKG_VERSION").into()),
                            comments: Some("A minimalist code editor.".into()),
                            ..Default::default()
                        }),
                    )),
                    Entry::Services => Box::new(PredefinedMenuItem::services(None)),
                    Entry::Hide => Box::new(PredefinedMenuItem::hide(Some("Hide Qell"))),
                    Entry::HideOthers => Box::new(PredefinedMenuItem::hide_others(None)),
                    Entry::ShowAll => Box::new(PredefinedMenuItem::show_all(None)),
                    Entry::Minimize => Box::new(PredefinedMenuItem::minimize(None)),
                    Entry::Zoom => Box::new(PredefinedMenuItem::maximize(Some("Zoom"))),
                    Entry::Fullscreen => Box::new(PredefinedMenuItem::fullscreen(None)),
                    Entry::BringAllToFront => Box::new(PredefinedMenuItem::bring_all_to_front(None)),
                };
                submenu.append(item.as_ref())?;
            }
            #[cfg(target_os = "macos")]
            if title == "Window" {
                submenu.set_as_windows_menu_for_nsapp();
            }
            menu.append(&submenu)?;
        }

        // Menu events arrive on a global channel; forward them to ours and
        // wake the UI so the action runs right away.
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id.0);
            ctx.request_repaint();
        }));
        Ok(Self { _menu: menu, actions, rx })
    }

    /// Build the menu and make it the application's menu bar (macOS only).
    #[cfg(target_os = "macos")]
    pub fn install(ctx: &egui::Context) -> muda::Result<Self> {
        let menu = Self::build(ctx)?;
        menu._menu.init_for_nsapp();
        Ok(menu)
    }

    /// Actions chosen since the last call.
    pub fn poll(&self) -> Vec<Action> {
        self.rx.try_iter().filter_map(|id| self.actions.get(&id).copied()).collect()
    }

    /// Input event to replay into egui for an Edit action.
    pub fn edit_event(action: EditAction) -> Option<Event> {
        action.event()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_shortcut_converts_to_an_accelerator() {
        for (_, entries) in layout() {
            for e in entries {
                let sc = match e {
                    Entry::Cmd(c, _) => c.shortcut(),
                    Entry::Edit(e) => Some(e.shortcut()),
                    _ => None,
                };
                let Some(sc) = sc else { continue };
                let a = accelerator(&sc).unwrap_or_else(|| panic!("{e:?}: no accelerator for {sc:?}"));
                assert!(a.parse::<Accelerator>().is_ok(), "{e:?}: {a} does not parse");
            }
        }
    }

    #[test]
    fn accelerator_syntax() {
        let sc = |m, k| KeyboardShortcut::new(m, k);
        assert_eq!(accelerator(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::P)).as_deref(), Some("cmd+shift+KeyP"));
        assert_eq!(accelerator(&sc(Modifiers::CTRL, Key::Backtick)).as_deref(), Some("ctrl+Backquote"));
        assert_eq!(accelerator(&sc(Modifiers::COMMAND, Key::Comma)).as_deref(), Some("cmd+Comma"));
        assert_eq!(accelerator(&sc(Modifiers::COMMAND, Key::Equals)).as_deref(), Some("cmd+Equal"));
    }

    #[test]
    fn no_two_items_share_a_shortcut() {
        let mut seen = HashMap::new();
        for (menu, entries) in layout() {
            for e in entries {
                let sc = match e {
                    Entry::Cmd(c, _) => c.shortcut(),
                    Entry::Edit(e) => Some(e.shortcut()),
                    _ => None,
                };
                if let Some(a) = sc.as_ref().and_then(accelerator) {
                    if let Some(prev) = seen.insert(a.clone(), (menu, e)) {
                        panic!("{a} used by {prev:?} and {:?}", (menu, e));
                    }
                }
            }
        }
    }

    #[test]
    fn every_menu_command_is_a_real_command() {
        for (_, entries) in layout() {
            for e in entries {
                if let Entry::Cmd(c, _) = e {
                    assert!(Command::ALL.contains(&c));
                }
            }
        }
    }

    // AppKit only creates menus on the main thread, and tests run on worker
    // threads; on macOS the app itself builds the menu at startup.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn whole_menu_builds() {
        // Every item, accelerator and predefined entry is accepted by muda.
        let menu = NativeMenu::build(&egui::Context::default()).expect("menu builds");
        let commands = menu.actions.values().filter(|a| matches!(a, Action::Command(_))).count();
        assert!(commands >= 15, "{commands}");
        assert_eq!(menu.actions.values().filter(|a| matches!(a, Action::Edit(_))).count(), 6);
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    }

    #[test]
    fn edit_items_replay_their_input() {
        assert_eq!(EditAction::Copy.event(), Some(Event::Copy));
        assert_eq!(EditAction::Cut.event(), Some(Event::Cut));
        let Some(Event::Key { key, modifiers, .. }) = EditAction::Redo.event() else { panic!() };
        assert_eq!(key, Key::Z);
        assert!(modifiers.shift && modifiers.command);
    }
}
