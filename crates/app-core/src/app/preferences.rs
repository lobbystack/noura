//! Appearance, text size, line width, zoom and spell check. Each change
//! applies at once and is saved for this device.

use super::{App, Change};
use crate::{Appearance, LineWidth, Preferences, TextSize, preferences::ACTUAL_SIZE};

impl App {
    /// This device's appearance, text size, line width, zoom and spell
    /// check.
    pub fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.change_preferences(|preferences| preferences.appearance = appearance);
    }

    /// Set the size of document text.
    pub fn set_text_size(&mut self, text_size: TextSize) {
        self.change_preferences(|preferences| preferences.text_size = text_size);
    }

    /// Use `width` for every open note from now on.
    pub fn set_line_width(&mut self, line_width: LineWidth) {
        self.change_preferences(|preferences| preferences.line_width = line_width);
    }

    /// Mark misspelled words in every open note, or stop.
    pub fn set_spell_check(&mut self, on: bool) {
        self.change_preferences(|preferences| preferences.spell_check = on);
    }

    /// Zoom in one level, up to 200%.
    pub fn zoom_in(&mut self) {
        let zoom = self.preferences.zoom_step(1);
        self.change_preferences(|preferences| preferences.zoom = zoom);
    }

    /// Zoom out one level, down to 50%.
    pub fn zoom_out(&mut self) {
        let zoom = self.preferences.zoom_step(-1);
        self.change_preferences(|preferences| preferences.zoom = zoom);
    }

    /// Zoom back to 100%.
    pub fn actual_size(&mut self) {
        self.change_preferences(|preferences| preferences.zoom = ACTUAL_SIZE);
    }

    fn change_preferences(&mut self, change: impl FnOnce(&mut Preferences)) {
        let before = self.preferences;
        change(&mut self.preferences);
        if self.preferences != before {
            self.preferences_store.save(&self.preferences);
            self.listeners.emit(Change::Preferences);
            // Every window draws with the same preferences.
            self.preferences_seen = self.session.set_preferences(self.key, self.preferences);
        }
    }
}
