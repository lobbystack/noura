//! What a running plugin asks the window you acted in to show, and the
//! clock it reads. A plugin's request waits here until that window polls,
//! and only a call that answers something you did may make one: an event
//! in one of the plugin's views or panels, or a command you ran from the
//! command palette. Such a call runs on the plugin's own thread, so a call
//! into the plugin from another thread, such as another plugin's command
//! call or a tool call, opens nothing even while one waits its turn.

use std::{
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

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
    /// The window whose action the plugin's current call answers, if any,
    /// and the thread that call runs on.
    origin: Mutex<Option<(ThreadId, WindowKey)>>,
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

    /// Note that the plugin's next calls on this thread answer what you
    /// did in `window`, or nothing you did when `None`.
    pub(crate) fn set_origin(&self, window: Option<WindowKey>) {
        *self
            .origin
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            window.map(|window| (thread::current().id(), window));
    }

    fn request(&self, opening: Opening) -> Result<(), ServiceError> {
        let origin = *self
            .origin
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let here = thread::current().id();
        let Some(window) = origin
            .filter(|(thread, _)| *thread == here)
            .map(|(_, window)| window)
        else {
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

#[cfg(test)]
mod tests {
    use super::*;

    const REFUSED: &str = "Noura opens things only in answer to something you did.";

    fn refused(answer: Result<(), ServiceError>) -> bool {
        matches!(answer, Err(ServiceError::Failed(message)) if message == REFUSED)
    }

    #[test]
    fn only_the_call_that_answers_you_opens_and_only_in_your_window() {
        let openings = Openings::default();
        let navigator = Arc::new(Navigator::new("probe", openings.clone()));
        let window = WindowKey::nth(7);

        // Outside a call that answers something you did, such as while a
        // view draws, nothing opens.
        assert!(refused(navigator.open_page("events")));
        navigator.set_origin(Some(window));
        // A call into the plugin from another thread, such as another
        // plugin's command call that runs first, opens nothing.
        let elsewhere = Arc::clone(&navigator);
        let answer = std::thread::spawn(move || {
            elsewhere.open_object("task_01jbcdefghjkmnpqrstvwxyz0a", Placement::Preview)
        })
        .join()
        .unwrap();
        assert!(refused(answer));
        navigator.open_page("events").unwrap();
        navigator.set_origin(None);
        assert!(refused(navigator.open_page("events")));

        assert_eq!(
            *openings.lock().unwrap(),
            [(window, Opening::Page("plugin:probe:events".to_owned()))]
        );
    }
}
