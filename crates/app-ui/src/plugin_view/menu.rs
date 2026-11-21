//! Menus on list items, table rows and buttons, with egui's popup menus:
//! disabled items show but can't be chosen, separators draw as lines, and
//! the arrow keys move between items. Choosing an item sends its node ID.

use app_core::{PluginEvent, ViewKind, ViewNode};
use egui::{
    Align, Button, Event, Key, Layout, Modifiers, Popup, PopupKind, Pos2, Response, Stroke, Ui,
    accesskit::HasPopup, containers::menu::menu_style,
};

use super::Kit;
use crate::{
    common::{Btn, Size, Variant},
    icons,
    theme::{Palette, SUPPORTING_SIZE, radius},
};

/// What happened to an open menu this frame.
pub(super) enum Outcome {
    Open,
    Closed,
    /// The menu item with this node ID was chosen.
    Chose(String),
}

/// The menu `menu` of the widget behind `response`, below the widget or
/// at `at`. `focus_first` gives its first item the keyboard, as a menu
/// opened from the keyboard does; it clears once the item has it.
pub(super) fn show(
    ui: &mut Ui,
    response: &Response,
    menu: &ViewNode,
    at: Option<Pos2>,
    focus_first: &mut bool,
) -> Outcome {
    let ViewKind::Menu { label } = &menu.kind else {
        return Outcome::Closed;
    };
    let mut open = true;
    let mut chosen = None;
    let mut popup = Popup::from_response(response)
        .id(response.id.with("menu"))
        .kind(PopupKind::Menu)
        .layout(Layout::top_down_justified(Align::Min))
        .style(menu_style)
        .frame(crate::common::menu_frame(ui.ctx()))
        .open_bool(&mut open)
        .accessible_name(label.clone());
    if let Some(at) = at {
        popup = popup.at_position(at);
    }
    popup.show(|ui| {
        ui.set_min_width(180.0);
        let mut first = true;
        for item in &menu.children {
            match &item.kind {
                ViewKind::MenuItem { label, enabled } => {
                    let response = ui
                        .add_enabled(*enabled, Button::new(label))
                        .accessible_name(label);
                    if first && *enabled && *focus_first {
                        response.request_focus();
                        *focus_first = false;
                    }
                    if *enabled {
                        first = false;
                    }
                    if response.clicked() {
                        chosen = Some(item.id.clone());
                    }
                }
                ViewKind::Separator => {
                    ui.separator();
                }
                _ => {}
            }
        }
    });
    // The caller forgets the open menu, which closes the popup. Closing
    // `ui` here would close the nearest closable parent instead, such as
    // the Settings window a plugin's page sits in.
    if let Some(chosen) = chosen {
        return Outcome::Chose(chosen);
    }
    if open { Outcome::Open } else { Outcome::Closed }
}

/// Whether a key that opens a menu from the keyboard, the context-menu key
/// or Shift-F10, is in this frame's input. Takes it out when `take` is set.
pub(super) fn menu_key(ui: &Ui, take: bool) -> bool {
    let is_menu_key = |event: &Event| {
        matches!(
            event,
            Event::Key { key: Key::Menu, pressed: true, modifiers, .. } if !modifiers.any()
        ) || matches!(
            event,
            Event::Key { key: Key::F10, pressed: true, modifiers, .. }
                if modifiers.matches_exact(Modifiers::SHIFT)
        )
    };
    ui.input_mut(|input| {
        let found = input.events.iter().any(is_menu_key);
        if found && take {
            input.events.retain(|event| !is_menu_key(event));
        }
        found
    })
}

/// A menu button's state between frames.
#[derive(Clone, Copy)]
struct Opened {
    focus_first: bool,
}

/// A button. One that holds a menu opens it on a press, the context-menu
/// key or Shift-F10, and sends the chosen item; any other sends its press.
pub(super) fn button(ui: &mut Ui, node: &ViewNode, label: &str, kit: &mut Kit) {
    let look = if matches!(kit.place, super::Place::Sidebar(_)) {
        Look::Sidebar
    } else if kit.quiet {
        Look::Small
    } else {
        Look::Plain
    };
    styled_button(ui, node, label, look, kit);
}

