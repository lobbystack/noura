//! The editor sessions service: each Markdown, plain-text or code file open
//! in a tab, what the editor shows of it, and the saves that bring its edits
//! to disk.
//!
//! A managed note's editor shows the text after its frontmatter, and the
//! properties panel shows the frontmatter. A Markdown file without an ID
//! shows its whole text, frontmatter included. A plain-text or code file
//! shows its whole text as source, with no Markdown drawn. Saves write the
//! file's whole text through `local-core`, which keeps the bytes nobody
//! edited: the byte-order mark, each line's ending and the final newline. A
//! save runs on a worker thread and counts only once the durable write
//! returns.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use local_core::{
    ConflictResolution, CoreError, ErrorCategory, ObjectPatch, ParsedMarkdown,
    RawConflictResolveInput, RawRestoreInput, RawSaveInput, RawSaveResult, WorkspaceEngine,
};

use crate::{AppError, TabId};

/// How long typing must pause before a save starts.
pub const SAVE_PAUSE: Duration = Duration::from_millis(300);

/// The longest continuous typing goes before a save starts anyway.
pub const SAVE_AT_LEAST_EVERY: Duration = Duration::from_secs(2);

/// How long a save that must finish first, as before a rename, may take.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Whether `text` is a day on the calendar written `YYYY-MM-DD`, which the
/// panel edits with a date picker.
fn is_civil_date(text: &str) -> bool {
    text.len() == 10
        && text.as_bytes().get(4) == Some(&b'-')
        && text.as_bytes().get(7) == Some(&b'-')
        && text
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        && text.parse::<jiff::civil::Date>().is_ok()
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

/// Why a note with a conflict can't save, as the prompt before leaving it
/// says.
const CONFLICT_REASON: &str = "The file changed in another app.";

/// Why a note whose file went away can't save.
const GONE_REASON: &str = "The file was moved or deleted outside Noura.";

/// Whether `error` says the note's file isn't there anymore.
fn is_gone(error: &CoreError) -> bool {
    matches!(
        error.code.as_str(),
        "raw_markdown_missing" | "object_not_found" | "file_not_found"
    )
}

/// Another app changed the text you typed in, in the same place. Saves
/// wait while the conflict review compares your text with the file's and
/// you choose one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteConflict {
    /// The file's whole text, with LF line breaks, and its revision.
    file: String,
    revision: String,
    /// The part of `file` the editor would show.
    shown: String,
    reviewing: bool,
    confirming: Option<ConflictChoice>,
    /// Why the version you chose didn't go in.
    problem: Option<String>,
}

impl NoteConflict {
    /// Why the version you chose didn't go in, until you choose again.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The file's version, as the editor would show it.
    pub fn file_text(&self) -> &str {
        &self.shown
    }

    /// Whether the review is open. Keep editing closes it and leaves a
    /// banner that opens it again.
    pub fn is_reviewing(&self) -> bool {
        self.reviewing
    }

    /// The version waiting for you to confirm it.
    pub fn confirming(&self) -> Option<ConflictChoice> {
        self.confirming
    }
}

/// Which version a conflict keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Your text replaces the file's.
    Mine,
    /// The note takes the file's text.
    File,
}

