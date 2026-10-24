//! The account backend that talks to a sync server: it runs the flows of
//! `crate::sync` on its own thread, with its own async runtime, and keeps
//! the screens' state in step with them.
//!
//! It remembers the server address in the app's data folder and keeps the
//! device's secrets only in the credential store it gets.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sync_protocol::types::Id;

use super::{
    AccountAction, AccountBackend, AccountPhase, AccountScreen, AlertEntry, ApprovalView,
    DeviceEntry, KitView, RecoveryView, RemovalView, RequestEntry, SignOutView, SignedInView,
    fingerprint,
};
use crate::sync::approval::{Candidate, CandidateStep, OutstandingOffer, PendingDevice};
use crate::sync::device::{DeviceSignIn, Joined, SignedIn, StoredDevice, join, server_address};
use crate::sync::device_list::{AddedBy, DeviceAlert};
use crate::sync::recovery::{Recovery, open_kit};
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

struct PendingSignIn {
    grant: DeviceSignIn,
    next_poll: Instant,
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
        SyncError::Check("sync_approval_mismatch") => {
            "Another offer reached this device, so the approval stopped. Start it again from one of your devices."
                .into()
        }
        SyncError::Network(_) => "The sync server can't be reached. Try again in a moment.".into(),
        SyncError::Invalid(text) => text.clone(),
        other => format!("Sync stopped: {other}."),
    }
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
    /// server, if it joined its chain.
    async fn resume(&mut self) {
        if self.server.is_empty() {
            return;
        }
        let stored = match StoredDevice::load(&*self.store, &self.server) {
            Ok(Some(stored)) => stored,
            _ => return,
        };
        match stored.credentials() {
            Ok(Some(credentials)) => {
                if let Err(error) = self.connect(credentials).await {
                    self.fail(&error);
                }
            }
            _ => self.fail(&SyncError::Invalid(
                "This device signed in but wasn't approved. Sign in again.".into(),
            )),
        }
    }

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
        self.open_workspaces().await?;
        self.refresh().await
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
                let text = match alert {
                    DeviceAlert::UnknownCard { card, .. } => format!(
                        "A device that nobody approved signed in to your account: {}.",
                        fingerprint(&card.fingerprint())
                    ),
                    DeviceAlert::UnverifiedWorkspace { .. } => {
                        "Noura can't verify one of your workspaces on this server.".to_owned()
                    }
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
                    DeviceAlert::PolicyChange { kind, signer, .. } => {
                        format!("Device {} changed a workspace ({kind}).", short(signer))
                    }
                };
                let end_sessions = match alert {
                    DeviceAlert::UnknownCard { card, .. } => Some(card.device),
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
                let open = self.open_browser.clone();
                let grant = DeviceSignIn::start(&self.server, &move |url| open(url)).await?;
                let user_code = grant.user_code().to_owned();
                self.sign_in = Some(PendingSignIn {
                    next_poll: Instant::now() + Duration::from_secs(grant.interval()),
                    grant,
                });
                self.shared.update(|screen| {
                    screen.busy = false;
                    screen.phase = AccountPhase::SigningIn { user_code };
                });
            }
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
                service
                    .remove_device(removal.target, &keep, &mut self.workspaces)
                    .await?;
                self.signed_in(|view| {
                    view.removal = None;
                    view.offer_kit = true;
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
                let export = service.export_kit(&path, &[], &mut self.workspaces).await?;
                let view = KitView {
                    path: export.path.clone(),
                    secret: export.secret.to_string(),
                };
                self.signed_in(|signed_in| {
                    signed_in.kit = Some(view);
                    signed_in.offer_kit = false;
                });
                self.refresh().await?;
            }
            AccountAction::DismissKit => self.signed_in(|view| view.kit = None),
            AccountAction::Recover { path, secret } => {
                let typed = self
                    .shared
                    .screen
                    .lock()
                    .map(|s| s.server.clone())
                    .unwrap_or_default();
                let server = server_address(&typed).map_err(|e| SyncError::Invalid(e.into()))?;
                let kit = open_kit(&path, &secret)?;
                self.server = server;
                self.remember();
                self.shared.update(|screen| screen.busy = true);
                let open = self.open_browser.clone();
                let grant = DeviceSignIn::start(&self.server, &move |url| open(url)).await?;
                let user_code = grant.user_code().to_owned();
                self.shared.update(|screen| {
                    screen.busy = false;
                    screen.phase = AccountPhase::SigningIn { user_code };
                });
                let signed_in = grant.finish(&crate::sync::device::Sleep).await?;
                let state = self.app_data.join("sync").join("recovery");
                let recovery = Recovery::begin(&signed_in, kit, self.store.clone(), &state).await?;
                let review = recovery.review();
                let view =
                    RecoveryView {
                        changes: review
                            .links
                            .iter()
                            .map(|l| format!("Your account: {} by {}", l.kind, l.signer.to_hex()))
                            .chain(review.changes.iter().map(|c| {
                                format!("A workspace: {} by {}", c.kind, c.signer.to_hex())
                            }))
                            .collect(),
                        devices: review
                            .devices
                            .iter()
                            .map(|c| (c.device, fingerprint(&c.fingerprint())))
                            .collect(),
                        keep: BTreeSet::new(),
                    };
                self.recovery = Some(recovery);
                self.shared
                    .update(|screen| screen.phase = AccountPhase::Recovering(view));
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
                let (service, workspaces, _added) = recovery.finish(&keep, &*self.store).await?;
                self.service = Some(service);
                self.workspaces = workspaces;
                self.shared.update(|screen| {
                    screen.phase = AccountPhase::SignedIn(Box::new(SignedInView {
                        offer_kit: true,
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
                let service = self.service.take().ok_or_else(not_signed_in)?;
                self.shared.update(|screen| screen.busy = true);
                service.sign_out(&mut self.workspaces, &*self.store).await?;
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
                    self.sign_in = None;
                    self.joined(signed_in).await?;
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
        if let (Some(offer), Some(service)) = (self.offer.as_mut(), self.service.as_mut())
            && let Some(code) = service.approval_code(offer).await?
        {
            self.signed_in(|view| {
                if let Some(approval) = &mut view.approval {
                    approval.code = Some(code);
                }
            });
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

    /// After the browser sign-in: the first device of an account starts its
    /// chain, and a later one waits for approval.
    async fn joined(&mut self, signed_in: SignedIn) -> Result<(), SyncError> {
        match join(&signed_in, &*self.store).await? {
            Joined::First(credentials) => self.connect(credentials).await,
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
