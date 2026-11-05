//! App updates: Noura checks a signed feed, downloads and verifies a newer
//! release in the background, and installs it only when the person chooses
//! to restart.
//!
//! The desktop app supplies an [`UpdateSource`] that talks to the feed;
//! tests supply a fake. Checks and downloads run on background threads, and
//! [`crate::App::poll_at`] applies their results. Every window shares one
//! update service, and each one hears when its status changes.

use std::{
    fmt,
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    time::{Duration, Instant},
};

use crate::{AppError, PendingSaves, PendingWrites, app::Change, writes::WriteFailure};

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
    /// Noura kept running. `installed` says whether the update replaced
    /// the installed app before the problem.
    NotRestarted {
        update: Box<dyn PendingUpdate>,
        installed: bool,
        problem: String,
    },
    Restarting,
}

/// Wakes every window with a change.
pub(crate) type Notify = Arc<dyn Fn(Change) + Send + Sync>;

/// The update service. Every window of the app shares it, so clones share
/// one state. [`crate::App`] exposes its actions.
#[derive(Clone)]
pub struct Updates(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    writes: PendingWrites,
    notify: Notify,
}

struct State {
    current_version: String,
    status: UpdateStatus,
    source: Option<Arc<dyn UpdateSource>>,
    next_check: Option<Instant>,
    /// The downloaded update, waiting for a restart.
    ready: Option<Box<dyn PendingUpdate>>,
    /// The ready update already replaced the installed app, and only the
    /// restart is left.
    installed: bool,
    /// Results from the background thread, while one runs.
    events: Option<Receiver<Event>>,
    offer_hidden: bool,
    /// The status or the offer changed, and the windows haven't heard yet.
    changed: bool,
}

