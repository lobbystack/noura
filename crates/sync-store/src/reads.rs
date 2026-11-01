//! The read routes of §26, and the trusted account chain writes that tests
//! use to set up a store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{
    EntryRecord, EpochsResponse, HeadResponse, KeyEntry, KeyRecord, KeysResponse, MAX_BODY,
    PolicyResponse, RewrapRequest, WorkspaceInfo, WorkspacesResponse,
};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{
    AccountLink, Envelope, Epoch, PolicyChange, PolicyKind, Profile, Role, Signed,
};
use sync_protocol::types::{Digest, Id};

use crate::policy::{self, ChainCache, PolicyCache};
use crate::pull::entry;
use crate::push::{KEY_ENVELOPE, KEY_EPOCH, KEY_REWRAP};
use crate::{Error, LinkRow, ReadTxn, Result, RewrapRow, Session, WorkspaceRow, WriteTxn, refuse};

/// Key stream rows read per query.
const CHUNK: u32 = 256;

/// The session device's account chain: its account's current chain, in
/// which the device's card must be active at the head. A session whose card
/// a link removed answers `device_removed` before it gets here, and one
/// whose device hasn't enrolled or joined the chain yet gets `forbidden`.
pub(crate) async fn session_chain(
    txn: &mut impl ReadTxn,
    chains: &ChainCache,
    session: Session,
) -> Result<Arc<AccountChain>> {
    policy::cached_current_chain(txn, chains, session.account)
        .await?
        .filter(|chain| chain.is_active_at(&session.device, chain.head_seqno()))
        .ok_or_else(|| {
            refuse(
                "forbidden",
                "the session's card isn't active in its account chain",
            )
        })
}

/// Appends links to an account's current chain, or starts one with a
/// genesis, after checking the rules of §7 that the links alone show but
/// not who sends them: tests set up stores this way, and the chain route
/// runs `chains::append`. Returns the new head's seqno.
pub(crate) async fn append_account_links(
    txn: &mut impl WriteTxn,
    links: &[Signed<AccountLink>],
) -> Result<u64> {
    let first = links
        .first()
        .ok_or_else(|| refuse("bad_request", "no links"))?;
    let account = first.body.account;
    // The account's chain row alone, before the chain is read (§15).
    txn.lock_account(account).await?;
    let none = BTreeSet::new();
    let refusal = |e: sync_protocol::account::AccountError| refuse(e.server_code(), e.to_string());
    let (mut chain, rest, new_chain) = match policy::current_chain(txn, account).await? {
        Some(chain) => (chain, links, false),
        None => (
            AccountChain::from_genesis(first, &none).map_err(refusal)?,
            &links[1..],
            true,
        ),
    };
    let start = if new_chain { 1 } else { chain.head_seqno() + 1 };
    for link in rest {
        chain.append(link, &none).map_err(refusal)?;
    }
    for link in links {
        let seqno = link.body.seqno;
        if seqno < start {
            continue;
        }
        txn.insert_account_link(
            account,
            &LinkRow {
                genesis: chain.genesis(),
                seqno,
                link: link.to_bytes(),
            },
            link.digest(),
        )
        .await?;
    }
    if new_chain {
        txn.set_current_genesis(account, chain.genesis()).await?;
    }
    crate::chains::end_removed_sessions(txn, &chain, start).await?;
    Ok(chain.head_seqno())
}

