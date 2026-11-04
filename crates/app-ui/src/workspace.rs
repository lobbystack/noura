//! The workspace screen: the rail, the sidebar with the workspace switcher
//! and the file tree, the tab bar, and the document area.

use app_core::{INBOX_PAGE, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, TabContent, TabPlace};
use egui::{
    Button, CentralPanel, Frame, Id, Margin, Modal, Panel, RichText, ScrollArea, Ui, UiBuilder,
    accesskit::Role, vec2,
};

use crate::{
    NouraUi,
    common::{self, Btn, NewWorkspace, Size, Variant, muted},
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
            // The bar is the title bar row: its empty part moves the
            // window.
            let rect = ui.max_rect().expand2(vec2(8.0, 0.0));
            common::title_bar_area(ui, rect, "tab-bar");
            ui.horizontal_centered(|ui| {
                // With the sidebar hidden, its switcher moves here, 24 px
                // in, clear of the window's buttons.
                if !sidebar_shown {
                    ui.add_space(16.0);
                    ui.scope(|ui| {
                        ui.set_max_width(200.0);
                        switcher(state, ui);
                    });
                }
                tab_bar::show(ui, &mut state.app, &mut state.tab_bar);
            });
        });
    // A page scrolls edge to edge, so its header's line runs the page's
    // whole width; its contents keep the 24 px margin inside.
    let page = state
        .app
        .active_tab()
        .is_some_and(|tab| matches!(tab.content(), TabContent::Page { .. }));
    let margin = if page { 0 } else { 24 };
    CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.background)
                .inner_margin(Margin::same(margin)),
        )
        .show(ui, |ui| {
            common::notice(state, ui);
            content(state, ui);
        });
    new_workspace_dialog(state, ui);
    trash_dialog(state, ui);
}

