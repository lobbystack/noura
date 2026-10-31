//! The open workspace's sync rounds, which the account backend's thread
//! runs with the account's sync service (docs/workspace-format/sync-v2.md
//! §15): it opens the folder's journal when a workspace opens, runs one
//! round each tick unless you paused sync, and keeps the Workspace sync
//! section's state in step.
//!
//! The rounds share the account's one service, so the device keeps one
//! view of its account chains. When the driver has nothing else to poll,
//! the folder follows the workspace's realtime channel (§23): it applies
//! the records frames that follow on from its cursor, pulls on a gap or a
//! notify and once a minute, and while the channel is down each round's
//! pull waits up to [`PULL_WAIT_SECONDS`] for new records. A change in
//! the workspace's files or something you do ends the wait, so local
//! changes go out at once (§15).
//!
//! [`PULL_WAIT_SECONDS`]: crate::sync::workspace::PULL_WAIT_SECONDS

use std::path::Path;
use std::sync::Arc;

use local_core::WorkspaceEngine;

use crate::folder_sync::{
    ArchivedEntry, BranchEntry, ConflictEntry, FolderSyncAction, FolderSyncPhase, FolderSyncScreen,
    HeldEntry, HeldRootEntry, StoppedNoteEntry,
};
use crate::sync::realtime::Live;
use crate::sync::registry::Registry;
use crate::sync::workspace::WorkspaceSync;
use crate::sync::workspace::text::{TextReview, TextReviewKind};
use crate::sync::{Secrets, SyncError, SyncService};

/// How many text review lines Settings lists.
const TEXT_REVIEW_LINES: usize = 32;
/// What Settings says after a device reports `sync_text_conflict`.
const TEXT_CONFLICT: &str = "A collaborator's version of a note disagreed with this computer's records, so Noura rebuilt the note. Check it below.";

/// One line of Settings for a text review entry, unless the note has its
/// own entry with a choice.
fn text_review_line(sync: &WorkspaceSync, review: &TextReview) -> Option<String> {
    let path = sync
        .local_path(&review.rid)
        .unwrap_or_else(|| "A note".into());
    let device = review.signer.as_ref().map_or_else(
        || "another device".into(),
        |d| format!("device {}", short(d)),
    );
    Some(match review.kind {
        TextReviewKind::Unreadable => {
            format!(
                "{path}: this computer couldn't read an edit from {device}, so it left the edit out."
            )
        }
        TextReviewKind::Refused => format!(
            "{path}: this computer left out an edit from {device}, because it would land in different places on different computers."
        ),
        TextReviewKind::Disagrees => format!(
            "{path}: a version from {device} disagreed with this computer's records, and Noura rebuilt the note."
        ),
        TextReviewKind::Pending => {
            format!("{path}: some edits wait for an earlier edit this computer couldn't read.")
        }
        TextReviewKind::EndedLineage => format!(
            "{path}: an edit from {device} arrived after the note started over, so Noura didn't merge it."
        ),
        TextReviewKind::Stopped | TextReviewKind::OutsideRoot => return None,
    })
}

/// The review entries of text-mode notes, within what Settings lists.
fn text_review_lines(sync: &WorkspaceSync) -> Vec<String> {
    let mut lines: Vec<String> = sync
        .text_reviews()
        .iter()
        .filter_map(|review| text_review_line(sync, review))
        .collect();
    let more = lines.len().saturating_sub(TEXT_REVIEW_LINES);
    if more > 0 {
        lines.truncate(TEXT_REVIEW_LINES);
        lines.push(format!("And {more} more."));
    }
    lines
}

