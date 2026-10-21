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
    trash_dialog(state, ui);
    ignored_files_dialog(state, ui);
}

/// Ask before a folder goes to the trash, and say where it goes.
fn trash_dialog(state: &mut NouraUi, ui: &mut Ui) {
    let Some((_, question)) = state.app.pending_trash() else {
        return;
    };
    let question = question.clone();
    let palette = Palette::of(ui.ctx());
    let mut confirm = false;
    let mut cancel = false;
    let modal = Modal::new(Id::unique("trash-folder")).show(ui.ctx(), |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(&question.title).size(TITLE_SIZE).strong());
        muted(ui, &question.message);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            cancel = ui.button("Cancel").clicked();
            confirm = ui
                .add(
                    Button::new(RichText::new("Move to Trash").color(palette.primary_foreground))
                        .fill(palette.destructive),
                )
                .clicked();
        });
    });
    if confirm {
        state.app.confirm_trash();
        state.tree.refocus();
    } else if cancel || modal.should_close() {
        state.app.cancel_trash();
        state.tree.refocus();
    }
}

/// The Ignored files page, in a dialog until Settings exists.
fn ignored_files_dialog(state: &mut NouraUi, ui: &mut Ui) {
    if state.app.ignored_files().is_none() {
        return;
    }
    let mut done = false;
    let modal = Modal::new(Id::unique("ignored-files")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        ui.label(RichText::new("Ignored files").size(TITLE_SIZE).strong());
        crate::ignored_files::page(ui, &mut state.app, &mut state.ignored);
        ui.add_space(8.0);
        done = common::primary_button(ui, "Done", true).clicked();
    });
    if done || modal.should_close() {
        state.app.close_ignored_files();
        state.ignored = Default::default();
    }
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
    let mut reveal = false;
    let mut ignored = false;
    let reveal_label = state.app.platform().reveal_label();
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
        ui.separator();
        if ui.button(reveal_label).clicked() {
            reveal = true;
            ui.close();
        }
        if ui.button("Ignored files\u{2026}").clicked() {
            ignored = true;
            ui.close();
        }
        let others: Vec<_> = recents
            .iter()
            .filter(|recent| recent.workspace_id != current)
            .collect();
        if !others.is_empty() {
            ui.menu_button("Remove from recents", |ui| {
                for recent in others {
                    if ui.add_enabled(!busy, Button::new(&recent.name)).clicked() {
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
    if reveal {
        state.app.reveal_workspace();
    }
    if ignored {
        state.app.open_ignored_files();
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