/// Ask before a folder goes to the trash, and say where it goes, as the
/// legacy alert dialog did: its question, what happens, then Cancel and
/// the destructive Move to Trash on the right.
fn trash_dialog(state: &mut NouraUi, ui: &mut Ui) {
    let Some((_, question)) = state.app.pending_trash() else {
        return;
    };
    let question = question.clone();
    let mut confirm = false;
    let mut cancel = false;
    let modal = Modal::new(Id::unique("trash-folder"))
        .frame(common::dialog_frame(ui.ctx()))
        .backdrop_color(common::BACKDROP)
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            common::dialog_title(ui, &question.title);
            common::description(ui, &question.message);
            ui.add_space(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                confirm = Btn::new("Move to Trash")
                    .variant(Variant::Destructive)
                    .show(ui)
                    .clicked();
                cancel = Btn::new("Cancel").show(ui).clicked();
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

/// The resizable sidebar: the workspace switcher and the Files section.
fn sidebar(state: &mut NouraUi, ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    let width = state.app.layout().sidebar_width;
    let panel = Panel::left("sidebar")
        .resizable(true)
        .default_size(width)
        .size_range(SIDEBAR_MIN_WIDTH..=SIDEBAR_MAX_WIDTH)
        .frame(Frame::new().fill(palette.sidebar))
        .show(ui, |ui| {
            // The header: the switcher in the title bar row, over a line.
            let header = egui::Rect::from_min_size(
                ui.max_rect().min,
                vec2(ui.max_rect().width(), common::TITLE_BAR_HEIGHT),
            );
            common::title_bar_area(ui, header, "sidebar");
            ui.painter().hline(
                header.x_range(),
                header.bottom() - 0.5,
                egui::Stroke::new(1.0, palette.sidebar_border),
            );
            let switcher_rect =
                egui::Rect::from_min_max(header.min + vec2(24.0, 4.0), header.max - vec2(8.0, 4.0));
            let mut header_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(switcher_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            switcher(state, &mut header_ui);
            ui.add_space(common::TITLE_BAR_HEIGHT);
            Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| {
                plugin_section(state, ui);
                tree::show(ui, &mut state.app, &mut state.tree);
            });
        });
    // Save the width once a resize ends.
    let shown = panel.response.rect.width().round();
    if shown != width && !ui.input(|input| input.pointer.any_down()) {
        state.app.set_sidebar_width(shown);
    }
}

/// The section the plugin page in the active tab adds to the sidebar,
/// above the file tree, while the page shows. It scrolls on its own once
/// it is taller than half the sidebar.
fn plugin_section(state: &mut NouraUi, ui: &mut Ui) {
    let Some(page_id) = state.app.active_tab().and_then(|tab| match tab.content() {
        TabContent::Page { page_id, .. } if state.app.is_plugin_page(page_id) => {
            Some(page_id.clone())
        }
        _ => None,
    }) else {
        return;
    };
    let drawn = ScrollArea::vertical()
        .id_salt(("plugin-sidebar", &page_id))
        .max_height(ui.available_height() / 2.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            crate::plugin_view::show_sidebar(ui, &mut state.app, state.window, &page_id)
        })
        .inner;
    if drawn {
        ui.separator();
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
    let reveal_label = state.app.platform().reveal_label();
    let label = format!("Switch workspace. Current workspace: {name}");
    let trigger = Btn::new(&name)
        .icon(crate::icons::FOLDER_OPEN)
        .variant(Variant::Ghost)
        .size(Size::Sm)
        .width(ui.available_width())
        .show(ui);
    ui.ctx().accesskit_node_builder(trigger.id, |node| {
        node.set_label(label);
        node.set_has_popup(egui::accesskit::HasPopup::Menu);
    });
    egui::Popup::menu(&trigger)
        .width(320.0)
        .frame(common::menu_frame(ui.ctx()))
        .align(egui::RectAlign::BOTTOM_START)
        .gap(4.0)
        .show(|ui| {
            ui.set_width(312.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            common::menu_label(ui, "Workspaces");
            for recent in &recents {
                let is_current = recent.workspace_id == current;
                let item = common::menu_item(
                    ui,
                    Some(crate::icons::FOLDER_OPEN),
                    &recent.name,
                    Some(&recent.path),
                    is_current,
                    !is_current && !busy,
                );
                let path = recent.path.clone();
                ui.ctx()
                    .accesskit_node_builder(item.id, |node| node.set_description(path));
                if item.on_hover_text(&recent.path).clicked() {
                    open = Some(recent.workspace_id.clone());
                    ui.close();
                }
            }
            common::menu_separator(ui);
            if common::menu_item(
                ui,
                Some(crate::icons::FOLDER_OPEN),
                "Open folder\u{2026}",
                None,
                false,
                !busy,
            )
            .clicked()
            {
                pick = true;
                ui.close();
            }
            if common::menu_item(
                ui,
                Some(crate::icons::FOLDER_PLUS),
                "New workspace\u{2026}",
                None,
                false,
                !busy,
            )
            .clicked()
            {
                create = true;
                ui.close();
            }
            if common::menu_item(
                ui,
                Some(crate::icons::REVEAL),
                reveal_label,
                None,
                false,
                true,
            )
            .clicked()
            {
                reveal = true;
                ui.close();
            }
            let others: Vec<_> = recents
                .iter()
                .filter(|recent| recent.workspace_id != current)
                .collect();
            if !others.is_empty() {
                let button = Button::new("Remove from recents");
                egui::containers::menu::SubMenuButton::from_button(button).ui(ui, |ui| {
                    for recent in others {
                        if common::menu_item(ui, None, &recent.name, None, false, !busy).clicked() {
                            remove = Some(recent.workspace_id.clone());
                            ui.close();
                        }
                    }
                });
            }
        });
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
}

/// What a tab shows that no viewer draws: a file's name and path, the
/// Inbox, or a page's title and its plugin's view.
fn page_content(state: &mut NouraUi, ui: &mut Ui, content: &TabContent, is_inbox: bool) {
    match content {
        TabContent::File { file, .. } => {
            ui.label(RichText::new(&file.name).font(crate::theme::font(
                ui.ctx(),
                TITLE_SIZE,
                crate::theme::Weight::Medium,
            )));
            muted(ui, &file.relative_path);
        }
        TabContent::Page { .. } if is_inbox => inbox::show(ui, &mut state.app, state.window),
        TabContent::Page { title, page_id } => {
            // A plugin page that draws its own heading gets no other.
            let plugin = state.app.is_plugin_page(page_id);
            if !(plugin && crate::plugin_view::has_page_heading(&state.app, page_id)) {
                ui.label(RichText::new(title).font(crate::theme::font(
                    ui.ctx(),
                    TITLE_SIZE,
                    crate::theme::Weight::Medium,
                )));
                ui.add_space(8.0);
            }
            if plugin {
                crate::plugin_view::show(ui, &mut state.app, state.window, page_id);
            }
        }
    }
}

/// What the active tab shows: a PDF in the PDF viewer, an image in the
/// image viewer, a Markdown, text or code file in the note editor, any
/// other file's name and workspace-relative path, the Inbox, or another
/// page's title, which the page's own view replaces. Each tab keeps its
/// scroll position; the viewers and a note's editor keep their own.
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
            ui.add_space((ui.available_height() / 2.0 - 120.0).max(0.0));
            common::empty_state(
                ui,
                Some(crate::icons::NEW_NOTE),
                "No file open",
                Some("Pick a file from the sidebar."),
            );
            return;
        };
        if matches!(content, TabContent::File { .. })
            && (crate::pdf_view::show(ui, &mut state.app, &mut state.pdfs, state.window, id)
                || crate::image_view::show(ui, &state.app, &mut state.images, id)
                || crate::note::show(ui, &mut state.app, &mut state.notes, state.window, id))
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
        // A plugin page's header sits in the top 56 px, as the legacy page
        // header did; other pages keep 24 px all round.
        let margin = match &content {
            TabContent::Page { page_id, .. }
                if state.app.is_plugin_page(page_id)
                    && crate::plugin_view::has_page_heading(&state.app, page_id) =>
            {
                Margin {
                    left: 24,
                    right: 24,
                    top: 12,
                    bottom: 24,
                }
            }
            TabContent::Page { .. } => Margin::same(24),
            _ => Margin::ZERO,
        };
        let output = scroll.show(ui, |ui| {
            Frame::new()
                .inner_margin(margin)
                .show(ui, |ui| page_content(state, ui, &content, is_inbox));
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
    let modal = crate::common::dialog(ui.ctx(), Id::unique("new-workspace")).show(ui.ctx(), |ui| {
        ui.set_width(320.0);
        ui.label(RichText::new("New workspace").font(crate::theme::font(
            ui.ctx(),
            TITLE_SIZE,
            crate::theme::Weight::Medium,
        )));
        common::new_workspace_form(state, ui);
    });
    if modal.should_close() {
        state.new_workspace = None;
    }
}
