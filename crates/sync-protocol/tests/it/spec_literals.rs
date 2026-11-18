//! Values that docs/workspace-format/sync-v2.md states literally: the tag
//! table and the device card example of §32, the padding figures of §6 and
//! §13, and the record sizes of §28. They come from the spec text, not from
//! this crate, so they check the encoder independently.

use sync_protocol::codec::{Decode, Encode};
use sync_protocol::ids::device_id;
use sync_protocol::records::Card;
use sync_protocol::tag::Tag;
use sync_protocol::types::{Id, Key};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// §32, "Tag bytes".
const TAG_TABLE: &[(&str, &str)] = &[
    (
        "noura.sync.device.card",
        "166e6f7572612e73796e632e6465766963652e636172640002",
    ),
    (
        "noura.sync.device.enroll",
        "186e6f7572612e73796e632e6465766963652e656e726f6c6c0002",
    ),
    (
        "noura.sync.device.id",
        "146e6f7572612e73796e632e6465766963652e69640002",
    ),
    (
        "noura.sync.account",
        "126e6f7572612e73796e632e6163636f756e740002",
    ),
    (
        "noura.sync.approval.commit",
        "1a6e6f7572612e73796e632e617070726f76616c2e636f6d6d69740002",
    ),
    (
        "noura.sync.approval.offer",
        "196e6f7572612e73796e632e617070726f76616c2e6f666665720002",
    ),
    (
        "noura.sync.approval.code",
        "186e6f7572612e73796e632e617070726f76616c2e636f64650002",
    ),
    (
        "noura.sync.policy",
        "116e6f7572612e73796e632e706f6c6963790002",
    ),
    ("noura.sync.epoch", "106e6f7572612e73796e632e65706f63680002"),
    (
        "noura.sync.epoch.recipients",
        "1b6e6f7572612e73796e632e65706f63682e726563697069656e74730002",
    ),
    (
        "noura.sync.key-check",
        "146e6f7572612e73796e632e6b65792d636865636b0002",
    ),
    (
        "noura.sync.envelope",
        "136e6f7572612e73796e632e656e76656c6f70650002",
    ),
    ("noura.sync.op", "0d6e6f7572612e73796e632e6f700002"),
    (
        "noura.sync.payload",
        "126e6f7572612e73796e632e7061796c6f61640002",
    ),
    (
        "noura.sync.checkpoint",
        "156e6f7572612e73796e632e636865636b706f696e740002",
    ),
    (
        "noura.sync.manifest.leaf",
        "186e6f7572612e73796e632e6d616e69666573742e6c6561660002",
    ),
    (
        "noura.sync.manifest.bucket",
        "1a6e6f7572612e73796e632e6d616e69666573742e6275636b65740002",
    ),
    (
        "noura.sync.manifest.node",
        "186e6f7572612e73796e632e6d616e69666573742e6e6f64650002",
    ),
    (
        "noura.sync.manifest.heads",
        "196e6f7572612e73796e632e6d616e69666573742e68656164730002",
    ),
    ("noura.sync.blob", "0f6e6f7572612e73796e632e626c6f620002"),
    (
        "noura.sync.blob.parts",
        "156e6f7572612e73796e632e626c6f622e70617274730002",
    ),
    (
        "noura.sync.presence",
        "136e6f7572612e73796e632e70726573656e63650002",
    ),
    (
        "noura.sync.presence.payload",
        "1b6e6f7572612e73796e632e70726573656e63652e7061796c6f61640002",
    ),
    (
        "noura.sync.presence.channel",
        "1b6e6f7572612e73796e632e70726573656e63652e6368616e6e656c0002",
    ),
    (
        "noura.sync.profile",
        "126e6f7572612e73796e632e70726f66696c650002",
    ),
    (
        "noura.sync.profile.payload",
        "1a6e6f7572612e73796e632e70726f66696c652e7061796c6f61640002",
    ),
    (
        "noura.sync.invite.token",
        "176e6f7572612e73796e632e696e766974652e746f6b656e0002",
    ),
    (
        "noura.sync.invite.context",
        "196e6f7572612e73796e632e696e766974652e636f6e746578740002",
    ),
    (
        "noura.sync.invite.payload",
        "196e6f7572612e73796e632e696e766974652e7061796c6f61640002",
    ),
    (
        "noura.sync.share.token",
        "166e6f7572612e73796e632e73686172652e746f6b656e0002",
    ),
    ("noura.sync.share", "106e6f7572612e73796e632e73686172650002"),
    (
        "noura.sync.share.payload",
        "186e6f7572612e73796e632e73686172652e7061796c6f61640002",
    ),
    (
        "noura.sync.text.lineage",
        "176e6f7572612e73796e632e746578742e6c696e656167650002",
    ),
    (
        "noura.sync.recovery-kit",
        "176e6f7572612e73796e632e7265636f766572792d6b69740002",
    ),
    (
        "noura.sync.recovery-kit.payload",
        "1f6e6f7572612e73796e632e7265636f766572792d6b69742e7061796c6f61640002",
    ),
    (
        "noura.sync.recovery.proof",
        "196e6f7572612e73796e632e7265636f766572792e70726f6f660002",
    ),
];

