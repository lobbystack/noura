//! Checkpoints (§17, §18): due points, the checks and recomputation of a
//! pushed checkpoint, and the routes that serve checkpoints, their chain
//! and the manifest at a checkpoint.

use std::collections::{BTreeMap, BTreeSet};

use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::manifest::{
    BUCKETS, bucket_hash, bucket_of, descent_path, heads_digest, leaf_hash, node_children,
    root_of_buckets, skip_number,
};
use sync_protocol::messages::{
    CheckpointChainResponse, CheckpointResponse, CursorCompactedDetail, Entry, EntryRecord,
    LeafRef, LiveCursor, LiveResponse, MAX_BODY, ManifestBucketsResponse, ManifestLeavesResponse,
    ManifestNodeResponse, PolicyEntry, PushResponse, Receipt,
};
use sync_protocol::policy::{PolicyChain, Snapshot};
use sync_protocol::records::{Checkpoint, CheckpointBundle, Epoch, Heads, Ref, Signed};
use sync_protocol::types::{Digest, Id};

use crate::policy::{self, PolicyCache};
use crate::push::{ENTRY_EPOCH, ENTRY_POLICY};
use crate::reads::{authorize_domain, reader};
use crate::{
    CheckpointInterval, CheckpointRow, DeleteRow, Error, LockPlan, ReadTxn, Refusal, Result,
    Session, WriteTxn, refuse,
};

/// The log entry record type of a checkpoint bundle (§15).
pub(crate) const ENTRY_CHECKPOINT: u8 = 5;

/// `rid`s read per query while walking a bucket or a `live` page.
const RIDS_PER_QUERY: u32 = 512;

/// Counts a new log entry of `weight` toward the next due point (§17).
/// A checkpoint falls due at the entry that brings the log to the
/// interval's entries or weight since the latest checkpoint entry, and at
/// an entry that `due_now` marks: a policy entry that ends a writer's
/// access, or a replacement's epoch entry.
pub(crate) async fn note_entry(
    txn: &mut impl WriteTxn,
    interval: CheckpointInterval,
    workspace: Id,
    domain: Id,
    seq: u64,
    weight: u64,
    due_now: bool,
) -> Result<()> {
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| Error::internal("a log entry's domain has no head row"))?;
    let entries = row.entries_since_checkpoint + 1;
    let weight = row.weight_since_checkpoint.saturating_add(weight);
    let mut due = row.checkpoint_due;
    if due == 0 && (entries >= interval.entries || weight >= interval.weight) {
        due = seq;
    }
    if due_now {
        due = due.max(seq);
    }
    txn.set_checkpoint_progress(workspace, domain, entries, weight, due)
        .await
}

/// Whether a policy change from `prev` to `next` ends the write access, in
/// `domain`, of the latest checkpoint's signer or of a device with a chain
/// head there (§17), which makes a checkpoint due at its entry.
pub(crate) async fn ends_write_access(
    txn: &mut impl ReadTxn,
    workspace: Id,
    domain: Id,
    prev: &Snapshot,
    next: &Snapshot,
) -> Result<bool> {
    let mut devices = txn.chain_head_devices(workspace, domain).await?;
    if let Some(latest) = txn.latest_checkpoint(workspace, domain).await? {
        devices.push(signed(&latest)?.body.signer);
    }
    Ok(devices
        .iter()
        .any(|d| prev.can_write(d, &domain) && !next.can_write(d, &domain)))
}

/// Appends a policy entry to a domain's log and counts it (§8, §17).
pub(crate) async fn append_policy_entry(
    txn: &mut impl WriteTxn,
    interval: CheckpointInterval,
    workspace: Id,
    domain: Id,
    entry: &PolicyEntry,
    prev: Option<&Snapshot>,
    next: &Snapshot,
) -> Result<u64> {
    let due_now = match prev {
        Some(prev) => ends_write_access(txn, workspace, domain, prev, next).await?,
        None => false,
    };
    let bytes = entry.to_bytes();
    let seq = txn
        .append_entry(workspace, domain, ENTRY_POLICY, &bytes)
        .await?;
    note_entry(
        txn,
        interval,
        workspace,
        domain,
        seq,
        bytes.len() as u64,
        due_now,
    )
    .await?;
    Ok(seq)
}