/// A property edit waiting to be written.
#[derive(Debug, Clone)]
struct PropertyEdit {
    key: String,
    value: serde_json::Value,
    /// The value the panel showed when you made the edit, `None` standing
    /// for no such property. Only a file that still holds it takes the
    /// edit, so an edit never overwrites another program's.
    base: Option<serde_json::Value>,
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

impl Managed {
    /// Whether the kernel's panel edits this object's properties. Notes
    /// only: another type's plugin supplies its own panel (ticket 25), and
    /// until then its properties show read-only.
    fn edits_in_place(&self) -> bool {
        self.object_type == "note"
    }
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
    /// without an ID. A managed note's file that holds no managed note
    /// anymore shows whole from then on, as [`Self::read`] says. A Markdown
    /// file shown whole that a save or a conflict choice leaves with an ID
    /// and a type opens as a note, as [`Self::reopen_if_fixed`] says.
    whole_file: bool,
    /// Whether the editor shows the text as source, without the live
    /// preview. Decided when the note opens.
    source_only: bool,
    /// Whether the file is a plain-text or code file rather than Markdown:
    /// its text is never read as frontmatter. Decided when it opens, and
    /// again once nothing is unsaved after a move changed its extension.
    text_file: bool,
    /// Why the properties of a Markdown file shown whole don't parse, as
    /// the file holds them now.
    broken: Option<&'static str>,
    /// Whether the editor shows the file as source because its properties
    /// didn't parse when it opened, or when a managed note's file lost its
    /// managed note. Once a save leaves them readable, the file shows as it
    /// would open then.
    broken_layout: bool,
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
    property_edits: Vec<PropertyEdit>,
    problem: Option<SaveProblem>,
    /// After a text save failed, saves wait for the next edit or Try
    /// again.
    waiting_for_edit: bool,
    /// After a property write failed, property writes wait for the next
    /// edit or Try again. Text typed meanwhile saves on its own schedule.
    properties_held: bool,
    /// The next save starts at once, as Mod-S or leaving the note asks.
    forced: bool,
    sync: Option<NoteSync>,
    /// Discard changes dropped the edits that weren't on disk: the editor
    /// shows this text, the file's, in place of its own.
    reset: Option<String>,
    name_problem: Option<String>,
    /// The files changed while a save ran: look at the file again once the
    /// save is in.
    look_again: bool,
    conflict: Option<NoteConflict>,
    /// The note's text lives in a document in the collaboration sandbox,
    /// and its file follows the document, so nothing here saves it.
    text_mode: Option<sync_protocol::types::Id>,
    /// Another program deleted the file while the note had unsaved edits:
    /// saves wait, and Restore writes the note back.
    deleted: bool,
    restore_problem: Option<String>,
    /// Whether most lines ended in CRLF and a byte-order mark came first
    /// when Noura last read the file, for Restore.
    uses_crlf: bool,
    has_bom: bool,
}

impl NoteSession {
    /// Read the Markdown, plain-text or code file at `relative_path`.
    pub(crate) fn open(engine: &WorkspaceEngine, relative_path: &str) -> Result<Self, AppError> {
        let read = engine
            .read_raw_markdown(relative_path)
            .map_err(|error| open_error(error, relative_path))?;
        let mut session = Self {
            relative_path: relative_path.to_owned(),
            base: read.body,
            revision: read.revision,
            body_start: 0,
            whole_file: true,
            source_only: false,
            text_file: false,
            broken: None,
            broken_layout: false,
            managed: None,
            text: String::new(),
            version: 0,
            first_unsaved: None,
            last_edit: None,
            writing: None,
            property_edits: Vec::new(),
            problem: None,
            waiting_for_edit: false,
            properties_held: false,
            forced: false,
            sync: None,
            reset: None,
            name_problem: None,
            look_again: false,
            conflict: None,
            text_mode: None,
            deleted: false,
            restore_problem: None,
            uses_crlf: read.uses_crlf,
            has_bom: read.has_bom,
        };
        session.lay_out();
        Ok(session)
    }

    /// Show the text Noura last read as the file at its path opens: a
    /// plain-text or code file as source, a managed note after its
    /// frontmatter, and any other Markdown file whole, as source when its
    /// properties don't parse.
    fn lay_out(&mut self) {
        self.text_file = !is_markdown(&self.relative_path);
        self.whole_file = self.text_file
            || !matches!(
                local_core::parse_markdown(&self.relative_path, self.base.as_bytes()),
                ParsedMarkdown::Managed(_)
            );
        let base = std::mem::take(&mut self.base);
        let revision = std::mem::take(&mut self.revision);
        self.read(base, revision);
        self.text = self.body().to_owned();
        self.broken_layout = self.broken.is_some();
        self.source_only = self.text_file || self.broken_layout;
    }

    /// Take `read` as what the file holds now, as [`Self::read`] says.
    fn take(&mut self, read: local_core::RawMarkdownRead) -> Option<String> {
        self.uses_crlf = read.uses_crlf;
        self.has_bom = read.has_bom;
        self.read(read.body, read.revision)
    }

