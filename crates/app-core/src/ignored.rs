//! The workspace's Ignored files list: gitignore-style patterns in the
//! workspace manifest that keep folders and files out of the tree and
//! search, on top of the folders Noura always leaves out.

use local_core::ManifestUpdateInput;

use crate::{AppError, Workspace};

/// The patterns as the manifest on disk holds them, and the revision of
/// the manifest they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IgnoreList {
    pub(crate) patterns: Vec<String>,
    pub(crate) revision: String,
}

impl Workspace {
    /// Read the Ignored files list from the manifest on disk.
    pub(crate) fn ignore_list(&self) -> Result<IgnoreList, AppError> {
        let (manifest, revision) = self
            .engine()
            .read_manifest_revision()
            .map_err(|error| AppError::from_change(error, ".noura/workspace.yaml"))?;
        Ok(IgnoreList {
            patterns: manifest.ignore,
            revision,
        })
    }

    /// Save `patterns` to the manifest, unless the manifest changed on disk
    /// since `revision` was read. `local-core` then indexes the workspace
    /// again as Noura's own change, so ignored items leave search. An
    /// invalid pattern saves nothing.
    pub(crate) fn save_ignore_list(
        &self,
        patterns: Vec<String>,
        revision: &str,
    ) -> Result<(), AppError> {
        self.engine()
            .manifest_update(ManifestUpdateInput {
                ignore: Some(patterns),
                expected_revision: Some(revision.to_owned()),
                ..ManifestUpdateInput::default()
            })
            .map_err(|error| AppError::from_change(error, ".noura/workspace.yaml"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use local_core::SearchInput;

    use crate::WorkspaceService;

    fn found(workspace: &crate::Workspace, query: &str) -> Vec<String> {
        workspace
            .engine()
            .search(&SearchInput {
                query: query.to_owned(),
                ..SearchInput::default()
            })
            .unwrap()
            .into_iter()
            .map(|result| result.relative_path)
            .collect()
    }

    #[test]
    fn ignored_items_leave_search_and_come_back_when_the_pattern_goes() {
        let app_data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("drafts")).unwrap();
        std::fs::write(folder.path().join("drafts/Idea.md"), "# Idea\n\nzucchini\n").unwrap();
        std::fs::write(folder.path().join("Kept.md"), "# Kept\n\nzucchini\n").unwrap();
        let service = WorkspaceService::new(app_data.path());
        let workspace = service.create("Garden", folder.path()).unwrap();
        assert_eq!(found(&workspace, "zucchini").len(), 2);

        let list = workspace.ignore_list().unwrap();
        workspace
            .save_ignore_list(vec!["drafts/".into()], &list.revision)
            .unwrap();
        assert_eq!(found(&workspace, "zucchini"), ["Kept.md"]);

        let list = workspace.ignore_list().unwrap();
        workspace
            .save_ignore_list(Vec::new(), &list.revision)
            .unwrap();
        assert_eq!(found(&workspace, "zucchini").len(), 2);
    }
}
