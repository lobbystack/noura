//! Workspace problems you can fix yourself, such as files that share an ID,
//! properties Noura can't read and names it can't represent.

use local_core::{CoreError, WorkspaceEngine};

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
/// them, sorted and without repeats.
pub(crate) fn collect(engine: &WorkspaceEngine) -> Result<Vec<Problem>, CoreError> {
    let mut problems: Vec<Problem> = engine
        .state()
        .diagnostics
        .into_iter()
        // The index lists shared IDs with every path in one string; the
        // conflicts below name each file on its own.
        .filter(|diagnostic| diagnostic.code != "identity_conflict")
        .map(|diagnostic| Problem {
            code: diagnostic.code,
            message: diagnostic.message,
            paths: diagnostic.relative_path.into_iter().collect(),
            object_id: diagnostic.object_id,
        })
        .collect();
    for conflict in engine.identity_conflicts()? {
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
