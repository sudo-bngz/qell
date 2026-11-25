//! The panels that make up the window. Each one is a function taking the
//! whole app so it can read state and queue commands; none of them keeps
//! state of its own beyond what egui remembers between frames.

pub mod aws;
pub mod editor;
pub mod find;
pub mod k8s;
pub mod media;
pub mod menu;
pub mod minimap;
pub mod palette;
pub mod scm;
pub mod search;
pub mod settings;
pub mod sidebar;
pub mod status;
pub mod terminal;

/// A curly underline from `x0` to `x1` along `y`, the way errors are
/// marked: in the editor under the faulty code, in the file tree under
/// the file (and the folders holding it).
pub fn wavy_line(painter: &egui::Painter, x0: f32, x1: f32, y: f32, color: egui::Color32) {
    const STEP: f32 = 2.0;
    const HEIGHT: f32 = 1.5;
    if x1 <= x0 {
        return;
    }
    let mut points = Vec::with_capacity(((x1 - x0) / STEP) as usize + 2);
    let mut x = x0;
    let mut up = true;
    while x < x1 {
        points.push(egui::pos2(x, if up { y - HEIGHT } else { y + HEIGHT }));
        x += STEP;
        up = !up;
    }
    points.push(egui::pos2(x1, if up { y - HEIGHT } else { y + HEIGHT }));
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0_f32, color)));
}

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
