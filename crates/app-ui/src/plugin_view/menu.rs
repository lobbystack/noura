//! Menus on list items, table rows and buttons, with egui's popup menus:
//! disabled items show but can't be chosen, separators draw as lines, and
//! the arrow keys move between items. Choosing an item sends its node ID.

use app_core::{PluginEvent, ViewKind, ViewNode};
use egui::{
    Align, Button, Event, Key, Layout, Modifiers, Popup, PopupKind, Pos2, Response, Ui,
    accesskit::HasPopup, containers::menu::menu_style,
};

use super::Kit;

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
    if let Some(chosen) = chosen {
        ui.close();
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
    let menu = node
        .children
        .first()
        .filter(|child| matches!(child.kind, ViewKind::Menu { .. }));
    let Some(menu) = menu else {
        if ui.button(label).clicked() {
            kit.events.push(PluginEvent::Pressed(node.id.clone()));
        }
        return;
    };
    let id = kit.id(&node.id);
    let mut opened: Option<Opened> = ui.data(|data| data.get_temp(id));
    let response = ui.add(Button::new(label));
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
