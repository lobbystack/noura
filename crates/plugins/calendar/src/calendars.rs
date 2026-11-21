//! Calendars: yours, each a file with a name and a color, and the
//! built-in ones, one for each other type whose objects have dates. Every
//! entry belongs to one calendar. Which calendars you hid lives in the
//! settings file, so it syncs with the workspace.

use plugin_sdk::{
    Color, Error,
    objects::{self, Changes, NewObject, Object},
};
use serde_json::{Map, Value, json};

use crate::{
    names,
    types::{CALENDAR, EVENT, SETTINGS},
};

/// Where Noura writes the settings file when there is none.
pub(crate) const SETTINGS_PATH: &str = "Calendars/Calendar settings.md";

/// The palette, in the order menus list it: each color, the name a file
/// holds, and the name a menu shows.
pub(crate) const PALETTE: [(Color, &str, &str); 8] = [
    (Color::Red, "red", "Red"),
    (Color::Orange, "orange", "Orange"),
    (Color::Yellow, "yellow", "Yellow"),
    (Color::Green, "green", "Green"),
    (Color::Blue, "blue", "Blue"),
    (Color::Purple, "purple", "Purple"),
    (Color::Brown, "brown", "Brown"),
    (Color::Gray, "gray", "Gray"),
];

/// The order new calendars take colors in.
const NEW_COLORS: [Color; 8] = [
    Color::Blue,
    Color::Green,
    Color::Purple,
    Color::Orange,
    Color::Red,
    Color::Yellow,
    Color::Brown,
    Color::Gray,
];

/// The color a file names, such as `green`.
pub(crate) fn color_named(name: &str) -> Option<Color> {
    PALETTE
        .iter()
        .find(|(_, id, _)| *id == name)
        .map(|(color, _, _)| *color)
}

/// The name a file holds for `color`.
pub(crate) fn color_id(color: Color) -> &'static str {
    PALETTE
        .iter()
        .find(|(known, _, _)| *known == color)
        .map_or("blue", |(_, id, _)| id)
}

/// One of your calendars, as its file holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Own {
    pub(crate) id: String,
    /// The file's title.
    pub(crate) name: String,
    pub(crate) color: Color,
}

/// The calendar an entry belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    /// One of yours, by stable ID.
    Own(String),
    /// The built-in calendar of an object type.
    Type(String),
}

impl Key {
    /// The node ID of the calendar's item in the sidebar.
    pub(crate) fn node(&self) -> String {
        match self {
            Self::Own(id) => format!("calendar:{id}"),
            Self::Type(object_type) => format!("type:{object_type}"),
        }
    }

    /// The calendar an item's node ID names.
    pub(crate) fn of_node(node: &str) -> Option<Self> {
        if let Some(id) = node.strip_prefix("calendar:") {
            return Some(Self::Own(id.to_owned()));
        }
        node.strip_prefix("type:")
            .map(|object_type| Self::Type(object_type.to_owned()))
    }
}

/// Your calendars among `objects`, the calendar objects as stable ID,
/// title and properties in the order of their paths, by name without
/// letter case, then by ID. When two files hold one ID, as after a copy
/// in another app, the first by path stands for the calendar.
pub(crate) fn own<'a>(
    objects: impl Iterator<Item = (&'a str, &'a str, &'a Map<String, Value>)>,
) -> Vec<Own> {
    let mut seen = std::collections::BTreeSet::new();
    let mut own: Vec<Own> = objects
        .filter(|(id, _, _)| seen.insert(*id))
        .map(|(id, title, properties)| Own {
            id: id.to_owned(),
            name: if title.trim().is_empty() {
                "Untitled".to_owned()
            } else {
                title.to_owned()
            },
            color: properties
                .get("color")
                .and_then(Value::as_str)
                .and_then(color_named)
                .unwrap_or(Color::Blue),
        })
        .collect();
    own.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    own
}

fn properties(object: &Object) -> Map<String, Value> {
    serde_json::from_str(&object.properties).unwrap_or_default()
}