/// The open workspace and its sync.
#[derive(Default)]
pub(super) struct Folder {
    /// You asked to reset your devices from the restore notice.
    reset_wanted: bool,
    /// This folder's workspace has yet to accept a reset, so it stays out
    /// of sync until then.
    reset_held: bool,
    engine: Option<Arc<WorkspaceEngine>>,
    sync: Option<WorkspaceSync>,
    last_error: Option<String>,
    /// What a note's records reported, until its review entry goes.
    text_problem: Option<String>,
    /// The folder lost or refused its journal, so it syncs again from the
    /// server and matches its files at the next round (§15).
    rebuild: bool,
    /// The folder's journal holds another replica ID: it waits for you to
    /// connect it.
    not_connected: bool,
    /// The server lost the account this folder synced under, with the
    /// device this computer held there (§14).
    account_lost: Option<sync_protocol::types::Id>,
    /// The workspace's realtime channel and its pull schedule.
    live: Live,
}

/// What a failed round or action tells you in the Workspace sync section.
fn folder_message(error: &SyncError) -> String {
    match error {
        SyncError::Network(_) => {
            "The sync server can't be reached. Your changes wait on this computer.".into()
        }
        SyncError::Check("sync_reseal_limit") => {
            "The server keeps refusing a change, so this computer stopped sending changes. Your files keep them.".into()
        }
        SyncError::Check("sync_journal_rollback") => {
            "This folder's sync records are older than this computer expected, so sync stopped here.".into()
        }
        SyncError::Check("sync_folder_not_connected") => {
            "This folder is a copy of a synced folder. Turn on sync to connect it.".into()
        }
        SyncError::Check(code) => format!("Sync stopped: the server's answer failed a check ({code})."),
        SyncError::Invalid(text) => text.clone(),
        other => format!("Sync stopped: {other}."),
    }
}

/// What the window says about `waiting` moves and held folders, and about
/// `moved_in` files whose earlier versions stayed behind (§9).
fn attention(waiting: usize, moved_in: usize) -> Option<String> {
    match (waiting, moved_in) {
        (0, 0) => None,
        (0, _) => Some(
            "Earlier versions of a file you moved into a restricted folder stayed in the folder it left. Open Settings > Members to read more.".into(),
        ),
        _ => Some(
            "Files wait for you before they reach more people. Open Settings > Members to decide.".into(),
        ),
    }
}

fn short(id: &sync_protocol::types::Id) -> String {
    id.to_hex().chars().take(8).collect()
}

impl Folder {
    /// The section's state now.
    pub(super) fn screen(&self, signed_in: bool) -> FolderSyncScreen {
        if self.engine.is_none() {
            return FolderSyncScreen::default();
        }
        let mut screen = match &self.sync {
            Some(sync) => {
                let own = sync.device();
                let mut screen = FolderSyncScreen::of(&sync.status(), sync.paused());
                screen.conflicts = sync
                    .conflicts()
                    .into_iter()
                    .map(|conflict| ConflictEntry {
                        rid: conflict.rid,
                        path: sync.local_path(&conflict.rid).unwrap_or_default(),
                        branches: conflict
                            .branches
                            .iter()
                            .map(|branch| BranchEntry {
                                label: match (&branch.path, branch.device == own) {
                                    (None, _) => "Delete the file".into(),
                                    (Some(_), true) => "Keep this computer's version".into(),
                                    (Some(_), false) => format!(
                                        "Keep the version from device {}",
                                        short(&branch.device)
                                    ),
                                },
                                key: branch.key,
                            })
                            .collect(),
                    })
                    .collect();
                screen.stopped_notes =
                    sync.stopped_notes()
                        .into_iter()
                        .map(|(rid, path)| StoppedNoteEntry {
                            rid,
                            path,
                            outside: false,
                        })
                        .chain(sync.outside_notes().into_iter().map(|(rid, path)| {
                            StoppedNoteEntry {
                                rid,
                                path,
                                outside: true,
                            }
                        }))
                        .collect();
                screen.text_reviews = text_review_lines(sync);
                screen.held = sync
                    .held()
                    .into_iter()
                    .map(|(rid, path)| HeldEntry { rid, path })
                    .collect();
                screen.held_roots = sync
                    .held_roots()
                    .into_iter()
                    .map(|(folder, files, people)| HeldRootEntry {
                        restore: sync.restorable_domain(&folder),
                        folder,
                        files,
                        people,
                    })
                    .collect();
                screen.unreadable = sync
                    .unreadable()
                    .into_iter()
                    .map(|(rid, device, _)| {
                        let path = sync.local_path(&rid).unwrap_or_else(|| "A file".into());
                        format!(
                            "{path}: a version from device {} couldn't be read, so this computer keeps the last version that could.",
                            short(&device)
                        )
                    })
                    .collect();
                screen.removal_hold = sync.removal_hold();
                screen.removal_over_limit = sync.removal_over_limit();
                screen.attention = attention(
                    sync.moves_for_review().len() + screen.held_roots.len(),
                    sync.moved_in().len(),
                );
                screen.deleted_away = sync.deleted_away();
                screen.archived = sync
                    .archived()
                    .iter()
                    .enumerate()
                    .map(|(index, a)| ArchivedEntry {
                        index,
                        path: a.path.clone(),
                    })
                    .collect();
                screen
            }
            None if signed_in && self.account_lost.is_some() => FolderSyncScreen {
                phase: FolderSyncPhase::AccountLost,
                restore: Some(crate::folder_sync::lost_notice(
                    &crate::sync::workspace::restore::Lost::Server,
                    false,
                )),
                ..FolderSyncScreen::default()
            },
            None if signed_in && self.not_connected => FolderSyncScreen {
                phase: FolderSyncPhase::NotConnected,
                ..FolderSyncScreen::default()
            },
            None if signed_in => FolderSyncScreen {
                phase: FolderSyncPhase::Off,
                ..FolderSyncScreen::default()
            },
            None => FolderSyncScreen::default(),
        };
        screen.last_error = self.last_error.clone().or(self.text_problem.clone());
        screen
    }

