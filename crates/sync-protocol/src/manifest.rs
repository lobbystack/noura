//! The Merkle-rooted manifest and the checkpoint chain (§17).
//!
//! ```text
//! leaf_hash       = BLAKE3(tag(noura.sync.manifest.leaf) || rid || list<ref>)
//! bucket_hash(i)  = BLAKE3(tag(noura.sync.manifest.bucket) || u16 i || list<digest>)
//! node(2, j)      = BLAKE3(tag(noura.sync.manifest.node) || u8 2 || u16 j || 16 bucket hashes)
//! node(1, j)      = BLAKE3(tag(noura.sync.manifest.node) || u8 1 || u16 j || 16 level-2 nodes)
//! root            = BLAKE3(tag(noura.sync.manifest.node) || u8 0 || u16 0 || 16 level-1 nodes)
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::codec::{Writer, encode_list};
use crate::crypto::hash_parts;
use crate::records::{Checkpoint, Ref, Signed};
use crate::tag::Tag;
use crate::types::{Digest, Id};

/// The number of buckets.
pub const BUCKETS: usize = 4_096;

/// The bucket of a `rid`: its first 12 bits.
pub fn bucket_of(rid: &Id) -> u16 {
    (u16::from(rid.0[0]) << 4) | (u16::from(rid.0[1]) >> 4)
}

/// A leaf's hash over the `rid` and its live refs, sorted by device, then
/// counter.
pub fn leaf_hash(rid: &Id, refs: &[Ref]) -> Digest {
    let mut w = Writer::new();
    w.tag(Tag::ManifestLeaf);
    w.put(rid);
    encode_list(&mut w, refs);
    hash_parts(&[w.as_bytes()])
}

/// A bucket's hash over its leaf hashes, sorted by `rid`.
pub fn bucket_hash(index: u16, leaf_hashes: &[Digest]) -> Digest {
    let mut w = Writer::new();
    w.tag(Tag::ManifestBucket);
    w.u16(index);
    encode_list(&mut w, leaf_hashes);
    hash_parts(&[w.as_bytes()])
}

/// A node's hash over its 16 children, concatenated.
pub fn node_hash(level: u8, index: u16, children: &[Digest]) -> Digest {
    debug_assert_eq!(children.len(), 16);
    let mut w = Writer::new();
    w.tag(Tag::ManifestNode);
    w.u8(level);
    w.u16(index);
    for child in children {
        w.put(child);
    }
    hash_parts(&[w.as_bytes()])
}

/// The root over all 4,096 bucket hashes.
pub fn root_of_buckets(buckets: &[Digest]) -> Digest {
    assert_eq!(buckets.len(), BUCKETS, "a manifest has 4,096 buckets");
    let level2: Vec<Digest> = (0..256)
        .map(|j| node_hash(2, j as u16, &buckets[j * 16..j * 16 + 16]))
        .collect();
    let level1: Vec<Digest> = (0..16)
        .map(|j| node_hash(1, j as u16, &level2[j * 16..j * 16 + 16]))
        .collect();
    node_hash(0, 0, &level1)
}

/// The 16 children of a node, as `manifest` levels 0 to 2 return them.
pub fn node_children(buckets: &[Digest], level: u8, index: u16) -> Option<Vec<Digest>> {
    let index = usize::from(index);
    let level2 = |j: usize| node_hash(2, j as u16, &buckets[j * 16..j * 16 + 16]);
    let level1 = |j: usize| {
        let children: Vec<Digest> = (j * 16..j * 16 + 16).map(level2).collect();
        node_hash(1, j as u16, &children)
    };
    match level {
        0 if index == 0 => Some((0..16).map(level1).collect()),
        1 if index < 16 => Some((index * 16..index * 16 + 16).map(level2).collect()),
        2 if index < 256 => Some(buckets[index * 16..index * 16 + 16].to_vec()),
        _ => None,
    }
}