/// The calendar an object of `source_type` with `properties` belongs to:
/// an event's own calendar, or the first of yours when it names none of
/// them, and any other object the built-in calendar of its type. An
/// event belongs to the built-in Events only while you have no calendar.
pub(crate) fn key_of(source_type: &str, properties: &Map<String, Value>, own: &[Own]) -> Key {
    if source_type == EVENT {
        let named = properties.get("calendar").and_then(Value::as_str);
        if let Some(calendar) = own
            .iter()
            .find(|calendar| Some(calendar.id.as_str()) == named)
        {
            return Key::Own(calendar.id.clone());
        }
        if let Some(first) = own.first() {
            return Key::Own(first.id.clone());
        }
    }
    Key::Type(source_type.to_owned())
}

/// The name and color of the built-in calendar of `object_type`: the type
/// in the plural, as its folder is named, except Meetings for
/// Transcriptions.
pub(crate) fn built_in(object_type: &str) -> (String, Color) {
    match object_type {
        "task" => ("Tasks".to_owned(), Color::Orange),
        "transcription" => ("Meetings".to_owned(), Color::Purple),
        "note" => ("Notes".to_owned(), Color::Yellow),
        "project" => ("Projects".to_owned(), Color::Green),
        other => {
            let mut letters = other.chars();
            let name = letters.next().map_or_else(String::new, |first| {
                format!("{}{}s", first.to_uppercase(), letters.as_str()).replace('-', " ")
            });
            (name, Color::Gray)
        }
    }
}

/// The color a new calendar takes: the first that none of `own` has,
/// starting from blue, or the next in turn once each is taken.
pub(crate) fn next_color(own: &[Own]) -> Color {
    NEW_COLORS
        .iter()
        .copied()
        .find(|color| own.iter().all(|calendar| calendar.color != *color))
        .unwrap_or(NEW_COLORS[own.len() % NEW_COLORS.len()])
}

/// Create a calendar named `name` in `Calendars/`, in the first color
/// none of `own` has.
/// Returns its stable ID.
pub(crate) fn create(name: &str, own: &[Own]) -> Result<String, Error> {
    let color = next_color(own);
    objects::create(&NewObject {
        object_type: CALENDAR.to_owned(),
        title: names::file_name(name),
        path: None,
        body: String::new(),
        properties: json!({ "color": color_id(color) }).to_string(),
    })
    .map(|object| object.id)
}

/// Rename the calendar `id` to `name`, which renames its file in its
/// folder. Its events keep it by ID.
pub(crate) fn rename(id: &str, name: &str) -> Result<(), Error> {
    let latest = objects::get(id)?;
    let name = names::file_name(name);
    if name.is_empty() || name == latest.title {
        return Ok(());
    }
    objects::move_to(id, &latest.revision, &names::renamed(&latest.path, &name)).map(drop)
}

/// Give the calendar `id` the color `color`.
pub(crate) fn recolor(id: &str, color: Color) -> Result<(), Error> {
    let latest = objects::get(id)?;
    objects::update(
        id,
        &latest.revision,
        &Changes {
            body: None,
            properties: json!({ "color": color_id(color) }).to_string(),
            remove: Vec::new(),
        },
    )
    .map(drop)
}

/// The events whose `calendar` names the calendar `id`. Events that only
/// fall back to it, because their calendar is gone, aren't its own.
pub(crate) fn events_of(id: &str) -> Result<Vec<Object>, Error> {
    Ok(objects::list(EVENT)?
        .into_iter()
        .filter(|event| properties(event).get("calendar").and_then(Value::as_str) == Some(id))
        .collect())
}

/// Move the calendar `id` and its events to the trash: the events first,
/// so a failure leaves the calendar with the events still in it.
pub(crate) fn delete(id: &str) -> Result<(), Error> {
    for event in events_of(id)? {
        objects::delete(&event.id, &event.revision)?;
    }
    let calendar = objects::get(id)?;
    objects::delete(id, &calendar.revision)
}

/// The calendars you hid, as the settings file holds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The settings file's ID and revision, when there is one.
    file: Option<(String, String)>,
    hidden_calendars: Vec<String>,
    hidden_types: Vec<String>,
}

impl Settings {
    /// The settings now: the first settings file by path, or nothing
    /// hidden.
    pub(crate) fn read() -> Self {
        let mut files = objects::list(SETTINGS).unwrap_or_default();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let Some(file) = files.into_iter().next() else {
            return Self::default();
        };
        let properties = properties(&file);
        let strings = |key: &str| -> Vec<String> {
            properties
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            hidden_calendars: strings("hidden_calendars"),
            hidden_types: strings("hidden_types"),
            file: Some((file.id, file.revision)),
        }
    }

