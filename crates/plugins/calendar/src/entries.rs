//! The workspace's dated entries: one for each `due`, `date` and `start`
//! property of any object, whatever its type and whichever plugin owns
//! it, with the object's `end` beside it. Each object's file is read
//! again only when the listing's revision differs from the revision of
//! the bytes last read.

use std::{collections::BTreeMap, sync::Arc};

use plugin_sdk::workspace::{self, EntryKind};
use serde_json::{Map, Value};

use crate::rules::{CalendarItem, all_day_end, calendar_date, calendar_instant, timed_end};

/// The properties an entry can start from.
const STARTS: [&str; 3] = ["due", "date", "start"];

/// A dated entry: one date property of one object.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// Unique among the entries: the object's stable ID and the property,
    /// and a number after a second file with the same ID.
    pub(crate) key: String,
    pub(crate) source_id: String,
    /// Whether another file holds the same stable ID.
    pub(crate) shared_id: bool,
    pub(crate) source_type: String,
    pub(crate) title: String,
    pub(crate) property: String,
    pub(crate) start: String,
    pub(crate) end: Option<String>,
    /// The object's other properties, as its frontmatter holds them,
    /// shared by the object's entries.
    pub(crate) properties: Arc<Map<String, Value>>,
}

impl CalendarItem for Entry {
    fn start(&self) -> &str {
        &self.start
    }
    fn end(&self) -> Option<&str> {
        self.end.as_deref()
    }
    fn all_day(&self) -> bool {
        self.start.len() == 10
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn source_id(&self) -> &str {
        &self.source_id
    }
    fn property(&self) -> &str {
        &self.property
    }
}

impl Entry {
    /// The start and end a calendar grid draws: a date and the day after
    /// the last, or two instants, written the way the grid reads them. An
    /// end that isn't after the start, or isn't of the start's form, is
    /// left out, so the grid draws a point. `None` for a malformed start.
    pub(crate) fn span(&self) -> Option<(String, Option<String>)> {
        if self.all_day() {
            let start = calendar_date(&self.start).ok()?;
            let end = all_day_end(self.end.as_deref(), start);
            Some((start.to_string(), end.map(|end| end.to_string())))
        } else {
            let start = calendar_instant(&self.start).ok()?;
            let end = timed_end(self.end.as_deref(), start);
            Some((start.to_string(), end.map(|end| end.to_string())))
        }
    }
}

/// An object's file as last read.
struct Read {
    /// The revision of the bytes read, which can differ from the
    /// listing's when the file changed before the index saw it.
    revision: String,
    object: Object,
}

/// What an entry needs from its object.
#[derive(Clone)]
struct Object {
    id: String,
    object_type: String,
    title: String,
    properties: Arc<Map<String, Value>>,
}

/// The objects read so far, by the path of their files.
#[derive(Default)]
pub(crate) struct Entries {
    reads: BTreeMap<String, Read>,
}

impl Entries {
    pub(crate) const fn new() -> Self {
        Self {
            reads: BTreeMap::new(),
        }
    }

