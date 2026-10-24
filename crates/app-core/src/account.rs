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

pub mod live;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use sync_protocol::records::fingerprint_text;
use sync_protocol::types::Digest;
pub use sync_protocol::types::Id;

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
    SignedIn(SignedInView),
    /// A device of your account removed this one (§7).
    Removed {
        /// The fingerprint of the device that removed it.
        by: String,
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
    pub alerts: Vec<String>,
    /// New devices of your account that wait for approval.
    pub requests: Vec<RequestEntry>,
    /// The approval in progress on this device.
    pub approval: Option<ApprovalView>,
    /// The removal you're about to confirm.
    pub removal: Option<RemovalView>,
    /// A recovery kit you just exported, with its secret shown once.
    pub kit: Option<KitView>,
    /// Whether the app asks you to export a new kit, after a removal or
    /// after you join a workspace.
    pub offer_kit: bool,
    /// The sign-out you're about to confirm.
    pub sign_out: Option<SignOutView>,
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

/// A recovery kit you just exported.
#[derive(Clone, PartialEq, Eq)]
pub struct KitView {
    /// The kit file.
    pub path: PathBuf,
    /// The secret, as 8 groups of 4.
    pub secret: String,
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
            AccountAction::DismissKit => f.write_str("DismissKit"),
            AccountAction::ConfirmRecovery => f.write_str("ConfirmRecovery"),
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
