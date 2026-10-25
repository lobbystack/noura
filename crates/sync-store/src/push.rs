//! Push (§15): decoding and the checks that need no transaction, then the
//! checks of §13 against the current state plus the records before each
//! one, and the commit, all or none.

mod keys;

use std::sync::Arc;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Bytes, Decode, DecodeError, Encode, List, Reader, Text, UNBOUNDED};
use sync_protocol::live::{LiveOp, judge};
use sync_protocol::messages::{
    ChainConflictDetail, PushFailureDetail, PushResponse, Receipt, RecordRef,
};
use sync_protocol::padding::is_padded_ciphertext_length;
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Envelope, EpochBundle, Operation, PolicyChange, Signed};
use sync_protocol::types::{Digest, Id};

pub(crate) use keys::KEY_REWRAP;

use crate::policy::{self, PolicyCache};
use crate::{
    ChainHeadRow, Error, LockPlan, Refusal, Result, Session, WriteGroup, WriteTxn, refuse,
};

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
        retry_after_seconds: refusal.retry_after_seconds,
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

/// A pushed record, decoded, with its index in the push.
pub(crate) enum Pushed {
    /// An operation.
    Op(PushedOp),
    /// A policy change; this server takes `account_heads` so far, and
    /// ticket 50 adds the other kinds.
    Change(usize, Signed<PolicyChange>),
    /// An epoch bundle: a rotation or a replacement (§9).
    Epoch(usize, EpochBundle),
    /// An envelope: one the change or the rotation before it needs, or a
    /// lone envelope that answers a re-wrap request.
    Envelope(usize, Signed<Envelope>),
}

impl Pushed {
    fn index(&self) -> usize {
        match self {
            Pushed::Op(op) => op.index,
            Pushed::Change(index, _) | Pushed::Epoch(index, _) | Pushed::Envelope(index, _) => {
                *index
            }
        }
    }
}

/// Decodes a push and runs the checks that need no state: each record
/// belongs to this workspace and is signed by the session's device, listed
/// once, and an operation's ciphertext has a padded length. The decoder
/// already refused a ciphertext above 270,352 bytes with `too_large`.
pub(crate) fn decode(body: &[u8], workspace: Id, session: Session) -> Result<Vec<Pushed>> {
    let records = decode_records(body)?;
    let mut pushed = Vec::with_capacity(records.len());
    let mut digests = std::collections::BTreeSet::new();
    for (index, record) in records.into_iter().enumerate() {
        let fail = |code: &'static str, message: &str| failure(index, Refusal::new(code, message));
        // A second copy would take the retry path and get a second receipt
        // for one log entry.
        if !digests.insert(record.receipt_digest()) {
            return Err(fail("bad_request", "the push lists this record twice"));
        }
        let (record_workspace, signer) = match &record {
            RecordRef::Operation(op) => (op.body.workspace, op.body.device),
            RecordRef::Policy(change) => (change.body.workspace, change.body.signer),
            RecordRef::EpochBundle(bundle) => (
                bundle.epoch.0.body.workspace,
                bundle.epoch.0.body.rotating_device,
            ),
            RecordRef::Envelope(envelope) => {
                (envelope.body.workspace, envelope.body.wrapping_device)
            }
            RecordRef::CheckpointBundle(_) | RecordRef::Profile(_) => {
                // Checkpoints arrive with ticket 58 and profile pushes with
                // ticket 48.
                return Err(fail(
                    "bad_request",
                    "this server takes no checkpoint or profile in a push so far",
                ));
            }
        };
        if record_workspace != workspace {
            return Err(fail("bad_request", "the record names another workspace"));
        }
        if signer != session.device {
            return Err(fail(
                "forbidden",
                "the session's device signs every record it pushes",
            ));
        }
        pushed.push(match record {
            RecordRef::Operation(op) => {
                if !is_padded_ciphertext_length(op.body.ciphertext.len() as u64) {
                    return Err(fail(
                        "padding_invalid",
                        "the ciphertext isn't a padded length",
                    ));
                }
                let bytes = op.to_bytes();
                let digest = sync_protocol::crypto::hash(&bytes);
                Pushed::Op(PushedOp {
                    index,
                    op,
                    digest,
                    bytes,
                })
            }
            RecordRef::Policy(change) => Pushed::Change(index, change),
            RecordRef::EpochBundle(bundle) => Pushed::Epoch(index, bundle),
            RecordRef::Envelope(envelope) => Pushed::Envelope(index, envelope),
            RecordRef::CheckpointBundle(_) | RecordRef::Profile(_) => unreachable!("refused"),
        });
    }
    Ok(pushed)
}

