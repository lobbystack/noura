//! The editor sessions service: each Markdown file open in a tab, what the
//! editor shows of it, and the saves that bring its edits to disk.
//!
//! A managed note's editor shows the text after its frontmatter, and the
//! properties panel shows the frontmatter. A Markdown file without an ID
//! shows its whole text, frontmatter included. Saves write the file's whole
//! text through `local-core`, which keeps the bytes nobody edited: the
//! byte-order mark, each line's ending and the final newline. A save runs on
//! a worker thread and counts only once the durable write returns.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use local_core::{
    CoreError, ErrorCategory, ObjectPatch, ParsedMarkdown, RawSaveInput, RawSaveResult,
    WorkspaceEngine,
};
use serde::{Deserialize, Serialize};

use crate::{AppError, TabId};

/// How long typing must pause before a save starts.
pub const SAVE_PAUSE: Duration = Duration::from_millis(300);

/// The longest continuous typing goes before a save starts anyway.
pub const SAVE_AT_LEAST_EVERY: Duration = Duration::from_secs(2);

/// How wide a note's lines run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineWidth {
    Narrow,
    #[default]
    Default,
    /// The whole width of the window.
    Full,
}

impl LineWidth {
    /// Every width, in the order a menu offers them.
    pub const ALL: [Self; 3] = [Self::Narrow, Self::Default, Self::Full];

    pub fn title(self) -> &'static str {
        match self {
            Self::Narrow => "Narrow",
            Self::Default => "Default",
            Self::Full => "Full width",
        }
    }

    /// The widest a note's text runs, in points, as the legacy app sized it
    /// (40rem and 48rem), or `None` for the whole width.
    pub fn points(self) -> Option<f32> {
        match self {
            Self::Narrow => Some(640.0),
            Self::Default => Some(768.0),
            Self::Full => None,
        }
    }
}

/// A frontmatter property as the properties panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub key: String,
    pub value: PropertyValue,
    /// ID, type, created and updated belong to Noura, and values the panel
    /// can't edit in place show as text.
    pub read_only: bool,
}

/// A property's value, by the kind the panel edits it as.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Text(String),
    Number(serde_json::Number),
    Bool(bool),
    /// A civil date, `YYYY-MM-DD`.
    Date(String),
    /// Any other value, such as a list, written as the panel shows it.
    Other(String),
}

impl PropertyValue {
    /// The number typed in `text`, or `None` when it isn't one.
    pub fn number(text: &str) -> Option<Self> {
        let text = text.trim();
        text.parse::<i64>()
            .map(serde_json::Number::from)
            .ok()
            .or_else(|| {
                text.parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
            })
            .map(Self::Number)
    }

    fn of(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::String(text) if is_civil_date(text) => Self::Date(text.clone()),
            serde_json::Value::String(text) => Self::Text(text.clone()),
            serde_json::Value::Number(number) => Self::Number(number.clone()),
            serde_json::Value::Bool(value) => Self::Bool(*value),
            serde_json::Value::Null => Self::Other(String::new()),
            other => Self::Other(other.to_string()),
        }
    }

    /// The JSON value to write, or `None` for a value the panel doesn't
    /// edit.
    pub(crate) fn to_json(&self) -> Option<serde_json::Value> {
        match self {
            Self::Text(text) => Some(serde_json::Value::String(text.clone())),
            Self::Number(number) => Some(serde_json::Value::Number(number.clone())),
            Self::Bool(value) => Some(serde_json::Value::Bool(*value)),
            Self::Date(date) if is_civil_date(date) => {
                Some(serde_json::Value::String(date.clone()))
            }
            Self::Date(_) | Self::Other(_) => None,
        }
    }
}

fn is_civil_date(text: &str) -> bool {
    text.len() == 10
        && text.as_bytes().get(4) == Some(&b'-')
        && text.as_bytes().get(7) == Some(&b'-')
        && text
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

/// The properties Noura keeps for itself, which the panel shows read-only.
const NOURA_PROPERTIES: [&str; 4] = ["id", "type", "created", "updated"];

/// Why a save failed, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveProblem {
    pub reason: String,
}

/// What the editor showing a note has to catch up on after a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSync {
    /// The file now holds the editor's text as it was at this buffer
    /// version: the editor merges later outside edits against it.
    pub saved: (usize, String),
    /// The save merged an edit made outside Noura: the file now holds this
    /// text, which the editor merges in.
    pub merged: Option<String>,
}

/// The managed object a note's frontmatter describes.
#[derive(Debug, Clone, PartialEq)]
struct Managed {
    id: String,
    object_type: String,
    created: Option<String>,
    updated: Option<String>,
    properties: BTreeMap<String, serde_json::Value>,
}

