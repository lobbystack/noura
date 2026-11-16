//! A task's links: `project`, to its project, and `source`, to the object
//! it came from, such as a Transcription. The file holds each as a link,
//! `[[Garden]]`, which Noura keeps current when the project moves or gets
//! a new title. The plugin reads a link by asking Noura where it leads,
//! and other plugins and commands may pass a stable ID, which it writes as
//! the link Noura gives for it.

use std::collections::BTreeMap;

use plugin_sdk::{Error, workspace};

/// The task fields that hold links.
pub(crate) const LINK_FIELDS: [&str; 2] = ["project", "source"];

/// Resolves links from tasks' files, asking Noura once for each link and
/// folder. Make one for each read of the workspace: Noura keeps links
/// current, so an answer can change between calls.
#[derive(Default)]
pub(crate) struct Resolver {
    resolved: BTreeMap<(String, String), Option<(String, String)>>,
}

impl Resolver {
    /// The stable ID and type of the object `link`, in the file at `from`,
    /// leads to.
    pub(crate) fn object(&mut self, link: &str, from: &str) -> Option<(String, String)> {
        let folder = from.rsplit_once('/').map_or("", |(folder, _)| folder);
        let key = (link.to_owned(), folder.to_owned());
        self.resolved
            .entry(key)
            .or_insert_with(|| {
                workspace::resolve_link(link, Some(from))
                    .ok()
                    .flatten()
                    .map(|object| (object.id, object.object_type))
            })
            .clone()
    }

    /// The stable ID of the project `link`, in the file at `from`, leads
    /// to, if it leads to a project.
    pub(crate) fn project(&mut self, link: &str, from: &str) -> Option<String> {
        self.object(link, from)
            .filter(|(_, object_type)| object_type == "project")
            .map(|(id, _)| id)
    }
}

/// Whether `value` reads as a stable ID: a type, an underscore and a
/// 26-character ULID.
pub(crate) fn is_stable_id(value: &str) -> bool {
    value.rsplit_once('_').is_some_and(|(object_type, ulid)| {
        !object_type.is_empty()
            && ulid.len() == 26
            && ulid.chars().all(|ch| ch.is_ascii_alphanumeric())
    })
}

/// The link a task field holds for `value`: a link stays as it is, a
/// stable ID becomes the link Noura gives for its object, and a title
/// becomes `[[title]]`.
pub(crate) fn link_for(value: &str) -> Result<String, Error> {
    let value = value.trim();
    if value.starts_with("[[") && value.ends_with("]]") {
        Ok(value.to_owned())
    } else if is_stable_id(value) {
        workspace::link_to(value)
    } else {
        Ok(format!("[[{value}]]"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_read_as_ids_and_titles_as_titles() {
        assert!(is_stable_id("project_01j00000000000000000000000"));
        assert!(is_stable_id("reading-list_01j00000000000000000000000"));
        assert!(!is_stable_id("Garden"));
        assert!(!is_stable_id("Garden_shed"));
        assert_eq!(link_for(" [[Garden]] ").unwrap(), "[[Garden]]");
        assert_eq!(link_for("Garden").unwrap(), "[[Garden]]");
    }
}
