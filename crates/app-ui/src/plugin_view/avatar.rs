//! Avatars, pills and side panels. An avatar is an orb: a circle in a
//! palette color, lighter toward its top left, with a letter or an emoji
//! in it. It shows small before a list item's label and in a pill, and
//! large at the top of a profile. A pill is a rounded button with an
//! avatar and a label that opens and closes something, as the name at the
//! top of a conversation opens its side panel. A side panel runs down the
//! page's right edge, beside the rest of the page.

use app_core::{PluginAvatar, PluginEvent, ViewKind, ViewNode};
use egui::{
    Align2, Color32, Frame, Margin, Mesh, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, WidgetInfo,
    accesskit::Role, epaint::Vertex, pos2, vec2,
};

use super::Kit;
use crate::{
    common::mix,
    theme::{BODY_SIZE, Palette, Weight, font, pill as pill_radius},
};

/// The radius of a list item's or a pill's avatar.
pub(super) const SMALL: f32 = 9.0;

/// The radius of a large avatar.
const LARGE: f32 = 36.0;

/// The room around a side panel's contents.
const SIDE_MARGIN: i8 = 16;

/// Paint `avatar` as a circle of `radius` around `center`.
pub(super) fn paint(ui: &Ui, center: Pos2, radius: f32, avatar: &PluginAvatar) {
    let palette = Palette::of(ui.ctx());
    let base = avatar.color.map_or(palette.muted_foreground, |color| {
        crate::theme::plugin_color(ui.ctx(), color)
    });
    let painter = ui.painter();
    // A fan of triangles from a light spot up and to the left of the
    // center, so the circle reads as a sphere.
    let light = mix(base, Color32::WHITE, 0.45);
    let spot = center + vec2(-radius * 0.35, -radius * 0.4);
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: spot,
        uv: Pos2::ZERO,
        color: light,
    });
    let steps = 48;
    for step in 0..=steps {
        let angle = step as f32 / steps as f32 * std::f32::consts::TAU;
        mesh.vertices.push(Vertex {
            pos: center + radius * vec2(angle.cos(), angle.sin()),
            uv: Pos2::ZERO,
            color: base,
        });
    }
    for step in 0..steps {
        mesh.add_triangle(0, step + 1, step + 2);
    }
    painter.add(mesh);
    let Some(text) = avatar.text.as_deref() else {
        return;
    };
    // Dark text on the light colors, white on the others.
    let ink = if matches!(
        avatar.color,
        Some(app_core::PluginColor::Yellow | app_core::PluginColor::Orange)
    ) {
        Color32::from_black_alpha(200)
    } else {
        Color32::WHITE
    };
    let size = radius * if text.chars().count() > 2 { 0.7 } else { 1.0 };
    let galley = painter.layout_no_wrap(text.to_owned(), font(ui.ctx(), size, Weight::Medium), ink);
    let at = Align2::CENTER_CENTER.anchor_size(center, galley.size());
    painter.galley(at.min, galley, ink);
}

/// A large avatar in the middle of the line, as at the top of a profile.
/// Screen readers skip it: the name under it says who it is.
pub(super) fn large(ui: &mut Ui, avatar: &PluginAvatar) {
    let width = ui.available_width();
    let (_, rect) = ui.allocate_space(vec2(width, LARGE * 2.0 + 8.0));
    if ui.is_rect_visible(rect) {
        paint(ui, rect.center(), LARGE, avatar);
    }
}

/// A pill button: its avatar and label in a rounded box, filled while what
/// it opens shows or you point at it. A press sends `pressed`.
pub(super) fn pill(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Pill {
        label,
        avatar,
        open,
    } = &node.kind
    else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let text_font = font(ui.ctx(), BODY_SIZE, Weight::Medium);
    let galley = ui
        .painter()
        .layout_no_wrap(label.clone(), text_font, palette.foreground);
    let lead = if avatar.is_some() {
        SMALL * 2.0 + 8.0
    } else {
        0.0
    };
    let padding = vec2(if avatar.is_some() { 8.0 } else { 14.0 }, 6.0);
    let size = vec2(
        (lead + galley.size().x + 14.0 + padding.x).min(ui.available_width()),
        SMALL * 2.0 + padding.y * 2.0,
    );
    let id = kit.id(&node.id);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label.as_str()));
    let expanded = *open;
    ui.ctx().accesskit_node_builder(response.id, |builder| {
        builder.set_expanded(expanded);
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if *open || response.hovered() {
            palette.secondary
        } else {
            palette.background
        };
        painter.rect_filled(rect, pill_radius(), fill);
        painter.rect_stroke(
            rect,
            pill_radius(),
            Stroke::new(1.0, palette.border),
            egui::StrokeKind::Inside,
        );
        if response.has_focus() {
            crate::common::focus_ring(painter, rect, pill_radius(), &palette);
        }
        let mut x = rect.left() + padding.x;
        if let Some(avatar) = avatar {
            paint(ui, pos2(x + SMALL, rect.center().y), SMALL, avatar);
            x += lead;
        }
        let at = Align2::LEFT_CENTER.anchor_size(pos2(x, rect.center().y), galley.size());
        ui.painter().galley(at.min, galley, palette.foreground);
    }
    if response.clicked() {
        kit.events.push(PluginEvent::Pressed(node.id.clone()));
    }
}

/// The view's own panel when its last child is a side panel: the side
/// panel down the page's right edge, from its top to its bottom, and the
/// other children in the room to its left.
pub(super) fn page_with_side_panel(
    ui: &mut Ui,
    rest: &[ViewNode],
    side: &ViewNode,
    (label, width): (&str, u32),
    kit: &mut Kit,
) {
    let palette = Palette::of(ui.ctx());
    let available = ui.available_rect_before_wrap();
    // The page scrolls inside its viewport, which the clip rect is; the
    // panel runs past the page's margins to the viewport's edges.
    let viewport = ui.clip_rect();
    let width = (width as f32).min(available.width() / 2.0);
    let panel = Rect::from_min_max(
        pos2(viewport.right() - width, viewport.top()),
        viewport.right_bottom(),
    );
    let main = Rect::from_min_max(
        available.min,
        pos2(
            (panel.left() - 24.0).max(available.left()),
            available.bottom(),
        ),
    );
    let mut main_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(main)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    super::page_children(&mut main_ui, rest, kit);
    // Only the room to the panel's left counts toward the page's size, so
    // the page never scrolls sideways.
    ui.allocate_rect(
        Rect::from_min_size(
            available.min,
            vec2(main.width(), main_ui.min_rect().height()),
        ),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(panel, egui::CornerRadius::ZERO, palette.background);
    ui.painter().vline(
        panel.left(),
        panel.y_range(),
        Stroke::new(1.0, palette.border),
    );
    let inside = Rect::from_min_max(pos2(panel.left() + 1.0, panel.top()), panel.max);
    let mut panel_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(inside)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_clip_rect(inside.intersect(viewport));
    egui::ScrollArea::vertical()
        .id_salt((&side.id, "side-panel"))
        .auto_shrink(false)
        .show(&mut panel_ui, |ui| {
            Frame::new()
                .inner_margin(Margin::same(SIDE_MARGIN))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let builder = UiBuilder::new()
                        .accessibility_role(Role::Complementary)
                        .accessibility_label(label);
                    ui.push_id(&side.id, |ui| {
                        ui.scope_builder(builder, |ui| {
                            ui.spacing_mut().item_spacing.y = 12.0;
                            let side_before = std::mem::replace(&mut kit.side, true);
                            super::children(ui, &side.children, kit);
                            kit.side = side_before;
                        });
                    });
                });
        });
}
