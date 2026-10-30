//! The Members section's state (docs/workspace-format/sync-v2.md §8, §11):
//! the workspace's members with their roles, the open invitations with
//! their role and status, and what your role lets you do: invite, copy
//! the link of an invitation this computer sent, revoke, change a role,
//! remove a member, transfer ownership, or leave.
//!
//! [`MembersScreen`] is plain view state that `app-ui` draws, built by
//! [`MembersScreen::of`] from the verified policy and the server's list of
//! invitations, so the rights follow §8's authority table in one place.
//! What you do arrives as a [`MembersAction`] at the [`MembersBackend`]
//! the app runs with. The desktop's
//! [`LiveAccount`](crate::account::live::LiveAccount) carries the actions
//! out with its sync account; without one, [`NoMembers`] says that members
//! need an account.

use std::sync::{Arc, Mutex, PoisonError};

pub use sync_protocol::messages::InvitationStatus;
use sync_protocol::policy::{PolicyChain, Snapshot};
pub use sync_protocol::records::{Level, Role};
pub use sync_protocol::types::{Digest, Id};

use crate::account::fingerprint;
use crate::folder_sync::local_time;
pub use crate::shares::LinkText;
use crate::sync::invitations::WorkspaceInvitation;
use crate::sync::joining::InvitedRequest;
use crate::sync::workspace::WorkspaceSync;

/// What the section says on a computer without a sync account.
pub const NEEDS_ACCOUNT: &str = "Members need a sync account. Sign in under Settings > Sync account, then turn on sync for this workspace.";

/// What the section says while the workspace doesn't sync.
pub const NEEDS_SYNC: &str =
    "Turn on sync for this workspace under Settings > Sync to invite people to it.";

/// A role's name.
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "Owner",
        Role::Admin => "Admin",
        Role::Editor => "Editor",
        Role::Viewer => "Viewer",
    }
}

/// One member account of the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberEntry {
    /// The account.
    pub account: Id,
    /// What the list shows: "You" or the account's short ID, with the
    /// role and its devices.
    pub label: String,
    /// Its role.
    pub role: Role,
    /// Whether it's your account.
    pub you: bool,
    /// The roles you may give it instead of its own.
    pub roles: Vec<Role>,
    /// Whether you may remove it.
    pub removable: bool,
    /// Whether you may make it the owner.
    pub transferable: bool,
}

/// One open invitation of the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitationEntry {
    /// The invitation, which the actions name.
    pub invitation: Id,
    /// What the list shows: the role, the status and the expiry.
    pub label: String,
    /// The role it grants.
    pub role: Role,
    /// Pending, or accepted and waiting for approval.
    pub status: InvitationStatus,
    /// Whether this computer sent it, so it can copy the link again and
    /// approve the new member.
    pub sent_here: bool,
    /// Set when the account that accepted it was a member before: once
    /// approved, it can read what members wrote while it was away (§9).
    pub returning: Option<String>,
}

/// An invited device that waits for this computer's approval (§10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRequest {
    /// The request's commitment, which the action names.
    pub commit: Digest,
    /// What the row says.
    pub label: String,
    /// The device's fingerprint.
    pub fingerprint: String,
    /// Set when the account was a member before (§9).
    pub returning: Option<String>,
}

/// An approval in progress on this computer (§10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberApproval {
    /// The new device's fingerprint.
    pub fingerprint: String,
    /// The approval code once the new device revealed its nonce.
    pub code: Option<String>,
    /// Set when the account was a member before (§9).
    pub returning: Option<String>,
}

/// An account on a restricted folder's access list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderGrant {
    /// The account.
    pub account: Id,
    /// What the list shows.
    pub label: String,
    /// Whether you may take it off the list.
    pub removable: bool,
    /// Whether you may hand the folder to it and take yourself off.
    pub may_hand_off: bool,
}

