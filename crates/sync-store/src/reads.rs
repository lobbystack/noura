//! The read routes of §26 and the account chain writes that tests use
//! until ticket 46 adds the chain routes.

use std::collections::BTreeSet;
use std::sync::Arc;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{
    EpochsResponse, HeadResponse, KeyEntry, KeyRecord, KeysResponse, MAX_BODY, PolicyResponse,
    WorkspaceInfo, WorkspacesResponse,
};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{AccountLink, Envelope, Epoch, PolicyChange, Profile, Role, Signed};
use sync_protocol::types::Id;

use crate::policy::{self, PolicyCache};
use crate::pull::entry;
use crate::push::{KEY_ENVELOPE, KEY_EPOCH};
use crate::{Error, LinkRow, ReadTxn, Result, Session, WorkspaceRow, WriteTxn, refuse};

/// Key stream rows read per query.
const CHUNK: u32 = 256;

/// The session device's account chain: its account's current chain, in
/// which the device's card must be active at the head. A card that a link
/// removed gets `forbidden` until tickets 46 and 47 answer it with
/// `device_removed`.
pub(crate) async fn session_chain(
    txn: &mut impl ReadTxn,
    session: Session,
) -> Result<AccountChain> {
    policy::current_chain(txn, session.account)
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
/// genesis, after checking every rule of §7. Returns the new head's seqno.
pub(crate) async fn append_account_links(
    txn: &mut impl WriteTxn,
    links: &[Signed<AccountLink>],
) -> Result<u64> {
    let first = links
        .first()
        .ok_or_else(|| refuse("bad_request", "no links"))?;
    let account = first.body.account;
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
    if !policy.head_snapshot().is_device(&session.device) {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    Ok((row, policy))
}

/// Like [`authorize`], and the domain exists and the device reads it.
async fn authorize_domain(
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
    if !snapshot.can_read(&session.device, &domain) {
        return Err(refuse("forbidden", "the device can't read this domain"));
    }
    Ok(policy)
}

/// `GET /v2/workspaces`.
pub(crate) async fn workspaces(
    txn: &mut impl ReadTxn,
    session: Session,
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
        // Usage, history and quotas arrive with ticket 59.
        workspaces.push(WorkspaceInfo {
            workspace,
            role,
            used_bytes: 0,
            history_bytes: 0,
            quota_bytes: None,
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
    let (row, policy) = authorize(txn, cache, session, workspace).await?;
    let mut heads = Vec::new();
    for account in policy.head_snapshot().members.keys() {
        if let Some(chain) = policy::current_chain(txn, *account).await? {
            heads.push(chain.head());
        }
    }
    let heads = Sorted::new(heads).map_err(|e| Error::internal(e.name()))?;
    let mut budget = MAX_BODY.saturating_sub(8 + 4 + heads.to_bytes().len());
    let mut changes = Vec::new();
    for bytes in txn.policy_changes(workspace, after).await? {
        let size = 4 + bytes.len();
        if size > budget {
            break;
        }
        budget -= size;
        changes.push(Nested(
            Signed::<PolicyChange>::from_bytes(&bytes).map_err(Error::internal)?,
        ));
    }
    Ok(PolicyResponse {
        head_revision: row.policy_revision,
        changes: List(changes),
        account_heads: heads,
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
    // A recovery identity's proven session reads its own envelopes too
    // (ticket 47); every other session reads only its device's.
    if recipient != session.device {
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
            let reads = snapshot.can_read(&session.device, &row.domain);
            let record = match row.record_type {
                KEY_EPOCH if reads => KeyRecord::Epoch(
                    Signed::<Epoch>::from_bytes(&row.record).map_err(Error::internal)?,
                ),
                KEY_ENVELOPE if reads && row.recipient == Some(recipient) => KeyRecord::Envelope(
                    Signed::<Envelope>::from_bytes(&row.record).map_err(Error::internal)?,
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
    let raw = txn
        .position_at_or_above(workspace, domain, device, counter)
        .await?
        .ok_or_else(|| {
            refuse(
                "not_found",
                "the device has no record at or above that counter",
            )
        })?;
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