    /// Takes the workspace the windows show, and opens its journal when
    /// the folder syncs.
    pub(super) fn attach(
        &mut self,
        engine: Option<Arc<WorkspaceEngine>>,
        service: Option<&SyncService>,
        store: &Secrets,
        app_data: &Path,
    ) {
        self.sync = None;
        self.live = Live::default();
        self.last_error = None;
        self.rebuild = false;
        self.not_connected = false;
        self.account_lost = None;
        self.engine = engine;
        self.open(service, store, app_data);
    }

    fn open(&mut self, service: Option<&SyncService>, store: &Secrets, app_data: &Path) {
        let (Some(engine), Some(service)) = (&self.engine, service) else {
            return;
        };
        if self.sync.is_some() {
            return;
        }
        let Ok(registry) = Registry::open(app_data) else {
            return;
        };
        let Some(entry) = registry.of_folder(engine.root()) else {
            // A copy of a synced folder carries the journal of this device.
            let device = service.credentials().keys.device();
            self.not_connected = crate::sync::workspace::rebuild::journal_workspace(
                engine.root(),
                &device,
                &**store,
            )
            .is_some();
            return;
        };
        if !entry.on {
            return;
        }
        match WorkspaceSync::open(
            engine.clone(),
            service.credentials().clone(),
            store.clone(),
            &registry,
        ) {
            Ok(sync) => self.sync = Some(sync),
            Err(SyncError::Check("sync_restore_lost")) => {
                self.account_lost = crate::sync::workspace::restore::lost_with_its_server(
                    entry,
                    service.credentials(),
                    &**store,
                )
                .ok()
                .flatten();
            }
            Err(SyncError::Check("sync_journal_lost" | "sync_journal_rollback")) => {
                self.rebuild = true;
            }
            Err(SyncError::Check("sync_folder_not_connected")) => self.not_connected = true,
            Err(error) => self.last_error = Some(folder_message(&error)),
        }
    }

    /// Whether the windows show the workspace in `folder`.
    pub(super) fn shows(&self, folder: &Path) -> bool {
        let canonical = |path: &Path| std::fs::canonicalize(path).ok();
        self.engine.as_ref().is_some_and(|engine| {
            canonical(engine.root()).is_some() && canonical(engine.root()) == canonical(folder)
        })
    }

