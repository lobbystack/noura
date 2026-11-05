//! Retention, pruning, quota and the blob collector
//! (docs/workspace-format/sync-v2.md §19, §20).
//!
//! Each domain's head row carries its usage, its history and its log's
//! running weight, and each checkpoint keeps that running weight at its
//! entry, so the retention floor comes from one indexed read. Every change
//! to usage or history happens under the domain's head row. Pruning takes
//! the head row one batch at a time, so a push waits for at most one batch.

use sync_protocol::codec::Encode;
use sync_protocol::manifest::BUCKETS;
use sync_protocol::records::Heads;
use sync_protocol::types::{Digest, Id};

use crate::push::{ENTRY_EPOCH, ENTRY_OPERATION, ENTRY_POLICY};
use crate::{
    Backend, DEFAULT_RETAINED_BYTES, DomainRow, Error, LockPlan, PendingDomain, PruneScan, ReadTxn,
    Result, Store, WriteTxn, refuse,
};

/// How long an incomplete upload lasts after creation, and a completed one
/// that no push lists after completion (§19).
pub const UPLOAD_TTL: u64 = 24 * 60 * 60;

/// What a checkpoint's 4,096 bucket hashes add to its entry's weight (§19).
pub(crate) const BUCKET_HASH_WEIGHT: u64 = (BUCKETS * 32) as u64;

/// Entries one pruning transaction deletes.
const PRUNE_BATCH: u32 = 512;
/// Rows of each table one reclaiming transaction deletes.
const RECLAIM_BATCH: u32 = 1_024;
/// Domains, workspaces, blobs and share links one maintenance pass takes.
const PASS_LIMIT: u32 = 4_096;

/// The server settings of §19 and §20.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// The weight each domain's log keeps behind a checkpoint.
    pub retained_bytes: u64,
    /// The storage quota of each workspace; `None` is unlimited.
    pub quota_bytes: Option<u64>,
    /// Entries one pruning transaction deletes.
    pub prune_batch: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            retained_bytes: DEFAULT_RETAINED_BYTES,
            quota_bytes: None,
            prune_batch: PRUNE_BATCH,
        }
    }
}

/// A blob the collector may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobToCollect {
    /// Its workspace.
    pub workspace: Id,
    /// The domain whose head row guards it.
    pub domain: Id,
    /// The blob.
    pub blob: Id,
}

/// What one maintenance pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Maintenance {
    /// Domains whose floor rose.
    pub raised: u64,
    /// Log entries pruning deleted.
    pub pruned: u64,
    /// Domains that finished reclaiming.
    pub reclaimed: u64,
    /// Domains and workspaces the pass gave up on until the next one.
    pub failed: u64,
    /// The first failure, for the server's local log.
    pub failure: Option<String>,
}

impl Maintenance {
    /// One domain or workspace failing costs only its own work: the pass
    /// goes on to the rest.
    fn note(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.failed += 1;
            self.failure.get_or_insert_with(|| error.to_string());
        }
    }
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Refuses a change that raised usage when usage now stands above the
/// quota. The sum may miss a concurrent push into another domain (§15).
pub(crate) async fn check_quota(
    txn: &mut impl ReadTxn,
    workspace: Id,
    quota: Option<u64>,
    raised: i64,
) -> Result<()> {
    let Some(quota) = quota else {
        return Ok(());
    };
    if raised <= 0 {
        return Ok(());
    }
    if txn.workspace_usage(workspace).await?.usage > quota {
        return Err(refuse(
            "quota_exceeded",
            "the workspace's usage would exceed its quota",
        ));
    }
    Ok(())
}

/// Whether the workspace's usage stands above the quota right now.
pub(crate) async fn over_quota(
    txn: &mut impl ReadTxn,
    workspace: Id,
    quota: Option<u64>,
) -> Result<bool> {
    let Some(quota) = quota else {
        return Ok(false);
    };
    Ok(txn.workspace_usage(workspace).await?.usage > quota)
}

