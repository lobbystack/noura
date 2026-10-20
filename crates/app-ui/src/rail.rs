//! The rail: a narrow column of pages, such as Files and the Inbox. A
//! click shows a page, and you reorder pages by dragging them or with
//! Alt-Up and Alt-Down. `app-core` owns the pages and their order.

use app_core::{App, FILES_PAGE, INBOX_PAGE, OpenAs};
use egui::{
    Align2, Color32, FontId, Id, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder,
    WidgetInfo,
    accesskit::{Action, AriaCurrent, Role},
    pos2, vec2,
};

use crate::theme::{Palette, radius};

/// The rail's width.
pub(crate) const RAIL_WIDTH: f32 = 56.0;
const ITEM_SIZE: f32 = 36.0;
const ITEM_GAP: f32 = 4.0;

/// A page being dragged to a new place.
struct Drag {
    page_id: String,
    from: usize,
    /// Escape cancelled the drag; the release changes nothing.
    cancelled: bool,
}

/// What the rail remembers between frames.
#[derive(Default)]
pub(crate) struct RailView {
    drag: Option<Drag>,
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

pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut RailView) {
    let pages: Vec<(String, String)> = app
        .rail()
        .iter()
        .map(|page| (page.id.clone(), page.title.clone()))
        .collect();
    let current = app.current_page().to_owned();
    let palette = Palette::of(ui.ctx());
    let mut open = None;
    let mut moved = None;
    let mut released = None;
    let mut rects = Vec::with_capacity(pages.len());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Navigation)
        .accessibility_label("Pages");
    ui.scope_builder(builder, |ui| {
        ui.add_space(12.0);
        for (index, (page_id, title)) in pages.iter().enumerate() {
            let (_, slot) = ui.allocate_space(vec2(ui.available_width(), ITEM_SIZE));
            ui.add_space(ITEM_GAP);
            let rect = Rect::from_center_size(slot.center(), vec2(ITEM_SIZE, ITEM_SIZE));
            rects.push(rect);
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
            if response.drag_started() {
                view.drag = Some(Drag {
                    page_id: page_id.clone(),
                    from: index,
                    cancelled: false,
                });
            }
            let dragging = view
                .drag
                .as_ref()
                .is_some_and(|drag| drag.page_id == *page_id);
            if dragging && response.drag_stopped() {
                released = Some(
                    response
                        .interact_pointer_pos()
                        .map_or(rect.center().y, |pointer| pointer.y),
                );
            }

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
            }
        }
    });

    if let Some(y) = released
        && let Some(drag) = view.drag.take()
        && !drag.cancelled
    {
        let gap = gap_at(&rects, y);
        // The gap counts the dragged page; its final index doesn't.
        let index = if gap > drag.from { gap - 1 } else { gap };
        moved = Some((drag.page_id, Move::To(index)));
    }
    if let Some(drag) = &mut view.drag {
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            drag.cancelled = true;
        }
        if !drag.cancelled
            && let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
        {
            let gap = gap_at(&rects, pointer.y);
            let y = match rects.get(gap) {
                Some(rect) => rect.top() - ITEM_GAP / 2.0,
                None => rects
                    .last()
                    .map_or(0.0, |rect| rect.bottom() + ITEM_GAP / 2.0),
            };
            if let Some(first) = rects.first() {
                ui.painter().line_segment(
                    [pos2(first.left(), y), pos2(first.right(), y)],
                    Stroke::new(2.0, palette.primary),
                );
            }
        }
        if !ui.input(|input| input.pointer.any_down()) {
            view.drag = None;
        }
    }

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

/// The gap between pages that `y` points at: 0 above the first page, and
/// one more for each page whose middle is above `y`.
fn gap_at(rects: &[Rect], y: f32) -> usize {
    rects.iter().filter(|rect| rect.center().y < y).count()
}
