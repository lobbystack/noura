//! The Files page: one folder of the workspace as a list, the folder the
//! file tree's current row names, or the one you went into from the page.
//! The page and the tree stay in step: going into a folder on the page
//! makes it the tree's current row, and a folder that becomes the tree's
//! current row is the one the page lists.

use super::App;
use crate::{
    Change, FilesEntry, FilesListing, FolderStep, OpenAs, files::parent_of, rail::FILES_PAGE,
    viewers::opens_in_tab,
};

impl App {
    /// What the Files page lists now, or `None` without a workspace.
    pub fn files_listing(&mut self) -> Option<&FilesListing> {
        self.follow_tree_folder();
        let current = self
            .files_listing
            .as_ref()
            .is_some_and(|listing| listing.folder == self.files_folder);
        if !current {
            self.files_listing = self.list_files_folder();
        }
        self.files_listing.as_ref()
    }

    /// Go into the folder at `relative_path` on the Files page (`""` is
    /// the top of the workspace). It becomes the tree's current row, and
    /// the tree opens it and the folders above it, so the tree shows what
    /// the page lists.
    pub fn show_files_folder(&mut self, relative_path: &str) {
        let is_folder = relative_path.is_empty()
            || self
                .tree
                .as_ref()
                .is_some_and(|tree| tree.is_folder(relative_path));
        if !is_folder {
            return;
        }
        self.files_folder = relative_path.to_owned();
        self.change_tree(|tree| {
            if relative_path.is_empty() {
                tree.clear_focus();
            } else {
                tree.expand_folders([relative_path]);
                tree.select(relative_path);
            }
            Some(())
        });
        self.listeners.emit(Change::Tree);
    }

    /// Open the file at `relative_path` from the Files page, as a click in
    /// the tree opens it. The Files tab stays: when it was the preview,
    /// it is kept, so the file's preview opens beside it rather than in
    /// its place. A file that opens in no tab only becomes the tree's
    /// current row, as in the tree.
    pub fn open_from_files(&mut self, relative_path: &str, how: OpenAs) {
        if !opens_in_tab(relative_path) {
            self.change_tree(|tree| {
                tree.select(relative_path);
                Some(())
            });
            self.listeners.emit(Change::Tree);
            return;
        }
        let files_preview = self
            .tabs
            .active()
            .filter(|tab| tab.page_id() == Some(FILES_PAGE) && tab.is_preview())
            .map(|tab| tab.id());
        if let Some(tab) = files_preview {
            self.keep_tab(tab);
        }
        self.open_file(relative_path, how);
    }

    /// Whether the sidebar shows the file tree: always on the Files page,
    /// for a file's tab and with no tab open, and on another page when the
    /// Files in every sidebar setting is on or the page has no sidebar of
    /// its own.
    pub fn sidebar_shows_files(&self) -> bool {
        let page = self.current_page();
        if page == FILES_PAGE || self.preferences().files_in_every_sidebar {
            return true;
        }
        let own = self
            .plugin_sidebar(page)
            .is_some_and(|view| view.map_or(true, |view| !view.root().children.is_empty()));
        !own
    }

    /// Start the Files page at the top of the workspace, as when another
    /// workspace opens.
    pub(super) fn forget_files_folder(&mut self) {
        self.files_folder.clear();
        self.files_listing = None;
    }

    /// The tree changed: the listing reads the folder again.
    pub(super) fn files_changed(&mut self) {
        self.files_listing = None;
    }

    /// List the tree's current folder when it has one; otherwise keep the
    /// folder, or the nearest folder above it that is still there.
    fn follow_tree_folder(&mut self) {
        let Some(tree) = &self.tree else {
            return;
        };
        if let Some(folder) = tree.focused_folder() {
            if folder != self.files_folder {
                self.files_folder = folder.to_owned();
            }
            return;
        }
        while !self.files_folder.is_empty() && !tree.is_folder(&self.files_folder) {
            self.files_folder = parent_of(&self.files_folder).unwrap_or("").to_owned();
        }
    }

    fn list_files_folder(&self) -> Option<FilesListing> {
        let tree = self.tree.as_ref()?;
        let workspace = self.workspace.as_ref()?;
        let entries = tree
            .children(&self.files_folder)?
            .into_iter()
            .map(|row| FilesEntry {
                modified: workspace.modified(&row.relative_path),
                name: row.name,
                relative_path: row.relative_path,
                kind: row.kind,
                not_downloaded: row.not_downloaded,
            })
            .collect();
        let mut path = vec![FolderStep {
            name: workspace.name(),
            relative_path: String::new(),
        }];
        let mut at = String::new();
        for part in self.files_folder.split('/').filter(|part| !part.is_empty()) {
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(part);
            path.push(FolderStep {
                name: part.to_owned(),
                relative_path: at.clone(),
            });
        }
        Some(FilesListing {
            folder: self.files_folder.clone(),
            path,
            entries,
        })
    }
}
