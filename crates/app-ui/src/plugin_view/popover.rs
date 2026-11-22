//! Popovers: a box that floats over the page beside the node it points
//! at, such as a calendar entry, with an arrow toward it. It sits on the
//! anchor's right when there is room, then on its left, under it or over
//! it, and stays inside the page. While the anchor doesn't show, it floats
//! at the top of the page with no arrow.
//!
//! Escape or a press outside it sends `cancelled` naming it, ahead of what
//! the press does, so a click on another entry closes this popover before
//! it selects that entry. A press on the anchor keeps it open. Escape that
//! puts back an editable text's value closes nothing, and neither Escape
//! nor a press does while a picker or a menu inside the popover is open.
//! A press outside first sends the text you typed in an editable text, as
//! leaving the text would.
//!
//! A heading in a popover is its title, at the size of an editable
//! title. AccessKit sees a dialog named by the popover's label.

use app_core::{PluginEvent, ViewKind, ViewNode};
use egui::{
    Area, Color32, Frame, Key, LayerId, Margin, Order, Pos2, Rect, Shape, Stroke, Ui, UiBuilder,
    Vec2, accesskit::Role, pos2, vec2,
};

use super::{Kit, children, edit};
use crate::theme::{Palette, radius};

/// The popover's width.
const WIDTH: f32 = 300.0;
/// How far the arrow reaches from the popover's edge, and half its base.
const ARROW: f32 = 8.0;
const ARROW_HALF: f32 = 9.0;
/// The space between the arrow's tip and the anchor.
const GAP: f32 = 2.0;
/// The least space between the popover and the page's edges.
const EDGE: f32 = 8.0;

/// Every popover in `node`'s tree, in view order.
pub(super) fn find<'a>(node: &'a ViewNode, found: &mut Vec<&'a ViewNode>) {
    if matches!(node.kind, ViewKind::Popover { .. }) {
        found.push(node);
    }
    for child in &node.children {
        find(child, found);
    }
}

/// The side of the anchor a popover sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Right,
    Left,
    Below,
    Above,
}

/// Where a popover of `size` goes beside `anchor` within `bounds`: its
/// top left corner, and the side it sits on with where its arrow points,
/// or none when the anchor doesn't show.
fn place(anchor: Option<Rect>, size: Vec2, bounds: Rect) -> (Pos2, Option<(Side, Pos2)>) {
    let inner = bounds.shrink(EDGE);
    let clamp = |value: f32, low: f32, high: f32| value.min(high).max(low);
    let Some(anchor) = anchor.filter(|anchor| anchor.intersects(bounds)) else {
        return (
            pos2(inner.center().x - size.x / 2.0, inner.top()).max(inner.min),
            None,
        );
    };
    let reach = ARROW + GAP;
    let fits = [
        (
            Side::Right,
            anchor.right() + reach + size.x <= inner.right(),
        ),
        (Side::Left, anchor.left() - reach - size.x >= inner.left()),
        (
            Side::Below,
            anchor.bottom() + reach + size.y <= inner.bottom(),
        ),
        (Side::Above, anchor.top() - reach - size.y >= inner.top()),
    ];
    let side = fits
        .iter()
        .find(|(_, fits)| *fits)
        .map_or(Side::Right, |(side, _)| *side);
    let top = clamp(
        anchor.center().y - size.y / 2.0,
        inner.top(),
        (inner.bottom() - size.y).max(inner.top()),
    );
    let left = clamp(
        anchor.center().x - size.x / 2.0,
        inner.left(),
        (inner.right() - size.x).max(inner.left()),
    );
    let corner = match side {
        Side::Right => pos2(anchor.right() + reach, top),
        Side::Left => pos2(anchor.left() - reach - size.x, top),
        Side::Below => pos2(left, anchor.bottom() + reach),
        Side::Above => pos2(left, anchor.top() - reach - size.y),
    };
    // The arrow meets the popover's edge across from the anchor's middle,
    // clear of the rounded corners.
    let margin = ARROW_HALF + 8.0;
    let tip = match side {
        Side::Right | Side::Left => {
            let y = clamp(
                anchor.center().y,
                corner.y + margin,
                corner.y + size.y - margin,
            );
            let x = if side == Side::Right {
                corner.x - ARROW
            } else {
                corner.x + size.x + ARROW
            };
            pos2(x, y)
        }
        Side::Below | Side::Above => {
            let x = clamp(
                anchor.center().x,
                corner.x + margin,
                corner.x + size.x - margin,
            );
            let y = if side == Side::Below {
                corner.y - ARROW
            } else {
                corner.y + size.y + ARROW
            };
            pos2(x, y)
        }
    };
    (corner, Some((side, tip)))
}

/// The arrow's three corners: its tip, then the two ends of its base on
/// the popover's edge, a point inside it so the base hides the border.
fn arrow(side: Side, tip: Pos2) -> [Pos2; 3] {
    let (along, out) = match side {
        Side::Right => (vec2(0.0, 1.0), vec2(1.0, 0.0)),
        Side::Left => (vec2(0.0, 1.0), vec2(-1.0, 0.0)),
        Side::Below => (vec2(1.0, 0.0), vec2(0.0, 1.0)),
        Side::Above => (vec2(1.0, 0.0), vec2(0.0, -1.0)),
    };
    let base = tip + out * (ARROW + 1.0);
    [
        tip,
        base - along * (ARROW_HALF + 1.0),
        base + along * (ARROW_HALF + 1.0),
    ]
}