    /// Take `base`, the file's whole LF text, as what the file holds now.
    ///
    /// A managed note's file that holds no managed note anymore, as when
    /// another app broke its properties, shows whole from now on, as such
    /// a file opens: as source under the notice when its properties don't
    /// parse. The editor's text gains the frontmatter it followed, which
    /// comes back as `Some` for the caller to put before any other text it
    /// holds in the editor's old terms. The editor starts again from the
    /// whole text, as a change of [`Self::shows_whole_file`] tells it.
    /// Writes then send the whole text, so they never drop the frontmatter
    /// or write it twice.
    fn read(&mut self, base: String, revision: String) -> Option<String> {
        let parsed = (!self.text_file)
            .then(|| local_core::parse_markdown(&self.relative_path, base.as_bytes()));
        let lost = (!self.whole_file && !matches!(parsed, Some(ParsedMarkdown::Managed(_))))
            .then(|| self.base[..self.body_start].to_owned());
        if lost.is_some() {
            self.whole_file = true;
        }
        self.broken = match &parsed {
            Some(ParsedMarkdown::Malformed { error, .. }) if self.whole_file => {
                Some(properties_problem(error))
            }
            _ => None,
        };
        self.managed = match parsed {
            Some(ParsedMarkdown::Managed(object)) if !self.whole_file => Some(Managed {
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
        if let Some(frontmatter) = &lost {
            self.text.insert_str(0, frontmatter);
            self.broken_layout = self.broken.is_some();
            self.source_only = self.broken_layout;
            if let Some(conflict) = &mut self.conflict {
                conflict.shown.clone_from(&conflict.file);
            }
        }
        lost
    }

    /// The text the file holds for the editor: after the frontmatter of a
    /// managed note, or the whole file.
    fn body(&self) -> &str {
        &self.base[self.body_start..]
    }

    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// The note's name, as the name field shows it: its file name without
    /// `.md`, or a plain-text or code file's name without its extension.
    pub fn name(&self) -> String {
        let file_name = self.file_name();
        if self.text_file {
            return split_extension(file_name).0.to_owned();
        }
        crate::files::shown_name(file_name, crate::EntryKind::File)
    }

    /// The file name a rename to `name`, the name field's text, gives the
    /// file: a plain-text or code file keeps its extension. A Markdown
    /// file's `.md` comes back with the rename itself.
    pub(crate) fn renamed_file_name(&self, name: &str) -> String {
        if self.text_file {
            return format!("{name}{}", split_extension(self.file_name()).1);
        }
        name.to_owned()
    }

    fn file_name(&self) -> &str {
        self.relative_path
            .rsplit('/')
            .next()
            .unwrap_or(&self.relative_path)
    }

    /// The text the editor shows.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the editor shows the file's whole text, frontmatter
    /// included, rather than a managed note's text after its frontmatter.
    /// An editor drawn for one layout starts again when the other shows.
    pub fn shows_whole_file(&self) -> bool {
        self.whole_file
    }

    /// Whether the editor shows the text as source, without the live
    /// preview: a plain-text or code file, or a Markdown file shown whole
    /// whose properties don't parse.
    pub fn shows_source(&self) -> bool {
        self.source_only
    }

    /// Why the properties at the top of a Markdown file shown whole don't
    /// parse, in plain words: the editor shows them in its text, where you
    /// can fix them. `None` once they parse, and for every other file.
    pub fn broken_properties(&self) -> Option<&str> {
        self.broken
    }

    /// The stable ID of a managed note.
    pub fn object_id(&self) -> Option<&str> {
        self.managed.as_ref().map(|managed| managed.id.as_str())
    }

    /// The type of a managed note, such as `note` or a plugin's type.
    pub fn object_type(&self) -> Option<&str> {
        self.managed
            .as_ref()
            .map(|managed| managed.object_type.as_str())
    }

    /// The revision of the file as Noura last read or saved it.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Whether the note has edits that aren't on disk yet, counting a save
    /// that's still running.
    pub fn is_unsaved(&self) -> bool {
        if self.text_mode.is_some() {
            return self.writing.is_some() || !self.property_edits.is_empty();
        }
        self.text != self.body() || self.writing.is_some() || !self.property_edits.is_empty()
    }

    /// The note in text mode, when its text lives in a document.
    pub fn text_mode(&self) -> Option<sync_protocol::types::Id> {
        self.text_mode
    }

    /// The note's text lives in the document of `rid` from now on.
    pub(crate) fn enter_text_mode(&mut self, rid: sync_protocol::types::Id) {
        self.text_mode = Some(rid);
        self.first_unsaved = None;
        self.last_edit = None;
    }

    /// Whether a change in the editor could put this note in text mode: a
    /// Markdown file whose text the editor holds as the file does.
    pub(crate) fn may_enter_text_mode(&self) -> bool {
        self.text_mode.is_none()
            && !self.text_file
            && !self.has_bom
            && !self.uses_crlf
            && !self.deleted
            && self.conflict.is_none()
    }

    /// Whether a save is running.
    pub fn is_saving(&self) -> bool {
        self.writing.is_some()
    }

    /// Why the last save failed, until a save succeeds.
    pub fn save_problem(&self) -> Option<&SaveProblem> {
        self.problem.as_ref()
    }

    /// Another app's edit of the text you typed in, in the same place,
    /// until you choose a version.
    pub fn conflict(&self) -> Option<&NoteConflict> {
        self.conflict.as_ref()
    }

    /// Whether another program deleted the file while the note had
    /// unsaved edits. The note keeps its text and offers Restore.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Why the name field kept the old name, until the next rename.
    pub fn name_problem(&self) -> Option<&str> {
        self.name_problem.as_deref()
    }

    /// The frontmatter of a managed note for the properties panel: ID,
    /// type, the other properties sorted by key, then created and updated.
    /// `None` for a file without an ID.
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
                .find(|edit| edit.key == *key)
                .map_or(value, |edit| &edit.value);
            let value = PropertyValue::of(shown);
            properties.push(Property {
                key: key.clone(),
                read_only: !managed.edits_in_place() || value.to_json().is_none(),
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
        self.properties_held = false;
        true
    }

    /// Whether a save should start at `now`: once typing paused for
    /// [`SAVE_PAUSE`], or [`SAVE_AT_LEAST_EVERY`] after the oldest unsaved
    /// edit, or at once for property edits, Mod-S and leaving the note.
    /// Never while a save runs, or while a failed save waits for the next
    /// edit. A failed property write holds back only the property edits.
    pub(crate) fn is_due(&self, now: Instant) -> bool {
        if self.writing.is_some() || self.waiting_for_edit {
            return false;
        }
        if self.text_mode.is_some() {
            return !self.property_edits.is_empty() && !self.properties_held;
        }
        if self.conflict.is_some() || self.deleted {
            // Autosave waits for your choice, or for Restore. Try again
            // still looks, in case the file changed back.
            return self.forced;
        }
        if self.text == self.body() {
            return !self.property_edits.is_empty() && !self.properties_held;
        }
        self.forced || self.next_due().is_some_and(|due| due <= now)
    }

    /// When typing makes the next save due, for a client to wake up then.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        if self.writing.is_some()
            || self.waiting_for_edit
            || self.conflict.is_some()
            || self.deleted
            || self.text_mode.is_some()
            || self.text == self.body()
        {
            return None;
        }
        let pause = self.last_edit? + SAVE_PAUSE;
        let interval = self.first_unsaved? + SAVE_AT_LEAST_EVERY;
        Some(pause.min(interval))
    }

    /// Make the next save start at once.
    pub(crate) fn save_soon(&mut self) {
        self.waiting_for_edit = false;
        self.properties_held = false;
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
        if self.property_edits.is_empty() {
            return None;
        }
        // The last edit of each property wins, and the file must still
        // hold the value the first one replaced.
        let mut properties = BTreeMap::new();
        let mut before = BTreeMap::new();
        for edit in &self.property_edits {
            properties.insert(edit.key.clone(), edit.value.clone());
            before
                .entry(edit.key.clone())
                .or_insert_with(|| edit.base.clone());
        }
        self.writing = Some((self.version, self.text.clone()));
        self.forced = false;
        Some(Write::Properties {
            relative_path: self.relative_path.clone(),
            before,
            patch: ObjectPatch {
                title: None,
                body: None,
                properties,
                remove_properties: Vec::new(),
                // The write takes the revision of the file as it finds it.
                expected_revision: String::new(),
            },
            count: self.property_edits.len(),
        })
    }

    /// Take the result of the write [`Self::start_write`] handed out.
    /// Returns whether the save merged another app's edit with your
    /// typing.
    pub(crate) fn finish_write(&mut self, written: Written, now: Instant) -> bool {
        let Some((version, sent)) = self.writing.take() else {
            return false;
        };
        match written {
            Written::Text(Ok(RawSaveResult::Saved { current, .. })) => {
                let sent = in_whole_file(self.take(current), sent);
                self.saved(version, sent, None);
            }
            Written::Text(Ok(RawSaveResult::Merged { current, .. })) => {
                let sent = in_whole_file(self.take(current), sent);
                let merged = self.body().to_owned();
                // The editor merges the outside edit in when it shows the
                // note. Until then the session's text carries it too, so a
                // save of edits made meanwhile can't take it out again.
                // Where they overlap, the edits typed meanwhile win, as in
                // the editor.
                self.text = if self.text == sent {
                    merged.clone()
                } else {
                    local_core::merge_text(&sent, &self.text, &merged)
                        .unwrap_or_else(|| std::mem::take(&mut self.text))
                };
                self.saved(version, sent, Some(merged));
                return true;
            }
            Written::Text(Ok(RawSaveResult::Conflict { current, .. })) => {
                tracing::info!("an outside edit changed the text a note's typing changed");
                self.conflicted(current.body, current.revision);
            }
            Written::Properties(Ok((count, read))) => {
                self.property_edits
                    .drain(..count.min(self.property_edits.len()));
                let body = self.body().to_owned();
                let frontmatter = self.take(read);
                let body = in_whole_file(frontmatter.clone(), body);
                let sent = in_whole_file(frontmatter, sent);
                self.problem = None;
                if self.body() != body {
                    // The edits went into the file as another program left
                    // it, with an edit of its text that the editor merges
                    // in. A property write starts only while the editor's
                    // text is on disk, so `sent` is the text it replaced.
                    let merged = self.body().to_owned();
                    self.text = if self.text == sent {
                        merged.clone()
                    } else {
                        local_core::merge_text(&sent, &self.text, &merged)
                            .unwrap_or_else(|| std::mem::take(&mut self.text))
                    };
                    self.saved(version, sent, Some(merged));
                }
            }
            Written::PropertyChanged(key) => {
                tracing::warn!("a property edit met an outside edit of the same property");
                self.property_failed(format!(
                    "Another program changed \u{201c}{key}\u{201d} while you were editing it."
                ));
            }
            Written::Properties(Err(error)) => {
                tracing::warn!(code = %error.code, operation = %error.operation, "a property edit didn't save");
                self.property_failed(save_reason(&error));
            }
            Written::Text(Err(error)) if is_gone(&error) => {
                tracing::warn!(code = %error.code, operation = %error.operation, "a note's file is gone");
                self.gone(now);
            }
            Written::Text(Err(error)) => {
                tracing::warn!(code = %error.code, operation = %error.operation, "a note didn't save");
                self.failed(save_reason(&error), now);
            }
        }
        false
    }

    /// Another app changed the text you typed in: the file holds `file`
    /// at `revision`. Saves wait until you choose a version. The review
    /// opens the first time, and stays as you left it when the file
    /// changes again.
    fn conflicted(&mut self, file: String, revision: String) {
        let shown = self.shown_part(&file).to_owned();
        let reviewing = self
            .conflict
            .as_ref()
            .is_none_or(|conflict| conflict.reviewing);
        let confirming = self
            .conflict
            .as_ref()
            .and_then(|conflict| conflict.confirming);
        self.conflict = Some(NoteConflict {
            file,
            revision,
            shown,
            reviewing,
            confirming,
            problem: None,
        });
        self.problem = Some(SaveProblem {
            reason: CONFLICT_REASON.to_owned(),
        });
    }

    /// The part of `file`, a whole LF text, that the editor shows: after
    /// the frontmatter of a managed note, or all of it.
    fn shown_part<'a>(&self, file: &'a str) -> &'a str {
        if self.whole_file {
            return file;
        }
        local_core::frontmatter_end(file).map_or(file, |end| &file[end..])
    }

    /// The file holds `sent`, the editor's text at `version`.
    fn saved(&mut self, version: usize, sent: String, merged: Option<String>) {
        self.conflict = None;
        self.deleted = false;
        self.restore_problem = None;
        // Property edits still waiting keep the problem until they're
        // written, so a text save never hides a property edit that can't
        // go in.
        if self.property_edits.is_empty() {
            self.problem = None;
        }
        self.sync = Some(NoteSync {
            saved: (version, sent),
            merged,
        });
        // Edits made while the save ran wait their own turn.
        if self.text != self.body() && self.first_unsaved.is_none() {
            self.first_unsaved = self.last_edit;
        }
        self.reopen_if_fixed();
    }

    /// Show the file as it would open now, once nothing is unsaved. A
    /// Markdown file shown whole whose properties hold an ID and a type
    /// shows as a note with its properties panel: the file opened with
    /// them broken, they broke while you typed, or another app broke them.
    /// A file shown as source for its broken properties whose properties
    /// now read without an ID shows in the live preview. A move that
    /// changed the file's extension shows it as its new kind. The editor
    /// starts again from the note's text, as [`Self::take_reset`] tells it,
    /// which writes nothing. Edits typed while the save ran wait for their
    /// own save first, so the text the editor shows always maps to the file
    /// the same way.
    fn reopen_if_fixed(&mut self) {
        if self.is_unsaved() {
            return;
        }
        let same_kind = self.text_file != is_markdown(&self.relative_path);
        if same_kind {
            if self.text_file || !self.whole_file || self.broken.is_some() {
                return;
            }
            let managed = matches!(
                local_core::parse_markdown(&self.relative_path, self.base.as_bytes()),
                ParsedMarkdown::Managed(_)
            );
            if !managed && !self.broken_layout {
                return;
            }
        }
        self.lay_out();
        self.sync = None;
        self.reset = Some(self.text.clone());
    }

    /// Another program deleted the file, or moved a file Noura can't
    /// follow: the note keeps its text and offers Restore, and saves wait.
    fn gone(&mut self, now: Instant) {
        self.deleted = true;
        self.conflict = None;
        self.problem = Some(SaveProblem {
            reason: GONE_REASON.to_owned(),
        });
        self.first_unsaved.get_or_insert(now);
    }

    /// Write the note back where its file was, after another program
    /// deleted it. Saves run again afterwards. When a file is there again,
    /// nothing is written.
    pub(crate) fn restore(&mut self, engine: &WorkspaceEngine) -> Restored {
        self.restore_problem = None;
        let body = format!("{}{}", &self.base[..self.body_start], self.text);
        let restored = match engine.restore_raw_markdown(RawRestoreInput {
            relative_path: self.relative_path.clone(),
            body,
            uses_crlf: self.uses_crlf,
            has_bom: self.has_bom,
        }) {
            Ok(restored) => restored,
            Err(error) if error.code == "raw_markdown_exists" => return Restored::FileBack,
            Err(error) => {
                tracing::warn!(code = %error.code, operation = %error.operation, "a note didn't restore");
                self.restore_problem = Some(save_reason(&error));
                return Restored::Failed;
            }
        };
        self.deleted = false;
        self.problem = None;
        self.waiting_for_edit = false;
        self.forced = false;
        self.first_unsaved = None;
        self.last_edit = None;
        self.take(restored);
        self.sync = Some(NoteSync {
            saved: (self.version, self.text.clone()),
            merged: None,
        });
        self.reopen_if_fixed();
        Restored::Written
    }

    /// Why Restore didn't write the file back, until the next try.
    pub fn restore_problem(&self) -> Option<&str> {
        self.restore_problem.as_deref()
    }

    fn failed(&mut self, reason: String, now: Instant) {
        self.problem = Some(SaveProblem { reason });
        self.waiting_for_edit = true;
        self.first_unsaved.get_or_insert(now);
    }

    /// A property write failed: the property edits wait for the next edit
    /// or Try again, and text typed meanwhile keeps its schedule.
    fn property_failed(&mut self, reason: String) {
        self.problem = Some(SaveProblem { reason });
        self.properties_held = true;
    }

    /// Drop the edits that aren't on disk, leaving the file as it is. The
    /// editor drops them too, as [`Self::take_reset`] tells it, in case the
    /// note stays open.
    pub(crate) fn discard(&mut self) {
        if let Some(conflict) = self.conflict.take() {
            // The file holds another app's version now, which the note
            // shows.
            self.read(conflict.file, conflict.revision);
        }
        if self.text != self.body() {
            self.text = self.body().to_owned();
            self.reset = Some(self.text.clone());
            self.sync = None;
        }
        self.property_edits.clear();
        self.problem = None;
        self.waiting_for_edit = false;
        self.properties_held = false;
        self.forced = false;
        self.first_unsaved = None;
        self.last_edit = None;
    }

    pub(crate) fn take_sync(&mut self) -> Option<NoteSync> {
        self.sync.take()
    }

    pub(crate) fn take_reset(&mut self) -> Option<String> {
        self.reset.take()
    }

    /// Queue `value` for the property `key`, over the value the panel
    /// shows for it now.
    pub(crate) fn queue_property(&mut self, key: String, value: serde_json::Value) {
        let base = match self
            .property_edits
            .iter()
            .rev()
            .find(|edit| edit.key == key)
        {
            Some(earlier) => Some(earlier.value.clone()),
            None => self
                .managed
                .as_ref()
                .and_then(|managed| managed.properties.get(&key).cloned()),
        };
        self.property_edits.push(PropertyEdit { key, value, base });
        self.waiting_for_edit = false;
        self.properties_held = false;
    }

    /// Whether the panel edits the property `key` in place.
    pub(crate) fn has_property(&self, key: &str) -> bool {
        self.managed
            .as_ref()
            .is_some_and(|managed| managed.edits_in_place() && managed.properties.contains_key(key))
            && !NOURA_PROPERTIES.contains(&key)
    }

    pub(crate) fn set_name_problem(&mut self, problem: Option<String>) {
        self.name_problem = problem;
    }

    /// Read the file again when it changed since Noura read it and the
    /// note has nothing unsaved, as when another program moved a note over
    /// it. The editor takes the file's text, as [`Self::take_reset`] tells
    /// it. Returns whether the note changed.
    pub(crate) fn reread(&mut self, engine: &WorkspaceEngine) -> bool {
        if self.is_unsaved() {
            return false;
        }
        let Ok(fresh) = Self::open(engine, &self.relative_path) else {
            return false;
        };
        if fresh.revision == self.revision {
            return false;
        }
        self.replace_with(fresh);
        true
    }

    /// Take `fresh`, the file opened again, in place of this session. The
    /// editor takes its text, as [`Self::take_reset`] tells it.
    fn replace_with(&mut self, fresh: Self) {
        let text = fresh.text.clone();
        *self = Self {
            version: self.version,
            reset: Some(text),
            ..fresh
        };
    }

    /// Follow the file to `relative_path`. A move that turns a plain-text
    /// or code file into a Markdown file, or the other way, as when the
    /// name field renames `LICENSE` to `LICENSE.md`, shows the file as its
    /// new kind once nothing is unsaved, as [`Self::reopen_if_fixed`] says.
    /// Noura's own renames and moves save first, so that's at once.
    pub(crate) fn moved_to(&mut self, relative_path: &str) {
        relative_path.clone_into(&mut self.relative_path);
        self.reopen_if_fixed();
    }

    /// Look at the file after the files changed on disk. A note with
    /// nothing unsaved takes what another app wrote, which the editor shows
    /// as an outside edit, as [`Self::take_reset`] tells it. A note with
    /// unsaved edits meets the other app's edit at its next save, and
    /// learns at once when its file went away or came back.
    pub(crate) fn file_changed(&mut self, engine: &WorkspaceEngine) -> FileCheck {
        if self.is_saving() {
            // The save's result tells whether it met an outside edit made
            // before it read the file; look again once it's in.
            self.look_again = true;
            return FileCheck::Same;
        }
        self.look_again = false;
        if self.text_mode.is_some() {
            // The editor hears of the file's changes from the document.
            return FileCheck::Same;
        }
        if !self.is_unsaved() {
            return if self.reread(engine) {
                FileCheck::Shown
            } else {
                FileCheck::Same
            };
        }
        let read = match engine.read_raw_markdown(&self.relative_path) {
            Ok(read) => read,
            Err(error) if is_gone(&error) && !self.deleted => {
                self.gone(Instant::now());
                return FileCheck::Shown;
            }
            Err(_) => return FileCheck::Same,
        };
        if self.deleted {
            // The file came back: its text meets your typing as another
            // app's edit would.
            self.deleted = false;
            self.problem = None;
            self.restore_problem = None;
            return FileCheck::Changed;
        }
        match &self.conflict {
            // The review compares an older version of the file: look
            // again, in case the edits no longer meet.
            Some(conflict) if read.revision != conflict.revision => FileCheck::Changed,
            // The next save, due once typing pauses, merges another app's
            // edit or finds a conflict.
            _ => FileCheck::Same,
        }
    }

    /// Ask for confirmation of `choice`, or take the question back with
    /// `None`. Returns whether the note has a conflict.
    pub(crate) fn set_confirming(&mut self, choice: Option<ConflictChoice>) -> bool {
        let Some(conflict) = &mut self.conflict else {
            return false;
        };
        conflict.confirming = choice;
        if choice.is_some() {
            conflict.reviewing = true;
        }
        conflict.problem = None;
        true
    }

    /// Keep the version you confirmed. Both choices first save the version
    /// they replace as a recovery snapshot in the workspace's history
    /// folder: Use my version writes your text over the file, and Use the
    /// file's version shows the file's text in the note. Saves then run
    /// again. When the file changed once more meanwhile, the review shows
    /// its newest version and you choose again.
    pub(crate) fn resolve(&mut self, engine: &WorkspaceEngine) {
        let Some(conflict) = &self.conflict else {
            return;
        };
        let Some(choice) = conflict.confirming else {
            return;
        };
        let input = RawConflictResolveInput {
            relative_path: self.relative_path.clone(),
            current_revision: conflict.revision.clone(),
            local_body: self.mine_over(&conflict.file),
            resolution: match choice {
                ConflictChoice::Mine => ConflictResolution::ReplaceExternal,
                ConflictChoice::File => ConflictResolution::UseExternal,
            },
        };
        match engine.resolve_raw_conflict(input) {
            Ok(resolved) => {
                self.conflict = None;
                self.problem = None;
                self.waiting_for_edit = false;
                self.forced = false;
                let lost = self.take(resolved.current).is_some();
                match choice {
                    // The file holds your text now. When it holds no
                    // managed note anymore, the editor shows all of it,
                    // with the properties another app broke.
                    ConflictChoice::Mine => {
                        if lost {
                            self.text = self.body().to_owned();
                        }
                        self.sync = Some(NoteSync {
                            saved: (self.version, self.text.clone()),
                            merged: None,
                        });
                    }
                    ConflictChoice::File => {
                        self.text = self.body().to_owned();
                        self.reset = Some(self.text.clone());
                        self.first_unsaved = None;
                        self.last_edit = None;
                    }
                }
                self.reopen_if_fixed();
            }
            Err(error) if error.code == "revision_conflict" => {
                tracing::info!("the file changed again during conflict review");
                if let Ok(read) = engine.read_raw_markdown(&self.relative_path) {
                    self.conflicted(read.body, read.revision);
                }
                self.set_confirming(None);
            }
            Err(error) => {
                tracing::warn!(code = %error.code, operation = %error.operation, "a conflict didn't resolve");
                if let Some(conflict) = &mut self.conflict {
                    conflict.confirming = None;
                    conflict.problem = Some(save_reason(&error));
                }
            }
        }
    }

    /// Your version as the whole file: your text after the frontmatter of
    /// `file`, the file's version, so a property another app changed
    /// stays. A file that lost its frontmatter gets the one Noura read.
    fn mine_over(&self, file: &str) -> String {
        if self.whole_file {
            return self.text.clone();
        }
        let frontmatter = match local_core::frontmatter_end(file) {
            Some(end) => &file[..end],
            None => &self.base[..self.body_start],
        };
        format!("{frontmatter}{}", self.text)
    }

    /// Open or close the conflict review. Closing it drops a choice
    /// waiting for confirmation.
    pub(crate) fn set_reviewing(&mut self, reviewing: bool) -> bool {
        let Some(conflict) = &mut self.conflict else {
            return false;
        };
        conflict.reviewing = reviewing;
        conflict.confirming = None;
        true
    }

    /// Whether the file changed while a save ran, so it needs another look
    /// now that the save is in.
    pub(crate) fn take_look_again(&mut self) -> bool {
        std::mem::take(&mut self.look_again)
    }
}

/// What [`NoteSession::restore`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Restored {
    /// The note's text is in its file again.
    Written,
    /// A file is at the path again, and nothing was written.
    FileBack,
    /// The write failed; [`NoteSession::restore_problem`] says why.
    Failed,
}

/// What [`NoteSession::file_changed`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileCheck {
    /// Nothing the note shows changed.
    Same,
    /// The note now shows the file's new text, or that the file is gone.
    Shown,
    /// The file came back, or changed again during a conflict: a save
    /// looks at once.
    Changed,
}