impl Updates {
    /// The update service for the windows `notify` wakes, waiting for
    /// `writes` before a restart.
    pub(crate) fn new(writes: PendingWrites, notify: Notify) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                current_version: env!("CARGO_PKG_VERSION").to_owned(),
                status: UpdateStatus::Unavailable {
                    reason: "This build doesn't update itself.".to_owned(),
                },
                source: None,
                next_check: None,
                ready: None,
                installed: false,
                events: None,
                offer_hidden: false,
                changed: false,
            }),
            writes,
            notify,
        }))
    }

    /// Run `change` on the state, then wake every window if the status or
    /// the offer changed. The windows hear it after the lock is free.
    fn change<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        let result = change(&mut state);
        let changed = std::mem::take(&mut state.changed);
        drop(state);
        if changed {
            (self.0.notify)(Change::Update);
        }
        result
    }

    fn read<R>(&self, read: impl FnOnce(&State) -> R) -> R {
        read(&self.0.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn status(&self) -> UpdateStatus {
        self.read(|state| state.status.clone())
    }

    /// The running app's version.
    pub fn current_version(&self) -> String {
        self.read(|state| state.current_version.clone())
    }

    /// When the next scheduled check runs, if updates are on.
    pub fn next_check(&self) -> Option<Instant> {
        self.read(|state| state.next_check)
    }

    /// Whether a check, download or install is running.
    pub fn is_busy(&self) -> bool {
        self.read(|state| state.events.is_some())
    }

    /// Whether to offer Restart outside Settings: an update is ready and
    /// the person hasn't put the offer off.
    pub fn offers_restart(&self) -> bool {
        self.read(|state| matches!(state.status, UpdateStatus::Ready { .. }) && !state.offer_hidden)
    }

    pub(crate) fn enable(
        &self,
        current_version: &str,
        source: Arc<dyn UpdateSource>,
        launched: Instant,
    ) {
        self.change(|state| {
            state.current_version = current_version.to_owned();
            state.source = Some(source);
            state.next_check = Some(launched + FIRST_UPDATE_CHECK);
            state.set(UpdateStatus::Idle);
        });
    }

    pub(crate) fn disable(&self, current_version: &str, reason: &str) {
        self.change(|state| {
            state.current_version = current_version.to_owned();
            state.source = None;
            state.next_check = None;
            state.ready = None;
            state.installed = false;
            state.set(UpdateStatus::Unavailable {
                reason: reason.to_owned(),
            });
        });
    }

    pub(crate) fn hide_offer(&self) {
        self.change(|state| {
            if !state.offer_hidden {
                state.offer_hidden = true;
                state.changed = true;
            }
        });
    }

    /// Start a check now. Does nothing while one runs or an update waits
    /// for a restart.
    pub(crate) fn check(&self) {
        self.change(|state| self.start_check(state));
    }

    fn start_check(&self, state: &mut State) {
        let checkable = matches!(
            state.status,
            UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Failed { .. }
        );
        let Some(source) = state.source.clone() else {
            return;
        };
        if !checkable || state.events.is_some() {
            return;
        }
        let started = self.spawn(state, "update-check", move |send| match source.check() {
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
            state.set(UpdateStatus::Checking);
        }
    }

    /// Save the changes `saves` holds back, wait for pending writes,
    /// install the downloaded update, save and wait again for changes made
    /// meanwhile, and restart. A save, a write or an install that fails
    /// keeps this version running and says why.
    pub(crate) fn restart(&self, saves: Arc<dyn PendingSaves>) {
        self.change(|state| self.start_restart(state, saves));
    }

    fn start_restart(&self, state: &mut State, saves: Arc<dyn PendingSaves>) {
        let UpdateStatus::Ready { version, .. } = &state.status else {
            return;
        };
        let version = version.clone();
        let Some(source) = state.source.clone() else {
            return;
        };
        if state.events.is_some() {
            return;
        }
        let Some(mut update) = state.ready.take() else {
            return;
        };
        let writes = self.0.writes.clone();
        let installing = version.clone();
        let mut installed = state.installed;
        let started = self.spawn(state, "update-install", move |send| {
            if !installed {
                if let Err(problem) = settle(saves.as_ref(), &writes) {
                    send(Event::NotRestarted {
                        update,
                        installed,
                        problem,
                    });
                    return;
                }
                if let Err(error) = update.install() {
                    tracing::warn!(detail = %error.detail, "the update could not be installed");
                    send(Event::NotRestarted {
                        update,
                        installed,
                        problem: format!(
                            "Noura couldn't install version {installing} and kept this \
                             version running. Try again later."
                        ),
                    });
                    return;
                }
                installed = true;
            }
            // The install can take a while, for example while macOS asks
            // for an administrator password, and you can keep working
            // meanwhile. Those changes reach disk before Noura quits too.
            if let Err(problem) = settle(saves.as_ref(), &writes) {
                send(Event::NotRestarted {
                    update,
                    installed,
                    problem: format!(
                        "{problem} Version {installing} runs the next time you open Noura."
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
            state.set(UpdateStatus::Installing { version });
        } else {
            state.set(UpdateStatus::Failed {
                message: "Noura couldn't start installing. Quit Noura and open it again."
                    .to_owned(),
            });
        }
    }

    /// Start a check that is due at `now`, and apply finished background
    /// work. Returns whether anything changed. Every window polls; the
    /// first to see a result applies it, and every window hears of it.
    pub(crate) fn poll(&self, now: Instant) -> bool {
        self.change(|state| {
            let mut changed = false;
            if let Some(next) = state.next_check
                && now >= next
            {
                state.next_check = Some(now + UPDATE_CHECK_INTERVAL);
                let before = state.status.clone();
                self.start_check(state);
                changed |= state.status != before;
            }
            changed | state.apply_events()
        })
    }

    /// Run `work` on a background thread that reports through `send`.
    /// Returns whether the thread started.
    fn spawn(
        &self,
        state: &mut State,
        name: &str,
        work: impl FnOnce(&dyn Fn(Event)) + Send + 'static,
    ) -> bool {
        let (sender, receiver): (Sender<Event>, Receiver<Event>) = channel();
        let notify = self.0.notify.clone();
        let spawned = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let send = |event| {
                    // The app may have closed meanwhile; nothing is waiting then.
                    let _ = sender.send(event);
                    notify(Change::WorkFinished);
                };
                work(&send);
                drop(sender);
                notify(Change::WorkFinished);
            });
        match spawned {
            Ok(_) => {
                state.events = Some(receiver);
                true
            }
            Err(error) => {
                tracing::error!(%error, "could not start the update thread");
                false
            }
        }
    }
}

impl State {
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
        if changed {
            // Busy or not, the windows show it.
            self.changed = true;
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
                self.installed = false;
                self.offer_hidden = false;
                self.set(status);
            }
            Event::NotRestarted {
                update,
                installed,
                problem,
            } => {
                let status = UpdateStatus::Ready {
                    version: update.version().to_owned(),
                    notes: update.notes().map(str::to_owned),
                    problem: Some(problem),
                };
                self.ready = Some(update);
                self.installed = installed;
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
        self.changed = true;
    }
}

/// Save the changes `saves` holds back and wait for pending writes. The
/// error says what didn't reach disk and what to do.
fn settle(saves: &dyn PendingSaves, writes: &PendingWrites) -> Result<(), String> {
    // The workspace root stands for every file in it.
    saves
        .save_now("")
        .map_err(|error| unsaved_changes_problem(&error))?;
    writes.flush().map_err(|failure| unsaved_problem(&failure))
}

fn unsaved_changes_problem(error: &AppError) -> String {
    format!(
        "Noura didn't restart, because it couldn't save your changes. {}",
        error.message
    )
}

fn unsaved_problem(failure: &WriteFailure) -> String {
    format!(
        "Noura didn't restart, because it couldn't save \u{201c}{}\u{201d}. {} Save it again, \
         or move it to the trash, then restart. If you quit Noura instead, you lose that change.",
        failure.file, failure.message
    )
}