pub(super) fn popover(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Popover { label, anchor } = &node.kind else {
        return;
    };
    let ctx = ui.ctx().clone();
    let palette = Palette::of(&ctx);
    let dark = ctx.theme() == egui::Theme::Dark;
    let id = kit.id(&node.id);
    let anchor_rect = kit
        .anchors
        .iter()
        .find(|(id, _)| id == anchor)
        .and_then(|(_, rect)| *rect);
    let size = ctx
        .memory(|memory| memory.area_rect(id))
        .map_or(vec2(WIDTH, 240.0), |rect| rect.size());
    let (corner, pointer) = place(anchor_rect, size, ui.clip_rect());
    let border = palette
        .foreground
        .gamma_multiply(if dark { 0.16 } else { 0.1 });

    kit.popup_open = false;
    kit.escape_taken = false;
    let depth = std::mem::replace(&mut kit.depth, 1);
    kit.in_popover = true;
    let shown = Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(corner)
        .constrain(false)
        .show(&ctx, |ui| {
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label(label.as_str());
            ui.scope_builder(builder, |ui| {
                Frame::new()
                    .fill(palette.popover)
                    .stroke(Stroke::new(1.0, border))
                    .corner_radius(radius(1.2))
                    .inner_margin(Margin::same(14))
                    .shadow(egui::Shadow {
                        offset: [0, 12],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(if dark { 90 } else { 40 }),
                    })
                    .show(ui, |ui| {
                        ui.set_width(WIDTH - 2.0 * 14.0 - 2.0);
                        ui.spacing_mut().item_spacing.y = 10.0;
                        ui.vertical(|ui| children(ui, &node.children, kit));
                    });
            });
        });
    kit.depth = depth;
    kit.in_popover = false;
    let rect = shown.response.rect;

    if let Some((side, tip)) = pointer {
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, id));
        let [tip, start, end] = arrow(side, tip);
        painter.add(Shape::convex_polygon(
            vec![tip, start, end],
            palette.popover,
            Stroke::NONE,
        ));
        let stroke = Stroke::new(1.0, border);
        painter.line_segment([start, tip], stroke);
        painter.line_segment([tip, end], stroke);
    }

    // Leave the popover open while something inside it, such as a
    // picker's menu, takes the press or the key first.
    let inner_popup = kit.popup_open;
    let escaped =
        !inner_popup && !kit.escape_taken && ctx.input(|input| input.key_pressed(Key::Escape));
    let pressed = ctx.input(|input| {
        input
            .pointer
            .any_pressed()
            .then(|| input.pointer.press_origin())
            .flatten()
    });
    let outside = !inner_popup
        && pressed.is_some_and(|at| {
            !rect.contains(at) && !anchor_rect.is_some_and(|anchor| anchor.contains(at))
        });
    if escaped || outside {
        let mut closing: Vec<PluginEvent> = Vec::new();
        if outside {
            pending(ui, &node.children, kit, &mut closing);
        }
        closing.push(PluginEvent::Cancelled(node.id.clone()));
        kit.events.splice(0..0, closing);
    }
}

/// The text you typed in an editable text among `nodes` that still has
/// the keyboard, as leaving it would send it.
fn pending(ui: &Ui, nodes: &[ViewNode], kit: &Kit, found: &mut Vec<PluginEvent>) {
    for node in nodes {
        if let Some(event) = edit::pending(ui, node, kit) {
            found.push(event);
        }
        pending(ui, &node.children, kit, found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1200.0, 800.0));
    const SIZE: Vec2 = vec2(300.0, 200.0);

    #[test]
    fn a_popover_sits_on_the_anchors_right_when_there_is_room() {
        let anchor = Rect::from_min_size(pos2(100.0, 300.0), vec2(140.0, 22.0));
        let (corner, pointer) = place(Some(anchor), SIZE, BOUNDS);
        assert_eq!(corner, pos2(250.0, 211.0));
        assert_eq!(pointer, Some((Side::Right, pos2(242.0, 311.0))));
    }

    #[test]
    fn near_the_right_edge_it_moves_to_the_left_and_stays_in_the_page() {
        let anchor = Rect::from_min_size(pos2(1000.0, 10.0), vec2(140.0, 22.0));
        let (corner, pointer) = place(Some(anchor), SIZE, BOUNDS);
        assert_eq!(corner, pos2(1000.0 - 10.0 - 300.0, 8.0));
        let (side, tip) = pointer.unwrap();
        assert_eq!(side, Side::Left);
        // The arrow stays clear of the top corner.
        assert_eq!(tip, pos2(1000.0 - 2.0, 8.0 + 17.0));
    }

    #[test]
    fn in_a_narrow_page_it_goes_under_the_anchor() {
        let bounds = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 800.0));
        let anchor = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 22.0));
        let (corner, pointer) = place(Some(anchor), SIZE, bounds);
        assert_eq!(corner, pos2(50.0, 132.0));
        assert_eq!(pointer, Some((Side::Below, pos2(200.0, 124.0))));
    }

    #[test]
    fn without_its_anchor_it_floats_at_the_top_of_the_page() {
        let (corner, pointer) = place(None, SIZE, BOUNDS);
        assert_eq!(corner, pos2(450.0, 8.0));
        assert_eq!(pointer, None);
        let gone = Rect::from_min_size(pos2(100.0, 2000.0), vec2(140.0, 22.0));
        assert_eq!(place(Some(gone), SIZE, BOUNDS).1, None);
    }
}