/// A write a worker carries out for a note.
#[derive(Debug, Clone)]
pub(crate) enum Write {
    /// The file's whole new text, merged with outside edits made since
    /// `base_revision`.
    Text(RawSaveInput),
    /// Property edits for the file at `relative_path`, which go into the
    /// file as it is when the write runs, but only while its edited
    /// properties still hold the values in `before`. `count` edits are in
    /// the patch.
    Properties {
        relative_path: String,
        before: BTreeMap<String, Option<serde_json::Value>>,
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
    /// Another program changed this edited property, and its value stays.
    PropertyChanged(String),
}

impl Write {
    /// Carry out the write. Returns once the bytes are durably on disk, or
    /// the write failed.
    pub(crate) fn run(self, engine: &WorkspaceEngine) -> Written {
        match self {
            Self::Text(input) => Written::Text(engine.save_raw_markdown(input)),
            Self::Properties {
                relative_path,
                before,
                patch,
                count,
            } => {
                // The file as it is now: another program may have changed it
                // since Noura read it, and a text save may have merged that
                // in since you made the edits.
                let current = match engine.read_raw_markdown(&relative_path) {
                    Ok(current) => current,
                    Err(error) => return Written::Properties(Err(error)),
                };
                if let Some(key) = changed_property(&relative_path, &current.body, &before) {
                    return Written::PropertyChanged(key);
                }
                // The edits go into the file as it is now, keeping every
                // other change, and only while it still holds what Noura
                // just read.
                let patch = ObjectPatch {
                    expected_revision: current.revision,
                    ..patch
                };
                Written::Properties(match engine.update_object_at(&relative_path, patch) {
                    Ok(_) => engine
                        .read_raw_markdown(&relative_path)
                        .map(|read| (count, read)),
                    Err(error) => Err(error),
                })
            }
        }
    }
}

/// The first property in `values` to which the managed note `text` no
/// longer gives that value, `None` standing for no such property. `None`
/// when they all hold, or when `text` holds no managed note, which the
/// write then reports.
fn changed_property(
    relative_path: &str,
    text: &str,
    values: &BTreeMap<String, Option<serde_json::Value>>,
) -> Option<String> {
    match local_core::parse_markdown(relative_path, text.as_bytes()) {
        ParsedMarkdown::Managed(object) => values
            .iter()
            .find(|(key, value)| object.properties.get(*key) != value.as_ref())
            .map(|(key, _)| key.clone()),
        _ => None,
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
    /// How long a save that must finish first may take.
    pub(crate) flush_timeout: Duration,
}

impl Default for Notes {
    fn default() -> Self {
        let (sender, results) = channel();
        Self {
            sessions: HashMap::new(),
            unreadable: HashMap::new(),
            sender,
            results,
            flush_timeout: FLUSH_TIMEOUT,
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
            "Noura can't read this file as text because it isn't UTF-8.",
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
        _ if is_gone(error) => GONE_REASON.to_owned(),
        "file_not_downloaded" => {
            "The file is online-only now. Download it, then try again.".to_owned()
        }
        "invalid_utf8" => "The file on disk isn't UTF-8 text Noura can read anymore.".to_owned(),
        "object_parse_failed" => "Noura can't read the file's properties anymore.".to_owned(),
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

/// Why frontmatter failed to parse with `error`, `workspace-format`'s
/// message, in plain words. The conformance fixtures pin how each message
/// starts.
fn properties_problem(error: &str) -> &'static str {
    if error.starts_with("Frontmatter has no closing delimiter") {
        "The properties have no closing --- line."
    } else if error.starts_with("Invalid YAML frontmatter") {
        "The properties aren\u{2019}t valid YAML."
    } else if error.starts_with("Managed frontmatter requires a string type") {
        "The properties have an ID but no type."
    } else if error.starts_with("The stable ID does not match the object type") {
        "The ID doesn\u{2019}t match the type."
    } else {
        "Noura can\u{2019}t read the properties."
    }
}

/// `text`, which follows the frontmatter of a managed note, as the whole
/// file's text once [`NoteSession::read`] returned that `frontmatter`.
fn in_whole_file(frontmatter: Option<String>, text: String) -> String {
    match frontmatter {
        Some(mut whole) => {
            whole.push_str(&text);
            whole
        }
        None => text,
    }
}

/// `file_name` split before its extension's dot: `config.json` into
/// `config` and `.json`. A name with no dot past its first character has no
/// extension.
fn split_extension(file_name: &str) -> (&str, &str) {
    match file_name.rfind('.') {
        Some(dot) if dot > 0 => file_name.split_at(dot),
        _ => (file_name, ""),
    }
}

/// Whether the file at `relative_path` opens in the editor: a Markdown file,
/// or a plain-text or code file.
pub(crate) fn opens_in_editor(relative_path: &str) -> bool {
    is_markdown(relative_path) || local_core::is_plain_text_path(relative_path)
}

/// Whether the file at `relative_path` is Markdown, which opens as a note.
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
                error("file_not_downloaded", ErrorCategory::Filesystem),
                "The file is online-only now. Download it, then try again.",
            ),
            (
                error("invalid_utf8", ErrorCategory::Parse),
                "The file on disk isn't UTF-8 text Noura can read anymore.",
            ),
            (
                error("object_parse_failed", ErrorCategory::Parse),
                "Noura can't read the file's properties anymore.",
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
        assert!(is_civil_date("2028-02-29"));
        assert!(!is_civil_date("2026-10-21T10:00:00Z"));
        assert!(!is_civil_date("21-10-2026"));
        assert!(!is_civil_date("2026-99-99"));
        assert!(!is_civil_date("2026-02-30"));
        assert_eq!(
            PropertyValue::of(&serde_json::json!("2026-99-99")),
            PropertyValue::Text("2026-99-99".into())
        );
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
