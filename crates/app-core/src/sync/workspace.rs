//! The desktop sync client of one folder (docs/workspace-format/sync-v2.md
//! §13 to §16, §25): it keeps a workspace's files in step with your other
//! desktops over sync v2.
//!
//! Turning sync on creates the remote workspace in one request and uploads
//! the folder. Every change to the workspace's files, from Noura or from
//! another program, becomes a padded, encrypted operation signed into this
//! device's chain, persisted in the journal's outbox before anything is
//! sent, and pushed one push at a time. One pull per round fetches the
//! domain, checks each record before it decrypts it, applies the changes
//! through `local-core`'s checked writes and only then advances the
//! cursor. Conflicts keep both versions.
//!
//! [`WorkspaceSync`] holds the folder's state and needs no network to
//! capture changes; the calls that reach the server take the device's
//! [`SyncService`].

mod access;
mod approve;
mod bootstrap;
mod capture;
mod domains;
mod moves;
pub mod presence;
pub mod rebuild;
pub mod restore;
mod settle;
pub mod state;
pub mod text;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use local_core::WorkspaceEngine;
use sync_key_envelope::v2::unwrap;
use sync_protocol::chain::{ChainError, ChainFetcher, ChainRecord, ChainVerdict, Fetched};
use sync_protocol::codec::{Bytes, Decode, Encode, Sorted, Text};
use sync_protocol::crypto::{hash, random_nonce};
use sync_protocol::keys::{
    SkippedEpochRecord, judge_skipped_epoch_record, key_matches, walk_lineage,
};
use sync_protocol::live::LiveOp;
use sync_protocol::matching::{domain_of, synced_path};
use sync_protocol::messages::{
    ChainConflictDetail, Entry, EntryRecord, Frame, FrameAction, MAX_BODY, Page, PageStatus,
    PullCursor, PushFailureDetail, Receipt, RecordRef, records_frame_action,
    records_frame_is_well_formed,
};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{
    Content, Cover, DeleteReason, OpClass, Operation, Payload, PayloadKind, PolicyKind, Profile,
    ProfilePayload, Role, Signed, SyncPath,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use capture::{Change, Found};
pub use capture::{SkipWhy, Skipped};
pub use moves::{HeldFile, MOVE_WAIT, MoveReview};
use settle::{Bodies, Body};
pub use state::Ended;
use state::{
    Content as Carried, Held, Log, Object, OpKey, Outgoing, Own, ProfileMark, State, StateInfo,
    Wanted,
};

use super::journal::{self, Journal, STATE_FILE};
use super::marks::Mark;
use super::registry::Registry;
use super::{DeviceCredentials, RemoteWorkspace, Secrets, SyncError, SyncService, random};

/// The longest a pull waits for new records (§15).
pub const PULL_WAIT_SECONDS: u32 = 25;
/// The re-seals per chain position that no newer epoch or policy change
/// forces (§14, §27).
pub const RESEAL_LIMIT: u32 = 3;
/// The longest random wait before a device writes again other devices'
/// records under an epoch a replacement skipped (§9).
pub const REWRITE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The longest random wait before a writer signs a checkpoint that fell
/// due (§17).
pub const CHECKPOINT_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
/// How far below 1 MiB a push stays, for its framing.
const PUSH_MARGIN: usize = 4_096;

/// What one round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Round {
    /// The local changes it signed into the outbox.
    pub captured: usize,
    /// The records the server acknowledged.
    pub pushed: usize,
    /// The other devices' records it applied.
    pub applied: usize,
    /// Whether the pull waits for a key before it goes on (§13).
    pub waiting_for_key: bool,
}

/// One branch of a conflict, as the review lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// The device that wrote it.
    pub device: Id,
    /// Its path, or `None` for a delete.
    pub path: Option<String>,
    /// Whether the local file holds it.
    pub on_disk: bool,
    /// Its position, which choosing names.
    pub key: OpKey,
}

/// An object whose live set holds two or more different states (§16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The object.
    pub rid: Id,
    /// Its branches, lowest operation digest first.
    pub branches: Vec<Branch>,
}

/// What sync shows of a folder (§30, Settings).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The workspace's name.
    pub name: String,
    /// Records signed and not yet acknowledged.
    pub pending: usize,
    /// Objects in conflict.
    pub conflicts: usize,
    /// Changes the server refused for lack of space, which stay on this
    /// device until space frees up (§20).
    pub waiting_for_space: usize,
    /// Files sync leaves out.
    pub skipped: Vec<Skipped>,
    /// Unix seconds of the last round without an error.
    pub last_success: Option<u64>,
    /// Records that didn't open, attributed to their signers.
    pub unreadable: usize,
    /// Local versions held for review.
    pub held: usize,
    /// Folders whose files wait until you confirm where they go.
    pub held_roots: usize,
    /// Versions an earlier journal's review store kept.
    pub archived: usize,
    /// The change that took this device's access, after which it syncs
    /// nothing more and the files stay as they are (§15).
    pub ended: Option<Ended>,
    /// Objects the device refuses to write.
    pub refused: usize,
    /// Files deleted on another device while this one was away, which wait
    /// for you (§18).
    pub deleted_away: usize,
    /// The workspace's usage against its quota, once the server reported
    /// it (§20).
    pub usage: Option<Usage>,
    /// What a restore lost that stopped this workspace (§14).
    pub restore_lost: Option<restore::Lost>,
    /// A rollback that holds this workspace until the generation changes.
    pub rollback: Option<String>,
    /// The generation of a restore the app hasn't shown you yet.
    pub restored: Option<u64>,
    /// The paths this device wrote again after a restore.
    pub written_again: Vec<String>,
}

/// A workspace's storage, as `GET /v2/workspaces` reports it (§20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    /// The bytes that count toward the quota.
    pub used: u64,
    /// The bytes of history, which yield to the quota.
    pub history: u64,
    /// The quota, absent when unlimited.
    pub quota: Option<u64>,
}

/// How long the device keeps the usage it last read.
const USAGE_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// The sync state of one folder.
pub struct WorkspaceSync {
    engine: Arc<WorkspaceEngine>,
    journal: Journal,
    state: State,
    mark: Mark,
    credentials: DeviceCredentials,
    secrets: Secrets,
    /// The epoch keys the device holds, by epoch.
    keys: BTreeMap<u64, SecretKey>,
    /// The verified policy and keys, once the device reached the server.
    remote: Option<RemoteWorkspace>,
    skipped: Vec<Skipped>,
    /// Why the device stopped writing the domain, such as
    /// `sync_reseal_limit` (§14).
    stopped: Option<&'static str>,
    /// Whether you paused sync.
    paused: bool,
    /// Whether a pull only records states, for the matching that follows
    /// a sync without the journal (§15).
    matching: bool,
    /// The files those pulls carried, by revision, until the matching.
    cache: Bodies,
    /// The latest policy entry before an epoch entry the device hasn't
    /// accepted yet, from the page that carried it (§9, Envelopes).
    entry_hint: Option<u64>,
    /// The log entries the device required before it sent the current
    /// pull, which that pull's page owes.
    armed: BTreeSet<(bool, u64)>,
    /// The longest random wait before the device writes again other
    /// devices' records under a skipped epoch (§9).
    rewrite_wait: std::time::Duration,
    /// The longest random wait before the device signs a checkpoint that
    /// fell due (§17).
    checkpoint_wait: std::time::Duration,
    /// The restore generation of the latest `restored` frame, which every
    /// other frame is judged against (§23).
    frame_generation: Option<u64>,
    /// The restricted domain the device works in, whose part of the
    /// journal the state's fields hold for now.
    active: Option<Id>,
    /// What the device holds in memory of each restricted domain, or of
    /// the workspace domain under the active domain's key.
    parked: BTreeMap<Id, domains::Runtime>,
    /// Whether a hold ended, so the files beneath it wait to be matched.
    rematch: bool,
    /// The moves that wait for you, from the latest capture.
    reviews: Vec<MoveReview>,
    /// The workspace's usage, and when the device last read it.
    usage: Option<Usage>,
    usage_checked: Option<std::time::Instant>,
    /// The moves you confirmed, which the next capture signs.
    confirmed_moves: BTreeSet<Id>,
    /// What the running capture leaves out.
    skipped_now: Vec<Skipped>,
    /// How long a manager's device leaves another's moves (§9).
    move_wait: std::time::Duration,
    /// The longest random wait before the device rotates a restricted
    /// domain whose rotation is pending (§9).
    rotation_wait: std::time::Duration,
    /// When the device rotates each such domain.
    rotation_due: BTreeMap<Id, std::time::Instant>,
    /// What the device holds in memory for text mode (§22).
    text: text::TextRuntime,
    /// What the device holds of presence (§23).
    presence: presence::PresenceRuntime,
    /// The longest wait before the device writes lost objects again (§14).
    restore_wait: std::time::Duration,
    /// The objects a rewind writes again, which matching leaves alone.
    lagging: BTreeSet<Id>,
}

/// What handling one realtime frame did (§23).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameOutcome {
    /// Other devices' records it applied.
    pub applied: usize,
    /// Whether it pulled, for a gap, a notify, a frame that failed a
    /// check or a restore generation the device hasn't seen.
    pub pulled: bool,
}

impl std::fmt::Debug for WorkspaceSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSync")
            .field("workspace", &self.state.workspace)
            .finish_non_exhaustive()
    }
}