    /// Downloads the remote workspace `workspace` into the open folder
    /// (§15, §18).
    pub(super) async fn download(
        &mut self,
        service: &mut SyncService,
        workspace: sync_protocol::types::Id,
        app_data: &Path,
    ) -> Result<(), SyncError> {
        let engine = self
            .engine
            .clone()
            .ok_or_else(|| SyncError::Invalid("Choose a folder first.".into()))?;
        let mut registry = Registry::open(app_data)?;
        // The folder you synced before connects to the workspace an owner
        // set up again (§14); it matches rather than downloads.
        let rejoining = registry
            .of_folder(engine.root())
            .is_some_and(|entry| entry.workspace != workspace);
        let sync = if rejoining {
            // The old workspace's sync lets go of the folder's journal; the
            // next tick opens it again after a failure.
            self.sync = None;
            WorkspaceSync::rejoin(service, engine, workspace, &mut registry).await?
        } else {
            WorkspaceSync::download(service, engine, workspace, &mut registry).await?
        };
        self.sync = Some(sync);
        self.last_error = None;
        self.not_connected = false;
        Ok(())
    }

    /// Lets go of the device a reset retired. While `waits` names the
    /// workspace of this folder, it stays out of sync until that workspace
    /// accepts the reset (§7) and the folder connects again.
    pub(super) fn after_reset(&mut self, waits: &[sync_protocol::types::Id], app_data: &Path) {
        self.sync = None;
        self.live = Live::default();
        self.rebuild = false;
        self.account_lost = None;
        self.reset_held = self
            .workspace_of_folder(app_data)
            .is_some_and(|w| waits.contains(&w));
    }

    /// The workspace the registry maps this folder to.
    pub(super) fn workspace_of_folder(&self, app_data: &Path) -> Option<sync_protocol::types::Id> {
        let engine = self.engine.as_ref()?;
        let registry = Registry::open(app_data).ok()?;
        registry
            .of_folder(engine.root())
            .map(|entry| entry.workspace)
    }

    /// Set up sync again from a reset owner's new card (§14).
    pub(super) async fn set_up_after_reset(
        &mut self,
        service: &mut SyncService,
        retired: Option<sync_protocol::types::Id>,
        app_data: &Path,
    ) -> Result<(), SyncError> {
        let engine = self
            .engine
            .clone()
            .ok_or_else(|| SyncError::Invalid("Open the workspace's folder first.".into()))?;
        let mut registry = Registry::open(app_data)?;
        let sync =
            WorkspaceSync::set_up_after_reset(service, engine, &mut registry, retired).await?;
        self.sync = Some(sync);
        self.reset_held = false;
        self.not_connected = false;
        self.account_lost = None;
        self.last_error = None;
        Ok(())
    }

    /// The open workspace's sync, when it syncs.
    pub(super) fn take_reset_wanted(&mut self) -> bool {
        std::mem::take(&mut self.reset_wanted)
    }

    pub(super) fn sync_mut(&mut self) -> Option<&mut WorkspaceSync> {
        self.sync.as_mut()
    }

    /// Whether a note's document is open.
    pub(super) fn text_active(&self) -> bool {
        self.sync.as_ref().is_some_and(WorkspaceSync::text_active)
    }

    /// Whether the open workspace syncs and isn't paused, so a round's
    /// pull may wait for new records.
    pub(super) fn waits(&self) -> bool {
        self.sync
            .as_ref()
            .is_some_and(|sync| !sync.paused() && sync.status().ended.is_none())
    }

