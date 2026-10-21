//! Account chain verification (§7) from the links alone.

mod support;

use std::collections::BTreeSet;

use support::{ChainBuilder, Device, account, device, remove_kind};
use sync_protocol::account::{AccountChain, AccountError, KeptHead};
use sync_protocol::codec::Nested;
use sync_protocol::records::{Card, LinkKind};
use sync_protocol::types::{Digest, Id};

fn none() -> BTreeSet<Id> {
    BTreeSet::new()
}

fn verify(chain: &ChainBuilder) -> Result<AccountChain, AccountError> {
    AccountChain::verify(&chain.links, &none())
}

fn active(chain: &AccountChain) -> Vec<Id> {
    let mut ids: Vec<Id> = chain
        .active_at(chain.head_seqno())
        .iter()
        .map(|s| s.card.device)
        .collect();
    ids.sort();
    ids
}

fn ids(devices: &[&Device]) -> Vec<Id> {
    let mut ids: Vec<Id> = devices.iter().map(|d| d.id()).collect();
    ids.sort();
    ids
}

struct Family {
    laptop: Device,
    phone: Device,
    tablet: Device,
    watch: Device,
}

fn family() -> Family {
    let a = account("alice");
    Family {
        laptop: device(a, "alice laptop"),
        phone: device(a, "alice phone"),
        tablet: device(a, "alice tablet"),
        watch: device(a, "alice watch"),
    }
}

#[test]
fn the_fingerprint_is_the_genesis_digest() {
    let f = family();
    let chain = ChainBuilder::new(&f.laptop);
    let verified = verify(&chain).unwrap();
    assert_eq!(verified.genesis(), chain.links[0].digest());
    assert_eq!(active(&verified), ids(&[&f.laptop]));
}

#[test]
fn seqno_and_previous_must_link() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain.add_device(&f.laptop, &f.phone);
    let mut skipped = chain.links.clone();
    skipped[1].body.seqno = 3;
    skipped[1] = sync_protocol::records::Signed::sign(skipped[1].body.clone(), &f.laptop.key);
    assert_eq!(
        AccountChain::verify(&skipped, &none()),
        Err(AccountError::Seqno { seqno: 3 })
    );
    let mut unlinked = chain.links.clone();
    unlinked[1].body.previous = Digest([1; 32]);
    unlinked[1] = sync_protocol::records::Signed::sign(unlinked[1].body.clone(), &f.laptop.key);
    assert_eq!(
        AccountChain::verify(&unlinked, &none()),
        Err(AccountError::Previous { seqno: 2 })
    );
}

#[test]
fn a_genesis_is_signed_by_its_own_card() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    let mut body = chain.links[0].body.clone();
    body.signer = f.phone.id();
    chain.links[0] = sync_protocol::records::Signed::sign(body, &f.phone.key);
    assert_eq!(
        verify(&chain),
        Err(AccountError::GenesisSigner { seqno: 1 })
    );
}

#[test]
fn a_bad_signature_is_refused() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain.add_device(&f.laptop, &f.phone);
    chain.links[1].sig.0[0] ^= 1;
    assert_eq!(verify(&chain), Err(AccountError::Signature { seqno: 2 }));
}

#[test]
fn a_removed_card_signs_nothing() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .remove(&f.laptop, &f.phone, &[])
        .add_device(&f.phone, &f.tablet);
    assert_eq!(
        verify(&chain),
        Err(AccountError::SignerNotActive { seqno: 4 })
    );
}

#[test]
fn a_card_naming_another_account_is_refused() {
    let f = family();
    let stranger = device(account("mallory"), "mallory laptop");
    let mut chain = ChainBuilder::new(&f.laptop);
    chain.add_device(&f.laptop, &stranger);
    assert_eq!(verify(&chain), Err(AccountError::CardAccount { seqno: 2 }));
}

#[test]
fn a_device_id_listed_before_never_returns() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .remove(&f.laptop, &f.phone, &[])
        .add_device(&f.laptop, &f.phone);
    assert_eq!(verify(&chain), Err(AccountError::DeviceListed { seqno: 4 }));
    // A device ID that another chain of the account listed, as before a
    // reset.
    let mut fresh = ChainBuilder::new(&f.laptop);
    fresh.add_device(&f.laptop, &f.phone);
    let listed = BTreeSet::from([f.phone.id()]);
    assert_eq!(
        AccountChain::verify(&fresh.links, &listed),
        Err(AccountError::DeviceListed { seqno: 2 })
    );
}