fn signed(row: &CheckpointRow) -> Result<Signed<Checkpoint>> {
    Signed::<Checkpoint>::from_bytes(&row.record).map_err(Error::internal)
}

fn stored_heads(row: &CheckpointRow) -> Result<Heads> {
    Heads::from_bytes(&row.heads).map_err(Error::internal)
}

fn bundle_of(row: &CheckpointRow) -> Result<CheckpointBundle> {
    Ok(CheckpointBundle {
        checkpoint: Nested(signed(row)?),
        heads: stored_heads(row)?,
    })
}

fn buckets_from(bytes: &[u8]) -> Result<Vec<Digest>> {
    if bytes.len() != BUCKETS * 32 {
        return Err(Error::internal("stored bucket hashes aren't 4,096 digests"));
    }
    Ok(bytes
        .as_chunks::<32>()
        .0
        .iter()
        .map(|c| Digest(*c))
        .collect())
}

fn empty_buckets() -> Vec<Digest> {
    (0..BUCKETS as u16).map(|i| bucket_hash(i, &[])).collect()
}

/// The first and last `rid` of bucket `index`.
fn bucket_range(index: u16) -> (Id, Id) {
    let mut low = [0u8; 16];
    low[0] = (index >> 4) as u8;
    low[1] = ((index & 0x0f) << 4) as u8;
    let mut high = [0xffu8; 16];
    high[0] = low[0];
    high[1] = low[1] | 0x0f;
    (Id(low), Id(high))
}

/// The leaves of bucket `index` at sequence `at`, sorted by `rid`, each
/// with its live refs sorted by device and counter, leaving out the
/// operations in `exclude`.
async fn bucket_leaves(
    txn: &mut impl ReadTxn,
    workspace: Id,
    domain: Id,
    index: u16,
    at: u64,
    exclude: &BTreeSet<(Id, Id, u64)>,
) -> Result<Vec<(Id, Vec<Ref>)>> {
    let (mut from, through) = bucket_range(index);
    let mut leaves = Vec::new();
    loop {
        let rids = txn
            .domain_rids(workspace, domain, from, through, RIDS_PER_QUERY)
            .await?;
        let full = rids.len() == RIDS_PER_QUERY as usize;
        for rid in &rids {
            let refs = live_refs(txn, workspace, *rid, at, exclude).await?;
            if !refs.is_empty() {
                leaves.push((*rid, refs));
            }
        }
        match rids.last() {
            Some(last) if full => match next_id(last) {
                Some(next) => from = next,
                None => break,
            },
            _ => break,
        }
    }
    Ok(leaves)
}

async fn live_refs(
    txn: &mut impl ReadTxn,
    workspace: Id,
    rid: Id,
    at: u64,
    exclude: &BTreeSet<(Id, Id, u64)>,
) -> Result<Vec<Ref>> {
    let mut refs: Vec<Ref> = txn
        .live_ops(workspace, rid, at)
        .await?
        .into_iter()
        .filter(|(op, _)| !exclude.contains(&(rid, op.device, op.counter)))
        .map(|(op, _)| Ref {
            device: op.device,
            counter: op.counter,
            seq: op.seq,
            op_digest: op.digest,
        })
        .collect();
    refs.sort_by_key(|r| (r.device, r.counter));
    Ok(refs)
}

/// The `rid` right after `id` in byte order.
fn next_id(id: &Id) -> Option<Id> {
    let value = u128::from_be_bytes(id.0).checked_add(1)?;
    Some(Id(value.to_be_bytes()))
}

