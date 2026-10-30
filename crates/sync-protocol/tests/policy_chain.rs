//! Policy verification (§8): chain links, account heads and authority.

mod support;

use support::{ChainBuilder, Device, Person, PolicyBuilder, account, chains, device};
use sync_protocol::codec::Sorted;
use sync_protocol::policy::{
    AccountChains, MAX_CARD_INDEX, PolicyChain, PolicyError, next_card_index,
};
use sync_protocol::records::{Access, Level, PolicyKind, Role};
use sync_protocol::types::{Digest, Id};

const WORKSPACE: Id = Id([0x77; 16]);
const WS_DOMAIN: Id = Id([0x78; 16]);
const RESTRICTED: Id = Id([0x79; 16]);
const MARKER: Id = Id([0x7a; 16]);
const INVITATION: Id = Id([0x7b; 16]);

fn genesis(owner: &Person) -> PolicyBuilder {
    let mut policy = PolicyBuilder::new(WORKSPACE);
    policy.push(
        owner.first(),
        vec![owner.chain.head()],
        PolicyKind::Genesis {
            workspace_domain: WS_DOMAIN,
        },
    );
    policy
}

fn add_member(policy: &mut PolicyBuilder, by: &Person, who: &Person, role: Role) {
    let heads = vec![by.chain.head(), who.chain.head()];
    policy.push(
        by.first(),
        heads,
        PolicyKind::AddMember {
            account: who.account(),
            role,
            invitation: INVITATION,
        },
    );
}

fn verify(policy: &PolicyBuilder, people: &[&Person]) -> Result<PolicyChain, PolicyError> {
    let builders: Vec<&ChainBuilder> = people.iter().map(|p| &p.chain).collect();
    PolicyChain::verify(&policy.changes, &chains(&builders))
}