/// Follows the pressure trigger (§20): usage and history going from within
/// the quota to above it count one crossing, and it re-arms only once they
/// fall below 90% of the quota.
pub(crate) async fn note_pressure(
    txn: &mut impl WriteTxn,
    workspace: Id,
    quota: Option<u64>,
) -> Result<()> {
    let Some(quota) = quota else {
        return Ok(());
    };
    let usage = txn.workspace_usage(workspace).await?;
    let Some(pressure) = usage.quota_pressure else {
        return Ok(());
    };
    let total = u128::from(usage.usage) + u128::from(usage.history);
    if !pressure && total > u128::from(quota) {
        txn.set_quota_pressure(workspace, true).await?;
    } else if pressure && total * 10 < u128::from(quota) * 9 {
        txn.set_quota_pressure(workspace, false).await?;
    }
    Ok(())
}

/// An operation left the live set: its bytes move to history, and so does
/// each blob that no other live operation lists. Returns the change to
/// usage.
async fn leave_live(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    seq: u64,
) -> Result<(i64, i64)> {
    let length = signed(
        txn.entry_length(workspace, domain, seq)
            .await?
            .ok_or_else(|| Error::internal("a live operation has no entry"))?,
    );
    let (mut usage, mut history) = (-length, length);
    for blob in txn.listed_blobs(workspace, domain, seq).await? {
        txn.add_blob_live(workspace, blob, -1).await?;
        if let Some(row) = txn.blob(workspace, blob).await?
            && row.live_listings == 0
        {
            usage -= signed(row.length);
            history += signed(row.length);
        }
    }
    Ok((usage, history))
}

/// What listing `blob` in a new operation found, read before the listing.
pub(crate) struct NewListing {
    pub(crate) blob: Id,
    /// Whether an earlier retained operation lists it.
    pub(crate) listed: bool,
}

/// Whether a new operation's record is all it adds to the counters, which
/// its write then counts on the head row, so [`note_operation`] reads
/// nothing to count it.
pub(crate) fn counted_by_write(blobs: &[NewListing], left: &[u64]) -> bool {
    blobs.is_empty() && left.is_empty()
}

/// What a record of `length` bytes adds to usage.
pub(crate) fn record_usage(length: u64) -> i64 {
    signed(length)
}

/// Counts an operation the push just wrote (§20): it adds its record to
/// usage, brings back to usage each blob only history listed, and moves
/// each operation it took out of the live set to history. `left` holds the
/// sequences of those operations. A delete never counts as raising usage,
/// and neither do an edit's signature and covers: only its payload and
/// blobs do, so a replacement of equal size passes a full quota. When
/// [`counted_by_write`] holds, the write already counted the record and
/// only the quota remains to check.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn note_operation(
    txn: &mut impl WriteTxn,
    quota: Option<u64>,
    workspace: Id,
    domain: Id,
    length: u64,
    payload: Option<u64>,
    blobs: &[NewListing],
    left: &[u64],
) -> Result<()> {
    if counted_by_write(blobs, left) {
        return check_quota(txn, workspace, quota, payload.map_or(0, signed)).await;
    }
    let (mut usage, mut history) = (signed(length), 0);
    for listing in blobs {
        let row = txn
            .blob(workspace, listing.blob)
            .await?
            .ok_or_else(|| Error::internal("a listed blob has no row"))?;
        if listing.listed && row.live_listings == 0 {
            usage += signed(row.length);
            history -= signed(row.length);
        }
        txn.add_blob_live(workspace, listing.blob, 1).await?;
        txn.set_blob_delete_after(workspace, listing.blob, None)
            .await?;
    }
    for seq in left {
        let (used, kept) = leave_live(txn, workspace, domain, *seq).await?;
        usage += used;
        history += kept;
    }
    txn.add_usage(workspace, domain, usage, history).await?;
    let raised = match payload {
        Some(payload) => usage - signed(length) + signed(payload),
        None => 0,
    };
    check_quota(txn, workspace, quota, raised).await
}

