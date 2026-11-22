//! The window's own minimize, maximize and close buttons, for a window
//! the system draws no frame around, as on Linux. They sit in Noura's
//! title bar row, at the ends the desktop's button layout names, drawn as
//! GNOME's header bars draw theirs: a faint circle that darkens under the
//! pointer and while pressed, around a small symbolic icon.
//!
//! Around them the window keeps what a frame gives: its edges and corners
//! resize it, and the title bar row moves it, maximizes it on a double
//! click and asks for the window menu on a right click (see
//! [`crate::common::title_bar_area`]).

use egui::{
    Color32, CursorIcon, Id, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, ViewportCommand,
    WidgetInfo, accesskit::Role, pos2, vec2,
};

use crate::{common::TITLE_BAR_HEIGHT, theme::Palette};

/// One of the window's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowControl {
    Minimize,
    Maximize,
    Close,
}

/// Which buttons the window shows at the start and at the end of its
/// title bar row, in order, as the desktop's button layout says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowControls {
    pub start: Vec<WindowControl>,
    pub end: Vec<WindowControl>,
}

/// A button's target: libadwaita's 24 px circle and 5 px of padding.
const BUTTON: f32 = 34.0;
/// The circle drawn inside it.
const CIRCLE: f32 = 24.0;
/// The space between two buttons, and between the buttons and the
/// window's edge: libadwaita's 3 px between controls and its header bar's
/// 7 px of padding.
const SPACING: f32 = 3.0;
const EDGE: f32 = 7.0;
/// How far from the window's edges a press resizes it, and how far along
/// an edge from a corner it resizes both ways.
const RESIZE_BAND: f32 = 5.0;
const RESIZE_CORNER: f32 = 16.0;

/// How far `controls` reach in from the window's edge, the space before
/// them included; nothing when there are none.
pub(crate) fn reach(controls: &[WindowControl]) -> f32 {
    if controls.is_empty() {
        return 0.0;
    }
    let count = controls.len() as f32;
    EDGE + count * BUTTON + (count - 1.0) * SPACING
}

/// The space between the buttons and what the title bar row shows beside
/// them.
pub(crate) const GAP: f32 = 6.0;

fn name(control: WindowControl, maximized: bool) -> &'static str {
    match control {
        WindowControl::Minimize => "Minimize",
        WindowControl::Maximize if maximized => "Restore",
        WindowControl::Maximize => "Maximize",
        WindowControl::Close => "Close",
    }
}

/// Draw `controls` in the title bar row of the window `ui` draws, and do
/// what they ask. Call this after everything else in the row, so the
/// buttons sit over the row's empty part, which moves the window.
pub(crate) fn show(ui: &mut Ui, controls: &WindowControls) {
    let window = ui.ctx().viewport_rect();
    let maximized = ui.input(|input| input.viewport().maximized == Some(true));
    let top = window.top() + (TITLE_BAR_HEIGHT - BUTTON) / 2.0;
    let mut x = window.left() + EDGE;
    for control in &controls.start {
        button(ui, *control, pos2(x, top), maximized);
        x += BUTTON + SPACING;
    }
    let mut x = window.right() - reach(&controls.end);
    for control in &controls.end {
        button(ui, *control, pos2(x, top), maximized);
        x += BUTTON + SPACING;
    }
}

fn button(ui: &mut Ui, control: WindowControl, at: Pos2, maximized: bool) {
    let rect = Rect::from_min_size(at, vec2(BUTTON, BUTTON));
    let id = Id::unique(("window-control", control));
    let response = ui.interact(rect, id, Sense::click());
    let label = name(control, maximized);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let response = response.on_hover_text(label);
    if ui.is_rect_visible(rect) {
        let palette = Palette::of(ui.ctx());
        // libadwaita mixes the circle from the text color: 10 %, 15 %
        // under the pointer and 30 % while pressed.
        let strength = if response.is_pointer_button_down_on() {
            0.30
        } else if response.hovered() {
            0.15
        } else {
            0.10
        };
        let painter = ui.painter();
        painter.circle_filled(
            rect.center(),
            CIRCLE / 2.0,
            palette.foreground.gamma_multiply(strength),
        );
        if response.has_focus() {
            crate::common::focus_ring(
                painter,
                Rect::from_center_size(rect.center(), vec2(CIRCLE, CIRCLE)),
                egui::CornerRadius::same((CIRCLE / 2.0) as u8),
                &palette,
            );
        }
        icon(ui, control, rect.center(), maximized, palette.foreground);
    }
    if response.clicked() {
        let command = match control {
            WindowControl::Minimize => ViewportCommand::Minimized(true),
            WindowControl::Maximize => ViewportCommand::Maximized(!maximized),
            WindowControl::Close => ViewportCommand::Close,
        };
        ui.ctx().send_viewport_cmd(command);
    }
}

