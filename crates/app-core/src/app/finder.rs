//! Search's files: ranking them by name as you type, searching their
//! contents in the background, and the type, folder and tag filters that
//! search filtered to contents offers.

use super::{App, Change, SearchScope};
use crate::{SearchState, quick_open};

impl App {
    /// Rank the tree's files for what search's field holds, and highlight
    /// the first row. The file tree calls this when it reads the files
    /// again.
    pub(super) fn rank_files(&mut self) {
        let (Some(palette), Some(tree)) = (&mut self.palette, &self.tree) else {
            return;
        };
        palette.names = quick_open::rank(tree.file_paths(), &palette.query.text);
        palette.selected = 0;
        self.listeners.emit(Change::Palette);
    }

    /// Show only objects of `object_type`, or every file for `None`.
    pub fn set_search_type(&mut self, object_type: Option<&str>) {
        let Some(palette) = self.contents_palette() else {
            return;
        };
        if palette.query.object_type.as_deref() != object_type {
            palette.query.object_type = object_type.map(str::to_owned);
            self.search_again();
        }
    }

    /// Show only files in `folder` and its subfolders, or every file for
    /// `""`.
    pub fn set_search_folder(&mut self, folder: &str) {
        let Some(palette) = self.contents_palette() else {
            return;
        };
        if palette.query.folder != folder {
            folder.clone_into(&mut palette.query.folder);
            self.search_again();
        }
    }

    /// Show only notes tagged `tag`, or every file for `""`.
    pub fn set_search_tag(&mut self, tag: &str) {
        let Some(palette) = self.contents_palette() else {
            return;
        };
        if palette.query.tag != tag {
            tag.clone_into(&mut palette.query.tag);
            self.search_again();
        }
    }

    /// Search, while it's open filtered to contents.
    fn contents_palette(&mut self) -> Option<&mut super::Palette> {
        self.palette
            .as_mut()
            .filter(|palette| palette.scope == SearchScope::Contents)
    }

    /// Search the contents for the current query and filters in place of
    /// the search before, whose answer no longer applies, and highlight
    /// the first row. Search filtered to names, or with nothing typed,
    /// searches no contents.
    pub(super) fn search_again(&mut self) {
        let (Some(palette), Some(workspace)) = (&mut self.palette, &self.workspace) else {
            return;
        };
        palette.selected = 0;
        if palette.scope == SearchScope::Names || palette.query.text.trim().is_empty() {
            self.searcher.cancel_search();
            palette.state = SearchState::Idle;
        } else {
            self.searcher
                .start(workspace.clone(), palette.query.clone());
            palette.state = SearchState::Searching;
        }
        self.listeners.emit(Change::Palette);
    }

    /// Show the newest search's answer and the index's types, once they
    /// arrive, keeping the highlighted row. Returns whether search
    /// changed.
    pub(super) fn take_search_answer(&mut self) -> bool {
        let answers = self.searcher.take();
        if self.palette.is_none() || (answers.types.is_none() && answers.hits.is_none()) {
            return false;
        }
        let kept = self.selected_key();
        let Some(palette) = &mut self.palette else {
            return false;
        };
        match answers.types {
            Some(Ok(types)) => palette.types = types,
            // Until a search runs and says why itself, search says the
            // index failed.
            Some(Err(error)) if palette.state == SearchState::Idle => {
                palette.state = SearchState::Failed(error);
            }
            Some(Err(_)) | None => {}
        }
        match answers.hits {
            Some(Ok(hits)) => palette.state = SearchState::Done(hits),
            Some(Err(error)) => palette.state = SearchState::Failed(error),
            None => {}
        }
        self.keep_selection(kept);
        self.listeners.emit(Change::Palette);
        true
    }
}
