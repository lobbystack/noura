//! The palette: one overlay with a field and a list, in three modes. The
//! command palette filters every command as you type and runs the
//! highlighted one; quick open and search list files and open the
//! highlighted one (see `finder.rs`).

use std::time::Instant;

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

use super::{App, Change, QuickOpen, Search};
use crate::{Command, CommandSource, OpenAs, SearchState};

/// The command palette's mode of the palette.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandPalette {
    /// What you typed.
    pub query: String,
    /// The highlighted row of [`App::palette_matches`].
    pub selected: usize,
}

/// What the palette shows while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Palette {
    /// Every command, filtered by what you typed (Mod-K).
    Commands(CommandPalette),
    /// Files by name (Mod-O).
    QuickOpen(QuickOpen),
    /// File contents (Mod-Shift-F).
    Search(Search),
}

/// Which of its modes the palette shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaletteMode {
    Commands,
    QuickOpen,
    Search,
}

impl Palette {
    pub fn mode(&self) -> PaletteMode {
        match self {
            Self::Commands(_) => PaletteMode::Commands,
            Self::QuickOpen(_) => PaletteMode::QuickOpen,
            Self::Search(_) => PaletteMode::Search,
        }
    }

    /// What the palette's field holds.
    pub fn query(&self) -> &str {
        match self {
            Self::Commands(commands) => &commands.query,
            Self::QuickOpen(quick_open) => &quick_open.query,
            Self::Search(search) => &search.query.text,
        }
    }

    /// The highlighted row.
    pub fn selected(&self) -> usize {
        match self {
            Self::Commands(commands) => commands.selected,
            Self::QuickOpen(quick_open) => quick_open.selected,
            Self::Search(search) => search.selected,
        }
    }

    fn selected_mut(&mut self) -> &mut usize {
        match self {
            Self::Commands(commands) => &mut commands.selected,
            Self::QuickOpen(quick_open) => &mut quick_open.selected,
            Self::Search(search) => &mut search.selected,
        }
    }

    /// The workspace paths of the files quick open or search lists, in
    /// order. The command palette lists none.
    pub fn paths(&self) -> Vec<&str> {
        match self {
            Self::Commands(_) => Vec::new(),
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

    /// Open the command palette with an empty field, or close the palette
    /// in whichever mode it shows.
    pub fn toggle_palette(&mut self) {
        if self.palette.is_some() {
            self.close_palette();
        } else if self.workspace.is_some() {
            self.palette = Some(Palette::Commands(CommandPalette::default()));
            self.listeners.emit(Change::Palette);
        }
    }

    /// Close the palette, as Escape does. A search still running is
    /// forgotten.
    pub fn close_palette(&mut self) {
        self.searcher.cancel();
        if self.palette.take().is_some() {
            self.listeners.emit(Change::Palette);
        }
    }

    /// Change what the palette's field holds, and highlight the best
    /// match. The command palette filters its commands, quick open ranks
    /// the files again, and search starts again in the background, where
    /// clearing the field clears the results.
    pub fn set_palette_query(&mut self, query: &str) {
        match &mut self.palette {
            Some(Palette::Commands(palette)) if palette.query != query => {
                query.clone_into(&mut palette.query);
                palette.selected = 0;
                self.listeners.emit(Change::Palette);
            }
            Some(Palette::QuickOpen(quick_open)) if quick_open.query != query => {
                query.clone_into(&mut quick_open.query);
                self.rank_files();
            }
            Some(Palette::Search(search)) if search.query.text != query => {
                query.clone_into(&mut search.query.text);
                self.search_again();
            }
            _ => {}
        }
    }

    /// The commands the command palette lists, best match first: every
    /// command whose label holds the typed words' letters in order,
    /// ignoring case and accents. With nothing typed, every command in
    /// registry order. Empty while the palette shows another mode.
    pub fn palette_matches(&self) -> Vec<Command> {
        let Some(Palette::Commands(palette)) = &self.palette else {
            return Vec::new();
        };
        let commands = self.commands();
        let pattern = Pattern::new(
            &palette.query,
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

    /// How many rows the palette lists.
    fn palette_rows(&self) -> usize {
        match &self.palette {
            Some(Palette::Commands(_)) => self.palette_matches().len(),
            Some(palette) => palette.paths().len(),
            None => 0,
        }
    }

    /// Highlight the row `offset` rows below the highlighted one, or above
    /// it when `offset` is negative, stopping at the first and last rows.
    pub fn move_palette_selection(&mut self, offset: isize) {
        let last = self.palette_rows().saturating_sub(1);
        let Some(palette) = &mut self.palette else {
            return;
        };
        let selected = palette.selected_mut();
        let moved = selected.saturating_add_signed(offset).min(last);
        if moved != *selected {
            *selected = moved;
            self.listeners.emit(Change::Palette);
        }
    }

    /// Highlight the palette's row `row`.
    pub fn select_palette_row(&mut self, row: usize) {
        let count = self.palette_rows();
        let Some(palette) = &mut self.palette else {
            return;
        };
        let selected = palette.selected_mut();
        if row < count && *selected != row {
            *selected = row;
            self.listeners.emit(Change::Palette);
        }
    }

    /// Close the palette and act on the highlighted row, chosen at `at`,
    /// as Enter does: run the highlighted command through the same
    /// dispatch as keys and menus, or open the highlighted file the way
    /// the file tree opens one. Returns whether a command ran or the file
    /// opened.
    pub fn run_palette_selection(&mut self, at: Instant) -> bool {
        let Some(palette) = &self.palette else {
            return false;
        };
        let selected = palette.selected();
        if palette.mode() == PaletteMode::Commands {
            let chosen = self.palette_matches().into_iter().nth(selected);
            self.close_palette();
            return match chosen {
                Some(command) => self.run_command(&command.id, CommandSource::Palette, at),
                None => false,
            };
        }
        let Some(path) = palette.paths().get(selected).map(|path| (*path).to_owned()) else {
            return false;
        };
        self.close_palette();
        self.open_file(&path, OpenAs::Preview);
        self.document()
            .is_some_and(|file| file.relative_path == path)
    }
}
