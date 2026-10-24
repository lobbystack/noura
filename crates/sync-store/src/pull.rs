//! Pull (§15): one page per listed domain from one snapshot, at most 1 MiB
//! in all.

use sync_protocol::codec::{Decode, Sorted};
use sync_protocol::messages::{
    Entries, Entry, EntryRecord, MAX_BODY, Page, PageStatus, PolicyEntry, PullCursor,
};
use sync_protocol::records::{CheckpointBundle, Epoch, Operation, Signed};
use sync_protocol::types::Id;

use crate::policy::{self, PolicyCache};
use crate::{Error, RawEntry, ReadTxn, Result, Session, refuse};

/// The fixed bytes of a page: its fields and its entry count.
const PAGE_FIXED: usize = 16 + 1 + 8 * 8 + 4;
/// An entry's bytes besides its record: sequence, type and length.
const ENTRY_FIXED: usize = 8 + 1 + 4;
/// Entries read per query.
const CHUNK: u32 = 256;

/// Decodes a stored log entry.
pub(crate) fn entry(raw: RawEntry) -> Result<Entry> {
    let record = match raw.record_type {
        1 => EntryRecord::Operation(
            Signed::<Operation>::from_bytes(&raw.record).map_err(Error::internal)?,
        ),
        5 => EntryRecord::Checkpoint(
            CheckpointBundle::from_bytes(&raw.record).map_err(Error::internal)?,
        ),
        8 => EntryRecord::Epoch(Signed::<Epoch>::from_bytes(&raw.record).map_err(Error::internal)?),
        9 => EntryRecord::Policy(PolicyEntry::from_bytes(&raw.record).map_err(Error::internal)?),
        other => return Err(Error::internal(format!("unknown entry type {other}"))),
    };
    Ok(Entry {
        seq: raw.seq,
        record,
    })
}

/// The pages of one pull, read in the caller's one transaction.
pub(crate) async fn snapshot(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    generation: u64,
    session: Session,
    workspace: Id,
    cursors: &[PullCursor],
) -> Result<Vec<Page>> {
    let Some((row, policy)) = policy::load(txn, cache, workspace).await? else {
        return Err(refuse("not_found", "no such workspace"));
    };
    let snapshot = policy.head_snapshot();
    let active = snapshot.is_device(&session.device);
    // A device the policy took out gets status 4 on every page; a device
    // that never belonged to the workspace gets nothing.
    let former = (1..row.policy_revision)
        .any(|r| policy.at(r).is_some_and(|s| s.is_device(&session.device)));
    if !active && !former {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    // Every page's fixed fields come off the budget before any page takes
    // entries, so one domain's entries can't use bytes a later page needs.
    // `Store::pull` caps the cursors at 256, whose 21,760 fixed bytes leave
    // room for the largest entry.
    let mut budget = MAX_BODY
        .saturating_sub(4)
        .saturating_sub(PAGE_FIXED.saturating_mul(cursors.len()));
    let mut pages = Vec::with_capacity(cursors.len());
    for cursor in cursors {
        let empty = |status: PageStatus| Page {
            domain: cursor.domain,
            status,
            after: cursor.after,
            complete_through: cursor.after,
            head: 0,
            retention_floor: 0,
            checkpoint_seq: 0,
            restore_generation: generation,
            policy_revision: row.policy_revision,
            checkpoint_due: 0,
            entries: Entries::default(),
        };
        if !active {
            pages.push(empty(PageStatus::Forbidden));
            continue;
        }
        if !snapshot.domains.contains_key(&cursor.domain) {
            pages.push(empty(PageStatus::NotFound));
            continue;
        }
        if !snapshot.can_read(&session.device, &cursor.domain) {
            pages.push(empty(PageStatus::Forbidden));
            continue;
        }
        let Some(domain) = txn.domain(workspace, cursor.domain).await? else {
            pages.push(empty(PageStatus::NotFound));
            continue;
        };
        let mut entries = Vec::new();
        let mut complete_through = cursor.after;
        let mut truncated = false;
        'read: while complete_through < domain.head_seq {
            let chunk = txn
                .entries_after(workspace, cursor.domain, complete_through, CHUNK)
                .await?;
            if chunk.is_empty() {
                break;
            }
            for raw in chunk {
                let size = ENTRY_FIXED + raw.record.len();
                if size > budget {
                    truncated = true;
                    break 'read;
                }
                budget -= size;
                complete_through = raw.seq;
                entries.push(entry(raw)?);
            }
        }
        if !truncated {
            complete_through = complete_through.max(domain.head_seq);
        }
        pages.push(Page {
            complete_through,
            head: domain.head_seq,
            retention_floor: domain.retention_floor,
            entries: Sorted::new(entries).map_err(|e| Error::internal(e.name()))?,
            ..empty(PageStatus::Ok)
        });
    }
    Ok(pages)
}