/// The group a push may commit with (§15): pushes that take only the
/// shared policy lock and one domain's head row, so a group's transaction
/// takes its locks in the order every push does. Those are pushes of
/// operations that all write one domain and list no blob. A push that
/// locks blob rows, the key-sequence row for an epoch bundle or envelope,
/// or the policy lock exclusively for a policy change runs alone: in a
/// group it would take that lock after the head row a push before it holds.
pub(crate) fn group(workspace: Id, pushed: &[Pushed]) -> Option<WriteGroup> {
    let Some(Pushed::Op(first)) = pushed.first() else {
        return None;
    };
    let domain = first.op.body.domain;
    let groups = pushed.iter().all(|p| match p {
        Pushed::Op(op) => op.op.body.domain == domain && op.op.body.blobs.is_empty(),
        _ => false,
    });
    groups.then_some(WriteGroup { workspace, domain })
}

/// Checks the signature of each operation, epoch record and envelope
/// under the session device's card from its account chain. A policy
/// change's signature is checked with the rest of §8 in the transaction.
pub(crate) fn check_signatures(
    pushed: &[Pushed],
    session: Session,
    chain: &AccountChain,
) -> Result<()> {
    let card = chain
        .card(&session.device)
        .ok_or_else(|| refuse("forbidden", "the session's card is in no account chain"))?;
    let key = &card.card.signing_public;
    for record in pushed {
        let valid = match record {
            Pushed::Op(op) => op.op.verify(key).is_ok(),
            Pushed::Change(..) => true,
            Pushed::Epoch(_, bundle) => bundle.epoch.0.verify(key).is_ok(),
            Pushed::Envelope(_, envelope) => envelope.verify(key).is_ok(),
        };
        if !valid {
            return Err(failure(
                record.index(),
                Refusal::new(
                    "signature_invalid",
                    "the signature fails under the signer's card",
                ),
            ));
        }
    }
    Ok(())
}

