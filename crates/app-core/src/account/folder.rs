//! The open workspace's sync rounds, which the account backend's thread
//! runs with the account's sync service (docs/workspace-format/sync-v2.md
//! §15): it opens the folder's journal when a workspace opens, runs one
//! round each tick unless you paused sync, and keeps the Workspace sync
//! section's state in step.
//!
//! The rounds share the account's one service, so the device keeps one
//! view of its account chains. Until the realtime channel arrives (ticket
//! 61), each round's pull asks once and returns at once, every tick.

use std::path::PathBuf;
use std::sync::Arc;

use local_core::WorkspaceEngine;

use crate::folder_sync::{
    BranchEntry, ConflictEntry, FolderSyncAction, FolderSyncPhase, FolderSyncScreen,
};
use crate::sync::registry::Registry;
use crate::sync::workspace::WorkspaceSync;
use crate::sync::{Secrets, SyncError, SyncService};

/// The open workspace and its sync.
#[derive(Default)]
pub(super) struct Folder {
    engine: Option<Arc<WorkspaceEngine>>,
    sync: Option<WorkspaceSync>,
    last_error: Option<String>,
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
                screen
            }
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
        app_data: &PathBuf,
    ) {
        self.sync = None;
        self.last_error = None;
        self.engine = engine;
        self.open(service, store, app_data);
    }

    fn open(&mut self, service: Option<&SyncService>, store: &Secrets, app_data: &PathBuf) {
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
            Err(error) => self.last_error = Some(folder_message(&error)),
        }
    }

    /// One round, unless you paused sync, when a workspace that syncs is
    /// open. A failed round keeps the outbox and shows why.
    pub(super) async fn round(
        &mut self,
        service: Option<&mut SyncService>,
        store: &Secrets,
        app_data: &PathBuf,
    ) {
        let Some(service) = service else {
            return;
        };
        self.open(Some(service), store, app_data);
        let Some(sync) = self.sync.as_mut() else {
            return;
        };
        let result = if sync.paused() {
            sync.capture().map(|_| ())
        } else {
            sync.round(service, 0).await.map(|_| ())
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
        app_data: &PathBuf,
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
        app_data: &PathBuf,
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
            FolderSyncAction::Choose { rid, key } => {
                if let Some(sync) = self.sync.as_mut() {
                    sync.choose(rid, key)?;
                }
            }
        }
        Ok(())
    }
}