/// A workspace's row and policy, for a session whose device is active in
/// it: `not_found` for a workspace the store doesn't hold, `forbidden` for
/// any other device.
async fn authorize(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
) -> Result<(WorkspaceRow, Arc<PolicyChain>)> {
    let (row, policy) = policy::load(txn, cache, workspace)
        .await?
        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
    if reader(policy.head_snapshot(), session).is_none() {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    Ok((row, policy))
}

/// The device a session reads a workspace as: its own card when the policy
/// counts it among the workspace's devices, else the recovery identity it
/// proved, which reads as itself until the recovered card joins (§12).
pub(crate) fn reader(snapshot: &sync_protocol::policy::Snapshot, session: Session) -> Option<Id> {
    if snapshot.is_device(&session.device) {
        return Some(session.device);
    }
    session.recovery.filter(|r| snapshot.is_device(r))
}

/// Like [`authorize`], and the domain exists and the device reads it.
pub(crate) async fn authorize_domain(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
) -> Result<Arc<PolicyChain>> {
    let (_, policy) = authorize(txn, cache, session, workspace).await?;
    let snapshot = policy.head_snapshot();
    if !snapshot.domains.contains_key(&domain) {
        return Err(refuse("not_found", "no such domain"));
    }
    let device = reader(snapshot, session).unwrap_or(session.device);
    if !snapshot.can_read(&device, &domain) {
        return Err(refuse("forbidden", "the device can't read this domain"));
    }
    Ok(policy)
}

/// `GET /v2/workspaces`.
pub(crate) async fn workspaces(
    txn: &mut impl ReadTxn,
    session: Session,
    quota: Option<u64>,
) -> Result<WorkspacesResponse> {
    let mut workspaces = Vec::new();
    for (workspace, role) in txn.memberships(session.account).await? {
        let role = Role::from_code(role).ok_or_else(|| Error::internal("unknown role"))?;
        let profile = txn
            .latest_profile(workspace)
            .await?
            .map(|bytes| Signed::<Profile>::from_bytes(&bytes).map(Nested))
            .transpose()
            .map_err(Error::internal)?;
        let usage = txn.workspace_usage(workspace).await?;
        workspaces.push(WorkspaceInfo {
            workspace,
            role,
            used_bytes: usage.usage,
            history_bytes: usage.history,
            quota_bytes: quota,
            profile,
        });
    }
    Ok(WorkspacesResponse {
        workspaces: List(workspaces),
    })
}

/// `GET .../policy?after={revision}`: changes in revision order, at most
/// 1 MiB, and the current head of each member account's chain.
pub(crate) async fn policy(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    after: u64,
) -> Result<PolicyResponse> {
    let (row, policy, removed_at) = match authorize(txn, cache, session, workspace).await {
        Ok((row, policy)) => (row, policy, None),
        Err(refused) => {
            // A device whose account `remove_member` removed still fetches
            // the chain up to and including that change (§8).
            let Some((row, policy)) = policy::load(txn, cache, workspace).await? else {
                return Err(refused);
            };
            let Some(revision) = removed_member_at(&policy, session) else {
                return Err(refused);
            };
            (row, policy, Some(revision))
        }
    };
    let mut heads = Vec::new();
    if removed_at.is_none() {
        for account in policy.head_snapshot().members.keys() {
            if let Some(chain) = policy::current_chain(txn, *account).await? {
                heads.push(chain.head());
            }
        }
    }
    let heads = Sorted::new(heads).map_err(|e| Error::internal(e.name()))?;
    let mut budget = MAX_BODY.saturating_sub(8 + 4 + heads.to_bytes().len());
    let mut changes = Vec::new();
    for bytes in txn.policy_changes(workspace, after).await? {
        let change = Signed::<PolicyChange>::from_bytes(&bytes).map_err(Error::internal)?;
        if removed_at.is_some_and(|r| change.body.revision > r) {
            break;
        }
        let size = 4 + bytes.len();
        if size > budget {
            break;
        }
        budget -= size;
        changes.push(Nested(change));
    }
    Ok(PolicyResponse {
        head_revision: removed_at.unwrap_or(row.policy_revision),
        changes: List(changes),
        account_heads: heads,
    })
}

/// The revision of the `remove_member` that removed the session's account
/// while the policy before it counted the session's device, if any (§8).
fn removed_member_at(policy: &PolicyChain, session: Session) -> Option<u64> {
    (2..=policy.head().0).rev().find(|&revision| {
        policy.change(revision).is_some_and(|change| {
            matches!(change.body.kind, PolicyKind::RemoveMember { account } if account == session.account)
        }) && policy
            .at(revision - 1)
            .is_some_and(|before| reader(before, session).is_some())
    })
}

/// `GET .../keys?after={key_seq}&recipient={device}`: the epoch records of
/// the domains the device reads, without their recipients, and the
/// envelopes addressed to it, in key-sequence order, at most 1 MiB.
pub(crate) async fn keys(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    after: u64,
    recipient: Id,
) -> Result<KeysResponse> {
    let (_, policy) = authorize(txn, cache, session, workspace).await?;
    // A session reads its own device's envelopes, and a session that proved
    // a recovery identity reads the identity's too (§12).
    if recipient != session.device && Some(recipient) != session.recovery {
        return Err(refuse(
            "forbidden",
            "a session reads only its own device's envelopes",
        ));
    }
    let snapshot = policy.head_snapshot();
    let head_key_seq = txn.key_head(workspace).await?;
    let mut budget = MAX_BODY.saturating_sub(8 + 4);
    let mut entries = Vec::new();
    let mut cursor = after;
    'read: loop {
        let rows = txn.key_entries(workspace, cursor, CHUNK).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.key_seq;
            let reads = snapshot.can_read(&recipient, &row.domain);
            let record = match row.record_type {
                KEY_EPOCH if reads => KeyRecord::Epoch(
                    Signed::<Epoch>::from_bytes(&row.record).map_err(Error::internal)?,
                ),
                KEY_ENVELOPE if reads && row.recipient == Some(recipient) => KeyRecord::Envelope(
                    Signed::<Envelope>::from_bytes(&row.record).map_err(Error::internal)?,
                ),
                KEY_REWRAP if reads => KeyRecord::Rewrap(
                    RewrapRequest::from_bytes(&row.record).map_err(Error::internal)?,
                ),
                _ => continue,
            };
            let size = 8 + 1 + 4 + row.record.len();
            if size > budget {
                break 'read;
            }
            budget -= size;
            entries.push(KeyEntry {
                key_seq: row.key_seq,
                record,
            });
        }
    }
    Ok(KeysResponse {
        head_key_seq,
        entries: List(entries),
    })
}

