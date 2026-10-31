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
use sync_protocol::types::{Digest, Id};

use super::state::{BootstrapProgress, Content, Log, Object, OpKey};
use super::{SyncError, SyncService, WorkspaceSync, unix_now};
use crate::sync::account_heads::KeptCheckpointAnchor;

/// The longest a device waits before it writes a lost object again (§14).
pub const REWRITE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

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
    /// Records or heads that disagree with a final head the device kept.
    FinalHead(Id),
    /// The account chain under this device's account ID, so a new server
    /// took over without the old secret.
    Server,
    /// The owner closed the workspace.
    Closed,
}

/// A device's final head in a domain, kept for a change that ended its
/// write access there (§14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// A journal's last applied state of an object that the restore left
/// behind, which the device writes again once its wait ends.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rewrite {
    /// Unix seconds from which the device writes it.
    pub due: u64,
    /// The object as the journal held it.
    pub object: Object,
    /// The state to write again.
    pub content: Content,
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
    /// While the device rewinds, each object as the journal held it, with
    /// its live states at that moment.
    pub journal: Option<BTreeMap<Id, (Object, Vec<OpKey>)>>,
    /// The objects the device writes again, by `rid`.
    pub rewrites: BTreeMap<Id, Rewrite>,
    /// The paths the device wrote again since the restore.
    pub written_again: Vec<String>,
}