/// What the server computes for a pushed checkpoint before it opens the
/// committing transaction (§17).
pub(crate) struct Plan {
    previous: Option<CheckpointRow>,
    buckets: Vec<Digest>,
    root: Digest,
    live_objects: u64,
    heads: Vec<Ref>,
    dropped: Vec<DeleteRow>,
}

/// The checks of §17 that a pushed checkpoint's own fields and the
/// committed log decide, then the recomputation of its root, leaf count
/// and heads from the previous checkpoint's stored bucket hashes and
/// heads, the records between the two, and the lone deletes its floor
/// drops. The records at or below `covered_seq` are committed, so this
/// runs in a read transaction.
pub(crate) async fn plan(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    bundle: &CheckpointBundle,
) -> Result<Plan> {
    let record = &bundle.checkpoint.0;
    let c = &record.body;
    let domain = c.domain;
    let Some((_, policy)) = policy::load(txn, cache, workspace).await? else {
        return Err(refuse("not_found", "no such workspace"));
    };
    check_signer(&policy, session, c)?;
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "no such domain"))?;
    check_rotation(&policy, domain, row.epoch_policy_revision)?;
    let previous = txn.latest_checkpoint(workspace, domain).await?;
    if previous
        .as_ref()
        .is_some_and(|p| p.digest == record.digest())
    {
        // A retry: the commit returns the original receipt.
        return Ok(Plan {
            previous,
            buckets: Vec::new(),
            root: Digest::ZERO,
            live_objects: 0,
            heads: Vec::new(),
            dropped: Vec::new(),
        });
    }
    check_policy_and_epoch(txn, workspace, c).await?;
    check_link(txn, workspace, c, previous.as_ref()).await?;
    check_bounds(c, previous.as_ref(), row.retention_floor, row.head_seq)?;

    let (prev_covered, prev_floor, prev_live, prev_revision) = match &previous {
        Some(p) => {
            let body = signed(p)?.body;
            (
                p.covered_seq,
                p.floor,
                body.live_objects,
                Some(body.policy_revision),
            )
        }
        None => (0, 0, 0, None),
    };
    let mut buckets = match &previous {
        Some(p) => buckets_from(
            &txn.checkpoint_buckets(workspace, domain, p.number)
                .await?
                .ok_or_else(|| Error::internal("a checkpoint lacks its bucket hashes"))?,
        )?,
        None => empty_buckets(),
    };
    let mut heads: BTreeMap<Id, Ref> = match &previous {
        Some(p) => stored_heads(p)?
            .iter()
            .map(|h| (h.device, h.clone()))
            .collect(),
        None => BTreeMap::new(),
    };
    let ops = txn
        .ops_between(workspace, domain, prev_covered, c.covered_seq)
        .await?;
    let dropped =
        crate::live::floor_drops(txn, workspace, domain, prev_floor, c.floor, c.covered_seq)
            .await?;
    let exclude: BTreeSet<(Id, Id, u64)> = dropped
        .iter()
        .map(|d| (d.rid, d.op.device, d.op.counter))
        .collect();
    let touched: BTreeSet<u16> = ops
        .iter()
        .map(|(rid, _)| bucket_of(rid))
        .chain(dropped.iter().map(|d| bucket_of(&d.rid)))
        .collect();
    let mut live_objects = prev_live;
    for index in touched {
        let before = bucket_leaves(
            txn,
            workspace,
            domain,
            index,
            prev_covered,
            &BTreeSet::new(),
        )
        .await?
        .len() as u64;
        let after = bucket_leaves(txn, workspace, domain, index, c.covered_seq, &exclude).await?;
        let hashes: Vec<Digest> = after
            .iter()
            .map(|(rid, refs)| leaf_hash(rid, refs))
            .collect();
        buckets[usize::from(index)] = bucket_hash(index, &hashes);
        live_objects = (live_objects + after.len() as u64)
            .checked_sub(before)
            .ok_or_else(|| Error::internal("a bucket lost more leaves than the manifest held"))?;
    }
    for (_, op) in ops {
        match heads.get(&op.device) {
            Some(head) if head.counter >= op.counter => {}
            _ => {
                heads.insert(op.device, op);
            }
        }
    }
    if let Some(revision) = prev_revision {
        heads.retain(|device, _| !policy.card_gone(device, revision));
    }
    Ok(Plan {
        previous,
        root: root_of_buckets(&buckets),
        buckets,
        live_objects,
        heads: heads.into_values().collect(),
        dropped,
    })
}