/// A Markdown file open in a tab.
#[derive(Debug, Clone)]
pub struct NoteSession {
    relative_path: String,
    /// The file's whole text, with LF line breaks, when Noura last read or
    /// saved it.
    base: String,
    revision: String,
    /// Where the editor's text starts in `base`: after the frontmatter of a
    /// managed note, or 0.
    body_start: usize,
    /// Whether the editor shows the whole file, as for a Markdown file
    /// without an ID. Decided when the note opens, so a file that gains or
    /// loses an ID while open keeps how its text maps to the editor.
    whole_file: bool,
    managed: Option<Managed>,
    /// The editor's text and its buffer version, as the editor last
    /// reported them.
    text: String,
    version: usize,
    /// When the oldest edit not yet handed to a save happened.
    first_unsaved: Option<Instant>,
    last_edit: Option<Instant>,
    /// The save running now, with the text and version it writes.
    writing: Option<(usize, String)>,
    /// Property edits waiting to be written, in order.
    property_edits: Vec<(String, serde_json::Value)>,
    problem: Option<SaveProblem>,
    /// After a failure, saves wait for the next edit or Try again.
    waiting_for_edit: bool,
    /// The next save starts at once, as Mod-S or leaving the note asks.
    forced: bool,
    sync: Option<NoteSync>,
    name_problem: Option<String>,
}

impl NoteSession {
    /// Read the Markdown file at `relative_path`.
    pub(crate) fn open(engine: &WorkspaceEngine, relative_path: &str) -> Result<Self, AppError> {
        let read = engine
            .read_raw_markdown(relative_path)
            .map_err(|error| open_error(error, relative_path))?;
        let mut session = Self {
            relative_path: relative_path.to_owned(),
            base: String::new(),
            revision: String::new(),
            body_start: 0,
            whole_file: true,
            managed: None,
            text: String::new(),
            version: 0,
            first_unsaved: None,
            last_edit: None,
            writing: None,
            property_edits: Vec::new(),
            problem: None,
            waiting_for_edit: false,
            forced: false,
            sync: None,
            name_problem: None,
        };
        session.whole_file = !matches!(
            local_core::parse_markdown(relative_path, read.body.as_bytes()),
            ParsedMarkdown::Managed(_)
        );
        session.read(read.body, read.revision);
        session.text = session.body().to_owned();
        Ok(session)
    }

    /// Take `base`, the file's whole LF text, as what the file holds now.
    fn read(&mut self, base: String, revision: String) {
        let parsed = local_core::parse_markdown(&self.relative_path, base.as_bytes());
        self.managed = match parsed {
            ParsedMarkdown::Managed(object) if !self.whole_file => Some(Managed {
                id: object.id,
                object_type: object.object_type,
                created: object.created,
                updated: object.updated,
                properties: object.properties,
            }),
            _ => None,
        };
        self.body_start = match &self.managed {
            Some(_) => local_core::frontmatter_end(&base).unwrap_or(0),
            None => 0,
        };
        self.base = base;
        self.revision = revision;
    }

    /// The text the file holds for the editor: after the frontmatter of a
    /// managed note, or the whole file.
    fn body(&self) -> &str {
        &self.base[self.body_start..]
    }

    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// The note's name: its file name without `.md`.
    pub fn name(&self) -> String {
        crate::files::shown_name(
            self.relative_path
                .rsplit('/')
                .next()
                .unwrap_or(&self.relative_path),
            crate::EntryKind::File,
        )
    }

    /// The text the editor shows.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The stable ID of a managed note.
    pub fn object_id(&self) -> Option<&str> {
        self.managed.as_ref().map(|managed| managed.id.as_str())
    }

    /// Whether the note has edits that aren't on disk yet, counting a save
    /// that's still running.
    pub fn is_unsaved(&self) -> bool {
        self.text != self.body() || self.writing.is_some() || !self.property_edits.is_empty()
    }

    /// Whether a save is running.
    pub fn is_saving(&self) -> bool {
        self.writing.is_some()
    }

    /// Why the last save failed, until a save succeeds.
    pub fn save_problem(&self) -> Option<&SaveProblem> {
        self.problem.as_ref()
    }

    /// Why the name field kept the old name, until the next rename.
    pub fn name_problem(&self) -> Option<&str> {
        self.name_problem.as_deref()
    }

