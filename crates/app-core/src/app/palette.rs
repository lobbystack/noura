//! The command palette: a field that filters every command as you type,
//! and runs the highlighted one.

use std::time::Instant;

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

use super::{App, Change};
use crate::{Command, CommandSource};

/// The open command palette.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandPalette {
    /// What you typed.
    pub query: String,
    /// The highlighted row of [`App::palette_matches`].
    pub selected: usize,
}

impl App {
    /// The command palette, while it's open.
    pub fn palette(&self) -> Option<&CommandPalette> {
        self.palette.as_ref()
    }

    /// Open the command palette with an empty field, or close it.
    pub fn toggle_palette(&mut self) {
        if self.palette.is_some() {
            self.close_palette();
        } else if self.workspace.is_some() {
            self.palette = Some(CommandPalette::default());
            self.listeners.emit(Change::Palette);
        }
    }

    pub fn close_palette(&mut self) {
        if self.palette.take().is_some() {
            self.listeners.emit(Change::Palette);
        }
    }

    /// Filter the palette by `query`, and highlight the best match.
    pub fn set_palette_query(&mut self, query: &str) {
        if let Some(palette) = &mut self.palette
            && palette.query != query
        {
            query.clone_into(&mut palette.query);
            palette.selected = 0;
            self.listeners.emit(Change::Palette);
        }
    }

    /// The commands the palette lists, best match first: every command
    /// whose label holds the typed words' letters in order, ignoring case
    /// and accents. With nothing typed, every command in registry order.
    pub fn palette_matches(&self) -> Vec<Command> {
        let commands = self.commands();
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
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

    /// Highlight the row `offset` rows below the highlighted one, or above
    /// it when `offset` is negative, stopping at the first and last rows.
    pub fn move_palette_selection(&mut self, offset: isize) {
        let count = self.palette_matches().len();
        if let Some(palette) = &mut self.palette {
            let last = count.saturating_sub(1);
            let selected = palette.selected.saturating_add_signed(offset).min(last);
            if selected != palette.selected {
                palette.selected = selected;
                self.listeners.emit(Change::Palette);
            }
        }
    }

    /// Highlight the palette's row `row`.
    pub fn select_palette_row(&mut self, row: usize) {
        if let Some(palette) = &mut self.palette
            && palette.selected != row
        {
            palette.selected = row;
            self.listeners.emit(Change::Palette);
        }
    }

    /// Close the palette and run the highlighted command, chosen at `at`.
    /// Returns whether a command ran.
    pub fn run_palette_selection(&mut self, at: Instant) -> bool {
        let Some(selected) = self.palette.as_ref().map(|palette| palette.selected) else {
            return false;
        };
        let chosen = self.palette_matches().into_iter().nth(selected);
        self.close_palette();
        match chosen {
            Some(command) => self.run_command(&command.id, CommandSource::Palette, at),
            None => false,
        }
    }
}