/// A member you may give access to a restricted folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderCandidate {
    /// The account.
    pub account: Id,
    /// What the list shows.
    pub label: String,
    /// Set when the account had access before: it can read what people
    /// wrote there while it was away (§9).
    pub returning: Option<String>,
}

/// One restricted folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    /// The domain.
    pub domain: Id,
    /// The folder's path, when this device knows it.
    pub label: String,
    /// Who reads it.
    pub grants: Vec<FolderGrant>,
    /// Who you may give access.
    pub candidates: Vec<FolderCandidate>,
    /// Whether you may delete the restriction.
    pub deletable: bool,
}

/// A file that waits for you before it reaches more people (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveEntry {
    /// The object.
    pub rid: Id,
    /// What the row says.
    pub label: String,
}

/// A held folder, whose files wait for you (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldEntry {
    /// The folder.
    pub folder: String,
    /// What the row says.
    pub label: String,
}

/// What the Members section shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MembersScreen {
    /// Why this computer can't manage members, such as when it has no
    /// sync account. The section then shows only this.
    pub unavailable: Option<String>,
    /// Your role.
    pub you: Option<Role>,
    /// The members, you first.
    pub members: Vec<MemberEntry>,
    /// The open invitations, which owners and admins see.
    pub invitations: Vec<InvitationEntry>,
    /// The invited devices that wait for this computer's approval.
    pub requests: Vec<MemberRequest>,
    /// The approval in progress on this computer.
    pub approval: Option<MemberApproval>,
    /// The roles you may invite people with.
    pub invite_roles: Vec<Role>,
    /// Whether you may leave the workspace; the owner transfers ownership
    /// first.
    pub may_leave: bool,
    /// Whether you may restrict a folder.
    pub may_restrict: bool,
    /// The restricted folders you read or manage.
    pub folders: Vec<FolderEntry>,
    /// The files that would reach more people, waiting for you.
    pub moves: Vec<MoveEntry>,
    /// The held folders.
    pub held: Vec<HeldEntry>,
    /// The warning about files you moved into a restricted folder.
    pub moved_in: Option<String>,
    /// A link to put on the clipboard, which the window copies once.
    pub copy: Option<LinkText>,
    /// What the last action did.
    pub notice: Option<String>,
    /// What went wrong last.
    pub problem: Option<String>,
    /// Whether an action is running.
    pub busy: bool,
}

fn short(id: &Id) -> String {
    id.to_hex().chars().take(8).collect()
}

fn status_text(status: InvitationStatus) -> &'static str {
    match status {
        InvitationStatus::Pending => "waiting for them to accept",
        InvitationStatus::Accepted => "accepted, waiting for approval",
    }
}

/// The text of a member's or an invitation's role, in lower case.
fn role_word(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Editor => "editor",
        Role::Viewer => "viewer",
    }
}