fn access(entries: &[(&Person, Level)]) -> Sorted<Access, sync_protocol::records::ByAccount, 1024> {
    Sorted::new(
        entries
            .iter()
            .map(|(p, level)| Access {
                account: p.account(),
                level: *level,
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn a_genesis_makes_its_signers_account_the_owner() {
    let alice = Person::new("alice");
    let policy = genesis(&alice);
    let verified = verify(&policy, &[&alice]).unwrap();
    let head = verified.head_snapshot();
    assert_eq!(head.owner(), Some(alice.account()));
    assert!(head.can_write(&alice.first().id(), &WS_DOMAIN));
    assert_eq!(head.card_indices[&alice.first().id()], 1);
}

#[test]
fn revisions_and_previous_link() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    let mut wrong = policy.changes[1].body.clone();
    wrong.previous = Digest([9; 32]);
    policy.changes[1] = sync_protocol::records::Signed::sign(wrong, &alice.first().key);
    assert_eq!(
        verify(&policy, &[&alice, &bob]).unwrap_err(),
        PolicyError::Previous { revision: 2 }
    );
}

#[test]
fn heads_must_hold_the_signers_account_where_the_signer_is_active() {
    let mut alice = Person::new("alice");
    let bob = Person::new("bob");
    let mut policy = genesis(&alice);
    // The heads lack the signer's account.
    policy.push(
        alice.first(),
        vec![bob.chain.head()],
        PolicyKind::AccountHeads,
    );
    assert_eq!(
        verify(&policy, &[&alice, &bob]).unwrap_err(),
        PolicyError::SignerNotInHeads { revision: 2 }
    );

    // A head at which the signer isn't active yet.
    let mut policy = genesis(&alice);
    let early = alice.chain.head();
    let phone = alice.add("alice phone");
    policy.push(&phone, vec![early], PolicyKind::AccountHeads);
    assert_eq!(
        verify(&policy, &[&alice]).unwrap_err(),
        PolicyError::SignerNotInHeads { revision: 2 }
    );
}

#[test]
fn heads_move_forward_on_the_recorded_chain() {
    let mut alice = Person::new("alice");
    alice.add("alice phone");
    let mut policy = genesis(&alice);
    // Genesis recorded seqno 2; a change at seqno 1 lies behind it.
    let behind = alice.chain.head_at(1);
    policy.push(
        alice.first(),
        vec![behind],
        PolicyKind::SetRole {
            account: alice.account(),
            role: Role::Admin,
        },
    );
    assert_eq!(
        verify(&policy, &[&alice]).unwrap_err(),
        PolicyError::HeadBehind { revision: 2 }
    );

    // A head off any verified chain.
    let mut policy = genesis(&alice);
    let mut forged = alice.chain.head();
    forged.head = Digest([1; 32]);
    policy.push(alice.first(), vec![forged], PolicyKind::AccountHeads);
    assert_eq!(
        verify(&policy, &[&alice]).unwrap_err(),
        PolicyError::UnknownHead { revision: 2 }
    );
}

#[test]
fn only_add_member_accept_reset_and_account_heads_record_other_accounts() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    policy.push(
        alice.first(),
        vec![alice.chain.head(), bob.chain.head()],
        PolicyKind::SetRole {
            account: bob.account(),
            role: Role::Viewer,
        },
    );
    assert_eq!(
        verify(&policy, &[&alice, &bob]).unwrap_err(),
        PolicyError::ForeignHead { revision: 3 }
    );
}

#[test]
fn add_member_refuses_a_member() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    add_member(&mut policy, &alice, &bob, Role::Viewer);
    assert_eq!(
        verify(&policy, &[&alice, &bob]).unwrap_err(),
        PolicyError::InvalidTarget { revision: 3 }
    );
}

#[test]
fn a_recovery_identity_signs_no_policy_change() {
    let mut alice = Person::new("alice");
    let kit = device(alice.account(), "alice kit");
    let signer = alice.first().clone();
    alice.chain.add_recovery(&signer, &kit);
    let mut policy = genesis(&alice);
    policy.push(&kit, vec![alice.chain.head()], PolicyKind::AccountHeads);
    assert_eq!(
        verify(&policy, &[&alice]).unwrap_err(),
        PolicyError::RecoverySigner { revision: 2 }
    );
}

#[test]
fn roles_limit_who_signs_each_kind() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let carol = Person::new("carol");
    let dave = Person::new("dave");
    let base = || {
        let mut policy = genesis(&alice);
        add_member(&mut policy, &alice, &bob, Role::Admin);
        add_member(&mut policy, &alice, &carol, Role::Editor);
        policy
    };
    let people = [&alice, &bob, &carol, &dave];
    assert!(verify(&base(), &people).is_ok());

    // An editor can't invite.
    let mut policy = base();
    add_member(&mut policy, &carol, &dave, Role::Viewer);
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 4 }
    );
    // An admin can't add an admin.
    let mut policy = base();
    add_member(&mut policy, &bob, &dave, Role::Admin);
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 4 }
    );
    // An admin removes an editor, not an admin; a member removes itself.
    let remove = |policy: &mut PolicyBuilder, by: &Person, who: &Person| {
        policy.push(
            by.first(),
            vec![by.chain.head()],
            PolicyKind::RemoveMember {
                account: who.account(),
            },
        );
    };
    let mut policy = base();
    remove(&mut policy, &bob, &carol);
    assert!(verify(&policy, &people).is_ok());
    let mut policy = base();
    remove(&mut policy, &carol, &bob);
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 4 }
    );
    let mut policy = base();
    remove(&mut policy, &bob, &bob);
    assert!(verify(&policy, &people).is_ok());
    // Nobody removes the owner.
    let mut policy = base();
    remove(&mut policy, &bob, &alice);
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::InvalidTarget { revision: 4 }
    );
    // An admin moves members only between editor and viewer.
    let set_role = |policy: &mut PolicyBuilder, by: &Person, who: &Person, role| {
        policy.push(
            by.first(),
            vec![by.chain.head()],
            PolicyKind::SetRole {
                account: who.account(),
                role,
            },
        );
    };
    let mut policy = base();
    set_role(&mut policy, &bob, &carol, Role::Viewer);
    assert!(verify(&policy, &people).is_ok());
    let mut policy = base();
    set_role(&mut policy, &bob, &carol, Role::Admin);
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 4 }
    );
    // Only the owner transfers ownership; the old owner becomes an admin.
    let mut policy = base();
    policy.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::TransferOwner {
            account: carol.account(),
        },
    );
    let verified = verify(&policy, &people).unwrap();
    assert_eq!(verified.head_snapshot().owner(), Some(carol.account()));
    assert_eq!(
        verified.head_snapshot().role(&alice.account()),
        Some(Role::Admin)
    );
    let mut policy = base();
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::TransferOwner {
            account: carol.account(),
        },
    );
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 4 }
    );
}

