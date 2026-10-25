//! The account backend that talks to a sync server: it runs the flows of
//! `crate::sync` on its own thread, with its own async runtime, and keeps
//! the screens' state in step with them.
//!
//! It remembers the server address in the app's data folder and keeps the
//! device's secrets only in the credential store it gets.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sync_protocol::records::KitPayload;
use sync_protocol::types::Id;
use zeroize::Zeroizing;

use super::{
    AccountAction, AccountBackend, AccountPhase, AccountScreen, AlertEntry, ApprovalView,
    DeviceEntry, KitReplacementView, KitView, RecoveryView, RemovalView, RequestEntry, SignOutView,
    SignedInView, fingerprint,
};
use crate::sync::approval::{Candidate, CandidateStep, OutstandingOffer, PendingDevice};
use crate::sync::device::{DeviceSignIn, Joined, SignedIn, StoredDevice, join, server_address};
use crate::sync::device_list::{AddedBy, DeviceAlert};
use crate::sync::recovery::{Recovery, RecoveryReview, open_kit};
use crate::sync::upkeep::Upkeep;
use crate::sync::{DeviceCredentials, KeptHeads, RemoteWorkspace, Secrets, SyncError, SyncService};

/// The file in the app's data folder that remembers the server address.
pub const ACCOUNT_FILE: &str = "sync-account.json";

/// How often the backend looks for offers, codes and new requests.
const TICK: Duration = Duration::from_secs(2);

#[derive(Default, Serialize, Deserialize)]
struct Remembered {
    server: String,
}

/// Opens a web page in the system browser.
pub type OpenBrowser = Arc<dyn Fn(&str) -> std::io::Result<()> + Send + Sync>;

