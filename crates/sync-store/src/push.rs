//! Push (§15): decoding and the checks that need no transaction, then the
//! checks of §13 against the current state plus the records before each
//! one, and the commit, all or none.

use std::sync::Arc;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Bytes, Decode, DecodeError, Encode, List, Reader, Text, UNBOUNDED};
use sync_protocol::live::{LiveOp, judge};
use sync_protocol::messages::{
    ChainConflictDetail, PushFailureDetail, PushResponse, Receipt, RecordRef,
};
use sync_protocol::padding::is_padded_ciphertext_length;
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Operation, Signed};
use sync_protocol::types::{Digest, Id};

use crate::policy::{self, PolicyCache};
use crate::{ChainHeadRow, Error, LockPlan, Refusal, Result, Session, WriteTxn, refuse};

/// Log entry record types (§15).
pub(crate) const ENTRY_OPERATION: u8 = 1;
pub(crate) const ENTRY_EPOCH: u8 = 8;
pub(crate) const ENTRY_POLICY: u8 = 9;
/// Key stream record types (§26).
pub(crate) const KEY_EPOCH: u8 = 3;
pub(crate) const KEY_ENVELOPE: u8 = 4;

/// A refusal of one record of a push: the record's own code, with the
/// `detail` of §26 that names its index, its code and its own detail.
pub(crate) fn failure(index: usize, refusal: Refusal) -> Error {
    let detail = PushFailureDetail {
        record_index: u32::try_from(index).unwrap_or(u32::MAX),
        code: Text(refusal.code.to_owned()),
        record_detail: refusal.detail.map(Bytes),
    };
    Error::Refused(Refusal {
        code: refusal.code,
        message: refusal.message,
        detail: Some(detail.to_bytes()),
    })
}

fn decode_refusal(error: DecodeError) -> Refusal {
    Refusal::new(
        error.kind.server_code(),
        format!("the body doesn't decode: {error}"),
    )
}

/// Decodes a `list<record_ref>` body, naming the index of a record that
/// fails to decode.
pub(crate) fn decode_records(body: &[u8]) -> Result<Vec<RecordRef>> {
    let mut reader = Reader::new(body);
    let count = reader
        .count(UNBOUNDED, RecordRef::MIN_SIZE)
        .map_err(|e| Error::Refused(decode_refusal(e)))?;
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let record =
            RecordRef::decode(&mut reader).map_err(|e| failure(index, decode_refusal(e)))?;
        records.push(record);
    }
    reader
        .finish()
        .map_err(|e| Error::Refused(decode_refusal(e)))?;
    Ok(records)
}

/// A pushed operation, decoded.
pub(crate) struct PushedOp {
    index: usize,
    op: Signed<Operation>,
    digest: Digest,
    bytes: Vec<u8>,
}

/// Decodes a push and runs the checks that need no state: each record is an
/// operation of this workspace, signed by the session's device, with a
/// ciphertext of a padded length. The decoder already refused a ciphertext
/// above 270,352 bytes with `too_large`.
pub(crate) fn decode(body: &[u8], workspace: Id, session: Session) -> Result<Vec<PushedOp>> {
    let records = decode_records(body)?;
    let mut ops = Vec::with_capacity(records.len());
    for (index, record) in records.into_iter().enumerate() {
        let RecordRef::Operation(op) = record else {
            return Err(failure(
                index,
                Refusal::new(
                    "bad_request",
                    "this server takes only operations in a push so far",
                ),
            ));
        };
        if op.body.workspace != workspace {
            return Err(failure(
                index,
                Refusal::new("bad_request", "the operation names another workspace"),
            ));
        }
        if op.body.device != session.device {
            return Err(failure(
                index,
                Refusal::new(
                    "forbidden",
                    "the session's device signs every operation it pushes",
                ),
            ));
        }
        if !is_padded_ciphertext_length(op.body.ciphertext.len() as u64) {
            return Err(failure(
                index,
                Refusal::new("padding_invalid", "the ciphertext isn't a padded length"),
            ));
        }
        let bytes = op.to_bytes();
        let digest = sync_protocol::crypto::hash(&bytes);
        ops.push(PushedOp {
            index,
            op,
            digest,
            bytes,
        });
    }
    Ok(ops)
}