    /// Every dated entry in the workspace now, in the order of the paths
    /// of their files. A file that fails to read, such as one that lives
    /// only in the cloud for now, keeps the entries of its last read, or
    /// has none until a read succeeds. Each listing tries it again.
    pub(crate) fn all(&mut self) -> Result<Vec<Entry>, plugin_sdk::Error> {
        let listed = workspace::list_entries()?;
        let mut kept = BTreeMap::new();
        for entry in listed.into_iter().flatten() {
            if entry.kind != EntryKind::File || entry.object_id.is_none() {
                continue;
            }
            let revision = entry.revision.unwrap_or_default();
            let read = match self.reads.remove(&entry.path) {
                Some(read) if read.revision == revision && !revision.is_empty() => Some(read),
                last => read_at(&entry.path).or(last),
            };
            if let Some(read) = read {
                kept.insert(entry.path, read);
            }
        }
        self.reads = kept;
        Ok(dated(self.reads.values().map(|read| &read.object)))
    }
}

/// The dated entries of `objects`, in their order: one for each `due`,
/// `date` and `start` that holds text, with the object's `end` beside it.
fn dated<'a>(objects: impl Iterator<Item = &'a Object> + Clone) -> Vec<Entry> {
    let mut files: BTreeMap<&str, usize> = BTreeMap::new();
    for object in objects.clone() {
        *files.entry(&object.id).or_default() += 1;
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut entries = Vec::new();
    for object in objects {
        let text = |key: &str| object.properties.get(key).and_then(Value::as_str);
        let end = text("end").map(str::to_owned);
        for property in STARTS {
            let Some(start) = text(property) else {
                continue;
            };
            let base = format!("{}:{property}", object.id);
            let count = seen.entry(base.clone()).or_default();
            *count += 1;
            let key = match *count {
                1 => base,
                count => format!("{base}:{count}"),
            };
            entries.push(Entry {
                key,
                source_id: object.id.clone(),
                shared_id: files
                    .get(object.id.as_str())
                    .is_some_and(|&count| count > 1),
                source_type: object.object_type.clone(),
                title: object.title.clone(),
                property: property.to_owned(),
                start: start.to_owned(),
                end: end.clone(),
                properties: object.properties.clone(),
            });
        }
    }
    entries
}

/// The object in the file at `path` and the revision of the bytes read,
/// or `None` when the file couldn't be read.
fn read_at(path: &str) -> Option<Read> {
    let object = workspace::read_object_at(path).ok()?;
    Some(Read {
        revision: object.revision,
        object: Object::new(
            object.id,
            object.object_type,
            object.title,
            &object.properties,
        ),
    })
}

impl Object {
    /// What an entry needs from an object the host read, with its
    /// properties as the host's JSON.
    fn new(id: String, object_type: String, title: String, properties: &str) -> Self {
        Self {
            id,
            object_type,
            // A grid entry and a list item need a label with more than
            // spaces.
            title: if title.trim().is_empty() {
                "Untitled".to_owned()
            } else {
                title
            },
            properties: Arc::new(serde_json::from_str(properties).unwrap_or_default()),
        }
    }
}

/// The calendar cases of the plugin's fixture, over the shared queries
/// fixture's files: the plugin's entries and range rules give each
/// answer, as the kernel's calendar query gave it before the kernel
/// dropped it.
#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use workspace_format::{ParsedMarkdown, parse_markdown};

    use super::{Object, dated};
    use crate::rules::select_calendar_entries;

    /// The managed objects of the shared queries fixture's files, in the
    /// order of their paths, as the host lists and reads them. The host
    /// leaves out paths with a part that starts with a dot.
    fn fixture_objects() -> Vec<Object> {
        let queries: Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/queries-v1.json"
        ))
        .unwrap();
        let mut files: Vec<(&str, &str)> = queries["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                (
                    file["path"].as_str().unwrap(),
                    file["content"].as_str().unwrap(),
                )
            })
            .filter(|(path, _)| !path.split('/').any(|part| part.starts_with('.')))
            .collect();
        files.sort();
        files
            .into_iter()
            .filter_map(
                |(path, content)| match parse_markdown(path, content.as_bytes()) {
                    ParsedMarkdown::Managed(object) => Some(Object::new(
                        object.id,
                        object.object_type,
                        object.title,
                        &serde_json::to_string(&object.properties).unwrap(),
                    )),
                    _ => None,
                },
            )
            .collect()
    }

    #[test]
    fn the_calendar_cases_give_the_answers_the_kernel_gave() {
        let fixture: Value =
            serde_json::from_str(include_str!("../fixtures/calendar-v1.json")).unwrap();
        let objects = fixture_objects();
        let mut checked = 0;
        for case in fixture["queries"].as_array().unwrap() {
            if case.get("error").is_some() {
                continue;
            }
            let selected = select_calendar_entries(
                dated(objects.iter()),
                case["start"].as_str().unwrap(),
                case["end"].as_str().unwrap(),
            )
            .unwrap();
            let answers: Vec<Value> = selected
                .iter()
                .map(|entry| json!([entry.source_id, entry.property]))
                .collect();
            assert_eq!(Value::Array(answers), case["expected"], "{case}");
            checked += 1;
        }
        assert_eq!(checked, 2);
    }
}