/// `GET .../keys/current?recipient={device}`: the current epoch record of
/// each domain the device reads, its envelopes for those epochs and the
/// open re-wrap requests of those epochs, in key-sequence order, with the
/// key sequence to continue from, at most 1 MiB (§9, §18).
pub(crate) async fn current_keys(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    recipient: Id,
) -> Result<KeysResponse> {
    let (_, policy) = authorize(txn, cache, session, workspace).await?;
    if recipient != session.device && Some(recipient) != session.recovery {
        return Err(refuse(
            "forbidden",
            "a session reads only its own device's envelopes",
        ));
    }
    let snapshot = policy.head_snapshot();
    let mut current = BTreeMap::new();
    for domain in snapshot.domains.keys() {
        if snapshot.can_read(&recipient, domain)
            && let Some(row) = txn.domain(workspace, *domain).await?
        {
            current.insert(*domain, row.epoch);
        }
    }
    let head_key_seq = txn.key_head(workspace).await?;
    let mut budget = MAX_BODY.saturating_sub(8 + 4);
    let mut entries = Vec::new();
    let mut cursor = 0;
    'read: loop {
        let rows = txn.key_entries(workspace, cursor, CHUNK).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.key_seq;
            let Some(epoch) = current.get(&row.domain).copied() else {
                continue;
            };
            let record = match row.record_type {
                KEY_EPOCH => {
                    let record =
                        Signed::<Epoch>::from_bytes(&row.record).map_err(Error::internal)?;
                    if record.body.epoch != epoch {
                        continue;
                    }
                    KeyRecord::Epoch(record)
                }
                KEY_ENVELOPE if row.recipient == Some(recipient) => {
                    let envelope =
                        Signed::<Envelope>::from_bytes(&row.record).map_err(Error::internal)?;
                    if envelope.body.epoch != epoch {
                        continue;
                    }
                    KeyRecord::Envelope(envelope)
                }
                KEY_REWRAP => {
                    let request =
                        RewrapRequest::from_bytes(&row.record).map_err(Error::internal)?;
                    if request.epoch != epoch {
                        continue;
                    }
                    KeyRecord::Rewrap(request)
                }
                _ => continue,
            };
            let size = 8 + 1 + 4 + row.record.len();
            if size > budget {
                break 'read;
            }
            budget -= size;
            entries.push(KeyEntry {
                key_seq: row.key_seq,
                record,
            });
        }
    }
    Ok(KeysResponse {
        head_key_seq,
        entries: List(entries),
    })
}

/// The open re-wrap requests of `device` that an answer at or below key
/// sequence `after` acknowledges: the device's key fetch passed that
/// envelope, so the server closes the request (§9).
pub(crate) async fn acknowledged_rewraps(
    txn: &mut impl ReadTxn,
    workspace: Id,
    device: Id,
    after: u64,
) -> Result<Vec<RewrapRow>> {
    let mut acknowledged = Vec::new();
    for request in txn.rewrap_requests(workspace).await? {
        if request.device != device {
            continue;
        }
        let answers = txn
            .rewrap_answers(workspace, request.domain, request.epoch, device)
            .await?;
        if answers.iter().any(|(_, key_seq)| *key_seq <= after) {
            acknowledged.push(request);
        }
    }
    Ok(acknowledged)
}

