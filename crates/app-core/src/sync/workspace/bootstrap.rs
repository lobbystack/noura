//! Bootstrapping a domain from its latest checkpoint
//! (docs/workspace-format/sync-v2.md §17, §18): a new device loads the
//! operations live at the checkpoint instead of replaying the whole log.
//!
//! Before it installs anything, the device checks the checkpoint's heads
//! digest, `policy_head` and epoch record, its descent from the device's
//! anchor when it holds one, its signer by position after pulling the log
//! up to the checkpoint's entry, and the recency of its policy. It then
//! checks the bucket hashes against the root and fetches the live
//! operations page by page. As each bucket completes, it checks the
//! bucket's hash, writes its files and records its progress, so a restart
//! resumes at the next bucket; the leaf count is checked after the last.
//! The chain verifier starts at the checkpoint's heads, and the device
//! pulls on from `covered_seq`.

use std::collections::{BTreeMap, BTreeSet};

use sync_protocol::chain::ChainVerifier;
use sync_protocol::keys::LogJudge;
use sync_protocol::live::{LiveOp, LiveSet};
use sync_protocol::manifest::{
    BUCKETS, bucket_hash, bucket_of, heads_digest, leaf_hash, skip_number, verify_bucket,
    verify_buckets, verify_descent,
};
use sync_protocol::messages::{EntryRecord, PageStatus, PullCursor};
use sync_protocol::records::{CheckpointBundle, OpClass, Operation, PolicyKind, Signed};
use sync_protocol::types::{Digest, Id};

use super::state::{BootstrapProgress, DeletedAway, Log, OpKey, StateInfo};
use super::{Checked, CheckedRecord, SyncError, SyncService, WorkspaceSync};
use crate::sync::account_heads::KeptCheckpointAnchor;

/// How many times a bootstrap restarts because the floor overtook it.
const MAX_RESTARTS: usize = 8;

/// The buckets whose hash differs from the one the journal's live set
/// yields, in ascending order.
fn differing_buckets(live: &LiveSet, buckets: &[Digest]) -> Vec<u16> {
    let mut hashes: Vec<Vec<Digest>> = vec![Vec::new(); BUCKETS];
    for (rid, mut refs) in live.leaves_at(u64::MAX) {
        if refs.is_empty() {
            continue;
        }
        refs.sort_by_key(|r| (r.device, r.counter));
        hashes[usize::from(bucket_of(&rid))].push(leaf_hash(&rid, &refs));
    }
    (0..BUCKETS as u16)
        .filter(|i| bucket_hash(*i, &hashes[usize::from(*i)]) != buckets[usize::from(*i)])
        .collect()
}

/// Why a bootstrap stops: the client code of the check that failed.
fn fail(code: &'static str) -> SyncError {
    SyncError::Check(code)
}

impl WorkspaceSync {
    /// Bootstraps the workspace domain from its latest checkpoint, when
    /// the server holds one; returns whether it did. A failed check reports
    /// `sync_manifest_rollback`, `sync_manifest_mismatch` or the check's own
    /// code, and installs nothing.
    pub(super) async fn bootstrap(&mut self, service: &mut SyncService) -> Result<bool, SyncError> {
        let mut attempts = 0;
        loop {
            let result = self.bootstrap_latest(service).await;
            match result {
                // The floor passed the checkpoint before the bootstrap
                // ended: it restarts from the latest one, and the buckets
                // whose hashes match stay (§18).
                Err(SyncError::Refused { code, .. })
                    if code == "cursor_compacted" && attempts < MAX_RESTARTS =>
                {
                    attempts += 1;
                    self.state.bootstrap_progress = None;
                    self.save()?;
                }
                // A failed check drops the progress; a lost connection
                // keeps it, and the next attempt resumes at the next
                // bucket.
                Err(SyncError::Check(_)) => {
                    self.state.bootstrap_progress = None;
                    self.save()?;
                    return result;
                }
                other => return other,
            }
        }
    }

