//! What a running plugin asks the window you acted in to show, and the
//! clock it reads. A plugin's request waits here until that window polls,
//! and only a call that answers something you did may make one: an event
//! in one of the plugin's views or panels, or a command you ran from the
//! command palette.

use std::sync::{Arc, Mutex};

use plugin_host::{ClockReading, DeviceClock, Navigation, Placement, ServiceError};

use crate::{OpenAs, System, session::WindowKey};

/// Something a plugin asked a window to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Opening {
    /// The object with this stable ID, in the note editor.
    Object { id: String, how: OpenAs },
    /// The rail page with this ID.
    Page(String),
}

/// The requests waiting for each window, oldest first.
pub(crate) type Openings = Arc<Mutex<Vec<(WindowKey, Opening)>>>;

/// One running plugin's navigation service.
pub(crate) struct Navigator {
    /// The plugin's ID, whose own pages it may open.
    plugin: String,
    /// The window whose action the plugin's current call answers, if any.
    origin: Mutex<Option<WindowKey>>,
    openings: Openings,
}

impl Navigator {
    pub(crate) fn new(plugin: &str, openings: Openings) -> Self {
        Self {
            plugin: plugin.to_owned(),
            origin: Mutex::new(None),
            openings,
        }
    }

    /// Note that the plugin's next calls answer what you did in `window`,
    /// or nothing you did when `None`.
    pub(crate) fn set_origin(&self, window: Option<WindowKey>) {
        *self
            .origin
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = window;
    }

    fn request(&self, opening: Opening) -> Result<(), ServiceError> {
        let origin = *self
            .origin
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(window) = origin else {
            return Err(ServiceError::Failed(
                "Noura opens things only in answer to something you did.".to_owned(),
            ));
        };
        self.openings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((window, opening));
        Ok(())
    }
}

impl Navigation for Navigator {
    fn open_object(&self, id: &str, placement: Placement) -> Result<(), ServiceError> {
        let how = match placement {
            Placement::Preview => OpenAs::Preview,
            Placement::NewTab => OpenAs::Kept,
        };
        self.request(Opening::Object {
            id: id.to_owned(),
            how,
        })
    }

    fn open_page(&self, page: &str) -> Result<(), ServiceError> {
        self.request(Opening::Page(super::page_id(&self.plugin, page)))
    }
}

/// The clock plugins read: this computer's, or the one a test passes as
/// its system.
pub(crate) struct SystemClock(pub(crate) Arc<dyn System>);

impl DeviceClock for SystemClock {
    fn now(&self) -> ClockReading {
        ClockReading::at(self.0.now(), &self.0.time_zone())
    }
}
