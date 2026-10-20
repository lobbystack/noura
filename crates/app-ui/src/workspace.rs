//! The workspace screen: the sidebar with the workspace switcher and the
//! file tree, and the document area.

use egui::{
    Button, CentralPanel, Frame, Id, Margin, Modal, Panel, RichText, Ui, UiBuilder,
    accesskit::Role, containers::menu::MenuButton,
};

use crate::{
    NouraUi,
    common::{self, NewWorkspace, muted},
    theme::{Palette, TITLE_SIZE},
    tree,
};

pub(crate) fn show(state: &mut NouraUi, ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    Panel::left("sidebar")
        .resizable(true)
        .default_size(260.0)
        .size_range(180.0..=480.0)
        .frame(
            Frame::new()
                .fill(palette.sidebar)
                .inner_margin(Margin::same(8)),
        )
        .show(ui, |ui| {
            switcher(state, ui);
            ui.separator();
            tree::show(ui, &mut state.app, &mut state.tree);
        });
    CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.background)
                .inner_margin(Margin::same(24)),
        )
        .show(ui, |ui| {
            common::notice(state, ui);
            document(state, ui);
        });
    new_workspace_dialog(state, ui);
}

/// The workspace switcher: the current workspace's name, opening a menu of
/// recent workspaces and workspace actions.
fn switcher(state: &mut NouraUi, ui: &mut Ui) {
    let Some(workspace) = state.app.workspace() else {
        return;
    };
    let current = workspace.id().to_owned();
    let name = workspace.name().to_owned();
    let recents = state.app.recents().to_vec();
    let busy = state.app.is_busy();
    let mut open = None;
    let mut remove = None;
    let mut pick = false;
    let mut create = false;
    let button = Button::new(RichText::new(&name).strong()).frame(false);
    let (menu, _) = MenuButton::from_button(button).ui(ui, |ui| {
        muted(ui, "Workspaces");
        for recent in &recents {
            let is_current = recent.workspace_id == current;
            let item = ui.add_enabled(
                !is_current && !busy,
                Button::selectable(is_current, &recent.name),
            );
            let path = recent.path.clone();
            ui.ctx()
                .accesskit_node_builder(item.id, |node| node.set_description(path));
            if item.on_hover_text(&recent.path).clicked() {
                open = Some(recent.workspace_id.clone());
                ui.close();
            }
        }
        ui.separator();
        if ui.add_enabled(!busy, Button::new("Open folder…")).clicked() {
            pick = true;
            ui.close();
        }
        if ui
            .add_enabled(!busy, Button::new("New workspace…"))
            .clicked()
        {
            create = true;
            ui.close();
        }
        let others: Vec<_> = recents
            .iter()
            .filter(|recent| recent.workspace_id != current)
            .collect();
        if !others.is_empty() {
            ui.menu_button("Remove from recents", |ui| {
                for recent in others {
                    if ui.button(&recent.name).clicked() {
                        remove = Some(recent.workspace_id.clone());
                        ui.close();
                    }
                }
            });
        }
    });
    menu.accessible_name(format!("Switch workspace. Current workspace: {name}"));
    if let Some(workspace_id) = open {
        state.app.open_recent(&workspace_id);
    }
    if let Some(workspace_id) = remove {
        state.app.remove_recent(&workspace_id);
    }
    if pick {
        state.pick_and_open();
    }
    if create {
        state.new_workspace = Some(NewWorkspace::new());
    }
}

/// The open file's name and workspace-relative path. The editor and the
/// file viewers replace this later.
fn document(state: &NouraUi, ui: &mut Ui) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Region)
        .accessibility_label("Document");
    ui.scope_builder(builder, |ui| match state.app.document() {
        Some(file) => {
            ui.label(RichText::new(&file.name).size(TITLE_SIZE).strong());
            muted(ui, &file.relative_path);
        }
        None => {
            ui.centered_and_justified(|ui| muted(ui, "Open a file from the sidebar."));
        }
    });
}

fn new_workspace_dialog(state: &mut NouraUi, ui: &mut Ui) {
    if state.new_workspace.is_none() {
        return;
    }
    let modal = Modal::new(Id::unique("new-workspace")).show(ui.ctx(), |ui| {
        ui.set_width(320.0);
        ui.label(RichText::new("New workspace").size(TITLE_SIZE).strong());
        muted(ui, "Name it, then choose its folder.");
        common::new_workspace_form(state, ui);
    });
    if modal.should_close() {
        state.new_workspace = None;
    }
}