    /// The frontmatter of a managed note for the properties panel: ID,
    /// type, the other properties in the file's order, then created and
    /// updated. `None` for a file without an ID.
    pub fn properties(&self) -> Option<Vec<Property>> {
        let managed = self.managed.as_ref()?;
        let fixed = |key: &str, value: Option<&str>| {
            value.map(|value| Property {
                key: key.to_owned(),
                value: PropertyValue::Text(value.to_owned()),
                read_only: true,
            })
        };
        let mut properties: Vec<Property> = [
            fixed("id", Some(&managed.id)),
            fixed("type", Some(&managed.object_type)),
        ]
        .into_iter()
        .flatten()
        .collect();
        for (key, value) in &managed.properties {
            let shown = self
                .property_edits
                .iter()
                .rev()
                .find(|(edited, _)| edited == key)
                .map_or(value, |(_, value)| value);
            let value = PropertyValue::of(shown);
            properties.push(Property {
                key: key.clone(),
                read_only: value.to_json().is_none(),
                value,
            });
        }
        properties.extend(fixed("created", managed.created.as_deref()));
        properties.extend(fixed("updated", managed.updated.as_deref()));
        Some(properties)
    }

    /// The editor reported its text at buffer version `version`, at `at`.
    /// Returns whether the text differs from what the session had.
    pub(crate) fn edited(&mut self, version: usize, text: &str, at: Instant) -> bool {
        self.version = version;
        if self.text == text {
            return false;
        }
        text.clone_into(&mut self.text);
        self.first_unsaved.get_or_insert(at);
        self.last_edit = Some(at);
        self.waiting_for_edit = false;
        true
    }

    /// Whether a save should start at `now`: once typing paused for
    /// [`SAVE_PAUSE`], or [`SAVE_AT_LEAST_EVERY`] after the oldest unsaved
    /// edit, or at once for property edits, Mod-S and leaving the note.
    /// Never while a save runs, or while a failed save waits for the next
    /// edit.
    pub(crate) fn is_due(&self, now: Instant) -> bool {
        if self.writing.is_some() || self.waiting_for_edit {
            return false;
        }
        if !self.property_edits.is_empty() {
            return true;
        }
        if self.text == self.body() {
            return false;
        }
        self.forced || self.next_due().is_some_and(|due| due <= now)
    }

    /// When typing makes the next save due, for a client to wake up then.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        if self.writing.is_some() || self.waiting_for_edit || self.text == self.body() {
            return None;
        }
        let pause = self.last_edit? + SAVE_PAUSE;
        let interval = self.first_unsaved? + SAVE_AT_LEAST_EVERY;
        Some(pause.min(interval))
    }

    /// Make the next save start at once.
    pub(crate) fn save_soon(&mut self) {
        self.waiting_for_edit = false;
        self.forced = true;
    }

    /// Hand the next write to a worker: the text when it changed, or else
    /// the waiting property edits.
    pub(crate) fn start_write(&mut self) -> Option<Write> {
        if self.writing.is_some() {
            return None;
        }
        if self.text != self.body() {
            let local = format!("{}{}", &self.base[..self.body_start], self.text);
            self.writing = Some((self.version, self.text.clone()));
            self.first_unsaved = None;
            self.last_edit = None;
            self.forced = false;
            return Some(Write::Text(RawSaveInput {
                relative_path: self.relative_path.clone(),
                base_revision: self.revision.clone(),
                base_body: self.base.clone(),
                local_body: local,
            }));
        }
        let managed = self.managed.as_ref()?;
        if self.property_edits.is_empty() {
            return None;
        }
        let properties: BTreeMap<String, serde_json::Value> =
            self.property_edits.iter().cloned().collect();
        self.writing = Some((self.version, self.text.clone()));
        self.forced = false;
        Some(Write::Properties {
            id: managed.id.clone(),
            relative_path: self.relative_path.clone(),
            patch: ObjectPatch {
                title: None,
                body: None,
                properties,
                remove_properties: Vec::new(),
                expected_revision: self.revision.clone(),
            },
            count: self.property_edits.len(),
        })
    }

    /// Take the result of the write [`Self::start_write`] handed out.
    pub(crate) fn finish_write(&mut self, written: Written, now: Instant) {
        let Some((version, sent)) = self.writing.take() else {
            return;
        };
        match written {
            Written::Text(Ok(RawSaveResult::Saved { current, .. })) => {
                self.read(current.body, current.revision);
                self.saved(version, sent, None);
            }
            Written::Text(Ok(RawSaveResult::Merged { current, .. })) => {
                self.read(current.body, current.revision);
                let merged = self.body().to_owned();
                // The editor merges the outside edit in, and reports its
                // text again if it held more edits.
                if self.text == sent {
                    merged.clone_into(&mut self.text);
                }
                self.saved(version, sent, Some(merged));
            }
            Written::Text(Ok(RawSaveResult::Conflict { .. })) => {
                self.failed(
                    "The file changed outside Noura while you were typing, in the same \
                     place."
                        .to_owned(),
                    now,
                );
            }
            Written::Properties(Ok((count, read))) => {
                self.property_edits
                    .drain(..count.min(self.property_edits.len()));
                self.read(read.body, read.revision);
                self.problem = None;
            }
            Written::Text(Err(error)) | Written::Properties(Err(error)) => {
                tracing::warn!(code = %error.code, operation = %error.operation, "a note didn't save");
                self.failed(save_reason(&error), now);
            }
        }
    }

    /// The file holds `sent`, the editor's text at `version`.
    fn saved(&mut self, version: usize, sent: String, merged: Option<String>) {
        self.problem = None;
        self.sync = Some(NoteSync {
            saved: (version, sent),
            merged,
        });
        // Edits made while the save ran wait their own turn.
        if self.text != self.body() && self.first_unsaved.is_none() {
            self.first_unsaved = self.last_edit;
        }
    }

    fn failed(&mut self, reason: String, now: Instant) {
        self.problem = Some(SaveProblem { reason });
        self.waiting_for_edit = true;
        self.first_unsaved.get_or_insert(now);
    }

    /// Drop the edits that aren't on disk, leaving the file as it is. The
    /// note's tab is about to close.
    pub(crate) fn discard(&mut self) {
        self.text = self.body().to_owned();
        self.property_edits.clear();
        self.problem = None;
        self.waiting_for_edit = false;
        self.forced = false;
        self.first_unsaved = None;
        self.last_edit = None;
    }

    pub(crate) fn take_sync(&mut self) -> Option<NoteSync> {
        self.sync.take()
    }

    pub(crate) fn queue_property(&mut self, key: String, value: serde_json::Value) {
        self.property_edits.push((key, value));
        self.waiting_for_edit = false;
    }

    pub(crate) fn has_property(&self, key: &str) -> bool {
        self.managed
            .as_ref()
            .is_some_and(|managed| managed.properties.contains_key(key))
            && !NOURA_PROPERTIES.contains(&key)
    }

    pub(crate) fn set_name_problem(&mut self, problem: Option<String>) {
        self.name_problem = problem;
    }

    pub(crate) fn moved_to(&mut self, relative_path: &str) {
        relative_path.clone_into(&mut self.relative_path);
    }
}

