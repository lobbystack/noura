//! The live set as a store keeps it (§13, §17): the checks run on what the
//! store reads, through `sync_protocol::live::judge`, so the store and every
//! client compute the same live sets.

use sync_protocol::live::{LiveError, LiveOp, class_of, judge};
use sync_protocol::records::OpClass;
use sync_protocol::types::Id;

use crate::{DeleteRow, ReadTxn, Result, WriteTxn};

/// The live-set checks of §13 for `op`.
pub(crate) async fn check(
    txn: &mut impl ReadTxn,
    workspace: Id,
    op: &LiveOp,
) -> Result<std::result::Result<OpClass, LiveError>> {
    let rid_exists = txn.rid_domain(workspace, op.rid).await?.is_some();
    let mut unknown_cover = false;
    for cover in &op.covers {
        if !txn
            .op_exists(workspace, op.rid, cover.device, cover.counter)
            .await?
        {
            unknown_cover = true;
            break;
        }
    }
    let covered = txn
        .covered(workspace, op.rid, op.device, op.counter, u64::MAX)
        .await?;
    let live = txn.live_ops(workspace, op.rid, u64::MAX).await?;
    Ok(judge(op, rid_exists, unknown_cover, covered, &live))
}

/// Records `op` in `domain`, classing it as the server does, and records
/// the `rid` in that domain when it is new.
pub(crate) async fn record(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    op: &LiveOp,
) -> Result<OpClass> {
    let known = txn.rid_domain(workspace, op.rid).await?.is_some();
    let class = class_of(op, known);
    if !known {
        txn.insert_rid(workspace, op.rid, domain).await?;
    }
    txn.insert_operation(workspace, domain, op, class).await?;
    Ok(class)
}

/// The deletes a checkpoint's rising floor drops: each delete whose
/// sequence lies in (`previous_floor`, `floor`] and that is its `rid`'s
/// only live operation at `covered_seq`, sorted by `rid`, device and
/// counter.
pub(crate) async fn floor_drops(
    txn: &mut impl ReadTxn,
    workspace: Id,
    domain: Id,
    previous_floor: u64,
    floor: u64,
    covered_seq: u64,
) -> Result<Vec<DeleteRow>> {
    let mut dropped = Vec::new();
    if floor <= previous_floor {
        return Ok(dropped);
    }
    for candidate in txn
        .deletes_between(workspace, domain, previous_floor, floor)
        .await?
    {
        let live = txn.live_ops(workspace, candidate.rid, covered_seq).await?;
        if let [(op, _)] = live.as_slice()
            && op.delete
            && op.device == candidate.op.device
            && op.counter == candidate.op.counter
        {
            dropped.push(candidate);
        }
    }
    dropped.sort_by_key(|d| (d.rid, d.op.device, d.op.counter));
    Ok(dropped)
}

/// Applies a checkpoint's rising floor (§17): the deletes of
/// [`floor_drops`] leave the live set for good. Returns them.
pub(crate) async fn apply_floor(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    previous_floor: u64,
    floor: u64,
    covered_seq: u64,
) -> Result<Vec<DeleteRow>> {
    let dropped = floor_drops(txn, workspace, domain, previous_floor, floor, covered_seq).await?;
    for delete in &dropped {
        txn.drop_delete(workspace, delete.rid, delete.op.device, delete.op.counter)
            .await?;
    }
    Ok(dropped)
}
