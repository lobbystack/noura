//! Identifiers (§5): random remote IDs and the IDs that derive.

use std::collections::BTreeSet;

use sync_protocol::ids::{account_id, normalize_email};
use sync_protocol::types::{Id, SecretKey};

#[test]
fn remote_ids_are_random_nonzero_and_distinct() {
    let ids: BTreeSet<Id> = (0..1_000).map(|_| Id::random().unwrap()).collect();
    assert_eq!(ids.len(), 1_000);
    assert!(ids.iter().all(|id| !id.is_zero()));
    // No byte position is constant across the sample, as a timestamp,
    // version or type field would be.
    for position in 0..16 {
        let values: BTreeSet<u8> = ids.iter().map(|id| id.0[position]).collect();
        assert!(values.len() > 1, "byte {position} never varies");
    }
}

#[test]
fn an_account_id_ignores_ascii_case_only() {
    let secret = SecretKey([0x5e; 32]);
    assert_eq!(
        account_id(&secret, "Alice.Example@Example.COM"),
        account_id(&secret, "alice.example@example.com")
    );
    assert_ne!(
        account_id(&secret, "alice@example.com"),
        account_id(&SecretKey([0x5f; 32]), "alice@example.com")
    );
    // Non-ASCII letters keep their case, so a Unicode update never changes
    // an ID.
    assert_eq!(normalize_email("ÉLISE@Example.com"), "Élise@example.com");
    assert_ne!(
        account_id(&secret, "élise@example.com"),
        account_id(&secret, "Élise@example.com")
    );
}
