//! Live sets (§13, "Supersession and live operations").
//!
//! The server and the client compute the same live set for each `rid` from
//! cleartext fields: an operation is live unless another operation on the
//! same `rid` covers it, and a lone delete that a rising checkpoint `floor`
//! passes leaves the live set for good (§17).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::records::{Cover, MAX_COVERS, OpClass, Operation, Ref, Signed};
use crate::types::{Digest, Id};

/// The most live patches a `rid` holds on the server (§27).
pub const MAX_LIVE_PATCHES: usize = 2_048;
/// The most ciphertext bytes of live patches a `rid` holds.
pub const MAX_LIVE_PATCH_BYTES: u64 = 8 * 1024 * 1024;
/// The most live state operations a `rid` holds.
pub const MAX_LIVE_STATES: usize = 64;

/// The cleartext fields of an operation that the live set needs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveOp {
    /// The object.
    pub rid: Id,
    /// The signing device.
    pub device: Id,
    /// Its chain counter.
    pub counter: u64,
    /// The log sequence.
    pub seq: u64,
    /// The operation's digest.
    pub digest: Digest,
    /// The delete flag.
    pub delete: bool,
    /// Its covers.
    pub covers: Vec<Cover>,
    /// Its ciphertext length, plus any new blobs it lists, for the patch
    /// byte limit.
    pub weight: u64,
}

impl LiveOp {
    /// The live-set fields of an operation at sequence `seq`.
    pub fn from_operation(op: &Signed<Operation>, seq: u64) -> Self {
        Self {
            rid: op.body.rid,
            device: op.body.device,
            counter: op.body.counter,
            seq,
            digest: op.digest(),
            delete: op.body.delete,
            covers: op.body.supersedes.to_vec(),
            weight: op.body.ciphertext.len() as u64,
        }
    }

    /// The operation's `ref`.
    pub fn as_ref(&self) -> Ref {
        Ref {
            device: self.device,
            counter: self.counter,
            seq: self.seq,
            op_digest: self.digest,
        }
    }

    fn covered_by(&self, cover: &Cover) -> bool {
        self.device == cover.device && self.counter <= cover.counter
    }
}

#[derive(Clone, Debug, Default)]
struct RidState {
    /// Every operation on the `rid` the store retains, in log order, with
    /// the class the server gave it.
    ops: Vec<(LiveOp, OpClass)>,
    /// Lone deletes a floor dropped, by device and counter.
    dropped: BTreeSet<(Id, u64)>,
}

/// Why the server refuses an operation on its live set (§13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveError {
    /// A cover names no existing operation on this `rid` with exactly that
    /// device and counter.
    #[error("supersedes_unknown")]
    SupersedesUnknown,
    /// A state operation leaves its own device's earlier state operation
    /// on the `rid` live.
    #[error("supersedes_incomplete")]
    SupersedesIncomplete,
    /// The `rid` would hold too many live patches.
    #[error("snapshot_required")]
    SnapshotRequired,
    /// The `rid` would hold too many live state operations.
    #[error("conflict_limit")]
    ConflictLimit,
}

impl LiveError {
    /// The server error code.
    pub fn server_code(&self) -> &'static str {
        match self {
            LiveError::SupersedesUnknown => "supersedes_unknown",
            LiveError::SupersedesIncomplete => "supersedes_incomplete",
            LiveError::SnapshotRequired => "snapshot_required",
            LiveError::ConflictLimit => "conflict_limit",
        }
    }
}

/// The live sets of one domain.
#[derive(Clone, Debug, Default)]
pub struct LiveSet {
    rids: BTreeMap<Id, RidState>,
}

impl LiveSet {
    /// An empty domain.
    pub fn new() -> Self {
        Self::default()
    }

    /// The class the server gives `op`: a patch when its `supersedes` is
    /// empty and its `rid` already holds an operation, else a state
    /// operation.
    pub fn class_of(&self, op: &LiveOp) -> OpClass {
        if op.covers.is_empty() && self.rids.contains_key(&op.rid) {
            OpClass::Patch
        } else {
            OpClass::State
        }
    }

    /// The server's checks of §13 that the live set decides.
    pub fn check(&self, op: &LiveOp) -> Result<OpClass, LiveError> {
        let class = self.class_of(op);
        let state = self.rids.get(&op.rid);
        for cover in &op.covers {
            let exists = state.is_some_and(|s| {
                s.ops
                    .iter()
                    .any(|(o, _)| o.device == cover.device && o.counter == cover.counter)
            });
            if !exists {
                return Err(LiveError::SupersedesUnknown);
            }
        }
        let mut after = self.clone();
        after.insert(op.clone(), class);
        let live = after.live(&op.rid);
        if class == OpClass::State {
            let own_states = live
                .iter()
                .filter(|(o, c)| *c == OpClass::State && o.device == op.device)
                .count();
            if own_states > 1 {
                return Err(LiveError::SupersedesIncomplete);
            }
        }
        let patches: Vec<_> = live.iter().filter(|(_, c)| *c == OpClass::Patch).collect();
        if patches.len() > MAX_LIVE_PATCHES
            || patches.iter().map(|(o, _)| o.weight).sum::<u64>() > MAX_LIVE_PATCH_BYTES
        {
            return Err(LiveError::SnapshotRequired);
        }
        if live.iter().filter(|(_, c)| *c == OpClass::State).count() > MAX_LIVE_STATES {
            return Err(LiveError::ConflictLimit);
        }
        Ok(class)
    }