/// Validates the records in order against the current state plus the ones
/// before them, and writes each. The caller's transaction commits all of
/// them or none.
pub(crate) async fn commit(
    txn: &mut impl WriteTxn,
    cache: &PolicyCache,
    generation: u64,
    session: Session,
    workspace: Id,
    pushed: Vec<Pushed>,
) -> Result<PushResponse> {
    let changes = pushed.iter().any(|p| matches!(p, Pushed::Change(..)));
    let keys = pushed
        .iter()
        .any(|p| matches!(p, Pushed::Epoch(..) | Pushed::Envelope(..)));
    let mut domains: Vec<Id> = pushed
        .iter()
        .filter_map(|p| match p {
            Pushed::Op(op) => Some(op.op.body.domain),
            Pushed::Change(..) => None,
            Pushed::Epoch(_, bundle) => Some(bundle.epoch.0.body.domain),
            Pushed::Envelope(_, envelope) => Some(envelope.body.domain),
        })
        .collect();
    let mut blobs: Vec<Id> = pushed
        .iter()
        .filter_map(|p| match p {
            Pushed::Op(op) => Some(op.op.body.blobs.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect();
    blobs.sort();
    blobs.dedup();
    if changes {
        // A policy change writes an entry into every domain's log, so it
        // takes the policy lock exclusively first; while it holds it, no
        // other change can add or delete a domain, and it then locks every
        // domain's head row in ID order (§15).
        txn.lock(&LockPlan {
            workspace: Some(workspace),
            policy_exclusive: true,
            key_sequence: keys,
            domains: Vec::new(),
            blobs: Vec::new(),
        })
        .await?;
        if let Some((_, policy)) = policy::load(txn, cache, workspace).await? {
            domains.extend(policy.head_snapshot().domains.keys().copied());
        }
    }
    domains.sort();
    domains.dedup();
    txn.lock(&LockPlan {
        workspace: Some(workspace),
        policy_exclusive: changes,
        key_sequence: keys,
        domains,
        blobs,
    })
    .await?;
    let Some((row, policy)) = policy::load(txn, cache, workspace).await? else {
        return Err(refuse("not_found", "no such workspace"));
    };
    if crate::reads::reader(policy.head_snapshot(), session).is_none() {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    let mut state = keys::PushState::new(policy);
    let mut receipts = Vec::with_capacity(pushed.len());
    for record in &pushed {
        let index = record.index();
        let result = match record {
            Pushed::Op(op) => apply(txn, state.policy(), generation, workspace, op)
                .await
                .map(|receipt| vec![receipt]),
            Pushed::Change(_, change) => {
                keys::apply_change(txn, &mut state, session, workspace, index, change).await
            }
            Pushed::Epoch(_, bundle) => {
                keys::apply_epoch(txn, &mut state, session, workspace, index, bundle).await
            }
            Pushed::Envelope(_, envelope) => {
                keys::apply_envelope(txn, &mut state, session, workspace, envelope)
                    .await
                    .map(|()| Vec::new())
            }
        };
        let created = result.map_err(|e| match e {
            Error::Refused(refusal) => failure(index, refusal),
            internal => internal,
        })?;
        receipts.extend(created);
    }
    if let Some((index, refusal)) = state.unmet() {
        return Err(failure(index, refusal));
    }
    let policy_revision = state.policy().head().0.max(row.policy_revision);
    if changes {
        cache.put(state.policy().clone());
    }
    Ok(PushResponse {
        policy_revision,
        receipts: List(receipts),
    })
}

fn chain_conflict(domain: Id, head: Option<ChainHeadRow>) -> Error {
    let (counter, record_digest) = head.map_or((0, Digest::ZERO), |h| (h.counter, h.digest));
    Error::Refused(Refusal {
        code: "chain_conflict",
        message: "the counter or previous digest doesn't follow the server's head".into(),
        retry_after_seconds: None,
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

#[cfg(test)]
mod tests {
    use sync_protocol::codec::{Bytes, Encode, Sorted};
    use sync_protocol::crypto::SigningKey;
    use sync_protocol::records::{Operation, Signed};
    use sync_protocol::types::{Digest, Id, Nonce};

    use super::{Pushed, PushedOp, group};
    use crate::WriteGroup;

    fn pushed(index: usize, domain: Id, blobs: Vec<Id>) -> Pushed {
        let key = SigningKey::random().expect("random");
        let op = Signed::sign(
            Operation {
                workspace: Id([1; 16]),
                domain,
                device: Id([2; 16]),
                counter: index as u64 + 1,
                previous: Digest::ZERO,
                epoch: 1,
                policy_revision: 1,
                policy_head: Digest::ZERO,
                previous_attempt: Digest::ZERO,
                generation: 1,
                checkpoint_seq: 0,
                checkpoint: Digest::ZERO,
                rid: Id::random().expect("random"),
                delete: false,
                supersedes: Sorted::default(),
                blobs: Sorted::new(blobs).expect("sorted"),
                nonce: Nonce::ZERO,
                ciphertext: Bytes(vec![0; 272]),
            },
            &key,
        );
        let bytes = op.to_bytes();
        Pushed::Op(PushedOp {
            index,
            digest: op.digest(),
            op,
            bytes,
        })
    }

    #[test]
    fn only_pushes_that_lock_one_head_row_group() {
        let workspace = Id([1; 16]);
        let (domain, other) = (Id([3; 16]), Id([4; 16]));
        let one_domain = [pushed(0, domain, vec![]), pushed(1, domain, vec![])];
        assert_eq!(
            group(workspace, &one_domain),
            Some(WriteGroup { workspace, domain })
        );
        let two_domains = [pushed(0, domain, vec![]), pushed(1, other, vec![])];
        assert_eq!(group(workspace, &two_domains), None);
        // Its blob rows come after the head row in the lock order, so a
        // group would take them after a push before it held that row.
        let with_blob = [
            pushed(0, domain, vec![]),
            pushed(1, domain, vec![Id([5; 16])]),
        ];
        assert_eq!(group(workspace, &with_blob), None);
        assert_eq!(group(workspace, &[]), None);
    }
}