/// A write a worker carries out for a note.
#[derive(Debug, Clone)]
pub(crate) enum Write {
    /// The file's whole new text, merged with outside edits made since
    /// `base_revision`.
    Text(RawSaveInput),
    /// Property edits, which only a file still at the expected revision
    /// takes. `count` edits are in the patch.
    Properties {
        id: String,
        relative_path: String,
        patch: ObjectPatch,
        count: usize,
    },
}

/// What a write did.
#[derive(Debug)]
pub(crate) enum Written {
    Text(Result<RawSaveResult, CoreError>),
    /// How many property edits went in, and the file as it is now.
    Properties(Result<(usize, local_core::RawMarkdownRead), CoreError>),
}

impl Write {
    /// Carry out the write. Returns once the bytes are durably on disk, or
    /// the write failed.
    pub(crate) fn run(self, engine: &WorkspaceEngine) -> Written {
        match self {
            Self::Text(input) => Written::Text(engine.save_raw_markdown(input)),
            Self::Properties {
                id,
                relative_path,
                patch,
                count,
            } => Written::Properties(match engine.update_object(&id, patch) {
                Ok(_) => engine
                    .read_raw_markdown(&relative_path)
                    .map(|read| (count, read)),
                Err(error) => Err(error),
            }),
        }
    }
}

/// A finished write, sent back from its worker.
pub(crate) struct Finished {
    pub(crate) tab: TabId,
    pub(crate) written: Written,
}

/// The open notes, by tab, and the writes running for them.
pub(crate) struct Notes {
    pub(crate) sessions: HashMap<TabId, NoteSession>,
    /// Tabs whose Markdown file couldn't be read, and why.
    pub(crate) unreadable: HashMap<TabId, AppError>,
    sender: Sender<Finished>,
    pub(crate) results: Receiver<Finished>,
}

impl Default for Notes {
    fn default() -> Self {
        let (sender, results) = channel();
        Self {
            sessions: HashMap::new(),
            unreadable: HashMap::new(),
            sender,
            results,
        }
    }
}

