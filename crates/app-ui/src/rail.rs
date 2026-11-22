//! The rail: a narrow column of pages, such as Files and the Inbox, with
//! the Search and Settings buttons at the bottom. A click shows a page,
//! and you reorder pages by dragging them or with Alt-Up and Alt-Down.
//! `app-core` owns the pages and their order.

use std::time::Instant;

use app_core::{App, CommandSource, FILES_PAGE, INBOX_PAGE, OpenAs, Platform, command_id};
use egui::{
    Align, Align2, Color32, DragAndDrop, FontId, Id, Key, Layout, Modifiers, Rect, Sense, Stroke,
    Ui, UiBuilder, WidgetInfo,
    accesskit::{Action, AriaCurrent, HasPopup, Role},
    pos2, vec2,
};

use crate::{
    icons, keys,
    theme::{Palette, radius},
};

/// The rail's width on Windows and Linux.
const RAIL_WIDTH: f32 = 56.0;
/// The rail's width on macOS, where the window's close, minimize and zoom
/// buttons sit in its top row. It stays this many points of the screen
/// wide as you zoom out, since the buttons don't zoom.
const MAC_RAIL_WIDTH: f32 = 62.0;
/// The space between two of the window's buttons in the rail, in points
/// of the screen: less than AppKit's 9, so all three fit.
const WINDOW_BUTTON_GAP: f32 = 4.0;
const ITEM_SIZE: f32 = 36.0;
const ITEM_GAP: f32 = 4.0;
/// The side of a rail icon.
const ICON_SIZE: f32 = 20.0;

/// The rail's width on `platform` at the zoom factor `zoom`.
pub(crate) fn width(platform: Platform, zoom: f32) -> f32 {
    match platform {
        Platform::MacOs => MAC_RAIL_WIDTH.max(MAC_RAIL_WIDTH / zoom),
        Platform::Windows | Platform::Linux => RAIL_WIDTH,
    }
}

/// Where the window's close, minimize and zoom buttons go on macOS, in
/// points of the screen from the window's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowButtons {
    /// The middle of the three buttons, which is the middle of the rail's
    /// top row.
    pub center: egui::Pos2,
    /// The space between two buttons.
    pub gap: f32,
}

/// Where the window's buttons go on macOS at the zoom factor `zoom`:
/// centered in the rail's top row, closer together than AppKit sets them.
pub fn window_buttons(zoom: f32) -> WindowButtons {
    let width = width(Platform::MacOs, zoom) * zoom;
    WindowButtons {
        center: pos2(width / 2.0, crate::common::TITLE_BAR_HEIGHT * zoom / 2.0),
        gap: WINDOW_BUTTON_GAP,
    }
}

/// The Settings button's ID, so focus can return to it when Settings
/// closes.
pub(crate) fn settings_button_id() -> Id {
    Id::unique("rail-settings")
}

/// The page you're dragging to a new place, as egui's drag-and-drop
/// payload. egui drops the payload when you press Escape.
struct Dragged {
    page_id: String,
    from: usize,
}

fn item_id(page_id: &str) -> Id {
    Id::unique(("rail-page", page_id))
}

/// Whether one of the rail's pages has keyboard focus.
pub(crate) fn has_focus(ctx: &egui::Context, app: &App) -> bool {
    ctx.memory(|memory| memory.focused())
        .is_some_and(|focused| app.rail().iter().any(|page| item_id(&page.id) == focused))
}

/// The icon of a page: the kernel's pages and the plugins Noura ships
/// have their own. Another plugin's page shows its title's first letter.
fn page_icon(page_id: &str) -> Option<icons::Icon> {
    let shipped = |page| page_id.ends_with(&format!(":{page}"));
    Some(match page_id {
        FILES_PAGE => icons::FILES,
        INBOX_PAGE => icons::INBOX,
        _ if shipped("tasks") => icons::TASKS,
        _ if shipped("calendar") => icons::CALENDAR,
        _ if shipped("projects") => icons::PROJECTS,
        _ if shipped("ai") => icons::AI,
        _ => return None,
    })
}

