//! Search: one overlay with one field, like Spotlight. As you type it
//! lists files whose name holds the typed letters, then files whose text
//! holds the typed words, then commands, and Enter opens the highlighted
//! file or runs the highlighted command. Mod-K opens it unfiltered; Mod-O
//! opens it filtered to file names and Mod-Shift-F to file contents (see
//! `finder.rs` for both).
//!
//! Unfiltered, a group of files that holds more than it lists ends with a
//! row that shows them all, as Spotlight's Show More does: search filters
//! itself to that group. Filtered, search says how many files match and
//! lists a page of them, then another each time you reach the end, as VS
//! Code's lists do, so the first ones never wait for the rest.

use std::{collections::HashSet, time::Instant};

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

use super::{App, Change};
use crate::{
    Command, CommandSource, OpenAs, QuickOpenMatch, SEARCH_PAGE, SearchHit, SearchQuery,
    SearchState, quick_open,
};

/// Unfiltered, search lists at most this many files by name, and as many
/// by contents.
pub const GROUP_LIMIT: usize = 5;

/// With nothing typed, unfiltered search lists at most this many
/// commands under the recent files.
pub const EMPTY_COMMANDS: usize = 5;

/// What search lists: everything, or one kind of result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SearchScope {
    /// Files by name, files by contents and commands (Mod-K).
    #[default]
    All,
    /// Files by name only (Mod-O).
    Names,
    /// Files by contents only, with the type, folder and tag filters
    /// (Mod-Shift-F).
    Contents,
}

/// The groups search lists its results in, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaletteGroup {
    /// Files whose name holds the typed letters, or the recent files.
    Files,
    /// Files whose text holds the typed words.
    Contents,
    Commands,
}

/// One row search lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItem {
    /// A file found by name, or a recent file.
    File(QuickOpenMatch),
    /// A file found by its text, with the matching text.
    Hit(SearchHit),
    Command(Command),
    /// The last row of a group that lists fewer files than match: it
    /// filters search to that group. `total` counts them all, once known.
    More {
        group: PaletteGroup,
        total: Option<usize>,
    },
}

impl PaletteItem {
    pub fn group(&self) -> PaletteGroup {
        match self {
            Self::File(_) => PaletteGroup::Files,
            Self::Hit(_) => PaletteGroup::Contents,
            Self::Command(_) => PaletteGroup::Commands,
            Self::More { group, .. } => *group,
        }
    }

    /// The file the row opens, if it opens one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File(found) => Some(&found.relative_path),
            Self::Hit(hit) => Some(&hit.relative_path),
            Self::Command(_) | Self::More { .. } => None,
        }
    }

    /// What tells this row apart from the others, across new results.
    fn key(&self) -> (PaletteGroup, &str) {
        match self {
            Self::Command(command) => (self.group(), &command.id),
            // No file has an empty path.
            _ => (self.group(), self.path().unwrap_or_default()),
        }
    }
}

/// What search shows in place of rows, or that it shows rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteStatus {
    /// Rows to show.
    Results,
    /// Nothing to show yet: the field is empty and nothing is recent, or
    /// unfiltered search waits for the index.
    Idle,
    /// Search filtered to contents waits for the index.
    Searching,
    /// Nothing matches what you typed.
    NoResults,
    /// The index couldn't answer; [`Palette::state`] says why.
    Failed,
}

/// Search, while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// What search lists.
    pub scope: SearchScope,
    /// What you typed, and the type, folder and tag filters, which only
    /// search filtered to contents offers.
    pub query: SearchQuery,
    /// The highlighted row of [`App::palette_results`].
    pub selected: usize,
    /// The files whose name holds the typed letters, best first.
    pub names: Vec<QuickOpenMatch>,
    /// The object types the type filter offers: the types the index
    /// holds.
    pub types: Vec<String>,
    /// The search of file contents.
    pub state: SearchState,
    /// How many rows search filtered to names or contents lists at most:
    /// a page more each time you reach the end of the list.
    pub shown: usize,
    /// How many files match the contents search in all, once counted.
    pub hit_count: Option<usize>,
}

