//! The palette's quick open and search modes: what you typed, the
//! highlighted row, and the files that match.

use super::{App, Change};
use crate::{OpenAs, QuickOpenMatch, SearchQuery, SearchState, quick_open};

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

/// What the palette shows while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Palette {
    QuickOpen(QuickOpen),
    Search(Search),
}

impl Palette {
    /// What the palette's field holds.
    pub fn query(&self) -> &str {
        match self {
            Self::QuickOpen(quick_open) => &quick_open.query,
            Self::Search(search) => &search.query.text,
        }
    }

    /// The highlighted row.
    pub fn selected(&self) -> usize {
        match self {
            Self::QuickOpen(quick_open) => quick_open.selected,
            Self::Search(search) => search.selected,
        }
    }

    fn selected_mut(&mut self) -> &mut usize {
        match self {
            Self::QuickOpen(quick_open) => &mut quick_open.selected,
            Self::Search(search) => &mut search.selected,
        }
    }

    /// The workspace paths of the files the palette lists, in order.
    pub fn paths(&self) -> Vec<&str> {
        match self {
            Self::QuickOpen(quick_open) => quick_open
                .matches
                .iter()
                .map(|found| found.relative_path.as_str())
                .collect(),
            Self::Search(Search {
                state: SearchState::Done(hits),
                ..
            }) => hits.iter().map(|hit| hit.relative_path.as_str()).collect(),
            Self::Search(_) => Vec::new(),
        }
    }
}

impl App {
    /// The palette, while it's open.
    pub fn palette(&self) -> Option<&Palette> {
        self.palette.as_ref()
    }

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
    /// replaces what the palette showed.
    pub fn open_search(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        self.searcher.cancel();
        let (types, state) = match workspace.search_types() {
            Ok(types) => (types, SearchState::Idle),
            Err(error) => (Vec::new(), SearchState::Failed(error)),
        };
        self.palette = Some(Palette::Search(Search {
            query: SearchQuery::default(),
            selected: 0,
            types,
            state,
        }));
        self.listeners.emit(Change::Palette);
    }

    /// Close the palette, as Escape does. A search still running is
    /// forgotten.
    pub fn close_palette(&mut self) {
        self.searcher.cancel();
        if self.palette.take().is_some() {
            self.listeners.emit(Change::Palette);
        }
    }

    /// Change what the palette's field holds. Quick open ranks the files
    /// again; search starts again in the background, and clearing the
    /// field clears the results.
    pub fn set_palette_query(&mut self, text: &str) {
        match &mut self.palette {
            Some(Palette::QuickOpen(quick_open)) if quick_open.query != text => {
                text.clone_into(&mut quick_open.query);
                self.rank_files();
            }
            Some(Palette::Search(search)) if search.query.text != text => {
                text.clone_into(&mut search.query.text);
                self.search_again();
            }
            _ => {}
        }
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
    fn search_again(&mut self) {
        let (Some(Palette::Search(search)), Some(workspace)) = (&mut self.palette, &self.workspace)
        else {
            return;
        };
        search.selected = 0;
        if search.query.text.trim().is_empty() {
            self.searcher.cancel();
            search.state = SearchState::Idle;
        } else {
            self.searcher.start(workspace.clone(), search.query.clone());
            search.state = SearchState::Searching;
        }
        self.listeners.emit(Change::Palette);
    }

    /// Show the newest search's answer, once it arrives. Returns whether
    /// the palette changed.
    pub(super) fn take_search_answer(&mut self) -> bool {
        let Some(result) = self.searcher.take() else {
            return false;
        };
        let Some(Palette::Search(search)) = &mut self.palette else {
            return false;
        };
        search.state = match result {
            Ok(hits) => SearchState::Done(hits),
            Err(error) => SearchState::Failed(error),
        };
        self.listeners.emit(Change::Palette);
        true
    }

    /// Highlight the row `offset` rows below the highlighted one, or above
    /// it when `offset` is negative, stopping at the first and last rows.
    pub fn move_palette_selection(&mut self, offset: isize) {
        let Some(palette) = &mut self.palette else {
            return;
        };
        let last = palette.paths().len().saturating_sub(1);
        let selected = palette.selected_mut();
        let moved = selected.saturating_add_signed(offset).min(last);
        if moved != *selected {
            *selected = moved;
            self.listeners.emit(Change::Palette);
        }
    }

    /// Highlight the palette's row `row`.
    pub fn select_palette_row(&mut self, row: usize) {
        let Some(palette) = &mut self.palette else {
            return;
        };
        let count = palette.paths().len();
        let selected = palette.selected_mut();
        if row < count && *selected != row {
            *selected = row;
            self.listeners.emit(Change::Palette);
        }
    }

    /// Close the palette and open the highlighted file the way the file
    /// tree opens a file, as Enter does. Returns whether the file opened.
    pub fn open_palette_selection(&mut self) -> bool {
        let Some(palette) = &self.palette else {
            return false;
        };
        let Some(path) = palette
            .paths()
            .get(palette.selected())
            .map(|path| (*path).to_owned())
        else {
            return false;
        };
        self.close_palette();
        self.open_file(&path, OpenAs::Preview);
        self.document()
            .is_some_and(|file| file.relative_path == path)
    }
}
