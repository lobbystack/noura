//! The Members section on the account backend's thread (sync-v2.md §8,
//! §11): it invites, copies and revokes invitations with the account's
//! sync service, changes roles, removes members and leaves through the
//! open workspace's sync, and lists the section again after each action.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use sync_protocol::records::{Access, Level};
use sync_protocol::types::Id;

use super::folder::Folder;
use crate::account::fingerprint;
use crate::members::{
    LinkText, MemberApproval, MembersAction, MembersScreen, NEEDS_ACCOUNT, NEEDS_SYNC, role_name,
};
use crate::sync::approval::OutstandingOffer;
use crate::sync::invitations::{InvitationBook, NewInvitation};
use crate::sync::joining::InvitedRequest;
use crate::sync::members::MemberChange;
use crate::sync::workspace::WorkspaceSync;
use crate::sync::{SyncError, SyncService};

/// How often the open section lists the members and requests again.
const LIST_AGAIN: Duration = Duration::from_secs(10);

/// What a failed action tells you.
fn members_message(error: &SyncError) -> String {
    match error {
        SyncError::Refused { code, .. } if code == "forbidden" => {
            "Your role in this workspace doesn't allow that.".into()
        }
        SyncError::Check("sync_policy_unauthorized") => {
            "Your role in this workspace doesn't allow that.".into()
        }
        SyncError::Refused { code, .. } if code == "invitation_limit" => {
            "This workspace already has 100 open invitations. Revoke one to make room.".into()
        }
        SyncError::Refused { code, .. } if code == "offer_expired" => {
            "The approval took longer than 10 minutes, so it stopped. Choose Approve to start again.".into()
        }
        SyncError::Network(_) => "The sync server can't be reached. Try again in a moment.".into(),
        SyncError::Invalid(text) => text.clone(),
        other => format!("That didn't work: {other}."),
    }
}

/// A restricted folder's access list at the accepted policy.
fn access_of(sync: &mut WorkspaceSync, domain: Id) -> Result<BTreeMap<Id, Level>, SyncError> {
    sync.remote_mut()
        .and_then(|remote| {
            let snapshot = remote.policy().head_snapshot();
            snapshot.domains.get(&domain).map(|d| d.access.clone())
        })
        .ok_or_else(|| SyncError::Invalid("That restricted folder is gone.".into()))
}

fn entries(list: &BTreeMap<Id, Level>) -> Vec<Access> {
    list.iter()
        .map(|(account, level)| Access {
            account: *account,
            level: *level,
        })
        .collect()
}

/// The section's state between actions.
#[derive(Default)]
pub(super) struct Membership {
    pub(super) screen: MembersScreen,
    /// The invited requests the section lists.
    requests: Vec<InvitedRequest>,
    /// The offer of the approval in progress.
    offer: Option<OutstandingOffer>,
    /// When the section last listed everything; `None` until it opened.
    listed: Option<Instant>,
}

impl Membership {
    /// Whether an approval waits for a code, so the backend keeps polling.
    pub(super) fn waits(&self) -> bool {
        self.offer.is_some()
    }

    /// Follows the approval in progress, and lists the section again every
    /// so often once it opened, so a new request reaches you.
    pub(super) async fn tick(
        &mut self,
        service: Option<&mut SyncService>,
        folder: &mut Folder,
        app_data: &Path,
    ) {
        let Some(service) = service else {
            return;
        };
        if let Some(offer) = self.offer.as_mut() {
            match service.approval_code(offer).await {
                Ok(Some(code)) => {
                    if let Some(approval) = &mut self.screen.approval {
                        approval.code = Some(code);
                    }
                }
                Ok(None) | Err(SyncError::Network(_)) => {}
                // An expired offer, or a nonce that doesn't open the
                // commitment, ends the attempt here too.
                Err(error) => {
                    if let Some(offer) = self.offer.take() {
                        service.cancel_approval(offer);
                    }
                    self.screen.approval = None;
                    self.screen.problem = Some(members_message(&error));
                }
            }
            return;
        }
        if self.listed.is_none_or(|at| at.elapsed() < LIST_AGAIN) {
            return;
        }
        let device = service.credentials().keys.device();
        let listed = match InvitationBook::open(app_data, device) {
            Ok(mut book) => self.refresh(service, folder, &mut book).await,
            Err(error) => Err(error),
        };
        if let Err(error) = listed {
            tracing::warn!(kind = ?error.code(), "the Members section didn't refresh");
            self.listed = Some(Instant::now());
        }
    }