/// Counts a lone delete a checkpoint's `floor` dropped from the live set.
pub(crate) async fn note_dropped(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    seq: u64,
) -> Result<()> {
    let (usage, history) = leave_live(txn, workspace, domain, seq).await?;
    txn.add_usage(workspace, domain, usage, history).await
}

/// The floor the `retained_bytes` rule gives `row` (§19), never below the
/// one the server published.
async fn rule_floor(
    txn: &mut impl ReadTxn,
    retained_bytes: u64,
    workspace: Id,
    domain: Id,
    row: &DomainRow,
) -> Result<u64> {
    let qualifies = match row.log_weight.checked_sub(retained_bytes) {
        Some(through) => txn.checkpoint_at_weight(workspace, domain, through).await?,
        None => None,
    };
    Ok(qualifies.unwrap_or(0).max(row.retention_floor))
}

/// The lock plan of pruning one domain: the shared policy lock and the
/// domain's head row.
fn head_plan(workspace: Id, domain: Id, blobs: Vec<Id>) -> LockPlan {
    LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: false,
        domains: vec![domain],
        blobs,
    }
}

/// The sequence of the newest entry of `record_type` at or below `floor`.
async fn newest_entry(
    txn: &mut impl ReadTxn,
    workspace: Id,
    domain: Id,
    record_type: u8,
    floor: u64,
) -> Result<u64> {
    Ok(txn
        .latest_entry(workspace, domain, record_type, floor)
        .await?
        .map_or(0, |entry| entry.seq))
}

/// Deletes one batch of what the floor passed, in ascending sequence order
/// (§19), after `after`. Returns the sequence to continue after, or `None`
/// when nothing is left.
async fn prune_batch(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    after: u64,
    now: u64,
    batch: u32,
) -> Result<(u64, Option<u64>)> {
    let Some(row) = txn.domain(workspace, domain).await? else {
        return Ok((0, None));
    };
    let floor = row.retention_floor;
    let scan = PruneScan {
        after,
        floor,
        delete_floor: txn
            .servable_floor(workspace, domain, floor)
            .await?
            .map_or(0, |lowest| lowest.min(floor)),
        keep_epoch: newest_entry(txn, workspace, domain, ENTRY_EPOCH, floor).await?,
        keep_policy: newest_entry(txn, workspace, domain, ENTRY_POLICY, floor).await?,
    };
    let entries = txn
        .prunable_entries(workspace, domain, &scan, batch)
        .await?;
    let count = entries.len() as u64;
    let last = entries.last().map(|entry| entry.seq);
    let (mut usage, mut history) = (0, 0);
    for entry in &entries {
        match entry.record_type {
            ENTRY_OPERATION => {
                history -= signed(entry.length);
                for blob in txn.listed_blobs(workspace, domain, entry.seq).await? {
                    txn.delete_blob_listing(workspace, blob, domain, entry.seq)
                        .await?;
                    if !txn.blob_listed(workspace, blob).await?
                        && let Some(row) = txn.blob(workspace, blob).await?
                    {
                        // Unlisted again: it counts as an upload until the
                        // collector takes it.
                        history -= signed(row.length);
                        usage += signed(row.length);
                        txn.set_blob_delete_after(workspace, blob, Some(now))
                            .await?;
                    }
                }
            }
            ENTRY_EPOCH | ENTRY_POLICY => history -= signed(entry.length),
            _ => {}
        }
        txn.delete_log_entry(workspace, domain, entry.seq, entry.record_type)
            .await?;
        if let Some(rid) = entry.rid
            && !txn.rid_in_use(workspace, rid).await?
        {
            txn.delete_rid(workspace, rid).await?;
        }
    }
    txn.add_usage(workspace, domain, usage, history).await?;
    if entries.len() == batch as usize {
        return Ok((count, last));
    }
    let empty = Heads::default().to_bytes();
    let cleared = txn
        .clear_checkpoint_data(workspace, domain, floor, &empty, PRUNE_BATCH)
        .await?;
    if cleared == u64::from(PRUNE_BATCH) {
        return Ok((count, Some(last.unwrap_or(after))));
    }
    txn.set_pruned_floor(workspace, domain, floor).await?;
    Ok((count, None))
}