#[test]
fn restricted_domains_follow_their_access_lists() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let carol = Person::new("carol");
    let people = [&alice, &bob, &carol];
    let base = || {
        let mut policy = genesis(&alice);
        add_member(&mut policy, &alice, &bob, Role::Admin);
        add_member(&mut policy, &alice, &carol, Role::Editor);
        policy
    };

    // The creator must name itself a manager.
    let mut policy = base();
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::CreateDomain {
            domain: RESTRICTED,
            marker_rid: MARKER,
            access: access(&[(&carol, Level::Manage)]),
        },
    );
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::InvalidTarget { revision: 4 }
    );

    let mut policy = base();
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::CreateDomain {
            domain: RESTRICTED,
            marker_rid: MARKER,
            access: access(&[(&bob, Level::Manage), (&carol, Level::Read)]),
        },
    );
    let verified = verify(&policy, &people).unwrap();
    let head = verified.head_snapshot();
    // The owner reads no restricted domain unless granted.
    assert!(!head.can_read(&alice.first().id(), &RESTRICTED));
    assert!(head.can_read(&carol.first().id(), &RESTRICTED));
    assert!(!head.can_write(&carol.first().id(), &RESTRICTED));

    // An owner who doesn't manage it may only remove entries.
    let mut grant = PolicyBuilder::new(WORKSPACE);
    grant.changes = policy.changes.clone();
    grant.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::SetAccess {
            domain: RESTRICTED,
            access: access(&[
                (&alice, Level::Read),
                (&bob, Level::Manage),
                (&carol, Level::Read),
            ]),
        },
    );
    assert_eq!(
        verify(&grant, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 5 }
    );
    let mut cut = PolicyBuilder::new(WORKSPACE);
    cut.changes = policy.changes.clone();
    cut.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::SetAccess {
            domain: RESTRICTED,
            access: access(&[(&bob, Level::Manage)]),
        },
    );
    let verified = verify(&cut, &people).unwrap();
    assert_eq!(
        verified.head_snapshot().rotations,
        [RESTRICTED].into_iter().collect()
    );

    // A remove_member drops the account from every access list.
    let mut removed = PolicyBuilder::new(WORKSPACE);
    removed.changes = policy.changes.clone();
    removed.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::RemoveMember {
            account: carol.account(),
        },
    );
    let verified = verify(&removed, &people).unwrap();
    let domain = &verified.head_snapshot().domains[&RESTRICTED];
    assert!(!domain.access.contains_key(&carol.account()));
    assert_eq!(
        verified.head_snapshot().rotations,
        [WS_DOMAIN, RESTRICTED].into_iter().collect()
    );
}

#[test]
fn account_heads_records_something_new_or_the_one_allowed_repeat() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let people = [&alice, &bob];
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    // Alice records nothing new.
    let mut repeat = PolicyBuilder::new(WORKSPACE);
    repeat.changes = policy.changes.clone();
    repeat.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::AccountHeads,
    );
    assert_eq!(
        verify(&repeat, &people).unwrap_err(),
        PolicyError::NothingNew { revision: 3 }
    );
    // Bob's head was recorded by Alice's add_member: Bob's own device records
    // it again once, then never again.
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::AccountHeads,
    );
    assert!(verify(&policy, &people).is_ok());
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::AccountHeads,
    );
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::NothingNew { revision: 4 }
    );
}