    /// Whether you hid the calendar `key`.
    pub(crate) fn hides(&self, key: &Key) -> bool {
        match key {
            Key::Own(id) => self.hidden_calendars.contains(id),
            Key::Type(object_type) => self.hidden_types.contains(object_type),
        }
    }

    /// Hide or show the calendar `key`, reading the settings again first
    /// so a change from another device stays. A change that lands between
    /// the read and the write is read once more.
    pub(crate) fn set_hidden(key: &Key, hidden: bool) -> Result<(), Error> {
        match Self::read().write_hidden(key, hidden) {
            Err(Error::RevisionConflict(_)) => Self::read().write_hidden(key, hidden),
            other => other,
        }
    }

    fn write_hidden(mut self, key: &Key, hidden: bool) -> Result<(), Error> {
        if self.hides(key) == hidden {
            return Ok(());
        }
        let (list, value) = match key {
            Key::Own(id) => (&mut self.hidden_calendars, id),
            Key::Type(object_type) => (&mut self.hidden_types, object_type),
        };
        if hidden {
            list.push(value.clone());
        } else {
            list.retain(|held| held != value);
        }
        let properties = json!({
            "hidden_calendars": self.hidden_calendars,
            "hidden_types": self.hidden_types,
        })
        .to_string();
        match &self.file {
            Some((id, revision)) => objects::update(
                id,
                revision,
                &Changes {
                    body: None,
                    properties,
                    remove: Vec::new(),
                },
            )
            .map(drop),
            None => objects::create(&NewObject {
                object_type: SETTINGS.to_owned(),
                title: String::new(),
                path: Some(SETTINGS_PATH.to_owned()),
                body: String::new(),
                properties,
            })
            .map(drop),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar(id: &str, name: &str, color: Color) -> Own {
        Own {
            id: id.to_owned(),
            name: name.to_owned(),
            color,
        }
    }

    #[test]
    fn an_event_belongs_to_its_calendar_or_falls_back_to_the_first() {
        let own = [
            calendar("calendar_a", "Home", Color::Blue),
            calendar("calendar_b", "Work", Color::Green),
        ];
        let event = |calendar: Value| {
            let mut properties = Map::new();
            properties.insert("calendar".into(), calendar);
            properties
        };
        assert_eq!(
            key_of(EVENT, &event(json!("calendar_b")), &own),
            Key::Own("calendar_b".into())
        );
        assert_eq!(
            key_of(EVENT, &event(json!("calendar_gone")), &own),
            Key::Own("calendar_a".into())
        );
        assert_eq!(key_of(EVENT, &Map::new(), &[]), Key::Type("event".into()));
        assert_eq!(
            key_of("task", &event(json!("calendar_b")), &own),
            Key::Type("task".into())
        );
    }

    #[test]
    fn built_in_calendars_are_named_after_their_type() {
        assert_eq!(built_in("task").0, "Tasks");
        assert_eq!(built_in("transcription").0, "Meetings");
        assert_eq!(built_in("recipe"), ("Recipes".to_owned(), Color::Gray));
        assert_eq!(built_in("reading-list").0, "Reading lists");
    }

    #[test]
    fn new_calendars_take_the_first_free_color() {
        assert_eq!(next_color(&[]), Color::Blue);
        let own = [calendar("calendar_a", "Home", Color::Blue)];
        assert_eq!(next_color(&own), Color::Green);
        let full: Vec<Own> = NEW_COLORS
            .iter()
            .map(|color| calendar("calendar_x", "X", *color))
            .collect();
        assert_eq!(next_color(&full), Color::Blue);
    }

    #[test]
    fn node_ids_name_their_calendar() {
        for key in [Key::Own("calendar_a".into()), Key::Type("task".into())] {
            assert_eq!(Key::of_node(&key.node()), Some(key));
        }
        assert_eq!(Key::of_node("new-calendar"), None);
    }

    #[test]
    fn every_palette_color_has_a_file_name() {
        for (color, id, _) in PALETTE {
            assert_eq!(color_named(id), Some(color));
            assert_eq!(color_id(color), id);
        }
    }
}