/// The signer is the session's device, active in the workspace, and can
/// write the domain at the latest policy entry (§17).
fn check_signer(policy: &PolicyChain, session: Session, c: &Checkpoint) -> Result<()> {
    let snapshot = policy.head_snapshot();
    if reader(snapshot, session).is_none() {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    if !snapshot.domains.contains_key(&c.domain) {
        return Err(refuse("not_found", "no such domain"));
    }
    if !snapshot.can_write(&c.signer, &c.domain) {
        return Err(refuse(
            "forbidden",
            "the signer can't write the domain at the latest policy entry",
        ));
    }
    Ok(())
}

/// The domain takes no record while it waits for a rotation (§9).
fn check_rotation(policy: &PolicyChain, domain: Id, epoch_policy_revision: u64) -> Result<()> {
    let (head_revision, _) = policy.head();
    if policy
        .rotation_required(&domain, epoch_policy_revision, head_revision)
        .is_some()
    {
        return Err(refuse(
            "rotation_pending",
            "the domain waits for a rotation",
        ));
    }
    Ok(())
}

/// `policy_revision` and `policy_head` name the latest policy entry at or
/// below `covered_seq`, and `epoch` and `epoch_record` the latest epoch
/// entry there.
async fn check_policy_and_epoch(
    txn: &mut impl ReadTxn,
    workspace: Id,
    c: &Checkpoint,
) -> Result<()> {
    let policy_entry = txn
        .latest_entry(workspace, c.domain, ENTRY_POLICY, c.covered_seq)
        .await?
        .map(|e| PolicyEntry::from_bytes(&e.record).map_err(Error::internal))
        .transpose()?;
    if policy_entry.is_none_or(|e| e.revision != c.policy_revision || e.head != c.policy_head) {
        return Err(refuse(
            "manifest_mismatch",
            "policy_revision and policy_head don't name the latest policy entry at covered_seq",
        ));
    }
    let epoch = txn
        .latest_entry(workspace, c.domain, ENTRY_EPOCH, c.covered_seq)
        .await?
        .map(|e| Signed::<Epoch>::from_bytes(&e.record).map_err(Error::internal))
        .transpose()?;
    if epoch.is_none_or(|e| e.body.epoch != c.epoch || e.digest() != c.epoch_record) {
        return Err(refuse(
            "manifest_mismatch",
            "epoch and epoch_record don't name the latest epoch entry at covered_seq",
        ));
    }
    Ok(())
}

/// `previous` names the domain's latest checkpoint, else
/// `checkpoint_stale`, and `number` and `skip` follow from it.
async fn check_link(
    txn: &mut impl ReadTxn,
    workspace: Id,
    c: &Checkpoint,
    previous: Option<&CheckpointRow>,
) -> Result<()> {
    let (latest_digest, latest_number) =
        previous.map_or((Digest::ZERO, 0), |p| (p.digest, p.number));
    if c.previous != latest_digest {
        return Err(refuse(
            "checkpoint_stale",
            "previous isn't the domain's latest checkpoint",
        ));
    }
    let skip = skip_number(c.number);
    let skip_digest = if skip == 0 {
        Some(Digest::ZERO)
    } else {
        txn.checkpoint_by_number(workspace, c.domain, skip)
            .await?
            .map(|row| row.digest)
    };
    if c.number != latest_number + 1 || skip_digest != Some(c.skip) {
        return Err(refuse(
            "manifest_mismatch",
            "number and skip don't follow from previous",
        ));
    }
    Ok(())
}

/// `covered_seq` sits at or above the previous checkpoint's entry and the
/// retention floor, and within the log; `floor` lies between the previous
/// checkpoint's `floor` and the retention floor.
fn check_bounds(
    c: &Checkpoint,
    previous: Option<&CheckpointRow>,
    retention_floor: u64,
    head_seq: u64,
) -> Result<()> {
    let (prev_seq, prev_floor) = previous.map_or((0, 0), |p| (p.seq, p.floor));
    if c.covered_seq < prev_seq || c.covered_seq < retention_floor || c.covered_seq > head_seq {
        return Err(refuse(
            "checkpoint_not_due",
            "covered_seq lies below the previous checkpoint's entry or the retention floor, or above the log",
        ));
    }
    if c.floor < prev_floor || c.floor > retention_floor {
        return Err(refuse(
            "manifest_mismatch",
            "floor lies outside the previous checkpoint's floor and the retention floor",
        ));
    }
    Ok(())
}

/// Commits a checkpoint under the domain's head row (§17): `previous`
/// again, before a mismatch, so a checkpoint that raced another gets
/// `checkpoint_stale`; the due point; then the planned root, leaf count
/// and heads. The checkpoint becomes a log entry and the domain's latest.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit(
    txn: &mut impl WriteTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    bundle: &CheckpointBundle,
    plan: Plan,
) -> Result<PushResponse> {
    let domain = bundle.checkpoint.0.body.domain;
    txn.lock(&LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: false,
        domains: vec![domain],
        blobs: Vec::new(),
    })
    .await?;
    let Some((ws_row, policy)) = policy::load(txn, cache, workspace).await? else {
        return Err(refuse("not_found", "no such workspace"));
    };
    let receipt = commit_locked(txn, &policy, session, workspace, bundle, plan, false).await?;
    Ok(PushResponse {
        policy_revision: ws_row.policy_revision,
        receipts: List(vec![receipt]),
    })
}