/// `POST .../domains/{d}/rewrap`: a device that the current policy lets read
/// the domain, and for which the server took an envelope of the domain's
/// current epoch, asks for a new one (§9). The request reaches the
/// domain's readers in the key stream; one per device and epoch, and a
/// repeated request returns the original result. None while the domain
/// waits for a rotation, which hands out a new key anyway.
pub(crate) async fn rewrap(
    txn: &mut impl WriteTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    epoch: u64,
) -> Result<()> {
    txn.lock(&crate::LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: true,
        domains: vec![domain],
        blobs: Vec::new(),
    })
    .await?;
    let policy = authorize_domain(txn, cache, session, workspace, domain).await?;
    let snapshot = policy.head_snapshot();
    let device = reader(snapshot, session).unwrap_or(session.device);
    let row = txn
        .domain(workspace, domain)
        .await?
        .ok_or_else(|| refuse("not_found", "no such domain"))?;
    if epoch != row.epoch {
        return Err(refuse(
            "epoch_stale",
            "a re-wrap asks for the domain's current epoch",
        ));
    }
    let (head_revision, _) = policy.head();
    if policy
        .rotation_required(&domain, row.epoch_policy_revision, head_revision)
        .is_some()
    {
        return Err(refuse(
            "rotation_pending",
            "the domain waits for a rotation, which wraps a new key",
        ));
    }
    if txn
        .rewrap_requests(workspace)
        .await?
        .iter()
        .any(|r| r.domain == domain && r.epoch == epoch && r.device == device)
    {
        return Ok(());
    }
    let mut accepted = false;
    let mut cursor = 0;
    loop {
        let rows = txn.key_entries(workspace, cursor, CHUNK).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.key_seq;
            if row.record_type == KEY_ENVELOPE
                && row.domain == domain
                && row.recipient == Some(device)
            {
                let envelope =
                    Signed::<Envelope>::from_bytes(&row.record).map_err(Error::internal)?;
                accepted |= envelope.body.epoch == epoch;
            }
        }
    }
    if !accepted {
        return Err(refuse(
            "forbidden",
            "the server took no envelope of this epoch for the device",
        ));
    }
    let record = RewrapRequest {
        domain,
        device,
        epoch,
    }
    .to_bytes();
    let key_seq = txn
        .append_key_entry(workspace, KEY_REWRAP, domain, epoch, None, &record)
        .await?;
    txn.insert_rewrap_request(
        workspace,
        &RewrapRow {
            domain,
            epoch,
            device,
            key_seq,
        },
    )
    .await
}

/// `GET .../domains/{d}/epochs/{epoch}`: that epoch record and the ones its
/// `previous` links reach, newest first, as many as fit 1 MiB.
pub(crate) async fn epochs(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    epoch: u64,
) -> Result<EpochsResponse> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let mut next = txn.epoch(workspace, domain, epoch).await?;
    if next.is_none() {
        return Err(refuse("not_found", "no such epoch"));
    }
    let mut budget = MAX_BODY.saturating_sub(4);
    let mut records = Vec::new();
    while let Some(row) = next {
        let size = 4 + row.record.len();
        if size > budget {
            break;
        }
        budget -= size;
        records.push(Nested(
            Signed::<Epoch>::from_bytes(&row.record).map_err(Error::internal)?,
        ));
        next = if row.previous.is_zero() {
            None
        } else {
            txn.epoch_by_digest(workspace, domain, row.previous).await?
        };
    }
    Ok(EpochsResponse {
        epochs: List(records),
    })
}

/// `GET .../domains/{d}/chain/{device}/{counter}`: the device's record at
/// that counter, or its next one above it.
pub(crate) async fn chain_record(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    device: Id,
    counter: u64,
) -> Result<sync_protocol::messages::Entry> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let pruned = || refuse("pruned", "the server pruned that chain position");
    let Some(raw) = txn
        .position_at_or_above(workspace, domain, device, counter)
        .await?
    else {
        // The head row outlives its record (§19).
        return Err(match txn.chain_head(workspace, domain, device).await? {
            Some(head) if counter <= head.counter => pruned(),
            _ => refuse(
                "not_found",
                "the device has no record at or above that counter",
            ),
        });
    };
    // A record above the counter either follows the record below it, so
    // the device skipped the counter, or pruning took what lay between.
    if let EntryRecord::Operation(op) = entry(raw.clone())?.record
        && op.body.counter != counter
    {
        let below = txn
            .position_below(workspace, domain, device, counter)
            .await?
            .map_or(Digest::ZERO, |(_, digest)| digest);
        if op.body.previous != below {
            return Err(pruned());
        }
    }
    // Another device's record at or below the account's history floor
    // stays out of reach (§9); the device's own records it signed.
    if device != session.device
        && raw.seq
            <= txn
                .history_floor(workspace, session.account, domain)
                .await?
    {
        return Err(refuse(
            "before_history_floor",
            "that record lies below the account's history floor",
        ));
    }
    entry(raw)
}

/// `GET .../domains/{d}/heads/{device}`.
pub(crate) async fn chain_head(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    domain: Id,
    device: Id,
) -> Result<HeadResponse> {
    authorize_domain(txn, cache, session, workspace, domain).await?;
    let head = txn
        .chain_head(workspace, domain, device)
        .await?
        .ok_or_else(|| refuse("not_found", "the device has no record in this domain"))?;
    Ok(HeadResponse {
        counter: head.counter,
        record_digest: head.digest,
        seq: head.seq,
    })
}