/// A rollback code that holds the device until the generation changes.
fn holds(code: &str) -> bool {
    matches!(code, "sync_server_rollback" | "sync_policy_rollback")
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

/// The seconds a device of rank `rank` among `writers` waits, within
/// `limit`: each rank gets its own stretch of the wait, and a random point
/// inside it.
pub fn wait_for(rank: usize, writers: usize, limit: u64) -> u64 {
    if limit == 0 {
        return 0;
    }
    let writers = writers.max(1) as u64;
    let stretch = (limit / writers).max(1);
    (rank as u64).saturating_mul(stretch).min(limit - 1) + random_below(stretch).min(limit - 1)
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

    /// Keeps a rollback that holds the device, and returns the error.
    pub(super) fn note_failure(&mut self, error: SyncError) -> Result<(), SyncError> {
        if error.code() == Some("sync_restore_lost") {
            return self.stop_lost(Lost::FinalHead(self.credentials.keys.device()));
        }
        if let Some(code) = error.code()
            && holds(code)
        {
            self.state.restore.rollback = Some(code.to_owned());
            self.save()?;
        }
        Err(error)
    }

    /// Runs before anything else in a round. Returns whether the device
    /// may go on. A generation above the last one starts the comparison; a
    /// lower one reports `sync_server_rollback`.
    pub(super) async fn restore_gate(
        &mut self,
        service: &mut SyncService,
    ) -> Result<bool, SyncError> {
        let seen = service.client().restore_generation();
        if seen < self.state.generation {
            return Err(SyncError::Check("sync_server_rollback"));
        }
        if seen > self.state.generation {
            self.state.restore.alerted = Some(seen);
            self.state.restore.rollback = None;
            self.state.generation = seen;
            if self.state.restore.lost.is_none() && self.state.restore.journal.is_none() {
                self.save()?;
                self.compare(service).await?;
            } else {
                self.save()?;
            }
        }
        if let Some(Lost::MemberAccount(account)) = self.state.restore.lost.clone() {
            self.resume_after_reset(service, account).await?;
        }
        if self.state.restore.lost.is_some() {
            return Ok(false);
        }
        if let Some(code) = self.state.restore.rollback.clone() {
            let code = match code.as_str() {
                "sync_policy_rollback" => "sync_policy_rollback",
                _ => "sync_server_rollback",
            };
            return Err(SyncError::Check(code));
        }
        if self.state.restore.journal.is_some() {
            self.finish_rewind(service).await?;
        }
        Ok(true)
    }

    fn stop_lost(&mut self, lost: Lost) -> Result<(), SyncError> {
        self.state.restore.lost = Some(lost);
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
        // A restore may lose a rename: a profile below the lost revision
        // reads as new again (§25).
        self.state.profile = None;
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
    /// that names the account's new chain (§14). The retired chain then no
    /// longer counts here.
    async fn resume_after_reset(
        &mut self,
        service: &mut SyncService,
        account: Id,
    ) -> Result<(), SyncError> {
        let changes = service.client().policy(self.state.workspace, 0).await?;
        let accepted = changes.changes.0.iter().find_map(|change| {
            let body = &change.0.body;
            matches!(body.kind, sync_protocol::records::PolicyKind::AcceptReset { account: named }
                if named == account)
            .then(|| body.heads.iter().find(|h| h.account == account))
            .flatten()
            .map(|head| head.genesis)
        });
        let Some(genesis) = accepted else {
            return Ok(());
        };
        service
            .kept_heads_mut()
            .drop_retired(&account, &genesis, self.state.workspace)?;
        self.state.restore.lost = None;
        self.save()?;
        self.compare(service).await
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
        let accepted = self.state.policy.last().cloned();
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
        // The accepted policy head.
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
        // The latest accepted epoch record, and its envelope while that
        // epoch stays the server's current one.
        let remote = match service.open_workspace(workspace).await {
            Ok(remote) => remote,
            Err(SyncError::KeyMissing) => return Ok(Some(Lost::Envelope)),
            Err(SyncError::Check("sync_key_mismatch")) => return Ok(Some(Lost::Epoch)),
            Err(error) => return Err(error),
        };
        if let Some(epoch) = self.state.epochs.last() {
            let number = epoch.body.epoch;
            let kept = remote
                .epochs
                .get(number)
                .is_some_and(|e| e.digest == epoch.digest())
                || remote.epochs.ended_at(number).is_some();
            if !kept {
                return Ok(Some(Lost::Epoch));
            }
            let current = remote.epochs.latest().map(|e| e.record.body.epoch);
            if current == Some(number)
                && let Some(envelope) = &self.state.envelope
                && envelope.body.epoch == number
                && remote.envelope.as_ref().map(|e| e.digest()) != Some(envelope.digest())
            {
                return Ok(Some(Lost::Envelope));
            }
        }
        self.remote = Some(remote);
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

    /// Step 1: takes the server's positions again from the newest
    /// checkpoint the device reached that the server still serves.
    async fn rewind(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        // Before the pull below, which needs those blobs.
        self.upload_missing(service).await?;
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
            return self.stop_lost(Lost::Checkpoints);
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
        let covered = start
            .as_ref()
            .map_or(0, |b| b.checkpoint.0.body.covered_seq);
        // The final heads, before anything the server serves applies.
        let finals = self.state.log.final_heads.clone();
        for kept in &finals {
            if !self
                .check_final_head(service, kept, covered, &start_heads)
                .await?
            {
                return self.stop_lost(Lost::FinalHead(kept.device));
            }
        }

        // The journal keeps the objects as they stand, for step 2.
        let journal: BTreeMap<Id, (Object, Vec<OpKey>)> = self
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
            .collect();
        self.state.restore.journal = Some(journal);
        let mut log = Log::new(domain);
        log.final_heads = finals;
        self.state.log = log;
        self.state.objects.clear();
        self.state.outbox.clear();
        self.state.unreadable.clear();
        self.state.own.head = None;
        self.state.own.acked = None;
        self.state.restricted.clear();
        self.parked.clear();
        self.state.bootstrap_progress = None;
        self.state.bootstrap = start.is_some();
        if let Some(bundle) = &start {
            let digest = bundle.checkpoint.0.digest();
            let generation = self.state.generation;
            service
                .kept_heads_mut()
                .set_checkpoint_anchor(KeptCheckpointAnchor {
                    workspace,
                    domain,
                    covered_seq: covered,
                    checkpoint: digest,
                    generation,
                })?;
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
    ) -> Result<bool, SyncError> {
        if kept.until.is_some_and(|until| until <= covered) {
            return Ok(true);
        }
        let listed = heads.get(&kept.device).copied();
        if kept.entry <= covered {
            return Ok(match listed {
                Some((counter, digest)) => counter == kept.counter && digest == kept.digest,
                None => true,
            });
        }
        let (mut counter, mut previous) = listed.unwrap_or((0, Digest::ZERO));
        if counter > kept.counter {
            return Ok(false);
        }
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        while counter < kept.counter {
            let next = counter + 1;
            let Some(entry) = service
                .client()
                .chain_position(workspace, domain, kept.device, next)
                .await?
            else {
                return Ok(false);
            };
            let EntryRecord::Operation(op) = &entry.record else {
                return Ok(false);
            };
            if op.body.device != kept.device
                || op.body.previous != previous
                || op.body.counter <= counter
                || op.body.counter > kept.counter
            {
                return Ok(false);
            }
            previous = op.digest();
            counter = op.body.counter;
        }
        Ok(previous == kept.digest)
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
                return self.stop_lost(Lost::FinalHead(self.credentials.keys.device()));
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
        let now = unix_now();
        let mut lag = BTreeSet::new();
        for (rid, (object, live)) in journal {
            let Some(content) = Self::contents(object, live).into_iter().next() else {
                continue;
            };
            let server_live = if self.state.objects.contains_key(rid) {
                self.live_states(rid)
            } else {
                Vec::new()
            };
            let same_keys =
                server_live.len() == live.len() && server_live.iter().all(|key| live.contains(key));
            let behind = !same_keys
                && server_live
                    .iter()
                    .all(|key| object.states.contains_key(key) && !live.contains(key));
            if !behind || (content.delete && !self.state.objects.contains_key(rid)) {
                continue;
            }
            lag.insert(*rid);
            if can_write {
                let wait = wait_for(rank(rid, &device, &writers), writers.len(), limit);
                self.state.restore.rewrites.insert(
                    *rid,
                    Rewrite {
                        due: now + wait,
                        object: object.clone(),
                        content,
                    },
                );
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
            let applied = Self::contents(object, live);
            let server: Vec<(OpKey, Content)> = self
                .live_states(&rid)
                .into_iter()
                .filter_map(|key| {
                    let content = self.state.objects[&rid].states.get(&key)?.content.clone()?;
                    Some((key, content))
                })
                .collect();
            let matching = server.iter().find(|(_, c)| {
                applied
                    .iter()
                    .any(|a| a.path == c.path && a.revision == c.revision)
            });
            if let Some((key, _)) = matching {
                let key = *key;
                self.state.held.remove(&rid);
                self.state.objects.get_mut(&rid).expect("listed").base = Some(key);
            }
        }
        self.save()
    }

    /// Writes again each lost object whose wait ended and that still lags,
    /// as a new operation covering the server's live operations.
    pub(super) fn write_again_due(&mut self) -> Result<usize, SyncError> {
        let now = unix_now();
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
            if content.delete {
                let covers = self.covers_of(&rid, &view);
                let path = object.local.as_ref().map(|l| l.path.clone());
                self.sign_delete(rid, covers)?;
                written += 1;
                self.state.restore.written_again.extend(path);
                continue;
            }
            let Ok(found) = self.found_on_disk(&content.path) else {
                continue;
            };
            let known = self.state.objects.get(&rid).cloned();
            if let Some(known) = known {
                object.states = known.states;
            } else {
                object.states.clear();
            }
            object.base = None;
            object.own_highest = 0;
            self.state.objects.insert(rid, object);
            let covers = self.covers_of(&rid, &view);
            self.sign_file(rid, &found, covers)?;
            written += 1;
            self.state.restore.written_again.push(content.path);
        }
        if written > 0 {
            self.save()?;
        }
        Ok(written)
    }

    /// Whether objects still wait to be written again, so the device holds
    /// its other writes.
    pub(super) fn rewrites_pending(&self) -> bool {
        !self.state.restore.rewrites.is_empty()
    }

    /// Set up sync again (§14): closes this workspace on the server, which
    /// answers `not_found` from then on, carries the review store over
    /// through the archive, and turns sync on for this folder as a new
    /// workspace, with a new ID, genesis and keys. An owner device can do
    /// this whether or not it found the loss itself. You invite the members
    /// again.
    pub async fn set_up_again(
        self,
        service: &mut SyncService,
        registry: &mut crate::sync::registry::Registry,
    ) -> Result<Self, SyncError> {
        match service.client().close_workspace(self.state.workspace).await {
            Ok(()) => {}
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {}
            Err(error) => return Err(error),
        }
        let name = self.state.name.clone();
        let engine = self.engine.clone();
        let Self { journal, state, .. } = self;
        let mut archived = Self::archive(journal, Some(&state), &state.replica)?;
        archived.extend(state.archived.clone());
        service.kept_heads_mut().forget_workspace(state.workspace)?;
        registry.remove(&state.server, &state.account, &state.workspace)?;
        let mut fresh = Self::turn_on(service, engine, &name, registry).await?;
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

    /// Whether the app may offer Set up sync again here: this device's
    /// account owns the workspace.
    pub fn may_set_up_again(&self) -> bool {
        self.state
            .policy
            .iter()
            .rev()
            .find_map(|change| {
                change
                    .body
                    .heads
                    .iter()
                    .any(|h| h.account == self.state.account)
                    .then_some(())
            })
            .is_some()
            && self.remote.as_ref().is_none_or(|remote| {
                remote
                    .policy
                    .head_snapshot()
                    .members
                    .get(&self.state.account)
                    .is_some_and(|m| m.role == sync_protocol::records::Role::Owner)
            })
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
                if ended && !regained {
                    finals.push(FinalHead {
                        device: head.device,
                        revision: at,
                        entry: covered,
                        counter: head.counter,
                        digest: head.op_digest,
                        until: None,
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
    ) -> Result<usize, SyncError> {
        let workspace = self.state.workspace;
        let missing = service.client().missing_blobs(workspace).await?.blobs.0;
        let mut uploaded = 0;
        for blob in missing {
            let found = self.state.objects.values().find_map(|object| {
                let local = object.local.as_ref()?;
                object.states.values().find_map(|state| {
                    let content = state.content.as_ref()?;
                    let blob_ref = content.blob.as_ref()?;
                    (blob_ref.blob == blob && content.revision == local.revision)
                        .then(|| (local.path.clone(), blob_ref.clone()))
                })
            });
            let Some((path, blob_ref)) = found else {
                continue;
            };
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
                service
                    .client()
                    .upload_blob(workspace, self.state.domain, &blob_ref, &path)
                    .await?;
                uploaded += 1;
            }
            self.journal.forget(&upload);
        }
        Ok(uploaded)
    }
}
