//! The share dialog on the account backend's thread (sync-v2.md §24): it
//! creates, copies, updates and revokes links with the account's sync
//! service and the open workspace's sync, and keeps the dialog's state.

use std::path::Path;

use sync_protocol::policy::Snapshot;
use sync_protocol::records::Role;
use sync_protocol::types::{Digest, Id, to_hex};

use super::folder::Folder;
use crate::folder_sync::local_time;
use crate::shares::{
    DEFAULT_SHARE_DAYS, LinkText, NEEDS_ACCOUNT, NoteLinkEntry, ShareAction, ShareScreen,
    WorkspaceLinkEntry,
};
use crate::sync::shares::{NewShare, ShareBook, snapshot_of};
use crate::sync::{SyncError, SyncService};

/// The note the dialog shares.
struct Note {
    path: String,
    name: String,
    text: String,
}

/// The dialog's state and the note it shares.
#[derive(Default)]
pub(super) struct Sharing {
    pub(super) screen: ShareScreen,
    note: Option<Note>,
}

/// What a failed share action tells you.
fn share_message(error: &SyncError) -> String {
    match error {
        SyncError::Refused { code, .. } if code == "forbidden" => {
            "Only owners and admins of this workspace can share notes.".into()
        }
        SyncError::Refused { code, .. } if code == "share_limit" => {
            "This workspace already has 1,000 active links. Revoke one to make room.".into()
        }
        SyncError::Refused { code, .. } if code == "not_found" => {
            "That link no longer exists. Someone revoked it, or it expired.".into()
        }
        SyncError::Network(_) => "The sync server can't be reached. Try again in a moment.".into(),
        SyncError::Invalid(text) => text.clone(),
        other => format!("Sharing stopped: {other}."),
    }
}

fn short(bytes: &[u8]) -> String {
    to_hex(bytes).chars().take(8).collect()
}

fn manages_links(snapshot: &Snapshot, device: &Id) -> bool {
    snapshot
        .account_of(device)
        .and_then(|account| snapshot.role(&account))
        .is_some_and(|role| matches!(role, Role::Owner | Role::Admin))
}

impl Sharing {
    pub(super) async fn act(
        &mut self,
        action: ShareAction,
        service: Option<&mut SyncService>,
        folder: &mut Folder,
        app_data: &Path,
    ) {
        self.screen.notice = None;
        self.screen.problem = None;
        let result = self.try_act(action, service, folder, app_data).await;
        if let Err(error) = result {
            self.screen.problem = Some(share_message(&error));
        }
        self.screen.busy = false;
    }