/// Draw a page's symbol in `rect`: its icon, filled while it shows, or
/// else the first letter of its title.
fn paint_symbol(
    painter: &egui::Painter,
    page_id: &str,
    title: &str,
    rect: Rect,
    color: Color32,
    current: bool,
) {
    if let Some(icon) = page_icon(page_id) {
        let square = Rect::from_center_size(rect.center(), vec2(ICON_SIZE, ICON_SIZE));
        icons::paint(painter, icon, square, color, current);
        return;
    }
    let letter: String = title
        .chars()
        .next()
        .map(|letter| letter.to_uppercase().collect())
        .unwrap_or_default();
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId::proportional(16.0),
        color,
    );
}

pub(crate) fn show(ui: &mut Ui, app: &mut App) {
    let pages: Vec<(String, String)> = app
        .rail()
        .iter()
        .map(|page| (page.id.clone(), page.title.clone()))
        .collect();
    let current = app.current_page().to_owned();
    let palette = Palette::of(ui.ctx());
    let mut open = None;
    let mut moved = None;
    let dragged = DragAndDrop::payload::<Dragged>(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Navigation)
        .accessibility_label("Pages");
    // The title bar row runs across the rail's top. On macOS the window's
    // buttons sit in it.
    let title = Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), crate::common::TITLE_BAR_HEIGHT),
    );
    crate::common::title_bar_area(ui, title, "rail");
    ui.painter().hline(
        title.x_range(),
        title.bottom() - 0.5,
        Stroke::new(1.0, palette.sidebar_border),
    );
    ui.add_space(crate::common::TITLE_BAR_HEIGHT);
    ui.scope_builder(builder, |ui| {
        ui.add_space(12.0);
        for (index, (page_id, title)) in pages.iter().enumerate() {
            let (_, slot) = ui.allocate_space(vec2(ui.available_width(), ITEM_SIZE));
            ui.add_space(ITEM_GAP);
            let rect = Rect::from_center_size(slot.center(), vec2(ITEM_SIZE, ITEM_SIZE));
            let id = item_id(page_id);
            let response = ui.interact(rect, id, Sense::click_and_drag());
            response.widget_info(|| WidgetInfo::labeled(Role::Link, true, title));
            let is_current = *page_id == current;
            ui.ctx().accesskit_node_builder(id, |node| {
                node.add_action(Action::Focus);
                if is_current {
                    node.set_aria_current(AriaCurrent::Page);
                }
            });
            let response = response.on_hover_text(title);

            // A page opens in a new tab with Cmd-click (Ctrl-click on
            // Windows and Linux), a middle-click or its menu. Files shows
            // a document tab and has no tab of its own.
            let new_tab = *page_id != FILES_PAGE;
            if response.clicked() {
                let command = ui.input(|input| input.modifiers.command);
                let how = if command && new_tab {
                    OpenAs::Kept
                } else {
                    OpenAs::Preview
                };
                open = Some((page_id.clone(), how));
            } else if response.middle_clicked() && new_tab {
                open = Some((page_id.clone(), OpenAs::Kept));
            }
            if new_tab {
                egui::Popup::context_menu(&response)
                    .style(crate::theme::menu_style)
                    .show(|ui| {
                        if ui.button("Open in new tab").clicked() {
                            open = Some((page_id.clone(), OpenAs::Kept));
                            ui.close();
                        }
                    });
            }
            if response.has_focus() {
                let step = ui.input_mut(|input| {
                    if input.consume_key(Modifiers::ALT, Key::ArrowUp) {
                        Some(-1)
                    } else if input.consume_key(Modifiers::ALT, Key::ArrowDown) {
                        Some(1)
                    } else {
                        None
                    }
                });
                if let Some(step) = step {
                    moved = Some((page_id.clone(), Move::By(step)));
                }
            }
            response.dnd_set_drag_payload(Dragged {
                page_id: page_id.clone(),
                from: index,
            });
            // Dropping on a page's top half puts the dragged page before
            // it, and on its bottom half after it.
            let gap = |pointer: Option<egui::Pos2>| {
                let below = pointer.is_some_and(|pointer| pointer.y > rect.center().y);
                index + usize::from(below)
            };
            let hovering = response
                .dnd_hover_payload::<Dragged>()
                .map(|_| gap(response.hover_pos()));
            if let Some(dropped) = response.dnd_release_payload::<Dragged>() {
                let gap = gap(response.hover_pos());
                // The gap counts the dragged page; its new index doesn't.
                let to = if gap > dropped.from { gap - 1 } else { gap };
                moved = Some((dropped.page_id.clone(), Move::To(to)));
            }
            let dragging = dragged
                .as_ref()
                .is_some_and(|dragged| dragged.page_id == *page_id);

            if ui.is_rect_visible(rect) {
                let painter = ui.painter();
                if is_current || response.hovered() {
                    painter.rect_filled(rect, radius(1.4), palette.sidebar_accent);
                }
                if response.has_focus() {
                    crate::common::focus_ring(painter, rect, radius(1.4), &palette);
                }
                let mut color = if is_current || response.hovered() {
                    palette.foreground
                } else {
                    palette.foreground.gamma_multiply(0.7)
                };
                if dragging {
                    color = color.gamma_multiply(0.4);
                }
                paint_symbol(painter, page_id, title, rect, color, is_current);
                if let Some(gap) = hovering {
                    let y = if gap == index {
                        rect.top() - ITEM_GAP / 2.0
                    } else {
                        rect.bottom() + ITEM_GAP / 2.0
                    };
                    painter.line_segment(
                        [pos2(rect.left(), y), pos2(rect.right(), y)],
                        Stroke::new(2.0, palette.primary),
                    );
                }
            }
        }
    });

    ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
        ui.add_space(12.0);
        if settings_button(ui, app).clicked() {
            app.open_settings();
        }
        ui.add_space(ITEM_GAP);
        if search_button(ui, app).clicked() {
            app.run_command(
                command_id::COMMAND_PALETTE,
                CommandSource::Button,
                Instant::now(),
            );
        }
    });

    if let Some((page_id, how)) = open {
        app.open_page(&page_id, how);
    }
    match moved {
        Some((page_id, Move::By(step))) => app.move_page_by(&page_id, step),
        Some((page_id, Move::To(index))) => app.move_page(&page_id, index),
        None => {}
    }
}

