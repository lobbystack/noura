//! The identifiers of §5 that derive instead of being drawn: device IDs,
//! text lineage IDs and account IDs, plus the server secret's key check.
//! Random IDs come from [`Id::random`](crate::types::Id::random).

use crate::crypto::{context, derive_key, hash_parts, keyed_hash};
use crate::tag::Tag;
use crate::types::{Digest, Id, Key, SecretKey};

fn first_16(digest: &Digest) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest.0[..16]);
    out
}

/// A device ID: the first 16 bytes of
/// `BLAKE3(tag(noura.sync.device.id) || signing_public)`, so no other key
/// pair can claim it.
pub fn device_id(signing_public: &Key) -> Id {
    Id(first_16(&hash_parts(&[
        &Tag::DeviceId.bytes(),
        &signing_public.0,
    ])))
}

/// A text lineage ID (§22): the first 16 bytes of
/// `BLAKE3(tag(noura.sync.text.lineage) || rid || base || document)`, where
/// `base` is the base operation's digest and `document` the first document's
/// encoded bytes.
pub fn text_lineage_id(rid: &Id, base: &Digest, document: &[u8]) -> Id {
    Id(first_16(&hash_parts(&[
        &Tag::TextLineage.bytes(),
        &rid.0,
        &base.0,
        document,
    ])))
}

/// The key check the server stores for its secret (§14):
/// `derive_key("noura.sync.v2 secret check", secret)`.
pub fn secret_check(server_secret: &SecretKey) -> SecretKey {
    derive_key(context::SECRET_CHECK, server_secret.expose())
}

/// The address an account ID derives from: the sign-up email with ASCII
/// letters lowercased and every other byte kept, so no Unicode update ever
/// changes an ID.
pub fn normalize_email(email: &str) -> String {
    email.to_ascii_lowercase()
}

/// An account's ID (§14): the first 16 bytes of
/// `BLAKE3.keyed_hash(derive_key("noura.sync.v2 account id", secret), email)`
/// over the normalized sign-up email. The result is never all zero in
/// practice; a zero result would be refused like any reserved ID.
pub fn account_id(server_secret: &SecretKey, email: &str) -> Id {
    let key = derive_key(context::ACCOUNT_ID, server_secret.expose());
    Id(first_16(&keyed_hash(
        &key,
        normalize_email(email).as_bytes(),
    )))
}