#[test]
fn every_tag_matches_the_spec_table() {
    assert_eq!(Tag::ALL.len(), TAG_TABLE.len());
    for (domain, bytes) in TAG_TABLE {
        let tag = Tag::from_domain(domain).unwrap_or_else(|| panic!("no tag for {domain}"));
        assert_eq!(tag.domain(), *domain);
        assert_eq!(hex(&tag.bytes()), *bytes, "{domain}");
    }
}

#[test]
fn the_device_card_example_encodes_as_section_32_shows() {
    let signing_public = Key([0x33; 32]);
    let device = device_id(&signing_public);
    assert_eq!(hex(&device.0), "ab553dd81a57e9291f87d0b55e520c0b");

    let card = Card::new(Id([0x11; 16]), signing_public, vec![0x44; 1216]).unwrap();
    let bytes = card.to_bytes();
    assert_eq!(bytes.len(), 1_309);
    let mut expected = String::new();
    expected.push_str("166e6f7572612e73796e632e6465766963652e636172640002");
    expected.push_str(&"11".repeat(16));
    expected.push_str("ab553dd81a57e9291f87d0b55e520c0b");
    expected.push_str(&"33".repeat(32));
    expected.push_str("000004c0");
    expected.push_str(&"44".repeat(1_216));
    assert_eq!(hex(&bytes), expected);
    assert_eq!(Card::from_bytes(&bytes).unwrap(), card);
}

mod sizes {
    //! Record sizes that §28 states.

    use sync_protocol::codec::{Bytes, Encode, ExactBytes, Nested, Sorted};
    use sync_protocol::crypto::SigningKey;
    use sync_protocol::records::{
        AccountHead, AccountLink, Card, Checkpoint, CheckpointBundle, Cover, Envelope, Epoch,
        EpochBundle, LinkKind, Operation, PolicyChange, PolicyKind, Ref, Role, Signed,
    };
    use sync_protocol::types::{Digest, Id, Key, Nonce, SecretKey};

    fn key() -> SigningKey {
        SigningKey::from_seed(&SecretKey([1; 32]))
    }

    fn id(n: u8) -> Id {
        Id([n; 16])
    }

    fn head(n: u8) -> AccountHead {
        AccountHead {
            account: id(n),
            genesis: Digest([n; 32]),
            seqno: 3,
            head: Digest([n; 32]),
        }
    }

    fn epoch() -> Signed<Epoch> {
        Signed::sign(
            Epoch {
                workspace: id(1),
                domain: id(2),
                epoch: 2,
                previous: Digest([3; 32]),
                policy_revision: 4,
                policy_head: Digest([5; 32]),
                rotating_device: id(6),
                recipients_digest: Digest([7; 32]),
                key_check: Digest([8; 32]),
                lineage_nonce: Nonce([9; 24]),
                lineage: sync_protocol::records::Lineage(vec![10; 48]),
            },
            &key(),
        )
    }

    #[test]
    fn an_envelope_is_1366_bytes() {
        let envelope = Signed::sign(
            Envelope {
                workspace: id(1),
                domain: id(2),
                epoch: 3,
                recipient: id(4),
                recipient_hpke_digest: Digest([5; 32]),
                wrapping_device: id(6),
                enc: ExactBytes::new(vec![7; 1_120]).unwrap(),
                ciphertext: ExactBytes::new(vec![8; 48]).unwrap(),
            },
            &key(),
        );
        assert_eq!(envelope.to_bytes().len(), 1_366);
    }

