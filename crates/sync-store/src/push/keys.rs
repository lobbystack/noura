//! The key records of a push (§8, §9): `account_heads` changes, epoch
//! bundles and envelopes, checked against the current state plus the
//! records before them in the same push.
//!
//! A change whose heads bring a card into a domain owes, later in the same
//! push, a current-epoch envelope for that card, when its signer could read
//! the domain before the change; in a domain the signer couldn't read, only
//! a change that also takes a card out commits without them, and the
//! rotation it makes due wraps for the new card. A rotation owes an
//! envelope for each of its recipients. Apart from those, the server takes
//! a lone envelope only for an open re-wrap request, from at most a few
//! wrapping devices, and none while the domain waits for a rotation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sync_protocol::codec::Encode;
use sync_protocol::keys::{check_replacement, recipients_digest};
use sync_protocol::messages::{PolicyEntry, Receipt};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Envelope, EpochBundle, PolicyChange, PolicyKind, Profile, Signed};
use sync_protocol::types::Id;

use super::{ENTRY_EPOCH, KEY_ENVELOPE, KEY_EPOCH};
use crate::policy;
use crate::{CheckpointInterval, EpochRow, Error, Refusal, Result, Session, WriteTxn, refuse};

/// The key stream type of a re-wrap request (§26).
pub(crate) const KEY_REWRAP: u8 = 5;

/// The most lone envelopes one re-wrap request takes: the first few
/// holders that answer within their wait (§9).
pub(crate) const MAX_REWRAP_ANSWERS: usize = 3;

/// What a push's records have done so far: the policy after its changes,
/// and the envelopes it still owes, by domain, epoch and recipient, with
/// the index of the record that needs each.
pub(crate) struct PushState {
    policy: Arc<PolicyChain>,
    owed: BTreeMap<(Id, u64, Id), usize>,
    interval: CheckpointInterval,
}

impl PushState {
    pub(crate) fn new(policy: Arc<PolicyChain>, interval: CheckpointInterval) -> Self {
        Self {
            policy,
            owed: BTreeMap::new(),
            interval,
        }
    }

    /// The policy after the changes so far.
    pub(crate) fn policy(&self) -> &Arc<PolicyChain> {
        &self.policy
    }

    /// The first record whose envelopes the push doesn't carry.
    pub(crate) fn unmet(&self) -> Option<(usize, Refusal)> {
        self.owed.values().min().map(|index| {
            (
                *index,
                Refusal::new(
                    "recipients_invalid",
                    "the push lacks an envelope that a new reader of the domain needs",
                ),
            )
        })
    }
}

/// Commits an `account_heads` change (§8): the rules of §8 against the
/// account chains the store holds, read after the policy lock, then the
/// change and its entry in every domain's log, in domain-ID order.
pub(crate) async fn apply_change(
    txn: &mut impl WriteTxn,
    state: &mut PushState,
    session: Session,
    workspace: Id,
    index: usize,
    change: &Signed<PolicyChange>,
) -> Result<Vec<Receipt>> {
    if !matches!(change.body.kind, PolicyKind::AccountHeads) {
        // Ticket 50 adds the membership and domain changes.
        return Err(refuse(
            "bad_request",
            "this server takes account_heads changes in a push so far",
        ));
    }
    let current = state.policy.clone();
    let prev = current.head_snapshot();
    let accounts: BTreeSet<Id> = prev
        .members
        .keys()
        .copied()
        .chain(prev.former.keys().copied())
        .chain(change.body.heads.iter().map(|h| h.account))
        .collect();
    let chains = policy::account_chains(txn, accounts).await?;
    let next = current
        .check(change, &chains)
        .map_err(|e| refuse(e.server_code(), e.to_string()))?;

    // The envelopes it owes the cards it brings in.
    let read_before = |domain: &Id| {
        prev.can_read(&session.device, domain)
            || session.recovery.is_some_and(|r| prev.can_read(&r, domain))
    };
    for (domain, added) in &next.new_readers {
        if !prev.domains.contains_key(domain) {
            continue;
        }
        if read_before(domain) {
            let row = txn
                .domain(workspace, *domain)
                .await?
                .ok_or_else(|| Error::internal("a domain of the policy has no head row"))?;
            for device in added {
                state.owed.insert((*domain, row.epoch, *device), index);
            }
        } else if !next.rotations.contains(domain) {
            return Err(refuse(
                "forbidden",
                "a device that can't read the domain records a head that brings cards in without making it due for rotation",
            ));
        }
    }

    let revision = change.body.revision;
    let digest = change.digest();
    policy::store_change(txn, workspace, change).await?;
    txn.set_policy_head(workspace, revision, digest).await?;
    for (account, member) in &next.members {
        txn.upsert_member(workspace, *account, member.role.code())
            .await?;
    }
    let entry = PolicyEntry {
        revision,
        head: digest,
    };
    let mut receipts = Vec::with_capacity(next.domains.len());
    for domain in next.domains.keys() {
        let seq = crate::checkpoints::append_policy_entry(
            txn,
            state.interval,
            workspace,
            *domain,
            &entry,
            Some(prev),
            &next,
        )
        .await?;
        receipts.push(Receipt {
            record_digest: digest,
            domain: *domain,
            seq,
        });
    }
    let mut updated = (*current).clone();
    updated
        .append(change, &chains)
        .map_err(|e| refuse(e.server_code(), e.to_string()))?;
    state.policy = Arc::new(updated);
    Ok(receipts)
}

