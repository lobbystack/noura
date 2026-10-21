//! The palette's quick open and search modes: what you typed, the
//! highlighted row, and the files that match. Quick Open (Mod-O) and
//! Search Files (Mod-Shift-F) are commands that open them.

use super::{App, Change, Palette};
use crate::{QuickOpenMatch, SearchQuery, SearchState, quick_open};

/// Quick open, as the palette shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickOpen {
    /// What you typed.
    pub query: String,
    /// The highlighted match.
    pub selected: usize,
    /// The files whose name holds the typed letters, best first.
    pub matches: Vec<QuickOpenMatch>,
}

/// Search, as the palette shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// What you typed, and the type, folder and tag filters.
    pub query: SearchQuery,
    /// The highlighted hit.
    pub selected: usize,
    /// The object types the type filter offers: the types the index
    /// holds.
    pub types: Vec<String>,
    pub state: SearchState,
}

impl App {
    /// Open quick open with an empty field, as Mod-O does. Quick open
    /// replaces what the palette showed.
    pub fn open_quick_open(&mut self) {
        if self.workspace.is_none() {
            return;
        }
        self.searcher.cancel();
        self.palette = Some(Palette::QuickOpen(QuickOpen {
            query: String::new(),
            selected: 0,
            matches: Vec::new(),
        }));
        self.listeners.emit(Change::Palette);
    }

    /// Open search with an empty field, as Mod-Shift-F does. Search
    /// replaces what the palette showed. The type filter's types arrive
    /// from the search thread, so the UI never waits on the index.
    pub fn open_search(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        self.searcher.cancel();
        self.searcher.list_types(workspace.clone());
        self.palette = Some(Palette::Search(Search {
            query: SearchQuery::default(),
            selected: 0,
            types: Vec::new(),
            state: SearchState::Idle,
        }));
        self.listeners.emit(Change::Palette);
    }

    /// Rank the tree's files for quick open's query, and highlight the
    /// best match. The file tree calls this when it reads the files again.
    pub(super) fn rank_files(&mut self) {
        let (Some(Palette::QuickOpen(quick_open)), Some(tree)) = (&mut self.palette, &self.tree)
        else {
            return;
        };
        quick_open.matches = quick_open::rank(tree.file_paths(), &quick_open.query);
        quick_open.selected = 0;
        self.listeners.emit(Change::Palette);
    }

    /// Show only objects of `object_type`, or every file for `None`.
    pub fn set_search_type(&mut self, object_type: Option<&str>) {
        let Some(Palette::Search(search)) = &mut self.palette else {
            return;
        };
        if search.query.object_type.as_deref() != object_type {
            search.query.object_type = object_type.map(str::to_owned);
            self.search_again();
        }
    }

    /// Show only files in `folder` and its subfolders, or every file for
    /// `""`.
    pub fn set_search_folder(&mut self, folder: &str) {
        let Some(Palette::Search(search)) = &mut self.palette else {
            return;
        };
        if search.query.folder != folder {
            folder.clone_into(&mut search.query.folder);
            self.search_again();
        }
    }

    /// Show only notes tagged `tag`, or every file for `""`.
    pub fn set_search_tag(&mut self, tag: &str) {
        let Some(Palette::Search(search)) = &mut self.palette else {
            return;
        };
        if search.query.tag != tag {
            tag.clone_into(&mut search.query.tag);
            self.search_again();
        }
    }

    /// Run the search for the current query and filters in place of the
    /// one before, whose answer no longer applies.
    pub(super) fn search_again(&mut self) {
        let (Some(Palette::Search(search)), Some(workspace)) = (&mut self.palette, &self.workspace)
        else {
            return;
        };
        search.selected = 0;
        if search.query.text.trim().is_empty() {
            self.searcher.cancel_search();
            search.state = SearchState::Idle;
        } else {
            self.searcher.start(workspace.clone(), search.query.clone());
            search.state = SearchState::Searching;
        }
        self.listeners.emit(Change::Palette);
    }

    /// Show the newest search's answer and the index's types, once they
    /// arrive. Returns whether the palette changed.
    pub(super) fn take_search_answer(&mut self) -> bool {
        let answers = self.searcher.take();
        let Some(Palette::Search(search)) = &mut self.palette else {
            return false;
        };
        if answers.types.is_none() && answers.hits.is_none() {
            return false;
        }
        match answers.types {
            Some(Ok(types)) => search.types = types,
            // Until a search runs and says why itself, the palette says
            // the index failed.
            Some(Err(error)) if search.state == SearchState::Idle => {
                search.state = SearchState::Failed(error);
            }
            Some(Err(_)) | None => {}
        }
        match answers.hits {
            Some(Ok(hits)) => search.state = SearchState::Done(hits),
            Some(Err(error)) => search.state = SearchState::Failed(error),
            None => {}
        }
        self.listeners.emit(Change::Palette);
        true
    }
}
