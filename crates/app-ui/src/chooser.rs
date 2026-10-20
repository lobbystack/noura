//! The chooser: shown when no workspace is open, and when the last
//! workspace can't open.

use app_core::RecentWorkspace;
use egui::{Button, CentralPanel, RichText, Ui, Widget};

use crate::{
    NouraUi,
    common::{self, NewWorkspace, muted, primary_button},
    theme::TITLE_SIZE,
};

const WIDTH: f32 = 440.0;

pub(crate) fn show(state: &mut NouraUi, ui: &mut Ui, problem: Option<&str>) {
    CentralPanel::default().show(ui, |ui| {
        let width = ui.available_width().min(WIDTH);
        let side = (ui.available_width() - width) / 2.0;
        ui.add_space((ui.available_height() * 0.15).min(120.0));
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(width);
                contents(state, ui, problem);
            });
        });
    });
}

fn contents(state: &mut NouraUi, ui: &mut Ui, problem: Option<&str>) {
    let busy = state.app.is_busy();
    let (title, description) = match problem {
        Some(problem) => ("Can't open the workspace", problem),
        None => (
            "Open a folder",
            "Noura adds a small .noura folder. Your files stay as they are.",
        ),
    };
    ui.label(RichText::new(title).size(TITLE_SIZE).strong());
    ui.label(description);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if primary_button(ui, "Open folder…", !busy).clicked() {
            state.pick_and_open();
        }
        if problem.is_some() && ui.add_enabled(!busy, Button::new("Try again")).clicked() {
            state.app.try_again();
        }
        if busy {
            ui.spinner().accessible_name("Opening workspace");
        }
    });
    common::notice(state, ui);

    let recents = state.app.recents().to_vec();
    if !recents.is_empty() {
        ui.separator();
        muted(ui, "Recent workspaces");
        for recent in &recents {
            if recent_row(ui, recent, !busy).clicked() {
                state.app.open_recent(&recent.workspace_id);
            }
        }
    }

    ui.separator();
    if state.new_workspace.is_some() {
        ui.label(RichText::new("New workspace").strong());
        common::new_workspace_form(state, ui);
    } else if ui
        .add_enabled(!busy, Button::new("New workspace…").frame(false))
        .clicked()
    {
        state.new_workspace = Some(NewWorkspace::new());
    }
}

/// One recent workspace: its name, with its folder below.
fn recent_row(ui: &mut Ui, recent: &RecentWorkspace, enabled: bool) -> egui::Response {
    let text = RichText::new(&recent.name).strong();
    let response = ui
        .add_enabled_ui(enabled, |ui| Button::new(text).frame(false).ui(ui))
        .inner;
    muted(ui, &recent.path);
    let path = recent.path.clone();
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_description(path));
    response
}