/// Commits an epoch bundle (§9): a rotation that a committed change
/// requires, with a `policy_revision` at or above the latest change that
/// requires it, or a replacement that skips at least one epoch, each used
/// by no device but its rotator and devices that lost read access through a
/// change at or below the replacement's `policy_revision`. The rotating
/// device must read the domain at that revision and now, and the
/// recipients must be the domain's readers. The bundle owes an envelope for
/// every recipient later in the push, and the server deletes the older
/// epochs' envelopes and closes their re-wrap requests.
pub(crate) async fn apply_epoch(
    txn: &mut impl WriteTxn,
    state: &mut PushState,
    _session: Session,
    workspace: Id,
    index: usize,
    bundle: &EpochBundle,
) -> Result<Vec<Receipt>> {
    let record = &bundle.epoch.0;
    let body = &record.body;
    let policy = state.policy.clone();
    let snapshot = policy.head_snapshot();
    let (head_revision, _) = policy.head();
    let domain = body.domain;
    if !snapshot.domains.contains_key(&domain) {
        return Err(refuse("not_found", "no such domain"));
    }
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "no such domain"))?;
    let at = (body.policy_revision <= head_revision)
        .then(|| policy.at(body.policy_revision))
        .flatten()
        .ok_or_else(|| refuse("policy_stale", "the epoch names a revision above the head"))?;
    if at.digest != body.policy_head {
        return Err(refuse(
            "policy_conflict",
            "policy_head isn't the server's change at that revision",
        ));
    }
    let rotator = body.rotating_device;
    if !at.can_read(&rotator, &domain) || !snapshot.can_read(&rotator, &domain) {
        return Err(refuse(
            "forbidden",
            "the rotating device couldn't read the domain at its revision or can't now",
        ));
    }
    if body.epoch <= row.epoch {
        return Err(refuse("epoch_conflict", "another rotation took this epoch"));
    }
    if body.epoch != row.epoch + 1 {
        return Err(refuse(
            "epoch_conflict",
            "an epoch record comes one above the domain's latest",
        ));
    }
    let replacement = sync_protocol::keys::makes_checkpoint_due(&row.epoch_digest, body);
    if !replacement {
        let due = policy.rotation_required(&domain, row.epoch_policy_revision, head_revision);
        if due.is_none_or(|r| body.policy_revision < r) {
            return Err(refuse(
                "rotation_not_due",
                "no committed change requires this domain to rotate at or below the epoch's revision",
            ));
        }
    } else {
        let named = txn
            .epoch_by_digest(workspace, domain, body.previous)
            .await?
            .filter(|named| named.epoch < row.epoch)
            .ok_or_else(|| {
                refuse(
                    "epoch_conflict",
                    "previous names no epoch record of this domain below the latest",
                )
            })?;
        if let Some(required) = policy.rotation_required(&domain, 0, head_revision)
            && body.policy_revision < required
        {
            return Err(refuse(
                "rotation_not_due",
                "a replacement names a revision below the latest change that requires a rotation",
            ));
        }
        let users = skipped_users(txn, workspace, domain, named.epoch, row.epoch).await?;
        check_replacement(&policy, body, named.epoch, row.epoch, &users).map_err(|_| {
            refuse(
                "epoch_conflict",
                "a device other than a skipped epoch's rotator used it and kept read access",
            )
        })?;
    }
    let readers = snapshot.readers(&domain);
    let listed: BTreeSet<Id> = bundle.recipients.iter().copied().collect();
    if listed != readers
        || at.readers(&domain) != readers
        || body.recipients_digest != recipients_digest(&readers)
    {
        return Err(refuse(
            "recipients_invalid",
            "the epoch's recipients aren't the domain's readers",
        ));
    }

    let bytes = record.to_bytes();
    let digest = record.digest();
    let seq = txn
        .append_entry(workspace, domain, ENTRY_EPOCH, &bytes)
        .await?;
    // A replacement makes a checkpoint due at its entry, since the latest
    // checkpoint may name the epoch it skips (§17).
    crate::checkpoints::note_entry(
        txn,
        state.interval,
        workspace,
        domain,
        seq,
        bytes.len() as u64,
        replacement,
    )
    .await?;
    txn.insert_epoch(
        workspace,
        domain,
        &EpochRow {
            epoch: body.epoch,
            seq,
            digest,
            previous: body.previous,
            record: bytes.clone(),
        },
        body.policy_revision,
        &bundle.to_bytes(),
    )
    .await?;
    txn.append_key_entry(workspace, KEY_EPOCH, domain, body.epoch, None, &bytes)
        .await?;
    txn.drop_old_epochs(workspace, domain, body.epoch).await?;
    state
        .owed
        .retain(|(owed_domain, _, _), _| *owed_domain != domain);
    for reader in readers {
        state.owed.insert((domain, body.epoch, reader), index);
    }
    Ok(vec![Receipt {
        record_digest: digest,
        domain,
        seq,
    }])
}

