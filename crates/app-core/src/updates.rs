//! App updates: Noura checks a signed feed, downloads and verifies a newer
//! release in the background, and installs it only when the person chooses
//! to restart.
//!
//! The desktop app supplies an [`UpdateSource`] that talks to the feed;
//! tests supply a fake. Checks and downloads run on background threads, and
//! [`crate::App::poll_at`] applies their results.

use std::{
    fmt,
    sync::{
        Arc,
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    time::{Duration, Instant},
};

use crate::{
    PendingWrites,
    app::{Change, Listeners},
    writes::WriteFailure,
};

/// How long after launch the first check runs.
pub const FIRST_UPDATE_CHECK: Duration = Duration::from_secs(5);

/// How long Noura waits between checks.
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Where releases come from: the desktop app's signed feed, or a fake.
pub trait UpdateSource: Send + Sync {
    /// Ask the feed for a release newer than the running app.
    fn check(&self) -> Result<Option<Box<dyn PendingUpdate>>, UpdateError>;

    /// Start the newly installed app and quit this one. On a real system
    /// this returns only when it fails.
    fn restart(&self) -> Result<(), UpdateError>;
}

/// A release newer than the running app.
pub trait PendingUpdate: Send {
    fn version(&self) -> &str;

    /// The release notes, when the feed has them.
    fn notes(&self) -> Option<&str>;

    /// Download the update and verify its signature. Installs nothing.
    fn download(&mut self) -> Result<(), UpdateError>;

    /// Replace the installed app with the downloaded update.
    fn install(&mut self) -> Result<(), UpdateError>;
}

/// What went wrong with an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateErrorKind {
    /// The feed or the download couldn't be reached or read.
    Unreachable,
    /// The download didn't carry a valid signature from Noura's key.
    BadSignature,
    /// The update couldn't replace the installed app.
    Install,
    /// The updated app couldn't be started.
    Restart,
}

/// An update failure. The detail goes to the diagnostic log, never to the
/// screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateError {
    pub kind: UpdateErrorKind,
    pub detail: String,
}

impl UpdateError {
    pub fn new(kind: UpdateErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for UpdateError {}

/// Where updating stands, as Settings shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// This build doesn't update itself, and `reason` says what to do
    /// instead.
    Unavailable {
        reason: String,
    },
    /// No check has run yet.
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
    },
    /// The update is downloaded and verified, and installs when the person
    /// restarts. `problem` says why the last restart didn't happen.
    Ready {
        version: String,
        notes: Option<String>,
        problem: Option<String>,
    },
    /// Waiting for changes to reach disk, then installing.
    Installing {
        version: String,
    },
    /// Installed; the new version is starting.
    Restarting {
        version: String,
    },
    /// The last check or download failed.
    Failed {
        message: String,
    },
}

enum Event {
    Downloading(String),
    UpToDate,
    Ready(Box<dyn PendingUpdate>),
    Failed(UpdateError),
    NotInstalled {
        update: Box<dyn PendingUpdate>,
        problem: String,
    },
    Restarting,
}

/// The update service's state. [`crate::App`] owns it and exposes its
/// actions.
pub struct Updates {
    current_version: String,
    status: UpdateStatus,
    source: Option<Arc<dyn UpdateSource>>,
    next_check: Option<Instant>,
    /// The downloaded update, waiting for a restart.
    ready: Option<Box<dyn PendingUpdate>>,
    /// Results from the background thread, while one runs.
    events: Option<Receiver<Event>>,
    offer_hidden: bool,
    writes: PendingWrites,
    listeners: Listeners,
}

