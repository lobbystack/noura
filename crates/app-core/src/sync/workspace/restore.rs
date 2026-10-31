//! What a device does after a server restore (docs/workspace-format/
//! sync-v2.md §14). A generation above the last one it saw stops it writing
//! and applying. It then compares what it accepted with what the server
//! holds: the account chain heads it keeps, its policy head, its latest
//! epoch record and envelope. When the server lost any of them the device
//! stops with `sync_restore_lost` and you recover by hand. Otherwise the
//! server lost at most operations, checkpoints and blobs, and the device
//! rewinds to a checkpoint it checked, takes the server's positions again
//! and writes the lost content again from its files.
//!
//! The journal keeps every step, so a device that stops halfway resumes
//! where it left off.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sync_protocol::crypto::hash;
use sync_protocol::messages::EntryRecord;
use sync_protocol::records::{CheckpointBundle, Envelope, Epoch, Signed};
use sync_protocol::types::{Digest, Id};

use super::state::{BootstrapProgress, Content, Local, Log, Object, OpKey};
use super::{SyncError, SyncService, WorkspaceSync};
use crate::sync::account_heads::KeptCheckpointAnchor;

/// The longest a device waits before it writes a lost object again (§14).
pub const REWRITE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// How often a device that isn't in a restore's alert asks which blobs the
/// server misses: the server's check may mark one after the alert ended.
pub const MISSING_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// What the server lost, which names the way back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lost {
    /// A link of this device's own account chain: Reset devices.
    OwnAccount,
    /// A link of another member's chain: that person resets their devices
    /// and an owner or admin accepts it.
    MemberAccount(Id),
    /// A policy change this device accepted.
    Policy,
    /// An epoch record this device accepted.
    Epoch,
    /// The envelope this device accepted of the current epoch.
    Envelope,
    /// Every checkpoint this device reached in the domain.
    Checkpoints,
    /// Records or heads that disagree with a final head the device kept,
    /// of this device when the check names one.
    FinalHead(Option<Id>),
    /// The account chain under this device's account ID, so a new server
    /// took over without the old secret.
    Server,
    /// The owner closed the workspace.
    Closed,
}

/// A device's final head in a domain, kept for a change that ended its
/// write access there (§14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalHead {
    /// The device.
    pub device: Id,
    /// The change's revision.
    pub revision: u64,
    /// The sequence of the change's entry in this domain, when the device
    /// applied the entry itself.
    pub entry: u64,
    /// The final head's counter, 0 when the device held no record.
    pub counter: u64,
    /// The final head's digest.
    pub digest: Digest,
    /// The sequence of the entry of a later change that gave write access
    /// back.
    pub until: Option<u64>,
    /// The records the device fetched by chain position up to the final
    /// head for a rewind that starts below the entry: the pull takes no
    /// others at those counters.
    #[serde(default)]
    pub records: Vec<(u64, Digest)>,
}

/// A journal's last applied state of an object that the restore left
/// behind, which the device writes again once its wait ends.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rewrite {
    /// Unix milliseconds from which the device writes it.
    pub due: u64,
    /// The object as the journal held it.
    pub object: Object,
    /// The state to write again.
    pub content: Content,
    /// The live states the journal held, so a second restore during the
    /// wait starts from the same place.
    #[serde(default)]
    pub live: Vec<OpKey>,
}

/// What the journal keeps of a restore.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Restore {
    /// The generation the app last alerted you to.
    pub alerted: Option<u64>,
    /// What stopped this workspace, for good until you recover by hand.
    pub lost: Option<Lost>,
    /// A rollback the device caught in one generation, which holds it
    /// until the generation changes.
    pub rollback: Option<String>,
    /// A generation change whose comparison hasn't ended in a stop or a
    /// saved journal. It stays set through a failed request or a crash, so
    /// no later round runs on the old cursor and verifier.
    #[serde(default)]
    pub pending: bool,
    /// Whether the policy this device last accepted names its account as
    /// the workspace's owner, which holds through a stop.
    #[serde(default)]
    pub owner: bool,
    /// A `sync_chain_gap` holds the cursor until a page fills the gap.
    #[serde(default)]
    pub gap: bool,
    /// A generation change still owes the share links another revocation.
    #[serde(default)]
    pub revoke: bool,
    /// While the device rewinds, each object as the journal held it, with
    /// its live states at that moment.
    pub journal: Option<BTreeMap<Id, (Object, Vec<OpKey>)>>,
    /// The objects the device writes again, by `rid`.
    pub rewrites: BTreeMap<Id, Rewrite>,
    /// The paths the device wrote again since the restore.
    pub written_again: Vec<String>,
    /// Other branches of conflicts the journal held, and newer files a
    /// viewer device can't write again, for you to review.
    #[serde(default)]
    pub to_review: Vec<String>,
    /// Text blobs a snapshot of this device already stands in for.
    #[serde(default)]
    pub covered: BTreeSet<Id>,
    /// This device's rewrites that named no cover, which another device's
    /// rewrite may have raced, with the keys they took.
    #[serde(default)]
    pub uncovered: BTreeMap<Id, OpKey>,
}

/// A rollback code that holds the device until the generation changes.
fn names_reset(kind: &sync_protocol::records::PolicyKind, account: Id) -> bool {
    matches!(kind, sync_protocol::records::PolicyKind::AcceptReset { account: named }
        if *named == account)
}

fn holds(code: &str) -> bool {
    matches!(code, "sync_server_rollback" | "sync_policy_rollback")
}

/// A restricted domain with its latest accepted epoch and envelope.
type DomainKeys = (Id, Option<Signed<Epoch>>, Option<Signed<Envelope>>);

/// The fixed code of a kept rollback.
fn rollback_code(code: &str) -> &'static str {
    match code {
        "sync_policy_rollback" => "sync_policy_rollback",
        _ => "sync_server_rollback",
    }
}

/// A random number of seconds below `limit`.
fn random_below(limit: u64) -> u64 {
    if limit == 0 {
        return 0;
    }
    let bytes: [u8; 8] = sync_protocol::crypto::random_array().unwrap_or_default();
    u64::from_le_bytes(bytes) % limit
}

/// This writer's place among `writers` for `rid`: a hash of the two ranks
/// the writers the same way on every device, so one writer goes first and
/// the others find the object written (§14).
pub fn rank(rid: &Id, device: &Id, writers: &BTreeSet<Id>) -> usize {
    let score = |d: &Id| {
        let mut bytes = rid.0.to_vec();
        bytes.extend_from_slice(&d.0);
        hash(&bytes)
    };
    let own = score(device);
    writers.iter().filter(|w| score(w).0 < own.0).count()
}

/// The milliseconds a device of rank `rank` among `writers` waits, within
/// `limit` seconds: each rank gets its own stretch of the wait, and a
/// random point inside it. Stretches never collapse into one, however many
/// writers share the wait.
pub fn wait_for(rank: usize, writers: usize, limit: u64) -> u64 {
    let limit = limit.saturating_mul(1000);
    if limit == 0 {
        return 0;
    }
    let writers = writers.max(1) as u64;
    let stretch = (limit / writers).max(1);
    (rank as u64).min(writers - 1).saturating_mul(stretch) + random_below(stretch)
}

/// The current Unix time in milliseconds.
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Step 2's lag test (§14): the server lacks the `rid`, or the journal's
/// live operations cover all of the server's.
pub fn lags(journal_live: &[OpKey], known: impl Fn(&OpKey) -> bool, server_live: &[OpKey]) -> bool {
    let same = server_live.len() == journal_live.len()
        && server_live.iter().all(|key| journal_live.contains(key));
    !same
        && server_live
            .iter()
            .all(|key| known(key) && !journal_live.contains(key))
}