    /// The checkpoint an interrupted bootstrap was installing, when the
    /// server still serves it and it still passes the bootstrap checks.
    async fn resumed_checkpoint(
        &mut self,
        service: &mut SyncService,
    ) -> Result<Option<(CheckpointBundle, u64)>, SyncError> {
        let Some(digest) = self.state.bootstrap_progress.as_ref().map(|p| p.checkpoint) else {
            return Ok(None);
        };
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let Some(entry) = service
            .client()
            .checkpoint_by_digest(workspace, domain, digest)
            .await?
        else {
            return Ok(None);
        };
        let EntryRecord::Checkpoint(bundle) = entry.record else {
            return Ok(None);
        };
        // The floor passed it: the server keeps its record without heads.
        if bundle.checkpoint.0.digest() != digest
            || heads_digest(&bundle.heads) != bundle.checkpoint.0.body.heads_digest
            || !self
                .check_bootstrap_checkpoint(service, &bundle, entry.seq)
                .await?
        {
            return Ok(None);
        }
        Ok(Some((bundle, entry.seq)))
    }

    async fn bootstrap_latest(&mut self, service: &mut SyncService) -> Result<bool, SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let Some(latest) = service.client().checkpoint(workspace, domain).await? else {
            return Ok(false);
        };
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        let resumed = self.resumed_checkpoint(service).await?;
        let (bundle, entry_seq) = if let Some(resumed) = resumed {
            resumed
        } else if self
            .check_bootstrap_checkpoint(service, &latest.bundle, latest.seq)
            .await?
        {
            (latest.bundle, latest.seq)
        } else {
            // Recency refused the latest checkpoint, as after a recovery that
            // removed its signer while no other writer is left to sign the
            // next one. The device bootstraps from its anchor instead, which
            // Recency skips, and pulls on from there, checking every later
            // entry as a log entry (§18).
            let anchor = self
                .kept_anchor(service)
                .filter(|a| a.checkpoint != latest.bundle.checkpoint.0.digest())
                .ok_or(fail("sync_manifest_rollback"))?;
            let entry = service
                .client()
                .checkpoint_by_digest(workspace, domain, anchor.checkpoint)
                .await?
                .ok_or(fail("sync_manifest_rollback"))?;
            let EntryRecord::Checkpoint(bundle) = entry.record else {
                return Err(fail("sync_manifest_rollback"));
            };
            if bundle.checkpoint.0.digest() != anchor.checkpoint
                || !self
                    .check_bootstrap_checkpoint(service, &bundle, entry.seq)
                    .await?
            {
                return Err(fail("sync_manifest_rollback"));
            }
            (bundle, entry.seq)
        };
        let c = bundle.checkpoint.0.body.clone();
        // A restricted domain's marker comes before its other objects, so
        // the root stands before the first file beneath it (§9).
        self.read_marker_first(service, c.covered_seq).await?;

        // The bucket hashes, against the root.
        let buckets = service
            .client()
            .manifest_buckets(workspace, domain, c.covered_seq)
            .await?
            .buckets
            .0;
        verify_buckets(&buckets, &c.manifest_root).map_err(|_| fail("sync_manifest_mismatch"))?;