/// [`commit`] under locks the caller holds, against `policy`. A checkpoint
/// that follows an approval or a grant in its push (`floor_setting`) needs
/// no due point: it sets the account's history floor (§9, §17).
pub(crate) async fn commit_locked(
    txn: &mut impl WriteTxn,
    policy: &PolicyChain,
    session: Session,
    workspace: Id,
    bundle: &CheckpointBundle,
    plan: Plan,
    floor_setting: bool,
) -> Result<Receipt> {
    let record = &bundle.checkpoint.0;
    let c = &record.body;
    let domain = c.domain;
    let digest = record.digest();
    check_signer(policy, session, c)?;
    let receipt = |seq| Receipt {
        record_digest: digest,
        domain,
        seq,
    };
    let latest = txn.latest_checkpoint(workspace, domain).await?;
    if let Some(latest) = &latest
        && latest.digest == digest
    {
        return Ok(receipt(latest.seq));
    }
    let latest_digest = latest.as_ref().map_or(Digest::ZERO, |l| l.digest);
    let planned_digest = plan.previous.as_ref().map_or(Digest::ZERO, |p| p.digest);
    if latest_digest != c.previous || latest_digest != planned_digest {
        return Err(refuse(
            "checkpoint_stale",
            "previous isn't the domain's latest checkpoint",
        ));
    }
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "no such domain"))?;
    check_rotation(policy, domain, row.epoch_policy_revision)?;
    if !floor_setting && (row.checkpoint_due == 0 || c.covered_seq < row.checkpoint_due) {
        return Err(refuse(
            "checkpoint_not_due",
            "no checkpoint is due, or covered_seq falls short of the due point",
        ));
    }
    if plan.root != c.manifest_root
        || plan.live_objects != c.live_objects
        || heads_digest(&plan.heads) != c.heads_digest
        || heads_digest(&bundle.heads) != c.heads_digest
    {
        return Err(refuse(
            "manifest_mismatch",
            "the root, live_objects or heads_digest differs from the server's",
        ));
    }
    let bytes = bundle.to_bytes();
    let seq = txn
        .append_entry(workspace, domain, ENTRY_CHECKPOINT, &bytes)
        .await?;
    let buckets: Vec<u8> = plan.buckets.iter().flat_map(|d| d.0).collect();
    txn.insert_checkpoint(
        workspace,
        domain,
        &CheckpointRow {
            number: c.number,
            digest,
            seq,
            covered_seq: c.covered_seq,
            floor: c.floor,
            record: record.to_bytes(),
            heads: bundle.heads.to_bytes(),
        },
        &buckets,
    )
    .await?;
    for delete in &plan.dropped {
        txn.drop_delete(workspace, delete.rid, delete.op.device, delete.op.counter)
            .await?;
    }
    txn.set_checkpoint_progress(workspace, domain, 0, 0, 0)
        .await?;
    Ok(receipt(seq))
}

