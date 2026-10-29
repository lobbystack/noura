//! Notes, tasks, projects and other Markdown files as an AI tool outside
//! Noura reads them, and the notes it creates and changes. Every path
//! stays inside the workspace, and every change goes through `local-core`,
//! which checks the revision the tool read and writes durably before it
//! returns.

use std::collections::BTreeMap;

use local_core::{CoreError, CreateObjectInput, ObjectPatch, ParsedMarkdown};

use crate::{
    AppError, Workspace,
    error::Access,
    operations::{free_name, join, name_problem, name_taken},
};

/// The object type of notes.
const NOTE: &str = "note";

/// A Markdown file as an AI tool reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The stable ID of a managed object, such as a note or a task.
    pub id: Option<String>,
    /// The managed object's type, such as `note` or `task`.
    pub object_type: Option<String>,
    /// The file's name without its extension.
    pub title: String,
    pub relative_path: String,
    /// The revision of the file's bytes, which a change must name.
    pub revision: String,
    /// A managed object's properties, by name. None for a plain Markdown
    /// file.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// The text after a managed object's properties, or the whole file.
    pub body: String,
}

/// What [`Workspace::update_note`] changes in a note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoteChanges {
    /// A new title, which renames the file in its folder.
    pub title: Option<String>,
    /// A new body, after the properties.
    pub body: Option<String>,
    /// Properties to set, by name. A `null` value removes the property.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// The revision the caller read. The note changes only while its file
    /// still holds it.
    pub expected_revision: String,
}

impl Workspace {
    /// The managed object with stable ID `id`, read from its file
    /// wherever it is now.
    pub fn read_item(&self, id: &str) -> Result<Item, AppError> {
        let object = self
            .engine()
            .read_object(id)
            .map_err(|error| object_error(error, id))?;
        self.read_item_at(&object.relative_path)
    }

    /// The Markdown or plain-text file at `relative_path`, which must be
    /// inside the workspace.
    pub fn read_item_at(&self, relative_path: &str) -> Result<Item, AppError> {
        let read = self
            .engine()
            .read_raw_markdown(relative_path)
            .map_err(|error| read_error(error, relative_path))?;
        let title = local_core::title_from_path(relative_path);
        Ok(
            match local_core::parse_markdown(relative_path, read.body.as_bytes()) {
                ParsedMarkdown::Managed(object) => Item {
                    id: Some(object.id),
                    object_type: Some(object.object_type),
                    title,
                    relative_path: relative_path.to_owned(),
                    revision: read.revision,
                    properties: object.properties,
                    body: object.body,
                },
                _ => Item {
                    id: None,
                    object_type: None,
                    title,
                    relative_path: relative_path.to_owned(),
                    revision: read.revision,
                    properties: BTreeMap::new(),
                    body: read.body,
                },
            },
        )
    }

    /// Create a managed note titled `title` holding `body`, in `folder`
    /// (`""` is the top of the workspace). The title follows the names
    /// the file tree accepts and must be free in the folder. Returns the
    /// note once it is on disk.
    pub fn create_named_note(
        &self,
        folder: &str,
        title: &str,
        body: &str,
    ) -> Result<Item, AppError> {
        name_problem(title)?;
        let directory = self.folder_path(folder)?;
        let name = format!("{title}.md");
        if free_name(&directory, title, ".md") != name {
            return Err(name_taken(title, &place(folder)));
        }
        let relative_path = self.create_note_file(folder, &name, body)?;
        self.read_item_at(&relative_path)
    }

    /// Create the managed note `name`, such as `Plan.md`, in `folder`.
    /// Returns its workspace path once it is on disk.
    pub(crate) fn create_note_file(
        &self,
        folder: &str,
        name: &str,
        body: &str,
    ) -> Result<String, AppError> {
        let relative_path = join(folder, name);
        self.engine()
            .create_object(CreateObjectInput {
                object_type: NOTE.to_owned(),
                title: local_core::title_from_path(name),
                body: body.to_owned(),
                relative_path: Some(relative_path.clone()),
                properties: BTreeMap::new(),
            })
            .map_err(|error| AppError::from_change(error, &relative_path))?;
        Ok(relative_path)
    }