impl MembersScreen {
    /// The section for `account` in the workspace whose verified policy
    /// head is `snapshot`, with the server's open `invitations`. The
    /// rights follow §8: an owner gives any role but owner and removes
    /// anyone else, an admin moves editors and viewers between those two
    /// roles and removes them, and only the owner invites admins and
    /// transfers ownership.
    pub fn of(snapshot: &Snapshot, account: Id, invitations: &[WorkspaceInvitation]) -> Self {
        let you = snapshot.role(&account);
        let devices = |member: &Id| {
            snapshot
                .devices
                .values()
                .filter(|d| d.account == *member && !d.recovery)
                .count()
        };
        let mut members: Vec<MemberEntry> = snapshot
            .members
            .iter()
            .map(|(id, member)| {
                let mine = *id == account;
                let roles = match (you, member.role) {
                    (_, Role::Owner) => Vec::new(),
                    _ if mine => Vec::new(),
                    (Some(Role::Owner), current) => [Role::Admin, Role::Editor, Role::Viewer]
                        .into_iter()
                        .filter(|r| *r != current)
                        .collect(),
                    (Some(Role::Admin), Role::Editor) => vec![Role::Viewer],
                    (Some(Role::Admin), Role::Viewer) => vec![Role::Editor],
                    _ => Vec::new(),
                };
                let removable = !mine
                    && match (you, member.role) {
                        (_, Role::Owner) => false,
                        (Some(Role::Owner), _) => true,
                        (Some(Role::Admin), Role::Editor | Role::Viewer) => true,
                        _ => false,
                    };
                let count = devices(id);
                let who = if mine {
                    "You".to_owned()
                } else {
                    format!("Account {}", short(id))
                };
                let devices = match count {
                    1 => "1 device".to_owned(),
                    n => format!("{n} devices"),
                };
                MemberEntry {
                    account: *id,
                    label: format!("{who}, {}, {devices}", role_word(member.role)),
                    role: member.role,
                    you: mine,
                    roles,
                    removable,
                    transferable: you == Some(Role::Owner) && !mine,
                }
            })
            .collect();
        members.sort_by_key(|m| (!m.you, m.role, m.account));
        let manages = matches!(you, Some(Role::Owner | Role::Admin));
        let invitations = if manages {
            invitations
                .iter()
                .map(|i| InvitationEntry {
                    invitation: i.invitation,
                    label: format!(
                        "{} invitation, {}, expires {}",
                        role_name(i.role),
                        status_text(i.status),
                        local_time(i.expires_at)
                    ),
                    role: i.role,
                    status: i.status,
                    sent_here: i.sent_here,
                    returning: i
                        .accepted_by
                        .filter(|a| snapshot.former.contains_key(a))
                        .map(|_| {
                            "This account was a member before. Once you approve it, it can read what members wrote while it was away.".to_owned()
                        }),
                })
                .collect()
        } else {
            Vec::new()
        };
        let invite_roles = match you {
            Some(Role::Owner) => vec![Role::Admin, Role::Editor, Role::Viewer],
            Some(Role::Admin) => vec![Role::Editor, Role::Viewer],
            _ => Vec::new(),
        };
        Self {
            unavailable: None,
            you,
            members,
            invitations,
            invite_roles,
            may_leave: you.is_some_and(|r| r != Role::Owner),
            may_restrict: manages,
            ..Self::default()
        }
    }

    /// Adds the invited devices that wait for this computer: `requests`
    /// holds only those whose proof checked out (§11).
    pub fn with_requests(mut self, snapshot: &Snapshot, requests: &[InvitedRequest]) -> Self {
        self.requests = requests
            .iter()
            .map(|request| MemberRequest {
                commit: request.commit,
                label: format!("{} invitation, accepted", role_name(request.role)),
                fingerprint: fingerprint(&request.fingerprint),
                returning: snapshot.former.contains_key(&request.card.account).then(|| {
                    "This account was a member before. Once you approve it, it can read what members wrote while it was away.".to_owned()
                }),
            })
            .collect();
        self
    }