struct Shared {
    screen: Mutex<AccountScreen>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut AccountScreen)) {
        change(&mut self.screen.lock().unwrap_or_else(PoisonError::into_inner));
        let wake = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

/// The account backend of the desktop app.
pub struct LiveAccount {
    shared: Arc<Shared>,
    actions: Mutex<Sender<AccountAction>>,
}

impl LiveAccount {
    /// Starts the backend, which reads the server address from `app_data`
    /// and keeps the device's secrets in `store`, and opens the device
    /// page with `open_browser`.
    pub fn start(app_data: PathBuf, store: Secrets, open_browser: OpenBrowser) -> Self {
        let remembered: Remembered = std::fs::read(app_data.join(ACCOUNT_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let shared = Arc::new(Shared {
            screen: Mutex::new(AccountScreen {
                server: remembered.server.clone(),
                ..AccountScreen::default()
            }),
            wake: Mutex::new(None),
        });
        let (sender, receiver) = channel();
        let driver = Driver {
            shared: shared.clone(),
            app_data,
            store,
            open_browser,
            server: remembered.server,
            sign_in: None,
            candidate: None,
            service: None,
            workspaces: Vec::new(),
            requests: Vec::new(),
            offer: None,
            recovery: None,
            last_refresh: None,
            upkeep: Upkeep::default(),
        };
        let spawned = std::thread::Builder::new()
            .name("noura-sync-account".into())
            .spawn(move || driver.run(receiver));
        if spawned.is_err() {
            shared.update(|screen| {
                screen.problem = Some("Sync couldn't start on this computer.".into());
            });
        }
        Self {
            shared,
            actions: Mutex::new(sender),
        }
    }
}

impl AccountBackend for LiveAccount {
    fn screen(&self) -> AccountScreen {
        self.shared
            .screen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn act(&self, action: AccountAction) {
        let _ = self
            .actions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(action);
    }

    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self
            .shared
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(wake);
    }
}

/// What a browser sign-in is for.
enum Purpose {
    /// Signing this computer in as a device of your account.
    Join,
    /// Recovering your account with this kit (§12).
    Recover(Box<KitPayload>),
}

struct PendingSignIn {
    grant: DeviceSignIn,
    next_poll: Instant,
    purpose: Purpose,
}

struct Waiting {
    candidate: Candidate,
    heads: KeptHeads,
}

struct Driver {
    shared: Arc<Shared>,
    app_data: PathBuf,
    store: Secrets,
    open_browser: OpenBrowser,
    server: String,
    sign_in: Option<PendingSignIn>,
    candidate: Option<Waiting>,
    service: Option<SyncService>,
    workspaces: Vec<RemoteWorkspace>,
    requests: Vec<PendingDevice>,
    offer: Option<OutstandingOffer>,
    recovery: Option<Recovery>,
    last_refresh: Option<Instant>,
    upkeep: Upkeep,
}

/// What a failed action tells you.
fn message(error: &SyncError) -> String {
    match error {
        SyncError::SignIn(end) => end.to_string(),
        SyncError::KitRevoked => error.to_string(),
        SyncError::Check("sync_recovery_kit_invalid") => {
            "The secret doesn't open this kit, or the kit file changed. Check both and try again."
                .into()
        }
        SyncError::Refused { code, .. } if code == "offer_expired" => {
            "The approval took longer than 10 minutes, so it stopped. Start it again from the new device."
                .into()
        }
        SyncError::Check("sync_approval_mismatch") => {
            "Another offer reached this device, so the approval stopped. Start it again from one of your devices."
                .into()
        }
        SyncError::Network(_) => "The sync server can't be reached. Try again in a moment.".into(),
        SyncError::Invalid(text) => text.clone(),
        other => format!("Sync stopped: {other}."),
    }
}

/// The first 8 hexadecimal characters of an ID, as the screens name a
/// workspace, an account or a device they show no fingerprint for.
fn short_id(id: &Id) -> String {
    id.to_hex().chars().take(8).collect()
}

/// What the device list says about `alert`, naming devices with `short`.
pub(crate) fn alert_text(alert: &DeviceAlert, short: &dyn Fn(&Id) -> String) -> String {
    match alert {
        DeviceAlert::UnknownCard { card, .. } => format!(
            "A device that nobody approved signed in to your account: {}.",
            fingerprint(&card.fingerprint())
        ),
        DeviceAlert::UnverifiedWorkspace { workspace, code } => format!(
            "Noura can't verify workspace {} on this server ({code}).",
            short_id(workspace)
        ),
        DeviceAlert::Link {
            kind,
            signer,
            card,
            recovery,
            ..
        } => format!(
            "Device {} changed your account ({kind}) for {} {}.",
            short(signer),
            if *recovery { "recovery kit" } else { "device" },
            fingerprint(&card.fingerprint())
        ),
        DeviceAlert::PolicyChange {
            workspace,
            revision,
            kind,
            signer,
        } => format!(
            "Device {} changed workspace {} at revision {revision} ({kind}).",
            short(signer),
            short_id(workspace)
        ),
    }
}

/// What a recovery shows you before it goes on (§12, step 3): the changes
/// since the kit with their signers, the members of each workspace the kit
/// doesn't list, the workspaces it couldn't verify, and your other devices.
pub(crate) fn recovery_view(account: Id, review: &RecoveryReview) -> RecoveryView {
    let member = |id: &Id| {
        if *id == account {
            "your account".to_owned()
        } else {
            format!("account {}", short_id(id))
        }
    };
    RecoveryView {
        changes: review
            .links
            .iter()
            .map(|l| {
                format!(
                    "Your account: {} by device {}.",
                    l.kind,
                    short_id(&l.signer)
                )
            })
            .chain(review.changes.iter().map(|c| {
                format!(
                    "Workspace {} at revision {}: {} by device {}.",
                    short_id(&c.workspace),
                    c.revision,
                    c.kind,
                    short_id(&c.signer)
                )
            }))
            .collect(),
        members: review
            .members
            .iter()
            .map(|(workspace, members)| {
                format!(
                    "Workspace {} has these members: {}.",
                    short_id(workspace),
                    members.iter().map(member).collect::<Vec<_>>().join(", ")
                )
            })
            .collect(),
        unverified: review
            .unverified
            .iter()
            .map(|(workspace, code)| {
                format!(
                    "Noura couldn't verify workspace {} ({code}), so the recovery leaves it out.",
                    short_id(workspace)
                )
            })
            .collect(),
        devices: review
            .devices
            .iter()
            .map(|c| (c.device, fingerprint(&c.fingerprint())))
            .collect(),
        keep: BTreeSet::new(),
    }
}

/// The members a recovery found added after the kit's heads, one line each
/// (§12).
pub(crate) fn added_member_lines(added: &[(Id, Id)]) -> Vec<String> {
    added
        .iter()
        .map(|(workspace, member)| {
            format!(
                "Account {} joined workspace {} after you exported the kit.",
                short_id(member),
                short_id(workspace)
            )
        })
        .collect()
}

impl Driver {
    fn run(mut self, actions: Receiver<AccountAction>) {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            self.shared.update(|screen| {
                screen.problem = Some("Sync couldn't start on this computer.".into());
            });
            return;
        };
        runtime.block_on(self.resume());
        loop {
            match actions.recv_timeout(TICK) {
                Ok(action) => runtime.block_on(self.handle(action)),
                Err(RecvTimeoutError::Timeout) => runtime.block_on(self.tick()),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn fail(&self, error: &SyncError) {
        let text = message(error);
        tracing::warn!(kind = ?error.code(), "a sync account action failed");
        self.shared.update(|screen| {
            screen.busy = false;
            screen.problem = Some(text);
        });
    }

    fn remember(&self) {
        let bytes = serde_json::to_vec(&Remembered {
            server: self.server.clone(),
        })
        .unwrap_or_default();
        let _ = std::fs::create_dir_all(&self.app_data);
        let _ = std::fs::write(self.app_data.join(ACCOUNT_FILE), bytes);
    }

    fn state_dir(&self, device: &Id) -> PathBuf {
        self.app_data.join("sync").join(device.to_hex())
    }

    /// Connects the device the credential store holds for the remembered
    /// server, if it joined its chain. When it can't, the screen offers to
    /// try again, and signing in again binds a new session to the same
    /// card (§7).
    async fn resume(&mut self) {
        self.shared.update(|screen| screen.can_retry = false);
        if self.server.is_empty() {
            return;
        }
        let stored = match StoredDevice::load(&*self.store, &self.server) {
            Ok(Some(stored)) => stored,
            Ok(None) => return,
            Err(error) => return self.fail(&error),
        };
        match stored.credentials() {
            Ok(Some(credentials)) => {
                if let Err(error) = self.connect(credentials).await {
                    let text = match &error {
                        SyncError::Refused { code, .. } if code == "unauthorized" => {
                            "This computer's session with the sync server ended. Sign in again with your browser, and this device keeps its keys.".to_owned()
                        }
                        other => message(other),
                    };
                    tracing::warn!(kind = ?error.code(), "the sync account didn't connect");
                    self.shared.update(|screen| {
                        screen.busy = false;
                        screen.problem = Some(text);
                        screen.can_retry = true;
                    });
                }
            }
            _ => self.fail(&SyncError::Invalid(
                "This device signed in but wasn't approved. Sign in again.".into(),
            )),
        }
    }

    /// Connects the sync service, opens the workspaces and shows the
    /// account; nothing stays connected when a step fails.
    async fn connect(&mut self, credentials: DeviceCredentials) -> Result<(), SyncError> {
        let state = self.state_dir(&credentials.keys.device());
        let heads = KeptHeads::open(
            &state,
            credentials.account,
            credentials.keys.device(),
            &*self.store,
        )?;
        let service =
            SyncService::connect(&self.server, credentials, heads, self.store.clone()).await?;
        self.service = Some(service);
        let connected = match self.open_workspaces().await {
            Ok(()) => self.refresh().await,
            Err(error) => Err(error),
        };
        if connected.is_err() {
            self.service = None;
            self.workspaces.clear();
        }
        connected
    }

    async fn open_workspaces(&mut self) -> Result<(), SyncError> {
        let Some(service) = self.service.as_mut() else {
            return Ok(());
        };
        let listed = service.client().workspaces().await?;
        let mut workspaces = Vec::new();
        for workspace in listed.workspaces.iter() {
            if let Ok(remote) = service.open_workspace(workspace.workspace).await {
                workspaces.push(remote);
            }
        }
        self.workspaces = workspaces;
        Ok(())
    }

    /// Reads the device list and the requests again and shows them.
    async fn refresh(&mut self) -> Result<(), SyncError> {
        self.last_refresh = Some(Instant::now());
        let Some(service) = self.service.as_mut() else {
            return Ok(());
        };
        let list = match service.device_list(&self.workspaces).await {
            Ok(list) => list,
            Err(SyncError::Refused { code, detail, .. }) if code == "device_removed" => {
                let by = match detail {
                    Some(detail) => service
                        .verify_removal(&detail)
                        .await
                        .map(|by| fingerprint(&by.signer_fingerprint))
                        .unwrap_or_default(),
                    None => String::new(),
                };
                self.service = None;
                self.shared.update(|screen| {
                    screen.busy = false;
                    screen.phase = AccountPhase::Removed { by };
                });
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        self.requests = service.pending_devices().await.unwrap_or_default();
        let this = service.credentials().keys.device();
        let fingerprints: std::collections::BTreeMap<Id, String> = list
            .active
            .iter()
            .chain(list.removed.iter())
            .map(|row| (row.card.device, fingerprint(&row.fingerprint)))
            .collect();
        let short = |id: &Id| {
            fingerprints
                .get(id)
                .map(|f| f.chars().take(9).collect::<String>())
                .unwrap_or_else(|| id.to_hex().chars().take(8).collect())
        };
        let mut devices = Vec::new();
        for row in list.active.iter().chain(list.removed.iter()) {
            let how = match row.added {
                AddedBy::Genesis => "Started the account".to_owned(),
                AddedBy::Approval(by) => format!("Approved by {}", short(&by)),
                AddedBy::Kit(by) => format!("Recovery kit exported by {}", short(&by)),
                AddedBy::Recovery(_) => "Recovered with a kit".to_owned(),
            };
            let how = match row.removed {
                Some((_, by)) => format!("{how}. Removed by {}", short(&by)),
                None => how,
            };
            devices.push(DeviceEntry {
                device: row.card.device,
                fingerprint: fingerprint(&row.fingerprint),
                how,
                this_device: row.card.device == this,
                recovery: row.recovery,
                removed: row.removed.is_some(),
            });
        }
        let alerts = list
            .alerts
            .iter()
            .map(|alert| {
                let text = alert_text(alert, &short);
                // A card with no session left holds nothing more to end.
                let end_sessions = match alert {
                    DeviceAlert::UnknownCard { card, sessions } if *sessions > 0 => {
                        Some(card.device)
                    }
                    _ => None,
                };
                AlertEntry { text, end_sessions }
            })
            .collect();
        let account_fingerprint = fingerprint(&list.account_fingerprint);
        let requests = self
            .requests
            .iter()
            .map(|request| RequestEntry {
                fingerprint: fingerprint(&request.fingerprint),
            })
            .collect();
        self.shared.update(|screen| {
            screen.busy = false;
            let mut view = match std::mem::take(&mut screen.phase) {
                AccountPhase::SignedIn(view) => view,
                _ => Box::default(),
            };
            view.account_fingerprint = account_fingerprint;
            view.devices = devices;
            view.alerts = alerts;
            view.requests = requests;
            screen.phase = AccountPhase::SignedIn(view);
        });
        Ok(())
    }

    fn signed_in(&self, change: impl FnOnce(&mut SignedInView)) {
        self.shared.update(|screen| {
            if let AccountPhase::SignedIn(view) = &mut screen.phase {
                change(view);
            }
        });
    }

    async fn handle(&mut self, action: AccountAction) {
        self.shared.update(|screen| screen.problem = None);
        if let Err(error) = self.carry_out(action).await {
            self.fail(&error);
        }
    }

    async fn carry_out(&mut self, action: AccountAction) -> Result<(), SyncError> {
        match action {
            AccountAction::SetServer(text) => {
                let problem = server_address(&text).err().map(str::to_owned);
                self.shared.update(|screen| {
                    screen.server = text;
                    screen.server_problem = problem;
                });
            }
            AccountAction::SignIn => {
                let typed = self
                    .shared
                    .screen
                    .lock()
                    .map(|s| s.server.clone())
                    .unwrap_or_default();
                let server = server_address(&typed).map_err(|e| SyncError::Invalid(e.into()))?;
                self.server = server;
                self.remember();
                self.shared.update(|screen| screen.busy = true);
                self.start_sign_in(Purpose::Join).await?;
            }
            AccountAction::Retry => self.resume().await,
            AccountAction::CancelSignIn => {
                self.sign_in = None;
                self.candidate = None;
                self.shared
                    .update(|screen| screen.phase = AccountPhase::SignedOut);
            }
            AccountAction::ConfirmCode => {
                if let Some(waiting) = self.candidate.as_mut() {
                    waiting.candidate.confirm();
                    self.shared.update(|screen| {
                        if let AccountPhase::WaitingForApproval { confirmed, .. } =
                            &mut screen.phase
                        {
                            *confirmed = true;
                        }
                    });
                }
            }
            AccountAction::Approve(index) => {
                let request = self
                    .requests
                    .get(index)
                    .cloned()
                    .ok_or_else(|| SyncError::Invalid("That request ended.".into()))?;
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                let offer = service
                    .offer_approval(&request, &mut self.workspaces)
                    .await?;
                let view = ApprovalView {
                    fingerprint: fingerprint(&offer.fingerprint()),
                    code: None,
                };
                self.offer = Some(offer);
                self.signed_in(|signed_in| signed_in.approval = Some(view));
            }
            AccountAction::ConfirmApproval => {
                let offer = self
                    .offer
                    .take()
                    .ok_or_else(|| SyncError::Invalid("No approval is open.".into()))?;
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                service
                    .confirm_approval(offer, &mut self.workspaces)
                    .await?;
                self.signed_in(|view| view.approval = None);
                self.refresh().await?;
            }
            AccountAction::CancelApproval => {
                if let (Some(offer), Some(service)) = (self.offer.take(), self.service.as_mut()) {
                    service.cancel_approval(offer);
                }
                self.candidate = None;
                self.signed_in(|view| view.approval = None);
            }
            AccountAction::Remove(device) => {
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                let preview = service.removal_preview(device).await?;
                let keepable: BTreeSet<Id> = preview.keepable.iter().copied().collect();
                let view = RemovalView {
                    target: device,
                    fingerprint: fingerprint(&preview.target.fingerprint()),
                    with_it: preview
                        .with_it
                        .iter()
                        .map(|card| {
                            (
                                card.device,
                                fingerprint(&card.fingerprint()),
                                keepable.contains(&card.device),
                            )
                        })
                        .collect(),
                    keep: BTreeSet::new(),
                };
                self.signed_in(|signed_in| signed_in.removal = Some(view));
            }
            AccountAction::ToggleKeep(device) => {
                self.shared.update(|screen| match &mut screen.phase {
                    AccountPhase::SignedIn(view) => {
                        if let Some(removal) = &mut view.removal
                            && removal
                                .with_it
                                .iter()
                                .any(|(id, _, keepable)| *id == device && *keepable)
                            && !removal.keep.remove(&device)
                        {
                            removal.keep.insert(device);
                        }
                        if let Some(replacement) = &mut view.kit_replacement
                            && replacement.devices.iter().any(|(id, _)| *id == device)
                            && !replacement.keep.remove(&device)
                        {
                            replacement.keep.insert(device);
                        }
                    }
                    AccountPhase::Recovering(view) => {
                        if !view.keep.remove(&device) {
                            view.keep.insert(device);
                        }
                    }
                    _ => {}
                });
            }
            AccountAction::ConfirmRemove => {
                let removal = match &self
                    .shared
                    .screen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .phase
                {
                    AccountPhase::SignedIn(view) => view.removal.clone(),
                    _ => None,
                }
                .ok_or_else(|| SyncError::Invalid("No removal is open.".into()))?;
                self.shared.update(|screen| screen.busy = true);
                let keep: Vec<Id> = removal.keep.iter().copied().collect();
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                let done = service
                    .remove_device(removal.target, &keep, &mut self.workspaces)
                    .await?;
                let mut names: BTreeMap<Id, String> = removal
                    .with_it
                    .iter()
                    .map(|(device, shown, _)| (*device, shown.clone()))
                    .collect();
                names.insert(removal.target, removal.fingerprint.clone());
                let changes = done
                    .changes
                    .iter()
                    .map(|change| {
                        let by: String = match names.get(&change.signer) {
                            Some(shown) => shown.chars().take(9).collect(),
                            None => change.signer.to_hex().chars().take(8).collect(),
                        };
                        let workspace: String = change.workspace.to_hex().chars().take(8).collect();
                        let undo = match change.undo {
                            Some(undo) => format!("Your role can undo it with {undo}."),
                            None => "Your role can't undo it.".to_owned(),
                        };
                        format!(
                            "Device {by} made a change ({}) in workspace {workspace}, and it stays. {undo}",
                            change.kind
                        )
                    })
                    .collect();
                self.signed_in(|view| {
                    view.removal = None;
                    view.offer_kit = true;
                    view.removed_changes = changes;
                });
                self.refresh().await?;
            }
            AccountAction::CancelRemove => self.signed_in(|view| view.removal = None),
            AccountAction::EndSessions(device) => {
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                service.end_unknown_sessions(device).await?;
                self.refresh().await?;
            }
            AccountAction::ExportKit(path) => {
                self.shared.update(|screen| screen.busy = true);
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                // Replacing a kit takes down the devices it recovered unless
                // you keep them, so the app lists them first (§12).
                let recovered = service.kit_replacement().await?;
                if recovered.is_empty() {
                    self.export_kit(path, &[]).await?;
                } else {
                    let view = KitReplacementView {
                        path,
                        devices: recovered
                            .iter()
                            .map(|card| (card.device, fingerprint(&card.fingerprint())))
                            .collect(),
                        keep: BTreeSet::new(),
                    };
                    self.shared.update(|screen| {
                        screen.busy = false;
                        if let AccountPhase::SignedIn(signed_in) = &mut screen.phase {
                            signed_in.kit_replacement = Some(view);
                        }
                    });
                }
            }
            AccountAction::ConfirmExportKit => {
                let replacement = match &self
                    .shared
                    .screen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .phase
                {
                    AccountPhase::SignedIn(view) => view.kit_replacement.clone(),
                    _ => None,
                }
                .ok_or_else(|| SyncError::Invalid("No kit replacement is open.".into()))?;
                self.shared.update(|screen| screen.busy = true);
                let keep: Vec<Id> = replacement.keep.iter().copied().collect();
                self.export_kit(replacement.path, &keep).await?;
            }
            AccountAction::CancelExportKit => self.signed_in(|view| view.kit_replacement = None),
            AccountAction::DismissKit => self.signed_in(|view| view.kit = None),
            AccountAction::Recover { path, secret } => {
                let typed = self
                    .shared
                    .screen
                    .lock()
                    .map(|s| s.server.clone())
                    .unwrap_or_default();
                let server = server_address(&typed).map_err(|e| SyncError::Invalid(e.into()))?;
                let secret = Zeroizing::new(secret);
                let kit = open_kit(&path, &secret)?;
                self.server = server;
                self.remember();
                self.shared.update(|screen| screen.busy = true);
                self.start_sign_in(Purpose::Recover(Box::new(kit))).await?;
            }
            AccountAction::ConfirmRecovery => {
                let recovery = self
                    .recovery
                    .take()
                    .ok_or_else(|| SyncError::Invalid("No recovery is open.".into()))?;
                let keep: Vec<Id> = match &self
                    .shared
                    .screen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .phase
                {
                    AccountPhase::Recovering(view) => view.keep.iter().copied().collect(),
                    _ => Vec::new(),
                };
                self.shared.update(|screen| screen.busy = true);
                let (service, workspaces, added) = recovery.finish(&keep, &*self.store).await?;
                self.service = Some(service);
                self.workspaces = workspaces;
                let added_members = added_member_lines(&added);
                self.shared.update(|screen| {
                    screen.phase = AccountPhase::SignedIn(Box::new(SignedInView {
                        offer_kit: true,
                        added_members,
                        ..SignedInView::default()
                    }));
                });
                self.refresh().await?;
            }
            AccountAction::SignOut => {
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                let last_device = service.is_last_device().await?;
                self.signed_in(|view| view.sign_out = Some(SignOutView { last_device }));
            }
            AccountAction::ConfirmSignOut => {
                let service = self.service.as_mut().ok_or_else(not_signed_in)?;
                self.shared.update(|screen| screen.busy = true);
                // The service stays when the sign-out fails, so you can try
                // again.
                service.sign_out(&mut self.workspaces, &*self.store).await?;
                self.service = None;
                self.workspaces.clear();
                self.shared.update(|screen| {
                    screen.busy = false;
                    screen.phase = AccountPhase::SignedOut;
                });
            }
            AccountAction::CancelSignOut => self.signed_in(|view| view.sign_out = None),
        }
        Ok(())
    }

    /// Polls what waits: the device grant, the new device's approval, the
    /// approving device's code, and every few seconds the device list.
    async fn tick(&mut self) {
        if let Err(error) = self.poll().await {
            self.fail(&error);
        }
    }

    async fn poll(&mut self) -> Result<(), SyncError> {
        if let Some(pending) = self.sign_in.as_mut()
            && Instant::now() >= pending.next_poll
        {
            match pending.grant.poll_once().await {
                Ok(Some(signed_in)) => {
                    let purpose = self.sign_in.take().map(|p| p.purpose);
                    match purpose {
                        Some(Purpose::Recover(kit)) => self.begin_recovery(signed_in, *kit).await?,
                        _ => self.joined(signed_in).await?,
                    }
                }
                Ok(None) => {
                    pending.next_poll =
                        Instant::now() + Duration::from_secs(pending.grant.interval());
                }
                Err(error) => {
                    self.sign_in = None;
                    self.shared
                        .update(|screen| screen.phase = AccountPhase::SignedOut);
                    return Err(error);
                }
            }
        }
        if let Some(waiting) = self.candidate.as_mut() {
            match waiting.candidate.poll().await {
                Ok(CandidateStep::Code(code)) => self.shared.update(|screen| {
                    if let AccountPhase::WaitingForApproval { code: shown, .. } = &mut screen.phase
                    {
                        *shown = Some(code);
                    }
                }),
                Ok(CandidateStep::Waiting) => {}
                Err(error) => {
                    self.candidate = None;
                    self.shared
                        .update(|screen| screen.phase = AccountPhase::SignedOut);
                    return Err(error);
                }
            }
            let store = self.store.clone();
            if let Some(waiting) = self.candidate.as_mut()
                && let Some(credentials) = waiting
                    .candidate
                    .joined(&mut waiting.heads, &*store)
                    .await?
            {
                self.candidate = None;
                self.connect(credentials).await?;
            }
        }
        if let (Some(offer), Some(service)) = (self.offer.as_mut(), self.service.as_mut()) {
            match service.approval_code(offer).await {
                Ok(Some(code)) => self.signed_in(|view| {
                    if let Some(approval) = &mut view.approval {
                        approval.code = Some(code);
                    }
                }),
                Ok(None) => {}
                // A server it can't reach leaves the offer for the next
                // poll.
                Err(error @ SyncError::Network(_)) => return Err(error),
                // An expired offer, or a nonce that doesn't open the
                // commitment, ends the attempt on this device too.
                Err(error) => {
                    self.offer = None;
                    self.signed_in(|view| view.approval = None);
                    return Err(error);
                }
            }
        }
        let stale = self
            .last_refresh
            .is_none_or(|at| at.elapsed() > Duration::from_secs(10));
        if self.service.is_some() && self.offer.is_none() && stale {
            self.refresh().await?;
            self.keep_up().await;
        }
        Ok(())
    }

    /// The key upkeep that waits its turn (§8, §9): later heads of member
    /// accounts, pending rotations and re-wrap requests in each workspace,
    /// each carried out once its random wait passes. A failure waits for
    /// the next round and goes to the diagnostic log.
    async fn keep_up(&mut self) {
        let Some(service) = self.service.as_mut() else {
            return;
        };
        let mut waiting = Vec::new();
        for remote in self.workspaces.iter_mut() {
            match service.upkeep_waiting(remote).await {
                Ok(tasks) => waiting.extend(tasks),
                Err(error) => {
                    tracing::warn!(kind = ?error.code(), "key upkeep couldn't read a workspace");
                }
            }
        }
        for task in self.upkeep.plan(waiting, Instant::now()) {
            let Some(remote) = self
                .workspaces
                .iter_mut()
                .find(|remote| remote.workspace == task.workspace())
            else {
                continue;
            };
            if let Err(error) = service.run_upkeep(remote, &task).await {
                tracing::warn!(kind = ?error.code(), "key upkeep failed");
            }
        }
    }

    /// Asks the server for a device grant, opens the device page and waits
    /// for you there; [`Driver::poll`] polls it.
    async fn start_sign_in(&mut self, purpose: Purpose) -> Result<(), SyncError> {
        let open = self.open_browser.clone();
        let grant = DeviceSignIn::start(&self.server, &move |url| open(url)).await?;
        let user_code = grant.user_code().to_owned();
        self.sign_in = Some(PendingSignIn {
            next_poll: Instant::now() + Duration::from_secs(grant.interval()),
            grant,
            purpose,
        });
        self.shared.update(|screen| {
            screen.busy = false;
            screen.can_retry = false;
            screen.phase = AccountPhase::SigningIn { user_code };
        });
        Ok(())
    }

    /// Exports a kit to `path`, keeping `keep` among the devices the old kit
    /// recovered, and shows its secret once.
    async fn export_kit(&mut self, path: PathBuf, keep: &[Id]) -> Result<(), SyncError> {
        let service = self.service.as_mut().ok_or_else(not_signed_in)?;
        let export = service
            .export_kit(&path, keep, &mut self.workspaces)
            .await?;
        let view = KitView {
            path: export.path.clone(),
            secret: export.secret.clone(),
        };
        self.signed_in(|signed_in| {
            signed_in.kit = Some(view);
            signed_in.kit_replacement = None;
            signed_in.offer_kit = false;
        });
        self.refresh().await
    }

    /// Starts recovering with `kit` once the browser sign-in returned a
    /// session (§12, steps 1 to 3), and shows what changed since the kit.
    async fn begin_recovery(
        &mut self,
        signed_in: SignedIn,
        kit: KitPayload,
    ) -> Result<(), SyncError> {
        self.shared.update(|screen| screen.busy = true);
        let state = self.app_data.join("sync").join("recovery");
        let recovery = match Recovery::begin(&signed_in, kit, self.store.clone(), &state).await {
            Ok(recovery) => recovery,
            Err(error) => {
                self.shared
                    .update(|screen| screen.phase = AccountPhase::SignedOut);
                return Err(error);
            }
        };
        let view = recovery_view(signed_in.account, recovery.review());
        self.recovery = Some(recovery);
        self.shared.update(|screen| {
            screen.busy = false;
            screen.phase = AccountPhase::Recovering(view);
        });
        Ok(())
    }

    /// After the browser sign-in: the first device of an account starts its
    /// chain, a device whose session ended takes the new one, and a later
    /// one waits for approval.
    async fn joined(&mut self, signed_in: SignedIn) -> Result<(), SyncError> {
        match join(&signed_in, &*self.store).await? {
            Joined::First(credentials) | Joined::Again(credentials) => {
                self.connect(credentials).await
            }
            Joined::NeedsApproval { keys, client } => {
                let device = keys.device();
                let state = self.state_dir(&device);
                let heads = KeptHeads::open(&state, signed_in.account, device, &*self.store)?;
                let candidate = Candidate::request(client, keys, signed_in.account).await?;
                let shown = fingerprint(&candidate.fingerprint());
                self.candidate = Some(Waiting { candidate, heads });
                self.shared.update(|screen| {
                    screen.phase = AccountPhase::WaitingForApproval {
                        fingerprint: shown,
                        code: None,
                        confirmed: false,
                    };
                });
                Ok(())
            }
        }
    }
}

fn not_signed_in() -> SyncError {
    SyncError::Invalid("Sign in first.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::recovery::{ReviewChange, ReviewLink};
    use sync_key_envelope::v2::DeviceKeys;

    fn id(byte: u8) -> Id {
        Id([byte; 16])
    }

    #[test]
    fn a_policy_change_alert_names_the_device_the_workspace_and_the_change() {
        let short = |device: &Id| format!("fp-{}", device.0[0]);
        let text = alert_text(
            &DeviceAlert::PolicyChange {
                workspace: id(0xab),
                revision: 7,
                kind: "set_role",
                signer: id(2),
            },
            &short,
        );
        assert_eq!(
            text,
            "Device fp-2 changed workspace abababab at revision 7 (set_role)."
        );
        let text = alert_text(
            &DeviceAlert::UnverifiedWorkspace {
                workspace: id(0xcd),
                code: "sync_policy_forked",
            },
            &short,
        );
        assert_eq!(
            text,
            "Noura can't verify workspace cdcdcdcd on this server (sync_policy_forked)."
        );
        let card = DeviceKeys::generate().unwrap().card(id(1));
        let text = alert_text(
            &DeviceAlert::Link {
                seqno: 3,
                kind: "add_device",
                signer: id(4),
                card: card.clone(),
                recovery: false,
            },
            &short,
        );
        assert_eq!(
            text,
            format!(
                "Device fp-4 changed your account (add_device) for device {}.",
                fingerprint(&card.fingerprint())
            )
        );
    }

    #[test]
    fn the_recovery_review_shows_members_unverified_workspaces_and_added_members() {
        let account = id(1);
        let other = DeviceKeys::generate().unwrap().card(account);
        let review = RecoveryReview {
            links: vec![ReviewLink {
                seqno: 3,
                kind: "add_device",
                signer: id(0x22),
            }],
            changes: vec![ReviewChange {
                workspace: id(0x33),
                revision: 2,
                kind: "add_member",
                signer: id(0x44),
            }],
            members: vec![(id(0x33), vec![account, id(0x55)])],
            devices: vec![other.clone()],
            unverified: vec![(id(0x66), "sync_workspace_unverified")],
        };
        let view = recovery_view(account, &review);
        assert_eq!(
            view.changes,
            vec![
                "Your account: add_device by device 22222222.".to_owned(),
                "Workspace 33333333 at revision 2: add_member by device 44444444.".to_owned(),
            ]
        );
        assert_eq!(
            view.members,
            vec![
                "Workspace 33333333 has these members: your account, account 55555555.".to_owned()
            ]
        );
        assert_eq!(
            view.unverified,
            vec!["Noura couldn't verify workspace 66666666 (sync_workspace_unverified), so the recovery leaves it out.".to_owned()]
        );
        assert_eq!(
            view.devices,
            vec![(other.device, fingerprint(&other.fingerprint()))]
        );
        assert_eq!(
            added_member_lines(&[(id(0x33), id(0x77))]),
            vec![
                "Account 77777777 joined workspace 33333333 after you exported the kit.".to_owned()
            ]
        );
    }
}