/// `GET .../checkpoint`.
pub(crate) async fn latest(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
) -> Result<CheckpointResponse> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let row = txn
        .latest_checkpoint(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "the domain holds no checkpoint"))?;
    Ok(CheckpointResponse {
        seq: row.seq,
        bundle: bundle_of(&row)?,
    })
}

/// `GET .../checkpoints/{digest}`, as an `entry`.
pub(crate) async fn by_digest(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    digest: Digest,
) -> Result<Entry> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let row = txn
        .checkpoint_by_digest(workspace, domain, digest)
        .await?
        .ok_or_else(|| refuse("not_found", "the domain holds no such checkpoint"))?;
    Ok(Entry {
        seq: row.seq,
        record: EntryRecord::Checkpoint(bundle_of(&row)?),
    })
}

/// `GET .../checkpoint-chain?from&to`: the records on the walk from
/// `from` back to `to`, through `skip` wherever that doesn't pass `to` and
/// through `previous` elsewhere, newest first, as many as fit 1 MiB.
pub(crate) async fn chain(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    from: Digest,
    to: Digest,
) -> Result<CheckpointChainResponse> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let not_found = || refuse("not_found", "the domain holds no such checkpoint");
    let from = txn
        .checkpoint_by_digest(workspace, domain, from)
        .await?
        .ok_or_else(not_found)?;
    let to = txn
        .checkpoint_by_digest(workspace, domain, to)
        .await?
        .ok_or_else(not_found)?;
    if to.number > from.number {
        return Err(refuse("bad_request", "to comes after from"));
    }
    let mut budget = MAX_BODY - 4;
    let mut records = Vec::new();
    for number in descent_path(from.number, to.number) {
        let row = txn
            .checkpoint_by_number(workspace, domain, number)
            .await?
            .ok_or_else(|| Error::internal("a checkpoint number on the walk is missing"))?;
        let size = 4 + row.record.len();
        if size > budget {
            break;
        }
        budget -= size;
        records.push(Nested(signed(&row)?));
    }
    Ok(CheckpointChainResponse {
        checkpoints: List(records),
    })
}

/// The servable checkpoint at `at` (§26): one whose `covered_seq` is `at`,
/// at or above the retention floor, else `cursor_compacted`.
async fn servable(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    at: u64,
) -> Result<CheckpointRow> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "no such domain"))?;
    if at
        < txn
            .history_floor(workspace, session.account, domain)
            .await?
    {
        return Err(refuse(
            "before_history_floor",
            "that checkpoint lies below the account's history floor",
        ));
    }
    match txn.checkpoint_at(workspace, domain, at).await? {
        Some(checkpoint) if at >= row.retention_floor => Ok(checkpoint),
        _ => Err(Error::Refused(Refusal {
            detail: Some(
                CursorCompactedDetail {
                    retention_floor: row.retention_floor,
                    checkpoint_covered_seq: row.checkpoint_seq,
                }
                .to_bytes(),
            ),
            ..Refusal::new(
                "cursor_compacted",
                "no servable checkpoint covers that sequence",
            )
        })),
    }
}