/// `heads_digest = BLAKE3(tag(noura.sync.manifest.heads) || list<ref>)`.
pub fn heads_digest(heads: &[Ref]) -> Digest {
    let mut w = Writer::new();
    w.tag(Tag::ManifestHeads);
    encode_list(&mut w, heads);
    hash_parts(&[w.as_bytes()])
}

/// A domain's manifest: each live `rid` with its live refs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    leaves: BTreeMap<Id, Vec<Ref>>,
}

impl Manifest {
    /// A manifest over `leaves`, each with its refs sorted by device, then
    /// counter. A `rid` without refs has no leaf.
    pub fn new(leaves: BTreeMap<Id, Vec<Ref>>) -> Self {
        let leaves = leaves
            .into_iter()
            .filter(|(_, refs)| !refs.is_empty())
            .map(|(rid, mut refs)| {
                refs.sort_by_key(|r| (r.device, r.counter));
                (rid, refs)
            })
            .collect();
        Self { leaves }
    }

    /// The number of leaves, a checkpoint's `live_objects`.
    pub fn live_objects(&self) -> u64 {
        self.leaves.len() as u64
    }

    /// The leaves.
    pub fn leaves(&self) -> &BTreeMap<Id, Vec<Ref>> {
        &self.leaves
    }

    /// The 4,096 bucket hashes.
    pub fn bucket_hashes(&self) -> Vec<Digest> {
        let mut per_bucket: Vec<Vec<Digest>> = vec![Vec::new(); BUCKETS];
        for (rid, refs) in &self.leaves {
            per_bucket[usize::from(bucket_of(rid))].push(leaf_hash(rid, refs));
        }
        per_bucket
            .iter()
            .enumerate()
            .map(|(i, hashes)| bucket_hash(i as u16, hashes))
            .collect()
    }

    /// The root.
    pub fn root(&self) -> Digest {
        root_of_buckets(&self.bucket_hashes())
    }

    /// The leaf refs of one bucket, as level 3 of `manifest` returns them.
    pub fn bucket_leaves(&self, index: u16) -> Vec<(Id, Vec<Ref>)> {
        self.leaves
            .iter()
            .filter(|(rid, _)| bucket_of(rid) == index)
            .map(|(rid, refs)| (*rid, refs.clone()))
            .collect()
    }
}

/// Why a manifest or one of its parts fails against the root it should
/// hash to: `sync_manifest_mismatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("sync_manifest_mismatch")]
pub struct ManifestMismatch;

/// Checks 4,096 bucket hashes against a checkpoint's root.
pub fn verify_buckets(buckets: &[Digest], root: &Digest) -> Result<(), ManifestMismatch> {
    if buckets.len() != BUCKETS || root_of_buckets(buckets) != *root {
        return Err(ManifestMismatch);
    }
    Ok(())
}

/// Checks one bucket's leaves, as a bootstrap fetches them, against its
/// hash. Leaves out of order, in another bucket, or missing fail.
pub fn verify_bucket(
    index: u16,
    leaves: &[(Id, Vec<Ref>)],
    hash: &Digest,
) -> Result<(), ManifestMismatch> {
    let sorted = leaves.windows(2).all(|w| w[0].0 < w[1].0);
    if !sorted || leaves.iter().any(|(rid, _)| bucket_of(rid) != index) {
        return Err(ManifestMismatch);
    }
    let hashes: Vec<Digest> = leaves
        .iter()
        .map(|(rid, refs)| leaf_hash(rid, refs))
        .collect();
    if bucket_hash(index, &hashes) != *hash {
        return Err(ManifestMismatch);
    }
    Ok(())
}

/// Checks a whole set of leaves against a root and a leaf count.
pub fn verify_manifest(
    manifest: &Manifest,
    root: &Digest,
    live_objects: u64,
) -> Result<(), ManifestMismatch> {
    if manifest.root() != *root || manifest.live_objects() != live_objects {
        return Err(ManifestMismatch);
    }
    Ok(())
}

/// The number `skip` names: `number & (number − 1)`, with its lowest set bit
/// cleared.
pub fn skip_number(number: u64) -> u64 {
    number & number.wrapping_sub(1)
}