/// The button's symbolic icon, 8 points across inside its 16-point box,
/// as Adwaita draws `window-*-symbolic`.
fn icon(ui: &Ui, control: WindowControl, center: Pos2, maximized: bool, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let half = 4.0;
    match control {
        WindowControl::Close => {
            let a = half - 0.5;
            painter.line_segment([center + vec2(-a, -a), center + vec2(a, a)], stroke);
            painter.line_segment([center + vec2(-a, a), center + vec2(a, -a)], stroke);
        }
        WindowControl::Minimize => {
            let y = center.y + half - 0.75;
            painter.line_segment([pos2(center.x - half, y), pos2(center.x + half, y)], stroke);
        }
        WindowControl::Maximize if maximized => {
            // Two windows: the one behind shows its top and right edges.
            let front = Rect::from_center_size(center + vec2(-1.0, 1.0), vec2(6.5, 6.5));
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
            let back = front.translate(vec2(2.5, -2.5));
            painter.line_segment([back.left_top(), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom()], stroke);
        }
        WindowControl::Maximize => {
            let square = Rect::from_center_size(center, vec2(2.0 * half, 2.0 * half));
            painter.rect_stroke(square, 1.5, stroke, egui::StrokeKind::Middle);
        }
    }
}

/// Resize the window from its edges and corners, as its frame would:
/// the pointer shows which way near an edge, and a press there starts the
/// system's resize. A maximized or full-screen window doesn't resize.
pub(crate) fn resize_from_edges(ui: &Ui) {
    let (maximized, fullscreen) = ui.input(|input| {
        let viewport = input.viewport();
        (
            viewport.maximized == Some(true),
            viewport.fullscreen == Some(true),
        )
    });
    if maximized || fullscreen {
        return;
    }
    let window = ui.ctx().viewport_rect();
    let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    let Some(direction) = edge(window, pointer) else {
        return;
    };
    ui.ctx().set_cursor_icon(cursor(direction));
    if ui.input(|input| input.pointer.primary_pressed()) {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Which way a press at `pointer` resizes `window`, if it's near an edge.
fn edge(window: Rect, pointer: Pos2) -> Option<ResizeDirection> {
    if !window.contains(pointer) {
        return None;
    }
    let near = |distance: f32, band: f32| distance <= band;
    let left = pointer.x - window.left();
    let right = window.right() - pointer.x;
    let top = pointer.y - window.top();
    let bottom = window.bottom() - pointer.y;
    let west = near(left, RESIZE_BAND)
        || (near(left, RESIZE_CORNER) && near(top.min(bottom), RESIZE_BAND));
    let east = near(right, RESIZE_BAND)
        || (near(right, RESIZE_CORNER) && near(top.min(bottom), RESIZE_BAND));
    let north =
        near(top, RESIZE_BAND) || (near(top, RESIZE_CORNER) && near(left.min(right), RESIZE_BAND));
    let south = near(bottom, RESIZE_BAND)
        || (near(bottom, RESIZE_CORNER) && near(left.min(right), RESIZE_BAND));
    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (.., true, _) => ResizeDirection::West,
        (.., true) => ResizeDirection::East,
        _ => return None,
    })
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_corners_resize_their_way() {
        let window = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        for (pointer, direction) in [
            (pos2(2.0, 300.0), Some(ResizeDirection::West)),
            (pos2(798.0, 300.0), Some(ResizeDirection::East)),
            (pos2(400.0, 1.0), Some(ResizeDirection::North)),
            (pos2(400.0, 599.0), Some(ResizeDirection::South)),
            (pos2(10.0, 2.0), Some(ResizeDirection::NorthWest)),
            (pos2(2.0, 10.0), Some(ResizeDirection::NorthWest)),
            (pos2(795.0, 597.0), Some(ResizeDirection::SouthEast)),
            (pos2(400.0, 300.0), None),
            (pos2(30.0, 30.0), None),
        ] {
            assert_eq!(edge(window, pointer), direction, "{pointer:?}");
        }
    }

    #[test]
    fn the_buttons_reach_in_as_libadwaita_spaces_them() {
        assert_eq!(reach(&[]), 0.0);
        assert_eq!(reach(&[WindowControl::Close]), 7.0 + 34.0);
        assert_eq!(
            reach(&[WindowControl::Minimize, WindowControl::Close]),
            7.0 + 34.0 + 3.0 + 34.0
        );
    }
}