    async fn try_act(
        &mut self,
        action: ShareAction,
        service: Option<&mut SyncService>,
        folder: &mut Folder,
        app_data: &Path,
    ) -> Result<(), SyncError> {
        match &action {
            ShareAction::Open { path, name, text } => {
                self.screen = ShareScreen {
                    open: true,
                    note: name.clone(),
                    days: DEFAULT_SHARE_DAYS,
                    ..ShareScreen::default()
                };
                self.note = Some(Note {
                    path: path.clone(),
                    name: name.clone(),
                    text: text.clone(),
                });
            }
            ShareAction::Close => {
                *self = Self::default();
                return Ok(());
            }
            ShareAction::SetDays(days) => {
                self.screen.days = (*days).clamp(1, DEFAULT_SHARE_DAYS);
                return Ok(());
            }
            ShareAction::Copied => {
                self.screen.copy = None;
                return Ok(());
            }
            _ => {}
        }
        let Some(service) = service else {
            self.screen.unavailable = Some(NEEDS_ACCOUNT.into());
            return Ok(());
        };
        let Some(note) = self.note.as_ref() else {
            return Ok(());
        };
        let Some(sync) = folder.sync_mut() else {
            self.screen.unavailable =
                Some("Turn on sync for this workspace under Settings to share its notes.".into());
            return Ok(());
        };
        let Some(rid) = sync.rid_at(&note.path) else {
            self.screen.unavailable =
                Some("This note hasn't synced yet. Share it once sync uploads it.".into());
            return Ok(());
        };
        let (workspace, domain) = (sync.workspace(), sync.domain());
        let Some(remote) = sync.remote_mut() else {
            self.screen.unavailable = Some(
                "This computer hasn't reached the sync server yet. Try again in a moment.".into(),
            );
            return Ok(());
        };
        let snapshot = remote.policy().head_snapshot().clone();
        let device = service.credentials().keys.device();
        if !manages_links(&snapshot, &device) {
            self.screen.unavailable =
                Some("Only owners and admins of this workspace can share notes.".into());
            return Ok(());
        }
        self.screen.unavailable = None;
        let mut book = ShareBook::open(app_data, device)?;
        match action {
            ShareAction::Create => {
                let (title, markdown) = snapshot_of(&note.name, &note.text);
                let created = service
                    .create_share(
                        &mut book,
                        NewShare {
                            workspace,
                            domain,
                            note: rid,
                            title: &title,
                            markdown: markdown.as_bytes(),
                            days: self.screen.days,
                        },
                    )
                    .await?;
                self.screen.copy = Some(LinkText(created.url));
                self.screen.notice = Some(format!(
                    "Link copied. It expires {}.",
                    local_time(created.link.expires_at)
                ));
            }
            ShareAction::Copy(token_hash) => {
                let url = service.share_url(&book, &token_hash)?;
                self.screen.copy = Some(LinkText(url.into()));
                self.screen.notice = Some("Link copied.".into());
            }
            ShareAction::Update(token_hash) => {
                let (title, markdown) = snapshot_of(&note.name, &note.text);
                service
                    .update_share(&mut book, token_hash, &title, markdown.as_bytes())
                    .await?;
                self.screen.notice =
                    Some("Link updated. People who open it now see the note as it is.".into());
            }
            ShareAction::Revoke(token_hash) => {
                let expires_at = self
                    .screen
                    .workspace_links
                    .iter()
                    .find(|l| l.token_hash == token_hash)
                    .map(|l| l.expires_at)
                    .or_else(|| book.link(&token_hash).map(|l| l.expires_at))
                    .unwrap_or_default();
                service
                    .revoke_share(&mut book, workspace, token_hash, expires_at)
                    .await?;
                self.screen.notice = Some("Link revoked. It no longer opens.".into());
            }
            _ => {}
        }
        self.refresh(service, &book, workspace, rid, &snapshot)
            .await
    }

    /// Lists the note's links from this device and the workspace's links.
    async fn refresh(
        &mut self,
        service: &mut SyncService,
        book: &ShareBook,
        workspace: Id,
        rid: Id,
        snapshot: &Snapshot,
    ) -> Result<(), SyncError> {
        let listed = service.workspace_shares(workspace, snapshot).await?;
        let live: Vec<Digest> = listed.iter().map(|l| l.token_hash).collect();
        self.screen.links = book
            .note_links(workspace, rid)
            .into_iter()
            .filter(|l| live.contains(&l.token_hash))
            .map(|l| NoteLinkEntry {
                token_hash: l.token_hash,
                expires: local_time(l.expires_at),
                revision: l.revision,
            })
            .collect();
        let own = book.device();
        self.screen.workspace_links = listed
            .into_iter()
            .map(|l| {
                let from = if l.device == own {
                    "this computer".to_owned()
                } else if l.creator_removed {
                    format!("device {}, which left the workspace", short(&l.device.0))
                } else {
                    format!("device {}", short(&l.device.0))
                };
                WorkspaceLinkEntry {
                    token_hash: l.token_hash,
                    label: format!(
                        "Link {} from {from}, expires {}",
                        short(&l.token_hash.0),
                        local_time(l.expires_at)
                    ),
                    expires_at: l.expires_at,
                    own: l.device == own,
                    creator_removed: l.creator_removed,
                }
            })
            .collect();
        Ok(())
    }
}