/// The checkpoint numbers a descent walk visits from `from` back to `to`,
/// newest first: through `skip` wherever that doesn't pass the anchor and
/// through `previous` elsewhere. Both ends included.
pub fn descent_path(from: u64, to: u64) -> Vec<u64> {
    let mut path = vec![from];
    let mut current = from;
    while current > to {
        let skip = skip_number(current);
        current = if skip >= to && skip > 0 {
            skip
        } else {
            current - 1
        };
        path.push(current);
    }
    path
}

/// Why a descent walk fails: `sync_manifest_rollback`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("sync_manifest_rollback")]
pub struct DescentError;

/// Checks a descent walk from a bootstrap checkpoint back to the device's
/// anchor. `walk` runs newest first, from the bootstrap checkpoint to the
/// anchor; each record must be the `previous` or `skip` of the one before it,
/// with the matching number.
pub fn verify_descent(walk: &[Signed<Checkpoint>], anchor: &Digest) -> Result<(), DescentError> {
    let last = walk.last().ok_or(DescentError)?;
    if last.digest() != *anchor {
        return Err(DescentError);
    }
    for pair in walk.windows(2) {
        let (newer, older) = (&pair[0].body, &pair[1]);
        let digest = older.digest();
        let number = older.body.number;
        let via_previous = newer.previous == digest && newer.number == number + 1;
        let via_skip = newer.skip == digest && skip_number(newer.number) == number && number > 0;
        if !(via_previous || via_skip) {
            return Err(DescentError);
        }
    }
    Ok(())
}

/// What a client accepted of a domain's checkpoint chain.
#[derive(Clone, Debug, Default)]
pub struct CheckpointChain {
    by_number: BTreeMap<u64, (Digest, Checkpoint)>,
}

/// A log-entry checkpoint that fails a check of §17. It never becomes the
/// latest accepted checkpoint, and doesn't hold the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointError {
    /// The heads don't hash to `heads_digest`, or one lies above
    /// `covered_seq`.
    #[error("heads mismatch")]
    Heads,
    /// `previous`, `number` or `skip` doesn't follow the chain.
    #[error("chain link mismatch")]
    Link,
    /// `floor` fell, or `covered_seq` didn't rise, or a head fell back, or
    /// a device left that the heads rule keeps: `sync_manifest_rollback`.
    #[error("sync_manifest_rollback")]
    Rollback,
    /// The root or `live_objects` differs from the client's live set:
    /// `sync_manifest_mismatch`.
    #[error("sync_manifest_mismatch")]
    Mismatch,
}

impl CheckpointError {
    /// The client error code.
    pub fn client_code(&self) -> &'static str {
        match self {
            CheckpointError::Heads | CheckpointError::Mismatch => "sync_manifest_mismatch",
            CheckpointError::Link => "sync_chain_gap",
            CheckpointError::Rollback => "sync_manifest_rollback",
        }
    }
}

impl CheckpointChain {
    /// An empty chain, before the domain's first checkpoint.
    pub fn new() -> Self {
        Self::default()
    }

    /// The latest accepted checkpoint.
    pub fn latest(&self) -> Option<(&Digest, &Checkpoint)> {
        self.by_number.values().next_back().map(|(d, c)| (d, c))
    }

    /// The digest of the accepted checkpoint numbered `number`.
    pub fn digest_of(&self, number: u64) -> Option<Digest> {
        self.by_number.get(&number).map(|(d, _)| *d)
    }

    /// Checks a checkpoint the client reached in its log, after applying
    /// every record through its `covered_seq`, against `manifest`, the
    /// client's own live set at `covered_seq`. `may_leave` says whether a
    /// device that the previous checkpoint listed may leave the heads.
    /// The caller checks the signer, policy and epoch rules by position.
    pub fn check(
        &self,
        record: &Signed<Checkpoint>,
        heads: &[Ref],
        manifest: &Manifest,
        previous_heads: &[Ref],
        may_leave: impl Fn(&Id) -> bool,
    ) -> Result<(), CheckpointError> {
        let latest = self.latest();
        let view = ChainView {
            last: latest.map(|(d, c)| (*d, c.number)),
            skip_of: &|n| self.digest_of(n),
            accepted: latest.map(|(_, c)| (c, previous_heads)),
        };
        check_checkpoint(record, heads, manifest, &view, may_leave)
    }

