//! The sync account screens' state (docs/workspace-format/sync-v2.md §7,
//! §10, §12): the server address, signing in through the browser, a new
//! device waiting for approval, the signed-in account with its fingerprint
//! and every device with its full fingerprint, approving, removing, the
//! recovery kit and signing out.
//!
//! [`AccountScreen`] is plain view state that `app-ui` draws; what you do
//! there arrives as an [`AccountAction`] at the [`AccountBackend`] the app
//! runs with. [`live::LiveAccount`] carries the actions out against a sync
//! server on its own thread; tests supply a fake.

mod folder;
pub mod live;
mod members;
mod share;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use sync_protocol::records::fingerprint_text;
use sync_protocol::types::Digest;
pub use sync_protocol::types::Id;
use zeroize::Zeroizing;

/// What the screens say when the server no longer takes this computer's
/// session.
pub(crate) const SESSION_ENDED: &str = "This computer's session with the sync server ended. Sign in again with your browser, and this device keeps its keys.";

/// The sentence the screens show for `error`, for the errors a screen has
/// no sentence of its own for. Protocol codes and system details stay out
/// of it: the caller logs the error's code.
pub(crate) fn problem_text(error: &crate::sync::SyncError) -> String {
    use crate::sync::SyncError;
    match error {
        SyncError::ProtocolUnsupported(_) => {
            "This sync server runs a version of sync that Noura can't use. Update Noura, or ask the server's operator.".into()
        }
        SyncError::Refused { code, .. } => refused_text(code).into(),
        SyncError::Network(_) => "The sync server can't be reached. Try again in a moment.".into(),
        SyncError::Check(_) => {
            "Sync stopped because the server's answer failed a check. Your files stay on this computer.".into()
        }
        SyncError::KeyMissing => {
            "This workspace's key hasn't reached this computer yet. Another device sends it the next time it syncs.".into()
        }
        SyncError::Invalid(text) => text.clone(),
        SyncError::SignIn(end) => end.to_string(),
        SyncError::FileChanged | SyncError::KitRevoked => error.to_string(),
    }
}

/// What a refusal from the server means to you.
fn refused_text(code: &str) -> &'static str {
    match code {
        "unauthorized" => SESSION_ENDED,
        "device_removed" => "The sync server says this computer no longer belongs to your account.",
        "forbidden" => "Your role in this workspace doesn't allow that.",
        "not_found" => "The sync server no longer has that.",
        "rate_limited" => "The sync server asked Noura to slow down. Try again in a minute.",
        "quota_exceeded" => "This workspace is over its storage quota.",
        "workspace_limit" => "Your account has reached its limit of workspaces.",
        "recipient_limit" => "This workspace has reached its limit of 512 devices.",
        _ => "The sync server refused that. Try again in a moment.",
    }
}

/// A policy change, as what its signer did (§8).
pub(crate) fn policy_change_words(kind: &str) -> &'static str {
    match kind {
        "genesis" => "started the workspace",
        "add_member" => "added a member",
        "set_role" => "changed a member's role",
        "remove_member" => "removed a member",
        "create_domain" => "restricted a folder",
        "set_access" => "changed who can open a restricted folder",
        "delete_domain" => "deleted a restricted folder",
        "transfer_owner" => "transferred ownership",
        "accept_reset" => "accepted a member's device reset",
        "account_heads" => "recorded new devices",
        _ => "changed the workspace",
    }
}

/// An account chain link, as what its signer did (§7).
pub(crate) fn link_words(kind: &str) -> &'static str {
    match kind {
        "genesis" => "started your account",
        "add_device" => "added a device to your account",
        "add_recovery" => "added a recovery kit to your account",
        "recover_device" => "recovered your account with a kit",
        "remove" => "removed a device from your account",
        _ => "changed your account",
    }
}

/// A fingerprint as the screens show it: 64 lowercase hexadecimal
/// characters in 16 groups of 4 (§7).
pub fn fingerprint(digest: &Digest) -> String {
    fingerprint_text(digest)
}

/// What the account screens show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountScreen {
    /// The sync server's address, as you typed it.
    pub server: String,
    /// Why the address doesn't work, such as plain HTTP away from your own
    /// computer.
    pub server_problem: Option<String>,
    /// Where the account stands.
    pub phase: AccountPhase,
    /// Whether an action is running.
    pub busy: bool,
    /// What the last action did wrong, for you to read.
    pub problem: Option<String>,
    /// Whether this computer holds a device that couldn't connect, such as
    /// when the server can't be reached, so you can try again.
    pub can_retry: bool,
    /// The invitation you opened (§11).
    pub invitation: Option<InvitationView>,
}