    /// Adds the restricted folders, and what waits for you on this device.
    pub fn with_folders(mut self, policy: &PolicyChain, account: Id, sync: &WorkspaceSync) -> Self {
        let snapshot = policy.head_snapshot();
        let admin = matches!(snapshot.role(&account), Some(Role::Owner | Role::Admin));
        let name = |id: &Id| {
            if *id == account {
                "You".to_owned()
            } else {
                format!("Account {}", short(id))
            }
        };
        for (domain, entry) in snapshot.domains.iter().filter(|(_, d)| d.restricted) {
            let manages = snapshot.manages(&account, domain);
            if !manages && !admin && !entry.access.contains_key(&account) {
                continue;
            }
            let label = sync
                .domain_roots()
                .iter()
                .find(|root| root.domain == *domain)
                .map_or_else(
                    || format!("Restricted folder {}", short(domain)),
                    |root| root.folder.clone(),
                );
            let grants = entry
                .access
                .iter()
                .map(|(id, level)| FolderGrant {
                    account: *id,
                    label: format!("{}, {}", name(id), level_word(*level)),
                    removable: *id != account && (manages || admin),
                    may_hand_off: *id != account && manages,
                })
                .collect();
            let had_access = |id: &Id| {
                (1..snapshot.revision).any(|revision| {
                    policy
                        .at(revision)
                        .and_then(|s| s.domains.get(domain))
                        .is_some_and(|d| d.access.contains_key(id))
                })
            };
            let candidates = if manages {
                snapshot
                    .members
                    .keys()
                    .filter(|id| !entry.access.contains_key(id))
                    .map(|id| FolderCandidate {
                        account: *id,
                        label: name(id),
                        returning: had_access(id).then(|| {
                            "This account had access before. Grant it again and it can read what people wrote here while it was away.".to_owned()
                        }),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            self.folders.push(FolderEntry {
                domain: *domain,
                label,
                grants,
                candidates,
                deletable: manages || admin,
            });
        }
        let people = |n: usize| match n {
            1 => "1 more person".to_owned(),
            n => format!("{n} more people"),
        };
        self.moves = sync
            .moves_for_review()
            .iter()
            .map(|review| MoveEntry {
                rid: review.rid,
                label: format!(
                    "{} left a restricted folder. Moving it shows it to {}.",
                    review.path,
                    people(review.people)
                ),
            })
            .collect();
        let held = sync.held_files();
        let mut folders: Vec<&str> = held.iter().map(|f| f.folder.as_str()).collect();
        folders.dedup();
        self.held = folders
            .into_iter()
            .map(|folder| {
                let files: Vec<_> = held.iter().filter(|f| f.folder == folder).collect();
                let reach = files.iter().map(|f| f.people).max().unwrap_or(0);
                let count = match files.len() {
                    1 => "1 file".to_owned(),
                    n => format!("{n} files"),
                };
                HeldEntry {
                    folder: folder.to_owned(),
                    label: format!(
                        "{count} in {folder} wait for you. Confirming shows them to {}.",
                        people(reach)
                    ),
                }
            })
            .collect();
        self.moved_in = match sync.moved_in() {
            [] => None,
            [(_, path)] => Some(format!(
                "You moved {path} into a restricted folder. Its earlier versions stay in the log of the folder it left, and anyone who reads that log keeps them."
            )),
            many => Some(format!(
                "You moved {} files into a restricted folder. Their earlier versions stay in the log of the folder they left, and anyone who reads that log keeps them.",
                many.len()
            )),
        };
        self
    }
}

fn level_word(level: Level) -> &'static str {
    match level {
        Level::Read => "can read",
        Level::Write => "can edit",
        Level::Manage => "manages",
    }
}

/// What you do in the Members section.
#[derive(Clone, PartialEq, Eq)]
pub enum MembersAction {
    /// The section opened: list the members and invitations again.
    Refresh,
    /// Create an invitation with this role and copy its link.
    Invite {
        /// The role it grants.
        role: Role,
        /// Your name, as the invitee sees it.
        your_name: String,
    },
    /// Copy again the link of an invitation this computer sent.
    CopyLink(Id),
    /// The window copied the link.
    Copied,
    /// Revoke an invitation, so its link stops working.
    Revoke(Id),
    /// Give a member another role.
    SetRole {
        /// The member's account.
        account: Id,
        /// The new role.
        role: Role,
    },
    /// Remove a member with every one of its devices.
    Remove(Id),
    /// Make another member the owner; you become an admin.
    TransferOwner(Id),
    /// Leave the workspace.
    Leave,
    /// Offer to approve the invited device with this commitment.
    Approve(Digest),
    /// The approval codes match: add the member.
    ConfirmApproval,
    /// End the approval in progress.
    CancelApproval,
    /// Restrict a folder to you, for now.
    Restrict(String),
    /// Give an account access to a restricted folder.
    Grant {
        /// The folder's domain.
        domain: Id,
        /// The account.
        account: Id,
        /// What it may do.
        level: Level,
    },
    /// Take an account off a restricted folder.
    TakeOff {
        /// The folder's domain.
        domain: Id,
        /// The account.
        account: Id,
    },
    /// Hand a restricted folder to an account and take yourself off.
    HandOff {
        /// The folder's domain.
        domain: Id,
        /// The account.
        account: Id,
    },
    /// Delete a restricted folder's restriction.
    DeleteFolder(Id),
    /// Confirm a move that would show a file to more people.
    ConfirmMove(Id),
    /// Confirm where a held folder's files go.
    ConfirmHeld(String),
    /// Dismiss the warning about files moved into a restricted folder.
    DismissMovedIn,
}

impl std::fmt::Debug for MembersAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MembersAction::Refresh => f.write_str("Refresh"),
            MembersAction::Invite { role, .. } => f
                .debug_struct("Invite")
                .field("role", role)
                .finish_non_exhaustive(),
            MembersAction::CopyLink(id) => f.debug_tuple("CopyLink").field(id).finish(),
            MembersAction::Copied => f.write_str("Copied"),
            MembersAction::Revoke(id) => f.debug_tuple("Revoke").field(id).finish(),
            MembersAction::SetRole { account, role } => f
                .debug_struct("SetRole")
                .field("account", account)
                .field("role", role)
                .finish(),
            MembersAction::Remove(id) => f.debug_tuple("Remove").field(id).finish(),
            MembersAction::TransferOwner(id) => f.debug_tuple("TransferOwner").field(id).finish(),
            MembersAction::Leave => f.write_str("Leave"),
            MembersAction::Approve(commit) => f.debug_tuple("Approve").field(commit).finish(),
            MembersAction::ConfirmApproval => f.write_str("ConfirmApproval"),
            MembersAction::CancelApproval => f.write_str("CancelApproval"),
            MembersAction::Restrict(_) => f.write_str("Restrict"),
            MembersAction::Grant { domain, level, .. } => f
                .debug_struct("Grant")
                .field("domain", domain)
                .field("level", level)
                .finish_non_exhaustive(),
            MembersAction::TakeOff { domain, .. } => f
                .debug_struct("TakeOff")
                .field("domain", domain)
                .finish_non_exhaustive(),
            MembersAction::HandOff { domain, .. } => f
                .debug_struct("HandOff")
                .field("domain", domain)
                .finish_non_exhaustive(),
            MembersAction::DeleteFolder(id) => f.debug_tuple("DeleteFolder").field(id).finish(),
            MembersAction::ConfirmMove(id) => f.debug_tuple("ConfirmMove").field(id).finish(),
            MembersAction::ConfirmHeld(_) => f.write_str("ConfirmHeld"),
            MembersAction::DismissMovedIn => f.write_str("DismissMovedIn"),
        }
    }
}

/// Carries out what you do in the Members section and reports what it
/// shows.
pub trait MembersBackend: Send + Sync {
    /// What the section shows now.
    fn screen(&self) -> MembersScreen;
    /// Starts `action`; the section changes once it runs.
    fn act(&self, action: MembersAction);
    /// Calls `wake` whenever the section changes.
    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>);
}

/// The backend of an app without a sync account.
pub struct NoMembers {
    screen: Mutex<MembersScreen>,
}

impl Default for NoMembers {
    fn default() -> Self {
        Self {
            screen: Mutex::new(MembersScreen {
                unavailable: Some(NEEDS_ACCOUNT.into()),
                ..MembersScreen::default()
            }),
        }
    }
}

impl MembersBackend for NoMembers {
    fn screen(&self) -> MembersScreen {
        self.screen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn act(&self, _action: MembersAction) {}

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}
