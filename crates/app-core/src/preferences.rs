//! Appearance, text size and zoom: choices that stay on this device, kept
//! in `preferences.json` in the app data folder and never in a workspace.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::JsonStore;

/// Whether Noura draws light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Appearance {
    Light,
    Dark,
    /// Follow the system's appearance, including changes while Noura runs.
    #[default]
    MatchSystem,
}

impl Appearance {
    /// Every choice, in the order Settings offers them.
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::MatchSystem];

    pub fn title(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::MatchSystem => "Match system",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::MatchSystem => "system",
        }
    }
}

/// The size of document text, such as a note's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
}

impl TextSize {
    /// Every size, in the order Settings offers them.
    pub const ALL: [Self; 3] = [Self::Small, Self::Default, Self::Large];

    pub fn title(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
        }
    }

    /// The document font size in points at 100% zoom, as the legacy app
    /// sized it: 15, 16 or 18.
    pub fn points(self) -> f32 {
        match self {
            Self::Small => 15.0,
            Self::Default => 16.0,
            Self::Large => 18.0,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Default => "default",
            Self::Large => "large",
        }
    }
}

/// The zoom levels, in percent, that zoom in and zoom out step through,
/// as in a web browser.
pub const ZOOM_LEVELS: [u16; 11] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

/// The zoom level Actual size returns to.
pub const ACTUAL_SIZE: u16 = 100;

/// This device's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Preferences {
    pub appearance: Appearance,
    pub text_size: TextSize,
    /// The zoom level in percent, one of [`ZOOM_LEVELS`].
    pub zoom: u16,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            text_size: TextSize::default(),
            zoom: ACTUAL_SIZE,
        }
    }
}

impl Preferences {
    /// The zoom level as a factor: 1.0 at 100%.
    pub fn zoom_factor(&self) -> f32 {
        f32::from(self.zoom) / 100.0
    }

    /// The zoom level one step from the current one: up when `steps` is
    /// positive, down when it is negative, and no further than either end.
    pub(crate) fn zoom_step(&self, steps: isize) -> u16 {
        let position = |zoom| ZOOM_LEVELS.iter().position(|level| *level == zoom);
        let current = position(self.zoom)
            .or_else(|| position(ACTUAL_SIZE))
            .unwrap_or_default();
        let index = current
            .saturating_add_signed(steps)
            .min(ZOOM_LEVELS.len() - 1);
        ZOOM_LEVELS[index]
    }
}

/// The file's shape, read field by field so one unreadable choice falls
/// back to its default alone.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    appearance: Value,
    text_size: Value,
    zoom: Value,
}

impl From<Saved> for Preferences {
    fn from(saved: Saved) -> Self {
        let defaults = Self::default();
        let appearance = Appearance::ALL
            .into_iter()
            .find(|appearance| saved.appearance.as_str() == Some(appearance.key()))
            .unwrap_or(defaults.appearance);
        let text_size = TextSize::ALL
            .into_iter()
            .find(|size| saved.text_size.as_str() == Some(size.key()))
            .unwrap_or(defaults.text_size);
        let zoom = ZOOM_LEVELS
            .into_iter()
            .find(|level| saved.zoom.as_u64() == Some(u64::from(*level)))
            .unwrap_or(defaults.zoom);
        Self {
            appearance,
            text_size,
            zoom,
        }
    }
}

impl From<&Preferences> for Saved {
    fn from(preferences: &Preferences) -> Self {
        Self {
            appearance: preferences.appearance.key().into(),
            text_size: preferences.text_size.key().into(),
            zoom: preferences.zoom.into(),
        }
    }
}

/// Reads and writes `preferences.json` in the app data folder.
pub(crate) struct PreferencesStore {
    store: JsonStore<Saved>,
}

impl PreferencesStore {
    pub(crate) fn new(app_data: &Path) -> Self {
        Self {
            store: JsonStore::new(app_data.join("preferences.json"), "preferences"),
        }
    }

    /// The saved choices. A missing or unreadable file, or an unreadable
    /// choice in it, comes back as the default.
    pub(crate) fn load(&self) -> Preferences {
        self.store.load().into()
    }

    /// Save the choices. A failure goes to the log: they still apply
    /// until Noura quits.
    pub(crate) fn save(&self, preferences: &Preferences) {
        if let Err(error) = self.store.save(&preferences.into()) {
            tracing::warn!(%error, "the preferences could not be saved");
        }
    }
}
