//! The panels that make up the window. Each one is a function taking the
//! whole app so it can read state and queue commands; none of them keeps
//! state of its own beyond what egui remembers between frames.

pub mod editor;
pub mod find;
pub mod k8s;
pub mod menu;
pub mod minimap;
pub mod palette;
pub mod scm;
pub mod search;
pub mod settings;
pub mod sidebar;
pub mod status;
pub mod terminal;

/// Hold egui's keyboard focus for a pane that reads keys itself (the
/// editor, the terminal) while it is the active one.
///
/// egui reads Tab and the arrow keys at the start of every frame to move
/// focus between widgets, and Space/Enter "click" the focused one. Without
/// this, a shell's Tab completion and ↑/↓ history moved focus into the file
/// tree (scrolling it to follow) and Enter then opened or closed whatever
/// row was focused. A focus lock keeps those keys in the pane, as egui's own
/// text fields do. The focus is held by an invisible, non-clickable widget,
/// so Space/Enter never turn into a click on the pane itself.
pub fn hold_keyboard(ui: &egui::Ui, id: egui::Id, at: egui::Pos2) {
    let rect = egui::Rect::from_min_size(at, egui::Vec2::ZERO);
    let holder = ui.interact(rect, id, egui::Sense::focusable_noninteractive()).id;
    ui.memory_mut(|m| {
        m.request_focus(holder);
        m.set_focus_lock_filter(
            holder,
            egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
        );
    });
}