impl Notes {
    /// Start `write` for the note in `tab` on a worker thread, which calls
    /// `done` once the write finished. Hands the write back when no thread
    /// could start.
    pub(crate) fn spawn(
        &self,
        tab: TabId,
        write: Write,
        engine: Arc<WorkspaceEngine>,
        done: impl FnOnce() + Send + 'static,
    ) -> Option<Write> {
        let sender = self.sender.clone();
        let fallback = write.clone();
        let spawned = std::thread::Builder::new()
            .name("note-save".into())
            .spawn(move || {
                let written = write.run(&engine);
                // The app may have closed meanwhile; nothing waits then.
                let _ = sender.send(Finished { tab, written });
                done();
            });
        match spawned {
            Ok(_) => None,
            Err(error) => {
                tracing::error!(%error, "could not start a save thread");
                Some(fallback)
            }
        }
    }
}

/// Why the Markdown file at `relative_path` can't open in the editor.
fn open_error(error: CoreError, relative_path: &str) -> AppError {
    tracing::warn!(code = %error.code, operation = %error.operation, "a note didn't open");
    match error.code.as_str() {
        "invalid_utf8" => AppError::new(
            "invalid_utf8",
            "This file isn't UTF-8 text, so the editor can't open it.",
        ),
        "file_not_downloaded" => AppError::new(
            "file_not_downloaded",
            "This file isn't downloaded yet. Try again once it is.",
        ),
        _ => AppError::from_core(error, relative_path, crate::error::Access::ReadFile),
    }
}

/// Why a save failed, in plain words.
pub(crate) fn save_reason(error: &CoreError) -> String {
    match error.code.as_str() {
        "raw_markdown_missing" | "object_not_found" | "file_not_found" => {
            "The file was moved or deleted outside Noura.".to_owned()
        }
        "invalid_utf8" | "object_parse_failed" => {
            "The file on disk isn't UTF-8 text Noura can read anymore.".to_owned()
        }
        "revision_conflict" => "The file changed outside Noura while it was saving.".to_owned(),
        "disk_full" => "The disk is full. Free up some space, then try again.".to_owned(),
        _ => match error.category {
            ErrorCategory::Permission => {
                "Noura doesn't have permission to change this file.".to_owned()
            }
            _ => "Noura couldn't write the file to the disk.".to_owned(),
        },
    }
}

/// Whether the file at `relative_path` opens in the note editor.
pub(crate) fn is_markdown(relative_path: &str) -> bool {
    relative_path.len() > 3
        && relative_path.is_char_boundary(relative_path.len() - 3)
        && relative_path[relative_path.len() - 3..].eq_ignore_ascii_case(".md")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: &str, category: ErrorCategory) -> CoreError {
        CoreError::new(code, category, "detail", "raw_markdown_save")
    }

    #[test]
    fn save_failures_read_as_plain_words() {
        let cases = [
            (
                error("filesystem_error", ErrorCategory::Permission),
                "Noura doesn't have permission to change this file.",
            ),
            (
                error("disk_full", ErrorCategory::Filesystem),
                "The disk is full. Free up some space, then try again.",
            ),
            (
                error("raw_markdown_missing", ErrorCategory::Validation),
                "The file was moved or deleted outside Noura.",
            ),
            (
                error("invalid_utf8", ErrorCategory::Parse),
                "The file on disk isn't UTF-8 text Noura can read anymore.",
            ),
            (
                error("revision_conflict", ErrorCategory::Conflict),
                "The file changed outside Noura while it was saving.",
            ),
            (
                error("filesystem_error", ErrorCategory::Filesystem),
                "Noura couldn't write the file to the disk.",
            ),
        ];
        for (error, reason) in cases {
            assert_eq!(save_reason(&error), reason, "{}", error.code);
        }
    }

    #[test]
    fn typed_numbers_read_as_json_numbers() {
        assert_eq!(
            PropertyValue::number(" 42 "),
            Some(PropertyValue::Number(42.into()))
        );
        assert_eq!(
            PropertyValue::number("4.5"),
            Some(PropertyValue::Number(
                serde_json::Number::from_f64(4.5).unwrap()
            ))
        );
        assert_eq!(PropertyValue::number("four"), None);
        assert_eq!(PropertyValue::number("NaN"), None);
    }

    #[test]
    fn only_civil_dates_edit_as_dates() {
        assert!(is_civil_date("2026-10-21"));
        assert!(!is_civil_date("2026-10-21T10:00:00Z"));
        assert!(!is_civil_date("21-10-2026"));
        assert_eq!(
            PropertyValue::of(&serde_json::json!("2026-10-21")),
            PropertyValue::Date("2026-10-21".into())
        );
        assert_eq!(
            PropertyValue::of(&serde_json::json!(["a"])),
            PropertyValue::Other("[\"a\"]".into())
        );
    }
}
