//! The decoding rules of §4, each against a record built by hand.

use sync_protocol::codec::{Decode, DecodeErrorKind, Encode};
use sync_protocol::crypto::SigningKey;
use sync_protocol::records::{
    Access, AccessList, AccountHead, Card, Level, Meta, PolicyChange, PolicyKind, Signed,
};
use sync_protocol::types::{Digest, Id, Key, SecretKey};

fn card() -> Card {
    Card::new(Id([0x11; 16]), Key([0x33; 32]), vec![0x44; 1_216]).unwrap()
}

fn kind(bytes: &[u8]) -> DecodeErrorKind {
    Card::from_bytes(bytes).unwrap_err().kind
}

fn policy_change(kind: PolicyKind) -> Vec<u8> {
    let signer = SigningKey::from_seed(&SecretKey([7; 32]));
    let head = AccountHead {
        account: Id([1; 16]),
        genesis: Digest([2; 32]),
        seqno: 1,
        head: Digest([2; 32]),
    };
    let body = PolicyChange {
        workspace: Id([9; 16]),
        revision: 2,
        previous: Digest([3; 32]),
        signer: Id([4; 16]),
        heads: sync_protocol::codec::Sorted::new(vec![head]).unwrap(),
        kind,
    };
    Signed::sign(body, &signer).to_bytes()
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = card().to_bytes();
    bytes.push(0);
    assert_eq!(kind(&bytes), DecodeErrorKind::TrailingBytes);
}

#[test]
fn every_truncation_is_refused() {
    let bytes = card().to_bytes();
    for len in 0..bytes.len() {
        assert_eq!(kind(&bytes[..len]), DecodeErrorKind::Truncated, "{len}");
    }
}

#[test]
fn a_fixed_length_bytes_field_at_another_length_is_refused() {
    let mut bytes = card().to_bytes();
    // hpke_public's length prefix sits after the tag, two IDs and the key.
    let at = 25 + 16 + 16 + 32;
    bytes[at..at + 4].copy_from_slice(&1_215u32.to_be_bytes());
    assert_eq!(kind(&bytes), DecodeErrorKind::WrongLength);
}

#[test]
fn a_length_above_the_limit_is_refused_before_allocating() {
    // A str field claiming 65,535 bytes in a short body: refused by its
    // 4,096-byte limit, not by reading.
    let mut bytes = sync_protocol::tag::Tag::DeviceEnroll.bytes();
    bytes.extend_from_slice(&u16::MAX.to_be_bytes());
    let err = sync_protocol::records::Enroll::from_bytes(&bytes).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::TooLong);
    // A list count of 2^32 − 1 in a short body.
    let mut policy = policy_change(PolicyKind::AccountHeads);
    let heads_at = 20 + 16 + 8 + 32 + 16;
    policy[heads_at..heads_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    let err = Signed::<PolicyChange>::from_bytes(&policy).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::Truncated);
}

#[test]
fn tags_are_checked_for_domain_version_and_position() {
    let bytes = card().to_bytes();
    let mut unknown = bytes.clone();
    unknown[5] = b'X';
    assert_eq!(kind(&unknown), DecodeErrorKind::UnknownTag);
    let mut version = bytes.clone();
    version[24] = 3;
    assert_eq!(kind(&version), DecodeErrorKind::UnsupportedVersion);
    // A valid tag of another record in a card's position.
    let mut other = sync_protocol::tag::Tag::Account.bytes();
    other.extend_from_slice(&bytes[25..]);
    assert_eq!(kind(&other), DecodeErrorKind::WrongTag);
    // A v1 JSON tuple starts with `[`.
    assert_eq!(kind(b"[\"noura\"]"), DecodeErrorKind::UnknownTag);
}

#[test]
fn unknown_enum_values_and_option_discriminators_are_refused() {
    let mut bytes = policy_change(PolicyKind::AccountHeads);
    let kind_at = bytes.len() - 64 - 1;
    bytes[kind_at] = 11;
    let err = Signed::<PolicyChange>::from_bytes(&bytes).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnknownValue);

    let mut set_role = policy_change(PolicyKind::SetRole {
        account: Id([5; 16]),
        role: sync_protocol::records::Role::Editor,
    });
    let role_at = set_role.len() - 64 - 1;
    set_role[role_at] = 5;
    let err = Signed::<PolicyChange>::from_bytes(&set_role).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::UnknownValue);

    let meta = Meta {
        key: sync_protocol::codec::Text::new("path").unwrap(),
        seq: 0,
        value: None,
    };
    let mut meta_bytes = meta.to_bytes();
    let opt_at = meta_bytes.len() - 1;
    meta_bytes[opt_at] = 2;
    assert_eq!(
        Meta::from_bytes(&meta_bytes).unwrap_err().kind,
        DecodeErrorKind::BadOption
    );
}

#[test]
fn an_access_list_naming_one_account_twice_is_refused() {
    let entry = |level| Access {
        account: Id([6; 16]),
        level,
    };
    assert_eq!(
        AccessList::new(vec![entry(Level::Read), entry(Level::Manage)]).unwrap_err(),
        DecodeErrorKind::NotSorted
    );
    let unsorted = AccessList::unchecked(vec![entry(Level::Read), entry(Level::Manage)]);
    let bytes = policy_change(PolicyKind::SetAccess {
        domain: Id([8; 16]),
        access: unsorted,
    });
    let err = Signed::<PolicyChange>::from_bytes(&bytes).unwrap_err();
    assert_eq!(err.kind, DecodeErrorKind::NotSorted);
}

#[test]
fn an_all_zero_id_is_refused() {
    let mut bytes = card().to_bytes();
    bytes[25..41].fill(0);
    assert_eq!(kind(&bytes), DecodeErrorKind::ZeroId);
}

#[test]
fn invalid_utf8_is_refused() {
    let meta = Meta {
        key: sync_protocol::codec::Text::new("ab").unwrap(),
        seq: 0,
        value: None,
    };
    let mut bytes = meta.to_bytes();
    bytes[2] = 0xff;
    assert_eq!(
        Meta::from_bytes(&bytes).unwrap_err().kind,
        DecodeErrorKind::InvalidUtf8
    );
}

#[test]
fn a_card_whose_device_id_doesnt_derive_is_refused() {
    let mut bytes = card().to_bytes();
    bytes[41] ^= 1;
    assert_eq!(kind(&bytes), DecodeErrorKind::DeviceIdMismatch);
}