impl<B: Backend> Store<B> {
    /// Persists `floor`, or the floor the `retained_bytes` rule gives, when
    /// that lies above the published one (§19). Returns the published
    /// floor. The new floor commits before pruning deletes anything.
    async fn persist_floor(
        &self,
        workspace: Id,
        domain: Id,
        forced: Option<u64>,
    ) -> Result<(u64, bool)> {
        let retained = self.retention.retained_bytes;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_pruning(workspace, domain).await?;
                    txn.lock(&head_plan(workspace, domain, Vec::new())).await?;
                    let Some(row) = txn.domain(workspace, domain).await? else {
                        return Ok((0, false));
                    };
                    if row.deleted {
                        return Ok((row.retention_floor, false));
                    }
                    let rule = rule_floor(txn, retained, workspace, domain, &row).await?;
                    let floor = rule.max(forced.unwrap_or(0)).min(row.checkpoint_seq);
                    if floor > row.retention_floor {
                        txn.set_retention_floor(workspace, domain, floor).await?;
                        return Ok((floor, true));
                    }
                    Ok((row.retention_floor, false))
                })
            }))
            .await
    }

    /// Raises the domain's floor as the `retained_bytes` rule says, and
    /// returns the published floor (§19).
    pub async fn raise_floor(&self, workspace: Id, domain: Id) -> Result<u64> {
        Ok(self.persist_floor(workspace, domain, None).await?.0)
    }

    /// Deletes one batch of what the domain's floor passed, after `after`.
    /// Returns the entries it deleted and where the next batch continues,
    /// or `None` when pruning finished at the published floor.
    pub async fn prune_batch(
        &self,
        workspace: Id,
        domain: Id,
        after: u64,
    ) -> Result<(u64, Option<u64>)> {
        let now = self.clock().now();
        let batch = self.retention.prune_batch;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_pruning(workspace, domain).await?;
                    txn.lock(&head_plan(workspace, domain, Vec::new())).await?;
                    prune_batch(txn, workspace, domain, after, now, batch).await
                })
            }))
            .await
    }

    /// Prunes the domain down to its published floor.
    pub async fn prune_domain(&self, workspace: Id, domain: Id) -> Result<u64> {
        let (mut pruned, mut after) = (0, Some(0));
        while let Some(from) = after {
            let (count, next) = self.prune_batch(workspace, domain, from).await?;
            pruned += count;
            after = next;
        }
        Ok(pruned)
    }

    /// One pass of the server's background work (§19, §20): raises floors,
    /// prunes, reclaims deleted domains, and gives history up to the quota.
    /// Each instance may run it: the pruning lock serves one at a time, and
    /// every step reads its state again under the domain's head row.
    pub async fn maintain(&self) -> Result<Maintenance> {
        let mut report = Maintenance::default();
        let retained = self.retention.retained_bytes;
        let pending = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.pending_domains(retained, PASS_LIMIT).await })
            }))
            .await?;
        for domain in pending {
            let result = self.maintain_domain(domain, &mut report).await;
            report.note(result);
        }
        if let Some(quota) = self.retention.quota_bytes {
            let pressured = self
                .backend
                .read(Box::new(move |txn: &mut B::Read| {
                    Box::pin(async move { txn.pressured_workspaces(PASS_LIMIT).await })
                }))
                .await?;
            for workspace in pressured {
                let result = self.relieve(workspace, quota, &mut report).await;
                report.note(result);
            }
        }
        let drained = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.drained_closed_workspaces(PASS_LIMIT).await })
            }))
            .await?;
        for workspace in drained {
            let result = self.finish_closing(workspace).await;
            report.note(result);
        }
        Ok(report)
    }

    /// Deletes the workspace-level rows of a closed workspace whose last
    /// domain went (§14).
    async fn finish_closing(&self, workspace: Id) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&LockPlan {
                        workspace: Some(workspace),
                        policy_exclusive: true,
                        key_sequence: true,
                        domains: Vec::new(),
                        blobs: Vec::new(),
                    })
                    .await?;
                    txn.delete_closed_workspace_rows(workspace).await
                })
            }))
            .await
    }

    /// Reclaims a deleted domain, or raises another's floor and prunes it.
    async fn maintain_domain(&self, domain: PendingDomain, report: &mut Maintenance) -> Result<()> {
        if domain.deleted {
            if self.reclaim_domain(domain.workspace, domain.domain).await? {
                report.reclaimed += 1;
            }
            return Ok(());
        }
        let (_, raised) = self
            .persist_floor(domain.workspace, domain.domain, None)
            .await?;
        report.raised += u64::from(raised);
        report.pruned += self.prune_domain(domain.workspace, domain.domain).await?;
        Ok(())
    }

    /// Raises the workspace's floors one checkpoint at a time, starting
    /// with the domain that holds the most history, until usage and history
    /// fit the quota or every floor reaches its latest checkpoint (§20).
    /// When usage alone exceeds the quota, history can't bring it back, so
    /// no floor moves beyond the `retained_bytes` rule.
    async fn relieve(&self, workspace: Id, quota: u64, report: &mut Maintenance) -> Result<()> {
        loop {
            let (usage, history, domains) = self
                .backend
                .read(Box::new(move |txn: &mut B::Read| {
                    Box::pin(async move {
                        let usage = txn.workspace_usage(workspace).await?;
                        Ok((
                            usage.usage,
                            usage.history,
                            txn.domain_histories(workspace).await?,
                        ))
                    })
                }))
                .await?;
            let total = u128::from(usage) + u128::from(history);
            if usage > quota || total <= u128::from(quota) {
                break;
            }
            let Some(next) = domains
                .into_iter()
                .find(|d| d.retention_floor < d.checkpoint_seq)
            else {
                break;
            };
            let floor = self
                .backend
                .read(Box::new(move |txn: &mut B::Read| {
                    Box::pin(async move {
                        txn.checkpoint_above(workspace, next.domain, next.retention_floor)
                            .await
                    })
                }))
                .await?;
            let Some(floor) = floor else {
                break;
            };
            let (_, raised) = self
                .persist_floor(workspace, next.domain, Some(floor))
                .await?;
            report.raised += u64::from(raised);
            report.pruned += self.prune_domain(workspace, next.domain).await?;
        }
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { note_pressure(txn, workspace, Some(quota)).await })
            }))
            .await
    }

    /// One step of reclaiming a deleted domain (§19): batches of its log,
    /// checkpoints and keys, then its blobs through the collector. Returns
    /// whether the domain is gone.
    pub async fn reclaim_domain(&self, workspace: Id, domain: Id) -> Result<bool> {
        let now = self.clock().now();
        loop {
            let (deleted, done) = self
                .backend
                .write(Box::new(move |txn: &mut B::Write| {
                    Box::pin(async move {
                        txn.lock_pruning(workspace, domain).await?;
                        txn.lock(&head_plan(workspace, domain, Vec::new())).await?;
                        if !txn
                            .domain(workspace, domain)
                            .await?
                            .is_some_and(|d| d.deleted)
                        {
                            return Ok((0, true));
                        }
                        let rows = txn.reclaim_rows(workspace, domain, RECLAIM_BATCH).await?;
                        let blobs = txn
                            .expire_domain_blobs(workspace, domain, now, RECLAIM_BATCH)
                            .await?;
                        let idle = rows == 0 && blobs == 0;
                        if idle && !txn.domain_has_blobs(workspace, domain).await? {
                            txn.delete_domain_row(workspace, domain).await?;
                            return Ok((rows + blobs, true));
                        }
                        Ok((rows + blobs, false))
                    })
                }))
                .await?;
            if done {
                return Ok(true);
            }
            if deleted == 0 {
                // The collector still has blobs of this domain to take.
                return Ok(false);
            }
        }
    }

    /// Marks every domain of a closed workspace deleted and reclaims what
    /// it can (§14, §19). Returns whether every domain is gone.
    pub async fn reclaim_workspace(&self, workspace: Id, domains: Vec<Id>) -> Result<bool> {
        let mut sorted = domains.clone();
        sorted.sort();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&LockPlan {
                        workspace: Some(workspace),
                        policy_exclusive: true,
                        key_sequence: false,
                        domains: sorted.clone(),
                        blobs: Vec::new(),
                    })
                    .await?;
                    for domain in &sorted {
                        txn.mark_domain_deleted(workspace, *domain).await?;
                    }
                    Ok(())
                })
            }))
            .await?;
        let mut done = true;
        for domain in domains {
            done &= self.reclaim_domain(workspace, domain).await?;
        }
        Ok(done)
    }

    /// The blobs due for the collector.
    pub async fn collectable_blobs(&self) -> Result<Vec<BlobToCollect>> {
        let now = self.clock().now();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.collectable_blobs(now, PASS_LIMIT).await })
            }))
            .await
    }

    /// The collector's first step (§19): under the domain's head row and
    /// the blob's row, marks a blob no operation lists and whose time has
    /// come, and frees its usage. Returns its segments, whose store objects
    /// the caller deletes next, or `None` when a push got there first.
    pub async fn mark_blob(&self, target: BlobToCollect) -> Result<Option<Vec<Digest>>> {
        let now = self.clock().now();
        let BlobToCollect {
            workspace,
            domain,
            blob,
        } = target;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&head_plan(workspace, domain, vec![blob])).await?;
                    let Some(row) = txn.blob(workspace, blob).await? else {
                        return Ok(None);
                    };
                    if !row.marked {
                        let due = row.delete_after.is_some_and(|at| at <= now);
                        if !due || txn.blob_listed(workspace, blob).await? {
                            return Ok(None);
                        }
                        txn.mark_blob(workspace, blob).await?;
                        txn.add_usage(workspace, domain, -signed(row.length), 0)
                            .await?;
                    }
                    Ok(Some(txn.blob_segments(workspace, blob).await?))
                })
            }))
            .await
    }

    /// The collector's last step: deletes a marked blob's row once its
    /// store objects are gone.
    pub async fn finish_blob(&self, target: BlobToCollect) -> Result<()> {
        let BlobToCollect {
            workspace,
            domain,
            blob,
        } = target;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&head_plan(workspace, domain, vec![blob])).await?;
                    if txn.blob(workspace, blob).await?.is_some_and(|b| b.marked) {
                        txn.delete_blob(workspace, blob).await?;
                    }
                    Ok(())
                })
            }))
            .await
    }

    /// Deletes share links past their expiry, each under its domain's head
    /// row, which frees its snapshot's usage (§20). Returns how many.
    pub(crate) async fn delete_expired_shares(&self, now: u64) -> Result<u64> {
        let expired = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.expired_shares(now, PASS_LIMIT).await })
            }))
            .await?;
        let mut deleted = 0;
        for (token_hash, workspace, domain) in expired {
            deleted += self
                .backend
                .write(Box::new(move |txn: &mut B::Write| {
                    Box::pin(async move {
                        txn.lock(&head_plan(workspace, domain, Vec::new())).await?;
                        let Some(row) =
                            txn.share(token_hash).await?.filter(|s| s.expires_at <= now)
                        else {
                            return Ok(0);
                        };
                        txn.delete_share(token_hash).await?;
                        txn.add_usage(workspace, domain, -signed(row.share.len() as u64), 0)
                            .await?;
                        Ok(1)
                    })
                }))
                .await?;
        }
        Ok(deleted)
    }
}
