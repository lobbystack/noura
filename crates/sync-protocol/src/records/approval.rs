//! Approval commitments, offers and codes (§10).

use super::{AccountHead, ByWorkspace, Card, CheckpointAnchor};
use crate::codec::{Nested, SortSpec, Sorted, Writer};
use crate::crypto::hash_parts;
use crate::tag::Tag;
use crate::types::{Digest, Id, Key};

crate::wire_struct! {
    /// `anchor`: the approver's accepted policy head in one workspace, the
    /// restore generation in which it accepted the checkpoint that follows,
    /// and its latest accepted checkpoint of the workspace domain.
    pub struct Anchor {
        /// The workspace.
        pub workspace: Id,
        /// The accepted policy head revision.
        pub revision: u64,
        /// The accepted policy head digest.
        pub policy_head: Digest,
        /// The restore generation of the checkpoint anchor.
        pub generation: u64,
        /// The latest accepted checkpoint of the workspace domain.
        pub checkpoint: CheckpointAnchor,
    }
}

impl SortSpec<Anchor> for ByWorkspace {
    fn key(item: &Anchor) -> Vec<u8> {
        item.workspace.0.to_vec()
    }
}

crate::wire_struct! {
    /// `offer`, signed by the approving device.
    pub struct Offer: tag ApprovalOffer {
        /// The approving device's card.
        pub approver_card: Nested<Card>,
        /// `BLAKE3(candidate card)`.
        pub candidate_card_digest: Digest,
        /// The request's `commit`.
        pub candidate_commit: Digest,
        /// The candidate's account head, copied from the request.
        pub candidate_head: Option<AccountHead>,
        /// The approver's account chain head.
        pub approver_head: AccountHead,
        /// 32 random bytes the approver chose after it received the request.
        pub nonce: Key,
        /// The workspaces the approval covers, sorted by workspace.
        pub anchors: Sorted<Anchor, ByWorkspace>,
    }
}

/// `commit = BLAKE3(tag(noura.sync.approval.commit) || candidate_card_digest
/// || opt<account_head> candidate_head || nc)`.
pub fn approval_commit(
    candidate_card_digest: &Digest,
    candidate_head: Option<&AccountHead>,
    nc: &Key,
) -> Digest {
    let mut w = Writer::new();
    w.tag(Tag::ApprovalCommit);
    w.put(candidate_card_digest);
    w.put(&candidate_head.cloned());
    w.put(nc);
    hash_parts(&[w.as_bytes()])
}

/// The approval code: the first 8 bytes of
/// `BLAKE3(tag(noura.sync.approval.code) || offer_digest || nc)` as a
/// big-endian `u64`, modulo 100,000,000.
pub fn approval_code(offer_digest: &Digest, nc: &Key) -> u32 {
    let digest = hash_parts(&[&Tag::ApprovalCode.bytes(), &offer_digest.0, &nc.0]);
    let value = u64::from_be_bytes(digest.0[..8].try_into().expect("8 bytes"));
    (value % 100_000_000) as u32
}

/// The code as both screens show it: 8 decimal digits, with leading zeros,
/// in two groups of 4.
pub fn approval_code_text(code: u32) -> String {
    let digits = format!("{code:08}");
    format!("{} {}", &digits[..4], &digits[4..])
}
