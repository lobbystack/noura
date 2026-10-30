//! Bootstrapping a domain from its latest checkpoint
//! (docs/workspace-format/sync-v2.md §17, §18): a new device loads the
//! operations live at the checkpoint instead of replaying the whole log.
//!
//! Before it installs anything, the device checks the checkpoint's heads
//! digest, `policy_head` and epoch record, its descent from the device's
//! anchor when it holds one, its signer by position after pulling the log
//! up to the checkpoint's entry, and the recency of its policy. It then
//! checks the bucket hashes against the root, fetches the live operations,
//! checks each bucket's hash and the leaf count, and only then writes the
//! files. The chain verifier starts at the checkpoint's heads, and the
//! device pulls on from `covered_seq`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sync_protocol::chain::ChainVerifier;
use sync_protocol::keys::LogJudge;
use sync_protocol::live::{LiveOp, LiveSet};
use sync_protocol::manifest::{
    Manifest, heads_digest, skip_number, verify_buckets, verify_descent,
};
use sync_protocol::messages::{EntryRecord, LiveCursor, PageStatus, PullCursor};
use sync_protocol::records::{CheckpointBundle, Operation, Signed};
use sync_protocol::types::{Digest, Id};

use super::state::Log;
use super::{Checked, CheckedRecord, SyncError, SyncService, WorkspaceSync};
use crate::sync::account_heads::KeptCheckpointAnchor;

/// The journal file that keeps a bootstrap's progress through the live
/// pages.
const BOOTSTRAP_FILE: &str = "bootstrap.json";

/// The live pages a bootstrap fetched so far (§18).
#[derive(Clone, Serialize, Deserialize)]
struct Progress {
    /// The checkpoint the pages belong to.
    checkpoint: Digest,
    /// The cursor after the last page.
    after: Option<LiveCursor>,
    /// The operations the pages carried, with their sequences.
    ops: Vec<(Signed<Operation>, u64)>,
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
        let result = self.bootstrap_latest(service).await;
        // A failed check drops the pages fetched so far; a lost connection
        // keeps them for the next attempt.
        if let Err(SyncError::Check(_)) = result {
            self.journal.write(BOOTSTRAP_FILE, &None::<Progress>)?;
        }
        result
    }

    async fn bootstrap_latest(&mut self, service: &mut SyncService) -> Result<bool, SyncError> {
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let Some(latest) = service.client().checkpoint(workspace, domain).await? else {
            return Ok(false);
        };
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        let (bundle, entry_seq) = if self
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

        // The bucket hashes, against the root.
        let buckets = service
            .client()
            .manifest_buckets(workspace, domain, c.covered_seq)
            .await?
            .buckets
            .0;
        verify_buckets(&buckets, &c.manifest_root).map_err(|_| fail("sync_manifest_mismatch"))?;

        // The live operations, page by page. The journal keeps the pages
        // fetched so far, so a restart resumes after the last one (§18).
        let digest = bundle.checkpoint.0.digest();
        let kept = self
            .journal
            .read::<Option<Progress>>(BOOTSTRAP_FILE)?
            .flatten();
        let (mut ops, mut after) = match kept {
            Some(progress) if progress.checkpoint == digest => (progress.ops, progress.after),
            _ => (Vec::new(), None),
        };
        loop {
            if after.is_some() {
                self.journal.write(
                    BOOTSTRAP_FILE,
                    &Progress {
                        checkpoint: digest,
                        after: after.clone(),
                        ops: ops.clone(),
                    },
                )?;
            }
            let page = service
                .client()
                .live(workspace, domain, c.covered_seq, None, after.as_ref())
                .await?;
            for entry in page.entries.0 {
                let EntryRecord::Operation(op) = entry.record else {
                    return Err(fail("sync_manifest_mismatch"));
                };
                if entry.seq > c.covered_seq {
                    return Err(fail("sync_manifest_mismatch"));
                }
                ops.push((op, entry.seq));
            }
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        let remote = self
            .remote
            .as_ref()
            .ok_or(fail("sync_workspace_unverified"))?;
        let policy = &remote.policy;
        // The checks of §13 that need no chain or position, which the
        // checkpoint's signer applied.
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
            if policy.change(body.policy_revision).map(|h| h.digest()) != Some(body.policy_head) {
                return Err(fail("sync_policy_forked"));
            }
        }
        ops.sort_by_key(|(_, seq)| *seq);
        let mut live = LiveSet::new();
        let mut classes = Vec::with_capacity(ops.len());
        for (op, seq) in &ops {
            classes.push(live.apply(LiveOp::from_operation(op, *seq)));
        }
        let manifest = Manifest::new(live.leaves_at(c.covered_seq));
        let ours = manifest.bucket_hashes();
        if ours != buckets || manifest.live_objects() != c.live_objects {
            return Err(fail("sync_manifest_mismatch"));
        }

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
        log.live = live;
        log.last = BTreeMap::new();
        for (op, _) in &ops {
            let entry = log.last.entry(op.body.device).or_insert((0, 0));
            *entry = (
                entry.0.max(op.body.policy_revision),
                entry.1.max(op.body.generation),
            );
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
        let checked = Checked {
            log,
            records,
            walks: Vec::new(),
        };
        let fetched = self.fetch_blobs(service, &checked).await?;
        let applied = self.apply(checked, fetched);
        self.clear_downloads();
        applied?;
        self.save()?;
        self.journal.write(BOOTSTRAP_FILE, &None::<Progress>)?;
        Ok(true)
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