enum Move {
    By(isize),
    To(usize),
}

/// The shortcut of the command `id`, as the platform writes it.
fn shortcut_text(app: &App, id: &str) -> Option<String> {
    app.commands()
        .into_iter()
        .find(|command| command.id == id)
        .and_then(|command| command.shortcuts.first().copied())
        .map(|shortcut| keys::shortcut_text(&shortcut, app.platform()))
}

/// A button at the foot of the rail that opens a dialog. Its tooltip
/// shows the shortcut of the command `command`.
fn foot_button(
    ui: &mut Ui,
    app: &App,
    id: Id,
    label: &str,
    command: &str,
    icon: icons::Icon,
) -> egui::Response {
    let shortcut = shortcut_text(app, command);
    let rect = ui.allocate_space(vec2(ITEM_SIZE, ITEM_SIZE)).1;
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_has_popup(HasPopup::Dialog);
        if let Some(shortcut) = &shortcut {
            node.set_keyboard_shortcut(shortcut.clone());
        }
    });
    let palette = Palette::of(ui.ctx());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, radius(1.4), palette.sidebar_accent);
        }
        if response.has_focus() {
            crate::common::focus_ring(painter, rect, radius(1.4), &palette);
        }
        let color = if response.hovered() {
            palette.foreground
        } else {
            palette.foreground.gamma_multiply(0.7)
        };
        let square = Rect::from_center_size(rect.center(), vec2(ICON_SIZE, ICON_SIZE));
        icons::paint(painter, icon, square, color, false);
    }
    let tooltip = match &shortcut {
        Some(shortcut) => format!("{label} ({shortcut})"),
        None => label.to_owned(),
    };
    response.on_hover_text(tooltip)
}

/// The Search button, which opens the command palette.
fn search_button(ui: &mut Ui, app: &App) -> egui::Response {
    foot_button(
        ui,
        app,
        Id::unique("rail-search"),
        "Search",
        command_id::COMMAND_PALETTE,
        icons::SEARCH,
    )
}

/// The Settings button: a gear.
fn settings_button(ui: &mut Ui, app: &App) -> egui::Response {
    foot_button(
        ui,
        app,
        settings_button_id(),
        "Settings",
        command_id::SETTINGS,
        icons::SETTINGS,
    )
}