fn after_cursor(cursor: Option<&LiveCursor>, rid: &Id, op: &Ref) -> bool {
    cursor.is_none_or(|c| (*rid, op.device, op.counter) > (c.rid, c.device, c.counter))
}

/// `GET .../live?at&after` or `?at&rid&after`: the operations live at a
/// servable checkpoint, ordered by `rid`, device and counter, as many as
/// fit 1 MiB.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn live(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    at: u64,
    rid: Option<Id>,
    after: Option<LiveCursor>,
) -> Result<LiveResponse> {
    servable(txn, cache, session, workspace, domain, at).await?;
    // `at`, the cursor's presence and fields, and the entry count.
    let mut budget = MAX_BODY - (8 + 1 + 40 + 4);
    let mut entries = Vec::new();
    let mut from = match (&rid, &after) {
        (Some(rid), _) => *rid,
        (None, Some(cursor)) => cursor.rid,
        (None, None) => Id([0; 16]),
    };
    let through = rid.unwrap_or(Id([0xff; 16]));
    let none = BTreeSet::new();
    loop {
        let rids = txn
            .domain_rids(workspace, domain, from, through, RIDS_PER_QUERY)
            .await?;
        let full = rids.len() == RIDS_PER_QUERY as usize;
        for rid in &rids {
            for op in live_refs(txn, workspace, *rid, at, &none).await? {
                if !after_cursor(after.as_ref(), rid, &op) {
                    continue;
                }
                let raw = txn
                    .entries_after(workspace, domain, op.seq - 1, 1)
                    .await?
                    .into_iter()
                    .next()
                    .filter(|e| e.seq == op.seq)
                    .ok_or_else(|| Error::internal("a live operation has no log entry"))?;
                let size = 8 + 1 + 4 + raw.record.len();
                if size > budget {
                    let last = entries
                        .last()
                        .map(|(cursor, _): &(LiveCursor, Entry)| cursor.clone());
                    return Ok(LiveResponse {
                        at,
                        next: last.or(after),
                        entries: List(entries.into_iter().map(|(_, e)| e).collect()),
                    });
                }
                budget -= size;
                entries.push((
                    LiveCursor {
                        rid: *rid,
                        device: op.device,
                        counter: op.counter,
                    },
                    crate::pull::entry(raw)?,
                ));
            }
        }
        match rids.last().and_then(next_id) {
            Some(next) if full => from = next,
            _ => break,
        }
    }
    Ok(LiveResponse {
        at,
        next: None,
        entries: List(entries.into_iter().map(|(_, e)| e).collect()),
    })
}

/// `GET .../manifest?at&buckets=all`.
pub(crate) async fn manifest_buckets(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    at: u64,
) -> Result<ManifestBucketsResponse> {
    let buckets = stored_buckets(txn, cache, session, workspace, domain, at).await?;
    Ok(ManifestBucketsResponse {
        buckets: List(buckets),
    })
}

async fn stored_buckets(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    at: u64,
) -> Result<Vec<Digest>> {
    let checkpoint = servable(txn, cache, session, workspace, domain, at).await?;
    buckets_from(
        &txn.checkpoint_buckets(workspace, domain, checkpoint.number)
            .await?
            .ok_or_else(|| Error::internal("a servable checkpoint lacks its bucket hashes"))?,
    )
}

