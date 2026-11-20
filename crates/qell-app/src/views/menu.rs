//! The menu bar.

use egui::Context;

use crate::app::QellApp;
use crate::commands::Command;

fn item(ui: &mut egui::Ui, app: &mut QellApp, cmd: Command) {
    let label = cmd.label().split_once(": ").map(|(_, l)| l).unwrap_or(cmd.label());
    let shortcut = cmd.shortcut_text(ui.ctx());
    let button = egui::Button::new(label).shortcut_text(shortcut);
    if ui.add(button).clicked() {
        app.pending_commands.push(cmd);
        ui.close();
    }
}

pub fn show(app: &mut QellApp, ctx: &Context) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                item(ui, app, Command::NewFile);
                item(ui, app, Command::OpenFile);
                item(ui, app, Command::OpenFolder);
                ui.separator();
                item(ui, app, Command::Save);
                item(ui, app, Command::SaveAs);
                ui.separator();
                item(ui, app, Command::CloseTab);
                ui.separator();
                item(ui, app, Command::OpenSettings);
                ui.separator();
                item(ui, app, Command::Quit);
            });
            ui.menu_button("Edit", |ui| {
                item(ui, app, Command::Undo);
                item(ui, app, Command::Redo);
                ui.separator();
                item(ui, app, Command::SelectAll);
                item(ui, app, Command::TriggerCompletion);
            });
            ui.menu_button("View", |ui| {
                item(ui, app, Command::CommandPalette);
                item(ui, app, Command::QuickOpen);
                item(ui, app, Command::GoToLine);
                ui.separator();
                item(ui, app, Command::ToggleSidebar);
                item(ui, app, Command::ToggleTerminal);
                item(ui, app, Command::NewTerminal);
                ui.separator();
                item(ui, app, Command::ZoomIn);
                item(ui, app, Command::ZoomOut);
            });
            if !app.plugin_commands.is_empty() {
                ui.menu_button("Plugins", |ui| {
                    for name in app.plugin_commands.clone() {
                        if ui.button(&name).clicked() {
                            app.pending_plugin_commands.push(name);
                            ui.close();
                        }
                    }
                });
            }
        });
    });
}