/// The entries `log` requires, which the page a request brings owes.
fn wanted_entries(log: &Log) -> BTreeSet<(bool, u64)> {
    log.wanted.iter().map(|w| (w.epoch, w.number)).collect()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn core(error: local_core::CoreError) -> SyncError {
    SyncError::Invalid(format!("{}: {}", error.code, error.message))
}

/// Whether the folder at `folder` holds nothing, apart from the files the
/// system leaves in any folder.
pub fn is_empty_folder(folder: &Path) -> Result<bool, SyncError> {
    let entries = std::fs::read_dir(folder)
        .map_err(|e| SyncError::Invalid(format!("the folder can't be read: {e}")))?;
    Ok(entries.flatten().all(|entry| {
        matches!(
            entry.file_name().to_str(),
            Some(".DS_Store" | "desktop.ini")
        )
    }))
}

impl WorkspaceSync {
    /// Turns sync on for the workspace that `engine` opened: creates the
    /// remote workspace named `name` in one request (its genesis, the
    /// workspace domain's first epoch, an envelope for every active card of
    /// the account and the profile), starts this folder's journal and
    /// records the folder in `registry`. The first round uploads the
    /// files.
    pub async fn turn_on(
        service: &mut SyncService,
        engine: Arc<WorkspaceEngine>,
        name: &str,
        registry: &mut Registry,
    ) -> Result<Self, SyncError> {
        let mut remote = service.create_workspace(name).await?;
        let server = service.client().base().to_owned();
        let credentials = service.credentials().clone();
        let replica = registry.connect(
            &server,
            credentials.account,
            remote.workspace,
            engine.root(),
        )?;
        let generation = service.client().restore_generation();
        let (_, profile) = profile_of(service, &mut remote).await?;
        Self::start(
            service, engine, remote, server, replica, generation, name, profile,
        )
    }

    /// Downloads the remote workspace `workspace` into the folder `engine`
    /// opened, which must be empty apart from its new `.noura` folder: the
    /// device verifies the workspace, records the folder in `registry`, and
    /// the first round pulls every file from sequence 0 (§15, §18).
    pub async fn download(
        service: &mut SyncService,
        engine: Arc<WorkspaceEngine>,
        workspace: Id,
        registry: &mut Registry,
    ) -> Result<Self, SyncError> {
        let listing = engine.synced_listing().map_err(core)?;
        if !listing.files.is_empty() {
            return Err(SyncError::Invalid(
                "Choose an empty folder to download the workspace into.".into(),
            ));
        }
        // A new device takes the current epoch alone, and older ones only
        // once something it reads names one (§18, step 1).
        let mut remote = service.open_workspace_current(workspace).await?;
        let (name, profile) = profile_of(service, &mut remote).await?;
        let server = service.client().base().to_owned();
        let credentials = service.credentials().clone();
        let replica = registry.connect(&server, credentials.account, workspace, engine.root())?;
        let generation = service.client().restore_generation();
        let mut sync = Self::start(
            service, engine, remote, server, replica, generation, &name, profile,
        )?;
        // A new device loads the domain from its latest checkpoint (§18).
        sync.state.bootstrap = true;
        sync.save()?;
        Ok(sync)
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        service: &mut SyncService,
        engine: Arc<WorkspaceEngine>,
        remote: RemoteWorkspace,
        server: String,
        replica: Id,
        generation: u64,
        name: &str,
        profile: Option<ProfileMark>,
    ) -> Result<Self, SyncError> {
        let credentials = service.credentials().clone();
        let secrets = service.secrets.clone();
        let device = credentials.keys.device();
        let journal = Journal::open(engine.root(), &device, &*secrets)?;
        let mut mark = Mark::read(&*secrets, &device, &remote.workspace)?.unwrap_or_default();
        let next = mark.start().get(&remote.domain).copied().unwrap_or(1);
        let state = State {
            version: state::VERSION,
            replica,
            server,
            account: credentials.account,
            workspace: remote.workspace,
            domain: remote.domain,
            generation,
            deleted_away: Vec::new(),
            policy: policy_changes(&remote.policy),
            epochs: remote.epochs_oldest_first().into_iter().cloned().collect(),
            envelope: remote.envelope.clone(),
            name: name.to_owned(),
            profile,
            log: Log::new(remote.domain),
            own: Own {
                next,
                head: None,
                acked: None,
            },
            objects: BTreeMap::new(),
            outbox: Vec::new(),
            unreadable: Vec::new(),
            held: BTreeMap::new(),
            last_success: None,
            held_roots: Vec::new(),
            roots: Vec::new(),
            restricted: BTreeMap::new(),
            archived: Vec::new(),
            refused: Vec::new(),
            removal_hold: false,
            ended_epochs: Vec::new(),
            rewrite_epochs: Vec::new(),
            rewrite_others: Vec::new(),
            rewrite_due: 0,
            profile_recheck: false,
            ended: None,
            deferred_moves: BTreeMap::new(),
            moved_away: Vec::new(),
            move_due: BTreeMap::new(),
            moved_in: Vec::new(),
            bootstrap: false,
            bootstrap_progress: None,
            waiting_for_space: BTreeSet::new(),
            restore: Default::default(),
        };
        // A device accepts no policy head or epoch below its mark, also
        // when it starts again without a journal (§15).
        let epochs = BTreeMap::from([(
            state.domain,
            state.epochs.last().map_or(0, |e| e.body.epoch),
        )]);
        let revision = state.policy.last().map_or(0, |c| c.body.revision);
        if mark.above(revision, &epochs) {
            return Err(SyncError::Check("sync_policy_rollback"));
        }
        let keys = remote.keys.clone();
        let mut state = state;
        state.log.wanted = Vec::new();
        let mut sync = Self {
            engine,
            journal,
            state,
            mark,
            credentials,
            secrets,
            keys,
            remote: Some(remote),
            skipped: Vec::new(),
            stopped: None,
            paused: false,
            matching: false,
            rewrite_wait: REWRITE_WAIT,
            checkpoint_wait: CHECKPOINT_WAIT,
            cache: BTreeMap::new(),
            entry_hint: None,
            armed: BTreeSet::new(),
            frame_generation: None,
            restore_wait: restore::REWRITE_WAIT,
            lagging: BTreeSet::new(),
            active: None,
            parked: BTreeMap::new(),
            rematch: false,
            reviews: Vec::new(),
            usage: None,
            usage_checked: None,
            confirmed_moves: BTreeSet::new(),
            skipped_now: Vec::new(),
            move_wait: MOVE_WAIT,
            rotation_wait: super::upkeep::KEY_HOLDER_WAIT.end,
            rotation_due: BTreeMap::new(),
            text: text::TextRuntime::default(),
            presence: presence::PresenceRuntime::default(),
        };
        sync.want_entries();
        sync.save()?;
        Ok(sync)
    }

    /// Requires, in the log, the entry of each accepted change and each
    /// known epoch that the log hasn't carried yet (§8, §9).
    fn want_entries(&mut self) {
        let first = self.first_revision();
        let log = &mut self.state.log;
        let policy_seen = log.judge.latest_policy_entry().unwrap_or(0);
        for change in &self.state.policy {
            let number = change.body.revision;
            if number < first {
                continue;
            }
            let wanted = Wanted {
                epoch: false,
                number,
                by: u64::MAX,
            };
            if number > policy_seen && !log.wanted.iter().any(|w| !w.epoch && w.number == number) {
                log.wanted.push(wanted);
            }
        }
        for epoch in &self.state.epochs {
            let number = epoch.body.epoch;
            if number > log.epoch_entries
                && !log.wanted.iter().any(|w| w.epoch && w.number == number)
            {
                log.wanted.push(Wanted {
                    epoch: true,
                    number,
                    by: u64::MAX,
                });
            }
        }
    }

    /// Opens the journal of a folder that syncs, as each start of the app
    /// does, with no network: the device can capture and sign changes into
    /// its outbox before it reaches the server. Each start raises the
    /// counter reservation by one block (§14). A journal whose keyed hash
    /// fails, or that lies below the credential store's mark, reports
    /// `sync_journal_rollback`; a journal that holds another replica ID
    /// than `registry` gives this folder doesn't sync until you connect it
    /// (§15).
    pub fn open(
        engine: Arc<WorkspaceEngine>,
        credentials: DeviceCredentials,
        secrets: Secrets,
        registry: &Registry,
    ) -> Result<Self, SyncError> {
        let entry = registry
            .of_folder(engine.root())
            .ok_or_else(|| SyncError::Invalid("this folder doesn't sync".into()))?
            .clone();
        if !entry.on {
            return Err(SyncError::Invalid("sync is off for this folder".into()));
        }
        let device = credentials.keys.device();
        let journal = Journal::open(engine.root(), &device, &*secrets)?;
        let state: State = journal
            .read(STATE_FILE)?
            .ok_or(SyncError::Check("sync_journal_lost"))?;
        if state.replica != entry.replica || state.workspace != entry.workspace {
            return Err(SyncError::Check("sync_folder_not_connected"));
        }
        let mut mark = Mark::read(&*secrets, &device, &state.workspace)?.unwrap_or_default();
        let mut epochs: BTreeMap<Id, u64> = BTreeMap::from([(
            state.domain,
            state.epochs.last().map_or(0, |e| e.body.epoch),
        )]);
        for (domain, frame) in &state.restricted {
            epochs.insert(*domain, frame.epochs.last().map_or(0, |e| e.body.epoch));
        }
        let revision = state.policy.last().map_or(0, |c| c.body.revision);
        let checkpoint_seq = state
            .log
            .checkpoints
            .accepted
            .as_ref()
            .map_or(0, |a| a.record.body.covered_seq);
        if mark.above(revision, &epochs)
            || mark
                .domains
                .get(&state.domain)
                .is_some_and(|d| d.checkpoint_seq > checkpoint_seq)
        {
            return Err(SyncError::Check("sync_journal_rollback"));
        }
        let next = mark.start();
        mark.write(&*secrets, &device, &state.workspace)?;
        let mut sync = Self {
            engine,
            journal,
            keys: BTreeMap::new(),
            mark,
            credentials,
            secrets,
            remote: None,
            skipped: Vec::new(),
            stopped: None,
            paused: entry.paused,
            matching: false,
            rewrite_wait: REWRITE_WAIT,
            checkpoint_wait: CHECKPOINT_WAIT,
            cache: BTreeMap::new(),
            entry_hint: None,
            armed: BTreeSet::new(),
            frame_generation: None,
            restore_wait: restore::REWRITE_WAIT,
            lagging: BTreeSet::new(),
            active: None,
            parked: BTreeMap::new(),
            rematch: false,
            reviews: Vec::new(),
            usage: None,
            usage_checked: None,
            confirmed_moves: BTreeSet::new(),
            skipped_now: Vec::new(),
            move_wait: MOVE_WAIT,
            rotation_wait: super::upkeep::KEY_HOLDER_WAIT.end,
            rotation_due: BTreeMap::new(),
            text: text::TextRuntime::default(),
            presence: presence::PresenceRuntime::default(),
            state,
        };
        if let Some(next) = next.get(&sync.state.domain) {
            sync.state.own.next = sync.state.own.next.max(*next);
        }
        sync.keys = sync.offline_keys()?;
        for domain in sync.state.restricted.keys().copied().collect::<Vec<_>>() {
            sync.swap_domain(domain);
            if let Some(next) = next.get(&domain) {
                sync.state.own.next = sync.state.own.next.max(*next);
            }
            let keys = sync.offline_keys();
            if let Ok(keys) = &keys {
                sync.keys = keys.clone();
            }
            sync.swap_domain(domain);
            keys?;
        }
        sync.find_unsettled()?;
        sync.save()?;
        Ok(sync)
    }

    /// The epoch keys from the journal alone: this device's envelope of the
    /// latest epoch, unwrapped with its X-Wing secret and checked against
    /// the record's `key_check`, and every earlier key through lineage.
    fn offline_keys(&self) -> Result<BTreeMap<u64, SecretKey>, SyncError> {
        let (Some(latest), Some(envelope)) = (self.state.epochs.last(), &self.state.envelope)
        else {
            return Ok(BTreeMap::new());
        };
        let card = self.credentials.card();
        let key = unwrap(&envelope.body, &self.credentials.keys, &card)
            .map_err(|_| SyncError::Check("sync_key_mismatch"))?;
        if envelope.body.epoch != latest.body.epoch || !key_matches(&latest.body, &key) {
            return Err(SyncError::Check("sync_key_mismatch"));
        }
        let newest_first: Vec<_> = self.state.epochs.iter().rev().cloned().collect();
        let mut keys = BTreeMap::from([(latest.body.epoch, key.clone())]);
        // Lineage reaches the older keys when the chain is unbroken.
        if let Ok(derived) = walk_lineage(&newest_first, &key) {
            for (record, key) in newest_first.iter().zip(derived) {
                keys.insert(record.body.epoch, key);
            }
        }
        Ok(keys)
    }

    /// The workspace this folder syncs.
    pub fn workspace(&self) -> Id {
        self.state.workspace
    }

    /// The workspace domain, which holds this folder's files.
    pub fn domain(&self) -> Id {
        self.state.domain
    }

    /// This device.
    pub fn device(&self) -> Id {
        self.credentials.keys.device()
    }

    /// The object whose file sits at `path` on this device.
    pub fn rid_at(&self, path: &str) -> Option<Id> {
        self.state
            .objects
            .iter()
            .find(|(_, o)| o.local.as_ref().is_some_and(|l| l.path == path))
            .map(|(rid, _)| *rid)
    }

    /// Where `rid`'s file sits on this device, or the path its live state
    /// names when it has none here.
    pub fn local_path(&self, rid: &Id) -> Option<String> {
        let object = self.state.objects.get(rid)?;
        object.local.as_ref().map(|l| l.path.clone()).or_else(|| {
            object
                .states
                .values()
                .filter_map(|s| s.content.as_ref())
                .find(|c| !c.delete)
                .map(|c| c.path.clone())
        })
    }

    /// The workspace domain's cursor: the log position through which the
    /// device applied every record (§15).
    pub fn cursor(&self) -> u64 {
        self.state.log.cursor
    }

    /// The workspace's name, from its profile.
    pub fn name(&self) -> &str {
        &self.state.name
    }

    /// The folder's engine.
    pub fn engine(&self) -> &Arc<WorkspaceEngine> {
        &self.engine
    }

    /// The verified remote workspace, once the device reached the server.
    pub fn remote_mut(&mut self) -> Option<&mut RemoteWorkspace> {
        self.remote.as_mut()
    }

    /// What Settings shows.
    pub fn status(&self) -> Status {
        Status {
            name: self.state.name.clone(),
            pending: self.state.outbox.len()
                + self
                    .state
                    .restricted
                    .values()
                    .map(|f| f.outbox.len())
                    .sum::<usize>(),
            waiting_for_space: self.state.waiting_for_space.len(),
            restore_lost: self.state.restore.lost.clone(),
            rollback: self.state.restore.rollback.clone(),
            restored: self.state.restore.alerted,
            written_again: self.state.restore.written_again.clone(),
            conflicts: self.conflicts().len(),
            skipped: self.skipped.clone(),
            last_success: self.state.last_success,
            unreadable: self.state.unreadable.len(),
            held: self.state.held.len(),
            held_roots: self.state.held_roots.len(),
            archived: self.state.archived.len(),
            refused: self.state.refused.len(),
            ended: self.state.ended.clone(),
            deleted_away: self.state.deleted_away.len(),
            usage: self.usage,
        }
    }

    /// Files whose objects were deleted on another device while this one
    /// was away, by path (§18).
    pub fn deleted_away(&self) -> Vec<String> {
        self.state
            .deleted_away
            .iter()
            .map(|f| f.path.clone())
            .collect()
    }

    /// Confirms the deletion of a file [`Self::deleted_away`] lists.
    pub fn confirm_deleted_away(&mut self, path: &str) -> Result<(), SyncError> {
        let Some(index) = self.state.deleted_away.iter().position(|f| f.path == path) else {
            return Ok(());
        };
        let revision = self.state.deleted_away[index].revision.to_hex();
        self.engine.delete_synced(path, &revision).map_err(core)?;
        self.state.deleted_away.remove(index);
        self.save()
    }

    /// Keeps a file [`Self::deleted_away`] lists: it uploads as a new
    /// object in the next capture.
    pub fn keep_deleted_away(&mut self, path: &str) -> Result<(), SyncError> {
        self.state.deleted_away.retain(|f| f.path != path);
        self.save()
    }

    /// Reads the workspace's usage and quota, at most once a minute.
    async fn refresh_usage(&mut self, service: &mut SyncService) {
        let now = std::time::Instant::now();
        if self
            .usage_checked
            .is_some_and(|at| now.duration_since(at) < USAGE_REFRESH)
        {
            return;
        }
        if let Ok(response) = service.client().workspaces().await {
            self.usage_checked = Some(now);
            if let Some(info) = response
                .workspaces
                .iter()
                .find(|w| w.workspace == self.state.workspace)
            {
                self.usage = Some(Usage {
                    used: info.used_bytes,
                    history: info.history_bytes,
                    quota: info.quota_bytes,
                });
            }
        }
    }

    /// The records that passed every check but didn't open, each as the
    /// object, its signer and its counter (§13).
    pub fn unreadable(&self) -> Vec<(Id, Id, u64)> {
        self.state
            .unreadable
            .iter()
            .map(|u| (u.rid, u.device, u.counter))
            .collect()
    }

    /// The objects the device refuses to write, such as one at a
    /// `.noura-domain` path that no domain names as its marker (§9).
    pub fn refused_objects(&self) -> &[Id] {
        &self.state.refused
    }

    /// Signs a file operation at `path` into the outbox as the client would,
    /// whatever the scanner reports, for tests that need a record the
    /// client never writes on its own.
    #[cfg(feature = "test-hooks")]
    pub fn sign_file_at(&mut self, path: &str, bytes: &[u8]) -> Result<Id, SyncError> {
        let rid = random(Id::random())?;
        self.state.objects.insert(rid, Object::default());
        let found = Found {
            file: local_core::SyncedFile {
                relative_path: path.to_owned(),
                size: bytes.len() as u64,
                modified_ns: 0,
                file_id: None,
            },
            revision: hash(bytes),
            object_id: String::new(),
            bytes: Some(bytes.to_vec()),
        };
        self.sign_file(rid, &found, Vec::new())?;
        self.state.objects.get_mut(&rid).expect("signed").local = None;
        self.save()?;
        Ok(rid)
    }

    /// Signs into the outbox an operation whose ciphertext opens under no
    /// key, as a device whose key the server can't see misuses it, for
    /// tests of unreadable records (§13).
    #[cfg(feature = "test-hooks")]
    pub fn sign_unreadable(&mut self, rid: Id) -> Result<(), SyncError> {
        let covers = self.edit_covers(&rid);
        let payload = Payload {
            rid,
            kind: PayloadKind::Delete {
                object_id: Text(String::new()),
                revision: Digest::ZERO,
                reason: DeleteReason::Deleted,
                target: None,
            },
        };
        let carried = Carried {
            path: String::new(),
            revision: Digest::ZERO,
            delete: true,
            object_id: String::new(),
            kept: None,
            blob: None,
            text: None,
        };
        self.sign(rid, false, covers, &payload, carried)?;
        self.save()
    }

    /// Why the device stopped writing the domain, if it did.
    pub fn stopped(&self) -> Option<&'static str> {
        self.stopped
    }

    /// Takes again whether the device holds its writes after a removal
    /// the workspace hasn't recorded and rotated past (§9); when the hold
    /// ends, the device takes the new keys first.
    pub async fn update_hold(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let was = self.state.removal_hold;
        if was {
            // The change that records the head, and the rotation after it,
            // end the hold: the device takes the policy and keys again.
            self.refresh(service).await?;
        }
        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };
        let held = service.removal_hold(remote).await?;
        self.state.removal_hold = held;
        if was != held {
            self.save()?;
        }
        Ok(())
    }

    /// Whether the device holds its writes after it learned of a removal
    /// the workspace hasn't recorded and rotated past (§9).
    pub fn removal_hold(&self) -> bool {
        self.state.removal_hold
    }

    /// The digests of the signed records waiting for the server, in chain
    /// order.
    pub fn outbox(&self) -> Vec<Digest> {
        self.state
            .outbox
            .iter()
            .map(|o| o.record.digest())
            .collect()
    }

    /// The accepted policy head's revision.
    pub fn policy_revision(&self) -> u64 {
        self.accepted_revision()
    }

    /// Whether you paused sync for this folder.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Pauses or resumes sync: while paused, rounds capture changes into
    /// the outbox and send nothing; the pause holds across relaunch, and
    /// the round after Resume uploads what queued.
    pub fn set_paused(&mut self, registry: &mut Registry, paused: bool) -> Result<(), SyncError> {
        registry.update(
            &self.state.server,
            &self.state.account,
            &self.state.workspace,
            |entry| entry.paused = paused,
        )?;
        self.paused = paused;
        Ok(())
    }

    /// Turns sync off for this folder: no more traffic, and every file stays
    /// in place and editable. The journal stays, so turning sync on again
    /// with [`WorkspaceSync::turn_on_again`] continues the same remote
    /// workspace.
    pub fn turn_off(self, registry: &mut Registry) -> Result<(), SyncError> {
        registry.update(
            &self.state.server,
            &self.state.account,
            &self.state.workspace,
            |entry| entry.on = false,
        )
    }

    /// Turns sync on again for a folder whose sync you turned off, as
    /// [`WorkspaceSync::open`] does at each start.
    pub fn turn_on_again(
        engine: Arc<WorkspaceEngine>,
        credentials: DeviceCredentials,
        secrets: Secrets,
        registry: &mut Registry,
    ) -> Result<Self, SyncError> {
        let entry = registry
            .of_folder(engine.root())
            .ok_or_else(|| SyncError::Invalid("this folder doesn't sync".into()))?
            .clone();
        registry.update(&entry.server, &entry.account, &entry.workspace, |e| {
            e.on = true
        })?;
        Self::open(engine, credentials, secrets, registry)
    }

    /// Writes the journal, with the workspace domain's part in its place
    /// wherever the device works.
    fn save(&mut self) -> Result<(), SyncError> {
        let Some(active) = self.active else {
            return self.save_here();
        };
        self.swap_domain(active);
        let saved = self.save_here();
        self.swap_domain(active);
        saved
    }

    fn save_here(&mut self) -> Result<(), SyncError> {
        self.journal.write(STATE_FILE, &self.state)?;
        // An approval offer anchors the latest accepted checkpoint (§10).
        let accepted = self
            .state
            .log
            .checkpoints
            .accepted
            .as_ref()
            .map(|a| (a.record.body.covered_seq, a.record.digest()));
        if let (Some(remote), Some((covered_seq, checkpoint))) = (self.remote.as_mut(), accepted) {
            remote.checkpoint = Some((
                sync_protocol::records::CheckpointAnchor {
                    domain: self.state.domain,
                    covered_seq,
                    checkpoint,
                },
                self.state.generation,
            ));
        }
        // The mark follows the journal, never ahead of it (§15).
        let revision = self.state.policy.last().map_or(0, |c| c.body.revision);
        let head = self
            .state
            .policy
            .last()
            .map_or(Digest::ZERO, |c| c.digest());
        let domain = self.mark.domains.entry(self.state.domain).or_default();
        let (epoch, record) = self
            .state
            .epochs
            .last()
            .map_or((0, Digest::ZERO), |e| (e.body.epoch, e.digest()));
        let (checkpoint_seq, checkpoint) = accepted.unwrap_or((0, Digest::ZERO));
        if revision != self.mark.policy_revision
            || epoch != domain.epoch
            || checkpoint_seq > domain.checkpoint_seq
        {
            domain.epoch = epoch;
            domain.epoch_record = record;
            if checkpoint_seq > domain.checkpoint_seq {
                domain.checkpoint_seq = checkpoint_seq;
                domain.checkpoint = checkpoint;
                domain.checkpoint_generation = self.state.generation;
            }
            self.mark.policy_revision = revision;
            self.mark.policy_head = head;
            self.mark.write(
                &*self.secrets,
                &self.credentials.keys.device(),
                &self.state.workspace,
            )?;
        }
        // Each restricted domain's latest epoch and checkpoint (§15).
        let mut moved = false;
        for (id, frame) in &self.state.restricted {
            let domain = self.mark.domains.entry(*id).or_default();
            let (epoch, record) = frame
                .epochs
                .last()
                .map_or((0, Digest::ZERO), |e| (e.body.epoch, e.digest()));
            if epoch != domain.epoch {
                domain.epoch = epoch;
                domain.epoch_record = record;
                moved = true;
            }
            if let Some(accepted) = &frame.log.checkpoints.accepted {
                let covered = accepted.record.body.covered_seq;
                if covered > domain.checkpoint_seq {
                    domain.checkpoint_seq = covered;
                    domain.checkpoint = accepted.record.digest();
                    domain.checkpoint_generation = self.state.generation;
                    moved = true;
                }
            }
        }
        if moved {
            self.mark.write(
                &*self.secrets,
                &self.credentials.keys.device(),
                &self.state.workspace,
            )?;
        }
        Ok(())
    }

    /// One round: captures local changes into the outbox, connects when it
    /// hasn't yet, pushes the outbox, and pulls once, waiting up to
    /// `wait_seconds` for new records.
    pub async fn round(
        &mut self,
        service: &mut SyncService,
        wait_seconds: u32,
    ) -> Result<Round, SyncError> {
        self.round_until(service, wait_seconds, &tokio::sync::Notify::new())
            .await
    }

    /// A [`round`](Self::round) whose pull stops waiting once `wake` is
    /// notified and asks again without waiting, so a local change or
    /// something you did gets the next round at once rather than after the
    /// wait. Only the pull's wait ends early; a push in flight always
    /// finishes.
    pub async fn round_until(
        &mut self,
        service: &mut SyncService,
        wait_seconds: u32,
        wake: &tokio::sync::Notify,
    ) -> Result<Round, SyncError> {
        // A change took this device's access: nothing more goes out or
        // comes in, and the files stay as they are (§15).
        if self.state.ended.is_some() {
            return Ok(Round::default());
        }
        // A stopped workspace keeps its files and does nothing (§14).
        let gate = self.restore_gate(service).await;
        match gate {
            Ok(true) => {}
            Ok(false) => return Ok(Round::default()),
            Err(error) => {
                self.note_failure(error.clone_check())?;
                return Err(error);
            }
        }
        match self.running_round(service, wait_seconds, wake).await {
            // The round met a new generation: the next one compares.
            Err(SyncError::Check("sync_restore_generation")) => {
                match self.restore_gate(service).await {
                    Ok(_) => Ok(Round::default()),
                    Err(error) => Err(error),
                }
            }
            Err(SyncError::Refused {
                code,
                status,
                detail,
            }) if code == "not_found" => {
                if self.note_closed(service).await? {
                    return Ok(Round::default());
                }
                Err(SyncError::Refused {
                    code,
                    status,
                    detail,
                })
            }
            Err(error) => {
                self.note_failure(error.clone_check())?;
                Err(error)
            }
            round => round,
        }
    }

    async fn running_round(
        &mut self,
        service: &mut SyncService,
        wait_seconds: u32,
        wake: &tokio::sync::Notify,
    ) -> Result<Round, SyncError> {
        if self.paused {
            let captured = self.capture()?;
            return Ok(Round {
                captured,
                ..Round::default()
            });
        }
        let online = async {
            if self.remote.is_none() {
                self.connect(service).await?;
            }
            self.update_hold(service).await?;
            self.sync_domains(service).await
        }
        .await;
        if let Err(error) = online {
            // Offline, changes still enter the outbox.
            self.capture()?;
            return Err(error);
        }
        // The device reconciles a domain before it writes there again.
        let mut captured = if self.rewrites_pending() {
            self.write_again_due()?
        } else {
            self.capture()?
        };
        let mut pushed = self.push_everything(service).await?;
        if self.state.restore.alerted.is_some() {
            self.upload_missing(service).await?;
        }
        if self.state.profile_recheck && unix_now() >= self.state.rewrite_due {
            self.state.profile_recheck = false;
            self.follow_profile(service).await?;
        }
        let (applied, waiting_for_key) = self.pull_until(service, wait_seconds, wake).await?;
        self.settle_moved_away()?;
        self.rotate_domains(service, false).await?;
        self.sign_checkpoint(service).await?;
        for domain in self.loaded_domains() {
            self.swap_domain(domain);
            let result = self.sign_checkpoint(service).await;
            self.swap_domain(domain);
            result?;
        }
        // A replacement the pull brought ends epochs whose records this
        // device writes again at once (§9); merges go out next round.
        if !self.state.rewrite_epochs.is_empty() {
            captured += self.capture()?;
            pushed += self.push(service).await?;
        }
        if pushed > 0 {
            self.usage_checked = None;
        }
        self.refresh_usage(service).await;
        self.state.last_success = Some(unix_now());
        self.save()?;
        Ok(Round {
            captured,
            pushed,
            applied,
            waiting_for_key,
        })
    }

    /// Verifies the workspace on the server and takes its policy and keys,
    /// which must extend what the journal accepted (§8, §9), then compares
    /// this device's chain with the server's head (§14).
    pub async fn connect(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        // A device still bootstrapping takes the current epoch alone and
        // walks back only as far as something it reads needs (§18).
        let remote = if self.state.bootstrap {
            let mut remote = service.open_workspace_current(self.state.workspace).await?;
            if let Some(accepted) = self.state.epochs.last() {
                service
                    .older_epochs(&mut remote, accepted.body.epoch)
                    .await?;
            }
            remote
        } else {
            service.open_workspace(self.state.workspace).await?
        };
        self.adopt(remote)?;
        self.compare_own_chain(service).await?;
        self.follow_profile(service).await?;
        self.save()
    }

    /// Takes the workspace's latest profile, checked against the verified
    /// policy and the last one the device accepted (§25). When a change
    /// took the device that signed it out of the workspace, an owner or
    /// admin device signs the profile again at its accepted head; the
    /// server keeps the first.
    async fn follow_profile(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        // The profile lives in the workspace domain (§25).
        if self.remote.is_none() || self.active.is_some() {
            return Ok(());
        }
        let list = service.client().workspaces().await?;
        let Some(profile) = list
            .workspaces
            .0
            .into_iter()
            .find(|w| w.workspace == self.state.workspace)
            .and_then(|w| w.profile)
            .map(|p| p.0)
        else {
            return Ok(());
        };
        self.ensure_epoch(service, profile.body.epoch).await?;
        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };
        let accepted = self.state.profile;
        if remote.epochs.ended_at(profile.body.epoch).is_some() {
            // A replacement skipped the epoch the profile was signed under:
            // the device that signed it signs it again under the current
            // epoch (§25); others keep the name they accepted.
            // After the random wait, so does each owner or admin device
            // that accepted that epoch and finds the profile still under it.
            let signer = profile.body.signer == self.credentials.keys.device();
            let accepted_it = self
                .state
                .ended_epochs
                .iter()
                .any(|e| e.body.epoch == profile.body.epoch);
            let due = unix_now() >= self.state.rewrite_due;
            if self.may_rename() && (signer || (accepted_it && due)) {
                let name = self.state.name.clone();
                return match self
                    .sign_profile(service, &name, profile.body.profile_revision)
                    .await
                {
                    // Another device's profile arrived first; the server
                    // keeps the first (§25).
                    Err(SyncError::Refused { code, .. }) if code == "profile_conflict" => Ok(()),
                    other => other,
                };
            }
            self.state.profile_recheck = accepted_it;
            return Ok(());
        }
        let signer_gone = !remote
            .policy
            .head_snapshot()
            .is_device(&profile.body.signer);
        if signer_gone {
            // A change took its signer out of the workspace, so the device
            // shows it no more; an owner or admin device signs the same
            // name again at its accepted head (§25).
            let name = check_profile_from(remote, &profile, accepted.as_ref(), false)?;
            if self.may_rename() {
                return self
                    .sign_profile(service, &name, profile.body.profile_revision)
                    .await;
            }
            return Ok(());
        }
        let name = check_profile(remote, &profile, accepted.as_ref())?;
        self.state.name = name;
        self.state.profile = Some(mark_of(&profile));
        Ok(())
    }

    /// Whether this device may change the profile: an owner or admin
    /// device in the accepted policy.
    fn may_rename(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| {
            let snapshot = r.policy.head_snapshot();
            snapshot
                .account_of(&self.credentials.keys.device())
                .and_then(|account| snapshot.role(&account))
                .is_some_and(|role| matches!(role, Role::Owner | Role::Admin))
        })
    }

    /// Changes a member's role, removes a member, leaves, or transfers
    /// ownership (§8), as [`SyncService::change_member`], and takes the
    /// policy and keys that result. Returns the new policy revision.
    pub async fn change_member(
        &mut self,
        service: &mut SyncService,
        change: crate::sync::members::MemberChange,
    ) -> Result<u64, SyncError> {
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        let mut remote = self.remote.take().ok_or(SyncError::KeyMissing)?;
        let changed = service.change_member(&mut remote, change).await;
        self.adopt(remote)?;
        let revision = changed?;
        // The signer rotates each restricted domain it still reads (§9).
        self.sync_domains(service).await?;
        self.rotate_domains(service, true).await?;
        self.save()?;
        Ok(revision)
    }

    /// Renames the workspace by pushing a new profile, as an owner or admin
    /// device may (§25).
    pub async fn rename(&mut self, service: &mut SyncService, name: &str) -> Result<(), SyncError> {
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        if !self.may_rename() {
            return Err(SyncError::Invalid(
                "Only the workspace's owner or an admin can rename it.".into(),
            ));
        }
        let latest = self.state.profile.map_or(0, |p| p.revision);
        self.sign_profile(service, name, latest).await?;
        self.save()
    }

    /// Signs and pushes a profile named `name` one above `latest`, under
    /// the latest epoch at the accepted policy head. When another device's
    /// profile took that revision first, the device takes that one.
    async fn sign_profile(
        &mut self,
        service: &mut SyncService,
        name: &str,
        latest: u64,
    ) -> Result<(), SyncError> {
        let epoch = self
            .state
            .epochs
            .last()
            .ok_or(SyncError::Check("sync_key_mismatch"))?;
        let key = self
            .keys
            .get(&epoch.body.epoch)
            .ok_or(SyncError::Check("sync_key_mismatch"))?;
        let (policy_revision, policy_head) = self
            .remote
            .as_ref()
            .ok_or(SyncError::Check("sync_workspace_unverified"))?
            .policy
            .head();
        let mut profile = Profile {
            workspace: self.state.workspace,
            profile_revision: latest + 1,
            epoch: epoch.body.epoch,
            policy_revision,
            policy_head,
            signer: self.credentials.keys.device(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        profile.seal_payload(
            &ProfilePayload {
                name: Text::new(name).map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            },
            key,
            &epoch.digest(),
            random(random_nonce())?,
        );
        let signed = Signed::sign(profile, self.credentials.keys.signing());
        match service
            .client()
            .push(
                self.state.workspace,
                vec![RecordRef::Profile(signed.clone())],
            )
            .await
        {
            Ok(_) => {
                self.state.name = name.to_owned();
                self.state.profile = Some(mark_of(&signed));
                Ok(())
            }
            Err(SyncError::Refused { code, .. }) if code == "profile_conflict" => {
                Box::pin(self.follow_profile(service)).await
            }
            Err(error) => Err(error),
        }
    }

    /// Takes a verified remote workspace's policy and keys, which must
    /// extend the journal's: the same change at each accepted revision and
    /// the accepted epochs, or a replacement that ended them.
    fn adopt(&mut self, remote: RemoteWorkspace) -> Result<(), SyncError> {
        let (revision, _) = remote.policy.head();
        if let Some(accepted) = self.state.policy.last() {
            let at = accepted.body.revision;
            if revision < at {
                return Err(SyncError::Check("sync_policy_rollback"));
            }
            if remote.policy.change(at).map(|c| c.digest()) != Some(accepted.digest()) {
                return Err(SyncError::Check("sync_policy_forked"));
            }
        }
        if let Some(accepted) = self.state.epochs.last() {
            let number = accepted.body.epoch;
            let kept = remote
                .epochs
                .get(number)
                .is_some_and(|e| e.digest == accepted.digest())
                || remote.epochs.ended_at(number).is_some();
            if !kept {
                return Err(SyncError::Check("sync_key_mismatch"));
            }
        }
        self.state.policy = policy_changes(&remote.policy);
        let epochs: Vec<_> = remote.epochs_oldest_first().into_iter().cloned().collect();
        // Epochs this device accepted that a replacement ended: its own
        // records under them are written again (§9).
        let ended: Vec<_> = self
            .state
            .epochs
            .iter()
            .filter(|e| remote.epochs.ended_at(e.body.epoch).is_some())
            .cloned()
            .collect();
        for record in &ended {
            if !self
                .state
                .ended_epochs
                .iter()
                .any(|e| e.digest() == record.digest())
            {
                self.state.ended_epochs.push(record.clone());
                self.state.rewrite_epochs.push(record.body.epoch);
                // A rewrite already waiting keeps its deadline, so
                // replacements in quick succession can't keep pushing it
                // back (§9).
                let waiting = !self.state.rewrite_others.is_empty();
                self.state.rewrite_others.push(record.body.epoch);
                let due = unix_now()
                    + super::heads::random_wait(&(std::time::Duration::ZERO..self.rewrite_wait))
                        .as_secs();
                self.state.rewrite_due = if waiting {
                    self.state.rewrite_due.min(due)
                } else {
                    due
                };
            }
        }
        self.state.epochs = epochs;
        self.want_entries();
        if remote.envelope.is_some() {
            self.state.envelope = remote.envelope.clone();
        }
        self.keys.extend(remote.keys.clone());
        self.remote = Some(remote);
        Ok(())
    }

    /// Makes sure the device holds epoch `epoch`'s record and key, or
    /// knows that a replacement ended it, walking back through
    /// `epochs/{epoch}` from the oldest record it holds when it doesn't
    /// (§18, step 1).
    pub(super) async fn ensure_epoch(
        &mut self,
        service: &mut SyncService,
        epoch: u64,
    ) -> Result<(), SyncError> {
        let Some(remote) = self.remote.as_mut() else {
            return Ok(());
        };
        if remote.epochs.get(epoch).is_some() || remote.epochs.ended_at(epoch).is_some() {
            return Ok(());
        }
        service.older_epochs(remote, epoch).await?;
        self.keys.extend(remote.keys.clone());
        self.state.epochs = remote.epochs_oldest_first().into_iter().cloned().collect();
        self.save()
    }

    /// Takes the policy and keys again after a push response, a pull or a
    /// refusal named a revision above the accepted head (§9).
    async fn refresh(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let mut remote = match self.remote.take() {
            Some(remote) => remote,
            None => service.open_workspace(self.state.workspace).await?,
        };
        let hint = self.entry_hint;
        let refreshed = async {
            service.refresh_policy(&mut remote).await?;
            service.refresh_keys_at(&mut remote, hint).await
        }
        .await;
        if let Err(error) = refreshed {
            self.remote = Some(remote);
            return Err(error);
        }
        let before = (self.accepted_revision(), self.state.ended_epochs.len());
        self.adopt(remote)?;
        if (self.accepted_revision(), self.state.ended_epochs.len()) != before {
            self.follow_profile(service).await?;
        }
        Ok(())
    }

    /// Takes the policy again, and nothing else: a device that lost its
    /// access may read the policy up to the change that took it (§7).
    async fn refresh_policy(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let Some(mut remote) = self.remote.take() else {
            return Err(SyncError::KeyMissing);
        };
        let refreshed = service.refresh_policy(&mut remote).await;
        self.state.policy = policy_changes(&remote.policy);
        self.remote = Some(remote);
        refreshed.map(|_| ())
    }

    /// The latest accepted change that took this device's read access to
    /// the domain, if the verified policy holds one at its head.
    fn access_ended(&self) -> Option<Ended> {
        let policy = &self.remote.as_ref()?.policy;
        let own = self.credentials.keys.device();
        let domain = self.state.domain;
        if policy.head_snapshot().can_read(&own, &domain) {
            return None;
        }
        let head = policy.head().0;
        (2..=head).rev().find_map(|revision| {
            let before = policy.at(revision - 1)?;
            let after = policy.at(revision)?;
            if !before.can_read(&own, &domain) || after.can_read(&own, &domain) {
                return None;
            }
            let change = policy.change(revision)?;
            let account = self.credentials.account;
            Some(Ended {
                revision,
                by_device: change.body.signer,
                by_account: before.account_of(&change.body.signer),
                account_removed: matches!(
                    change.body.kind,
                    PolicyKind::RemoveMember { account: removed } if removed == account
                ),
                domain_deleted: !after.domains.contains_key(&domain),
            })
        })
    }

    /// Compares the journal's head of this device's chain with the server's
    /// (§14): a server head above it means the journal lost records the
    /// device sent, which it takes back after checking its own signature on
    /// each; a head below one the server acknowledged means the server went
    /// back.
    async fn compare_own_chain(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let device = self.credentials.keys.device();
        let head = service
            .client()
            .chain_head(self.state.workspace, self.state.domain, device)
            .await?;
        let acked = self.state.own.acked.map_or(0, |(c, _, _)| c);
        let server = head.as_ref().map_or(0, |h| h.counter);
        if server < acked {
            return Err(SyncError::Check("sync_server_rollback"));
        }
        if server == acked {
            return Ok(());
        }
        // Records the journal lost: take each back, oldest first.
        let mut counter = acked + 1;
        let mut previous = self.state.own.acked.map_or(Digest::ZERO, |(_, d, _)| d);
        while counter <= server {
            let Some(entry) = service
                .client()
                .chain_position(self.state.workspace, self.state.domain, device, counter)
                .await?
            else {
                return Err(SyncError::Check("sync_chain_gap"));
            };
            let EntryRecord::Operation(op) = &entry.record else {
                return Err(SyncError::Check("sync_chain_gap"));
            };
            if op.body.device != device
                || op.body.previous != previous
                || op
                    .verify(
                        &self
                            .credentials
                            .keys
                            .card(self.credentials.account)
                            .signing_public,
                    )
                    .is_err()
            {
                return Err(SyncError::Check("sync_chain_forked"));
            }
            previous = op.digest();
            self.state.own.acked = Some((op.body.counter, previous, entry.seq));
            counter = op.body.counter + 1;
        }
        // The outbox's first record follows the server's head now.
        let acked = self.state.own.acked.expect("taken back");
        if self.state.own.head.is_none_or(|(c, _)| c < acked.0) {
            self.state.own.head = Some((acked.0, acked.1));
        }
        self.state.own.next = self.state.own.next.max(acked.0 + 1);
        if !self.state.outbox.is_empty() {
            self.reseal_from(0, true)?;
        }
        Ok(())
    }

    /// Finds the local changes and signs each into the outbox, which
    /// reaches the disk before this returns. Works without a network.
    /// Returns how many changes it signed.
    pub fn capture(&mut self) -> Result<usize, SyncError> {
        // A held domain takes no new encryption; the files keep their
        // changes until the hold ends (§9).
        if self.stopped.is_some() || self.state.removal_hold || self.state.ended.is_some() {
            return Ok(0);
        }
        let rewritten = self.rewrite_skipped()?;
        let ticked = self.text_tick(std::time::Instant::now())?;
        let moved_markers = self.check_markers()?;
        let listing = self.engine.synced_listing().map_err(core)?;
        let held: BTreeSet<Id> = self.state.held.keys().copied().collect();
        let engine = self.engine.clone();
        let roots = self.state.held_roots.clone();
        let markers = self.marker_objects();
        let holding_new = self.markers_pending();
        let waiting: BTreeMap<String, Digest> = self
            .state
            .deleted_away
            .iter()
            .map(|f| (f.path.clone(), f.revision))
            .collect();
        let (changes, mut skipped) = capture::changes(
            &listing,
            &self.state.objects,
            &markers,
            &held,
            &waiting,
            |path| {
                roots
                    .iter()
                    .any(|root| rebuild::beneath(path, &root.folder))
            },
            |file| read_for_sync(&engine, &file.relative_path, file.size),
        );
        // A file you changed or took away leaves the list: a change uploads
        // as a new object (§18).
        let objects = &self.state.objects;
        self.state.deleted_away.retain(|f| {
            listing
                .files
                .iter()
                .any(|file| file.relative_path == f.path)
                && !objects
                    .values()
                    .any(|o| o.local.as_ref().is_some_and(|l| l.path == f.path))
        });
        let mut signed = rewritten + ticked + moved_markers;
        self.reviews.clear();
        for change in changes {
            match change {
                Change::Touch { rid, found } => {
                    if let Some(object) = self.state.objects.get_mut(&rid) {
                        object.local = Some(found.local());
                    }
                }
                Change::Create(found) => {
                    if found.file.size > sync_protocol::blob::MAX_BLOB_FILE {
                        skipped.push(Skipped {
                            path: found.file.relative_path.clone(),
                            why: SkipWhy::TooLarge,
                        });
                        continue;
                    }
                    // Until the device applied every marker it reads, it
                    // can't tell where a new file goes (§9).
                    if holding_new {
                        skipped.push(Skipped {
                            path: found.file.relative_path.clone(),
                            why: SkipWhy::HeldRoot,
                        });
                        continue;
                    }
                    let domain = domain_of(&found.file.relative_path, &self.state.roots);
                    let rid = random(Id::random())?;
                    self.state.objects.insert(
                        rid,
                        Object {
                            object_id: found.object_id.clone(),
                            domain,
                            ..Object::default()
                        },
                    );
                    self.in_domain(domain, |sync| sync.sign_file(rid, &found, Vec::new()))?;
                    signed += 1;
                }
                Change::Update { rid, found } => {
                    if found.file.size > sync_protocol::blob::MAX_BLOB_FILE {
                        skipped.push(Skipped {
                            path: found.file.relative_path.clone(),
                            why: SkipWhy::TooLarge,
                        });
                        continue;
                    }
                    let domain = self.object_domain(&rid);
                    // A file that crossed a root is a move between domains,
                    // and a workspace-domain object inside a new root waits
                    // for its move (§9).
                    let target = domain_of(&found.file.relative_path, &self.state.roots);
                    if target != domain {
                        // A note that moves to another domain leaves text
                        // mode as a `file` operation (§22).
                        let moved = self.crossing(rid, &found, domain, target)?;
                        signed += moved;
                        if moved > 0 {
                            self.end_text(rid)?;
                        }
                        continue;
                    }
                    if self.in_text_mode(&rid) {
                        signed += self.capture_text(rid, &found, false)?;
                        continue;
                    }
                    self.in_domain(domain, |sync| {
                        let covers = sync.edit_covers(&rid);
                        sync.sign_file(rid, &found, covers)
                    })?;
                    signed += 1;
                }
                Change::Delete { rid } => {
                    let domain = self.object_domain(&rid);
                    self.in_domain(domain, |sync| {
                        let covers = if sync.in_text_mode(&rid) {
                            let view = sync.live_ops(&rid).into_iter().collect();
                            sync.covers_of(&rid, &view)
                        } else {
                            sync.edit_covers(&rid)
                        };
                        sync.sign_delete(rid, covers)
                    })?;
                    self.end_text(rid)?;
                    signed += 1;
                }
            }
        }
        // An object this device wrote that yields its path to another
        // object moves to the name it shows (§16).
        for rid in self.yielded_moves() {
            let Some(local) = self.state.objects[&rid].local.clone() else {
                continue;
            };
            let Ok(found) = self.found_on_disk(&local.path) else {
                continue;
            };
            // A text-mode note moves by a text update that writes `path`
            // (§22).
            if self.in_text_mode(&rid) {
                signed += self.capture_text(rid, &found, true)?;
                continue;
            }
            let domain = self.object_domain(&rid);
            self.in_domain(domain, |sync| {
                let covers = sync.edit_covers(&rid);
                sync.sign_file(rid, &found, covers)
            })?;
            signed += 1;
        }
        signed += self.finish_moves()?;
        skipped.append(&mut self.skipped_now);
        skipped.sort();
        self.skipped = skipped;
        // A change that still waits for space went back into an outbox; one
        // you undid or deleted waits for nothing.
        let signed_rids: BTreeSet<Id> = self
            .state
            .outbox
            .iter()
            .chain(self.state.restricted.values().flat_map(|f| &f.outbox))
            .map(|o| o.record.body.rid)
            .collect();
        self.state
            .waiting_for_space
            .retain(|rid| signed_rids.contains(rid));
        self.save()?;
        Ok(signed)
    }

    /// The covers of an edit to `rid` (§13, §16): the state the local file
    /// holds and every live state identical to it, and this device's own
    /// earlier records, so the other branches of a conflict stay live.
    fn edit_covers(&self, rid: &Id) -> Vec<Cover> {
        let Some(object) = self.state.objects.get(rid) else {
            return Vec::new();
        };
        let mut view: BTreeSet<OpKey> = BTreeSet::new();
        if object.cover_all {
            view.extend(self.live_states(rid));
        }
        if let Some(base) = object.base {
            view.insert(base);
            let base_content = object.states.get(&base).and_then(|s| s.content.clone());
            if let Some(base_content) = base_content {
                for key in self.live_states(rid) {
                    let same = object
                        .states
                        .get(&key)
                        .and_then(|s| s.content.as_ref())
                        .is_some_and(|c| same_state(c, &base_content));
                    if same {
                        view.insert(key);
                    }
                }
            }
        }
        self.covers_of(rid, &view)
    }

    /// Covers naming each device's highest operation in `view`, and this
    /// device's highest record on `rid`, so it leaves none of its own
    /// earlier states live.
    fn covers_of(&self, rid: &Id, view: &BTreeSet<OpKey>) -> Vec<Cover> {
        let own = self.credentials.keys.device();
        let mut highest: BTreeMap<Id, u64> = BTreeMap::new();
        for key in view {
            let entry = highest.entry(key.device).or_default();
            *entry = (*entry).max(key.counter);
        }
        if let Some(object) = self.state.objects.get(rid)
            && object.own_highest > 0
        {
            let entry = highest.entry(own).or_default();
            *entry = (*entry).max(object.own_highest);
        }
        let mut covers: Vec<Cover> = highest
            .into_iter()
            .map(|(device, counter)| Cover { device, counter })
            .collect();
        covers.sort_by_key(|c| c.device);
        covers.truncate(sync_protocol::records::MAX_COVERS);
        covers
    }

    /// The live state operations on `rid`, in log order.
    fn live_states(&self, rid: &Id) -> Vec<OpKey> {
        let domain = self.state.objects.get(rid).and_then(|o| o.domain);
        self.log_of(domain)
            .live
            .live(rid)
            .into_iter()
            .filter(|(_, class)| *class == OpClass::State)
            .map(|(op, _)| OpKey {
                device: op.device,
                counter: op.counter,
            })
            .collect()
    }

    fn sign_file(&mut self, rid: Id, found: &Found, covers: Vec<Cover>) -> Result<(), SyncError> {
        self.sign_file_from(rid, found, covers, None)
    }

    /// [`Self::sign_file`], with the `origin` of a move between domains.
    fn sign_file_from(
        &mut self,
        rid: Id,
        found: &Found,
        covers: Vec<Cover>,
        origin: Option<sync_protocol::records::Origin>,
    ) -> Result<(), SyncError> {
        // A file relocated under `<root> (shared)` keeps its synced path
        // (§9).
        let existing = self
            .state
            .objects
            .get(&rid)
            .is_some_and(|o| o.base.is_some());
        let synced = if existing {
            synced_path(&found.file.relative_path, &self.state.roots)
        } else {
            found.file.relative_path.clone()
        };
        let path = SyncPath::new(synced.clone())
            .map_err(|e| SyncError::Invalid(format!("path: {}", e.name())))?;
        let size = found
            .bytes
            .as_ref()
            .map_or(found.file.size, |bytes| bytes.len() as u64);
        let (content, blob, upload) =
            if capture::fits_inline(&found.file.relative_path, &found.object_id, size) {
                let bytes = match &found.bytes {
                    Some(bytes) => bytes.clone(),
                    None => self
                        .engine
                        .read_synced(&found.file.relative_path)
                        .map_err(core)?
                        .ok_or_else(|| SyncError::Invalid("the file went away".into()))?,
                };
                (
                    Content::Inline {
                        bytes: Bytes(bytes),
                    },
                    None,
                    None,
                )
            } else {
                let (blob, upload) = self.blob_for(rid, found)?;
                (Content::Blob(blob.clone()), Some(blob), upload)
            };
        let payload = Payload {
            rid,
            kind: PayloadKind::File {
                object_id: Text::new(found.object_id.clone())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                path,
                revision: found.revision,
                content,
                origin,
            },
        };
        let carried = Carried {
            path: synced,
            revision: found.revision,
            delete: false,
            object_id: found.object_id.clone(),
            kept: None,
            blob,
            text: None,
        };
        let key = self.sign(rid, false, covers, &payload, carried)?;
        if let Some(outgoing) = self.state.outbox.last_mut() {
            outgoing.upload = upload;
        }
        let object = self.state.objects.get_mut(&rid).expect("signed for it");
        object.local = Some(found.local());
        object.object_id = found.object_id.clone();
        object.base = Some(key);
        object.cover_all = false;
        Ok(())
    }

    /// The blob for `found`'s bytes (§21). A state of the object that
    /// already carries those bytes in a blob lends its `blob_ref`, so a move
    /// inside the domain uploads nothing. Otherwise the bytes go into a new
    /// blob under a fresh key, encrypted as they stream from the file, or
    /// from memory when the walk read them, into the journal, which keeps
    /// the ciphertext until the upload completes; the second value is that
    /// pending upload. A file whose bytes no longer hash to `found`'s
    /// revision fails, and the next round captures it again.
    fn blob_for(
        &self,
        rid: Id,
        found: &Found,
    ) -> Result<
        (
            sync_protocol::records::BlobRef,
            Option<sync_protocol::records::BlobRef>,
        ),
        SyncError,
    > {
        let revision = found.revision;
        if let Some(object) = self.state.objects.get(&rid)
            && let Some(blob) = object
                .states
                .values()
                .filter_map(|state| state.content.as_ref())
                .find(|content| content.revision == revision && content.blob.is_some())
                .and_then(|content| content.blob.clone())
        {
            return Ok((blob, None));
        }
        let blob = random(Id::random())?;
        let key = random(SecretKey::random())?;
        let encrypt = |out: &mut dyn std::io::Write| -> Result<_, SyncError> {
            let result = match &found.bytes {
                Some(bytes) => sync_protocol::blob::encrypt_blob_to(
                    &bytes[..],
                    bytes.len() as u64,
                    blob,
                    key,
                    out,
                ),
                None => {
                    let file = self
                        .engine
                        .open_synced(&found.file.relative_path)
                        .map_err(core)?
                        .ok_or_else(|| SyncError::Invalid("the file went away".into()))?;
                    let len = file
                        .metadata()
                        .map_err(|e| SyncError::Invalid(format!("can't read the file: {e}")))?
                        .len();
                    sync_protocol::blob::encrypt_blob_to(
                        std::io::BufReader::with_capacity(1 << 16, file),
                        len,
                        blob,
                        key,
                        out,
                    )
                }
            };
            let (blob_ref, file_hash) = result.map_err(|e| SyncError::Invalid(e.to_string()))?;
            if file_hash != revision {
                return Err(SyncError::Invalid(
                    "the file changed while sync read it".into(),
                ));
            }
            Ok(blob_ref)
        };
        let upload = state::upload_path(&blob);
        let blob_ref = match self.journal.keep_with(&upload, encrypt) {
            Ok(blob_ref) => blob_ref,
            Err(error) => {
                self.journal.forget(&upload);
                return Err(error);
            }
        };
        Ok((blob_ref.clone(), Some(blob_ref)))
    }

    /// Uploads each blob the outbox's records list that hasn't reached the
    /// server yet, and drops its ciphertext from the journal once the
    /// upload completes (§21).
    async fn upload_blobs(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let mut index = 0;
        while index < self.state.outbox.len() {
            let Some(blob) = self.state.outbox[index].upload.clone() else {
                index += 1;
                continue;
            };
            let path = self.journal.dir().join(state::upload_path(&blob.blob));
            let uploaded = service
                .client()
                .upload_blob(self.state.workspace, self.state.domain, &blob, &path)
                .await;
            match uploaded {
                Ok(()) => {
                    self.state.outbox[index].upload = None;
                    index += 1;
                }
                // The records behind it still go out: one of them may be
                // the delete that frees the space (§20).
                Err(SyncError::Refused { code, .. }) if code == "quota_exceeded" => {
                    self.wait_for_space(index)?;
                }
                Err(error) => return Err(error),
            }
            self.save()?;
            self.journal.forget(&state::upload_path(&blob.blob));
        }
        Ok(())
    }

    fn sign_delete(&mut self, rid: Id, covers: Vec<Cover>) -> Result<(), SyncError> {
        self.sign_removal(rid, covers, DeleteReason::Deleted, None)
    }

    /// Signs a delete of `rid`: an ordinary one, or the source half of a
    /// move, which names the `target`.
    fn sign_removal(
        &mut self,
        rid: Id,
        covers: Vec<Cover>,
        reason: DeleteReason,
        target: Option<sync_protocol::records::Origin>,
    ) -> Result<(), SyncError> {
        let object = &self.state.objects[&rid];
        let revision = object.local.as_ref().map_or(Digest::ZERO, |l| l.revision);
        let payload = Payload {
            rid,
            kind: PayloadKind::Delete {
                object_id: Text::new(object.object_id.clone())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                revision,
                reason,
                target,
            },
        };
        let carried = Carried {
            path: String::new(),
            revision,
            delete: true,
            object_id: object.object_id.clone(),
            kept: None,
            blob: None,
            text: None,
        };
        let key = self.sign(rid, true, covers, &payload, carried)?;
        let object = self.state.objects.get_mut(&rid).expect("signed for it");
        object.local = None;
        object.base = Some(key);
        Ok(())
    }

    /// Seals `payload` under the latest epoch and signs it into this
    /// device's chain at its next counter, raising the reservation first
    /// when the counter lies past it, and appends it to the outbox.
    fn sign(
        &mut self,
        rid: Id,
        delete: bool,
        covers: Vec<Cover>,
        payload: &Payload,
        carried: Carried,
    ) -> Result<OpKey, SyncError> {
        let counter = self.state.own.next;
        let device = self.credentials.keys.device();
        if self.mark.needs_block(&self.state.domain, counter) {
            self.mark.reserve(&self.state.domain, counter);
            self.mark
                .write(&*self.secrets, &device, &self.state.workspace)?;
        }
        let previous = self.state.own.head.map_or(Digest::ZERO, |(_, d)| d);
        let signed = self.seal(
            counter,
            previous,
            Digest::ZERO,
            rid,
            delete,
            covers.clone(),
            payload,
        )?;
        self.state.own.next = counter + 1;
        self.state.own.head = Some((counter, signed.digest()));
        let key = OpKey { device, counter };
        let epoch = signed.body.epoch;
        let policy_revision = signed.body.policy_revision;
        let object = self.state.objects.entry(rid).or_default();
        object.own_highest = counter;
        object.states.insert(
            key,
            StateInfo {
                digest: signed.digest(),
                seq: None,
                covers,
                content: Some(carried),
                epoch,
                policy_revision,
            },
        );
        self.state.outbox.push(Outgoing {
            record: signed,
            reseals: 0,
            rewrite: false,
            upload: None,
            pair: None,
        });
        Ok(key)
    }

    /// Seals and signs an operation at `counter` under the latest epoch and
    /// the accepted policy head.
    #[allow(clippy::too_many_arguments)]
    fn seal(
        &self,
        counter: u64,
        previous: Digest,
        previous_attempt: Digest,
        rid: Id,
        delete: bool,
        covers: Vec<Cover>,
        payload: &Payload,
    ) -> Result<Signed<Operation>, SyncError> {
        let epoch = self
            .state
            .epochs
            .last()
            .ok_or(SyncError::Check("sync_key_mismatch"))?;
        let key = self
            .keys
            .get(&epoch.body.epoch)
            .ok_or(SyncError::Check("sync_key_mismatch"))?;
        let head = self
            .state
            .policy
            .last()
            .ok_or(SyncError::Check("sync_workspace_unverified"))?;
        // The latest checkpoint this device reached (§13).
        let (checkpoint_seq, checkpoint) = self.state.log.checkpoints.named();
        let mut op = Operation {
            workspace: self.state.workspace,
            domain: self.state.domain,
            device: self.credentials.keys.device(),
            counter,
            previous,
            epoch: epoch.body.epoch,
            policy_revision: head.body.revision,
            policy_head: head.digest(),
            previous_attempt,
            generation: self.state.generation,
            checkpoint_seq,
            checkpoint,
            rid,
            delete,
            supersedes: Sorted::new(covers).map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            blobs: Sorted::new(listed_blobs(payload))
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        op.seal_payload(payload, key, &epoch.digest(), random(random_nonce())?)
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
        Ok(Signed::sign(op, self.credentials.keys.signing()))
    }

    /// Re-seals the outbox's records from `index` on at their own
    /// positions, under the latest epoch and policy head, each naming the
    /// attempt it replaces in `previous_attempt`; the later records' links
    /// change with it (§14). A re-seal that a newer epoch or policy change
    /// doesn't force counts toward the cap of 3 at its position.
    fn reseal_from(&mut self, index: usize, forced: bool) -> Result<(), SyncError> {
        self.reseal(index, forced, None)
    }

    /// [`Self::reseal_from`], with `replacing` putting the first record at
    /// the position a rejected record held, naming that record (§14).
    fn reseal(
        &mut self,
        index: usize,
        forced: bool,
        replacing: Option<(u64, Digest)>,
    ) -> Result<(), SyncError> {
        self.reseal_covering(index, forced, replacing, None)
    }

    /// [`Self::reseal`], with `covers` taking the place of the first
    /// record's.
    fn reseal_covering(
        &mut self,
        index: usize,
        forced: bool,
        replacing: Option<(u64, Digest)>,
        covers: Option<Vec<Cover>>,
    ) -> Result<(), SyncError> {
        let own = self.credentials.keys.device();
        let mut previous = if index == 0 {
            self.state.own.acked.map_or(Digest::ZERO, |(_, d, _)| d)
        } else {
            self.state.outbox[index - 1].record.digest()
        };
        for i in index..self.state.outbox.len() {
            let old = self.state.outbox[i].record.clone();
            let key = self
                .keys
                .get(&old.body.epoch)
                .ok_or(SyncError::Check("sync_key_mismatch"))?;
            let epoch_record = self
                .state
                .epochs
                .iter()
                .chain(self.state.ended_epochs.iter())
                .find(|e| e.body.epoch == old.body.epoch)
                .map(|e| e.digest())
                .ok_or(SyncError::Check("sync_key_mismatch"))?;
            // A record that covers nothing may be a new object's state or
            // a text update, and only its payload tells which.
            let payload = old
                .body
                .open_payload(key, &epoch_record, OpClass::State)
                .or_else(|_| old.body.open_payload(key, &epoch_record, OpClass::Patch))
                .map_err(|_| SyncError::Check("sync_record_unreadable"))?;
            if i == index && !forced {
                self.state.outbox[i].reseals += 1;
                if self.state.outbox[i].reseals > RESEAL_LIMIT {
                    self.stopped = Some("sync_reseal_limit");
                    return Err(SyncError::Check("sync_reseal_limit"));
                }
            }
            let (counter, attempt) = match replacing {
                Some((counter, rejected)) if i == index => (counter, rejected),
                _ => (old.body.counter, old.digest()),
            };
            let resealed = self.seal(
                counter,
                previous,
                attempt,
                old.body.rid,
                old.body.delete,
                match &covers {
                    Some(covers) if i == index => covers.clone(),
                    _ => old.body.supersedes.to_vec(),
                },
                &payload,
            )?;
            previous = resealed.digest();
            let old_key = OpKey {
                device: own,
                counter: old.body.counter,
            };
            let new_key = OpKey {
                device: own,
                counter,
            };
            if let Some(object) = self.state.objects.get_mut(&old.body.rid) {
                if let Some(mut state) = object.states.remove(&old_key) {
                    state.digest = resealed.digest();
                    state.epoch = resealed.body.epoch;
                    object.states.insert(new_key, state);
                }
                if object.base == Some(old_key) {
                    object.base = Some(new_key);
                }
            }
            self.rekey_text(&old.body.rid, old_key, new_key)?;
            self.state.outbox[i].record = resealed;
            self.recount_own(&old.body.rid);
        }
        if let Some(last) = self.state.outbox.last() {
            self.state.own.head = Some((last.record.body.counter, last.record.digest()));
        }
        Ok(())
    }

    /// Sets `rid`'s highest own counter from the states the journal keeps,
    /// after records of this device left or moved.
    fn recount_own(&mut self, rid: &Id) {
        let own = self.credentials.keys.device();
        if let Some(object) = self.state.objects.get_mut(rid) {
            object.own_highest = object
                .states
                .keys()
                .filter(|k| k.device == own)
                .map(|k| k.counter)
                .max()
                .unwrap_or(0);
        }
    }

    /// Pushes every domain's outbox, with each move between domains in one
    /// push. Returns how many records the server acknowledged.
    async fn push_everything(&mut self, service: &mut SyncService) -> Result<usize, SyncError> {
        let mut pushed = 0;
        loop {
            pushed += self.push(service).await?;
            for domain in self.loaded_domains() {
                self.swap_domain(domain);
                let result = self.push(service).await;
                self.swap_domain(domain);
                pushed += result?;
            }
            let moved = self.push_pairs(service).await?;
            pushed += moved;
            if moved == 0 {
                return Ok(pushed);
            }
        }
    }

    /// Pushes the outbox in order, one push at a time, each at most 1 MiB,
    /// until it's empty or the server refuses a record that waits (§15).
    /// The same bytes go out on every retry. Returns how many records the
    /// server acknowledged.
    pub async fn push(&mut self, service: &mut SyncService) -> Result<usize, SyncError> {
        if service.client().restore_generation() > self.state.generation {
            return Err(SyncError::Check("sync_restore_generation"));
        }
        let mut acknowledged = 0;
        let mut attempts = 0;
        if self.state.removal_hold {
            return Ok(0);
        }
        // Records sealed under an epoch that a rotation since replaced go
        // out re-sealed under the current one (§9).
        let latest = self.state.epochs.last().map_or(0, |e| e.body.epoch);
        if let Some(index) = self
            .state
            .outbox
            .iter()
            .position(|o| o.record.body.epoch != latest)
        {
            self.reseal_from(index, true)?;
            self.save()?;
        }
        self.upload_blobs(service).await?;
        // A move's records leave in one push across domains (§9).
        while self.state.outbox.first().is_some_and(|o| o.pair.is_none()) {
            // At the re-seal cap the device signs nothing more there, but
            // retries the last attempt's bytes (§14).
            let at_cap = self.stopped == Some("sync_reseal_limit");
            if self.stopped.is_some() && !at_cap {
                return Ok(acknowledged);
            }
            attempts += 1;
            if attempts > 64 {
                return Err(SyncError::Network(
                    "the server keeps refusing the push".into(),
                ));
            }
            let mut size = 0;
            let mut batch = Vec::new();
            for outgoing in &self.state.outbox {
                let record = RecordRef::Operation(outgoing.record.clone());
                let len = record.to_bytes().len();
                if outgoing.pair.is_some()
                    || (!batch.is_empty() && size + len > MAX_BODY - PUSH_MARGIN)
                {
                    break;
                }
                size += len;
                batch.push(record);
            }
            let sent = batch.len();
            match service.client().push(self.state.workspace, batch).await {
                Ok(response) => {
                    let outgoing: Vec<Outgoing> = self.state.outbox.drain(..sent).collect();
                    for (outgoing, receipt) in outgoing.into_iter().zip(response.receipts.0.iter())
                    {
                        self.acknowledge(outgoing, receipt)?;
                    }
                    acknowledged += sent;
                    if at_cap {
                        self.stopped = None;
                    }
                    self.save()?;
                    let accepted = self.state.policy.last().map_or(0, |c| c.body.revision);
                    if response.policy_revision > accepted {
                        self.refresh(service).await?;
                    }
                }
                Err(SyncError::Refused { .. }) if at_cap => {
                    return Err(SyncError::Check("sync_reseal_limit"));
                }
                Err(SyncError::Refused { code, detail, .. }) => {
                    let handled = self.refused(service, &code, detail.as_deref()).await;
                    self.save()?;
                    handled?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(acknowledged)
    }

    /// Records the server's receipt for a record that left the outbox.
    fn acknowledge(&mut self, outgoing: Outgoing, receipt: &Receipt) -> Result<(), SyncError> {
        let record = &outgoing.record;
        if receipt.record_digest != record.digest() {
            return Err(SyncError::Network(
                "the push returned another record's receipt".into(),
            ));
        }
        self.state.own.acked = Some((record.body.counter, record.digest(), receipt.seq));
        self.state.waiting_for_space.remove(&record.body.rid);
        let key = OpKey {
            device: record.body.device,
            counter: record.body.counter,
        };
        if let Some(state) = self
            .state
            .objects
            .get_mut(&record.body.rid)
            .and_then(|o| o.states.get_mut(&key))
        {
            state.seq = Some(receipt.seq);
        }
        Ok(())
    }

    /// Handles a refused push (§14): checks the reason against what the
    /// device can verify, then re-seals, replaces, drops or keeps the
    /// failing record. Returns an error when the record waits.
    async fn refused(
        &mut self,
        service: &mut SyncService,
        code: &str,
        detail: Option<&[u8]>,
    ) -> Result<(), SyncError> {
        let failure = detail.and_then(|d| PushFailureDetail::from_bytes(d).ok());
        let (index, record_code, record_detail) = match &failure {
            Some(f) => (
                f.record_index as usize,
                f.code.0.as_str(),
                f.record_detail.as_ref().map(|d| d.0.clone()),
            ),
            None => (0, code, None),
        };
        self.refused_at(service, index, record_code, record_detail)
            .await
    }

    /// [`Self::refused`], for the record at `index` of the outbox.
    async fn refused_at(
        &mut self,
        service: &mut SyncService,
        index: usize,
        record_code: &str,
        record_detail: Option<Vec<u8>>,
    ) -> Result<(), SyncError> {
        let code = record_code;
        if index >= self.state.outbox.len() {
            return Err(SyncError::Refused {
                code: code.to_owned(),
                status: 0,
                detail: None,
            });
        }
        let refused = || SyncError::Refused {
            code: record_code.to_owned(),
            status: 409,
            detail: None,
        };
        match record_code {
            "epoch_stale" | "rotation_pending" => {
                let before = self.accepted_epoch();
                let revision = self.accepted_revision();
                // An envelope that waits for its epoch's entry holds up no
                // rotation: the device took the policy, and it rotates from
                // the key it accepted.
                match self.refresh(service).await {
                    Err(SyncError::KeyMissing)
                        if self
                            .remote
                            .as_ref()
                            .is_some_and(|remote| service.rotation_pending(remote)) => {}
                    refreshed => refreshed?,
                }
                if let Some(remote) = self.remote.as_mut()
                    && service.rotation_pending(remote)
                {
                    service.rotate_if_pending(remote).await?;
                    let remote = self.remote.take().expect("just used");
                    self.adopt(remote)?;
                }
                if self.accepted_epoch() != before || self.accepted_revision() != revision {
                    self.reseal_from(index, true)
                } else {
                    Err(refused())
                }
            }
            "supersedes_unknown" if self.state.outbox[index].rewrite => {
                // A rewrite keeps its cover or writes nothing; the version
                // waits for review (§9).
                let rejected = self.state.outbox.remove(index);
                self.forget_own(&rejected.record);
                let rid = rejected.record.body.rid;
                if let Some(local) = self.state.objects.get(&rid).and_then(|o| o.local.clone()) {
                    self.state.held.insert(
                        rid,
                        Held {
                            path: local.path,
                            revision: local.revision,
                        },
                    );
                }
                self.reseal_replacing_or_end(index, &rejected.record)
            }
            "supersedes_unknown" => {
                // Drop the covers the server can't return (§14).
                let record = self.state.outbox[index].record.clone();
                let mut kept = Vec::new();
                for cover in record.body.supersedes.iter() {
                    // Pruning takes an operation a writer's view still
                    // holds, and the cover goes with it (§19).
                    let found = match service
                        .client()
                        .chain_position(
                            self.state.workspace,
                            self.state.domain,
                            cover.device,
                            cover.counter,
                        )
                        .await
                    {
                        Err(SyncError::Refused { code, .. }) if code == "pruned" => None,
                        found => found?,
                    };
                    let exists = found.is_some_and(|entry| match entry.record {
                        EntryRecord::Operation(op) => {
                            op.body.counter == cover.counter && op.body.rid == record.body.rid
                        }
                        _ => false,
                    }) || self.state.outbox[..index].iter().any(|o| {
                        o.record.body.device == cover.device
                            && o.record.body.counter == cover.counter
                    });
                    if exists {
                        kept.push(cover.clone());
                    }
                }
                if kept.len() == record.body.supersedes.len() {
                    return Err(refused());
                }
                if kept.is_empty() {
                    // With no cover left it would class as a patch. The
                    // change stays local, and the next capture covers what
                    // the device pulls by then.
                    let rejected = self.state.outbox.remove(index);
                    self.forget_own(&rejected.record);
                    self.keep_local(&rejected.record, false);
                    return self.reseal_replacing_or_end(index, &rejected.record);
                }
                self.recover(index, kept)
            }
            "policy_regressed" => {
                self.refresh(service).await?;
                self.reseal_from(index, false)
            }
            "policy_stale" => {
                // Signed under a revision at which the device couldn't
                // write, such as a viewer's edit before the change that
                // made it an editor: sealed again under the policy it
                // accepts now, if that one moved on (§13).
                let revision = self.accepted_revision();
                self.refresh(service).await?;
                if self.accepted_revision() == revision {
                    return Err(refused());
                }
                self.reseal_from(index, false)
            }
            "chain_conflict" => {
                let head = record_detail
                    .as_deref()
                    .and_then(|d| ChainConflictDetail::from_bytes(d).ok());
                let acked = self.state.own.acked.map_or(0, |(c, _, _)| c);
                match head {
                    Some(head) if head.counter > acked => {
                        self.compare_own_chain(service).await?;
                        Ok(())
                    }
                    Some(head) if head.counter < acked => {
                        Err(SyncError::Check("sync_server_rollback"))
                    }
                    _ => Err(refused()),
                }
            }
            "snapshot_required" | "quota_exceeded"
                if self.is_text_update(&self.state.outbox[index].record) =>
            {
                // The document holds the patch, so a snapshot takes its
                // place and carries it (§14). The journal gives the patch
                // up only then, as a closed note rebuilds from the journal.
                let rejected = self.state.outbox.remove(index);
                self.forget_own(&rejected.record);
                self.snapshot_required(rejected.record.body.rid)?;
                self.forget_text_record(&rejected.record)?;
                self.reseal_replacing(index, &rejected.record)
            }
            "quota_exceeded" => self.wait_for_space(index),
            "conflict_limit" | "snapshot_required" => {
                // The change stays local; the next record takes its place.
                if index + 1 >= self.state.outbox.len() {
                    return Err(refused());
                }
                let rejected = self.state.outbox.remove(index);
                self.forget_own(&rejected.record);
                self.keep_local(&rejected.record, record_code == "conflict_limit");
                self.reseal_replacing(index, &rejected.record)
            }
            "forbidden" => {
                // The device's write access is gone: drop what waits; the
                // files keep their changes (§14).
                let dropped: Vec<_> = self.state.outbox.drain(index..).collect();
                for outgoing in &dropped {
                    self.forget_own(&outgoing.record);
                }
                self.state.own.head = self
                    .state
                    .outbox
                    .last()
                    .map(|o| (o.record.body.counter, o.record.digest()))
                    .or(self.state.own.acked.map(|(c, d, _)| (c, d)));
                Ok(())
            }
            _ => Err(refused()),
        }
    }

    fn accepted_epoch(&self) -> Option<Digest> {
        self.state.epochs.last().map(|e| e.digest())
    }

    fn accepted_revision(&self) -> u64 {
        // Each domain's verified policy follows the journal's on its own
        // time, and a page is judged against that one.
        match &self.remote {
            Some(remote) => remote.policy.head().0,
            None => self.state.policy.last().map_or(0, |c| c.body.revision),
        }
    }

    /// Re-seals the record at `index` with other covers, then the later
    /// ones, which counts toward the cap.
    fn recover(&mut self, index: usize, covers: Vec<Cover>) -> Result<(), SyncError> {
        self.reseal_covering(index, false, None, Some(covers))
    }

    /// Puts the outbox's record at `index`, the one after a rejected
    /// record, at the rejected record's position, naming it in
    /// `previous_attempt`, then re-seals the later ones (§14). The
    /// replacement doesn't count toward the cap.
    fn reseal_replacing(
        &mut self,
        index: usize,
        rejected: &Signed<Operation>,
    ) -> Result<(), SyncError> {
        self.reseal(
            index,
            true,
            Some((rejected.body.counter, rejected.digest())),
        )
    }

    /// Keeps the change a refused record carried in its file, so the next
    /// capture signs it again: after `conflict_limit` as a resolution that
    /// covers every live branch (§14).
    fn keep_local(&mut self, record: &Signed<Operation>, cover_all: bool) {
        let rid = record.body.rid;
        let live = self.live_states(&rid);
        let Some(object) = self.state.objects.get_mut(&rid) else {
            return;
        };
        if let Some(local) = object.local.as_mut() {
            local.revision = Digest::ZERO;
            local.size = u64::MAX;
        }
        if object.base.is_none() {
            object.base = live
                .into_iter()
                .max_by_key(|key| object.states.get(key).and_then(|s| s.seq).unwrap_or(0));
        }
        object.cover_all |= cover_all;
    }

    /// The server has no space for the outbox's record at `index`: the
    /// change stays local, the next capture signs it again, and the next
    /// record takes its place (§20).
    fn wait_for_space(&mut self, index: usize) -> Result<(), SyncError> {
        let rejected = self.state.outbox.remove(index);
        self.forget_own(&rejected.record);
        self.hold_refused_snapshot(&rejected.record)?;
        self.keep_local(&rejected.record, false);
        self.state
            .waiting_for_space
            .insert(rejected.record.body.rid);
        self.usage_checked = None;
        self.reseal_replacing_or_end(index, &rejected.record)
    }

    /// Puts the next record, if any, at a dropped record's position.
    fn reseal_replacing_or_end(
        &mut self,
        index: usize,
        rejected: &Signed<Operation>,
    ) -> Result<(), SyncError> {
        if index < self.state.outbox.len() {
            return self.reseal_replacing(index, rejected);
        }
        self.state.own.head = self
            .state
            .outbox
            .last()
            .map(|o| (o.record.body.counter, o.record.digest()))
            .or(self.state.own.acked.map(|(c, d, _)| (c, d)));
        Ok(())
    }

    /// Writes again, under the current epoch, each object where a record
    /// under an epoch a replacement skipped is live (§9): this device's own
    /// records at once, and other devices' records it can read once the
    /// random wait passes, skipping any that a rewrite covered meanwhile.
    /// Each is a state operation covering that record and this device's own
    /// highest live operation on the object, so the server neither counts
    /// it as a patch nor answers `supersedes_incomplete`.
    fn rewrite_skipped(&mut self) -> Result<usize, SyncError> {
        let mut written = 0;
        if !self.state.rewrite_epochs.is_empty() {
            let epochs: BTreeSet<u64> = self.state.rewrite_epochs.drain(..).collect();
            written += self.rewrite_text(&epochs)?;
            written += self.rewrite_under(&epochs, true)?;
        }
        if !self.state.rewrite_others.is_empty() && unix_now() >= self.state.rewrite_due {
            let epochs: BTreeSet<u64> = self.state.rewrite_others.drain(..).collect();
            written += self.rewrite_under(&epochs, false)?;
        }
        Ok(written)
    }

    /// Sets the longest random wait before the device signs a checkpoint
    /// that fell due; [`CHECKPOINT_WAIT`] by default.
    pub fn set_checkpoint_wait(&mut self, wait: std::time::Duration) {
        self.checkpoint_wait = wait;
        let latest = unix_now() + wait.as_secs();
        let frames = self.state.restricted.values_mut().map(|f| &mut f.log);
        for log in std::iter::once(&mut self.state.log).chain(frames) {
            let checkpoints = &mut log.checkpoints;
            if checkpoints.sign_at.is_some_and(|at| at > latest) {
                checkpoints.sign_at = Some(latest);
            }
        }
    }

    /// The latest checkpoint this device accepted in the domain's log.
    pub fn latest_checkpoint(&self) -> Option<&Signed<sync_protocol::records::Checkpoint>> {
        self.state
            .log
            .checkpoints
            .accepted
            .as_ref()
            .map(|a| &a.record)
    }

    /// The checkpoints in the log that failed a check, as reported (§17).
    pub fn checkpoint_failures(&self) -> &[crate::sync::checkpoints::Failure] {
        &self.state.log.checkpoints.failures
    }

    /// How many records the chain verifier keeps state for, which stays
    /// within one checkpoint interval (§14).
    pub fn verifier_retained(&self) -> usize {
        self.state.log.verifier.retained()
    }

    /// The devices whose forked chain holds the cursor (§14).
    pub fn forked_devices(&self) -> Vec<Id> {
        self.state.log.forked.keys().copied().collect()
    }

    /// Whether the chain verifier ignores `digest`, a record of `device` on
    /// the other branch of a settled fork (§14).
    pub fn ignores_record(&self, device: &Id, digest: &Digest) -> bool {
        self.state.log.verifier.ignores(device, digest)
    }

    /// Signs and pushes a checkpoint once the device has applied the log
    /// past the due point the server published and its random wait passed
    /// with none arriving (§17). A device that can't write the domain, or
    /// that an unrecorded removal holds (§9), signs none. A checkpoint the
    /// server answers with `checkpoint_stale` or `checkpoint_not_due` is
    /// dropped, and the device waits for the next due point. Returns
    /// whether one committed.
    async fn sign_checkpoint(&mut self, service: &mut SyncService) -> Result<bool, SyncError> {
        let device = self.credentials.keys.device();
        let cursor = self.state.log.cursor;
        let due = self.state.log.checkpoints.due;
        let Some(remote) = self.remote.as_ref() else {
            return Ok(false);
        };
        let writes = remote
            .policy
            .head_snapshot()
            .can_write(&device, &self.state.domain);
        let pending = self.state.log.judge.rotation_pending(&remote.policy);
        if due == 0
            || cursor < due
            || !writes
            || self.state.removal_hold
            || self.stopped.is_some()
            || self.state.ended.is_some()
            || pending
        {
            self.state.log.checkpoints.sign_at = None;
            return Ok(false);
        }
        let now = unix_now();
        let sign_at = match self.state.log.checkpoints.sign_at {
            Some(at) => at,
            None => {
                let at = now
                    + super::heads::random_wait(&(std::time::Duration::ZERO..self.checkpoint_wait))
                        .as_secs();
                self.state.log.checkpoints.sign_at = Some(at);
                self.save()?;
                at
            }
        };
        if now < sign_at {
            return Ok(false);
        }
        let Some(remote) = self.remote.as_ref() else {
            return Ok(false);
        };
        let Some(bundle) = self.state.log.checkpoints.sign(
            self.state.workspace,
            self.state.domain,
            cursor,
            self.state.log.retention_floor,
            &self.state.log.live,
            &remote.policy,
            device,
            self.credentials.keys.signing(),
        ) else {
            self.state.log.checkpoints.sign_at = None;
            return Ok(false);
        };
        let pushed = service
            .client()
            .push(
                self.state.workspace,
                vec![RecordRef::CheckpointBundle(bundle)],
            )
            .await;
        self.state.log.checkpoints.sign_at = None;
        let committed = match pushed {
            Ok(_) => true,
            Err(SyncError::Refused { code, .. })
                if code == "checkpoint_stale" || code == "checkpoint_not_due" =>
            {
                false
            }
            Err(error) => return Err(error),
        };
        // The next page names the next due point.
        self.state.log.checkpoints.due = 0;
        self.save()?;
        Ok(committed)
    }

    /// Sets the longest random wait before the device writes again other
    /// devices' records under a skipped epoch; [`REWRITE_WAIT`] by default.
    pub fn set_rewrite_wait(&mut self, wait: std::time::Duration) {
        self.rewrite_wait = wait;
        if self.state.rewrite_due > unix_now() + wait.as_secs() {
            self.state.rewrite_due = unix_now() + wait.as_secs();
        }
    }

    /// Writes again each object with a live record under `epochs`, this
    /// device's own when `own`, else another device's that opened here.
    fn rewrite_under(
        &mut self,
        epochs: &BTreeSet<u64>,
        own_records: bool,
    ) -> Result<usize, SyncError> {
        let own = self.credentials.keys.device();
        let rids: Vec<Id> = self.state.objects.keys().copied().collect();
        let mut written = 0;
        for rid in rids {
            let object = &self.state.objects[&rid];
            if object.domain != self.active || object.text.is_some() {
                continue;
            }
            let live: BTreeSet<OpKey> = self.live_states(&rid).into_iter().collect();
            let Some((key, content)) = live.iter().find_map(|key| {
                let info = object.states.get(key)?;
                ((key.device == own) == own_records && epochs.contains(&info.epoch))
                    .then(|| info.content.clone().map(|c| (*key, c)))?
            }) else {
                continue;
            };
            if !own_records && self.state.held.contains_key(&rid) {
                continue;
            }
            let mut view = BTreeSet::from([key]);
            view.extend(live.iter().filter(|k| k.device == own).copied());
            let covers = self.covers_of(&rid, &view);
            let before = self.state.outbox.len();
            if content.delete {
                self.sign_delete(rid, covers)?;
            } else {
                let body = self.content_body(&rid, &key, &BTreeMap::new())?;
                let path = self.state.objects[&rid]
                    .local
                    .as_ref()
                    .map_or(content.path.clone(), |l| l.path.clone());
                let found = match self.found_on_disk(&path) {
                    Ok(found) if found.revision == content.revision => found,
                    _ => Found {
                        file: local_core::SyncedFile {
                            relative_path: content.path.clone(),
                            size: body.len(),
                            modified_ns: 0,
                            file_id: None,
                        },
                        revision: content.revision,
                        object_id: content.object_id.clone(),
                        bytes: body.bytes().map(<[u8]>::to_vec),
                    },
                };
                self.sign_file(rid, &found, covers)?;
            }
            for outgoing in &mut self.state.outbox[before..] {
                outgoing.rewrite = true;
            }
            written += 1;
        }
        Ok(written)
    }

    /// Forgets a record of this device that never reached the server.
    fn forget_own(&mut self, record: &Signed<Operation>) {
        let key = OpKey {
            device: record.body.device,
            counter: record.body.counter,
        };
        if let Some(object) = self.state.objects.get_mut(&record.body.rid) {
            object.states.remove(&key);
            if object.base == Some(key) {
                object.base = None;
            }
        }
        self.recount_own(&record.body.rid);
    }

    /// Pulls the domain once (§15): checks the page against the cursor,
    /// fetches the policy and keys first when the page names a revision or
    /// an epoch the device hasn't accepted, checks every record before it
    /// decrypts it, applies the changes, and only then advances the cursor.
    /// A failed check applies nothing and keeps the cursor. A record under
    /// an epoch whose key the device doesn't hold yet holds the cursor
    /// without counting it as unreadable (§13). Returns how many other
    /// devices' records it applied, and whether it waits for a key.
    pub async fn pull(
        &mut self,
        service: &mut SyncService,
        wait_seconds: u32,
    ) -> Result<(usize, bool), SyncError> {
        self.pull_until(service, wait_seconds, &tokio::sync::Notify::new())
            .await
    }

    async fn pull_until(
        &mut self,
        service: &mut SyncService,
        wait_seconds: u32,
        wake: &tokio::sync::Notify,
    ) -> Result<(usize, bool), SyncError> {
        if self.active.is_none() {
            self.sync_domains(service).await?;
            if !self.loaded_domains().is_empty() {
                return self.pull_every_domain(service, wait_seconds, wake).await;
            }
        }
        if self.state.bootstrap {
            self.bootstrap(service).await?;
            self.state.bootstrap = false;
            self.save()?;
        }
        // An entry the device learned of before this request is owed by
        // the head of the page it answers; one it learns of while handling
        // the page, by the next page's (§8, §9).
        self.armed = self
            .state
            .log
            .wanted
            .iter()
            .map(|w| (w.epoch, w.number))
            .collect();
        let cursors = || {
            vec![PullCursor {
                domain: self.state.domain,
                after: self.state.log.cursor,
            }]
        };
        let workspace = self.state.workspace;
        let answered = if wait_seconds == 0 {
            Some(service.client().pull(workspace, cursors(), 0).await?)
        } else {
            let request = service.client().pull(workspace, cursors(), wait_seconds);
            tokio::select! {
                response = request => Some(response?),
                () = wake.notified() => None,
            }
        };
        // Woken while it waited, the round still pulls once, without
        // waiting; dropping the waiting request changed nothing.
        let response = match answered {
            Some(response) => response,
            None => service.client().pull(workspace, cursors(), 0).await?,
        };
        let page = response
            .pages
            .0
            .into_iter()
            .find(|p| p.domain == self.state.domain)
            .ok_or(SyncError::Check("sync_chain_gap"))?;
        self.take_page(service, page).await
    }

    /// Pulls every domain the device reads in one request (§15). Restricted
    /// domains go first, so their roots stand before a workspace object
    /// beneath one finds its place.
    async fn pull_every_domain(
        &mut self,
        service: &mut SyncService,
        wait_seconds: u32,
        wake: &tokio::sync::Notify,
    ) -> Result<(usize, bool), SyncError> {
        let domains = self.loaded_domains();
        for domain in &domains {
            self.swap_domain(*domain);
            let loaded = if self.state.bootstrap {
                match self.bootstrap(service).await {
                    Ok(_) => {
                        self.state.bootstrap = false;
                        self.save()
                    }
                    Err(error) => Err(error),
                }
            } else {
                Ok(())
            };
            self.swap_domain(*domain);
            loaded?;
        }
        if self.state.bootstrap {
            self.bootstrap(service).await?;
            self.state.bootstrap = false;
            self.save()?;
        }
        let mut cursors = vec![PullCursor {
            domain: self.state.domain,
            after: self.state.log.cursor,
        }];
        self.armed = wanted_entries(&self.state.log);
        for domain in &domains {
            self.swap_domain(*domain);
            self.armed = wanted_entries(&self.state.log);
            cursors.push(PullCursor {
                domain: *domain,
                after: self.state.log.cursor,
            });
            self.swap_domain(*domain);
        }
        let workspace = self.state.workspace;
        let answered = if wait_seconds == 0 {
            Some(service.client().pull(workspace, cursors.clone(), 0).await?)
        } else {
            let request = service
                .client()
                .pull(workspace, cursors.clone(), wait_seconds);
            tokio::select! {
                response = request => Some(response?),
                () = wake.notified() => None,
            }
        };
        let response = match answered {
            Some(response) => response,
            None => service.client().pull(workspace, cursors, 0).await?,
        };
        let mut pages = response.pages.0;
        let (mut applied, mut waiting) = (0, false);
        for domain in &domains {
            let index = pages
                .iter()
                .position(|p| p.domain == *domain)
                .ok_or(SyncError::Check("sync_chain_gap"))?;
            let page = pages.swap_remove(index);
            self.swap_domain(*domain);
            let taken = self.take_page(service, page).await;
            self.swap_domain(*domain);
            let (count, waits) = taken?;
            applied += count;
            waiting |= waits;
        }
        let page = pages
            .into_iter()
            .find(|p| p.domain == self.state.domain)
            .ok_or(SyncError::Check("sync_chain_gap"))?;
        let (count, waits) = self.take_page(service, page).await?;
        Ok((applied + count, waiting || waits))
    }

    /// Handles a frame of the realtime channel (§23). A records frame that
    /// follows straight on from the cursor goes through every check a
    /// pulled page does, and the cursor moves to its last entry only after
    /// every file change reaches disk; a gap, a frame inside the cursor, a
    /// notify of a later head, a frame that fails a check, or a restore
    /// generation the device hasn't seen sends the device to pull. A frame
    /// the cursor already covers changes nothing. A policy or key hint
    /// fetches the policy and keys, so they arrive before the next push,
    /// and an account chain hint reads that chain.
    pub async fn on_frame(
        &mut self,
        service: &mut SyncService,
        frame: &Frame,
    ) -> Result<FrameOutcome, SyncError> {
        if self.state.ended.is_some() || self.paused {
            return Ok(FrameOutcome::default());
        }
        // A restricted domain's frames take the same path in that domain.
        if self.active.is_none()
            && let Frame::Records { domain, .. } | Frame::Notify { domain, .. } = frame
            && self.parked.get(domain).is_some_and(|r| r.remote.is_some())
        {
            let domain = *domain;
            self.swap_domain(domain);
            let outcome = Box::pin(self.on_frame(service, frame)).await;
            self.swap_domain(domain);
            return outcome;
        }
        let current = self.frame_generation == Some(self.state.generation);
        match frame {
            Frame::Restored { restore_generation } => {
                self.frame_generation = Some(*restore_generation);
                if *restore_generation == self.state.generation {
                    return Ok(FrameOutcome::default());
                }
                // The generation's verdict comes from the pull, whose
                // pages carry it as the server's claim (§14).
                self.pull_after_frame(service).await
            }
            Frame::Records {
                domain,
                previous_seq,
                checkpoint_due,
                entries,
            } if *domain == self.state.domain => {
                let (Some(first), Some(last)) = (entries.first(), entries.last()) else {
                    return self.pull_after_frame(service).await;
                };
                let cursor = self.state.log.cursor;
                let action = records_frame_action(cursor, *previous_seq, first.seq, last.seq);
                match action {
                    FrameAction::Ignore if current => Ok(FrameOutcome::default()),
                    FrameAction::Apply
                        if current && records_frame_is_well_formed(*previous_seq, entries) =>
                    {
                        let page = Page {
                            domain: *domain,
                            status: PageStatus::Ok,
                            after: cursor,
                            complete_through: last.seq,
                            // The frame says nothing of later entries; the
                            // head the device saw last still stands.
                            head: last.seq.max(self.state.log.head),
                            retention_floor: self.state.log.retention_floor,
                            checkpoint_seq: self.state.log.checkpoint_seq,
                            restore_generation: self.state.generation,
                            policy_revision: self.accepted_revision(),
                            checkpoint_due: *checkpoint_due,
                            entries: entries.clone(),
                        };
                        // A frame names no head, so it bounds none of the
                        // entries the log waits for: the next pull's page
                        // does (§8, §9).
                        self.armed = BTreeSet::new();
                        match self.take_page(service, page).await {
                            Ok((applied, false)) => Ok(FrameOutcome {
                                applied,
                                pulled: false,
                            }),
                            // A frame that fails a check, or whose key
                            // hasn't arrived, counts like such a page: the
                            // cursor stays and the device pulls.
                            Ok((_, true)) | Err(_) => self.pull_after_frame(service).await,
                        }
                    }
                    _ => self.pull_after_frame(service).await,
                }
            }
            Frame::Notify { domain, head, .. }
                if *domain == self.state.domain && *head > self.state.log.cursor =>
            {
                self.pull_after_frame(service).await
            }
            Frame::PolicyChanged { revision } if *revision > self.accepted_revision() => {
                self.refresh(service).await?;
                Ok(FrameOutcome::default())
            }
            Frame::KeysChanged => {
                match self.refresh(service).await {
                    // An envelope still on its way arrives with a later
                    // hint or pull.
                    Ok(()) | Err(SyncError::KeyMissing) => {}
                    Err(error) => return Err(error),
                }
                Ok(FrameOutcome::default())
            }
            Frame::AccountChanged {
                account, genesis, ..
            } => {
                service.follow_account(*account, *genesis).await?;
                Ok(FrameOutcome::default())
            }
            Frame::Presence { record } => {
                self.take_presence(record, std::time::Instant::now());
                Ok(FrameOutcome::default())
            }
            Frame::PresenceGone { device, session } => {
                self.presence_gone(*device, *session);
                Ok(FrameOutcome::default())
            }
            Frame::Error {
                rejected_type,
                code,
                ..
            } => {
                tracing::warn!(code = %code.as_str(), "the realtime channel refused a frame");
                if *rejected_type == 2 {
                    self.presence_refused(service, code.as_str()).await?;
                }
                Ok(FrameOutcome::default())
            }
            _ => Ok(FrameOutcome::default()),
        }
    }

    /// A pull without waiting, in place of a frame.
    async fn pull_after_frame(
        &mut self,
        service: &mut SyncService,
    ) -> Result<FrameOutcome, SyncError> {
        let (applied, _) = self.pull(service, 0).await?;
        Ok(FrameOutcome {
            applied,
            pulled: true,
        })
    }

    /// Checks a page of the workspace domain, verifies its records and
    /// applies them, then advances the cursor once every file change
    /// reached disk (§15).
    async fn take_page(
        &mut self,
        service: &mut SyncService,
        page: Page,
    ) -> Result<(usize, bool), SyncError> {
        self.check_page(&page)?;
        match page.status {
            PageStatus::Ok => {}
            PageStatus::Forbidden | PageStatus::NotFound if self.active.is_some() => {
                // Losing a restricted domain ends only that domain: its
                // files stay, and the next round no longer pulls it.
                return self.refresh_policy(service).await.map(|()| (0, false));
            }
            PageStatus::Forbidden | PageStatus::NotFound => {
                // Once a change it accepted took its access or deleted the
                // domain, the device stops pulling it (§15).
                if self.refresh_policy(service).await.is_ok()
                    && let Some(ended) = self.access_ended()
                {
                    self.state.ended = Some(ended);
                    self.save()?;
                    return Ok((0, false));
                }
                self.refresh(service).await?;
                return Err(SyncError::Refused {
                    code: format!("{:?}", page.status).to_lowercase(),
                    status: 200,
                    detail: None,
                });
            }
            // A member or grantee back after a removal holds a cursor below
            // the floor its new approval or grant set: it loads the domain
            // from a checkpoint at or above the floor (§9, §18).
            PageStatus::BeforeHistoryFloor => {
                self.state.bootstrap = true;
                self.save()?;
                return Ok((0, false));
            }
            // The floor passed the cursor: the device loads what it lacks
            // from the latest checkpoint (§18).
            PageStatus::Compacted => {
                if !self.bootstrap_above(service, page.retention_floor).await? {
                    return Err(SyncError::Check("sync_manifest_rollback"));
                }
                return Ok((0, false));
            }
        }
        let accepted = self.accepted_revision();
        let above = page.policy_revision > accepted
            || page.entries.iter().any(
                |e| matches!(&e.record, EntryRecord::Policy(entry) if entry.revision > accepted),
            );
        let unknown_epoch = page.entries.iter().any(|e| match &e.record {
            EntryRecord::Epoch(epoch) => !self
                .state
                .epochs
                .iter()
                .any(|accepted| accepted.digest() == epoch.digest()),
            EntryRecord::Operation(op) => !self.keys.contains_key(&op.body.epoch),
            _ => false,
        });
        // The latest policy entry before the first epoch entry the device
        // hasn't accepted, which judges its envelope's wrapper (§9).
        self.entry_hint = None;
        let mut latest_entry = self.state.log.judge.latest_policy_entry();
        for entry in page.entries.iter() {
            match &entry.record {
                EntryRecord::Policy(policy) => latest_entry = Some(policy.revision),
                EntryRecord::Epoch(epoch)
                    if !self
                        .state
                        .epochs
                        .iter()
                        .any(|accepted| accepted.digest() == epoch.digest()) =>
                {
                    self.entry_hint = latest_entry;
                    break;
                }
                _ => {}
            }
        }
        if above || unknown_epoch || self.remote.is_none() {
            match self.refresh(service).await {
                Ok(()) => {}
                // An envelope still on its way isn't a failure: the device
                // holds its cursor until the key arrives (§9, §13).
                Err(SyncError::KeyMissing) if unknown_epoch => {
                    return Ok((0, true));
                }
                // The device holds the key of every epoch here. Its latest
                // envelope only waits again, since the wrapper lost its
                // access, so it takes the policy (§9).
                Err(SyncError::KeyMissing) => {
                    let remote = self.remote.take().ok_or(SyncError::KeyMissing)?;
                    self.adopt(remote)?;
                }
                Err(error) => return Err(error),
            }
        }
        self.settle_forks(service).await?;
        let extra = self.fetch_hidden_checkpoints(service, &page).await?;
        let mut checked = match self.judge_page(service, &page, &extra).await? {
            Some(checked) => checked,
            None => return Ok((0, true)),
        };
        let fetched = self.fetch_blobs(service, &checked).await?;
        let walks = std::mem::take(&mut checked.walks);
        let applied = self.apply(checked, fetched);
        self.clear_downloads();
        let applied = applied?;
        // Objects a file in their way held back before get another try.
        if !self.matching {
            for rid in self.unsettled() {
                self.settle(rid, &BTreeMap::new())?;
            }
            // A hold that ended leaves files to match with the domain's
            // live objects before anything writes there (§9).
            if std::mem::take(&mut self.rematch) {
                self.match_local()?;
            }
        }
        for (digest, at, own) in walks {
            let differing = self.walk_manifest(service, at, &own).await;
            self.state.log.checkpoints.set_differing(&digest, differing);
        }
        self.state.log.cursor = page.complete_through;
        self.state.log.checkpoints.due = page.checkpoint_due;
        self.state.log.head = self.state.log.head.max(page.head);
        self.state.log.checkpoint_seq = self.state.log.checkpoint_seq.max(page.checkpoint_seq);
        self.state.log.retention_floor = self.state.log.retention_floor.max(page.retention_floor);
        self.save()?;
        Ok((applied, false))
    }

    /// Settles each fork the log holds (§14): once the accepted policy
    /// ends the forked device's write access to the domain at a change r,
    /// the device fetches checkpoints from the latest back through
    /// `previous` to the first whose `policy_revision` is at or above r,
    /// reads the log from its cursor through that checkpoint's
    /// `covered_seq`, and keeps the branch whose head the checkpoint lists.
    /// A fork it can't settle yet keeps holding the cursor.
    async fn settle_forks(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        for (device, at) in self.state.log.forked.clone() {
            let Some(remote) = self.remote.as_ref() else {
                return Ok(());
            };
            let policy = &remote.policy;
            if policy.head_snapshot().can_write(&device, &domain) {
                continue;
            }
            // The first change after the forked record's position that ends
            // the device's write access (§14).
            let head = policy.head().0;
            let Some(ended) = (at.max(1) + 1..=head).find(|&r| {
                policy
                    .at(r - 1)
                    .is_some_and(|s| s.can_write(&device, &domain))
                    && policy.at(r).is_some_and(|s| !s.can_write(&device, &domain))
            }) else {
                continue;
            };
            let Some(latest) = service.client().checkpoint(workspace, domain).await? else {
                continue;
            };
            let mut covering = latest.bundle;
            if covering.checkpoint.0.body.policy_revision < ended {
                continue;
            }
            loop {
                let previous = covering.checkpoint.0.body.previous;
                if previous.is_zero() {
                    break;
                }
                let Some(entry) = service
                    .client()
                    .checkpoint_by_digest(workspace, domain, previous)
                    .await?
                else {
                    return Err(SyncError::Check("sync_chain_gap"));
                };
                let EntryRecord::Checkpoint(before) = entry.record else {
                    return Err(SyncError::Check("sync_chain_gap"));
                };
                if before.checkpoint.0.digest() != previous {
                    return Err(SyncError::Check("sync_chain_gap"));
                }
                if before.checkpoint.0.body.policy_revision < ended {
                    break;
                }
                covering = before;
            }
            let Some(kept) = covering.heads.iter().find(|h| h.device == device).cloned() else {
                continue;
            };
            let covered = covering.checkpoint.0.body.covered_seq;
            let mut records = Vec::new();
            let mut after = self.state.log.cursor;
            while after < covered {
                let response = service
                    .client()
                    .pull(workspace, vec![PullCursor { domain, after }], 0)
                    .await?;
                let Some(page) = response.pages.0.into_iter().find(|p| p.domain == domain) else {
                    return Err(SyncError::Check("sync_chain_gap"));
                };
                if page.complete_through <= after {
                    break;
                }
                for entry in page.entries.iter().filter(|e| e.seq <= covered) {
                    if let EntryRecord::Operation(op) = &entry.record
                        && op.body.device == device
                    {
                        records.push(ChainRecord::from_operation(op, entry.seq));
                    }
                }
                after = page.complete_through;
            }
            if let Ok(reverted) = self.state.log.verifier.resolve_fork(&kept, &records) {
                self.revert(device, &reverted, &BTreeMap::new())?;
                self.state.log.forked.remove(&device);
                self.save()?;
            }
        }
        Ok(())
    }

    /// Takes out of the journal the records of `device` that a settled
    /// fork reverted (§14): each leaves the live set of its object, its
    /// state and the per-record refs the next checkpoint's heads come
    /// from, and each touched object's file follows its live set again.
    fn revert(
        &mut self,
        device: Id,
        reverted: &[sync_protocol::chain::Reverted],
        contents: &Bodies,
    ) -> Result<(), SyncError> {
        let mut touched = Vec::new();
        for record in reverted {
            let Some(rid) = self.state.log.live.remove_digest(&record.digest) else {
                continue;
            };
            let key = OpKey {
                device,
                counter: record.counter,
            };
            if let Some(object) = self.state.objects.get_mut(&rid)
                && object
                    .states
                    .get(&key)
                    .is_some_and(|s| s.digest == record.digest)
            {
                object.states.remove(&key);
            }
            self.state.unreadable.retain(|u| {
                !(u.rid == rid
                    && u.device == device
                    && u.counter == record.counter
                    && u.seq == record.seq)
            });
            self.state
                .log
                .checkpoints
                .since
                .retain(|r| r.op_digest != record.digest);
            if !touched.contains(&rid) {
                touched.push(rid);
            }
        }
        // The kept branch's records carry the device's policy revision and
        // generation again.
        if !reverted.is_empty() {
            self.state.log.last.remove(&device);
        }
        for rid in touched {
            self.settle(rid, contents)?;
        }
        Ok(())
    }

    /// Walks the server's manifest at `at` against the device's own to
    /// find the objects whose leaves differ (§17): the bucket hashes first,
    /// then the leaf refs of each bucket that differs. A server that can't
    /// serve the manifest leaves the list empty.
    async fn walk_manifest(
        &mut self,
        service: &mut SyncService,
        at: u64,
        own: &sync_protocol::manifest::Manifest,
    ) -> Vec<Id> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let Ok(theirs) = service
            .client()
            .manifest_buckets(workspace, domain, at)
            .await
        else {
            return Vec::new();
        };
        let mine = own.bucket_hashes();
        let mut differing = BTreeSet::new();
        for (index, (a, b)) in mine.iter().zip(theirs.buckets.iter()).enumerate() {
            if a == b {
                continue;
            }
            let index = index as u16;
            let mut leaves: BTreeMap<Id, Vec<sync_protocol::records::Ref>> = BTreeMap::new();
            let mut after = None;
            loop {
                let Ok(page) = service
                    .client()
                    .manifest_leaves(workspace, domain, at, index, after.as_ref())
                    .await
                else {
                    break;
                };
                for leaf in page.leaves.iter() {
                    leaves.entry(leaf.rid).or_default().push(leaf.op.clone());
                }
                match page.next {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
            let ours: BTreeMap<Id, Vec<sync_protocol::records::Ref>> =
                own.bucket_leaves(index).into_iter().collect();
            for rid in ours.keys().chain(leaves.keys()) {
                if ours.get(rid) != leaves.get(rid) {
                    differing.insert(*rid);
                }
            }
        }
        differing.into_iter().collect()
    }

    /// Fetches by digest the checkpoints a page's entries show the log
    /// left out (§17, Checkpoint chain): before a checkpoint entry whose
    /// `previous` isn't the last checkpoint reached, and before a record
    /// of the current generation that names a checkpoint the device hasn't
    /// reached, at or above the retention floor. Each walk follows
    /// `previous` back to the last checkpoint reached; one the server can't
    /// serve, or a walk that passes it, reports `sync_chain_gap`. Returns
    /// the fetched entries by the index of the entry they go before,
    /// oldest first.
    async fn fetch_hidden_checkpoints(
        &mut self,
        service: &mut SyncService,
        page: &Page,
    ) -> Result<BTreeMap<usize, Vec<Entry>>, SyncError> {
        use sync_protocol::manifest::{ChainLink, CheckpointWalk, WalkEntry};

        let checkpoints = &self.state.log.checkpoints;
        // A writer that lags names a checkpoint below the floor or below
        // the device's bootstrap point, which needs no check.
        let floor = self
            .state
            .log
            .retention_floor
            .max(page.retention_floor)
            .max(checkpoints.bootstrap_seq);
        let reached: Vec<(u64, Digest)> = checkpoints
            .reached
            .iter()
            .map(|(n, r)| (*n, r.digest))
            .collect();
        let entries: Vec<WalkEntry> = page
            .entries
            .iter()
            .map(|entry| match &entry.record {
                EntryRecord::Checkpoint(bundle) => {
                    WalkEntry::Checkpoint(ChainLink::of(&bundle.checkpoint.0))
                }
                EntryRecord::Operation(op) => WalkEntry::Operation {
                    generation: op.body.generation,
                    checkpoint_seq: op.body.checkpoint_seq,
                    checkpoint: op.body.checkpoint,
                },
                _ => WalkEntry::Other,
            })
            .collect();
        // The walk runs on what the server served so far, fetching each
        // checkpoint it misses and running again.
        let mut cache: BTreeMap<Digest, Option<Entry>> = BTreeMap::new();
        loop {
            let mut fetcher = CachedCheckpoints {
                cache: &cache,
                workspace: self.state.workspace,
                domain: self.state.domain,
                misses: Vec::new(),
            };
            let planned =
                CheckpointWalk::new(reached.iter().copied(), floor, self.state.generation)
                    .plan(&entries, &mut fetcher);
            let misses = std::mem::take(&mut fetcher.misses);
            if misses.is_empty() {
                let planned = planned.map_err(|_| SyncError::Check("sync_chain_gap"))?;
                return Ok(planned
                    .into_iter()
                    .map(|(index, walk)| {
                        let fetched = walk
                            .iter()
                            .filter_map(|link| cache.get(&link.digest).cloned().flatten())
                            .collect();
                        (index, fetched)
                    })
                    .collect());
            }
            for digest in misses {
                let fetched = service
                    .client()
                    .checkpoint_by_digest(self.state.workspace, self.state.domain, digest)
                    .await?;
                cache.insert(digest, fetched);
            }
        }
    }

    /// Checks a page's shape and its positions against what the server
    /// reported before in this generation (§14, §15).
    fn check_page(&mut self, page: &Page) -> Result<(), SyncError> {
        if page.restore_generation < self.state.generation {
            return Err(SyncError::Check("sync_server_rollback"));
        }
        // A restore: the device applies nothing from this page, and the
        // round's gate compares (§14).
        if page.restore_generation > self.state.generation {
            return Err(SyncError::Check("sync_restore_generation"));
        }
        let floored = matches!(page.status, PageStatus::Ok | PageStatus::Compacted);
        if (page.status == PageStatus::Ok
            && (page.head < self.state.log.cursor || page.head < self.state.log.head))
            || (floored && page.checkpoint_seq < self.state.log.checkpoint_seq)
        {
            return Err(SyncError::Check("sync_server_rollback"));
        }
        if floored && page.retention_floor < self.state.log.retention_floor {
            return Err(SyncError::Check("sync_manifest_rollback"));
        }
        if page.status == PageStatus::Compacted && page.after >= page.retention_floor {
            return Err(SyncError::Check("sync_manifest_rollback"));
        }
        page.check_shape()
            .map_err(|_| SyncError::Network("the pull page breaks §15".into()))
    }

    /// Judges every record of `page` on copies of the log's state; returns
    /// the checked page, or `None` when a record's key hasn't arrived.
    async fn judge_page(
        &mut self,
        service: &mut SyncService,
        page: &Page,
        extra: &BTreeMap<usize, Vec<Entry>>,
    ) -> Result<Option<Checked>, SyncError> {
        let mut cache: BTreeMap<(Id, u64), Fetched> = BTreeMap::new();
        for _ in 0..64 {
            let mut fetcher = Cached {
                cache: &cache,
                misses: Vec::new(),
                forked: None,
            };
            let outcome = self.judge_with(page, extra, &mut fetcher);
            let misses = std::mem::take(&mut fetcher.misses);
            if let (Err(SyncError::Check("sync_chain_forked")), Some((device, at))) =
                (&outcome, fetcher.forked)
                && misses.is_empty()
                && !self.state.log.forked.contains_key(&device)
            {
                self.state.log.forked.insert(device, at);
                self.save()?;
            }
            match outcome {
                Err(error) if !misses.is_empty() => {
                    let _ = error;
                    for (device, counter) in misses {
                        let fetched = match service
                            .client()
                            .chain_position(
                                self.state.workspace,
                                self.state.domain,
                                device,
                                counter,
                            )
                            .await
                        {
                            Ok(Some(entry)) => match &entry.record {
                                EntryRecord::Operation(op) => {
                                    Fetched::Record(ChainRecord::from_operation(op, entry.seq))
                                }
                                _ => Fetched::NotFound,
                            },
                            Ok(None) => Fetched::NotFound,
                            Err(SyncError::Refused { code, .. }) if code == "pruned" => {
                                Fetched::Pruned
                            }
                            Err(error) => return Err(error),
                        };
                        cache.insert((device, counter), fetched);
                    }
                }
                other => return other,
            }
        }
        Err(SyncError::Check("sync_chain_gap"))
    }

    fn judge_with(
        &self,
        page: &Page,
        extra: &BTreeMap<usize, Vec<Entry>>,
        fetcher: &mut Cached<'_>,
    ) -> Result<Option<Checked>, SyncError> {
        let remote = self
            .remote
            .as_ref()
            .ok_or(SyncError::Check("sync_workspace_unverified"))?;
        let policy = &remote.policy;
        let ended: BTreeMap<u64, Digest> = self
            .state
            .ended_epochs
            .iter()
            .map(|e| (e.body.epoch, e.digest()))
            .collect();
        let mut log = self.state.log.clone();
        for wanted in &mut log.wanted {
            if wanted.by == u64::MAX && self.armed.contains(&(wanted.epoch, wanted.number)) {
                wanted.by = page.head;
            }
        }
        let mut records = Vec::new();
        let mut walks = Vec::new();
        // The checkpoints fetched by digest go in before the entry that
        // named them, oldest first, as if the page had carried them (§17).
        let entries = page.entries.iter().enumerate().flat_map(|(i, entry)| {
            extra
                .get(&i)
                .into_iter()
                .flatten()
                .chain(std::iter::once(entry))
        });
        for entry in entries {
            match &entry.record {
                EntryRecord::Policy(policy_entry) => {
                    log.wanted
                        .retain(|w| w.epoch || w.number != policy_entry.revision);
                    let change = policy
                        .change(policy_entry.revision)
                        .ok_or(SyncError::Check("sync_policy_rollback"))?;
                    if change.digest() != policy_entry.head {
                        return Err(SyncError::Check("sync_policy_forked"));
                    }
                    log.judge
                        .policy_entry(policy_entry.revision, policy)
                        .map_err(|e| SyncError::Check(e.client_code()))?;
                    log.checkpoints
                        .note_policy_entry(entry.seq, policy_entry.revision);
                    if let (Some(before), Some(after)) = (
                        policy.at(policy_entry.revision - 1),
                        policy.at(policy_entry.revision),
                    ) {
                        log.note_write_access(
                            &self.state.domain,
                            policy_entry.revision,
                            entry.seq,
                            before,
                            after,
                        )?;
                    }
                    if matches!(change.body.kind, PolicyKind::CreateDomain { .. }) {
                        log.domain_entries.insert(policy_entry.revision, entry.seq);
                    }
                }
                EntryRecord::Epoch(epoch) => {
                    log.wanted
                        .retain(|w| !w.epoch || w.number != epoch.body.epoch);
                    log.epoch_entries = log.epoch_entries.max(epoch.body.epoch);
                    log.checkpoints
                        .note_epoch_entry(entry.seq, epoch.body.epoch, epoch.digest());
                    let accepted = remote
                        .epochs
                        .get(epoch.body.epoch)
                        .filter(|a| a.digest == epoch.digest());
                    let Some(accepted) = accepted else {
                        if remote.epochs.ended_at(epoch.body.epoch).is_some() {
                            // A replacement ended it; its records are judged
                            // as records under a skipped epoch.
                            log.judge.epoch_entry(&epoch.body);
                            log.skipped_rotators
                                .insert(epoch.body.epoch, epoch.body.rotating_device);
                            continue;
                        }
                        return Ok(None);
                    };
                    log.judge.epoch_entry(&accepted.record.body);
                }
                EntryRecord::Checkpoint(bundle) => {
                    let record = &bundle.checkpoint.0;
                    log.check_final_heads(record.body.covered_seq, &bundle.heads)?;
                    // The checkpoint a client bootstrapped from, or one it
                    // fetched by digest, reaches it again (§17).
                    if log.checkpoints.has_reached(&record.digest()) {
                        // The pull that follows a bootstrap reaches the
                        // checkpoint's entry with every policy entry
                        // before it judged, so the signer's write access
                        // there gets its check by position again (§17).
                        let bootstrapped = log
                            .checkpoints
                            .accepted
                            .as_ref()
                            .is_some_and(|a| a.record.digest() == record.digest())
                            && log.checkpoints.bootstrap_seq == record.body.covered_seq;
                        if bootstrapped {
                            log.judge
                                .check_writer(
                                    &record.body.signer,
                                    record.body.policy_revision,
                                    policy,
                                )
                                .map_err(|e| SyncError::Check(e.client_code()))?;
                        }
                        continue;
                    }
                    let checked = if record.body.workspace != self.state.workspace
                        || record.body.domain != self.state.domain
                    {
                        Err("sync_manifest_mismatch")
                    } else {
                        log.checkpoints
                            .check(bundle, &log.live, &log.judge, policy, &remote.epochs)
                    };
                    match checked {
                        Ok(()) => {
                            log.checkpoints.accept(bundle, &mut log.live);
                            // Below an accepted checkpoint the verifier
                            // keeps only heads (§14).
                            log.verifier.forget_below(record.body.covered_seq);
                        }
                        Err(code) => {
                            tracing::warn!(
                                code,
                                number = record.body.number,
                                "a checkpoint failed its checks"
                            );
                            log.checkpoints.fail(record, code);
                            if code == "sync_manifest_mismatch" {
                                let own = log.checkpoints.own_manifest(bundle, &log.live);
                                if own.root() != record.body.manifest_root {
                                    walks.push((record.digest(), record.body.covered_seq, own));
                                }
                            }
                        }
                    }
                    log.checkpoints.reach(record, entry.seq);
                }
                EntryRecord::Operation(op) => {
                    match check_operation(
                        remote, &self.keys, &ended, &mut log, op, entry.seq, fetcher,
                    )? {
                        Judged::Record(record) => records.push(*record),
                        Judged::Ignored => {}
                        Judged::NoKey => return Ok(None),
                    }
                }
            }
        }
        // An entry the log owed by now, as for an accepted change or a
        // known epoch, is missing (§8, §9).
        if log.wanted.iter().any(|w| w.by <= page.complete_through) {
            return Err(SyncError::Check("sync_chain_gap"));
        }
        // The server acknowledged this device's records through a sequence
        // the page claims to cover, so they must be there (§14, Suffix
        // withholding).
        let own = self.credentials.keys.device();
        if let Some((counter, _, seq)) = self.state.own.acked
            && seq <= page.complete_through
            && log.verifier.head(&own).counter < counter
        {
            return Err(SyncError::Check("sync_chain_gap"));
        }
        Ok(Some(Checked {
            log,
            records,
            walks,
        }))
    }

    /// Applies a checked page: records each state in the journal, then
    /// brings each touched object's file in line with its live set.
    fn apply(&mut self, checked: Checked, fetched: Bodies) -> Result<usize, SyncError> {
        let own = self.credentials.keys.device();
        let Checked { log, records, .. } = checked;
        self.state.log = log;
        let mut touched: Vec<Id> = Vec::new();
        let mut contents: Bodies = fetched;
        let mut applied = 0;
        let mut arrivals = Vec::new();
        let mut departures = Vec::new();
        for record in records {
            let op = &record.op;
            let key = OpKey {
                device: op.body.device,
                counter: op.body.counter,
            };
            if let Some(payload) = record.payload.as_ref().filter(|p| is_text(p)) {
                let blob = text_blob(payload, &mut contents);
                self.take_text(&record, blob)?;
                let own = op.body.device == own;
                applied += usize::from(!own);
                continue;
            }
            let domain = self.active;
            let object = self.state.objects.entry(op.body.rid).or_default();
            object.domain = domain;
            if op.body.device == own {
                object.own_highest = object.own_highest.max(op.body.counter);
                // This device's own record: its file already holds it,
                // unless a lost journal forgot it.
                if let Some(state) = object.states.get_mut(&key) {
                    state.seq = Some(record.seq);
                    state.digest = op.digest();
                    state.epoch = op.body.epoch;
                    continue;
                }
            }
            applied += 1;
            match record.payload.as_ref().map(|p| &p.kind) {
                Some(PayloadKind::File {
                    origin: Some(origin),
                    ..
                }) => arrivals.push((op.body.rid, origin.clone())),
                Some(PayloadKind::Delete {
                    reason: DeleteReason::Moved,
                    target: Some(target),
                    ..
                }) => departures.push((op.body.rid, target.clone())),
                _ => {}
            }
            let content = match record.payload {
                Some(payload) => carried(&payload, &mut contents),
                None => {
                    self.state.unreadable.push(state::Unreadable {
                        rid: op.body.rid,
                        device: op.body.device,
                        counter: op.body.counter,
                        seq: record.seq,
                    });
                    None
                }
            };
            if let Some(content) = &content
                && object.object_id.is_empty()
            {
                object.object_id = content.object_id.clone();
            }
            if record.class == OpClass::State {
                object.states.insert(
                    key,
                    StateInfo {
                        digest: op.digest(),
                        seq: Some(record.seq),
                        covers: op.body.supersedes.to_vec(),
                        content,
                        epoch: op.body.epoch,
                        policy_revision: op.body.policy_revision,
                    },
                );
            }
            if !touched.contains(&op.body.rid) {
                touched.push(op.body.rid);
            }
        }
        // A move's create comes before its delete: the new object takes
        // the file, and a delete whose create hasn't arrived waits (§9).
        for (moved, origin) in arrivals {
            self.arrive(moved, origin);
        }
        for (rid, target) in departures {
            if self.defers_delete(&target) {
                self.state.deferred_moves.insert(rid, target.rid);
                touched.retain(|t| *t != rid);
            }
        }
        // Every marker in the page stands as a root before any file beneath
        // it finds its place, and the marker's own file goes first (§9).
        self.update_roots()?;
        if self.matching {
            // The matching after a sync without the journal writes these.
            self.cache.extend(contents);
            return Ok(applied);
        }
        let markers = self.marker_objects();
        touched.sort_by_key(|rid| !markers.contains(rid));
        for rid in touched {
            self.settle(rid, &contents)?;
            // A state operation may have ended the note's lineage.
            self.note_text_change(rid);
        }
        self.settle_text()?;
        Ok(applied)
    }

    /// Fetches the blobs a checked page's records carry before the page
    /// applies (§18, §21). Each one streams into the journal's
    /// `downloads/` folder, where its segment hashes, `plaintext_length` and
    /// revision are checked before any file in the workspace changes. A
    /// file this device holds at the payload's revision lends its bytes
    /// instead: the object's own file is used where it is, and another
    /// object's file, such as the original of a copy, is copied into
    /// `downloads/` and hashed again as it is copied, else the blob
    /// downloads. A blob that fails a check fails the pull, and the cursor
    /// stays where it was.
    async fn fetch_blobs(
        &mut self,
        service: &mut SyncService,
        checked: &Checked,
    ) -> Result<Bodies, SyncError> {
        self.clear_downloads();
        let own = self.credentials.keys.device();
        let downloads = self.journal.dir().join(journal::DOWNLOADS);
        let mut fetched = Bodies::new();
        for record in &checked.records {
            // A text record's blob holds yrs bytes, and its segments'
            // hashes check it (§22).
            if let Some(Payload {
                kind:
                    PayloadKind::TextSnapshot {
                        content: Content::Blob(blob),
                        ..
                    }
                    | PayloadKind::TextUpdate {
                        content: Content::Blob(blob),
                        ..
                    },
                ..
            }) = &record.payload
            {
                if record.op.body.device != own {
                    let path = service
                        .client()
                        .download_blob(self.state.workspace, blob, None, &downloads)
                        .await?;
                    fetched.insert(blob.ciphertext_digest, Body::File(path));
                }
                continue;
            }
            let Some(Payload {
                kind:
                    PayloadKind::File {
                        revision,
                        content: Content::Blob(blob),
                        ..
                    },
                ..
            }) = &record.payload
            else {
                continue;
            };
            let rid = record.op.body.rid;
            let key = OpKey {
                device: record.op.body.device,
                counter: record.op.body.counter,
            };
            let object = self.state.objects.get(&rid);
            // This device's own record, which the journal still holds: its
            // file holds it, or the review store does.
            if key.device == own && object.is_some_and(|o| o.states.contains_key(&key)) {
                continue;
            }
            if fetched.contains_key(revision) {
                continue;
            }
            let holds = |local: &state::Local| {
                local.revision == *revision
                    && self.engine.hash_synced(&local.path).ok().flatten()
                        == Some(revision.to_hex())
            };
            if object.and_then(|o| o.local.as_ref()).is_some_and(holds) {
                continue;
            }
            let lent = self
                .state
                .objects
                .iter()
                .filter(|(other, _)| **other != rid)
                .filter_map(|(_, o)| o.local.as_ref())
                .filter(|local| local.revision == *revision)
                .find_map(|local| {
                    let copy = downloads.join(format!("{}.copy", revision.to_hex()));
                    copy_checked(&self.engine.root().join(&local.path), &copy, revision)
                        .then_some(copy)
                });
            let path = match lent {
                Some(path) => path,
                None => {
                    service
                        .client()
                        .download_blob(self.state.workspace, blob, Some(revision), &downloads)
                        .await?
                }
            };
            fetched.insert(*revision, Body::File(path));
        }
        Ok(fetched)
    }

    /// Removes the files in the journal's `downloads/` folder that no
    /// matching still waits for.
    fn clear_downloads(&self) {
        let downloads = self.journal.dir().join(journal::DOWNLOADS);
        let waiting: BTreeSet<&std::path::PathBuf> = self
            .cache
            .values()
            .filter_map(|body| match body {
                Body::File(path) => Some(path),
                Body::Bytes(_) => None,
            })
            .collect();
        for entry in std::fs::read_dir(&downloads)
            .into_iter()
            .flatten()
            .flatten()
        {
            if !waiting.contains(&entry.path()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Lists the objects in conflict (§16).
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        for (rid, object) in &self.state.objects {
            let branches = self.branches(rid, object);
            if branches.len() > 1 {
                conflicts.push(Conflict {
                    rid: *rid,
                    branches,
                });
            }
        }
        conflicts
    }

    /// The distinct live states of `rid`, one per state, lowest operation
    /// digest first, identical states folded together.
    fn branches(&self, rid: &Id, object: &Object) -> Vec<Branch> {
        let mut distinct: Vec<(Digest, Branch, Carried)> = Vec::new();
        for key in self.live_states(rid) {
            let Some(info) = object.states.get(&key) else {
                continue;
            };
            let Some(content) = &info.content else {
                continue;
            };
            if let Some(existing) = distinct.iter_mut().find(|(_, _, c)| same_state(c, content)) {
                if info.digest < existing.0 {
                    existing.0 = info.digest;
                    existing.1.key = key;
                    existing.1.device = key.device;
                }
                continue;
            }
            let on_disk = object.local.as_ref().is_some_and(|l| {
                !content.delete && l.path == content.path && l.revision == content.revision
            }) || (content.delete && object.local.is_none());
            distinct.push((
                info.digest,
                Branch {
                    device: key.device,
                    path: (!content.delete).then(|| content.path.clone()),
                    on_disk,
                    key,
                },
                content.clone(),
            ));
        }
        distinct.sort_by_key(|(digest, _, _)| *digest);
        distinct.into_iter().map(|(_, b, _)| b).collect()
    }

    /// Resolves the conflict on `rid` with the branch at `chosen`: writes it
    /// to disk and signs a state operation that covers every branch (§16).
    /// The operation goes out in the next round.
    pub fn choose(&mut self, rid: Id, chosen: OpKey) -> Result<(), SyncError> {
        let object = self
            .state
            .objects
            .get(&rid)
            .ok_or_else(|| SyncError::Invalid("no such object".into()))?;
        let content = object
            .states
            .get(&chosen)
            .and_then(|s| s.content.clone())
            .ok_or_else(|| SyncError::Invalid("no such branch".into()))?;
        // A choice in a text-mode note covers its patches too, which ends
        // the lineage.
        let text = self.in_text_mode(&rid);
        let view: BTreeSet<OpKey> = if text {
            self.live_ops(&rid).into_iter().collect()
        } else {
            self.live_states(&rid).into_iter().collect()
        };
        let covers = self.covers_of(&rid, &view);
        if content.delete {
            if let Some(local) = object.local.clone() {
                self.engine
                    .delete_synced(&local.path, &local.revision.to_hex())
                    .map_err(core)?;
            }
            self.state.objects.get_mut(&rid).expect("checked").local = None;
            self.sign_delete(rid, covers)?;
        } else {
            // A text-mode branch's bytes come from its document.
            let merged = match content.text {
                Some(_) => self.text_body_of(&rid)?,
                None => None,
            };
            let (body, revision) = match merged {
                Some(bytes) => {
                    let revision = hash(&bytes);
                    (Body::Bytes(bytes), revision)
                }
                None => (
                    self.content_body(&rid, &chosen, &BTreeMap::new())?,
                    content.revision,
                ),
            };
            self.place(&rid, &content.path, &body, revision)?;
            let found = self.found_on_disk(&content.path)?;
            self.sign_file(rid, &found, covers)?;
        }
        if text {
            self.end_text(rid)?;
        }
        self.save()
    }

    /// The file at `path`, as the walk would find it: its bytes when it's
    /// Markdown or fits inline, else only its revision, hashed as it
    /// streams.
    fn found_on_disk(&self, path: &str) -> Result<Found, SyncError> {
        let (size, modified_ns, file_id) = stat(self.engine.root(), path)
            .ok_or_else(|| SyncError::Invalid("the file went away".into()))?;
        let (revision, bytes) = read_for_sync(&self.engine, path, size)
            .ok_or_else(|| SyncError::Invalid("the file went away".into()))?;
        Ok(Found {
            file: local_core::SyncedFile {
                relative_path: path.to_owned(),
                size,
                modified_ns,
                file_id,
            },
            revision,
            object_id: bytes
                .as_deref()
                .map(|bytes| capture::object_id_of(path, bytes))
                .unwrap_or_default(),
            bytes,
        })
    }
}

/// What sync needs of the file at `path`, whose size is `size`: its
/// revision, and its bytes when it's Markdown or fits inline. A larger file
/// is hashed as it streams, so an attachment never sits in memory.
pub(super) fn read_for_sync(
    engine: &WorkspaceEngine,
    path: &str,
    size: u64,
) -> Option<(Digest, Option<Vec<u8>>)> {
    if path.ends_with(".md") || capture::fits_inline(path, "", size) {
        let bytes = engine.read_synced(path).ok().flatten()?;
        Some((hash(&bytes), Some(bytes)))
    } else {
        let revision = engine.hash_synced(path).ok().flatten()?;
        Some((Digest::from_hex(&revision)?, None))
    }
}

/// A record that passed every check.
struct CheckedRecord {
    op: Signed<Operation>,
    seq: u64,
    class: OpClass,
    /// The payload, or `None` when the record didn't open.
    payload: Option<Payload>,
}

/// A page whose records all passed.
struct Checked {
    log: Log,
    records: Vec<CheckedRecord>,
    /// Checkpoints whose root differs from the device's own manifest at
    /// their `covered_seq`, which the device then walks (§17).
    walks: Vec<(Digest, u64, sync_protocol::manifest::Manifest)>,
}

/// Serves chain positions the device fetched, and notes the ones it
/// hasn't, which the next pass fetches.
/// The checkpoints a walk fetched so far, by digest, noting each one it
/// misses.
struct CachedCheckpoints<'a> {
    cache: &'a BTreeMap<Digest, Option<Entry>>,
    workspace: Id,
    domain: Id,
    misses: Vec<Digest>,
}

impl sync_protocol::manifest::CheckpointFetcher for CachedCheckpoints<'_> {
    fn checkpoint(&mut self, digest: &Digest) -> Option<sync_protocol::manifest::ChainLink> {
        let Some(fetched) = self.cache.get(digest) else {
            self.misses.push(*digest);
            return None;
        };
        let EntryRecord::Checkpoint(bundle) = &fetched.as_ref()?.record else {
            return None;
        };
        let record = &bundle.checkpoint.0;
        (record.body.workspace == self.workspace && record.body.domain == self.domain)
            .then(|| sync_protocol::manifest::ChainLink::of(record))
    }
}

struct Cached<'a> {
    cache: &'a BTreeMap<(Id, u64), Fetched>,
    misses: Vec<(Id, u64)>,
    /// The device whose record reported `sync_chain_forked`, with the
    /// revision of the latest policy entry before the record.
    forked: Option<(Id, u64)>,
}

impl ChainFetcher for Cached<'_> {
    fn fetch(&mut self, device: &Id, counter: u64) -> Fetched {
        match self.cache.get(&(*device, counter)) {
            Some(fetched) => *fetched,
            None => {
                self.misses.push((*device, counter));
                Fetched::NotFound
            }
        }
    }
}

/// What the checks of one operation found.
enum Judged {
    /// It passed, and the device applies it.
    Record(Box<CheckedRecord>),
    /// It repeats a position the device applied, or an earlier attempt.
    Ignored,
    /// The device doesn't hold the key of its epoch yet, which holds the
    /// cursor (§13).
    NoKey,
}

/// The client checks of §13 on one operation, before it decrypts it, then
/// its decryption.
fn check_operation(
    remote: &RemoteWorkspace,
    keys: &BTreeMap<u64, SecretKey>,
    ended: &BTreeMap<u64, Digest>,
    log: &mut Log,
    op: &Signed<Operation>,
    seq: u64,
    fetcher: &mut Cached<'_>,
) -> Result<Judged, SyncError> {
    let body = &op.body;
    let policy = &remote.policy;
    let device = policy
        .at(body.policy_revision)
        .and_then(|snapshot| snapshot.devices.get(&body.device))
        .ok_or(SyncError::Check("sync_writer_not_authorized"))?;
    op.verify(&device.card.signing_public)
        .map_err(|_| SyncError::Check("sync_writer_not_authorized"))?;
    // The other branch of a settled fork: the client ignores it (§14).
    if log.verifier.ignores(&body.device, &op.digest()) {
        return Ok(Judged::Ignored);
    }
    let change = policy
        .change(body.policy_revision)
        .ok_or(SyncError::Check("sync_writer_not_authorized"))?;
    if change.digest() != body.policy_head {
        return Err(SyncError::Check("sync_policy_forked"));
    }
    if let Some((revision, generation)) = log.last.get(&body.device)
        && (body.policy_revision < *revision || body.generation < *generation)
    {
        return Err(SyncError::Check("sync_policy_regressed"));
    }
    log.judge
        .check_operation(&body.device, body.policy_revision, body.epoch, policy)
        .map_err(|e| SyncError::Check(e.client_code()))?;
    let verdict = log
        .verifier
        .process_one(&ChainRecord::from_operation(op, seq), fetcher)
        .map_err(|e| {
            if e == ChainError::Forked {
                fetcher.forked = Some((body.device, log.judge.latest_policy_entry().unwrap_or(0)));
            }
            SyncError::Check(e.client_code())
        })?;
    // The key of its epoch, from an envelope or a later key's lineage, or
    // an epoch this device accepted before a replacement ended it.
    let epoch_record = remote
        .epochs
        .get(body.epoch)
        .map(|e| e.digest)
        .or_else(|| ended.get(&body.epoch).copied());
    let opened = match (epoch_record, keys.get(&body.epoch)) {
        (Some(digest), Some(key)) => Some((digest, key)),
        _ => match remote.epochs.ended_at(body.epoch) {
            // An epoch a replacement skipped and this device never
            // accepted: its rotator's records, and those of a device that
            // lost read access by the replacement, count as unreadable; any
            // other is one an honest server refused (§9).
            Some(revision) => match judge_skipped_epoch_record(
                policy,
                &remote.domain,
                &body.device,
                log.skipped_rotators.get(&body.epoch),
                revision,
            ) {
                SkippedEpochRecord::Unreadable => None,
                SkippedEpochRecord::Hold => return Err(SyncError::Check("sync_key_mismatch")),
            },
            None => return Ok(Judged::NoKey),
        },
    };
    let live = LiveOp::from_operation(op, seq);
    if !matches!(verdict, ChainVerdict::Ignore) {
        log.checkpoints.note_record(live.as_ref());
    }
    let class = match verdict {
        ChainVerdict::Ignore => return Ok(Judged::Ignored),
        ChainVerdict::Replace { replaced } => {
            log.live.remove(&body.rid, &replaced);
            log.live.apply(live)
        }
        ChainVerdict::Accept => log.live.apply(live),
    };
    log.last
        .insert(body.device, (body.policy_revision, body.generation));
    // A record that passes every check but doesn't open counts as applied
    // and unreadable (§13).
    let payload = opened.and_then(|(digest, key)| op.body.open_payload(key, &digest, class).ok());
    Ok(Judged::Record(Box::new(CheckedRecord {
        op: op.clone(),
        seq,
        class,
        payload,
    })))
}

/// Copies the file at `from` to `to`, streaming, and reports whether its
/// bytes hashed to `revision`; a copy that doesn't is removed.
fn copy_checked(from: &Path, to: &Path, revision: &Digest) -> bool {
    let copied = (|| -> std::io::Result<bool> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut reader = std::fs::File::open(from)?;
        let mut writer = std::io::BufWriter::new(std::fs::File::create(to)?);
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1 << 16];
        loop {
            let n = std::io::Read::read(&mut reader, &mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            std::io::Write::write_all(&mut writer, &buffer[..n])?;
        }
        std::io::Write::flush(&mut writer)?;
        Ok(Digest(*hasher.finalize().as_bytes()) == *revision)
    })();
    if copied.as_ref().is_ok_and(|same| *same) {
        return true;
    }
    let _ = std::fs::remove_file(to);
    false
}

/// The blobs a payload references, which its operation lists (§21).
fn listed_blobs(payload: &Payload) -> Vec<Id> {
    match &payload.kind {
        PayloadKind::File {
            content: Content::Blob(blob),
            ..
        }
        | PayloadKind::TextSnapshot {
            content: Content::Blob(blob),
            ..
        }
        | PayloadKind::TextUpdate {
            content: Content::Blob(blob),
            ..
        } => vec![blob.blob],
        _ => Vec::new(),
    }
}

/// Whether a payload belongs to text mode (§22).
fn is_text(payload: &Payload) -> bool {
    matches!(
        payload.kind,
        PayloadKind::TextSnapshot { .. } | PayloadKind::TextUpdate { .. }
    )
}

/// The file of the blob a text record's payload names, from the page's
/// downloads.
fn text_blob(payload: &Payload, contents: &mut Bodies) -> Option<Vec<u8>> {
    let blob = match &payload.kind {
        PayloadKind::TextSnapshot {
            content: Content::Blob(blob),
            ..
        }
        | PayloadKind::TextUpdate {
            content: Content::Blob(blob),
            ..
        } => blob,
        _ => return None,
    };
    match contents.remove(&blob.ciphertext_digest)? {
        Body::Bytes(bytes) => Some(bytes),
        Body::File(path) => std::fs::read(path).ok(),
    }
}

/// What a payload carries, keeping a file's bytes in `contents` by the
/// revision they hash to.
fn carried(payload: &Payload, contents: &mut Bodies) -> Option<Carried> {
    match &payload.kind {
        PayloadKind::File {
            object_id,
            path,
            revision,
            content: Content::Inline { bytes },
            ..
        } => {
            if hash(&bytes.0) != *revision {
                return None;
            }
            contents.insert(*revision, Body::Bytes(bytes.0.clone()));
            Some(Carried {
                path: path.as_str().to_owned(),
                revision: *revision,
                delete: false,
                object_id: object_id.0.clone(),
                kept: None,
                blob: None,
                text: None,
            })
        }
        PayloadKind::Delete {
            object_id,
            revision,
            ..
        } => Some(Carried {
            path: String::new(),
            revision: *revision,
            delete: true,
            object_id: object_id.0.clone(),
            kept: None,
            blob: None,
            text: None,
        }),
        PayloadKind::File {
            object_id,
            path,
            revision,
            content: Content::Blob(blob),
            ..
        } => Some(Carried {
            path: path.as_str().to_owned(),
            revision: *revision,
            delete: false,
            object_id: object_id.0.clone(),
            kept: None,
            blob: Some(blob.clone()),
            text: None,
        }),
        // Text records go through `take_text`.
        _ => None,
    }
}

/// Whether two states are the same: both deletes, or the same decrypted
/// path and revision (§16).
pub(crate) fn same_state(a: &Carried, b: &Carried) -> bool {
    (a.delete && b.delete)
        || (!a.delete && !b.delete && a.path == b.path && a.revision == b.revision)
        || (a.text.is_some() && a.text == b.text)
}

fn policy_changes(policy: &PolicyChain) -> Vec<Signed<sync_protocol::records::PolicyChange>> {
    (1..=policy.head().0)
        .filter_map(|revision| policy.change(revision).cloned())
        .collect()
}

/// A file's size, modification time and file ID, as the listing reports
/// them.
fn stat(root: &Path, path: &str) -> Option<(u64, i64, Option<u64>)> {
    let metadata = std::fs::symlink_metadata(root.join(path)).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    #[cfg(unix)]
    let file_id = {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.ino())
    };
    #[cfg(not(unix))]
    let file_id = None;
    Some((metadata.len(), modified, file_id))
}

/// The name in the workspace's latest profile, checked as §25 says, with
/// its revision and digest.
async fn profile_of(
    service: &mut SyncService,
    remote: &mut RemoteWorkspace,
) -> Result<(String, Option<ProfileMark>), SyncError> {
    let list = service.client().workspaces().await?;
    let profile = list
        .workspaces
        .0
        .into_iter()
        .find(|w| w.workspace == remote.workspace)
        .and_then(|w| w.profile)
        .map(|p| p.0)
        .ok_or_else(|| SyncError::Invalid("the workspace has no profile".into()))?;
    service.older_epochs(remote, profile.body.epoch).await?;
    let remote = &*remote;
    // A change took the profile's signer out of the workspace, as a
    // recovery does to the lost devices: an owner or admin device reads its
    // name and signs it again on its first round (§25).
    let snapshot = remote.policy.head_snapshot();
    let device = service.credentials().keys.device();
    let may_rename = snapshot
        .account_of(&device)
        .and_then(|account| snapshot.role(&account))
        .is_some_and(|role| matches!(role, Role::Owner | Role::Admin));
    if may_rename && !snapshot.is_device(&profile.body.signer) {
        let name = check_profile_from(remote, &profile, None, false)?;
        return Ok((name, None));
    }
    let name = check_profile(remote, &profile, None)?;
    Ok((name, Some(mark_of(&profile))))
}

fn mark_of(profile: &Signed<Profile>) -> ProfileMark {
    ProfileMark {
        revision: profile.body.profile_revision,
        digest: profile.digest(),
        policy_revision: profile.body.policy_revision,
        signer: profile.body.signer,
        epoch: profile.body.epoch,
    }
}

/// Checks a profile (§25) against the verified policy and the last one the
/// device accepted, and returns its name.
pub(crate) fn check_profile(
    remote: &RemoteWorkspace,
    profile: &Signed<Profile>,
    accepted: Option<&ProfileMark>,
) -> Result<String, SyncError> {
    check_profile_from(remote, profile, accepted, true)
}

/// [`check_profile`], leaving out, when `current_signer` is false, the
/// rule that no later change took its signer out of the workspace: the
/// device that signs a removed signer's profile again reads its name.
fn check_profile_from(
    remote: &RemoteWorkspace,
    profile: &Signed<Profile>,
    accepted: Option<&ProfileMark>,
    current_signer: bool,
) -> Result<String, SyncError> {
    let body = &profile.body;
    let policy = &remote.policy;
    if let Some(accepted) = accepted
        && (body.profile_revision < accepted.revision
            || (body.profile_revision == accepted.revision && profile.digest() != accepted.digest)
            || body.policy_revision < accepted.policy_revision)
    {
        return Err(SyncError::Check("sync_profile_rollback"));
    }
    let snapshot = policy
        .at(body.policy_revision)
        .ok_or(SyncError::Check("sync_policy_rollback"))?;
    if snapshot.digest != body.policy_head {
        return Err(SyncError::Check("sync_policy_forked"));
    }
    let signer = snapshot
        .devices
        .get(&body.signer)
        .ok_or(SyncError::Check("sync_writer_not_authorized"))?;
    let admin = snapshot
        .account_of(&body.signer)
        .and_then(|account| snapshot.role(&account))
        .is_some_and(|role| matches!(role, Role::Owner | Role::Admin));
    if !admin {
        return Err(SyncError::Check("sync_writer_not_authorized"));
    }
    profile
        .verify(&signer.card.signing_public)
        .map_err(|_| SyncError::Check("sync_writer_not_authorized"))?;
    // No change above its revision takes its signer out of the workspace.
    let (head, _) = policy.head();
    for revision in body.policy_revision + 1..=head {
        if current_signer
            && !policy
                .at(revision)
                .is_some_and(|s| s.is_device(&body.signer))
        {
            return Err(SyncError::Check("sync_writer_not_authorized"));
        }
    }
    // Its epoch passes the writer rule: no change at or below its revision
    // requires the workspace domain to rotate past it.
    let epoch = remote
        .epochs
        .get(body.epoch)
        .ok_or(SyncError::Check("sync_key_mismatch"))?;
    if policy
        .rotation_required(
            &remote.domain,
            epoch.record.body.policy_revision,
            body.policy_revision,
        )
        .is_some()
    {
        return Err(SyncError::Check("sync_epoch_stale"));
    }
    let key = remote
        .keys
        .get(&body.epoch)
        .ok_or(SyncError::Check("sync_key_mismatch"))?;
    let payload: ProfilePayload = body
        .open_payload(key, &epoch.digest)
        .map_err(|_| SyncError::Check("sync_record_unreadable"))?;
    Ok(payload.name.0)
}

impl RemoteWorkspace {
    /// The accepted epoch records, oldest first.
    pub(crate) fn epochs_oldest_first(&self) -> Vec<&Signed<sync_protocol::records::Epoch>> {
        (1..=self.epochs.latest().map_or(0, |e| e.record.body.epoch))
            .filter_map(|epoch| self.epochs.get(epoch).map(|e| &e.record))
            .collect()
    }
}

/// The workspaces your account syncs, by the names their encrypted
/// profiles carry (§25), for a second computer to pick from.
pub async fn synced_workspaces(service: &mut SyncService) -> Result<Vec<(Id, String)>, SyncError> {
    let list = service.client().workspaces().await?;
    let mut named = Vec::new();
    for info in list.workspaces.0 {
        let Ok(remote) = service.open_workspace(info.workspace).await else {
            continue;
        };
        let Some(profile) = info.profile.map(|p| p.0) else {
            continue;
        };
        if let Ok(name) = check_profile(&remote, &profile, None) {
            named.push((info.workspace, name));
        }
    }
    Ok(named)
}