/// What a file that differs from the rebuilt state is (§14), as path and
/// revision compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileVerdict {
    /// An edit of yours over a state the server kept: the next capture
    /// signs it.
    Edit,
    /// The file holds what the journal applied, so it follows the server.
    Follow,
    /// A version for you to review.
    Review,
}

/// Judges a file by the states the journal last applied and the server's
/// live states.
pub fn judge_file<S: PartialEq>(
    applied: &[(S, Digest)],
    server: &[(S, Digest)],
    file: &(S, Digest),
) -> FileVerdict {
    if server.iter().any(|state| applied.contains(state)) {
        FileVerdict::Edit
    } else if applied.contains(file) {
        FileVerdict::Follow
    } else {
        FileVerdict::Review
    }
}

/// Whether this device's rewrite lost a race: the server took it as a
/// patch next to another device's state (§14).
pub fn lost_rewrite_race(live: &[OpKey], states: &[OpKey], own: &OpKey) -> bool {
    live.contains(own) && !states.contains(own)
}

/// Whether a bootstrap keeps a final head for a change it passed (§14).
pub fn bootstrap_keeps_final(listed: bool, regained: bool) -> bool {
    listed && !regained
}

impl WorkspaceSync {
    /// What stopped this workspace after a restore.
    pub fn restore_lost(&self) -> Option<&Lost> {
        self.state.restore.lost.as_ref()
    }

    /// The rollback that holds this workspace until the generation changes.
    pub fn rollback(&self) -> Option<&str> {
        self.state.restore.rollback.as_deref()
    }

    /// The generation the app alerts you to, once.
    pub fn restore_alert(&self) -> Option<u64> {
        self.state.restore.alerted
    }

    /// Ends the alert.
    pub fn dismiss_restore_alert(&mut self) -> Result<(), SyncError> {
        self.state.restore.alerted = None;
        self.state.restore.written_again.clear();
        self.state.restore.to_review.clear();
        self.save()
    }

    /// Takes what the generation change owes: whether the app revokes the
    /// device's share links again. The caller gives it back on a failure.
    pub fn revoke_due(&self) -> bool {
        self.state.restore.revoke
    }

    /// Records that the share links went out again.
    pub fn revoked_again(&mut self) -> Result<(), SyncError> {
        self.state.restore.revoke = false;
        self.save()
    }

    /// The paths the device wrote again since the restore.
    pub fn written_again(&self) -> &[String] {
        &self.state.restore.written_again
    }

    /// Sets the longest wait before the device writes lost objects again.
    pub fn set_restore_wait(&mut self, wait: std::time::Duration) {
        self.restore_wait = wait;
    }

    /// Whether the device applies and writes nothing here: from a
    /// generation change until the rewind ends, and while a stop or a
    /// rollback holds it (§14).
    pub(super) fn halted(&self) -> bool {
        let restore = &self.state.restore;
        restore.pending
            || restore.lost.is_some()
            || restore.rollback.is_some()
            || restore.journal.is_some()
    }

    /// Refuses a write the app asked for while the device is
    /// [`halted`](Self::halted).
    pub(super) fn refuse_halted(&self) -> Result<(), SyncError> {
        let restore = &self.state.restore;
        if restore.lost.is_some() {
            return Err(SyncError::Check("sync_restore_lost"));
        }
        if let Some(code) = restore.rollback.as_deref() {
            return Err(SyncError::Check(rollback_code(code)));
        }
        if self.halted() {
            return Err(SyncError::Check("sync_restore_generation"));
        }
        Ok(())
    }

    /// Keeps a rollback that holds the device, and returns the error.
    pub(super) fn note_failure(&mut self, error: SyncError) -> Result<(), SyncError> {
        if error.code() == Some("sync_restore_lost") {
            return self.stop_lost(Lost::FinalHead(None));
        }
        if let Some(code) = error.code()
            && holds(code)
        {
            self.state.restore.rollback = Some(code.to_owned());
            self.save()?;
        }
        if error.code() == Some("sync_chain_gap") && !self.state.restore.gap {
            self.state.restore.gap = true;
            self.save()?;
        }
        Err(error)
    }

    /// Runs before anything else in a round, and whenever a frame or a
    /// write meets a new generation. Returns whether the device may go on.
    /// A generation above the last one starts the comparison; a lower one
    /// reports `sync_server_rollback`. The comparison stays pending until
    /// it ends in a stop or a saved journal, so a failed request or a crash
    /// repeats it, and a generation that rises during a rewind compares
    /// again.
    pub(super) async fn restore_gate(
        &mut self,
        service: &mut SyncService,
    ) -> Result<bool, SyncError> {
        // A held device sends nothing else, so it asks for the generation.
        if self.state.restore.rollback.is_some() {
            service.client().ask_generation().await?;
        }
        let seen = service.client().restore_generation();
        if seen < self.state.generation {
            return Err(SyncError::Check("sync_server_rollback"));
        }
        if seen > self.state.generation {
            self.state.restore.alerted = Some(seen);
            self.state.restore.rollback = None;
            self.state.generation = seen;
            self.state.restore.pending = self.state.restore.lost.is_none();
            // Whatever state the workspace is in (§14).
            self.state.restore.revoke = true;
            self.save()?;
        }
        if let Some(Lost::MemberAccount(account)) = self.state.restore.lost {
            self.resume_after_reset(service, account).await?;
        }
        if self.state.restore.lost.is_some() {
            return Ok(false);
        }
        if let Some(code) = self.state.restore.rollback.clone() {
            return Err(SyncError::Check(rollback_code(&code)));
        }
        if self.state.restore.pending {
            self.compare(service).await?;
            if self.state.restore.lost.is_some() {
                return Ok(false);
            }
        }
        if self.state.restore.journal.is_some() {
            self.finish_rewind(service).await?;
        }
        Ok(self.state.restore.lost.is_none())
    }

    fn stop_lost(&mut self, lost: Lost) -> Result<(), SyncError> {
        self.state.restore.lost = Some(lost);
        self.state.restore.pending = false;
        self.state.restore.journal = None;
        self.state.restore.rewrites.clear();
        self.remote = None;
        self.save()
    }

    /// Compares what the device accepted with what the server holds, and
    /// stops or starts the rewind.
    async fn compare(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        if let Some(lost) = self.lost_state(service).await? {
            return self.stop_lost(lost);
        }
        self.rewind(service).await
    }