    /// Records an accepted checkpoint.
    pub fn accept(&mut self, record: &Signed<Checkpoint>) {
        self.by_number
            .insert(record.body.number, (record.digest(), record.body.clone()));
    }
}

/// What a client holds of its checkpoint chain when it checks the next
/// checkpoint in its log (§17, Checking a checkpoint).
pub struct ChainView<'a> {
    /// The digest and number of the last checkpoint the client reached,
    /// `None` before the first.
    pub last: Option<(Digest, u64)>,
    /// The digest of the reached checkpoint with a number, when the client
    /// holds it.
    pub skip_of: &'a dyn Fn(u64) -> Option<Digest>,
    /// The latest checkpoint the client accepted, with its heads.
    pub accepted: Option<(&'a Checkpoint, &'a [Ref])>,
}

/// The checks of §17 on a checkpoint in the log that need no policy: its
/// heads digest, its link to the last checkpoint reached through
/// `previous`, `number` and `skip`, nothing going back from the latest
/// accepted checkpoint, and its root and leaf count against `manifest`,
/// the client's own live set at `covered_seq`. `may_leave` says whether a
/// device the accepted checkpoint listed may leave the heads.
pub fn check_checkpoint(
    record: &Signed<Checkpoint>,
    heads: &[Ref],
    manifest: &Manifest,
    view: &ChainView<'_>,
    may_leave: impl Fn(&Id) -> bool,
) -> Result<(), CheckpointError> {
    let body = &record.body;
    if heads_digest(heads) != body.heads_digest || heads.iter().any(|h| h.seq > body.covered_seq) {
        return Err(CheckpointError::Heads);
    }
    let (previous, number) = view.last.unwrap_or((Digest::ZERO, 0));
    let skip = skip_number(body.number);
    let expected_skip = if skip == 0 {
        Some(Digest::ZERO)
    } else {
        (view.skip_of)(skip)
    };
    if body.previous != previous
        || body.number != number + 1
        || expected_skip.is_some_and(|s| s != body.skip)
    {
        return Err(CheckpointError::Link);
    }
    if let Some((accepted, previous_heads)) = view.accepted {
        if body.floor < accepted.floor || body.covered_seq <= accepted.covered_seq {
            return Err(CheckpointError::Rollback);
        }
        for old in previous_heads {
            match heads.iter().find(|h| h.device == old.device) {
                Some(new) if new.counter < old.counter => {
                    return Err(CheckpointError::Rollback);
                }
                None if !may_leave(&old.device) => {
                    return Err(CheckpointError::Rollback);
                }
                _ => {}
            }
        }
    }
    if manifest.root() != body.manifest_root || manifest.live_objects() != body.live_objects {
        return Err(CheckpointError::Mismatch);
    }
    Ok(())
}

/// A checkpoint's place in its domain's chain, as a walk back through
/// `previous` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainLink {
    /// Its `number`.
    pub number: u64,
    /// Its digest.
    pub digest: Digest,
    /// Its `previous`.
    pub previous: Digest,
}

impl ChainLink {
    /// The link of a checkpoint record.
    pub fn of(record: &Signed<Checkpoint>) -> Self {
        Self {
            number: record.body.number,
            digest: record.digest(),
            previous: record.body.previous,
        }
    }
}

/// What a pull page's entry tells a walk for hidden checkpoints (§17,
/// Checkpoint chain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkEntry {
    /// A checkpoint entry.
    Checkpoint(ChainLink),
    /// An operation, which names the latest checkpoint its writer reached,
    /// with the restore generation it was written in.
    Operation {
        /// Its `generation`.
        generation: u64,
        /// Its `checkpoint_seq`.
        checkpoint_seq: u64,
        /// Its `checkpoint`.
        checkpoint: Digest,
    },
    /// Any other entry.
    Other,
}

