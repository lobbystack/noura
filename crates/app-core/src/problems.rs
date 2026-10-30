//! Workspace problems you can fix yourself, such as files that share an ID,
//! properties Noura can't read and names it can't represent.

use std::collections::BTreeMap;

use local_core::{WorkspaceEngine, WorkspaceObject};
use plugin_host::ObjectTypes;

use crate::{AppError, error::Access};

/// One problem in the open workspace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Problem {
    /// What kind of problem this is, such as `identity_conflict`,
    /// `parse_error` or `unsupported_path`.
    pub code: String,
    /// The problem in plain words.
    pub message: String,
    /// The workspace-relative paths of the files and folders it names.
    pub paths: Vec<String>,
    /// The object ID involved, if any.
    pub object_id: Option<String>,
}

/// The workspace's problems, as the index and the last full scan found
/// them, sorted and without repeats. `root` is the workspace folder, for
/// the error.
pub(crate) fn collect(engine: &WorkspaceEngine, root: &str) -> Result<Vec<Problem>, AppError> {
    let mut problems: Vec<Problem> = engine
        .state()
        .diagnostics
        .into_iter()
        // The index lists shared IDs with every path in one string; the
        // conflicts below name each file on its own.
        .filter(|diagnostic| diagnostic.code != "identity_conflict")
        .map(|diagnostic| Problem {
            message: in_plain_words(
                &diagnostic.code,
                diagnostic.relative_path.as_deref(),
                diagnostic.message,
            ),
            code: diagnostic.code,
            paths: diagnostic.relative_path.into_iter().collect(),
            object_id: diagnostic.object_id,
        })
        .collect();
    let conflicts = engine
        .identity_conflicts()
        .map_err(|error| AppError::from_core(error, root, Access::ReadWorkspace))?;
    for conflict in conflicts {
        problems.push(Problem {
            code: "identity_conflict".into(),
            message: shared_id(&conflict.relative_paths),
            paths: conflict.relative_paths,
            object_id: Some(conflict.object_id),
        });
    }
    problems.sort();
    problems.dedup();
    Ok(problems)
}

/// The objects whose properties don't fit the schema of the running plugin
/// that holds their type, one problem for each, naming the first property
/// in the schema's order that fails. The kernel holds no rules of its own
/// for any type: with no plugin running there are none. `names` gives the
/// name people see of each plugin, by plugin ID. Reads the index, which
/// keeps each object's properties, and no file.
pub(crate) fn schema(
    engine: &WorkspaceEngine,
    types: &ObjectTypes,
    names: &BTreeMap<String, String>,
) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (object_type, plugin) in types.held() {
        let objects = match engine.object_summaries_of_type(&object_type) {
            Ok(objects) => objects,
            Err(error) => {
                tracing::warn!(code = %error.code, "couldn't check objects against their schema");
                continue;
            }
        };
        let plugin = names.get(&plugin).unwrap_or(&plugin);
        for summary in objects {
            let object = WorkspaceObject {
                id: summary.id,
                object_type: summary.object_type,
                title: summary.title,
                body: String::new(),
                relative_path: summary.relative_path,
                revision: summary.revision,
                created: summary.created,
                updated: summary.updated,
                properties: summary.properties,
            };
            if let Some(problem) = types.problem(&object) {
                problems.push(Problem {
                    code: "invalid_property".into(),
                    message: format!(
                        "{} has a {} property that the {plugin} plugin doesn't accept.",
                        object.relative_path, problem.property
                    ),
                    paths: vec![object.relative_path],
                    object_id: Some(object.id),
                });
            }
        }
    }
    problems
}

/// `listed`, the problems a listing found, with `schema`'s, sorted and
/// without repeats.
pub(crate) fn with_schema(listed: &[Problem], schema: Vec<Problem>) -> Vec<Problem> {
    let mut problems = listed.to_vec();
    problems.extend(schema);
    problems.sort();
    problems.dedup();
    problems
}

/// The problem `code` that the index or the scan reported at `path`, in
/// words that name the file and leave out the details the log keeps, such
/// as the parser's error or the missing ID. A file that links to several
/// missing objects makes one problem this way.
fn in_plain_words(code: &str, path: Option<&str>, message: String) -> String {
    match (code, path) {
        ("parse_error", Some(path)) => {
            format!("Noura can't read the properties at the top of {path}.")
        }
        ("broken_reference", Some(path)) => {
            format!("{path} links to an object that no longer exists.")
        }
        ("unreadable_file", Some(path)) => {
            format!("Noura can't open {path}. Check that you have permission to read it.")
        }
        ("unreadable_folder", Some(path)) => {
            format!(
                "Noura can't open the folder {path}. Check that you have permission to read it."
            )
        }
        ("unsupported_path", folder) => {
            let place = folder.map_or_else(
                || "at the top of the workspace".to_owned(),
                |folder| format!("in {folder}"),
            );
            format!(
                "A name {place} has characters Noura can't represent, so Noura leaves it as \
                 it is. Rename it in another app to use it in Noura."
            )
        }
        _ => message,
    }
}

fn shared_id(paths: &[String]) -> String {
    let copies = if paths.len() > 2 { "copies" } else { "copy" };
    format!(
        "{} use the same ID. Remove the id line from the {copies}.",
        listed(paths)
    )
}

/// "a and b", or "a, b and c".
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