impl Updates {
    pub(crate) fn new(writes: PendingWrites, listeners: Listeners) -> Self {
        Self {
            current_version: env!("CARGO_PKG_VERSION").to_owned(),
            status: UpdateStatus::Unavailable {
                reason: "This build doesn't update itself.".to_owned(),
            },
            source: None,
            next_check: None,
            ready: None,
            events: None,
            offer_hidden: false,
            writes,
            listeners,
        }
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    /// The running app's version.
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// When the next scheduled check runs, if updates are on.
    pub fn next_check(&self) -> Option<Instant> {
        self.next_check
    }

    /// Whether a check, download or install is running.
    pub fn is_busy(&self) -> bool {
        self.events.is_some()
    }

    /// Whether to offer Restart outside Settings: an update is ready and
    /// the person hasn't put the offer off.
    pub fn offers_restart(&self) -> bool {
        matches!(self.status, UpdateStatus::Ready { .. }) && !self.offer_hidden
    }

    pub(crate) fn enable(
        &mut self,
        current_version: &str,
        source: Arc<dyn UpdateSource>,
        launched: Instant,
    ) {
        self.current_version = current_version.to_owned();
        self.source = Some(source);
        self.next_check = Some(launched + FIRST_UPDATE_CHECK);
        self.set(UpdateStatus::Idle);
    }

    pub(crate) fn disable(&mut self, current_version: &str, reason: &str) {
        self.current_version = current_version.to_owned();
        self.source = None;
        self.next_check = None;
        self.ready = None;
        self.set(UpdateStatus::Unavailable {
            reason: reason.to_owned(),
        });
    }

    pub(crate) fn hide_offer(&mut self) {
        if !self.offer_hidden {
            self.offer_hidden = true;
            self.listeners.emit(Change::Update);
        }
    }

    /// Start a check now. Does nothing while one runs or an update waits
    /// for a restart.
    pub(crate) fn check(&mut self) {
        let checkable = matches!(
            self.status,
            UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Failed { .. }
        );
        let Some(source) = self.source.clone() else {
            return;
        };
        if !checkable || self.events.is_some() {
            return;
        }
        let started = self.spawn("update-check", move |send| match source.check() {
            Ok(None) => send(Event::UpToDate),
            Ok(Some(mut update)) => {
                send(Event::Downloading(update.version().to_owned()));
                match update.download() {
                    Ok(()) => send(Event::Ready(update)),
                    Err(error) => send(Event::Failed(error)),
                }
            }
            Err(error) => send(Event::Failed(error)),
        });
        if started {
            self.set(UpdateStatus::Checking);
        }
    }

    /// Wait for pending writes, install the downloaded update and restart.
    /// A write that fails, or an install that fails, keeps this version
    /// running and says why.
    pub(crate) fn restart(&mut self) {
        let UpdateStatus::Ready { version, .. } = &self.status else {
            return;
        };
        let version = version.clone();
        let Some(source) = self.source.clone() else {
            return;
        };
        if self.events.is_some() {
            return;
        }
        let Some(mut update) = self.ready.take() else {
            return;
        };
        let writes = self.writes.clone();
        let installing = version.clone();
        let started = self.spawn("update-install", move |send| {
            if let Err(failure) = writes.flush() {
                send(Event::NotInstalled {
                    update,
                    problem: unsaved_problem(&failure),
                });
                return;
            }
            if let Err(error) = update.install() {
                tracing::warn!(detail = %error.detail, "the update could not be installed");
                send(Event::NotInstalled {
                    update,
                    problem: format!(
                        "Noura couldn't install version {installing} and kept this version \
                         running. Try again later."
                    ),
                });
                return;
            }
            send(Event::Restarting);
            if let Err(error) = source.restart() {
                send(Event::Failed(error));
            }
        });
        if started {
            self.set(UpdateStatus::Installing { version });
        } else {
            self.set(UpdateStatus::Failed {
                message: "Noura couldn't start installing. Quit Noura and open it again."
                    .to_owned(),
            });
        }
    }

    /// Start a check that is due at `now`, and apply finished background
    /// work. Returns whether anything changed.
    pub(crate) fn poll(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(next) = self.next_check
            && now >= next
        {
            self.next_check = Some(now + UPDATE_CHECK_INTERVAL);
            let before = self.status.clone();
            self.check();
            changed |= self.status != before;
        }
        changed | self.apply_events()
    }

    fn apply_events(&mut self) -> bool {
        let Some(receiver) = &self.events else {
            return false;
        };
        let mut events = Vec::new();
        let finished = loop {
            match receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        if finished {
            self.events = None;
        }
        let changed = !events.is_empty() || finished;
        for event in events {
            self.apply(event);
        }
        if finished
            && matches!(
                self.status,
                UpdateStatus::Checking
                    | UpdateStatus::Downloading { .. }
                    | UpdateStatus::Installing { .. }
            )
        {
            // The thread ended without a result, so it panicked.
            self.set(UpdateStatus::Failed {
                message: "Noura couldn't finish updating. Try again later.".to_owned(),
            });
        }
        changed
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Downloading(version) => self.set(UpdateStatus::Downloading { version }),
            Event::UpToDate => self.set(UpdateStatus::UpToDate),
            Event::Ready(update) => {
                let status = UpdateStatus::Ready {
                    version: update.version().to_owned(),
                    notes: update.notes().map(str::to_owned),
                    problem: None,
                };
                self.ready = Some(update);
                self.offer_hidden = false;
                self.set(status);
            }
            Event::NotInstalled { update, problem } => {
                let status = UpdateStatus::Ready {
                    version: update.version().to_owned(),
                    notes: update.notes().map(str::to_owned),
                    problem: Some(problem),
                };
                self.ready = Some(update);
                self.offer_hidden = false;
                self.set(status);
            }
            Event::Restarting => {
                if let UpdateStatus::Installing { version } = &self.status {
                    let version = version.clone();
                    self.set(UpdateStatus::Restarting { version });
                }
            }
            Event::Failed(error) => {
                tracing::warn!(kind = ?error.kind, detail = %error.detail, "an update failed");
                let message = match (error.kind, &self.status) {
                    (UpdateErrorKind::Unreachable, _) => {
                        "Couldn't reach the update server. Check your connection.".to_owned()
                    }
                    (UpdateErrorKind::BadSignature, _) => "The download's signature didn't \
                         match Noura's key, so Noura deleted it."
                        .to_owned(),
                    (_, UpdateStatus::Restarting { version }) => format!(
                        "Noura installed version {version} but couldn't restart. Quit Noura \
                         and open it again."
                    ),
                    _ => "Noura couldn't update. Try again later.".to_owned(),
                };
                self.set(UpdateStatus::Failed { message });
            }
        }
    }

    fn set(&mut self, status: UpdateStatus) {
        self.status = status;
        self.listeners.emit(Change::Update);
    }

    /// Run `work` on a background thread that reports through `send`.
    /// Returns whether the thread started.
    fn spawn(&mut self, name: &str, work: impl FnOnce(&dyn Fn(Event)) + Send + 'static) -> bool {
        let (sender, receiver): (Sender<Event>, Receiver<Event>) = channel();
        let listeners = self.listeners.clone();
        let spawned = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let send = |event| {
                    // The app may have closed meanwhile; nothing is waiting then.
                    let _ = sender.send(event);
                    listeners.emit(Change::WorkFinished);
                };
                work(&send);
                drop(sender);
                listeners.emit(Change::WorkFinished);
            });
        match spawned {
            Ok(_) => {
                self.events = Some(receiver);
                true
            }
            Err(error) => {
                tracing::error!(%error, "could not start the update thread");
                false
            }
        }
    }
}

fn unsaved_problem(failure: &WriteFailure) -> String {
    format!(
        "Noura didn't restart, because it couldn't save \u{201c}{}\u{201d}. {}",
        failure.file, failure.message
    )
}
