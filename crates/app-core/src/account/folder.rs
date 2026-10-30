//! The open workspace's sync rounds, which the account backend's thread
//! runs with the account's sync service (docs/workspace-format/sync-v2.md
//! §15): it opens the folder's journal when a workspace opens, runs one
//! round each tick unless you paused sync, and keeps the Workspace sync
//! section's state in step.
//!
//! The rounds share the account's one service, so the device keeps one
//! view of its account chains. Each round's pull waits up to
//! [`PULL_WAIT_SECONDS`] for new records when the driver has nothing else
//! to poll. A change in the workspace's files or something you do ends the
//! wait, so local changes go out at once (§15). The realtime channel
//! replaces the wait in ticket 61.

use std::path::Path;
use std::sync::Arc;

use local_core::WorkspaceEngine;

use crate::folder_sync::{
    ArchivedEntry, BranchEntry, ConflictEntry, FolderSyncAction, FolderSyncPhase, FolderSyncScreen,
    HeldEntry, HeldRootEntry,
};
use crate::sync::registry::Registry;
use crate::sync::workspace::{PULL_WAIT_SECONDS, WorkspaceSync};
use crate::sync::{Secrets, SyncError, SyncService};

/// The open workspace and its sync.
#[derive(Default)]
pub(super) struct Folder {
    engine: Option<Arc<WorkspaceEngine>>,
    sync: Option<WorkspaceSync>,
    last_error: Option<String>,
    /// The folder lost or refused its journal, so it syncs again from the
    /// server and matches its files at the next round (§15).
    rebuild: bool,
    /// The folder's journal holds another replica ID: it waits for you to
    /// connect it.
    not_connected: bool,
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
                screen.held = sync
                    .held()
                    .into_iter()
                    .map(|(rid, path)| HeldEntry { rid, path })
                    .collect();
                screen.held_roots = sync
                    .held_roots()
                    .into_iter()
                    .map(|(folder, files, people)| HeldRootEntry {
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
        screen.last_error = self.last_error.clone();
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
        self.last_error = None;
        self.rebuild = false;
        self.not_connected = false;
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
            Err(SyncError::Check("sync_journal_lost" | "sync_journal_rollback")) => {
                self.rebuild = true;
            }
            Err(SyncError::Check("sync_folder_not_connected")) => self.not_connected = true,
            Err(error) => self.last_error = Some(folder_message(&error)),
        }
    }

    /// The open workspace's sync, when it syncs.
    pub(super) fn sync_mut(&mut self) -> Option<&mut WorkspaceSync> {
        self.sync.as_mut()
    }

    /// Whether the open workspace syncs and isn't paused, so a round's
    /// pull may wait for new records.
    pub(super) fn waits(&self) -> bool {
        self.sync
            .as_ref()
            .is_some_and(|sync| !sync.paused() && sync.status().ended.is_none())
    }

    /// One round, unless you paused sync, when a workspace that syncs is
    /// open. Its pull waits up to [`PULL_WAIT_SECONDS`] when `long` and
    /// until `wake` or a change in the workspace's files ends the wait. A
    /// failed round keeps the outbox and shows why.
    pub(super) async fn round(
        &mut self,
        service: Option<&mut SyncService>,
        store: &Secrets,
        app_data: &Path,
        long: bool,
        wake: &tokio::sync::Notify,
    ) {
        let Some(service) = service else {
            return;
        };
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
        let result = if sync.paused() {
            sync.capture().map(|_| ())
        } else {
            let wait = if long { PULL_WAIT_SECONDS } else { 0 };
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
            tokio::select! {
                result = sync.round_until(service, wait, wake) => result.map(|_| ()),
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
            FolderSyncAction::RestoreArchived(index) | FolderSyncAction::DiscardArchived(index) => {
                let restore = matches!(action, FolderSyncAction::RestoreArchived(_));
                if let Some(sync) = self.sync.as_mut() {
                    sync.resolve_archived(index, restore)?;
                }
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
                self.not_connected = false;
                self.last_error = None;
            }
        }
        Ok(())
    }
}
