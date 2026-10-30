//! What the realtime channel tells one connected device (§23): after a
//! push commits, the new entries of each domain the device reads, in a
//! records frame when they fit one frame and otherwise as a notify, and
//! hints that the policy, the key stream or a member's account chain moved.
//!
//! A [`Watch`] keeps what the connection already told the device. Every
//! read takes one snapshot, so the entries a records frame carries are the
//! committed ones, from the entry after the head the device last heard of.

use std::collections::BTreeMap;

use sync_protocol::codec::Sorted;
use sync_protocol::messages::{Frame, MAX_FRAME};
use sync_protocol::types::{Digest, Id};

use crate::policy::{self, PolicyCache};
use crate::pull::{self, CHUNK, ENTRY_FIXED};
use crate::{Error, ReadTxn, Result, Session, refuse};

/// A records frame's fixed body bytes: domain, `previous_seq`,
/// `checkpoint_due` and the entry count.
const RECORDS_FIXED: usize = 16 + 8 + 8 + 4;

/// What a connection told its device so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// The head of each domain the device reads, as last told.
    pub heads: BTreeMap<Id, u64>,
    /// The policy head revision.
    pub policy_revision: u64,
    /// The key stream's head.
    pub keys: u64,
    /// Each member account's current chain, as genesis and head seqno.
    pub accounts: BTreeMap<Id, (Digest, u64)>,
}

/// Reads the workspace's state for `session` in one snapshot and returns
/// the frames that tell what changed since `watch`, which it brings up to
/// date. A device that isn't active in the workspace gets `forbidden`.
pub(crate) async fn read(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
    watch: &mut Watch,
    first: bool,
) -> Result<Vec<Frame>> {
    let Some((row, policy)) = policy::load(txn, cache, workspace).await? else {
        return Err(refuse("not_found", "no such workspace"));
    };
    let snapshot = policy.head_snapshot();
    if !snapshot.is_device(&session.device) {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    let mut frames = Vec::new();
    let readable: Vec<Id> = snapshot
        .domains
        .keys()
        .copied()
        .filter(|domain| snapshot.can_read(&session.device, domain))
        .collect();
    // A domain the device no longer reads drops out, so a later grant
    // starts from the head it then has.
    watch.heads.retain(|domain, _| readable.contains(domain));
    for domain in readable {
        let Some(row) = txn.domain(workspace, domain).await? else {
            continue;
        };
        let told = watch.heads.get(&domain).copied();
        watch.heads.insert(domain, row.head_seq);
        let previous = match told {
            Some(told) if row.head_seq > told => told,
            Some(_) => continue,
            // The device learns heads the first time from its pull.
            None if first => continue,
            // A domain it just started to read: everything from the start.
            None => 0,
        };
        // Nothing at or below the account's history floor reaches a frame
        // (§9); a device that sees the gap pulls, and bootstraps from there.
        let floor = txn
            .history_floor(workspace, session.account, domain)
            .await?;
        let previous = previous.max(floor);
        if previous >= row.head_seq {
            continue;
        }
        let checkpoint_due = row.checkpoint_due;
        frames.push(
            match entries(txn, workspace, domain, previous, row.head_seq).await? {
                Some(entries) => Frame::Records {
                    domain,
                    previous_seq: previous,
                    checkpoint_due,
                    entries: Sorted::new(entries).map_err(|e| Error::internal(e.name()))?,
                },
                None => Frame::Notify {
                    domain,
                    head: row.head_seq,
                    checkpoint_due,
                },
            },
        );
    }
    if row.policy_revision > watch.policy_revision && !first {
        frames.push(Frame::PolicyChanged {
            revision: row.policy_revision,
        });
    }
    watch.policy_revision = watch.policy_revision.max(row.policy_revision);
    // A new epoch, envelope or re-wrap request appends to the key stream.
    let keys = txn.key_head(workspace).await?;
    if keys > watch.keys && !first {
        frames.push(Frame::KeysChanged);
    }
    watch.keys = watch.keys.max(keys);
    let members: Vec<Id> = snapshot.members.keys().copied().collect();
    watch
        .accounts
        .retain(|account, _| members.contains(account));
    for account in members {
        let Some(head) = txn.current_head(account).await? else {
            continue;
        };
        let now = (head.genesis, head.seqno);
        let before = watch.accounts.insert(account, now);
        if !first && before != Some(now) {
            frames.push(Frame::AccountChanged {
                account,
                genesis: head.genesis,
                seqno: head.seqno,
            });
        }
    }
    Ok(frames)
}

/// Every entry of `domain` after `previous` through `head`, when they all
/// fit one records frame; `None` when they don't.
async fn entries(
    txn: &mut impl ReadTxn,
    workspace: Id,
    domain: Id,
    previous: u64,
    head: u64,
) -> Result<Option<Vec<sync_protocol::messages::Entry>>> {
    let mut budget = MAX_FRAME - RECORDS_FIXED;
    let mut found = Vec::new();
    let mut after = previous;
    while after < head {
        let chunk = txn.entries_after(workspace, domain, after, CHUNK).await?;
        if chunk.is_empty() {
            break;
        }
        for raw in chunk {
            if raw.seq > head {
                return Ok(Some(found));
            }
            let size = ENTRY_FIXED + raw.record.len();
            if size > budget {
                return Ok(None);
            }
            budget -= size;
            after = raw.seq;
            found.push(pull::entry(raw)?);
        }
    }
    // Entries that compaction removed leave nothing to forward.
    Ok((!found.is_empty()).then_some(found))
}