/// An invitation you opened, as its encrypted context reads (§11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitationView {
    /// The workspace's name.
    pub workspace: String,
    /// The inviter's name.
    pub inviter: String,
    /// The role it grants, in lower case.
    pub role: String,
    /// How far you got.
    pub stage: InvitationStage,
}

/// How far an invitation you opened got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvitationStage {
    /// You read it and haven't accepted.
    Opened,
    /// You accepted, and the inviting computer approves this one (§10).
    Waiting {
        /// This device's fingerprint.
        fingerprint: String,
        /// The approval code once an offer arrived, as 8 digits in two
        /// groups of 4.
        code: Option<String>,
        /// Whether you confirmed the code on this device.
        confirmed: bool,
    },
    /// You joined; the workspace waits for a folder.
    Joined {
        /// Whether the files started to download.
        downloading: bool,
    },
}

/// Where this device's account stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AccountPhase {
    /// No account on this computer.
    #[default]
    SignedOut,
    /// The browser shows the device page; you approve the code there.
    SigningIn {
        /// The code, as two groups of 4.
        user_code: String,
    },
    /// This new device waits for a device you hold to approve it (§10).
    WaitingForApproval {
        /// This device's fingerprint.
        fingerprint: String,
        /// The approval code once an offer arrived, as 8 digits in two
        /// groups of 4.
        code: Option<String>,
        /// Whether you confirmed the code on this device.
        confirmed: bool,
    },
    /// A recovery with a kit waits for you to confirm what changed since
    /// the kit (§12).
    Recovering(RecoveryView),
    /// The account on this computer.
    SignedIn(Box<SignedInView>),
    /// Reset devices: the emailed code starts a new chain (§7).
    ResetCode,
    /// A restore lost the account head your kit names, so the kit can't
    /// recover your account and Reset devices is the way back (§14).
    KitLost,
    /// This device stopped trusting its own account's chain, which stops
    /// every workspace (§14): Reset devices is the way on, and trying again
    /// judges the chain afresh.
    ChainStopped,
    /// A device of your account removed this one (§7).
    Removed {
        /// The fingerprint of the device that removed it.
        by: String,
    },
    /// A reset retired this device's chain (§7): it signs in as a new
    /// device, and a device on the new chain approves it.
    Retired {
        /// The account's new fingerprint.
        fingerprint: String,
    },
}

/// The signed-in account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedInView {
    /// The account's fingerprint.
    pub account_fingerprint: String,
    /// The account's devices, this one first, then removed ones.
    pub devices: Vec<DeviceEntry>,
    /// What the device list warns you about.
    pub alerts: Vec<AlertEntry>,
    /// New devices of your account that wait for approval.
    pub requests: Vec<RequestEntry>,
    /// The approval in progress on this device.
    pub approval: Option<ApprovalView>,
    /// The removal you're about to confirm.
    pub removal: Option<RemovalView>,
    /// The workspace changes that the devices you removed made, which stay
    /// in force, one line each with the undo your role allows (§7).
    pub removed_changes: Vec<String>,
    /// The kit replacement you're about to confirm, which takes down the
    /// devices the old kit recovered unless you keep them (§12).
    pub kit_replacement: Option<KitReplacementView>,
    /// A recovery kit you just exported, with its secret shown once.
    pub kit: Option<KitView>,
    /// The members a recovery found added to your workspaces after the
    /// kit's heads, one line each, for you to check (§12).
    pub added_members: Vec<String>,
    /// Whether the app asks you to export a new kit, after a removal or
    /// after you join a workspace.
    pub offer_kit: bool,
    /// The sign-out you're about to confirm.
    pub sign_out: Option<SignOutView>,
    /// The account's new fingerprint, after you reset your devices (§7).
    pub reset_fingerprint: Option<String>,
    /// Each workspace that still has to accept your reset (§7, §10).
    pub reset_waits: Vec<ResetWait>,
    /// Members whose reset you can accept in this workspace.
    pub reset_requests: Vec<RequestEntry>,
    /// The acceptance in progress on this device.
    pub reset_approval: Option<ApprovalView>,
}

/// A workspace that has to accept your reset, as the reset device sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetWait {
    /// The workspace, by its short ID.
    pub workspace: String,
    /// The approval code once an owner or admin answered.
    pub code: Option<String>,
    /// Whether you confirmed the code here.
    pub confirmed: bool,
    /// Whether this folder may start over instead: your account owns the
    /// workspace and the folder syncs it (§14).
    pub set_up_again: bool,
}

/// One device of your account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    /// The card's device ID.
    pub device: Id,
    /// Its full fingerprint.
    pub fingerprint: String,
    /// How it joined.
    pub how: String,
    /// Whether it's this device.
    pub this_device: bool,
    /// Whether it's a recovery identity.
    pub recovery: bool,
    /// Whether a link removed it.
    pub removed: bool,
}