impl Palette {
    fn new(scope: SearchScope) -> Self {
        Self {
            scope,
            query: SearchQuery::default(),
            selected: 0,
            names: Vec::new(),
            types: Vec::new(),
            state: SearchState::Idle,
            shown: SEARCH_PAGE,
            hit_count: None,
        }
    }

    /// What the field holds.
    pub fn query(&self) -> &str {
        &self.query.text
    }

    /// The highlighted row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    fn typed(&self) -> bool {
        !self.query.text.trim().is_empty()
    }
}

impl App {
    /// Search, while it's open.
    pub fn palette(&self) -> Option<&Palette> {
        self.palette.as_ref()
    }

    /// Open search unfiltered with an empty field, as Mod-K does, or
    /// close it whatever it shows.
    pub fn toggle_palette(&mut self) {
        if self.palette.is_some() {
            self.close_palette();
        } else {
            self.open_palette(SearchScope::All);
        }
    }

    /// Open search filtered to `scope`, as Mod-O and Mod-Shift-F do. Open
    /// already, search keeps what you typed and changes its filter.
    pub fn open_palette(&mut self, scope: SearchScope) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if self.palette.is_some() {
            self.set_palette_scope(scope);
            return;
        }
        if scope == SearchScope::Contents {
            // The type filter's types arrive from the search thread, so
            // the UI never waits on the index.
            self.searcher.list_types(workspace.clone());
        }
        self.palette = Some(Palette::new(scope));
        self.listeners.emit(Change::Palette);
    }

    /// Filter search to `scope`, or remove the filter with
    /// [`SearchScope::All`]. Leaving contents clears the type, folder and
    /// tag filters, which nothing shows anymore.
    pub fn set_palette_scope(&mut self, scope: SearchScope) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let Some(palette) = &mut self.palette else {
            return;
        };
        if palette.scope == scope {
            return;
        }
        if palette.scope == SearchScope::Contents {
            palette.query = SearchQuery {
                text: std::mem::take(&mut palette.query.text),
                ..SearchQuery::default()
            };
        }
        palette.scope = scope;
        if scope == SearchScope::Contents {
            self.searcher.list_types(workspace);
        }
        self.search_again();
    }

    /// Close search, as Escape does. A search still running is forgotten.
    pub fn close_palette(&mut self) {
        self.searcher.cancel();
        if self.palette.take().is_some() {
            self.listeners.emit(Change::Palette);
        }
    }

    /// Change what the field holds, rank the files again, search their
    /// contents again in the background, and highlight the first row.
    pub fn set_palette_query(&mut self, query: &str) {
        let Some(palette) = &mut self.palette else {
            return;
        };
        if palette.query.text != query {
            query.clone_into(&mut palette.query.text);
            self.rank_files();
            self.search_again();
        }
    }

    /// The commands whose label holds the typed words' letters in order,
    /// ignoring case and accents, best match first. With nothing typed,
    /// every command in registry order. Empty unless search is open
    /// unfiltered.
    pub fn palette_matches(&self) -> Vec<Command> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        if palette.scope != SearchScope::All {
            return Vec::new();
        }
        let commands = self.commands();
        let pattern = Pattern::new(
            &palette.query.text,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buffer = Vec::new();
        let mut scored: Vec<(u32, Command)> = commands
            .into_iter()
            .filter_map(|command| {
                let label = Utf32Str::new(&command.label, &mut buffer);
                let score = pattern.score(label, &mut matcher)?;
                Some((score, command))
            })
            .collect();
        // A stable sort keeps registry order among equal scores.
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, command)| command).collect()
    }

    /// The rows search lists, in group order: files by name, files by
    /// contents, then commands. Unfiltered, each file group stops at
    /// [`GROUP_LIMIT`], with a [`PaletteItem::More`] row when more files
    /// match, and a file found by name doesn't show again by contents.
    /// Filtered, the list stops at [`Palette::shown`]. With nothing typed,
    /// the recent files and the first [`EMPTY_COMMANDS`] commands.
    pub fn palette_results(&self) -> Vec<PaletteItem> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        let hits = match &palette.state {
            SearchState::Done(hits) => hits.as_slice(),
            _ => &[],
        };
        let typed = palette.typed();
        let mut rows = Vec::new();
        match palette.scope {
            SearchScope::Names if typed => {
                let names = palette.names.iter().take(palette.shown);
                rows.extend(names.cloned().map(PaletteItem::File));
            }
            SearchScope::Names => rows.extend(self.recent_files()),
            SearchScope::Contents => {
                let hits = hits.iter().take(palette.shown);
                rows.extend(hits.cloned().map(PaletteItem::Hit));
            }
            SearchScope::All if typed => {
                let names = &palette.names[..palette.names.len().min(GROUP_LIMIT)];
                let shown: HashSet<&str> = names
                    .iter()
                    .map(|found| found.relative_path.as_str())
                    .collect();
                rows.extend(names.iter().cloned().map(PaletteItem::File));
                if palette.names.len() > names.len() {
                    rows.push(PaletteItem::More {
                        group: PaletteGroup::Files,
                        total: Some(palette.names.len()),
                    });
                }
                let by_contents: Vec<&SearchHit> = hits
                    .iter()
                    .filter(|hit| !shown.contains(hit.relative_path.as_str()))
                    .take(GROUP_LIMIT)
                    .collect();
                rows.extend(
                    by_contents
                        .iter()
                        .map(|hit| PaletteItem::Hit((*hit).clone())),
                );
                // Files the group leaves out: matches past its rows, or
                // past the hits that have arrived. One listed by name
                // doesn't count.
                let not_by_name = hits
                    .iter()
                    .filter(|hit| !shown.contains(hit.relative_path.as_str()))
                    .count();
                let unloaded = palette.hit_count.is_some_and(|count| count > hits.len());
                if not_by_name > by_contents.len() || unloaded {
                    rows.push(PaletteItem::More {
                        group: PaletteGroup::Contents,
                        total: palette.hit_count,
                    });
                }
                rows.extend(self.palette_matches().into_iter().map(PaletteItem::Command));
            }
            SearchScope::All => {
                rows.extend(self.recent_files().take(GROUP_LIMIT));
                rows.extend(
                    self.palette_matches()
                        .into_iter()
                        .take(EMPTY_COMMANDS)
                        .map(PaletteItem::Command),
                );
            }
        }
        rows
    }

    /// How many files search filtered to names or contents finds in all,
    /// once it knows. `None` unfiltered, with nothing typed, and while
    /// the contents search counts.
    pub fn palette_total(&self) -> Option<usize> {
        let palette = self.palette.as_ref()?;
        if !palette.typed() {
            return None;
        }
        match palette.scope {
            SearchScope::Names => Some(palette.names.len()),
            SearchScope::Contents => match palette.state {
                SearchState::Done(_) => palette.hit_count,
                _ => None,
            },
            SearchScope::All => None,
        }
    }

    /// List another page of what search filtered to names or contents
    /// finds, as when you reach the end of its list. The contents search
    /// fetches the page first; nothing happens while a page is on its way
    /// or once every file shows.
    pub fn show_more_results(&mut self) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let Some(palette) = &mut self.palette else {
            return;
        };
        let (listed, total) = match (&palette.scope, &palette.state) {
            (SearchScope::Names, _) => (palette.names.len(), palette.names.len()),
            (SearchScope::Contents, SearchState::Done(hits)) => {
                // Until the count arrives, a full page may have more after it.
                let full = !hits.is_empty() && hits.len() % SEARCH_PAGE == 0;
                let unknown = if full { usize::MAX } else { hits.len() };
                (hits.len(), palette.hit_count.unwrap_or(unknown))
            }
            _ => return,
        };
        if palette.shown >= total || listed < palette.shown {
            return;
        }
        palette.shown += SEARCH_PAGE;
        if palette.scope == SearchScope::Contents {
            self.searcher.more(workspace, palette.query.clone(), listed);
        }
        self.listeners.emit(Change::Palette);
    }

    /// The notes changed last, newest first, as file rows.
    fn recent_files(&self) -> impl Iterator<Item = PaletteItem> + '_ {
        self.recent
            .iter()
            .map(|path| PaletteItem::File(quick_open::file_match(path)))
    }

    /// Whether search shows rows, or what it shows instead.
    pub fn palette_status(&self) -> PaletteStatus {
        let Some(palette) = &self.palette else {
            return PaletteStatus::Idle;
        };
        if matches!(palette.state, SearchState::Failed(_)) {
            return PaletteStatus::Failed;
        }
        if !self.palette_results().is_empty() {
            return PaletteStatus::Results;
        }
        let searching = palette.state == SearchState::Searching;
        match palette.scope {
            _ if !palette.typed() => PaletteStatus::Idle,
            SearchScope::Contents if searching => PaletteStatus::Searching,
            SearchScope::All if searching => PaletteStatus::Idle,
            _ => PaletteStatus::NoResults,
        }
    }

    /// Highlight the row `offset` rows below the highlighted one, or above
    /// it when `offset` is negative, across groups, stopping at the first
    /// and last rows.
    pub fn move_palette_selection(&mut self, offset: isize) {
        let last = self.palette_results().len().saturating_sub(1);
        let Some(palette) = &mut self.palette else {
            return;
        };
        let moved = palette.selected.saturating_add_signed(offset).min(last);
        if moved != palette.selected {
            palette.selected = moved;
            self.listeners.emit(Change::Palette);
        }
        // Down at the last row lists the next page, so the keys reach
        // every file.
        if offset > 0 && moved == last {
            self.show_more_results();
        }
    }

    /// Highlight the row `row`.
    pub fn select_palette_row(&mut self, row: usize) {
        let count = self.palette_results().len();
        let Some(palette) = &mut self.palette else {
            return;
        };
        if row < count && palette.selected != row {
            palette.selected = row;
            self.listeners.emit(Change::Palette);
        }
    }

    /// Keep the row `before` named highlighted once the rows changed, so
    /// results that arrive later don't move the highlight off it. The
    /// first row stays highlighted.
    pub(super) fn keep_selection(&mut self, before: Option<(PaletteGroup, String)>) {
        let Some(before) = before else {
            return;
        };
        let rows = self.palette_results();
        let found = rows
            .iter()
            .position(|row| {
                let (group, key) = row.key();
                group == before.0 && key == before.1
            })
            .unwrap_or(0);
        if let Some(palette) = &mut self.palette {
            palette.selected = found;
        }
    }

    /// The highlighted row's key, unless it's the first row.
    pub(super) fn selected_key(&self) -> Option<(PaletteGroup, String)> {
        let selected = self.palette.as_ref()?.selected;
        if selected == 0 {
            return None;
        }
        let rows = self.palette_results();
        let (group, key) = rows.get(selected)?.key();
        Some((group, key.to_owned()))
    }

    /// Close search and act on the highlighted row, chosen at `at`, as
    /// Enter does: open the highlighted file the way the file tree opens
    /// one, or run the highlighted command through the same dispatch as
    /// keys and menus. Returns whether the file opened or a command ran.
    pub fn run_palette_selection(&mut self, at: Instant) -> bool {
        let Some(palette) = &self.palette else {
            return false;
        };
        let chosen = self.palette_results().into_iter().nth(palette.selected);
        if let Some(PaletteItem::More { group, .. }) = chosen {
            let scope = match group {
                PaletteGroup::Files => SearchScope::Names,
                PaletteGroup::Contents | PaletteGroup::Commands => SearchScope::Contents,
            };
            self.set_palette_scope(scope);
            return true;
        }
        self.close_palette();
        match chosen {
            Some(PaletteItem::Command(command)) => {
                self.run_command(&command.id, CommandSource::Palette, at)
            }
            Some(PaletteItem::More { .. }) | None => false,
            Some(item) => {
                let path = item.path().unwrap_or_default().to_owned();
                self.open_file(&path, OpenAs::Preview);
                self.document()
                    .is_some_and(|file| file.relative_path == path)
            }
        }
    }
}
