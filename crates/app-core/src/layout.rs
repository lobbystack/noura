//! The window layout this device remembers for every workspace: whether
//! the sidebar shows, its width and the sidebar sections you folded.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store::JsonStore;

/// The sidebar's narrowest width in points, and its width until you
/// resize it.
pub const SIDEBAR_MIN_WIDTH: f32 = 224.0;

/// The layout this device remembers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Layout {
    /// False once you hide the sidebar.
    pub sidebar_shown: bool,
    /// The sidebar's width in points, at least [`SIDEBAR_MIN_WIDTH`].
    pub sidebar_width: f32,
    /// The sidebar sections you folded, by section key.
    pub collapsed_sections: Vec<String>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_shown: true,
            sidebar_width: SIDEBAR_MIN_WIDTH,
            collapsed_sections: Vec::new(),
        }
    }
}

/// Reads and writes `layout.json` in the app data folder.
pub(crate) struct LayoutStore {
    store: JsonStore<Layout>,
}

impl LayoutStore {
    pub(crate) fn new(app_data: &Path) -> Self {
        Self {
            store: JsonStore::new(app_data.join("layout.json"), "window layout"),
        }
    }

    /// The saved layout, or the default one when there is none or Noura
    /// can't read it. A width under the minimum comes back as the
    /// minimum.
    pub(crate) fn load(&self) -> Layout {
        let mut layout = self.store.load();
        layout.sidebar_width = sidebar_width(layout.sidebar_width).unwrap_or(SIDEBAR_MIN_WIDTH);
        layout
    }

    /// Save the layout. A failure goes to the log: the layout still
    /// applies until Noura quits.
    pub(crate) fn save(&self, layout: &Layout) {
        if let Err(error) = self.store.save(layout) {
            tracing::warn!(%error, "the window layout could not be saved");
        }
    }
}

/// `width` rounded to whole points and kept at the minimum or wider, or
/// `None` when it isn't a number.
pub(crate) fn sidebar_width(width: f32) -> Option<f32> {
    width
        .is_finite()
        .then(|| width.round().max(SIDEBAR_MIN_WIDTH))
}