    pub(super) async fn act(
        &mut self,
        action: MembersAction,
        service: Option<&mut SyncService>,
        folder: &mut Folder,
        app_data: &Path,
    ) {
        if action == MembersAction::Copied {
            self.screen.copy = None;
            return;
        }
        let notice = self.screen.notice.take();
        self.screen.problem = None;
        let result = self.try_act(action, service, folder, app_data).await;
        match result {
            Ok(Some(text)) => self.screen.notice = Some(text),
            Ok(None) => self.screen.notice = notice,
            Err(error) => self.screen.problem = Some(members_message(&error)),
        }
        self.screen.busy = false;
    }

    async fn try_act(
        &mut self,
        action: MembersAction,
        service: Option<&mut SyncService>,
        folder: &mut Folder,
        app_data: &Path,
    ) -> Result<Option<String>, SyncError> {
        let Some(service) = service else {
            self.screen = MembersScreen {
                unavailable: Some(NEEDS_ACCOUNT.into()),
                ..MembersScreen::default()
            };
            return Ok(None);
        };
        let Some(sync) = folder.sync_mut() else {
            self.screen = MembersScreen {
                unavailable: Some(NEEDS_SYNC.into()),
                ..MembersScreen::default()
            };
            return Ok(None);
        };
        let workspace = sync.workspace();
        let device = service.credentials().keys.device();
        let mut book = InvitationBook::open(app_data, device)?;
        let notice = match action {
            MembersAction::Refresh | MembersAction::Copied => None,
            MembersAction::Invite { role, your_name } => {
                let name = your_name.trim();
                let created = service
                    .create_invitation(
                        &mut book,
                        NewInvitation {
                            workspace,
                            role,
                            workspace_name: sync.name(),
                            inviter_name: if name.is_empty() { "A member" } else { name },
                        },
                    )
                    .await?;
                self.screen.copy = Some(LinkText(created.url));
                Some(format!(
                    "Noura copied the {} invitation link. Send it over a channel you trust. Only this computer can approve the new member.",
                    role_name(role).to_lowercase()
                ))
            }
            MembersAction::CopyLink(invitation) => {
                let url = service.invitation_url(&book, &invitation)?;
                self.screen.copy = Some(LinkText(url));
                Some("Noura copied the invitation link.".into())
            }
            MembersAction::Revoke(invitation) => {
                service
                    .revoke_invitation(&mut book, workspace, invitation)
                    .await?;
                Some("You revoked the invitation. Its link no longer opens.".into())
            }
            MembersAction::SetRole { account, role } => {
                sync.change_member(service, MemberChange::SetRole { account, role })
                    .await?;
                Some(format!(
                    "Role changed to {}.",
                    role_name(role).to_lowercase()
                ))
            }
            MembersAction::Remove(account) => {
                sync.change_member(service, MemberChange::Remove(account))
                    .await?;
                Some("You removed the member. They keep copies of files they already have.".into())
            }
            MembersAction::TransferOwner(account) => {
                sync.change_member(service, MemberChange::TransferOwner(account))
                    .await?;
                Some("You made them the owner. You're an admin now.".into())
            }
            MembersAction::Leave => {
                let own = service.credentials().account;
                sync.change_member(service, MemberChange::Remove(own))
                    .await?;
                self.screen = MembersScreen {
                    unavailable: Some(
                        "You left this workspace. Its files stay on this computer as they are."
                            .into(),
                    ),
                    ..MembersScreen::default()
                };
                return Ok(None);
            }
            MembersAction::Approve(commit) => {
                let request = self
                    .requests
                    .iter()
                    .find(|request| request.commit == commit)
                    .cloned()
                    .ok_or_else(|| SyncError::Invalid("That request ended.".into()))?;
                if sync.remote_mut().is_none() {
                    sync.connect(service).await?;
                }
                let remote = sync.remote_mut().ok_or(SyncError::KeyMissing)?;
                let offer = service.offer_invited(&request, remote).await?;
                let returning = self
                    .screen
                    .requests
                    .iter()
                    .find(|row| row.commit == commit)
                    .and_then(|row| row.returning.clone());
                self.screen.approval = Some(MemberApproval {
                    fingerprint: fingerprint(&offer.fingerprint()),
                    code: None,
                    returning,
                });
                self.offer = Some(offer);
                None
            }
            MembersAction::ConfirmApproval => {
                let offer = self
                    .offer
                    .take()
                    .ok_or_else(|| SyncError::Invalid("No approval is open.".into()))?;
                // Confirming spends the offer, so the approval leaves the
                // section whether or not it commits.
                self.screen.approval = None;
                sync.approve_member(service, offer).await?;
                Some(
                    "You added the new member. Their computer downloads the workspace next.".into(),
                )
            }
            MembersAction::CancelApproval => {
                if let Some(offer) = self.offer.take() {
                    service.cancel_approval(offer);
                }
                self.screen.approval = None;
                None
            }
            MembersAction::Restrict(path) => {
                let path = path.trim().trim_matches('/').to_owned();
                sync.restrict_folder(service, &path, &[]).await?;
                Some(format!(
                    "You restricted {path}. Only you read it for now. Noura moves its files in as sync runs."
                ))
            }
            MembersAction::Grant {
                domain,
                account,
                level,
            } => {
                let mut list = access_of(sync, domain)?;
                list.insert(account, level);
                sync.set_access(service, domain, &entries(&list)).await?;
                Some("You gave them access. Their devices fetch the folder as sync runs.".into())
            }
            MembersAction::TakeOff { domain, account } => {
                let mut list = access_of(sync, domain)?;
                list.remove(&account);
                sync.set_access(service, domain, &entries(&list)).await?;
                Some(
                    "You took them off. Noura changes the folder's key, so they can't read what people write there next. Copies they already have stay with them.".into(),
                )
            }
            MembersAction::HandOff { domain, account } => {
                sync.hand_off_domain(service, domain, account).await?;
                Some("You handed the folder over and took yourself off. Copies you already have stay on this computer.".into())
            }
            MembersAction::DeleteFolder(domain) => {
                sync.delete_domain(service, domain).await?;
                Some("You deleted the restriction. Each device keeps the files and holds them until you confirm where they go.".into())
            }
            MembersAction::ConfirmMove(rid) => {
                sync.confirm_move(rid);
                sync.round(service, 0).await?;
                Some("You confirmed the move.".into())
            }
            MembersAction::ConfirmHeld(path) => {
                sync.confirm_held(&path)?;
                sync.round(service, 0).await?;
                Some("You confirmed where those files go.".into())
            }
            MembersAction::DismissMovedIn => {
                sync.dismiss_moved_in()?;
                None
            }
        };
        // The action stands whether or not the list comes back.
        match self.refresh(service, folder, &mut book).await {
            Err(error) if notice.is_some() => {
                tracing::warn!(kind = ?error.code(), "the Members section didn't refresh");
                Ok(notice)
            }
            Err(error) => Err(error),
            Ok(()) => Ok(notice),
        }
    }