/// Checks each operation's signature under the session device's card from
/// its account chain.
pub(crate) fn check_signatures(
    ops: &[PushedOp],
    session: Session,
    chain: &AccountChain,
) -> Result<()> {
    let card = chain
        .card(&session.device)
        .ok_or_else(|| refuse("forbidden", "the session's card is in no account chain"))?;
    for op in ops {
        if op.op.verify(&card.card.signing_public).is_err() {
            return Err(failure(
                op.index,
                Refusal::new(
                    "signature_invalid",
                    "the signature fails under the signer's card",
                ),
            ));
        }
    }
    Ok(())
}

/// Validates the operations in order against the current state plus the
/// ones before them, and writes each. The caller's transaction commits all
/// of them or none.
pub(crate) async fn commit(
    txn: &mut impl WriteTxn,
    cache: &PolicyCache,
    generation: u64,
    session: Session,
    workspace: Id,
    ops: Vec<PushedOp>,
) -> Result<PushResponse> {
    let mut domains: Vec<Id> = ops.iter().map(|o| o.op.body.domain).collect();
    domains.sort();
    domains.dedup();
    let mut blobs: Vec<Id> = ops
        .iter()
        .flat_map(|o| o.op.body.blobs.iter().copied())
        .collect();
    blobs.sort();
    blobs.dedup();
    txn.lock(&LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: false,
        domains,
        blobs,
    })
    .await?;
    let Some((row, policy)) = policy::load(txn, cache, workspace).await? else {
        return Err(refuse("not_found", "no such workspace"));
    };
    if !policy.head_snapshot().is_device(&session.device) {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    let mut receipts = Vec::with_capacity(ops.len());
    for op in &ops {
        let receipt = apply(txn, &policy, generation, workspace, op)
            .await
            .map_err(|e| match e {
                Error::Refused(refusal) => failure(op.index, refusal),
                internal => internal,
            })?;
        receipts.push(receipt);
    }
    Ok(PushResponse {
        policy_revision: row.policy_revision,
        receipts: List(receipts),
    })
}

fn chain_conflict(domain: Id, head: Option<ChainHeadRow>) -> Error {
    let (counter, record_digest) = head.map_or((0, Digest::ZERO), |h| (h.counter, h.digest));
    Error::Refused(Refusal {
        code: "chain_conflict",
        message: "the counter or previous digest doesn't follow the server's head".into(),
        detail: Some(
            ChainConflictDetail {
                domain,
                counter,
                record_digest,
            }
            .to_bytes(),
        ),
    })
}