    #[test]
    fn an_epoch_record_is_351_bytes_and_its_bundle_for_199_recipients_3543() {
        assert_eq!(epoch().to_bytes().len(), 351);
        let recipients = (1..=199u8).map(id).collect();
        let bundle = EpochBundle {
            epoch: Nested(epoch()),
            recipients: Sorted::new(recipients).unwrap(),
        };
        assert_eq!(bundle.to_bytes().len(), 3_543);
    }

    #[test]
    fn a_checkpoint_is_376_bytes_and_its_bundle_with_200_heads_13184() {
        let checkpoint = Signed::sign(
            Checkpoint {
                workspace: id(1),
                domain: id(2),
                previous: Digest([3; 32]),
                number: 4,
                skip: Digest([5; 32]),
                covered_seq: 6,
                floor: 7,
                policy_revision: 8,
                policy_head: Digest([9; 32]),
                live_objects: 10,
                manifest_root: Digest([11; 32]),
                heads_digest: Digest([12; 32]),
                epoch: 13,
                epoch_record: Digest([14; 32]),
                signer: id(15),
            },
            &key(),
        );
        assert_eq!(checkpoint.to_bytes().len(), 376);
        let heads = (1..=200u8)
            .map(|n| Ref {
                device: id(n),
                counter: 1,
                seq: 1,
                op_digest: Digest([n; 32]),
            })
            .collect();
        let bundle = CheckpointBundle {
            checkpoint: Nested(checkpoint),
            heads: Sorted::new(heads).unwrap(),
        };
        assert_eq!(bundle.to_bytes().len(), 13_184);
    }

    fn link(kind: LinkKind) -> usize {
        Signed::sign(
            AccountLink {
                account: id(1),
                seqno: 2,
                previous: Digest([3; 32]),
                signer: id(4),
                kind,
            },
            &key(),
        )
        .to_bytes()
        .len()
    }

    #[test]
    fn a_remove_link_is_178_bytes_and_an_add_device_link_1503() {
        assert_eq!(
            link(LinkKind::Remove {
                device: id(5),
                keep: Sorted::default(),
            }),
            178
        );
        let card = Card::new(id(1), Key([0x33; 32]), vec![0x44; 1_216]).unwrap();
        assert_eq!(
            link(LinkKind::AddDevice {
                card: Nested(card),
                offer: Digest([6; 32]),
            }),
            1_503
        );
    }

    fn change(heads: Vec<AccountHead>, kind: PolicyKind) -> usize {
        Signed::sign(
            PolicyChange {
                workspace: id(1),
                revision: 2,
                previous: Digest([3; 32]),
                signer: id(4),
                heads: Sorted::new(heads).unwrap(),
                kind,
            },
            &key(),
        )
        .to_bytes()
        .len()
    }

    #[test]
    fn an_account_heads_change_is_249_bytes_and_add_member_with_two_heads_370() {
        assert_eq!(change(vec![head(1)], PolicyKind::AccountHeads), 249);
        assert_eq!(
            change(
                vec![head(1), head(2)],
                PolicyKind::AddMember {
                    account: id(2),
                    role: Role::Editor,
                    invitation: id(9),
                }
            ),
            370
        );
    }

    #[test]
    fn an_operation_carries_389_bytes_beside_its_padded_payload() {
        let op = Signed::sign(
            Operation {
                workspace: id(1),
                domain: id(2),
                device: id(3),
                counter: 4,
                previous: Digest([5; 32]),
                epoch: 6,
                policy_revision: 7,
                policy_head: Digest([8; 32]),
                previous_attempt: Digest::ZERO,
                generation: 9,
                checkpoint_seq: 10,
                checkpoint: Digest([11; 32]),
                rid: id(12),
                delete: false,
                supersedes: Sorted::new(vec![Cover {
                    device: id(3),
                    counter: 2,
                }])
                .unwrap(),
                blobs: Sorted::default(),
                nonce: Nonce([13; 24]),
                ciphertext: Bytes(vec![0; 3_328 + 16]),
            },
            &key(),
        );
        // §28: 389 bytes of fields, AEAD tag and signature, and a 3,328-byte
        // padded payload, 3,717 in all.
        assert_eq!(op.to_bytes().len(), 3_717);
    }
}
