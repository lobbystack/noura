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

use sync_protocol::chain::ChainVerifier;
use sync_protocol::keys::LogJudge;
use sync_protocol::live::{LiveOp, LiveSet};
use sync_protocol::manifest::{Manifest, heads_digest, verify_buckets, verify_descent};
use sync_protocol::messages::{EntryRecord, LiveCursor, PageStatus, PullCursor};
use sync_protocol::records::{CheckpointBundle, Operation, Signed};
use sync_protocol::types::Id;

use super::state::Log;
use super::{Checked, CheckedRecord, SyncError, SyncService, WorkspaceSync};

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
        let (workspace, domain) = (self.state.workspace, self.state.domain);
        let Some(latest) = service.client().checkpoint(workspace, domain).await? else {
            return Ok(false);
        };
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        let bundle = latest.bundle;
        let entry_seq = latest.seq;
        self.check_bootstrap_checkpoint(service, &bundle, entry_seq)
            .await?;
        let c = bundle.checkpoint.0.body.clone();

        // The bucket hashes, against the root.
        let buckets = service
            .client()
            .manifest_buckets(workspace, domain, c.covered_seq)
            .await?
            .buckets
            .0;
        verify_buckets(&buckets, &c.manifest_root).map_err(|_| fail("sync_manifest_mismatch"))?;

        // The live operations, page by page.
        let mut ops: Vec<(Signed<Operation>, u64)> = Vec::new();
        let mut after: Option<LiveCursor> = None;
        loop {
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
        Ok(true)
    }

    /// The checks of §17 on a checkpoint a device bootstraps from: the
    /// heads digest, `policy_head`, the epoch record, no rollback below
    /// what the device holds, descent from its anchor, the signer by
    /// position and the recency of its policy.
    async fn check_bootstrap_checkpoint(
        &mut self,
        service: &mut SyncService,
        bundle: &CheckpointBundle,
        entry_seq: u64,
    ) -> Result<(), SyncError> {
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
        let kept = service
            .kept_heads()
            .checkpoint_anchor(&workspace, &domain)
            .filter(|a| a.generation == self.state.generation);
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
                return Err(fail("sync_manifest_rollback"));
            }
        }
        Ok(())
    }
}