/// The checks of §13 for one operation, in the order the spec lists them,
/// then its write.
async fn apply(
    txn: &mut impl WriteTxn,
    policy: &Arc<PolicyChain>,
    generation: u64,
    workspace: Id,
    pushed: &PushedOp,
) -> Result<Receipt> {
    let op = &pushed.op.body;
    let snapshot = policy.head_snapshot();
    let (head_revision, _) = policy.head();
    let domain = op.domain;
    if !snapshot.is_device(&op.device) {
        return Err(refuse(
            "forbidden",
            "the signer isn't active in this workspace",
        ));
    }
    if !snapshot.domains.contains_key(&domain) {
        return Err(refuse("not_found", "no such domain"));
    }
    let Some(domain_row) = txn.domain(workspace, domain).await? else {
        return Err(refuse("not_found", "no such domain"));
    };

    // A retried upload gets its original receipt; another record at the
    // same position conflicts.
    let head = txn.chain_head(workspace, domain, op.device).await?;
    if let Some((seq, digest)) = txn
        .position(workspace, domain, op.device, op.counter)
        .await?
    {
        if digest == pushed.digest {
            return Ok(Receipt {
                record_digest: digest,
                domain,
                seq,
            });
        }
        return Err(chain_conflict(domain, head));
    }

    // Write access at the latest policy entry and at `policy_revision`.
    if !snapshot.can_write(&op.device, &domain) {
        return Err(refuse("forbidden", "the device can't write this domain"));
    }
    let at_revision = (op.policy_revision <= head_revision)
        .then(|| policy.at(op.policy_revision))
        .flatten();
    if !at_revision.is_some_and(|s| s.can_write(&op.device, &domain)) {
        return Err(refuse(
            "policy_stale",
            "the device couldn't write this domain at the operation's policy revision",
        ));
    }
    if head.is_some_and(|h| op.policy_revision < h.policy_revision) {
        return Err(refuse(
            "policy_regressed",
            "the policy revision is below the previous record's",
        ));
    }
    if at_revision.is_some_and(|s| s.digest != op.policy_head) {
        return Err(refuse(
            "policy_conflict",
            "policy_head isn't the server's change at that revision",
        ));
    }

    // The epoch of the latest epoch entry, with no rotation pending.
    if domain_row.epoch == 0 || op.epoch != domain_row.epoch {
        return Err(refuse(
            "epoch_stale",
            "the epoch isn't the latest epoch entry's",
        ));
    }
    if policy
        .rotation_required(&domain, domain_row.epoch_policy_revision, head_revision)
        .is_some()
    {
        return Err(refuse(
            "rotation_pending",
            "the domain waits for a rotation",
        ));
    }

    // The restore generation and the checkpoint the writer reached.
    if op.generation > generation || head.is_some_and(|h| op.generation < h.generation) {
        return Err(refuse(
            "policy_regressed",
            "the generation is above the server's or below the previous record's",
        ));
    }
    let reached_none = op.checkpoint_seq == 0 && op.checkpoint.is_zero();
    if op.generation == generation
        && op.checkpoint_seq >= domain_row.retention_floor
        && !reached_none
        && !txn
            .checkpoint_exists(workspace, domain, op.checkpoint_seq, op.checkpoint)
            .await?
    {
        return Err(refuse(
            "checkpoint_unknown",
            "the domain holds no such checkpoint at that covered_seq",
        ));
    }

    // The device's chain.
    let (head_counter, head_digest) = head.map_or((0, Digest::ZERO), |h| (h.counter, h.digest));
    if op.counter <= head_counter || op.previous != head_digest {
        return Err(chain_conflict(domain, head));
    }

    // The object.
    let rid_domain = txn.rid_domain(workspace, op.rid).await?;
    if rid_domain.is_some_and(|d| d != domain) {
        return Err(refuse(
            "rid_domain_mismatch",
            "the rid belongs to another domain",
        ));
    }
    let live_op = LiveOp::from_operation(&pushed.op, 0);
    let mut unknown_cover = false;
    for cover in op.supersedes.iter() {
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
    let class = judge(
        &live_op,
        rid_domain.is_some(),
        unknown_cover,
        covered,
        &live,
    )
    .map_err(|e| refuse(e.server_code(), e.to_string()))?;
    if !op.blobs.is_empty() {
        return Err(refuse(
            "blob_incomplete",
            "the domain holds no completed upload with that ID",
        ));
    }

    // The write.
    let seq = txn
        .append_entry(workspace, domain, ENTRY_OPERATION, &pushed.bytes)
        .await?;
    if rid_domain.is_none() {
        txn.insert_rid(workspace, op.rid, domain).await?;
    }
    let live_op = LiveOp { seq, ..live_op };
    txn.insert_operation(workspace, domain, &live_op, class)
        .await?;
    txn.set_chain_head(
        workspace,
        domain,
        op.device,
        &ChainHeadRow {
            counter: op.counter,
            digest: pushed.digest,
            seq,
            policy_revision: op.policy_revision,
            generation: op.generation,
        },
    )
    .await?;
    Ok(Receipt {
        record_digest: pushed.digest,
        domain,
        seq,
    })
}