#[test]
fn a_recovery_identity_signs_one_recover_device_and_nothing_else() {
    let f = family();
    let kit = device(f.laptop.card.account, "alice kit");
    let mut chain = ChainBuilder::new(&f.laptop);
    chain.add_recovery(&f.laptop, &kit).recover(&kit, &f.phone);
    let verified = verify(&chain).unwrap();
    assert_eq!(verified.card(&f.phone.id()).unwrap().parent, Some(kit.id()));

    let mut twice = ChainBuilder::new(&f.laptop);
    twice
        .add_recovery(&f.laptop, &kit)
        .recover(&kit, &f.phone)
        .recover(&kit, &f.tablet);
    assert_eq!(verify(&twice), Err(AccountError::RecoveryUsed { seqno: 4 }));

    let mut adds = ChainBuilder::new(&f.laptop);
    adds.add_recovery(&f.laptop, &kit)
        .add_device(&kit, &f.phone);
    assert_eq!(verify(&adds), Err(AccountError::RecoverySigns { seqno: 3 }));

    let mut removes = ChainBuilder::new(&f.laptop);
    removes
        .add_recovery(&f.laptop, &kit)
        .remove(&kit, &f.laptop, &[]);
    assert_eq!(
        verify(&removes),
        Err(AccountError::RecoverySigns { seqno: 3 })
    );

    let mut by_device = ChainBuilder::new(&f.laptop);
    by_device.recover(&f.laptop, &f.phone);
    assert_eq!(
        verify(&by_device),
        Err(AccountError::RecoveryNotSigner { seqno: 2 })
    );
}

#[test]
fn only_one_recovery_identity_is_active() {
    let f = family();
    let a = f.laptop.card.account;
    let (kit, kit2) = (device(a, "alice kit"), device(a, "alice kit 2"));
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_recovery(&f.laptop, &kit)
        .add_recovery(&f.laptop, &kit2);
    assert_eq!(
        verify(&chain),
        Err(AccountError::SecondRecovery { seqno: 3 })
    );
    // After removing the first, a second identity is fine.
    let mut replaced = ChainBuilder::new(&f.laptop);
    replaced
        .add_recovery(&f.laptop, &kit)
        .remove(&f.laptop, &kit, &[])
        .add_recovery(&f.laptop, &kit2);
    assert!(verify(&replaced).is_ok());
}

#[test]
fn at_most_256_cards_are_active() {
    let f = family();
    let a = f.laptop.card.account;
    let mut chain = ChainBuilder::new(&f.laptop);
    for i in 0..255 {
        chain.add_device(&f.laptop, &device(a, &format!("alice device {i}")));
    }
    assert_eq!(verify(&chain).unwrap().active_at(256).len(), 256);
    chain.add_device(&f.laptop, &device(a, "alice device 256"));
    assert_eq!(
        verify(&chain),
        Err(AccountError::TooManyCards { seqno: 257 })
    );
}

#[test]
fn a_removal_cascades_two_levels_deep() {
    // laptop adds phone, phone adds tablet, tablet adds watch.
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .add_device(&f.phone, &f.tablet)
        .add_device(&f.tablet, &f.watch)
        .remove(&f.laptop, &f.phone, &[]);
    assert_eq!(active(&verify(&chain).unwrap()), ids(&[&f.laptop]));
}

#[test]
fn keep_spares_a_device_and_its_cards() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .add_device(&f.phone, &f.tablet)
        .add_device(&f.tablet, &f.watch)
        .remove(&f.laptop, &f.phone, &[&f.tablet]);
    assert_eq!(
        active(&verify(&chain).unwrap()),
        ids(&[&f.laptop, &f.tablet, &f.watch])
    );
}

#[test]
fn keep_never_names_a_recovery_identity_or_a_card_the_link_keeps_anyway() {
    let f = family();
    let kit = device(f.laptop.card.account, "alice kit");
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .add_recovery(&f.phone, &kit)
        .remove(&f.laptop, &f.phone, &[&kit]);
    assert_eq!(verify(&chain), Err(AccountError::Keep { seqno: 4 }));

    let mut stray = ChainBuilder::new(&f.laptop);
    stray
        .add_device(&f.laptop, &f.phone)
        .add_device(&f.laptop, &f.tablet)
        .remove(&f.laptop, &f.phone, &[&f.tablet]);
    assert_eq!(verify(&stray), Err(AccountError::Keep { seqno: 4 }));
}