    /// Whether the server still holds the head this device keeps of
    /// `account`'s chains.
    async fn keeps_heads(&self, service: &mut SyncService, account: Id) -> Result<bool, SyncError> {
        for head in service.kept_heads().kept_of(&account) {
            let links = match service
                .client()
                .whole_chain(account, Some(head.genesis))
                .await
            {
                Ok((links, _)) => links,
                Err(SyncError::Refused { code, .. }) if code == "not_found" => return Ok(false),
                Err(error) => return Err(error),
            };
            let index = usize::try_from(head.seqno).unwrap_or(usize::MAX) - 1;
            if links.get(index).map(|link| link.digest()) != Some(head.head) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A workspace stopped only for `account`'s chain resumes, with the
    /// reconciliation, once the device verifies there an `accept_reset`
    /// that names the account's new chain (§14). The server's word starts
    /// the check and nothing more: the device verifies the policy chain,
    /// which must extend what it accepted, looks only at changes above its
    /// accepted head, and requires that the workspace now names that
    /// chain. Only then does the retired chain stop counting here.
    async fn resume_after_reset(
        &mut self,
        service: &mut SyncService,
        account: Id,
    ) -> Result<(), SyncError> {
        let workspace = self.state.workspace;
        let accepted = self.state.policy.last().map_or(0, |c| c.body.revision);
        let above = service.client().policy(workspace, accepted).await?;
        if !above
            .changes
            .0
            .iter()
            .any(|c| names_reset(&c.0.body.kind, account))
        {
            return Ok(());
        }
        service.kept_heads_mut().excuse_short(account);
        let resumed = self.verify_reset(service, account, accepted).await;
        if !matches!(resumed, Ok(true)) {
            service.kept_heads_mut().withdraw_excuse(&account);
        }
        resumed.map(|_| ())
    }

    /// Verifies the workspace again and ends the stop for `account` when a
    /// change above `accepted` names its new chain. Any other answer
    /// leaves the stop as it is.
    async fn verify_reset(
        &mut self,
        service: &mut SyncService,
        account: Id,
        accepted: u64,
    ) -> Result<bool, SyncError> {
        let workspace = self.state.workspace;
        let remote = match service.open_workspace(workspace).await {
            Ok(remote) => remote,
            // A forged or broken change leaves the stop as it is.
            Err(SyncError::Check(code)) => {
                tracing::warn!(code, "an accept_reset didn't verify");
                return Ok(false);
            }
            Err(SyncError::KeyMissing) => return Ok(false),
            Err(error) => return Err(error),
        };
        if let Some(kept) = self.state.policy.last()
            && remote.policy.change(kept.body.revision).map(|c| c.digest()) != Some(kept.digest())
        {
            return Ok(false);
        }
        let head = remote.policy.head().0;
        let named = (accepted + 1..=head)
            .filter_map(|revision| remote.policy.change(revision))
            .filter(|change| names_reset(&change.body.kind, account))
            .find_map(|change| change.body.heads.iter().find(|h| h.account == account))
            .map(|named| named.genesis);
        let current = remote
            .policy
            .head_snapshot()
            .members
            .get(&account)
            .map(|member| member.head.genesis);
        let (Some(genesis), true) = (named, named == current) else {
            return Ok(false);
        };
        service
            .kept_heads_mut()
            .drop_retired(&account, &genesis, workspace)?;
        self.state.restore.lost = None;
        self.state.restore.pending = true;
        self.save()?;
        Ok(true)
    }

    /// The verified workspace for an owner or admin device that answers
    /// the reset of the account this workspace stopped for (§14).
    pub async fn open_for_reset(
        &mut self,
        service: &mut SyncService,
    ) -> Result<crate::sync::RemoteWorkspace, SyncError> {
        if let Some(Lost::MemberAccount(account)) = self.state.restore.lost {
            service.kept_heads_mut().excuse_short(account);
        }
        service.open_workspace(self.state.workspace).await
    }

    /// What the server lost of the state this device accepted, if anything.
    async fn lost_state(&mut self, service: &mut SyncService) -> Result<Option<Lost>, SyncError> {
        let workspace = self.state.workspace;
        // A fresh server after a lost secret holds no chain under this ID.
        match service.client().whole_chain(self.state.account, None).await {
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {
                return Ok(Some(Lost::Server));
            }
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        if !self.keeps_heads(service, self.state.account).await? {
            return Ok(Some(Lost::OwnAccount));
        }
        // The accepted policy head comes first: a workspace that lost a
        // change is the owner's to set up again, whatever else went.
        let accepted = self.state.policy.last().cloned();
        if let Some(accepted) = &accepted {
            let revision = accepted.body.revision;
            let response = match service.client().policy(workspace, revision - 1).await {
                Ok(response) => response,
                Err(SyncError::Refused { code, .. }) if code == "not_found" => {
                    return Ok(Some(Lost::Closed));
                }
                Err(error) => return Err(error),
            };
            let held = response.changes.0.first().map(|change| change.0.digest());
            if held != Some(accepted.digest()) {
                return Ok(Some(Lost::Policy));
            }
        }
        let members: BTreeSet<Id> = accepted
            .iter()
            .flat_map(|change| change.body.heads.iter().map(|h| h.account))
            .chain(self.member_accounts())
            .filter(|account| *account != self.state.account)
            .collect();
        for account in members {
            if !self.keeps_heads(service, account).await? {
                return Ok(Some(Lost::MemberAccount(account)));
            }
        }
        // The latest accepted epoch record of each domain it reads, and its
        // envelope while that epoch stays the server's current one.
        let remote = match service.open_workspace(workspace).await {
            Ok(remote) => remote,
            Err(SyncError::KeyMissing) => return Ok(Some(Lost::Envelope)),
            Err(SyncError::Check("sync_key_mismatch")) => return Ok(Some(Lost::Epoch)),
            // The policy just passed, so the workspace stands: its epochs
            // went.
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {
                return Ok(Some(Lost::Epoch));
            }
            Err(error) => return Err(error),
        };
        if let Some(lost) = self
            .lost_epoch(
                service,
                self.state.domain,
                &remote.epochs,
                remote.envelope.as_ref(),
                self.state.epochs.last(),
                self.state.envelope.as_ref(),
            )
            .await?
        {
            return Ok(Some(lost));
        }
        let snapshot = remote.policy.head_snapshot();
        let device = self.credentials.keys.device();
        let recipient = self.credentials.keys.clone();
        let frames: Vec<DomainKeys> = self
            .state
            .restricted
            .iter()
            .filter(|(domain, _)| snapshot.can_read(&device, domain))
            .map(|(domain, frame)| {
                (
                    *domain,
                    frame.epochs.last().cloned(),
                    frame.envelope.clone(),
                )
            })
            .collect();
        for (domain, epoch, envelope) in frames {
            let loaded = service
                .load_keys(workspace, domain, &remote.policy, &recipient, true)
                .await;
            let (epochs, _, served) = match loaded {
                Ok(loaded) => loaded,
                Err(SyncError::KeyMissing) => return Ok(Some(Lost::Envelope)),
                Err(SyncError::Check("sync_key_mismatch")) => return Ok(Some(Lost::Epoch)),
                Err(SyncError::Refused { code, .. }) if code == "not_found" => {
                    return Ok(Some(Lost::Epoch));
                }
                Err(error) => return Err(error),
            };
            if let Some(lost) = self
                .lost_epoch(
                    service,
                    domain,
                    &epochs,
                    served.as_ref(),
                    epoch.as_ref(),
                    envelope.as_ref(),
                )
                .await?
            {
                return Ok(Some(lost));
            }
        }
        self.remote = Some(remote);
        Ok(None)
    }

    /// Compares one domain's latest accepted epoch record and envelope with
    /// the server's. An epoch a replacement ended counts as held only when
    /// the record the server still serves under that number is the same, or
    /// it serves none.
    async fn lost_epoch(
        &self,
        service: &mut SyncService,
        domain: Id,
        served: &sync_protocol::keys::EpochChain,
        served_envelope: Option<&Signed<Envelope>>,
        accepted: Option<&Signed<Epoch>>,
        envelope: Option<&Signed<Envelope>>,
    ) -> Result<Option<Lost>, SyncError> {
        let Some(accepted) = accepted else {
            return Ok(None);
        };
        let number = accepted.body.epoch;
        match served.get(number) {
            Some(held) if held.digest == accepted.digest() => {}
            Some(_) => return Ok(Some(Lost::Epoch)),
            None => {
                if served.ended_at(number).is_none() {
                    return Ok(Some(Lost::Epoch));
                }
                let answer = service
                    .client()
                    .epochs(self.state.workspace, domain, number)
                    .await;
                if let Ok(answer) = answer
                    && let Some(held) = answer.epochs.0.iter().find(|e| e.body.epoch == number)
                    && held.digest() != accepted.digest()
                {
                    return Ok(Some(Lost::Epoch));
                }
            }
        }
        let current = served.latest().map(|e| e.record.body.epoch);
        if current == Some(number)
            && let Some(envelope) = envelope
            && envelope.body.epoch == number
            && served_envelope.map(Signed::digest) != Some(envelope.digest())
        {
            return Ok(Some(Lost::Envelope));
        }
        Ok(None)
    }

    fn member_accounts(&self) -> Vec<Id> {
        self.remote
            .as_ref()
            .map(|remote| {
                remote
                    .policy
                    .head_snapshot()
                    .members
                    .keys()
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The rewind's start in the domain the device works in: the newest
    /// checkpoint it reached that the server still serves. `Ok(None)` stops
    /// the workspace; the inner `None` starts from sequence 0, for a device
    /// that reached none. Final heads pass before anything is rebuilt.
    async fn rewind_start(
        &mut self,
        service: &mut SyncService,
    ) -> Result<Option<Option<CheckpointBundle>>, SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let reached: Vec<_> = self
            .state
            .log
            .checkpoints
            .reached
            .values()
            .rev()
            .cloned()
            .collect();
        let mut start = None;
        for checkpoint in &reached {
            if let Some(entry) = service
                .client()
                .checkpoint_by_digest(workspace, domain, checkpoint.digest)
                .await?
                && let EntryRecord::Checkpoint(bundle) = entry.record
                && bundle.checkpoint.0.digest() == checkpoint.digest
            {
                start = Some(bundle);
                break;
            }
        }
        if start.is_none() && !reached.is_empty() {
            self.stop_lost(Lost::Checkpoints)?;
            return Ok(None);
        }
        let start_heads: BTreeMap<Id, (u64, Digest)> = start
            .as_ref()
            .map(|b| {
                b.heads
                    .iter()
                    .map(|h| (h.device, (h.counter, h.op_digest)))
                    .collect()
            })
            .unwrap_or_default();
        let (covered, previous) = start.as_ref().map_or((0, 0), |b| {
            let digest = b.checkpoint.0.body.previous;
            let revision = reached
                .iter()
                .find(|r| r.digest == digest)
                .map_or(0, |r| r.policy_revision);
            (b.checkpoint.0.body.covered_seq, revision)
        });
        let finals = self.state.log.final_heads.clone();
        for (index, kept) in finals.iter().enumerate() {
            let may_leave = self
                .remote
                .as_ref()
                .is_some_and(|remote| remote.policy.card_gone(&kept.device, previous));
            let checked = self
                .check_final_head(service, kept, covered, &start_heads, may_leave)
                .await?;
            let Some(records) = checked else {
                self.stop_lost(Lost::FinalHead(Some(kept.device)))?;
                return Ok(None);
            };
            self.state.log.final_heads[index].records = records;
        }
        Ok(Some(start))
    }

    /// Resets the domain the device works in to `start`: its chain heads,
    /// verifier and live set become the server's, except the final heads.
    /// The checkpoints it reached up to `start` stay, so a restore during
    /// the rewind finds them. The domain's log and its counters above every
    /// one it signed stay in `own.next`.
    fn reset_domain(
        &mut self,
        service: &mut SyncService,
        start: Option<&CheckpointBundle>,
    ) -> Result<(), SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let covered = start.map_or(0, |b| b.checkpoint.0.body.covered_seq);
        let finals = std::mem::take(&mut self.state.log.final_heads);
        let reached = std::mem::take(&mut self.state.log.checkpoints.reached);
        let mut log = Log::new(domain);
        log.final_heads = finals;
        if start.is_some() {
            log.checkpoints.reached = reached
                .into_iter()
                .filter(|(_, r)| r.covered_seq <= covered)
                .collect();
        }
        self.state.log = log;
        self.state.outbox.clear();
        self.state.own.head = None;
        self.state.own.acked = None;
        self.state.bootstrap_progress = None;
        self.state.bootstrap = start.is_some();
        if let Some(bundle) = start {
            let digest = bundle.checkpoint.0.digest();
            let generation = self.state.generation;
            if self.active.is_none() {
                service
                    .kept_heads_mut()
                    .set_checkpoint_anchor(KeptCheckpointAnchor {
                        workspace,
                        domain,
                        covered_seq: covered,
                        checkpoint: digest,
                        generation,
                    })?;
            }
            self.state.bootstrap_progress = Some(BootstrapProgress {
                checkpoint: digest,
                after: None,
                next_bucket: 0,
                pending: Vec::new(),
                live_objects: 0,
                last: BTreeMap::new(),
            });
        }
        self.want_entries();
        Ok(())
    }

    /// Step 1 in every domain the device reads: takes the server's
    /// positions again from the newest checkpoint it reached that the
    /// server still serves, in one step, once every domain's start passed
    /// its checks. A second restore during a rewind starts over from the
    /// journal the first one kept.
    async fn rewind(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let domain = self.state.domain;
        let mut starts = vec![(None, None)];
        let Some(start) = self.rewind_start(service).await? else {
            return Ok(());
        };
        starts[0].1 = start;
        let frames: Vec<Id> = self.state.restricted.keys().copied().collect();
        for restricted in frames {
            self.swap_domain(restricted);
            let found = self.rewind_start(service).await;
            self.swap_domain(restricted);
            match found? {
                Some(start) => starts.push((Some(restricted), start)),
                None => return Ok(()),
            }
        }
        // Before the pull, which needs those blobs, and after the checks
        // that can stop the workspace: a stopped one uploads nothing.
        if self.state.restore.journal.is_none() {
            self.upload_missing(service, false).await?;
        }

        // The journal keeps the objects as they stand, for step 2. What a
        // first rewind kept stays, and so does what still waited to go out.
        let mut journal = match self.state.restore.journal.take() {
            Some(journal) => journal,
            None => self
                .state
                .objects
                .keys()
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .map(|rid| {
                    (
                        rid,
                        (self.state.objects[&rid].clone(), self.live_states(&rid)),
                    )
                })
                .collect(),
        };
        for (rid, rewrite) in std::mem::take(&mut self.state.restore.rewrites) {
            journal.insert(rid, (rewrite.object, rewrite.live));
        }
        self.state.restore.journal = Some(journal);
        self.state.objects.clear();
        self.state.unreadable.clear();
        for (restricted, start) in &starts {
            match restricted {
                None => self.reset_domain(service, start.as_ref())?,
                Some(restricted) => {
                    self.swap_domain(*restricted);
                    let reset = self.reset_domain(service, start.as_ref());
                    self.swap_domain(*restricted);
                    reset?;
                }
            }
        }
        debug_assert_eq!(self.state.domain, domain);
        // A restore may lose a rename: a profile below the lost revision
        // reads as new again (§25).
        self.state.profile = None;
        let domains: Vec<Id> = self.parked.keys().copied().collect();
        for domain in domains {
            if let Some(runtime) = self.parked.get_mut(&domain) {
                *runtime = Default::default();
            }
        }
        self.state.restore.pending = false;
        self.save()?;
        self.finish_rewind(service).await
    }

    /// Whether the server's start agrees with a final head (§14, Final
    /// heads): a checkpoint that covers the change's entry lists exactly
    /// the final head, and one below it lists a head at or below the final
    /// head's counter that reaches it through the records it holds.
    async fn check_final_head(
        &self,
        service: &mut SyncService,
        kept: &FinalHead,
        covered: u64,
        heads: &BTreeMap<Id, (u64, Digest)>,
        may_leave: bool,
    ) -> Result<Option<Vec<(u64, Digest)>>, SyncError> {
        let passed = |ok: bool| ok.then(Vec::new);
        if kept.until.is_some_and(|until| until <= covered) {
            return Ok(passed(true));
        }
        let listed = heads.get(&kept.device).copied();
        if kept.entry <= covered {
            return Ok(passed(match listed {
                Some((counter, digest)) => counter == kept.counter && digest == kept.digest,
                None => kept.counter == 0 || may_leave,
            }));
        }
        let (mut counter, mut previous) = listed.unwrap_or((0, Digest::ZERO));
        if counter > kept.counter {
            return Ok(None);
        }
        let mut records = Vec::new();
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        while counter < kept.counter {
            let next = counter + 1;
            let Some(entry) = service
                .client()
                .chain_position(workspace, domain, kept.device, next)
                .await?
            else {
                return Ok(None);
            };
            let EntryRecord::Operation(op) = &entry.record else {
                return Ok(None);
            };
            if op.body.device != kept.device
                || op.body.previous != previous
                || op.body.counter <= counter
                || op.body.counter > kept.counter
            {
                return Ok(None);
            }
            previous = op.digest();
            counter = op.body.counter;
            records.push((counter, previous));
        }
        Ok(passed(previous == kept.digest).map(|_| records))
    }

    /// Pulls to the server's head from the start of the rewind, matches the
    /// files and decides, once, which objects lag behind the journal.
    async fn finish_rewind(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        if self.remote.is_none() {
            self.remote = Some(service.open_workspace(self.state.workspace).await?);
        }
        self.compare_own_chain(service).await?;
        self.matching = true;
        let caught_up = async {
            for _ in 0..10_000 {
                let (_, waiting) = self.pull(service, 0).await?;
                if waiting || self.state.log.cursor >= self.state.log.head {
                    return Ok(());
                }
            }
            Err(SyncError::Network("the domain's log didn't end".into()))
        }
        .await;
        self.matching = false;
        if let Err(error) = caught_up {
            // A failed check stops the workspace; files stay as they are.
            if matches!(error, SyncError::Check(code)
                if code.starts_with("sync_chain") || code == "sync_restore_lost")
            {
                return self.stop_lost(Lost::FinalHead(None));
            }
            return Err(error);
        }
        let Some(journal) = self.state.restore.journal.clone() else {
            return Ok(());
        };
        self.decide_lag(&journal)?;
        self.state.restore.journal = None;
        self.save()
    }

    /// The live contents of `object` at `keys`.
    fn contents(object: &Object, keys: &[OpKey]) -> Vec<Content> {
        keys.iter()
            .filter_map(|key| object.states.get(key)?.content.clone())
            .collect()
    }

    /// Step 2's lag test, taken once at the rewind: each `rid` the server
    /// lacks, or whose live operations on the server the journal's live
    /// operations all cover, is written again. A file that differs from
    /// the rebuilt state counts as a local edit when the server's live
    /// state is the one the journal last applied, as path and revision,
    /// and goes to review otherwise.
    fn decide_lag(
        &mut self,
        journal: &BTreeMap<Id, (Object, Vec<OpKey>)>,
    ) -> Result<(), SyncError> {
        let device = self.credentials.keys.device();
        let writers: BTreeSet<Id> = self
            .remote
            .as_ref()
            .map(|remote| {
                let snapshot = remote.policy.head_snapshot();
                snapshot
                    .devices
                    .keys()
                    .filter(|d| snapshot.can_write(d, &self.state.domain))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        let can_write = writers.contains(&device);
        let limit = self.restore_wait.as_secs();
        let now = unix_millis();
        let mut lag = BTreeSet::new();
        for (rid, (object, live)) in journal {
            let contents = Self::contents(object, live);
            let Some(content) = contents.first().cloned() else {
                continue;
            };
            let server_live = if self.state.objects.contains_key(rid) {
                self.live_states(rid)
            } else {
                Vec::new()
            };
            let behind = lags(live, |key| object.states.contains_key(key), &server_live);
            let text_behind = object.text.is_some() && !self.lost_text_records(rid)?.is_empty();
            if (!behind && !text_behind)
                || (content.delete && !self.state.objects.contains_key(rid))
            {
                continue;
            }
            lag.insert(*rid);
            // Every branch of a conflict the journal held goes to review;
            // the first is the one written again.
            for branch in contents.iter().skip(1).filter(|c| !c.delete) {
                self.keep_branch_for_review(branch)?;
            }
            if can_write {
                let wait = wait_for(rank(rid, &device, &writers), writers.len(), limit);
                self.state.restore.rewrites.insert(
                    *rid,
                    Rewrite {
                        due: now + wait,
                        object: object.clone(),
                        content,
                        live: live.clone(),
                    },
                );
            } else if !content.delete {
                // A viewer device writes nothing: its newer file stays, and
                // you decide what becomes of it.
                self.state.restore.to_review.push(content.path);
            }
        }
        self.lagging = lag.clone();
        self.match_local()?;
        self.lagging.clear();
        // A differing file over the state the journal last applied is an
        // edit you made, which the next capture signs.
        let held: Vec<Id> = self.state.held.keys().copied().collect();
        for rid in held {
            let Some((object, live)) = journal.get(&rid) else {
                continue;
            };
            let applied: Vec<(String, Digest)> = Self::contents(object, live)
                .into_iter()
                .map(|c| (c.path, c.revision))
                .collect();
            let server: Vec<(OpKey, Content)> = self
                .live_states(&rid)
                .into_iter()
                .filter_map(|key| {
                    let content = self.state.objects[&rid].states.get(&key)?.content.clone()?;
                    Some((key, content))
                })
                .collect();
            let states: Vec<(String, Digest)> = server
                .iter()
                .map(|(_, c)| (c.path.clone(), c.revision))
                .collect();
            let held = &self.state.held[&rid];
            let file = (held.path.clone(), held.revision);
            match judge_file(&applied, &states, &file) {
                // Against the state the journal applied, the next capture
                // finds your edit.
                FileVerdict::Edit => {
                    let matching = server.iter().find(|(_, c)| {
                        applied
                            .iter()
                            .any(|(path, revision)| *path == c.path && *revision == c.revision)
                    });
                    let Some((key, content)) = matching else {
                        continue;
                    };
                    let local = Local {
                        path: content.path.clone(),
                        revision: content.revision,
                        size: 0,
                        modified_ns: 0,
                        file_id: None,
                    };
                    let key = *key;
                    self.state.held.remove(&rid);
                    let object = self.state.objects.get_mut(&rid).expect("listed");
                    object.base = Some(key);
                    object.local = Some(local);
                }
                // The file holds no edit of yours: it follows the server's
                // newer state.
                FileVerdict::Follow => {
                    self.state.held.remove(&rid);
                    let kept = object.local.clone();
                    self.state.objects.get_mut(&rid).expect("listed").local = kept;
                    self.settle(rid, &BTreeMap::new())?;
                }
                FileVerdict::Review => {}
            }
        }
        self.save()
    }

    /// Lists a branch of a conflict the journal held for review, with a
    /// copy of its bytes in the archive when the journal kept them.
    fn keep_branch_for_review(&mut self, branch: &Content) -> Result<(), SyncError> {
        self.state.restore.to_review.push(branch.path.clone());
        let Some(kept) = branch.kept.as_deref() else {
            return Ok(());
        };
        let Ok(bytes) = self.journal.kept(kept, &branch.revision) else {
            return Ok(());
        };
        let file = format!(
            "{}/{}/{}",
            crate::sync::journal::ARCHIVE,
            self.state.replica.to_hex(),
            branch.revision.to_hex()
        );
        let journals = self.engine.root().join(crate::sync::journal::JOURNALS);
        let target = journals.join(&file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SyncError::Invalid(format!("can't keep the version: {e}")))?;
        }
        crate::sync::journal::durable_write(&target, &bytes)?;
        self.state.archived.push(super::rebuild::Archived {
            path: branch.path.clone(),
            file,
            revision: branch.revision,
        });
        Ok(())
    }

    /// The records of a text-mode note's journal that the server's chains
    /// no longer reach: writes the restore lost.
    fn lost_text_records(&self, rid: &Id) -> Result<Vec<OpKey>, SyncError> {
        let Some(file) = self.text_file(rid)? else {
            return Ok(Vec::new());
        };
        let log = self.log_of(self.state.objects.get(rid).and_then(|o| o.domain));
        Ok(file
            .records
            .iter()
            .filter(|record| log.verifier.head(&record.key.device).counter < record.key.counter)
            .map(|record| record.key)
            .collect())
    }

    /// Whether a rewrite's wait ended.
    pub(super) fn rewrites_due(&self) -> bool {
        let now = unix_millis();
        self.state.restore.rewrites.values().any(|r| r.due <= now)
    }

    /// Writes again each lost object whose wait ended and that still lags,
    /// as a new operation covering the server's live operations.
    pub(super) fn write_again_due(&mut self) -> Result<usize, SyncError> {
        let now = unix_millis();
        let due: Vec<Id> = self
            .state
            .restore
            .rewrites
            .iter()
            .filter(|(_, rewrite)| rewrite.due <= now)
            .map(|(rid, _)| *rid)
            .collect();
        let mut written = 0;
        for rid in due {
            let rewrite = self.state.restore.rewrites.remove(&rid).expect("listed");
            let view: BTreeSet<OpKey> = if self.state.objects.contains_key(&rid) {
                self.live_states(&rid).into_iter().collect()
            } else {
                BTreeSet::new()
            };
            // Another device wrote it first when the server's live state
            // moved past what the journal knew.
            let moved_on = view
                .iter()
                .any(|key| !rewrite.object.states.contains_key(key));
            if moved_on {
                continue;
            }
            let Rewrite {
                mut object,
                content,
                ..
            } = rewrite;
            let domain = object.domain;
            if content.delete {
                let covers = self.covers_of(&rid, &view);
                let path = object.local.as_ref().map(|l| l.path.clone());
                self.in_domain(domain, |sync| sync.sign_delete(rid, covers))?;
                written += 1;
                self.state.restore.written_again.extend(path);
                continue;
            }
            let known = self.state.objects.get(&rid).cloned();
            if let Some(known) = known {
                object.states = known.states;
            } else {
                object.states.clear();
            }
            object.base = None;
            object.own_highest = 0;
            let lineage = object.text;
            self.state.objects.insert(rid, object);
            if lineage.is_some() && self.text_file(&rid)?.is_some() {
                self.write_text_again(rid)?;
                written += 1;
                self.state.restore.written_again.push(content.path);
                continue;
            }
            let Ok(found) = self.found_on_disk(&content.path) else {
                continue;
            };
            let covers = self.covers_of(&rid, &view);
            let uncovered = covers.is_empty();
            // The same bytes keep their `blob_ref`, uploaded again byte for
            // byte when the server lacks it (§14).
            self.lent_blob = content
                .blob
                .clone()
                .filter(|_| found.revision == content.revision);
            let signed = self.in_domain(domain, |sync| sync.sign_file(rid, &found, covers));
            self.lent_blob = None;
            signed?;
            if uncovered && let Some(key) = self.state.objects[&rid].base {
                self.state.restore.uncovered.insert(rid, key);
            }
            written += 1;
            self.state.restore.written_again.push(content.path);
        }
        if written > 0 {
            self.save()?;
        }
        Ok(written)
    }

    /// Writes a text-mode note again as a `text_snapshot` under its lineage,
    /// merged with what the server took since the restore. Its `meta`
    /// entries name 0 for each write the restore lost, which therefore
    /// counts as a new write (§14, §22).
    fn write_text_again(&mut self, rid: Id) -> Result<(), SyncError> {
        let lost = self.lost_text_records(&rid)?;
        self.forget_text_positions(&rid, &lost)?;
        self.write_snapshot(rid, false)
    }

    /// Two rewrites raced when this device's record named no cover and the
    /// server already held another device's: the server took this one as a
    /// patch, which nobody can read. A state operation that covers both
    /// settles it (§14).
    pub(super) fn settle_rewrite_race(&mut self) -> Result<usize, SyncError> {
        let waiting = std::mem::take(&mut self.state.restore.uncovered);
        let mut written = 0;
        for (rid, key) in waiting {
            let Some(acked) = self
                .state
                .objects
                .get(&rid)
                .and_then(|o| o.states.get(&key))
            else {
                continue;
            };
            // Still on its way: the next round looks again.
            if acked.seq.is_none() {
                self.state.restore.uncovered.insert(rid, key);
                continue;
            }
            let live = self.live_ops(&rid);
            if !lost_rewrite_race(&live, &self.live_states(&rid), &key) {
                continue;
            }
            let all: BTreeSet<OpKey> = live.into_iter().collect();
            let path = self.state.objects[&rid]
                .local
                .as_ref()
                .map(|local| local.path.clone());
            let Some(found) = path.and_then(|path| self.found_on_disk(&path).ok()) else {
                continue;
            };
            let covers = self.covers_of(&rid, &all);
            let domain = self.state.objects[&rid].domain;
            self.in_domain(domain, |sync| sync.sign_file(rid, &found, covers))?;
            written += 1;
        }
        Ok(written)
    }

    /// Whether objects still wait to be written again, so the device holds
    /// its other writes.
    pub(super) fn rewrites_pending(&self) -> bool {
        !self.state.restore.rewrites.is_empty()
    }

    /// Set up sync again from a card of a reset owner account (§14): the
    /// card can't read the workspace in `engine`'s folder, whose journal
    /// belongs to the `retired` device, if this computer held one. The
    /// server closes the workspace only when no other owner or admin is
    /// active there; its refusal changes nothing here. The rest goes as
    /// in [`Self::set_up_again`], and a failure after the close can be
    /// tried again.
    pub async fn set_up_after_reset(
        service: &mut SyncService,
        engine: std::sync::Arc<local_core::WorkspaceEngine>,
        registry: &mut crate::sync::registry::Registry,
        retired: Option<Id>,
    ) -> Result<Self, SyncError> {
        let entry = registry
            .of_folder(engine.root())
            .cloned()
            .ok_or_else(|| SyncError::Invalid("this folder doesn't sync".into()))?;
        match service.client().close_workspace(entry.workspace).await {
            Ok(()) => {}
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {}
            Err(error) => return Err(error),
        }
        let secrets = service.secrets();
        let old = retired.and_then(|device| {
            let journal =
                crate::sync::journal::Journal::open(engine.root(), &device, &*secrets).ok()?;
            let state: super::state::State = journal
                .read(crate::sync::journal::STATE_FILE)
                .ok()
                .flatten()?;
            Some((journal, state))
        });
        let name = old.as_ref().map_or_else(
            || {
                engine
                    .root()
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
            },
            |(_, state)| state.name.clone(),
        );
        let mut remote = service.create_workspace(&name).await?;
        let (_, profile) = super::profile_of(service, &mut remote).await?;
        let mut archived = Vec::new();
        if let Some((journal, state)) = &old {
            archived = Self::archive(journal, Some(state), &state.replica)?;
            archived.extend(state.archived.clone());
        }
        drop(old);
        service.kept_heads_mut().forget_workspace(entry.workspace)?;
        registry.remove(&entry.server, &entry.account, &entry.workspace)?;
        let server = service.client().base().to_owned();
        let replica = registry.connect(
            &server,
            service.credentials().account,
            remote.workspace,
            engine.root(),
        )?;
        let generation = service.client().restore_generation();
        let mut fresh = Self::start(
            service, engine, remote, server, replica, generation, &name, profile,
        )?;
        fresh.state.archived = archived;
        fresh.save()?;
        Ok(fresh)
    }

    /// Set up sync again (§14): closes this workspace on the server, which
    /// answers `not_found` from then on, carries the review store over
    /// through the archive, and turns sync on for this folder as a new
    /// workspace, with a new ID, genesis and keys. An owner device can do
    /// this whether or not it found the loss itself. You invite the members
    /// again.
    ///
    /// This workspace stays as it is until the new one exists: a failure
    /// leaves it stopped as closed, with its journal, and you try again.
    pub async fn set_up_again(
        &mut self,
        service: &mut SyncService,
        registry: &mut crate::sync::registry::Registry,
    ) -> Result<Self, SyncError> {
        if !self.may_set_up_again() {
            return Err(SyncError::Invalid(
                "Only the workspace's owner can set it up again.".into(),
            ));
        }
        let workspace = self.state.workspace;
        match service.client().close_workspace(workspace).await {
            Ok(()) => {}
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {}
            Err(error) => return Err(error),
        }
        self.stop_lost(Lost::Closed)?;
        let name = self.state.name.clone();
        let mut remote = service.create_workspace(&name).await?;
        let (_, profile) = super::profile_of(service, &mut remote).await?;
        let mut archived = Self::archive(&self.journal, Some(&self.state), &self.state.replica)?;
        archived.extend(self.state.archived.clone());
        service.kept_heads_mut().forget_workspace(workspace)?;
        registry.remove(&self.state.server, &self.state.account, &workspace)?;
        let server = service.client().base().to_owned();
        let replica = registry.connect(
            &server,
            service.credentials().account,
            remote.workspace,
            self.engine.root(),
        )?;
        let generation = service.client().restore_generation();
        self.journal.release();
        let mut fresh = Self::start(
            service,
            self.engine.clone(),
            remote,
            server,
            replica,
            generation,
            &name,
            profile,
        )?;
        fresh.state.archived = archived;
        fresh.save()?;
        Ok(fresh)
    }

    /// Reports the owner's closing as a stop, when the server answers
    /// `not_found` for the whole workspace.
    pub(super) async fn note_closed(
        &mut self,
        service: &mut SyncService,
    ) -> Result<bool, SyncError> {
        match service.client().policy(self.state.workspace, 0).await {
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {
                self.stop_lost(Lost::Closed)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Whether the app may offer Set up sync again here: the policy this
    /// device last accepted names its account as the owner.
    pub fn may_set_up_again(&self) -> bool {
        self.state.restore.owner
    }

    /// Keeps a final head for each change the device bootstrapped past
    /// (§14): only when the checkpoint lists the device and no change up
    /// to the checkpoint's `policy_revision` gave it write access again.
    /// The device keeps none otherwise, the split-view residual of §30.
    pub(super) fn keep_bootstrap_finals(
        &mut self,
        revision: u64,
        covered: u64,
        heads: &[sync_protocol::records::Ref],
    ) {
        let Some(remote) = self.remote.as_ref() else {
            return;
        };
        let domain = self.state.domain;
        let mut finals = Vec::new();
        for at in 2..=revision {
            let (Some(prev), Some(next)) = (remote.policy.at(at - 1), remote.policy.at(at)) else {
                continue;
            };
            for head in heads {
                let ended =
                    prev.can_write(&head.device, &domain) && !next.can_write(&head.device, &domain);
                let regained = (at + 1..=revision).any(|later| {
                    remote
                        .policy
                        .at(later)
                        .is_some_and(|s| s.can_write(&head.device, &domain))
                });
                if ended && bootstrap_keeps_final(true, regained) {
                    finals.push(FinalHead {
                        device: head.device,
                        revision: at,
                        entry: covered,
                        counter: head.counter,
                        digest: head.op_digest,
                        until: None,
                        records: Vec::new(),
                    });
                }
            }
        }
        for kept in finals {
            let known = self
                .state
                .log
                .final_heads
                .iter()
                .any(|k| k.device == kept.device && k.revision == kept.revision);
            if !known {
                self.state.log.final_heads.push(kept);
            }
        }
    }

    /// Uploads again, byte for byte under the same ID, each blob the
    /// restore left missing whose file this device holds at the listing's
    /// revision (§19, §21). The blob's key and ID fix its ciphertext.
    pub(super) async fn upload_missing(
        &mut self,
        service: &mut SyncService,
        snapshots: bool,
    ) -> Result<usize, SyncError> {
        let workspace = self.state.workspace;
        let device = self.credentials.keys.device();
        // A viewer device writes nothing, blobs included (§14).
        let writer = |domain: &Id, sync: &Self| {
            sync.remote
                .as_ref()
                .is_none_or(|r| r.policy.head_snapshot().can_write(&device, domain))
        };
        let missing = service.client().missing_blobs(workspace).await?.blobs.0;
        let mut uploaded = 0;
        for blob in missing {
            let found = self.state.objects.values().find_map(|object| {
                let local = object.local.as_ref()?;
                object.states.values().find_map(|state| {
                    let content = state.content.as_ref()?;
                    let blob_ref = content.blob.as_ref()?;
                    (blob_ref.blob == blob && content.revision == local.revision).then(|| {
                        (
                            local.path.clone(),
                            blob_ref.clone(),
                            object.domain.unwrap_or(self.state.domain),
                        )
                    })
                })
            });
            let Some((path, blob_ref, domain)) = found else {
                // No file holds a text record's yrs bytes: a writer that
                // holds the note's document writes a snapshot that covers
                // the operation instead (§14, §21).
                if snapshots
                    && !self.state.restore.covered.contains(&blob)
                    && let Some(rid) = self.text_note_of_blob(&blob)?
                {
                    let domain = self.state.objects[&rid].domain.unwrap_or(self.state.domain);
                    if writer(&domain, self) {
                        self.write_snapshot(rid, false)?;
                        self.state.restore.covered.insert(blob);
                        self.save()?;
                    }
                }
                continue;
            };
            if !writer(&domain, self) {
                continue;
            }
            let Some(file) = self.engine.open_synced(&path).map_err(super::core)? else {
                continue;
            };
            let len = file
                .metadata()
                .map_err(|e| SyncError::Invalid(format!("can't read the file: {e}")))?
                .len();
            let upload = super::state::upload_path(&blob);
            let key = blob_ref.blob_key.clone();
            let encrypted = self.journal.keep_with(&upload, |out| {
                sync_protocol::blob::encrypt_blob_to(
                    std::io::BufReader::with_capacity(1 << 16, file),
                    len,
                    blob,
                    key,
                    out,
                )
                .map_err(|e| SyncError::Invalid(e.to_string()))
            });
            let same = matches!(&encrypted, Ok((rebuilt, _))
                if rebuilt.ciphertext_digest == blob_ref.ciphertext_digest);
            if same {
                let path = self.journal.dir().join(&upload);
                match service
                    .client()
                    .upload_blob(workspace, domain, &blob_ref, &path)
                    .await
                {
                    Ok(()) => uploaded += 1,
                    // Another device took the upload, or the server refuses
                    // this one: a later check asks again.
                    Err(SyncError::Refused { code, .. }) => {
                        tracing::warn!(code = %code, "a lost blob's upload was refused");
                    }
                    Err(error) => {
                        self.journal.forget(&upload);
                        return Err(error);
                    }
                }
            }
            self.journal.forget(&upload);
        }
        Ok(uploaded)
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use serde_json::Value;

    const LIVE: &str =
        include_str!("../../../../../docs/workspace-format/fixtures/sync-v2-live.json");
    const CHAIN: &str =
        include_str!("../../../../../docs/workspace-format/fixtures/sync-v2-chain.json");
    const PROFILE: &str =
        include_str!("../../../../../docs/workspace-format/fixtures/sync-v2-profile.json");

    fn cases(text: &str) -> Vec<Value> {
        let file: Value = serde_json::from_str(text).expect("the fixture reads");
        file["restore_cases"].as_array().expect("cases").clone()
    }

    /// `a2` as device a, counter 2.
    fn key(text: &str) -> OpKey {
        let (device, counter) = text.split_at(1);
        OpKey {
            device: Id([device.as_bytes()[0]; 16]),
            counter: counter.parse().expect("a counter"),
        }
    }

    fn keys(value: &Value) -> Vec<OpKey> {
        value
            .as_array()
            .expect("a list")
            .iter()
            .map(|v| key(v.as_str().expect("a key")))
            .collect()
    }

    fn state(value: &Value) -> (String, Digest) {
        let path = value[0].as_str().expect("a path").to_owned();
        let revision = value[1].as_str().expect("a revision").as_bytes()[1];
        (path, Digest([revision; 32]))
    }

    fn states(value: &Value) -> Vec<(String, Digest)> {
        value
            .as_array()
            .expect("a list")
            .iter()
            .map(state)
            .collect()
    }

    #[test]
    fn the_live_restore_cases_hold() {
        for case in cases(LIVE) {
            let name = case["name"].as_str().expect("a name");
            match case["kind"].as_str().expect("a kind") {
                "lag" => {
                    let known = keys(&case["journal_states"]);
                    assert_eq!(
                        lags(
                            &keys(&case["journal_live"]),
                            |k| known.contains(k),
                            &keys(&case["server_live"])
                        ),
                        case["expect"].as_bool().expect("a verdict"),
                        "{name}"
                    );
                }
                "file" => {
                    let verdict = judge_file(
                        &states(&case["applied"]),
                        &states(&case["server"]),
                        &state(&case["file"]),
                    );
                    let expected = match case["expect"].as_str().expect("a verdict") {
                        "edit" => FileVerdict::Edit,
                        "follow" => FileVerdict::Follow,
                        _ => FileVerdict::Review,
                    };
                    assert_eq!(verdict, expected, "{name}");
                }
                "race" => assert_eq!(
                    lost_rewrite_race(
                        &keys(&case["live"]),
                        &keys(&case["states"]),
                        &key(case["own"].as_str().expect("a key"))
                    ),
                    case["expect"].as_bool().expect("a verdict"),
                    "{name}"
                ),
                other => panic!("{other}"),
            }
        }
    }

    fn number(value: &Value) -> u64 {
        value.as_u64().expect("a number")
    }

    fn digest(value: &Value) -> Digest {
        let byte = number(value) as u8;
        if byte == 0 {
            Digest::ZERO
        } else {
            Digest([byte; 32])
        }
    }

    fn kept(value: &Value) -> FinalHead {
        FinalHead {
            device: Id([1; 16]),
            revision: 2,
            entry: number(&value["entry"]),
            counter: number(&value["counter"]),
            digest: digest(&value["digest"]),
            until: value.get("until").map(number),
            records: value.get("records").map_or_else(Vec::new, |records| {
                records
                    .as_array()
                    .expect("a list")
                    .iter()
                    .map(|r| (number(&r[0]), digest(&r[1])))
                    .collect()
            }),
        }
    }

    fn outcome(result: Result<(), SyncError>) -> String {
        match result {
            Ok(()) => "pass".to_owned(),
            Err(error) => error.code().unwrap_or_default().to_owned(),
        }
    }

    #[test]
    fn the_chain_restore_cases_hold() {
        for case in cases(CHAIN) {
            let name = case["name"].as_str().expect("a name");
            let device = Id([1; 16]);
            let mut log = Log::new(Id([2; 16]));
            match case["kind"].as_str().expect("a kind") {
                "final_checkpoint" => {
                    log.final_heads.push(kept(&case["final"]));
                    let heads: Vec<sync_protocol::records::Ref> = case["heads"]
                        .as_array()
                        .expect("a list")
                        .iter()
                        .map(|h| sync_protocol::records::Ref {
                            device,
                            counter: number(&h[0]),
                            seq: 1,
                            op_digest: digest(&h[1]),
                        })
                        .collect();
                    let leaves = case["may_leave"].as_bool().unwrap_or(false);
                    let covered = number(&case["covered"]);
                    let result = log.check_final_heads(covered, &heads, |_| leaves);
                    assert_eq!(outcome(result), case["expect"], "{name}");
                }
                "final_record" => {
                    log.final_heads.push(kept(&case["final"]));
                    let result = log.check_final_record(
                        &device,
                        number(&case["record"][0]),
                        &digest(&case["record"][1]),
                        number(&case["seq"]),
                    );
                    assert_eq!(outcome(result), case["expect"], "{name}");
                }
                "bootstrap_final" => assert_eq!(
                    bootstrap_keeps_final(
                        case["listed"].as_bool().expect("a flag"),
                        case["regained"].as_bool().expect("a flag")
                    ),
                    case["expect"].as_bool().expect("a verdict"),
                    "{name}"
                ),
                other => panic!("{other}"),
            }
        }
    }

    /// The client forgets its accepted profile revision once a comparison
    /// passed, so the server's lower revision reads as new (§25).
    #[test]
    fn the_profile_restore_cases_hold() {
        for case in cases(PROFILE) {
            let name = case["name"].as_str().expect("a name");
            let accepted = number(&case["accepted"]["revision"]);
            let forgets = case["forgets_after_comparison"].as_bool().expect("a flag");
            let mark = (!forgets).then_some(accepted);
            let refused = mark.is_some_and(|mark| number(&case["server_revision"]) < mark);
            let expected = if refused {
                "sync_profile_rollback"
            } else {
                "accepted"
            };
            assert_eq!(case["expect"], expected, "{name}");
        }
    }
}
