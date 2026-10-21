//! Dragging a card or a list item: what the drag carries, the line that
//! shows where a drop would land, and the label that follows the pointer.
//! The drag stays inside the board or list it started in, and a drop only
//! tells the plugin; nothing moves until the plugin draws it.

use std::sync::Arc;

use egui::{
    Align2, DragAndDrop, FontId, Id, LayerId, Order, Pos2, Rect, Response, Stroke, StrokeKind, Ui,
    UiBuilder, accesskit::Role, pos2, vec2,
};

use crate::theme::{BODY_SIZE, Palette, radius};

/// What a drag carries: the board or list it started in, and the card or
/// item, by node ID, with the label it shows.
pub(super) struct Dragged {
    pub owner: Id,
    pub item: String,
    pub label: String,
}

/// Start dragging `item` when `response` starts a drag.
pub(super) fn start(response: &Response, owner: Id, item: &str, label: &str) {
    if response.drag_started() {
        DragAndDrop::set_payload(
            &response.ctx,
            Dragged {
                owner,
                item: item.to_owned(),
                label: label.to_owned(),
            },
        );
    }
}

/// The drag under way that started in the board or list `owner`.
pub(super) fn dragging(ui: &Ui, owner: Id) -> Option<Arc<Dragged>> {
    DragAndDrop::payload::<Dragged>(ui.ctx()).filter(|dragged| dragged.owner == owner)
}

/// Where the pointer is during the drag, or where it let go.
pub(super) fn pointer(ui: &Ui) -> Option<Pos2> {
    ui.input(|input| input.pointer.interact_pos())
}

/// Whether the pointer let go this frame, which drops what it carries.
pub(super) fn released(ui: &Ui) -> bool {
    ui.input(|input| input.pointer.any_released())
}

/// End the drag once its drop has gone to the plugin.
pub(super) fn finish(ui: &Ui) {
    DragAndDrop::clear_payload(ui.ctx());
}

/// The line where a drop would land, `width` wide from `left`, named for
/// screen readers by `label`.
pub(super) fn indicator(ui: &mut Ui, left: Pos2, width: f32, label: &str) {
    let palette = Palette::of(ui.ctx());
    let rect = Rect::from_min_size(left - vec2(0.0, 1.0), vec2(width, 2.0));
    ui.painter().rect_filled(rect, radius(0.2), palette.primary);
    let builder = UiBuilder::new()
        .id_salt("drop-indicator")
        .max_rect(rect)
        .accessibility_role(Role::Status)
        .accessibility_label(label);
    ui.new_child(builder);
}

/// The dragged card's or item's label, following the pointer above
/// everything else.
pub(super) fn follow_pointer(ui: &Ui, label: &str) {
    let Some(at) = pointer(ui) else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let painter = ui
        .ctx()
        .layer_painter(LayerId::new(Order::Tooltip, Id::unique("plugin-drag")));
    let galley = painter.layout_no_wrap(
        label.to_owned(),
        FontId::proportional(BODY_SIZE),
        palette.foreground,
    );
    let size = galley.size() + vec2(20.0, 12.0);
    let rect = Align2::LEFT_TOP.anchor_size(pos2(at.x + 12.0, at.y + 8.0), size);
    painter.rect(
        rect,
        radius(0.6),
        palette.popover,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(10.0, 6.0), galley, palette.foreground);
}