#[test]
fn a_signer_inside_the_removed_subtree_stays_with_its_cards() {
    // laptop adds phone, phone adds tablet, tablet adds watch; tablet
    // removes phone: tablet stays, so watch's parent stays.
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .add_device(&f.phone, &f.tablet)
        .add_device(&f.tablet, &f.watch)
        .remove(&f.tablet, &f.phone, &[]);
    assert_eq!(
        active(&verify(&chain).unwrap()),
        ids(&[&f.laptop, &f.tablet, &f.watch])
    );
}

#[test]
fn a_self_removal_leaves_the_devices_cards_active() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .add_device(&f.phone, &f.tablet)
        .remove(&f.phone, &f.phone, &[]);
    assert_eq!(
        active(&verify(&chain).unwrap()),
        ids(&[&f.laptop, &f.tablet])
    );
}

#[test]
fn replacing_a_kit_takes_down_the_devices_it_recovered_unless_kept() {
    let f = family();
    let a = f.laptop.card.account;
    let (kit, kit2) = (device(a, "alice kit"), device(a, "alice kit 2"));
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_recovery(&f.laptop, &kit)
        .recover(&kit, &f.phone)
        .remove(&f.laptop, &kit, &[])
        .add_recovery(&f.laptop, &kit2);
    assert_eq!(active(&verify(&chain).unwrap()), ids(&[&f.laptop, &kit2]));

    let mut kept = ChainBuilder::new(&f.laptop);
    kept.add_recovery(&f.laptop, &kit)
        .recover(&kit, &f.phone)
        .remove(&f.laptop, &kit, &[&f.phone]);
    assert_eq!(active(&verify(&kept).unwrap()), ids(&[&f.laptop, &f.phone]));
}

#[test]
fn a_recovery_removes_every_other_card() {
    // The laptop is lost. The kit recovers the phone, which removes the
    // genesis card, the only active card without an active parent: the
    // cascade takes down the tablet and the kit too, but not the signer.
    let f = family();
    let kit = device(f.laptop.card.account, "alice kit");
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.tablet)
        .add_recovery(&f.laptop, &kit)
        .recover(&kit, &f.phone)
        .remove(&f.phone, &f.laptop, &[]);
    assert_eq!(active(&verify(&chain).unwrap()), ids(&[&f.phone]));
    // A second remove of the kit finds it gone.
    chain.push(&f.phone, remove_kind(&kit, &[]));
    assert_eq!(verify(&chain), Err(AccountError::RemoveTarget { seqno: 6 }));
}

#[test]
fn active_cards_answer_for_any_head() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .remove(&f.laptop, &f.phone, &[]);
    let verified = verify(&chain).unwrap();
    assert!(verified.is_active_at(&f.phone.id(), 2));
    assert!(!verified.is_active_at(&f.phone.id(), 3));
    assert!(!verified.is_active_at(&f.phone.id(), 1));
}

#[test]
fn a_kept_head_must_be_extended() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .add_device(&f.laptop, &f.tablet);
    let verified = verify(&chain).unwrap();
    let kept = KeptHead::from(verified.head_at(2).unwrap());
    assert!(kept.check(&verified).is_ok());

    // A fork at seqno 2.
    let mut fork = ChainBuilder::new(&f.laptop);
    fork.add_device(&f.laptop, &f.watch)
        .add_device(&f.laptop, &f.tablet);
    assert!(kept.check(&verify(&fork).unwrap()).is_err());

    // A chain cut back below the kept head.
    let mut short = ChainBuilder::new(&f.laptop);
    short.links.truncate(1);
    assert!(kept.check(&verify(&short).unwrap()).is_err());

    // A head recorded off the kept chain.
    let off = verify(&fork).unwrap().head_at(3).unwrap();
    assert!(kept.check_recorded(&verified, &off).is_err());
    assert!(kept.check_recorded(&verified, &verified.head()).is_ok());
}

#[test]
fn a_remove_of_an_inactive_card_is_refused() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain
        .add_device(&f.laptop, &f.phone)
        .remove(&f.laptop, &f.phone, &[])
        .push(&f.laptop, remove_kind(&f.phone, &[]));
    assert_eq!(verify(&chain), Err(AccountError::RemoveTarget { seqno: 4 }));
}

#[test]
fn a_second_genesis_is_refused() {
    let f = family();
    let mut chain = ChainBuilder::new(&f.laptop);
    chain.push(
        &f.laptop,
        LinkKind::Genesis {
            card: Nested(f.phone.card.clone()),
        },
    );
    assert_eq!(
        verify(&chain),
        Err(AccountError::GenesisPosition { seqno: 2 })
    );
    let _: &Card = &f.phone.card;
}