/// Fetches a checkpoint by digest (`GET .../checkpoints/{digest}`).
pub trait CheckpointFetcher {
    /// The checkpoint with `digest`, or `None` when the server can't serve
    /// it or serves another one.
    fn checkpoint(&mut self, digest: &Digest) -> Option<ChainLink>;
}

/// Why a walk for hidden checkpoints stopped: `sync_chain_gap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("sync_chain_gap")]
pub struct WalkGap;

/// The checkpoints a client reached in a domain, from which it finds the
/// ones the log left out (§17, Checkpoint chain).
#[derive(Clone, Debug)]
pub struct CheckpointWalk {
    last: (u64, Digest),
    known: std::collections::BTreeSet<Digest>,
    floor: u64,
    generation: u64,
}

impl CheckpointWalk {
    /// A walk from the checkpoints the client reached, as (number, digest).
    /// An operation naming a checkpoint below `floor` (the retention
    /// floor, or the checkpoint the device bootstrapped from) or written in
    /// another restore generation than `generation` needs no fetch.
    pub fn new(
        reached: impl IntoIterator<Item = (u64, Digest)>,
        floor: u64,
        generation: u64,
    ) -> Self {
        let mut last = (0, Digest::ZERO);
        let mut known = std::collections::BTreeSet::new();
        for (number, digest) in reached {
            last = last.max((number, digest));
            known.insert(digest);
        }
        Self {
            last,
            known,
            floor,
            generation,
        }
    }

    /// The checkpoints the log left out before each of `entries`, by the
    /// index of the entry they go before, oldest first: before a
    /// checkpoint entry whose `previous` the client hasn't reached, and
    /// before an operation that names a checkpoint it hasn't reached. Each
    /// walk follows `previous` back to the last checkpoint reached; one the
    /// server can't serve, or a walk that passes it, reports the index of
    /// the entry with [`WalkGap`].
    pub fn plan(
        &mut self,
        entries: &[WalkEntry],
        fetcher: &mut impl CheckpointFetcher,
    ) -> Result<BTreeMap<usize, Vec<ChainLink>>, (usize, WalkGap)> {
        let mut extra = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            let (named, then) = match *entry {
                WalkEntry::Checkpoint(link) => {
                    if self.known.contains(&link.digest) {
                        continue;
                    }
                    // A `previous` the client reached, but not the last, is
                    // a fork that the checks report; nothing to fetch.
                    if link.previous == self.last.1 || self.known.contains(&link.previous) {
                        self.known.insert(link.digest);
                        self.last = self.last.max((link.number, link.digest));
                        continue;
                    }
                    (link.previous, Some(link))
                }
                WalkEntry::Operation {
                    generation,
                    checkpoint_seq,
                    checkpoint,
                } if generation == self.generation
                    && !checkpoint.is_zero()
                    && checkpoint_seq >= self.floor
                    && !self.known.contains(&checkpoint) =>
                {
                    (checkpoint, None)
                }
                _ => continue,
            };
            let mut walk = Vec::new();
            let mut wanted = named;
            // Each `previous` names the number one below, so a walk ends.
            let mut above = then.map(|l| l.number);
            while wanted != self.last.1 {
                let link = fetcher.checkpoint(&wanted).ok_or((index, WalkGap))?;
                if link.digest != wanted
                    || link.number <= self.last.0
                    || above.is_some_and(|n| link.number + 1 != n)
                {
                    return Err((index, WalkGap));
                }
                above = Some(link.number);
                wanted = link.previous;
                walk.push(link);
            }
            walk.reverse();
            for link in &walk {
                self.known.insert(link.digest);
                self.last = (link.number, link.digest);
            }
            if let Some(link) = then {
                self.known.insert(link.digest);
                self.last = (link.number, link.digest);
            }
            if !walk.is_empty() {
                extra.insert(index, walk);
            }
        }
        Ok(extra)
    }
}