/// One warning of the device list (§7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertEntry {
    /// What it says.
    pub text: String,
    /// The enrolled card it names when your chain doesn't list it, whose
    /// sessions any device of yours can end.
    pub end_sessions: Option<Id>,
    /// The account whose refused chain you can have this device judge
    /// again (§7).
    pub check_again: Option<Id>,
}

/// A new device's approval request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestEntry {
    /// The new device's fingerprint.
    pub fingerprint: String,
}

/// An approval in progress on the approving device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalView {
    /// The new device's fingerprint.
    pub fingerprint: String,
    /// The approval code once the new device revealed its nonce.
    pub code: Option<String>,
}

/// A removal you're about to confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalView {
    /// The device you picked.
    pub target: Id,
    /// Its fingerprint.
    pub fingerprint: String,
    /// The devices that go with it, with their fingerprints and whether
    /// you may keep each.
    pub with_it: Vec<(Id, String, bool)>,
    /// The devices you chose to keep.
    pub keep: BTreeSet<Id>,
}

/// A new kit you're about to export over the active one (§12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitReplacementView {
    /// The new kit file.
    pub path: PathBuf,
    /// The devices the old kit recovered, which replacing it takes down
    /// unless you keep them, with their fingerprints.
    pub devices: Vec<(Id, String)>,
    /// The devices you chose to keep.
    pub keep: BTreeSet<Id>,
}

/// A recovery kit you just exported.
#[derive(Clone, PartialEq, Eq)]
pub struct KitView {
    /// The kit file.
    pub path: PathBuf,
    /// The secret, as 8 groups of 4, wiped from memory when the view goes.
    pub secret: Zeroizing<String>,
}

impl std::fmt::Debug for KitView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KitView")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// A sign-out you're about to confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignOutView {
    /// Whether this is the account's last active device.
    pub last_device: bool,
}

/// What a recovery shows you before it goes on (§12, step 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryView {
    /// What changed since the kit, one line each.
    pub changes: Vec<String>,
    /// The members of each workspace the kit doesn't list, one line per
    /// workspace.
    pub members: Vec<String>,
    /// The workspaces of your account the recovery couldn't verify, one
    /// line each with the error; the recovery leaves them out.
    pub unverified: Vec<String>,
    /// Your other devices, which the recovery removes unless you keep one
    /// you still hold, with their fingerprints.
    pub devices: Vec<(Id, String)>,
    /// The devices you chose to keep.
    pub keep: BTreeSet<Id>,
}

/// What you do on the account screens.
#[derive(Clone, PartialEq, Eq)]
pub enum AccountAction {
    /// You typed a server address.
    SetServer(String),
    /// Sign in through the browser.
    SignIn,
    /// Stop signing in.
    CancelSignIn,
    /// Connect the device this computer holds again, after it couldn't.
    Retry,
    /// The approval codes match, on this new device.
    ConfirmCode,
    /// Approve the request at this index of the list.
    Approve(usize),
    /// The approval codes match, on the approving device.
    ConfirmApproval,
    /// End the approval in progress.
    CancelApproval,
    /// Start removing a device.
    Remove(Id),
    /// Keep or stop keeping a device that goes with the removal, or that a
    /// recovery would remove.
    ToggleKeep(Id),
    /// Remove the device and the ones that go with it.
    ConfirmRemove,
    /// Stop removing.
    CancelRemove,
    /// End the sessions of an enrolled card your chain doesn't list.
    EndSessions(Id),
    /// Export a recovery kit to this new file.
    ExportKit(PathBuf),
    /// Replace the active kit, taking down the devices it recovered except
    /// the ones you keep.
    ConfirmExportKit,
    /// Keep the active kit.
    CancelExportKit,
    /// You wrote the secret down.
    DismissKit,
    /// Recover your account with a kit and its secret.
    Recover {
        /// The kit file.
        path: PathBuf,
        /// The secret as you typed it.
        secret: String,
    },
    /// Finish the recovery you reviewed.
    ConfirmRecovery,
    /// Open the invitation link you pasted.
    OpenInvitation(String),
    /// Accept the open invitation, signing in first when you have to.
    AcceptInvitation,
    /// The approval codes match, on the invited device.
    ConfirmInvitationCode,
    /// Close the invitation.
    CancelInvitation,
    /// Download the workspace you joined into this new workspace folder.
    DownloadInvited(PathBuf),
    /// Judge this account's refused chain again (§7).
    CheckChainAgain(Id),
    /// Reset your devices: sign in again and ask for an emailed code (§7).
    StartReset,
    /// Start the new chain with the emailed code.
    ConfirmReset(String),
    /// Your approval code matches the one on the accepting device.
    ConfirmResetWait(usize),
    /// Set up sync again for this reset wait's workspace, because nobody
    /// else can accept the reset (§14).
    SetUpAfterReset(usize),
    /// Answer a member's reset request here.
    AcceptReset(usize),
    /// You compared the code: this device commits the `accept_reset`.
    ConfirmAcceptReset,
    /// Leave the acceptance.
    CancelAcceptReset,
    /// Start signing out.
    SignOut,
    /// Sign out.
    ConfirmSignOut,
    /// Stay signed in.
    CancelSignOut,
}

