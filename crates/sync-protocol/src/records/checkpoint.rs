//! Checkpoints and checkpoint bundles (§17).

use serde::{Deserialize, Serialize};

use super::Signed;
use super::operation::ByDevice;
use crate::codec::{Decode, DecodeError, Encode, Nested, Reader, SortSpec, Sorted, Writer};
use crate::tag::Tag;
use crate::types::{Digest, Id};

crate::wire_struct! {
    /// `ref`: one operation's chain position, the sequence the server
    /// assigned it and its digest. Leaves hold the live operations' refs;
    /// checkpoint heads hold each device's chain head.
    pub struct Ref {
        /// The device.
        pub device: Id,
        /// The chain counter.
        pub counter: u64,
        /// The log sequence.
        pub seq: u64,
        /// The operation's digest.
        pub op_digest: Digest,
    }
}

impl SortSpec<Ref> for ByDevice {
    fn key(item: &Ref) -> Vec<u8> {
        item.device.0.to_vec()
    }
}

/// Sorts leaf refs by device, then counter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByDeviceCounter;

impl SortSpec<Ref> for ByDeviceCounter {
    fn key(item: &Ref) -> Vec<u8> {
        let mut key = item.device.0.to_vec();
        key.extend_from_slice(&item.counter.to_be_bytes());
        key
    }
}

/// The signed body of a checkpoint (§17).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The workspace.
    pub workspace: Id,
    /// The key domain.
    pub domain: Id,
    /// The digest of the domain's previous checkpoint, or zero.
    pub previous: Digest,
    /// 1 for the domain's first checkpoint, then +1.
    pub number: u64,
    /// The digest of checkpoint `number & (number − 1)`, or zero when that
    /// is 0.
    pub skip: Digest,
    /// The manifest covers every record through this sequence.
    pub covered_seq: u64,
    /// The retention floor as the signer read it.
    pub floor: u64,
    /// The revision of the latest policy entry at or below `covered_seq`.
    pub policy_revision: u64,
    /// The digest of the change at `policy_revision`.
    pub policy_head: Digest,
    /// The number of leaves.
    pub live_objects: u64,
    /// The manifest root.
    pub manifest_root: Digest,
    /// `BLAKE3(tag(noura.sync.manifest.heads) || list<ref>)` over the heads.
    pub heads_digest: Digest,
    /// The epoch of the latest epoch entry at or below `covered_seq`.
    pub epoch: u64,
    /// The digest of that epoch record.
    pub epoch_record: Digest,
    /// The signing device.
    pub signer: Id,
}

impl Encode for Checkpoint {
    fn encode(&self, w: &mut Writer) {
        w.tag(Tag::Checkpoint);
        w.put(&self.workspace);
        w.put(&self.domain);
        w.put(&self.previous);
        w.put(&self.number);
        w.put(&self.skip);
        w.put(&self.covered_seq);
        w.put(&self.floor);
        w.put(&self.policy_revision);
        w.put(&self.policy_head);
        w.put(&self.live_objects);
        w.put(&self.manifest_root);
        w.put(&self.heads_digest);
        w.put(&self.epoch);
        w.put(&self.epoch_record);
        w.put(&self.signer);
    }
}

impl Decode for Checkpoint {
    const MIN_SIZE: usize =
        Tag::Checkpoint.encoded_len() + 16 + 16 + 32 + 8 + 32 + 8 * 4 + 32 * 3 + 8 + 32 + 16;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Checkpoint)?;
        let workspace = r.get()?;
        let domain = r.get()?;
        let previous = r.get()?;
        let number_offset = r.offset();
        let number: u64 = r.get()?;
        if number == 0 {
            return Err(r.error_at(crate::codec::DecodeErrorKind::OutOfRange, number_offset));
        }
        Ok(Self {
            workspace,
            domain,
            previous,
            number,
            skip: r.get()?,
            covered_seq: r.get()?,
            floor: r.get()?,
            policy_revision: r.get()?,
            policy_head: r.get()?,
            live_objects: r.get()?,
            manifest_root: r.get()?,
            heads_digest: r.get()?,
            epoch: r.get()?,
            epoch_record: r.get()?,
            signer: r.get()?,
        })
    }
}

/// The chain heads a checkpoint lists, sorted by device.
pub type Heads = Sorted<Ref, ByDevice>;

crate::wire_struct! {
    /// `checkpoint_bundle`: a signed checkpoint and the chain heads its
    /// `heads_digest` covers.
    pub struct CheckpointBundle {
        /// The signed checkpoint.
        pub checkpoint: Nested<Signed<Checkpoint>>,
        /// The chain head of every device with an accepted record.
        pub heads: Heads,
    }
}