    /// Takes an accepted operation, classing it as the server does.
    pub fn apply(&mut self, op: LiveOp) -> OpClass {
        let class = self.class_of(&op);
        self.insert(op, class);
        class
    }

    fn insert(&mut self, op: LiveOp, class: OpClass) {
        self.rids.entry(op.rid).or_default().ops.push((op, class));
    }

    /// The operations live on `rid`, with their classes, in log order.
    pub fn live(&self, rid: &Id) -> Vec<(&LiveOp, OpClass)> {
        self.live_at(rid, u64::MAX)
    }

    /// The operations live on `rid` at sequence `at`: those at or below it
    /// that no operation at or below it covers, less dropped lone deletes.
    pub fn live_at(&self, rid: &Id, at: u64) -> Vec<(&LiveOp, OpClass)> {
        let Some(state) = self.rids.get(rid) else {
            return Vec::new();
        };
        let visible: Vec<&(LiveOp, OpClass)> =
            state.ops.iter().filter(|(o, _)| o.seq <= at).collect();
        visible
            .iter()
            .filter(|(o, _)| !state.dropped.contains(&(o.device, o.counter)))
            .filter(|(o, _)| {
                !visible
                    .iter()
                    .any(|(other, _)| other.covers.iter().any(|c| o.covered_by(c)))
            })
            .map(|(o, c)| (o, *c))
            .collect()
    }

    /// Every `rid` with at least one live operation at `at`, with its live
    /// refs sorted by device and counter: the manifest's leaves.
    pub fn leaves_at(&self, at: u64) -> BTreeMap<Id, Vec<Ref>> {
        self.rids
            .keys()
            .filter_map(|rid| {
                let mut refs: Vec<Ref> = self
                    .live_at(rid, at)
                    .iter()
                    .map(|(o, _)| o.as_ref())
                    .collect();
                if refs.is_empty() {
                    return None;
                }
                refs.sort_by_key(|r| (r.device, r.counter));
                Some((*rid, refs))
            })
            .collect()
    }

    /// Applies a checkpoint's rising floor (§17): each delete whose
    /// sequence lies in (`previous_floor`, `floor`] and that is its `rid`'s
    /// only live operation at `covered_seq` leaves the live set for good.
    /// Returns the dropped deletes.
    pub fn apply_floor(
        &mut self,
        previous_floor: u64,
        floor: u64,
        covered_seq: u64,
    ) -> Vec<(Id, Ref)> {
        let dropped: Vec<(Id, Ref)> = self
            .rids
            .keys()
            .filter_map(|rid| match self.live_at(rid, covered_seq).as_slice() {
                [(op, _)] if op.delete && op.seq > previous_floor && op.seq <= floor => {
                    Some((*rid, op.as_ref()))
                }
                _ => None,
            })
            .collect();
        for (rid, r) in &dropped {
            if let Some(state) = self.rids.get_mut(rid) {
                state.dropped.insert((r.device, r.counter));
            }
        }
        dropped
    }
}

/// The covers of a state operation that covers `chosen`, the live
/// operations the writer's view includes, together with its own device's
/// live operations. Each device's cover names its highest chosen counter
/// below any live operation of that device the writer leaves live, so the
/// other branches of a conflict stay live. Past 256 devices it keeps the 256
/// with the most live operations on the `rid`, its own among them.
pub fn covers_for(live: &[&LiveOp], chosen: &BTreeSet<(Id, u64)>, own_device: Id) -> Vec<Cover> {
    let mut by_device: BTreeMap<Id, (Option<u64>, Option<u64>, usize)> = BTreeMap::new();
    for op in live {
        let entry = by_device.entry(op.device).or_insert((None, None, 0));
        entry.2 += 1;
        if chosen.contains(&(op.device, op.counter)) || op.device == own_device {
            entry.0 = Some(entry.0.map_or(op.counter, |c: u64| c.max(op.counter)));
        } else {
            entry.1 = Some(entry.1.map_or(op.counter, |c: u64| c.min(op.counter)));
        }
    }
    let mut covers: Vec<(usize, Cover)> = by_device
        .into_iter()
        .filter_map(|(device, (highest, lowest_kept, count))| {
            let counter = match (highest, lowest_kept) {
                (Some(h), Some(k)) if h >= k => k.checked_sub(1).filter(|c| *c > 0)?,
                (Some(h), _) => h,
                (None, _) => return None,
            };
            Some((count, Cover { device, counter }))
        })
        .collect();
    if covers.len() > MAX_COVERS {
        covers.sort_by(|(a_count, a), (b_count, b)| {
            let a_own = a.device == own_device;
            let b_own = b.device == own_device;
            b_own
                .cmp(&a_own)
                .then(b_count.cmp(a_count))
                .then(a.device.cmp(&b.device))
        });
        covers.truncate(MAX_COVERS);
    }
    let mut covers: Vec<Cover> = covers.into_iter().map(|(_, c)| c).collect();
    covers.sort_by_key(|c| c.device);
    covers
}