#[test]
fn devices_and_card_indices_follow_the_recorded_heads() {
    let mut alice = Person::new("alice");
    let mut bob = Person::new("bob");
    let bob_phone = bob.add("bob phone");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    let verified = verify(&policy, &[&alice, &bob]).unwrap();
    let head = verified.head_snapshot();
    // Cards brought in at one change take indices in account order, then by
    // the seqno that added them.
    assert_eq!(head.card_indices[&alice.first().id()], 1);
    assert_eq!(head.card_indices[&bob.first().id()], 2);
    assert_eq!(head.card_indices[&bob_phone.id()], 3);

    // Alice adds a phone and records it; then removes it and records that.
    let phone = alice.add("alice phone");
    policy.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::AccountHeads,
    );
    let signer = alice.first().clone();
    alice.chain.remove(&signer, &phone, &[]);
    policy.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::AccountHeads,
    );
    let verified = verify(&policy, &[&alice, &bob]).unwrap();
    assert!(verified.at(3).unwrap().is_device(&phone.id()));
    assert!(!verified.at(4).unwrap().is_device(&phone.id()));
    assert_eq!(verified.at(3).unwrap().card_indices[&phone.id()], 4);
    assert_eq!(
        verified.at(4).unwrap().rotations,
        [WS_DOMAIN].into_iter().collect()
    );
    // The index stays for the workspace's life.
    assert_eq!(verified.at(4).unwrap().card_indices[&phone.id()], 4);
}

#[test]
fn card_indices_across_two_accounts_follow_account_order() {
    // Both accounts' cards enter at one change: the owner's at genesis is
    // index 1, then a member's two cards at the add_member.
    let alice = Person::new("alice");
    let mut zed = Person::new("zed");
    zed.add("zed phone");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &zed, Role::Viewer);
    let verified = verify(&policy, &[&alice, &zed]).unwrap();
    let indices = &verified.head_snapshot().card_indices;
    assert_eq!(indices[&zed.devices[0].id()], 2);
    assert_eq!(indices[&zed.devices[1].id()], 3);
}

/// A `remove_member` drops the account from every restricted access list,
/// as a grantee and as a manager, and an `add_member` that brings it back
/// grants it nothing there until a manager grants it again (§8).
#[test]
fn a_member_invited_again_stays_out_of_every_access_list() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let carol = Person::new("carol");
    let people = [&alice, &bob, &carol];
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Admin);
    add_member(&mut policy, &alice, &carol, Role::Editor);
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::CreateDomain {
            domain: RESTRICTED,
            marker_rid: MARKER,
            access: access(&[(&bob, Level::Manage), (&carol, Level::Write)]),
        },
    );
    for who in [&bob, &carol] {
        policy.push(
            alice.first(),
            vec![alice.chain.head()],
            PolicyKind::RemoveMember {
                account: who.account(),
            },
        );
    }
    let verified = verify(&policy, &people).unwrap();
    let domain = &verified.head_snapshot().domains[&RESTRICTED];
    assert!(domain.access.is_empty(), "the manager and the grantee left");

    add_member(&mut policy, &alice, &bob, Role::Admin);
    add_member(&mut policy, &alice, &carol, Role::Editor);
    let verified = verify(&policy, &people).unwrap();
    let head = verified.head_snapshot();
    assert!(head.domains[&RESTRICTED].access.is_empty());
    assert!(!head.can_read(&bob.first().id(), &RESTRICTED));
    assert!(!head.can_read(&carol.first().id(), &RESTRICTED));
    assert!(head.can_read(&carol.first().id(), &WS_DOMAIN));

    // An admin that no longer manages the domain only removes entries, so
    // it can't grant itself access again.
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::SetAccess {
            domain: RESTRICTED,
            access: access(&[(&bob, Level::Manage)]),
        },
    );
    assert_eq!(
        verify(&policy, &people).unwrap_err(),
        PolicyError::Unauthorized { revision: 9 }
    );
}

