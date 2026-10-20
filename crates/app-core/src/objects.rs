//! Notes, tasks, projects and other objects, found by their stable ID or
//! by the words in them.

use local_core::{CoreError, SearchInput, WorkspaceObject};

use crate::{AppError, Workspace};

/// Where an object lives. Its ID stays the same when another app or Noura
/// moves or renames its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    pub id: String,
    pub object_type: String,
    pub relative_path: String,
}

/// One file that matches a search.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// The ID of the object the file holds, if it has one.
    pub object_id: Option<String>,
    pub relative_path: String,
    pub title: String,
    /// The matching text from the body.
    pub snippet: String,
}

impl From<WorkspaceObject> for ObjectRef {
    fn from(object: WorkspaceObject) -> Self {
        Self {
            id: object.id,
            object_type: object.object_type,
            relative_path: object.relative_path,
        }
    }
}

impl Workspace {
    /// The object with stable ID `id`, wherever its file is now. Two files
    /// that share the ID stop the lookup with the `identity_conflict` error.
    pub fn find_object(&self, id: &str) -> Result<Option<ObjectRef>, AppError> {
        self.engine()
            .get_object(id)
            .map(|object| object.map(ObjectRef::from))
            .map_err(object_error)
    }

    /// Files whose text matches every word of `query`, best match first.
    /// The words are plain text: punctuation never acts as query syntax.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>, AppError> {
        let input = SearchInput {
            query: query.to_owned(),
            ..SearchInput::default()
        };
        let results = self.engine().search(&input).map_err(object_error)?;
        Ok(results
            .into_iter()
            .map(|result| SearchHit {
                object_id: result.object_id,
                relative_path: result.relative_path,
                title: result.title,
                snippet: result.snippet,
            })
            .collect())
    }

    /// Every object whose ID belongs to exactly one file, by ID.
    pub fn objects(&self) -> Result<Vec<ObjectRef>, AppError> {
        let mut objects: Vec<ObjectRef> = self
            .engine()
            .query_objects(None)
            .map_err(object_error)?
            .into_iter()
            .map(ObjectRef::from)
            .collect();
        objects.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(objects)
    }
}

fn object_error(error: CoreError) -> AppError {
    tracing::warn!(code = %error.code, details = ?error.details, "an object lookup failed");
    if error.code == "identity_conflict" {
        return AppError::new(
            "identity_conflict",
            "More than one file uses this ID, so Noura won't change any of them. Remove the \
             id line from the copy, then try again.",
        );
    }
    AppError::new(
        "index_unavailable",
        "Noura couldn't read the workspace index. Try again in a moment.",
    )
}