    /// One round, unless you paused sync, when a workspace that syncs is
    /// open. When `long`, it follows the realtime channel until a frame,
    /// the minute's pull, `wake` or a change in the workspace's files, and
    /// while the channel is down its pull waits up to the longest pull
    /// wait. A failed round keeps the outbox and shows why.
    pub(super) async fn round(
        &mut self,
        service: Option<&mut SyncService>,
        store: &Secrets,
        app_data: &Path,
        long: bool,
        wake: &tokio::sync::Notify,
        text: &crate::text_bridge::TextBridge,
    ) {
        let Some(service) = service else {
            return;
        };
        if self.reset_held {
            return;
        }
        self.open(Some(service), store, app_data);
        if self.rebuild
            && let Some(engine) = self.engine.clone()
            && let Ok(mut registry) = Registry::open(app_data)
        {
            match WorkspaceSync::rebuild(service, engine, &mut registry).await {
                Ok(sync) => {
                    self.sync = Some(sync);
                    self.rebuild = false;
                }
                Err(error) => {
                    self.last_error = Some(folder_message(&error));
                    return;
                }
            }
        }
        let Some(sync) = self.sync.as_mut() else {
            return;
        };
        sync.follow_changes();
        text.serve(sync);
        let result = if sync.paused() {
            sync.capture().map(|_| ())
        } else {
            let live = &mut self.live;
            let mut events = self.engine.as_ref().map(|engine| engine.subscribe());
            let changes = async {
                loop {
                    match events.as_mut() {
                        Some(events) => match events.recv().await {
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                wake.notify_one();
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                std::future::pending::<()>().await;
                            }
                        },
                        None => std::future::pending::<()>().await,
                    }
                }
            };
            let step = async {
                if long {
                    live.step(sync, service, wake).await.map(|_| ())
                } else {
                    sync.round_until(service, 0, wake).await.map(|_| ())
                }
            };
            tokio::select! {
                result = step => result,
                () = changes => Ok(()),
            }
        };
        match result {
            Ok(()) => self.last_error = None,
            Err(error) => {
                tracing::warn!(kind = ?error.code(), "a sync round failed");
                self.last_error = Some(folder_message(&error));
            }
        }
        if self.sync.as_ref().is_some_and(|sync| sync.revoke_due()) {
            let device = service.credentials().keys.device();
            let revoked = match crate::sync::shares::ShareBook::open(app_data, device) {
                Ok(book) => service.revoke_shares_again(&book).await,
                Err(error) => Err(error),
            };
            match revoked {
                Ok(()) => {
                    let saved = self.sync.as_mut().map(|sync| sync.revoked_again());
                    if let Some(Err(error)) = saved {
                        tracing::warn!(kind = ?error.code(), "keeping the revocation failed");
                    }
                }
                Err(error) => tracing::warn!(kind = ?error.code(), "revoking links again failed"),
            }
        }
        if let Some(sync) = self.sync.as_mut() {
            if sync.take_text_reports().contains(&"sync_text_conflict") {
                self.text_problem = Some(TEXT_CONFLICT.into());
            } else if !sync
                .text_reviews()
                .iter()
                .any(|review| review.kind == TextReviewKind::Disagrees)
            {
                self.text_problem = None;
            }
        }
    }

    /// Carries out what you did in the section.
    pub(super) async fn act(
        &mut self,
        action: FolderSyncAction,
        service: Option<&mut SyncService>,
        app_data: &Path,
    ) {
        let result = self.try_act(action, service, app_data).await;
        if let Err(error) = result {
            self.last_error = Some(folder_message(&error));
        }
    }

    async fn try_act(
        &mut self,
        action: FolderSyncAction,
        service: Option<&mut SyncService>,
        app_data: &Path,
    ) -> Result<(), SyncError> {
        let mut registry = Registry::open(app_data)?;
        match action {
            FolderSyncAction::TurnOn => {
                let (Some(engine), Some(service)) = (self.engine.clone(), service) else {
                    return Err(SyncError::Invalid(
                        "Sign in to a sync account to sync this workspace.".into(),
                    ));
                };
                let known = registry.of_folder(engine.root()).cloned();
                let sync = match known {
                    Some(entry)
                        if entry.server == service.client().base()
                            && entry.account == service.credentials().account =>
                    {
                        WorkspaceSync::turn_on_again(
                            engine,
                            service.credentials().clone(),
                            service.secrets(),
                            &mut registry,
                        )?
                    }
                    _ => {
                        let name = engine.manifest().name;
                        WorkspaceSync::turn_on(service, engine, &name, &mut registry).await?
                    }
                };
                self.sync = Some(sync);
                self.last_error = None;
            }
            FolderSyncAction::TurnOff => {
                if let Some(sync) = self.sync.take() {
                    sync.turn_off(&mut registry)?;
                }
                self.last_error = None;
            }
            FolderSyncAction::Pause | FolderSyncAction::Resume => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.set_paused(&mut registry, action == FolderSyncAction::Pause)?;
                }
                if action == FolderSyncAction::Resume
                    && let (Some(sync), Some(service)) = (self.sync.as_mut(), service)
                {
                    sync.round(service, 0).await?;
                }
            }
            FolderSyncAction::Rename(name) => {
                if let (Some(sync), Some(service)) = (self.sync.as_mut(), service) {
                    match sync.rename(service, &name).await {
                        Ok(()) => {}
                        // Only an owner or admin renames a shared workspace;
                        // the folder keeps its own name.
                        Err(SyncError::Invalid(_)) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            FolderSyncAction::Choose { rid, key } => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.choose(rid, key)?;
                }
            }
            FolderSyncAction::KeepLastGood(rid) => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.keep_last_good(rid)?;
                }
            }
            FolderSyncAction::KeepHeld { rid, choice } => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.resolve_held(rid, choice)?;
                }
            }
            FolderSyncAction::ConfirmFolder(folder) => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.confirm_held_root(&folder)?;
                }
            }
            FolderSyncAction::RestoreMarker(domain) => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.restore_domain_marker(domain)?;
                }
            }
            FolderSyncAction::RestoreArchived(index) | FolderSyncAction::DiscardArchived(index) => {
                let restore = matches!(action, FolderSyncAction::RestoreArchived(_));
                if let Some(sync) = self.sync.as_mut() {
                    sync.resolve_archived(index, restore)?;
                }
            }
            FolderSyncAction::ConfirmDeletedAway(path) => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.confirm_deleted_away(&path)?;
                }
            }
            FolderSyncAction::KeepDeletedAway(path) => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.keep_deleted_away(&path)?;
                }
            }
            FolderSyncAction::DismissRestore => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.dismiss_restore_alert()?;
                }
            }
            // The account screens take it from here: a new sign-in, then
            // the emailed code.
            FolderSyncAction::ResetDevices => self.reset_wanted = true,
            FolderSyncAction::SetUpAgain if self.account_lost.is_some() => {
                let Some(service) = service else {
                    return Err(SyncError::Invalid(
                        "Sign in to a sync account to set this workspace up again.".into(),
                    ));
                };
                self.set_up_after_reset(service, self.account_lost, app_data)
                    .await?;
            }
            FolderSyncAction::SetUpAgain => {
                let (Some(sync), Some(service)) = (self.sync.as_mut(), service) else {
                    return Err(SyncError::Invalid(
                        "Sign in to a sync account to set this workspace up again.".into(),
                    ));
                };
                let fresh = sync.set_up_again(service, &mut registry).await?;
                self.sync = Some(fresh);
            }
            FolderSyncAction::Connect => {
                let (Some(engine), Some(service)) = (self.engine.clone(), service) else {
                    return Err(SyncError::Invalid(
                        "Sign in to a sync account to sync this workspace.".into(),
                    ));
                };
                let named = match registry.of_folder(engine.root()) {
                    Some(entry) => Some((entry.server.clone(), entry.account, entry.workspace)),
                    None => crate::sync::workspace::rebuild::journal_workspace(
                        engine.root(),
                        &service.credentials().keys.device(),
                        &*service.secrets(),
                    ),
                };
                let Some((server, account, workspace)) = named else {
                    return Err(SyncError::Invalid("This folder doesn't sync.".into()));
                };
                registry.connect(&server, account, workspace, engine.root())?;
                self.sync = Some(WorkspaceSync::rebuild(service, engine, &mut registry).await?);
                self.reset_held = false;
                self.not_connected = false;
                self.last_error = None;
            }
        }
        Ok(())
    }
}
