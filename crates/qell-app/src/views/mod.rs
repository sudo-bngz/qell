//! The panels that make up the window. Each one is a function taking the
//! whole app so it can read state and queue commands; none of them keeps
//! state of its own beyond what egui remembers between frames.

pub mod editor;
pub mod menu;
pub mod palette;
pub mod scm;
pub mod search;
pub mod settings;
pub mod sidebar;
pub mod status;
pub mod terminal;
