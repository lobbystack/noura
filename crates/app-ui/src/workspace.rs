//! The workspace screen: the rail, the sidebar with the workspace switcher
//! and the file tree, the tab bar, and the document area.

use app_core::{INBOX_PAGE, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, TabContent, TabPlace};
use egui::{
    Button, CentralPanel, Frame, Id, Margin, Modal, Panel, RichText, ScrollArea, Ui, UiBuilder,
    accesskit::Role, containers::menu::MenuButton, vec2,
};

use crate::{
    NouraUi,
    common::{self, NewWorkspace, muted},
    inbox,
    rail::{self, RAIL_WIDTH},
    tab_bar::{self, BAR_HEIGHT},
    theme::{Palette, TITLE_SIZE},
    tree,
};

pub(crate) fn show(state: &mut NouraUi, ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    Panel::left("rail")
        .resizable(false)
        .exact_size(RAIL_WIDTH)
        .frame(Frame::new().fill(palette.sidebar))
        .show(ui, |ui| rail::show(ui, &mut state.app));
    if state.app.layout().sidebar_shown {
        sidebar(state, ui);
    }
    let sidebar_shown = state.app.layout().sidebar_shown;
    Panel::top("tab-bar")
        .resizable(false)
        .exact_size(BAR_HEIGHT)
        .frame(
            Frame::new()
                .fill(palette.sidebar)
                .inner_margin(Margin::symmetric(8, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                // With the sidebar hidden, its switcher moves here.
                if !sidebar_shown {
                    switcher(state, ui);
                }
                tab_bar::show(ui, &mut state.app, &mut state.tab_bar);
            });
        });
    CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.background)
                .inner_margin(Margin::same(24)),
        )
        .show(ui, |ui| {
            common::notice(state, ui);
            content(state, ui);
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

/// The resizable sidebar: the workspace switcher and the Files section.
fn sidebar(state: &mut NouraUi, ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    let width = state.app.layout().sidebar_width;
    let panel = Panel::left("sidebar")
        .resizable(true)
        .default_size(width)
        .size_range(SIDEBAR_MIN_WIDTH..=SIDEBAR_MAX_WIDTH)
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
    // Save the width once a resize ends.
    let shown = panel.response.rect.width().round();
    if shown != width && !ui.input(|input| input.pointer.any_down()) {
        state.app.set_sidebar_width(shown);
    }
}

/// The workspace switcher: the current workspace's name, opening a menu of
/// recent workspaces and workspace actions.
fn switcher(state: &mut NouraUi, ui: &mut Ui) {
    let Some(workspace) = state.app.workspace() else {
        return;
    };
    let current = workspace.id().to_owned();
    let name = workspace.name();
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

/// What the active tab shows: a Markdown file in the note editor, another
/// file's name and workspace-relative path, which the file viewers replace
/// later, the Inbox, or another page's title, which the page's own view
/// replaces. Each tab keeps its scroll position; a note's editor keeps its
/// own.
fn content(state: &mut NouraUi, ui: &mut Ui) {
    let active = state
        .app
        .active_tab()
        .map(|tab| (tab.id(), tab.content().clone(), tab.place().clone()));
    let builder = UiBuilder::new()
        .accessibility_role(Role::Region)
        .accessibility_label("Document");
    ui.scope_builder(builder, |ui| {
        let Some((id, content, place)) = active else {
            state.shown_tab = None;
            ui.centered_and_justified(|ui| muted(ui, "Open a file from the sidebar."));
            return;
        };
        if matches!(content, TabContent::File { .. })
            && crate::note::show(ui, &mut state.app, &mut state.notes, state.window, id)
        {
            state.shown_tab = Some(id);
            return;
        }
        let is_inbox = matches!(
            &content,
            TabContent::Page { page_id, .. } if page_id == INBOX_PAGE
        );
        // The Inbox's text wraps to the window, so it scrolls only down.
        let scroll = if is_inbox {
            ScrollArea::vertical()
        } else {
            ScrollArea::both()
        };
        let mut scroll = scroll.id_salt(("document", id)).auto_shrink(false);
        if state.shown_tab != Some(id) {
            state.shown_tab = Some(id);
            scroll = scroll.scroll_offset(vec2(place.scroll[0], place.scroll[1]));
        }
        let output = scroll.show(ui, |ui| match &content {
            TabContent::File { file, .. } => {
                ui.label(RichText::new(&file.name).size(TITLE_SIZE).strong());
                muted(ui, &file.relative_path);
            }
            TabContent::Page { .. } if is_inbox => inbox::show(ui, &mut state.app, state.window),
            TabContent::Page { title, page_id } => {
                ui.label(RichText::new(title).size(TITLE_SIZE).strong());
                if state.app.is_plugin_page(page_id) {
                    ui.add_space(8.0);
                    crate::plugin_view::show(ui, &mut state.app, state.window, page_id);
                }
            }
        });
        let offset = output.state.clamped_offset();
        if [offset.x, offset.y] != place.scroll {
            state.app.set_tab_place(
                id,
                TabPlace {
                    scroll: [offset.x, offset.y],
                    view: place.view,
                },
            );
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
