//! The rail: a narrow column of pages, such as Files and the Inbox, with
//! the Settings button at the bottom. A click shows a page, and you
//! reorder pages by dragging them or with Alt-Up and Alt-Down. `app-core`
//! owns the pages and their order.

use app_core::{App, FILES_PAGE, INBOX_PAGE, OpenAs};
use egui::{
    Align, Align2, Color32, DragAndDrop, FontId, Id, Key, Layout, Modifiers, Pos2, Rect, Sense,
    Shape, Stroke, Ui, UiBuilder, WidgetInfo,
    accesskit::{Action, AriaCurrent, HasPopup, Role},
    pos2, vec2,
};

use crate::theme::{Palette, radius};

/// The rail's width.
pub(crate) const RAIL_WIDTH: f32 = 56.0;
const ITEM_SIZE: f32 = 36.0;
const ITEM_GAP: f32 = 4.0;

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

/// Draw a page's symbol in `rect`: an outline icon for the kernel pages,
/// or else the first letter of its title. egui's bundled fonts have no
/// icon glyphs.
fn paint_symbol(painter: &egui::Painter, page_id: &str, title: &str, rect: Rect, color: Color32) {
    let center = rect.center();
    let at = |x: f32, y: f32| center + vec2(x, y);
    let stroke = Stroke::new(1.5, color);
    let outline = |points: Vec<Pos2>| {
        painter.add(Shape::closed_line(points, stroke));
    };
    match page_id {
        // A folder.
        FILES_PAGE => outline(vec![
            at(-8.0, -6.0),
            at(-3.0, -6.0),
            at(-1.0, -4.0),
            at(8.0, -4.0),
            at(8.0, 6.0),
            at(-8.0, 6.0),
        ]),
        // A tray.
        INBOX_PAGE => {
            outline(vec![
                at(-8.0, -6.0),
                at(8.0, -6.0),
                at(8.0, 6.0),
                at(-8.0, 6.0),
            ]);
            painter.add(Shape::line(
                vec![
                    at(-8.0, 1.0),
                    at(-3.0, 1.0),
                    at(-2.0, 3.0),
                    at(2.0, 3.0),
                    at(3.0, 1.0),
                    at(8.0, 1.0),
                ],
                stroke,
            ));
        }
        _ => {
            let letter: String = title
                .chars()
                .next()
                .map(|letter| letter.to_uppercase().collect())
                .unwrap_or_default();
            painter.text(
                center,
                Align2::CENTER_CENTER,
                letter,
                FontId::proportional(16.0),
                color,
            );
        }
    }
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
                response.context_menu(|ui| {
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
                    painter.rect_filled(rect, radius(1.2), palette.sidebar_accent);
                }
                if response.has_focus() {
                    painter.rect_stroke(
                        rect,
                        radius(1.2),
                        Stroke::new(1.0, palette.ring),
                        egui::StrokeKind::Inside,
                    );
                }
                let mut color = if is_current {
                    palette.foreground
                } else {
                    palette.muted_foreground
                };
                if dragging {
                    color = color.gamma_multiply(0.4);
                }
                paint_symbol(painter, page_id, title, rect, color);
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
        if settings_button(ui).clicked() {
            app.open_settings();
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

/// The Settings button: a gear.
fn settings_button(ui: &mut Ui) -> egui::Response {
    let rect = ui.allocate_space(vec2(ITEM_SIZE, ITEM_SIZE)).1;
    let response = ui.interact(rect, settings_button_id(), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Settings"));
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_has_popup(HasPopup::Dialog));
    let palette = Palette::of(ui.ctx());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, radius(1.2), palette.sidebar_accent);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                radius(1.2),
                Stroke::new(1.0, palette.ring),
                egui::StrokeKind::Inside,
            );
        }
        // A gear: a ring with eight teeth.
        let stroke = Stroke::new(1.5, palette.muted_foreground);
        let center = rect.center();
        painter.circle_stroke(center, 3.0, stroke);
        painter.circle_stroke(center, 6.5, stroke);
        for tooth in 0..8 {
            let angle = std::f32::consts::FRAC_PI_4 * tooth as f32;
            let direction = vec2(angle.cos(), angle.sin());
            painter.line_segment([center + direction * 6.5, center + direction * 9.0], stroke);
        }
    }
    response.on_hover_text("Settings")
}