#[test]
fn a_member_who_leaves_and_returns_keeps_its_card_indices() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    policy.push(
        bob.first(),
        vec![bob.chain.head()],
        PolicyKind::RemoveMember {
            account: bob.account(),
        },
    );
    add_member(&mut policy, &alice, &bob, Role::Editor);
    let verified = verify(&policy, &[&alice, &bob]).unwrap();
    assert_eq!(verified.head_snapshot().card_indices[&bob.first().id()], 2);
    assert_eq!(verified.head_snapshot().card_indices.len(), 2);
}

#[test]
fn one_device_id_never_serves_two_accounts() {
    let alice = Person::new("alice");
    // Mallory's chain lists a card for Mallory's account whose key is
    // Alice's: its device ID is Alice's laptop's.
    let mallory_account = account("mallory");
    let stolen = Device {
        key: alice.first().key.clone(),
        card: sync_protocol::records::Card::new(
            mallory_account,
            alice.first().key.public(),
            vec![0x5a; 1_216],
        )
        .unwrap(),
    };
    let mallory = Person {
        chain: ChainBuilder::new(&stolen),
        devices: vec![stolen],
    };
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &mallory, Role::Viewer);
    assert_eq!(
        verify(&policy, &[&alice, &mallory]).unwrap_err(),
        PolicyError::DeviceUnderTwoAccounts { revision: 2 }
    );
}

#[test]
fn a_domain_holds_at_most_512_readers() {
    let mut alice = Person::new("alice");
    for i in 1..256 {
        alice.add(&format!("alice device {i}"));
    }
    let mut bob = Person::new("bob");
    for i in 1..254 {
        bob.add(&format!("bob device {i}"));
    }
    let mut carol = Person::new("carol");
    carol.add("carol phone");
    let dave = Person::new("dave");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    add_member(&mut policy, &alice, &carol, Role::Editor);
    assert_eq!(
        verify(&policy, &[&alice, &bob, &carol])
            .unwrap()
            .head_snapshot()
            .readers(&WS_DOMAIN)
            .len(),
        512
    );
    // A 513th reader is refused.
    let mut over = PolicyBuilder::new(WORKSPACE);
    over.changes = policy.changes.clone();
    add_member(&mut over, &alice, &dave, Role::Viewer);
    assert_eq!(
        verify(&over, &[&alice, &bob, &carol, &dave]).unwrap_err(),
        PolicyError::RecipientLimit { revision: 4 }
    );

    // A later head that adds one card and removes one still commits.
    let signer = carol.first().clone();
    let phone = carol.devices[1].clone();
    carol.chain.remove(&signer, &phone, &[]);
    carol.add("carol tablet");
    policy.push(
        carol.first(),
        vec![carol.chain.head()],
        PolicyKind::AccountHeads,
    );
    assert!(verify(&policy, &[&alice, &bob, &carol]).is_ok());
    // One that adds more than it removes doesn't.
    carol.add("carol watch");
    policy.push(
        carol.first(),
        vec![carol.chain.head()],
        PolicyKind::AccountHeads,
    );
    assert_eq!(
        verify(&policy, &[&alice, &bob, &carol]).unwrap_err(),
        PolicyError::RecipientLimit { revision: 5 }
    );
}

#[test]
fn card_indices_stop_at_16777215() {
    assert_eq!(next_card_index(0), Some(1));
    assert_eq!(next_card_index(MAX_CARD_INDEX - 1), Some(MAX_CARD_INDEX));
    assert_eq!(next_card_index(MAX_CARD_INDEX), None);
    assert_eq!(MAX_CARD_INDEX, 16_777_215);
}