/// For each epoch in (`named`, `latest`], its rotator and the devices that
/// signed an operation or a profile under it: what a replacement that
/// skips those epochs must judge (§9).
async fn skipped_users(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    named: u64,
    latest: u64,
) -> Result<BTreeMap<u64, (Id, BTreeSet<Id>)>> {
    use sync_protocol::codec::Decode;
    let mut profiles: Vec<Profile> = Vec::new();
    for bytes in txn.profiles(workspace).await? {
        profiles.push(
            Signed::<Profile>::from_bytes(&bytes)
                .map_err(Error::internal)?
                .body,
        );
    }
    let mut users = BTreeMap::new();
    for epoch in (named + 1)..=latest {
        let Some(row) = txn.epoch(workspace, domain, epoch).await? else {
            continue;
        };
        let record = sync_protocol::records::Signed::<sync_protocol::records::Epoch>::from_bytes(
            &row.record,
        )
        .map_err(Error::internal)?;
        let next = match txn.epoch(workspace, domain, epoch + 1).await? {
            Some(next) => next.seq,
            None => u64::MAX,
        };
        let mut signers: BTreeSet<Id> = txn
            .writers_between(workspace, domain, row.seq, next)
            .await?
            .into_iter()
            .collect();
        // Profiles live in the workspace domain.
        signers.extend(
            profiles
                .iter()
                .filter(|p| p.epoch == epoch)
                .map(|p| p.signer),
        );
        users.insert(epoch, (record.body.rotating_device, signers));
    }
    Ok(users)
}

/// Takes an envelope (§9): from a device that reads the domain under the
/// current policy, for a reader of the domain's current epoch whose card's
/// X-Wing key it names. One that the push owes settles that debt; any
/// other is a lone envelope, which the server takes only for an open
/// re-wrap request, once per wrapping device and from at most a few, and
/// none while the domain waits for a rotation.
pub(crate) async fn apply_envelope(
    txn: &mut impl WriteTxn,
    state: &mut PushState,
    _session: Session,
    workspace: Id,
    envelope: &Signed<Envelope>,
) -> Result<()> {
    let body = &envelope.body;
    let policy = state.policy.clone();
    let snapshot = policy.head_snapshot();
    let (head_revision, _) = policy.head();
    let domain = body.domain;
    if !snapshot.domains.contains_key(&domain) {
        return Err(refuse("not_found", "no such domain"));
    }
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "no such domain"))?;
    if !snapshot.can_read(&body.wrapping_device, &domain) {
        return Err(refuse(
            "forbidden",
            "the wrapping device can't read the domain",
        ));
    }
    if body.epoch != row.epoch {
        return Err(refuse(
            "epoch_stale",
            "an envelope wraps the domain's current epoch",
        ));
    }
    let fits = snapshot
        .devices
        .get(&body.recipient)
        .filter(|_| snapshot.can_read(&body.recipient, &domain))
        .is_some_and(|device| device.card.hpke_digest() == body.recipient_hpke_digest);
    if !fits {
        return Err(refuse(
            "recipients_invalid",
            "the envelope's recipient isn't a reader of the domain with that key",
        ));
    }
    let bytes = envelope.to_bytes();
    if state
        .owed
        .remove(&(domain, body.epoch, body.recipient))
        .is_some()
    {
        txn.append_key_entry(
            workspace,
            KEY_ENVELOPE,
            domain,
            body.epoch,
            Some(body.recipient),
            &bytes,
        )
        .await?;
        return Ok(());
    }
    if policy
        .rotation_required(&domain, row.epoch_policy_revision, head_revision)
        .is_some()
    {
        return Err(refuse(
            "rotation_pending",
            "the domain waits for a rotation, so it takes no lone envelope",
        ));
    }
    let request = txn
        .rewrap_requests(workspace)
        .await?
        .into_iter()
        .find(|r| r.domain == domain && r.epoch == body.epoch && r.device == body.recipient)
        .ok_or_else(|| {
            refuse(
                "forbidden",
                "a lone envelope answers only an open re-wrap request",
            )
        })?;
    let answers = txn
        .rewrap_answers(workspace, domain, body.epoch, body.recipient)
        .await?;
    if answers
        .iter()
        .any(|(wrapper, _)| *wrapper == body.wrapping_device)
    {
        return Err(refuse(
            "forbidden",
            "this device already answered the re-wrap request",
        ));
    }
    if answers.len() >= MAX_REWRAP_ANSWERS {
        return Err(refuse(
            "forbidden",
            "the re-wrap request already took its answers",
        ));
    }
    let key_seq = txn
        .append_key_entry(
            workspace,
            KEY_ENVELOPE,
            domain,
            body.epoch,
            Some(body.recipient),
            &bytes,
        )
        .await?;
    txn.insert_rewrap_answer(workspace, &request, body.wrapping_device, key_seq)
        .await
}
