//! The Calendar page's sidebar section: your calendars, then the built-in
//! ones under Other, each with a checkbox in its color that shows or hides
//! its entries. Selecting one of yours makes it the calendar new events
//! go to. Its menu renames, recolors or deletes it.

use plugin_sdk::{Node, View};

use crate::{
    State,
    calendars::{Key, Own, PALETTE, Settings, built_in, next_color},
};

/// The list of your calendars and the list of built-in ones.
pub(crate) const MINE: &str = "calendars";
pub(crate) const OTHER: &str = "other-calendars";

/// The button that names a new calendar, and its name field.
pub(crate) const NEW: &str = "new-calendar";
pub(crate) const NEW_NAME: &str = "new-calendar-name";

/// The name field of a calendar you rename.
pub(crate) const RENAME: &str = "rename-calendar";

/// What a calendar's menu items start with, before its ID.
pub(crate) const RENAME_ITEM: &str = "rename:";
pub(crate) const DELETE_ITEM: &str = "delete:";
/// A color item is `color:<color>:<ID>`.
pub(crate) const COLOR_ITEM: &str = "color:";

/// The sidebar, with `types` as the object types whose entries the
/// workspace holds, for the built-in calendars.
pub(crate) fn render(
    state: &State,
    own: &[Own],
    types: &[String],
    settings: &Settings,
) -> Vec<Node> {
    let mut view = View::panel("calendar-sidebar", Some("Calendars"));
    let mine = view.list(View::ROOT, MINE, "Calendars");
    for calendar in own {
        let key = Key::Own(calendar.id.clone());
        let node = key.node();
        let item = view.checkbox_item(mine, &node, &calendar.name, None, !settings.hides(&key));
        view.color(item, calendar.color);
        if state.renaming.as_deref() == Some(calendar.id.as_str()) {
            view.name_field(item, RENAME, "Calendar name", &calendar.name);
        }
        let menu = view.menu(item, &format!("menu:{}", calendar.id), "Calendar");
        view.menu_item(
            menu,
            &format!("{RENAME_ITEM}{}", calendar.id),
            "Rename",
            true,
        );
        view.separator(menu, &format!("colors:{}", calendar.id));
        for (color, id, label) in PALETTE {
            view.menu_item(
                menu,
                &format!("{COLOR_ITEM}{id}:{}", calendar.id),
                label,
                color != calendar.color,
            );
        }
        view.separator(menu, &format!("end:{}", calendar.id));
        view.menu_item(
            menu,
            &format!("{DELETE_ITEM}{}", calendar.id),
            "Delete",
            true,
        );
    }
    if state.naming_calendar {
        let item = view.item(mine, "new-calendar-item", "New calendar", None);
        view.color(item, next_color(own));
        view.name_field(item, NEW_NAME, "Calendar name", "");
    }
    if let Some(chosen) = state
        .calendar
        .as_ref()
        .filter(|id| own.iter().any(|calendar| calendar.id == **id))
    {
        view.select(mine, &Key::Own(chosen.clone()).node());
    }
    view.button(View::ROOT, NEW, "New calendar");
    if let Some(notice) = &state.sidebar_notice {
        view.text(View::ROOT, "calendar-notice", notice);
    }

    let mut built: Vec<(String, Key, plugin_sdk::Color)> = types
        .iter()
        .map(|object_type| {
            let (name, color) = built_in(object_type);
            (name, Key::Type(object_type.clone()), color)
        })
        .collect();
    built.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    if !built.is_empty() {
        let other = view.list(View::ROOT, OTHER, "Other");
        for (name, key, color) in built {
            let item = view.checkbox_item(other, &key.node(), &name, None, !settings.hides(&key));
            view.color(item, color);
        }
    }
    view.into_nodes()
}
