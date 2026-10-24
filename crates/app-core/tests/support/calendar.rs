//! The Calendar plugin in a running app, installed from the folder its
//! crate ships with the build that `crates/plugin-sdk/build-plugins.sh`
//! makes, and helpers that read its page and its Inbox section.

use app_core::{App, CalendarMode, Field, InputValue, PluginEvent, Target, ViewKind, ViewNode};

use super::plugins::{self, PluginFolder};

/// The Calendar page.
pub const PAGE: &str = "plugin:calendar:calendar";

/// The Calendar plugin's manifest, as its crate ships it.
pub const MANIFEST: &str = include_str!("../../../plugins/calendar/plugin.toml");

/// The Calendar plugin's folder, or `None` when it isn't built.
pub fn folder() -> Option<PluginFolder> {
    plugins::built("calendar_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "calendar_plugin.wasm", &component))
}

/// The Calendar plugin installed and turned on in `app`'s workspace, with
/// its page drawn, or `None` when it isn't built. The folder lasts as long
/// as the guard.
pub fn with_calendar(app: &mut App) -> Option<PluginFolder> {
    let folder = folder()?;
    plugins::install_and_enable(app, folder.path(), "calendar");
    plugins::wait_until(app, "the Calendar page shows", |app| {
        app.plugin_view(PAGE).is_some()
    });
    if let Some(Err(problem)) = app.plugin_view(PAGE) {
        panic!("the Calendar page doesn't draw: {problem}");
    }
    Some(folder)
}

/// The node with `id` under `node`.
pub fn find<'a>(node: &'a ViewNode, id: &str) -> Option<&'a ViewNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, id))
}

/// The page's view, which must draw.
pub fn page(app: &App) -> app_core::View {
    app.plugin_view(PAGE)
        .expect("the Calendar page shows")
        .expect("the Calendar page draws")
}

/// An entry the page hands its calendar grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The entry's node ID.
    pub id: String,
    pub title: String,
    pub start: String,
    pub end: Option<String>,
}

impl Shown {
    /// The stable ID of the entry's object and the property it comes
    /// from, which the entry's node ID starts with.
    pub fn source(&self) -> (String, String) {
        let mut parts = self.id.splitn(3, ':');
        let id = parts.next().unwrap_or_default().to_owned();
        let property = parts.next().unwrap_or_default().to_owned();
        (id, property)
    }
}

/// The grid the page shows: its mode, the day it shows the period of,
/// and its entries in the order the plugin gave them.
pub fn grid(app: &App) -> (CalendarMode, String, Vec<Shown>) {
    let view = page(app);
    let node = find(view.root(), "calendar").expect("the page has a calendar grid");
    let ViewKind::Calendar { mode, date, .. } = &node.kind else {
        panic!("the calendar node isn't a grid: {:?}", node.kind);
    };
    let entries = node
        .children
        .iter()
        .filter_map(|entry| match &entry.kind {
            ViewKind::Entry { title, start, end } => Some(Shown {
                id: entry.id.clone(),
                title: title.clone(),
                start: start.clone(),
                end: end.clone(),
            }),
            _ => None,
        })
        .collect();
    (*mode, date.clone(), entries)
}

/// The titles of the grid's entries, in order.
pub fn titles(app: &App) -> Vec<String> {
    grid(app).2.into_iter().map(|entry| entry.title).collect()
}

/// Send `event` to the page and wait for the plugin to draw.
pub fn send(app: &mut App, event: PluginEvent) {
    app.send_plugin_event(PAGE, &event);
    super::settle(app);
}

/// Press the page's button `node` and wait for the plugin to draw.
pub fn press(app: &mut App, node: &str) {
    app.press_plugin_button(PAGE, node);
    super::settle(app);
}

/// Show the grid in `mode`, as choosing it in the page's View input does.
pub fn show(app: &mut App, mode: &str) {
    send(
        app,
        PluginEvent::Changed(Field {
            node: "mode".to_owned(),
            value: InputValue::Choice(Some(mode.to_owned())),
        }),
    );
}

/// Move the grid until it shows the period of `day`, one press of
/// Previous or Next at a time.
pub fn go_to(app: &mut App, day: &str) {
    for _ in 0..400 {
        let (_, shown, _) = grid(app);
        if shown == day {
            return;
        }
        press(
            app,
            if shown.as_str() < day {
                "next"
            } else {
                "previous"
            },
        );
    }
    panic!("the grid never showed {day}");
}

/// Select the entry `item` in the grid, as a click does.
pub fn select(app: &mut App, item: &str) {
    send(
        app,
        PluginEvent::Selected(Target {
            node: "calendar".to_owned(),
            item: item.to_owned(),
        }),
    );
}

/// The text of each text node under the node `id` of the page, top to
/// bottom, or `None` when the page has no such node.
pub fn texts_under(app: &App, id: &str) -> Option<Vec<String>> {
    let view = page(app);
    let node = find(view.root(), id)?;
    let mut texts = Vec::new();
    collect_texts(node, &mut texts);
    Some(texts)
}

fn collect_texts(node: &ViewNode, texts: &mut Vec<String>) {
    if let ViewKind::Text(text) = &node.kind {
        texts.push(text.clone());
    }
    for child in &node.children {
        collect_texts(child, texts);
    }
}