/// `GET .../manifest?at&level&index`, levels 0 to 2: a node's 16 children.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn manifest_node(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    at: u64,
    level: u8,
    index: u16,
) -> Result<ManifestNodeResponse> {
    let buckets = stored_buckets(txn, cache, session, workspace, domain, at).await?;
    let children = node_children(&buckets, level, index)
        .ok_or_else(|| refuse("bad_request", "no such manifest node"))?;
    Ok(ManifestNodeResponse {
        children: List(children),
    })
}

/// `GET .../manifest?at&level=3&index&after`: a bucket's leaf refs.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn manifest_leaves(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    at: u64,
    index: u16,
    after: Option<LiveCursor>,
) -> Result<ManifestLeavesResponse> {
    if usize::from(index) >= BUCKETS {
        return Err(refuse("bad_request", "no such bucket"));
    }
    servable(txn, cache, session, workspace, domain, at).await?;
    let leaves = bucket_leaves(txn, workspace, domain, index, at, &BTreeSet::new()).await?;
    // The entry count, the cursor's presence and fields.
    let fits = (MAX_BODY - (4 + 1 + 40)) / (16 + 16 + 8 + 8 + 32);
    let mut refs = Vec::new();
    let mut next = None;
    'leaves: for (rid, ops) in leaves {
        for op in ops {
            if !after_cursor(after.as_ref(), &rid, &op) {
                continue;
            }
            if refs.len() == fits {
                next = refs.last().map(|r: &LeafRef| LiveCursor {
                    rid: r.rid,
                    device: r.op.device,
                    counter: r.op.counter,
                });
                break 'leaves;
            }
            refs.push(LeafRef { rid, op });
        }
    }
    Ok(ManifestLeavesResponse {
        leaves: Sorted::new(refs).map_err(|e| Error::internal(e.name()))?,
        next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(covered_seq: u64, floor: u64) -> Checkpoint {
        Checkpoint {
            workspace: Id([1; 16]),
            domain: Id([2; 16]),
            previous: Digest::ZERO,
            number: 2,
            skip: Digest::ZERO,
            covered_seq,
            floor,
            policy_revision: 1,
            policy_head: Digest::ZERO,
            live_objects: 0,
            manifest_root: Digest::ZERO,
            heads_digest: Digest::ZERO,
            epoch: 1,
            epoch_record: Digest::ZERO,
            signer: Id([3; 16]),
        }
    }

    fn previous(seq: u64, floor: u64) -> CheckpointRow {
        CheckpointRow {
            number: 1,
            digest: Digest::ZERO,
            seq,
            covered_seq: seq - 1,
            floor,
            record: Vec::new(),
            heads: Vec::new(),
        }
    }

    fn code(result: Result<()>) -> &'static str {
        match result {
            Ok(()) => "ok",
            Err(Error::Refused(refusal)) => refusal.code,
            Err(other) => panic!("{other:?}"),
        }
    }

    /// The bounds of §17 against a raised retention floor, which no store
    /// raises until pruning lands: `covered_seq` at or above the previous
    /// checkpoint's entry and the floor, and `floor` between the previous
    /// checkpoint's and the retention floor.
    #[test]
    fn bounds_follow_the_retention_floor() {
        let prev = previous(5, 2);
        let check = |covered, floor, retention| {
            code(check_bounds(
                &checkpoint(covered, floor),
                Some(&prev),
                retention,
                40,
            ))
        };
        assert_eq!(check(20, 10, 10), "ok");
        assert_eq!(check(9, 2, 10), "checkpoint_not_due", "below the floor");
        assert_eq!(check(10, 2, 10), "ok", "at the floor");
        assert_eq!(check(4, 2, 2), "checkpoint_not_due", "below the entry");
        assert_eq!(check(41, 2, 2), "checkpoint_not_due", "above the log");
        assert_eq!(check(20, 11, 10), "manifest_mismatch", "above the floor");
        assert_eq!(check(20, 1, 10), "manifest_mismatch", "below the previous");
    }
}