        // The live operations, page by page, installed bucket by bucket:
        // pages come ordered by `rid`, so every bucket below the next
        // page's first `rid` is complete. The journal keeps the progress
        // with each bucket's files, so a restart resumes at the next
        // bucket (§18, step 5).
        let digest = bundle.checkpoint.0.digest();
        let resuming = self
            .state
            .bootstrap_progress
            .as_ref()
            .is_some_and(|p| p.checkpoint == digest);
        // What the journal holds decides what comes down: only the objects
        // of the buckets that differ, or, past half of them, every page
        // as a new device reads them (§18).
        let differing = if resuming {
            Vec::new()
        } else {
            differing_buckets(&self.state.log.live, &buckets)
        };
        let returning = !resuming
            && self.state.log.live.rids().next().is_some()
            && differing.len() * 2 <= BUCKETS;
        let mut progress = match self.state.bootstrap_progress.take() {
            Some(progress) if progress.checkpoint == digest => progress,
            _ => BootstrapProgress {
                checkpoint: digest,
                after: None,
                next_bucket: if returning { BUCKETS as u32 } else { 0 },
                pending: Vec::new(),
                live_objects: 0,
                last: BTreeMap::new(),
            },
        };
        self.state.bootstrap_progress = Some(progress.clone());
        if returning {
            for index in differing {
                self.install_differing(service, &bundle, &buckets, index, &mut progress)
                    .await?;
                self.state.bootstrap_progress = Some(progress.clone());
                self.save()?;
            }
        }
        while (progress.next_bucket as usize) < BUCKETS {
            let page = service
                .client()
                .live(
                    workspace,
                    domain,
                    c.covered_seq,
                    None,
                    progress.after.as_ref(),
                )
                .await?;
            for entry in page.entries.0 {
                let EntryRecord::Operation(op) = entry.record else {
                    return Err(fail("sync_manifest_mismatch"));
                };
                if entry.seq > c.covered_seq
                    || u32::from(bucket_of(&op.body.rid)) < progress.next_bucket
                {
                    return Err(fail("sync_manifest_mismatch"));
                }
                progress.pending.push((op, entry.seq));
            }
            let complete = match &page.next {
                Some(next) => u32::from(bucket_of(&next.rid)),
                None => BUCKETS as u32,
            };
            if progress
                .pending
                .iter()
                .any(|(op, _)| u32::from(bucket_of(&op.body.rid)) > complete)
            {
                return Err(fail("sync_manifest_mismatch"));
            }
            if complete > progress.next_bucket {
                let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut progress.pending)
                    .into_iter()
                    .partition(|(op, _)| u32::from(bucket_of(&op.body.rid)) < complete);
                progress.pending = rest;
                self.install_buckets(
                    service,
                    &bundle,
                    &buckets,
                    progress.next_bucket..complete,
                    ready,
                    &BTreeMap::new(),
                    &mut progress,
                )
                .await?;
                progress.next_bucket = complete;
            }
            progress.after = page.next;
            self.state.bootstrap_progress = Some(progress.clone());
            self.save()?;
        }
        let leaves = self.state.log.live.leaves_at(c.covered_seq);
        let live_objects = leaves.values().filter(|refs| !refs.is_empty()).count() as u64;
        if live_objects != c.live_objects || !progress.pending.is_empty() {
            return Err(fail("sync_manifest_mismatch"));
        }
        let remote = self
            .remote
            .as_ref()
            .ok_or(fail("sync_workspace_unverified"))?;

        // The log as it stands at the checkpoint.
        let epoch_policy_revision = remote
            .epochs
            .get(c.epoch)
            .map(|e| e.record.body.policy_revision)
            .ok_or(fail("sync_key_mismatch"))?;
        let mut log: Log = self.state.log.clone();
        log.cursor = c.covered_seq;
        log.head = log.head.max(entry_seq);
        log.checkpoint_seq = log.checkpoint_seq.max(c.covered_seq);
        log.judge =
            LogJudge::from_checkpoint(domain, c.policy_revision, c.epoch, epoch_policy_revision);
        log.verifier = ChainVerifier::from_heads(&bundle.heads);
        for (device, (revision, generation)) in &progress.last {
            let last = log.last.entry(*device).or_insert((0, 0));
            *last = (last.0.max(*revision), last.1.max(*generation));
        }
        log.wanted.retain(|w| {
            if w.epoch {
                w.number > c.epoch
            } else {
                w.number > c.policy_revision
            }
        });
        log.epoch_entries = log.epoch_entries.max(c.epoch);
        log.checkpoints = Default::default();
        log.checkpoints
            .note_policy_entry(c.covered_seq, c.policy_revision);
        log.checkpoints
            .note_epoch_entry(c.covered_seq, c.epoch, c.epoch_record);
        log.checkpoints.accept(&bundle, &mut log.live);
        log.checkpoints.reach(&bundle.checkpoint.0, entry_seq);
        log.checkpoints.bootstrap_seq = c.covered_seq;
        // The checkpoints on the bootstrap checkpoint's skip chain: every
        // later checkpoint's `skip` names one of them or one the device
        // reaches in its log, so the device can sign and check the next
        // ones (§17, Checkpoint chain).
        let (mut digest, mut number) = (c.skip, skip_number(c.number));
        while number != 0 {
            let entry = service
                .client()
                .checkpoint_by_digest(workspace, domain, digest)
                .await?
                .ok_or(fail("sync_chain_gap"))?;
            let EntryRecord::Checkpoint(found) = entry.record else {
                return Err(fail("sync_chain_gap"));
            };
            let record = &found.checkpoint.0;
            if record.digest() != digest
                || record.body.number != number
                || record.body.workspace != workspace
                || record.body.domain != domain
            {
                return Err(fail("sync_chain_gap"));
            }
            log.checkpoints.reach(record, entry.seq);
            (digest, number) = (record.body.skip, skip_number(number));
        }

        self.state.log = log;
        self.state.bootstrap_progress = None;
        self.save()?;
        Ok(true)
    }

    /// Reads the live states of the domain's marker at `at` into the
    /// journal and takes the domain's root from them, before the buckets
    /// bring its other objects.
    async fn read_marker_first(
        &mut self,
        service: &mut SyncService,
        at: u64,
    ) -> Result<(), SyncError> {
        let domain = self.state.domain;
        let Some(active) = self.active else {
            return Ok(());
        };
        let marker = self
            .state
            .policy
            .iter()
            .find_map(|change| match &change.body.kind {
                PolicyKind::CreateDomain {
                    domain: d,
                    marker_rid,
                    ..
                } if *d == domain => Some(*marker_rid),
                _ => None,
            });
        let Some(rid) = marker else {
            return Ok(());
        };
        let page = service
            .client()
            .live(self.state.workspace, domain, at, Some(rid), None)
            .await?;
        let mut states = Vec::new();
        for entry in page.entries.0 {
            let EntryRecord::Operation(op) = entry.record else {
                return Err(fail("sync_manifest_mismatch"));
            };
            if op.body.rid != rid || op.body.domain != domain {
                return Err(fail("sync_manifest_mismatch"));
            }
            self.ensure_epoch(service, op.body.epoch).await?;
            let remote = self
                .remote
                .as_ref()
                .ok_or(fail("sync_workspace_unverified"))?;
            let card = remote
                .policy
                .at(op.body.policy_revision)
                .and_then(|s| s.devices.get(&op.body.device))
                .ok_or(fail("sync_writer_not_authorized"))?;
            op.verify(&card.card.signing_public)
                .map_err(|_| fail("sync_writer_not_authorized"))?;
            let payload = match (
                remote.epochs.get(op.body.epoch),
                self.keys.get(&op.body.epoch),
            ) {
                (Some(epoch), Some(key)) => op
                    .body
                    .open_payload(key, &epoch.digest, OpClass::State)
                    .ok(),
                _ => None,
            };
            let content = payload.and_then(|p| super::carried(&p, &mut BTreeMap::new()));
            let key = OpKey {
                device: op.body.device,
                counter: op.body.counter,
            };
            states.push(key);
            let object = self.state.objects.entry(rid).or_default();
            object.domain = Some(active);
            object.states.insert(
                key,
                StateInfo {
                    digest: op.digest(),
                    seq: Some(entry.seq),
                    covers: op.body.supersedes.to_vec(),
                    content,
                    epoch: op.body.epoch,
                    policy_revision: op.body.policy_revision,
                },
            );
        }
        self.update_roots()?;
        // A valid marker's file goes first, and its bucket then finds it
        // in place.
        if let ([key], Some(root)) = (
            states.as_slice(),
            self.state.roots.iter().find(|r| r.rid == rid).cloned(),
        ) {
            self.restore_marker(&root)?;
            if let Some(object) = self.state.objects.get_mut(&rid) {
                object.base = Some(*key);
            }
        }
        Ok(())
    }

    /// Installs the buckets in `range` from `ops`, every live operation in
    /// them (§18, step 5): runs the checks of §13 that need no chain or
    /// position on each, checks each bucket's hash, then writes their files
    /// and adds them to the domain's live set. A bucket that fails its hash
    /// reports `sync_manifest_mismatch` and installs nothing.
    #[allow(clippy::too_many_arguments)]
    async fn install_buckets(
        &mut self,
        service: &mut SyncService,
        bundle: &CheckpointBundle,
        buckets: &[Digest],
        range: std::ops::Range<u32>,
        mut ops: Vec<(Signed<Operation>, u64)>,
        kept: &BTreeMap<Id, Vec<sync_protocol::records::Ref>>,
        progress: &mut BootstrapProgress,
    ) -> Result<(), SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let c = &bundle.checkpoint.0.body;
        {
            let remote = self
                .remote
                .as_ref()
                .ok_or(fail("sync_workspace_unverified"))?;
            let policy = &remote.policy;
            for (op, _) in &ops {
                let body = &op.body;
                if body.workspace != workspace || body.domain != domain {
                    return Err(fail("sync_manifest_mismatch"));
                }
                let card = policy
                    .at(body.policy_revision)
                    .and_then(|s| s.devices.get(&body.device))
                    .ok_or(fail("sync_writer_not_authorized"))?;
                op.verify(&card.card.signing_public)
                    .map_err(|_| fail("sync_writer_not_authorized"))?;
                if policy.change(body.policy_revision).map(|h| h.digest()) != Some(body.policy_head)
                {
                    return Err(fail("sync_policy_forked"));
                }
            }
        }
        ops.sort_by_key(|(_, seq)| *seq);
        // The buckets' own live set, from which their leaves and hashes
        // come; their `rid`s are new to the domain's live set.
        // Whatever else the journal held in these buckets leaves first: the
        // manifest says what stands now, and a `rid` it lacks is gone.
        let mut live = self.state.log.live.clone();
        let incoming: BTreeSet<Id> = ops.iter().map(|(op, _)| op.body.rid).collect();
        let replaced: Vec<Id> = live
            .rids()
            .filter(|rid| range.contains(&u32::from(bucket_of(rid))) && !kept.contains_key(rid))
            .copied()
            .collect();
        let gone: Vec<Id> = replaced
            .iter()
            .filter(|rid| !incoming.contains(rid))
            .copied()
            .collect();
        for rid in &replaced {
            live.forget(rid);
        }
        let mut fresh = LiveSet::new();
        let mut classes = Vec::with_capacity(ops.len());
        for (op, seq) in &ops {
            fresh.apply(LiveOp::from_operation(op, *seq));
            classes.push(live.apply(LiveOp::from_operation(op, *seq)));
        }
        let mut leaves = fresh.leaves_at(c.covered_seq);
        leaves.extend(kept.iter().map(|(rid, refs)| (*rid, refs.clone())));
        for index in range {
            let index = index as u16;
            let mine: Vec<_> = leaves
                .iter()
                .filter(|(rid, refs)| bucket_of(rid) == index && !refs.is_empty())
                .map(|(rid, refs)| {
                    let mut refs = refs.clone();
                    refs.sort_by_key(|r| (r.device, r.counter));
                    (*rid, refs)
                })
                .collect();
            verify_bucket(index, &mine, &buckets[usize::from(index)])
                .map_err(|_| fail("sync_manifest_mismatch"))?;
            progress.live_objects += mine.len() as u64;
        }
        for (op, _) in &ops {
            let entry = progress.last.entry(op.body.device).or_insert((0, 0));
            *entry = (
                entry.0.max(op.body.policy_revision),
                entry.1.max(op.body.generation),
            );
        }
        // Older epoch records only now that a live operation names one.
        let mut epochs: Vec<u64> = ops.iter().map(|(op, _)| op.body.epoch).collect();
        epochs.sort_unstable();
        epochs.dedup();
        for epoch in epochs {
            self.ensure_epoch(service, epoch).await?;
        }
        let remote = self
            .remote
            .as_ref()
            .ok_or(fail("sync_workspace_unverified"))?;
        let mut records = Vec::with_capacity(ops.len());
        for ((op, seq), class) in ops.into_iter().zip(classes) {
            let payload = match (
                remote.epochs.get(op.body.epoch),
                self.keys.get(&op.body.epoch),
            ) {
                (Some(epoch), Some(key)) => op.body.open_payload(key, &epoch.digest, class).ok(),
                _ => None,
            };
            records.push(CheckedRecord {
                op,
                seq,
                class,
                payload,
            });
        }
        let mut log = self.state.log.clone();
        log.live = live;
        let checked = Checked {
            log,
            records,
            walks: Vec::new(),
        };
        let fetched = self.fetch_blobs(service, &checked).await?;
        let applied = self.apply(checked, fetched);
        self.clear_downloads();
        applied?;
        for rid in gone {
            self.left_while_away(rid);
        }
        Ok(())
    }

    /// Fetches what bucket `index` holds that the journal lacks, through
    /// its leaf refs and `live` with `rid` (§18), and installs it. The
    /// objects whose refs the journal already holds stay as they are.
    async fn install_differing(
        &mut self,
        service: &mut SyncService,
        bundle: &CheckpointBundle,
        buckets: &[Digest],
        index: u16,
        progress: &mut BootstrapProgress,
    ) -> Result<(), SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let at = bundle.checkpoint.0.body.covered_seq;
        let mut after = None;
        let mut leaves: BTreeMap<Id, Vec<sync_protocol::records::Ref>> = BTreeMap::new();
        loop {
            let page = service
                .client()
                .manifest_leaves(workspace, domain, at, index, after.as_ref())
                .await?;
            for leaf in page.leaves.iter() {
                leaves.entry(leaf.rid).or_default().push(leaf.op.clone());
            }
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        let held = self.state.log.live.leaves_at(u64::MAX);
        let mut kept = BTreeMap::new();
        let mut ops = Vec::new();
        for (rid, refs) in leaves {
            let mut mine = held.get(&rid).cloned().unwrap_or_default();
            mine.sort_by_key(|r| (r.device, r.counter));
            if mine == refs {
                kept.insert(rid, refs);
                continue;
            }
            let mut after = None;
            loop {
                let page = service
                    .client()
                    .live(workspace, domain, at, Some(rid), after.as_ref())
                    .await?;
                for entry in page.entries.0 {
                    let EntryRecord::Operation(op) = entry.record else {
                        return Err(fail("sync_manifest_mismatch"));
                    };
                    if entry.seq > at || op.body.rid != rid {
                        return Err(fail("sync_manifest_mismatch"));
                    }
                    ops.push((op, entry.seq));
                }
                match page.next {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
        }
        let range = u32::from(index)..u32::from(index) + 1;
        self.install_buckets(service, bundle, buckets, range, ops, &kept, progress)
            .await
    }

    /// A `rid` the journal knew and the manifest lacks. The object goes
    /// from the journal and its file stays (§18): an unchanged file waits
    /// for you to confirm its deletion, and a changed one uploads under a
    /// new `rid` in the next capture.
    fn left_while_away(&mut self, rid: Id) {
        let edited = self.local_edit(&rid);
        let Some(object) = self.state.objects.remove(&rid) else {
            return;
        };
        if let Some(local) = object.local
            && !edited
        {
            self.state.deleted_away.push(DeletedAway {
                path: local.path,
                revision: local.revision,
            });
        }
    }

    /// The checkpoint anchor the device's approval or kit gave this domain
    /// in the current restore generation. An offer or kit made before the
    /// domain's first checkpoint carries a covered_seq of 0 and 32 zero
    /// bytes: no anchor.
    fn kept_anchor(&self, service: &SyncService) -> Option<KeptCheckpointAnchor> {
        service
            .kept_heads()
            .checkpoint_anchor(&self.state.workspace, &self.state.domain)
            .filter(|a| a.generation == self.state.generation)
            .filter(|a| !a.checkpoint.is_zero())
    }

    /// The checks of §17 on a checkpoint a device bootstraps from: the
    /// heads digest, `policy_head`, the epoch record, no rollback below
    /// what the device holds, descent from its anchor, the signer by
    /// position and the recency of its policy. Returns `false` when only
    /// Recency fails. The anchor itself skips Recency: the device that
    /// wrote the offer or kit accepted it (§17).
    async fn check_bootstrap_checkpoint(
        &mut self,
        service: &mut SyncService,
        bundle: &CheckpointBundle,
        entry_seq: u64,
    ) -> Result<bool, SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let record = &bundle.checkpoint.0;
        let c = &record.body;
        if c.workspace != workspace
            || c.domain != domain
            || heads_digest(&bundle.heads) != c.heads_digest
            || bundle.heads.iter().any(|h| h.seq > c.covered_seq)
            || c.covered_seq >= entry_seq
        {
            return Err(fail("sync_manifest_mismatch"));
        }
        let accepted = self.state.log.checkpoints.accepted.clone();
        if c.covered_seq < self.state.log.cursor
            || accepted
                .as_ref()
                .is_some_and(|a| c.covered_seq < a.record.body.covered_seq)
        {
            return Err(fail("sync_manifest_rollback"));
        }
        // The checkpoint's epoch record, walked back to when it is older
        // than the current one (§18, step 1).
        self.ensure_epoch(service, c.epoch).await?;
        {
            let remote = self
                .remote
                .as_ref()
                .ok_or(fail("sync_workspace_unverified"))?;
            if remote.policy.change(c.policy_revision).map(|h| h.digest()) != Some(c.policy_head) {
                return Err(fail("sync_policy_forked"));
            }
            if remote.epochs.get(c.epoch).map(|e| e.digest) != Some(c.epoch_record) {
                return Err(fail("sync_key_mismatch"));
            }
        }
        // Descent from the device's anchor, when it holds one: its latest
        // accepted checkpoint, or else the checkpoint its approval or kit
        // anchored in this restore generation. A restore that lost that
        // anchor leaves the other checks (ticket 60).
        let kept = self.kept_anchor(service);
        let is_anchor = kept
            .as_ref()
            .is_some_and(|k| k.checkpoint == record.digest());
        let anchor = match (&accepted, kept) {
            (Some(a), _) => Some((a.record.body.covered_seq, a.record.digest())),
            (None, Some(k)) => Some((k.covered_seq, k.checkpoint)),
            (None, None) => None,
        };
        if anchor.is_some_and(|(covered_seq, _)| c.covered_seq < covered_seq) {
            return Err(fail("sync_manifest_rollback"));
        }
        if let Some((_, anchor)) = anchor
            && anchor != record.digest()
        {
            let walk = service
                .client()
                .checkpoint_chain(workspace, domain, record.digest(), anchor)
                .await?
                .checkpoints
                .0;
            let walk: Vec<_> = walk.into_iter().map(|n| n.0).collect();
            if walk.first().map(Signed::digest) != Some(record.digest()) {
                return Err(fail("sync_manifest_rollback"));
            }
            verify_descent(&walk, &anchor).map_err(|_| fail("sync_manifest_rollback"))?;
        }
        // The policy entries up to the checkpoint's entry, applying nothing.
        let mut latest_entry = c.policy_revision;
        let mut after = c.covered_seq;
        while after + 1 < entry_seq {
            let response = service
                .client()
                .pull(workspace, vec![PullCursor { domain, after }], 0)
                .await?;
            let page = response
                .pages
                .0
                .into_iter()
                .find(|p| p.domain == domain)
                .ok_or(fail("sync_chain_gap"))?;
            crate::sync::check_page(&page, after)?;
            if page.status != PageStatus::Ok || page.complete_through <= after {
                return Err(fail("sync_chain_gap"));
            }
            for entry in page.entries.iter() {
                if entry.seq < entry_seq
                    && let EntryRecord::Policy(policy) = &entry.record
                {
                    latest_entry = latest_entry.max(policy.revision);
                }
            }
            after = page.complete_through;
        }
        if latest_entry > self.accepted_revision() {
            self.refresh(service).await?;
        }
        let remote = self
            .remote
            .as_ref()
            .ok_or(fail("sync_workspace_unverified"))?;
        let policy = &remote.policy;
        // The signer, by position and at its `policy_revision`.
        let writes = |revision: u64| {
            policy
                .at(revision)
                .is_some_and(|s| s.can_write(&c.signer, &domain))
        };
        let card = policy
            .at(c.policy_revision)
            .and_then(|s| s.devices.get(&c.signer))
            .ok_or(fail("sync_writer_not_authorized"))?;
        record
            .verify(&card.card.signing_public)
            .map_err(|_| fail("sync_writer_not_authorized"))?;
        if !writes(latest_entry) || !writes(c.policy_revision) {
            return Err(fail("sync_writer_not_authorized"));
        }
        // Recency: no change above its `policy_revision` takes its signer
        // or a device in its heads out of the workspace.
        if is_anchor {
            return Ok(true);
        }
        let devices: BTreeSet<Id> = std::iter::once(c.signer)
            .chain(bundle.heads.iter().map(|h| h.device))
            .collect();
        let head = policy.head().0;
        let at_checkpoint = policy.at(c.policy_revision);
        for device in devices {
            if !at_checkpoint.is_some_and(|s| s.is_device(&device)) {
                continue;
            }
            let removed = ((c.policy_revision + 1)..=head)
                .any(|r| policy.at(r).is_some_and(|s| !s.is_device(&device)));
            if removed {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