    /// Change the note with stable ID `id`, while its file still holds
    /// `changes.expected_revision`. A new title renames its file and keeps
    /// its ID. Anything other than a note, and a file that changed since,
    /// stays as it is. Returns the note once its file is on disk.
    pub fn update_note(&self, id: &str, changes: &NoteChanges) -> Result<Item, AppError> {
        let current = self.read_item(id)?;
        if current.object_type.as_deref() != Some(NOTE) {
            return Err(AppError::new(
                "not_a_note",
                format!(
                    "{} isn't a note, so Noura left it as it was.",
                    current.relative_path
                ),
            ));
        }
        if let Some(title) = &changes.title {
            name_problem(title)?;
        }
        let mut properties = BTreeMap::new();
        let mut remove_properties = Vec::new();
        for (key, value) in &changes.properties {
            if value.is_null() {
                remove_properties.push(key.clone());
            } else {
                properties.insert(key.clone(), value.clone());
            }
        }
        let patch = ObjectPatch {
            title: changes.title.clone(),
            body: changes.body.clone(),
            properties,
            remove_properties,
            expected_revision: changes.expected_revision.clone(),
        };
        let updated = self.engine().update_object(id, patch).map_err(|error| {
            if error.code == "path_exists"
                && let Some(title) = &changes.title
            {
                return name_taken(title, "in this folder");
            }
            change_error(error, &current.relative_path)
        })?;
        self.read_item_at(&updated.value.relative_path)
    }
}

/// Where `folder` is, as a sentence names it.
fn place(folder: &str) -> String {
    match folder {
        "" => "at the top of the workspace".to_owned(),
        _ => format!(
            "in \u{201c}{}\u{201d}",
            folder.rsplit('/').next().unwrap_or(folder)
        ),
    }
}

/// Why the object `id` couldn't be read.
fn object_error(error: CoreError, id: &str) -> AppError {
    match error.code.as_str() {
        "object_not_found" | "invalid_object_id" => {
            tracing::warn!(code = %error.code, "an item wasn't found");
            AppError::new("object_not_found", format!("No item has the ID {id}."))
        }
        "identity_conflict" => {
            tracing::warn!(code = %error.code, "an item's ID is in more than one file");
            AppError::new(
                "identity_conflict",
                "More than one file uses this ID, so Noura won't change any of them. Remove \
                 the id line from the copy, then try again.",
            )
        }
        _ => AppError::from_core(error, id, Access::ReadFile),
    }
}

/// Why the file at `relative_path` couldn't be read.
fn read_error(error: CoreError, relative_path: &str) -> AppError {
    let message = match error.code.as_str() {
        "empty_path" | "absolute_path" | "unsafe_path" => {
            "Noura only reads files inside this workspace.".to_owned()
        }
        "raw_markdown_missing" | "file_not_found" => {
            format!("{relative_path} isn't in the workspace.")
        }
        "invalid_raw_markdown_path" => {
            format!("{relative_path} isn't a Markdown or plain-text file.")
        }
        "invalid_utf8" => format!("{relative_path} isn't UTF-8 text, so Noura can't read it."),
        _ => return AppError::from_core(error, relative_path, Access::ReadFile),
    };
    tracing::warn!(code = %error.code, operation = %error.operation, "an item wasn't read");
    AppError::new(&error.code, message)
}

/// Why a change to the file at `relative_path` failed. A file that changed
/// since the caller read it says so.
fn change_error(error: CoreError, relative_path: &str) -> AppError {
    if error.code == "revision_conflict" {
        tracing::warn!(code = %error.code, "a change met a file that changed since");
        return AppError::new(
            "revision_conflict",
            format!(
                "{relative_path} changed since you read it, so Noura left it as it was. Read \
                 it again, then make the change."
            ),
        );
    }
    AppError::from_change(error, relative_path)
}