#[test]
fn accept_reset_takes_out_the_old_chain_and_brings_in_the_new() {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let carol = Person::new("carol");
    let bob_reset = {
        let new_first = device(bob.account(), "bob reset laptop");
        Person {
            chain: ChainBuilder::new(&new_first),
            devices: vec![new_first],
        }
    };
    let base = || {
        let mut policy = genesis(&alice);
        add_member(&mut policy, &alice, &bob, Role::Editor);
        add_member(&mut policy, &alice, &carol, Role::Editor);
        policy.push(
            alice.first(),
            vec![alice.chain.head()],
            PolicyKind::CreateDomain {
                domain: RESTRICTED,
                marker_rid: MARKER,
                access: access(&[(&alice, Level::Manage), (&bob, Level::Write)]),
            },
        );
        policy
    };
    let mut people_chains = AccountChains::new();
    for chain in [&alice.chain, &bob.chain, &bob_reset.chain, &carol.chain] {
        people_chains.insert(chain.verified());
    }
    let accept = |policy: &mut PolicyBuilder, by: &Person| {
        policy.push(
            by.first(),
            vec![by.chain.head(), bob_reset.chain.head()],
            PolicyKind::AcceptReset {
                account: bob.account(),
            },
        );
    };
    let mut policy = base();
    accept(&mut policy, &alice);
    let verified = PolicyChain::verify(&policy.changes, &people_chains).unwrap();
    let head = verified.head_snapshot();
    assert!(!head.is_device(&bob.first().id()));
    assert!(head.is_device(&bob_reset.first().id()));
    assert!(
        !head.domains[&RESTRICTED]
            .access
            .contains_key(&bob.account())
    );
    assert_eq!(head.role(&bob.account()), Some(Role::Editor));
    assert!(head.rotations.contains(&WS_DOMAIN) && head.rotations.contains(&RESTRICTED));

    // From an editor: refused.
    let mut policy = base();
    accept(&mut policy, &carol);
    assert_eq!(
        PolicyChain::verify(&policy.changes, &people_chains).unwrap_err(),
        PolicyError::Unauthorized { revision: 5 }
    );
    // From the reset account itself: refused.
    let mut policy = base();
    policy.push(
        bob_reset.first(),
        vec![bob_reset.chain.head()],
        PolicyKind::AcceptReset {
            account: bob.account(),
        },
    );
    assert!(PolicyChain::verify(&policy.changes, &people_chains).is_err());
}

#[test]
fn a_device_trusts_a_log_only_through_its_own_accounts_active_card() {
    let mut alice = Person::new("alice");
    let bob = Person::new("bob");
    let phone = alice.add("alice phone");
    let mut policy = genesis(&bob);
    // Bob invites Alice; Alice's head is recorded by Bob's change only.
    add_member(&mut policy, &bob, &alice, Role::Editor);
    let verified = verify(&policy, &[&alice, &bob]).unwrap();
    let own = alice.chain.verified();
    let reference = alice.chain.head_at(1);
    assert_eq!(verified.anchored_by(&own, &reference), None);
    // Alice's laptop records her head: now her other devices trust it.
    policy.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::AccountHeads,
    );
    let verified = verify(&policy, &[&alice, &bob]).unwrap();
    assert_eq!(verified.anchored_by(&own, &reference), Some(3));
    // A card removed since confers no trust.
    let signer = phone.clone();
    alice.chain.remove(&signer, &alice.devices[0].clone(), &[]);
    let own = alice.chain.verified();
    assert_eq!(verified.anchored_by(&own, &reference), None);
}

#[test]
fn cards_two_accounts_bring_in_at_one_change_take_indices_by_account_id() {
    let mut alice = Person::new("alice");
    let mut bob = Person::new("bob");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    let alice_phone = alice.add("alice phone");
    let alice_tablet = alice.add("alice tablet");
    let bob_phone = bob.add("bob phone");
    // Alice's device records both accounts' later heads in one change.
    policy.push(
        alice.first(),
        vec![alice.chain.head(), bob.chain.head()],
        PolicyKind::AccountHeads,
    );
    let verified = verify(&policy, &[&alice, &bob]).unwrap();
    let indices = &verified.head_snapshot().card_indices;
    let (first, second) = if alice.account() < bob.account() {
        (vec![&alice_phone, &alice_tablet], vec![&bob_phone])
    } else {
        (vec![&bob_phone], vec![&alice_phone, &alice_tablet])
    };
    let order: Vec<u32> = first
        .iter()
        .chain(second.iter())
        .map(|d| indices[&d.id()])
        .collect();
    assert_eq!(order, vec![3, 4, 5]);
}