impl std::fmt::Debug for AccountAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountAction::Recover { path, .. } => f
                .debug_struct("Recover")
                .field("path", path)
                .finish_non_exhaustive(),
            AccountAction::SetServer(server) => f.debug_tuple("SetServer").field(server).finish(),
            AccountAction::SignIn => f.write_str("SignIn"),
            AccountAction::CancelSignIn => f.write_str("CancelSignIn"),
            AccountAction::Retry => f.write_str("Retry"),
            AccountAction::CheckChainAgain(account) => {
                f.debug_tuple("CheckChainAgain").field(account).finish()
            }
            AccountAction::ConfirmCode => f.write_str("ConfirmCode"),
            AccountAction::Approve(index) => f.debug_tuple("Approve").field(index).finish(),
            AccountAction::ConfirmApproval => f.write_str("ConfirmApproval"),
            AccountAction::CancelApproval => f.write_str("CancelApproval"),
            AccountAction::Remove(id) => f.debug_tuple("Remove").field(id).finish(),
            AccountAction::ToggleKeep(id) => f.debug_tuple("ToggleKeep").field(id).finish(),
            AccountAction::ConfirmRemove => f.write_str("ConfirmRemove"),
            AccountAction::CancelRemove => f.write_str("CancelRemove"),
            AccountAction::EndSessions(id) => f.debug_tuple("EndSessions").field(id).finish(),
            AccountAction::ExportKit(path) => f.debug_tuple("ExportKit").field(path).finish(),
            AccountAction::ConfirmExportKit => f.write_str("ConfirmExportKit"),
            AccountAction::CancelExportKit => f.write_str("CancelExportKit"),
            AccountAction::DismissKit => f.write_str("DismissKit"),
            AccountAction::ConfirmRecovery => f.write_str("ConfirmRecovery"),
            AccountAction::OpenInvitation(_) => f.write_str("OpenInvitation"),
            AccountAction::AcceptInvitation => f.write_str("AcceptInvitation"),
            AccountAction::ConfirmInvitationCode => f.write_str("ConfirmInvitationCode"),
            AccountAction::CancelInvitation => f.write_str("CancelInvitation"),
            AccountAction::DownloadInvited(path) => {
                f.debug_tuple("DownloadInvited").field(path).finish()
            }
            AccountAction::StartReset => f.write_str("StartReset"),
            AccountAction::ConfirmReset(_) => f.write_str("ConfirmReset"),
            AccountAction::ConfirmResetWait(index) => {
                f.debug_tuple("ConfirmResetWait").field(index).finish()
            }
            AccountAction::SetUpAfterReset(index) => {
                f.debug_tuple("SetUpAfterReset").field(index).finish()
            }
            AccountAction::AcceptReset(index) => f.debug_tuple("AcceptReset").field(index).finish(),
            AccountAction::ConfirmAcceptReset => f.write_str("ConfirmAcceptReset"),
            AccountAction::CancelAcceptReset => f.write_str("CancelAcceptReset"),
            AccountAction::SignOut => f.write_str("SignOut"),
            AccountAction::ConfirmSignOut => f.write_str("ConfirmSignOut"),
            AccountAction::CancelSignOut => f.write_str("CancelSignOut"),
        }
    }
}

/// Carries out what you do on the account screens and reports where the
/// account stands.
pub trait AccountBackend: Send + Sync {
    /// What the screens show now.
    fn screen(&self) -> AccountScreen;
    /// Starts `action`; the screen changes once it runs.
    fn act(&self, action: AccountAction);
    /// Calls `wake` whenever the screen changes.
    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>);
}

/// The backend of an app that runs without sync, such as a test's: it
/// keeps the address you type and does nothing else.
#[derive(Default)]
pub struct NoAccount {
    screen: Mutex<AccountScreen>,
}

impl AccountBackend for NoAccount {
    fn screen(&self) -> AccountScreen {
        self.screen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn act(&self, action: AccountAction) {
        if let AccountAction::SetServer(server) = action {
            let mut screen = self.screen.lock().unwrap_or_else(PoisonError::into_inner);
            screen.server_problem = crate::sync::device::server_address(&server)
                .err()
                .map(str::to_owned);
            screen.server = server;
        }
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}