/// How a plugin's button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    /// The legacy outline button.
    Plain,
    /// The one action that stands out, as a page heading's first button.
    Primary,
    /// No fill until hovered, as a page heading's other buttons.
    Ghost,
    /// Muted text, as a section heading's buttons.
    Quiet,
    /// A small outline pill, as a conversation's Resume or a suggested
    /// reply.
    Small,
    /// A row as wide as the sidebar, like the sidebar's own items.
    Sidebar,
}

/// A plugin's button drawn the way `look` says.
pub(super) fn styled_button(ui: &mut Ui, node: &ViewNode, label: &str, look: Look, kit: &mut Kit) {
    // As in the legacy app, a heading's Previous and Next show as arrows,
    // and a button that creates something starts with a plus.
    let step = if label.starts_with("Previous ") {
        Some(icons::CARET_LEFT)
    } else if label.starts_with("Next ") {
        Some(icons::CARET_RIGHT)
    } else {
        None
    };
    let creates = label.starts_with("New ") || label.starts_with("Add ");
    let draw = |ui: &mut Ui| match (look, step) {
        (Look::Ghost, Some(icon)) => Btn::icon_only(icon, label).size(Size::Sm).show(ui),
        (Look::Plain, _) => Btn::new(label).show(ui),
        (Look::Primary, _) => {
            let button = Btn::new(label).variant(Variant::Default).size(Size::Sm);
            if creates {
                button.icon(icons::PLUS).show(ui)
            } else {
                button.show(ui)
            }
        }
        (Look::Ghost, None) => Btn::new(label)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .show(ui),
        (Look::Quiet, _) => quiet_link(ui, label),
        (Look::Small, _) => Btn::new(label).size(Size::Sm).show(ui),
        (Look::Sidebar, _) => {
            let button = Btn::new(label)
                .variant(Variant::Ghost)
                .size(Size::Sm)
                .width(ui.available_width());
            if creates {
                button.icon(icons::PLUS).show(ui)
            } else {
                button.show(ui)
            }
        }
    };
    let menu = node
        .children
        .first()
        .filter(|child| matches!(child.kind, ViewKind::Menu { .. }));
    let Some(menu) = menu else {
        if draw(ui).clicked() {
            kit.events.push(PluginEvent::Pressed(node.id.clone()));
        }
        return;
    };
    let id = kit.id(&node.id);
    let mut opened: Option<Opened> = ui.data(|data| data.get_temp(id));
    let response = draw(ui);
    let from_keys =
        ui.input(|input| input.key_pressed(Key::Enter) || input.key_pressed(Key::Space));
    if response.has_focus() && opened.is_none() && menu_key(ui, true) {
        opened = Some(Opened { focus_first: true });
    } else if response.clicked() {
        opened = match opened {
            Some(_) => None,
            None => Some(Opened {
                focus_first: from_keys,
            }),
        };
    }
    let expanded = opened.is_some();
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_has_popup(HasPopup::Menu);
        node.set_expanded(expanded);
    });
    if let Some(state) = &mut opened {
        match show(ui, &response, menu, None, &mut state.focus_first) {
            Outcome::Open => {}
            Outcome::Closed => {
                opened = None;
                response.request_focus();
            }
            Outcome::Chose(item) => {
                opened = None;
                kit.events.push(PluginEvent::Chosen(item));
                response.request_focus();
            }
        }
    }
    ui.data_mut(|data| match opened {
        Some(state) => {
            data.insert_temp(id, state);
        }
        None => data.remove::<Opened>(id),
    });
}

/// A link-like button in 12 px muted text that turns the foreground color
/// and underlines on hover, as the legacy section links did.
fn quiet_link(ui: &mut Ui, label: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(SUPPORTING_SIZE),
        egui::Color32::PLACEHOLDER,
    );
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, true, label));
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let color = if hovered {
            palette.foreground
        } else {
            palette.muted_foreground
        };
        ui.painter().galley(rect.min, galley, color);
        if hovered {
            ui.painter()
                .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, color));
        }
        if response.has_focus() {
            crate::common::focus_ring(ui.painter(), rect.expand(2.0), radius(0.4), &palette);
        }
    }
    response
}