    /// Lists the members and the open invitations again.
    async fn refresh(
        &mut self,
        service: &mut SyncService,
        folder: &mut Folder,
        book: &mut InvitationBook,
    ) -> Result<(), SyncError> {
        let Some(sync) = folder.sync_mut() else {
            return Ok(());
        };
        let workspace = sync.workspace();
        if sync.remote_mut().is_none() {
            sync.connect(service).await?;
        }
        let Some(remote) = sync.remote_mut() else {
            return Ok(());
        };
        service.refresh_policy(remote).await?;
        let policy = remote.policy().clone();
        let snapshot = policy.head_snapshot().clone();
        let account = service.credentials().account;
        let manages = matches!(
            snapshot.role(&account),
            Some(crate::members::Role::Owner | crate::members::Role::Admin)
        );
        let (invitations, requests) = if manages {
            (
                service.workspace_invitations(book, workspace).await?,
                service.invited_requests(book, workspace).await?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let shown = MembersScreen::of(&snapshot, account, &invitations)
            .with_requests(&snapshot, &requests)
            .with_folders(&policy, account, sync);
        self.requests = requests;
        self.listed = Some(Instant::now());
        self.screen = MembersScreen {
            copy: self.screen.copy.take(),
            notice: self.screen.notice.take(),
            problem: self.screen.problem.take(),
            approval: self.screen.approval.take(),
            ..shown
        };
        Ok(())
    }
}
