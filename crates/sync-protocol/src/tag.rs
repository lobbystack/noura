//! Record and tuple tags (§4, "Tags"; §32, "Tag bytes").
//!
//! A tag is one byte holding the length of its domain string, the ASCII domain
//! string, then the protocol version as a big-endian `u16`.

use crate::PROTOCOL_VERSION;

macro_rules! tags {
    ($($(#[doc = $doc:literal])* $name:ident => $domain:literal,)*) => {
        /// Every tag that version 2 defines.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Tag {
            $($(#[doc = $doc])* $name,)*
        }

        impl Tag {
            /// Every tag, in the order of the table in §32.
            pub const ALL: &'static [Tag] = &[$(Tag::$name,)*];

            /// The tag's ASCII domain string.
            pub const fn domain(self) -> &'static str {
                match self {
                    $(Tag::$name => $domain,)*
                }
            }
        }
    };
}

tags! {
    /// A device card (§7).
    DeviceCard => "noura.sync.device.card",
    /// An enrollment proof (§7).
    DeviceEnroll => "noura.sync.device.enroll",
    /// The input of a device ID's derivation (§5).
    DeviceId => "noura.sync.device.id",
    /// An account chain link (§7).
    Account => "noura.sync.account",
    /// An approval commitment (§10).
    ApprovalCommit => "noura.sync.approval.commit",
    /// An approval offer (§10).
    ApprovalOffer => "noura.sync.approval.offer",
    /// The input of an approval code (§10).
    ApprovalCode => "noura.sync.approval.code",
    /// A policy change (§8).
    Policy => "noura.sync.policy",
    /// An epoch record (§9).
    Epoch => "noura.sync.epoch",
    /// The input of an epoch's `recipients_digest` (§9).
    EpochRecipients => "noura.sync.epoch.recipients",
    /// The input of an epoch's `key_check` (§9).
    KeyCheck => "noura.sync.key-check",
    /// A key envelope (§9).
    Envelope => "noura.sync.envelope",
    /// An operation (§13).
    Op => "noura.sync.op",
    /// An operation payload (§13).
    Payload => "noura.sync.payload",
    /// A checkpoint (§17).
    Checkpoint => "noura.sync.checkpoint",
    /// A manifest leaf (§17).
    ManifestLeaf => "noura.sync.manifest.leaf",
    /// A manifest bucket (§17).
    ManifestBucket => "noura.sync.manifest.bucket",
    /// A manifest node (§17).
    ManifestNode => "noura.sync.manifest.node",
    /// A checkpoint's `heads_digest` input (§17).
    ManifestHeads => "noura.sync.manifest.heads",
    /// A blob header (§21).
    Blob => "noura.sync.blob",
    /// A blob's `ciphertext_digest` input (§21).
    BlobParts => "noura.sync.blob.parts",
    /// A presence record (§23).
    Presence => "noura.sync.presence",
    /// A presence payload (§23).
    PresencePayload => "noura.sync.presence.payload",
    /// The input of a presence channel (§23).
    PresenceChannel => "noura.sync.presence.channel",
    /// A workspace profile (§25).
    Profile => "noura.sync.profile",
    /// A workspace profile payload (§25).
    ProfilePayload => "noura.sync.profile.payload",
    /// The input of an invitation token's hash (§11).
    InviteToken => "noura.sync.invite.token",
    /// An invitation context (§11).
    InviteContext => "noura.sync.invite.context",
    /// An invitation context's payload (§11).
    InvitePayload => "noura.sync.invite.payload",
    /// The input of a share token's hash (§24).
    ShareToken => "noura.sync.share.token",
    /// A share snapshot (§24).
    Share => "noura.sync.share",
    /// A share snapshot's payload (§24).
    SharePayload => "noura.sync.share.payload",
    /// The input of a text lineage ID (§22).
    TextLineage => "noura.sync.text.lineage",
    /// A recovery kit (§12).
    RecoveryKit => "noura.sync.recovery-kit",
    /// A recovery kit's payload (§12).
    RecoveryKitPayload => "noura.sync.recovery-kit.payload",
    /// A recovery proof (§12).
    RecoveryProof => "noura.sync.recovery.proof",
}

impl Tag {
    /// The tag whose domain string is `domain`, if version 2 defines one.
    pub fn from_domain(domain: &str) -> Option<Tag> {
        Tag::ALL.iter().copied().find(|tag| tag.domain() == domain)
    }

    /// The tag's encoded bytes: length, domain string, version.
    pub fn bytes(self) -> Vec<u8> {
        let domain = self.domain().as_bytes();
        let mut out = Vec::with_capacity(domain.len() + 3);
        out.push(domain.len() as u8);
        out.extend_from_slice(domain);
        out.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        out
    }

    /// The tag's encoded length.
    pub const fn encoded_len(self) -> usize {
        self.domain().len() + 3
    }
}
