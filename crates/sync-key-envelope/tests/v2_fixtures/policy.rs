//! `sync-v2-policy.json`: policy changes and their verification against
//! the account chains they record (§8): links, account heads, authority,
//! devices and card indices at each revision, reader limits and trust.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use sync_protocol::codec::{Encode, Sorted};
use sync_protocol::records::{
    Access, AccountHead, Level, LinkKind, PolicyChange, PolicyKind, Role, Signed,
};
use sync_protocol::types::{Digest, Id, to_hex};

use super::common::{Actor, Fixture, id};
use super::world::{Chain, Policy};

pub const WS_DOMAIN_LABEL: &str = "policy workspace domain";

pub struct Person {
    pub chain: Chain,
    pub devices: Vec<Actor>,
}

impl Person {
    pub fn new(name: &str) -> Self {
        let first = Actor::new(name, &format!("{name} laptop"));
        Self {
            chain: Chain::genesis(&first),
            devices: vec![first],
        }
    }

    pub fn account(&self) -> Id {
        self.devices[0].account
    }

    pub fn first(&self) -> &Actor {
        &self.devices[0]
    }

    pub fn add(&mut self, name: &str) -> Actor {
        let new = Actor::new(&self.name(), name);
        let signer = self.devices[0].clone();
        self.chain.add_device(&signer, &new);
        self.devices.push(new.clone());
        new
    }

    fn name(&self) -> String {
        self.devices[0].name.trim_end_matches(" laptop").to_owned()
    }

    pub fn head(&self) -> AccountHead {
        self.chain.head()
    }
}

pub fn workspace() -> Id {
    id("policy workspace")
}

pub fn ws_domain() -> Id {
    id(WS_DOMAIN_LABEL)
}

pub fn restricted() -> Id {
    id("policy restricted domain")
}

pub fn genesis(owner: &Person) -> Policy {
    let mut policy = Policy::new(workspace());
    policy.push(
        owner.first(),
        vec![owner.head()],
        PolicyKind::Genesis {
            workspace_domain: ws_domain(),
        },
    );
    policy
}

pub fn add_member(policy: &mut Policy, by: &Person, who: &Person, role: Role) {
    policy.push(
        by.first(),
        vec![by.head(), who.head()],
        PolicyKind::AddMember {
            account: who.account(),
            role,
            invitation: id("policy invitation"),
        },
    );
}

pub fn access(
    entries: &[(&Person, Level)],
) -> Sorted<Access, sync_protocol::records::ByAccount, 1024> {
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

fn chains(people: &[&Chain]) -> Value {
    json!(
        people
            .iter()
            .map(|c| c
                .links
                .iter()
                .map(|l| to_hex(&l.to_bytes()))
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    )
}

fn changes(policy: &Policy) -> Vec<String> {
    policy
        .changes
        .iter()
        .map(|c| to_hex(&c.to_bytes()))
        .collect()
}

pub fn ids(actors: &[&Actor]) -> Vec<String> {
    let mut ids: Vec<Id> = actors.iter().map(|a| a.device()).collect();
    ids.sort();
    ids.iter().map(Id::to_hex).collect()
}

fn refused(
    f: &mut Fixture,
    name: &str,
    people: &[&Person],
    policy: &Policy,
    error: &str,
    revision: u64,
) {
    let chain_list: Vec<&Chain> = people.iter().map(|p| &p.chain).collect();
    f.case(json!({
        "kind": "policy_chain",
        "name": name,
        "account_chains": chains(&chain_list),
        "changes": changes(policy),
        "expect": {"error": error, "revision": revision},
    }));
}

fn accepted(f: &mut Fixture, name: &str, people: &[&Chain], policy: &Policy, checks: Value) {
    f.case(json!({
        "kind": "policy_chain",
        "name": name,
        "account_chains": chains(people),
        "changes": changes(policy),
        "expect": {"ok": checks},
    }));
}

fn resign(
    change: &Signed<PolicyChange>,
    signer: &Actor,
    edit: impl FnOnce(&mut PolicyChange),
) -> Signed<PolicyChange> {
    let mut body = change.body.clone();
    edit(&mut body);
    Signed::sign(body, signer.keys.signing())
}

fn people_chains(people: &[&Person]) -> Vec<Chain> {
    people.iter().map(|p| p.chain.clone()).collect()
}

/// Two accounts bring cards in at one change: card indices follow account
/// ID, then the seqno of the link that added each card.
pub fn two_accounts_one_change() -> (Vec<Chain>, Policy, BTreeMap<String, u32>) {
    let mut alice = Person::new("dora");
    let mut bob = Person::new("eli");
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Editor);
    let phone = alice.add("dora phone");
    let tablet = alice.add("dora tablet");
    let bob_phone = bob.add("eli phone");
    policy.push(
        alice.first(),
        vec![alice.head(), bob.head()],
        PolicyKind::AccountHeads,
    );
    let mut indices = BTreeMap::new();
    indices.insert(alice.first().device().to_hex(), 1);
    indices.insert(bob.first().device().to_hex(), 2);
    let (first, second): (Vec<&Actor>, Vec<&Actor>) = if alice.account() < bob.account() {
        (vec![&phone, &tablet], vec![&bob_phone])
    } else {
        (vec![&bob_phone], vec![&phone, &tablet])
    };
    for (i, actor) in first.iter().chain(second.iter()).enumerate() {
        indices.insert(actor.device().to_hex(), 3 + i as u32);
    }
    (
        vec![alice.chain.clone(), bob.chain.clone()],
        policy,
        indices,
    )
}

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-policy.json",
        "Workspace policy (sync-v2.md §8): each change kind with its heads, and verification from the chain and the account chains it records: revisions and previous links, account heads, the authority table, devices and card indices at each revision, one device ID under one account, the reader and card index limits, rotations each change makes due, and trust through the device's own account.",
    );
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let carol = Person::new("carol");
    let dave = Person::new("dave");
    for p in [&alice, &bob, &carol, &dave] {
        f.actor(p.first());
    }

    // Each change kind.
    let mut policy = genesis(&alice);
    add_member(&mut policy, &alice, &bob, Role::Admin);
    add_member(&mut policy, &alice, &carol, Role::Editor);
    policy.push(bob.first(), vec![bob.head()], PolicyKind::AccountHeads);
    policy.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::SetRole {
            account: carol.account(),
            role: Role::Viewer,
        },
    );
    policy.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::CreateDomain {
            domain: restricted(),
            marker_rid: id("policy marker"),
            access: access(&[(&alice, Level::Manage), (&carol, Level::Read)]),
        },
    );
    policy.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::SetAccess {
            domain: restricted(),
            access: access(&[(&alice, Level::Manage)]),
        },
    );
    policy.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::DeleteDomain {
            domain: restricted(),
        },
    );
    policy.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::RemoveMember {
            account: carol.account(),
        },
    );
    policy.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::TransferOwner {
            account: bob.account(),
        },
    );
    let names = [
        "genesis",
        "add_member",
        "add_member_editor",
        "account_heads",
        "set_role",
        "create_domain",
        "set_access",
        "delete_domain",
        "remove_member",
        "transfer_owner",
    ];
    for (change, name) in policy.changes.iter().zip(names) {
        let signer = if change.body.signer == bob.first().device() {
            bob.first()
        } else {
            alice.first()
        };
        f.accept_signed(name, "policy_change", change, signer);
    }
    let all = people_chains(&[&alice, &bob, &carol, &dave]);
    let all_refs: Vec<&Chain> = all.iter().collect();
    accepted(
        &mut f,
        "every_kind",
        &all_refs,
        &policy,
        json!({
            "owner": bob.account().to_hex(),
            "roles": {alice.account().to_hex(): "admin", bob.account().to_hex(): "owner"},
            "devices": {"9": ids(&[alice.first(), bob.first()])},
            "rotations": {"7": [restricted().to_hex()], "9": [ws_domain().to_hex()]},
        }),
    );

    // Accept reset: Bob's new chain replaces his old one.
    let bob_reset = {
        let first = Actor::new("bob", "bob reset laptop");
        Person {
            chain: Chain::genesis(&first),
            devices: vec![first],
        }
    };
    f.actor(bob_reset.first());
    let mut reset = genesis(&alice);
    add_member(&mut reset, &alice, &bob, Role::Editor);
    reset.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::CreateDomain {
            domain: restricted(),
            marker_rid: id("policy marker"),
            access: access(&[(&alice, Level::Manage), (&bob, Level::Write)]),
        },
    );
    let mut accept_reset = reset.clone();
    accept_reset.push(
        alice.first(),
        vec![alice.head(), bob_reset.head()],
        PolicyKind::AcceptReset {
            account: bob.account(),
        },
    );
    f.accept_signed(
        "accept_reset",
        "policy_change",
        accept_reset.changes.last().unwrap(),
        alice.first(),
    );
    let reset_chains = [
        alice.chain.clone(),
        bob.chain.clone(),
        bob_reset.chain.clone(),
        carol.chain.clone(),
    ];
    let reset_refs: Vec<&Chain> = reset_chains.iter().collect();
    accepted(
        &mut f,
        "accept_reset_by_the_owner",
        &reset_refs,
        &accept_reset,
        json!({
            "devices": {"4": ids(&[alice.first(), bob_reset.first()])},
            "access": {restricted().to_hex(): [alice.account().to_hex()]},
            "roles": {bob.account().to_hex(): "editor"},
            "rotations": {"4": [ws_domain().to_hex(), restricted().to_hex()]},
        }),
    );
    let mut by_editor = reset.clone();
    add_member(&mut by_editor, &alice, &carol, Role::Editor);
    by_editor.push(
        carol.first(),
        vec![carol.head(), bob_reset.head()],
        PolicyKind::AcceptReset {
            account: bob.account(),
        },
    );
    f.case(json!({
        "kind": "policy_chain",
        "name": "accept_reset_from_an_editor",
        "account_chains": chains(&reset_refs),
        "changes": changes(&by_editor),
        "expect": {"error": "unauthorized", "revision": 5},
    }));
    let mut by_reset_account = reset.clone();
    by_reset_account.push(
        bob_reset.first(),
        vec![bob_reset.head()],
        PolicyKind::AcceptReset {
            account: bob.account(),
        },
    );
    f.case(json!({
        "kind": "policy_chain",
        "name": "accept_reset_from_the_reset_account",
        "account_chains": chains(&reset_refs),
        "changes": changes(&by_reset_account),
        "expect": {"error": "unauthorized", "revision": 4},
    }));

    // Links.
    let mut base = genesis(&alice);
    add_member(&mut base, &alice, &bob, Role::Editor);
    let mut forked = base.clone();
    forked.changes[1] = resign(&base.changes[1], alice.first(), |b| {
        b.previous = Digest([9; 32])
    });
    refused(
        &mut f,
        "previous_not_the_head",
        &[&alice, &bob],
        &forked,
        "previous",
        2,
    );
    let mut skipped = base.clone();
    skipped.changes[1] = resign(&base.changes[1], alice.first(), |b| b.revision = 3);
    refused(
        &mut f,
        "revision_skips",
        &[&alice, &bob],
        &skipped,
        "revision",
        3,
    );
    let mut bad_sig = base.clone();
    bad_sig.changes[1].sig.0[3] ^= 1;
    refused(
        &mut f,
        "bad_signature",
        &[&alice, &bob],
        &bad_sig,
        "signature",
        2,
    );

    // Account heads.
    let mut lacks = genesis(&alice);
    lacks.push(alice.first(), vec![bob.head()], PolicyKind::AccountHeads);
    refused(
        &mut f,
        "heads_lack_the_signers_account",
        &[&alice, &bob],
        &lacks,
        "signer_not_in_heads",
        2,
    );
    let mut grown = Person::new("alice");
    let phone = grown.add("alice phone");
    let mut early = genesis(&grown);
    early.push(
        &phone,
        vec![grown.chain.head_at(1)],
        PolicyKind::AccountHeads,
    );
    refused(
        &mut f,
        "head_where_the_signer_isnt_active",
        &[&grown],
        &early,
        "signer_not_in_heads",
        2,
    );
    let mut behind = genesis(&grown);
    behind.push(
        grown.first(),
        vec![grown.chain.head_at(1)],
        PolicyKind::SetRole {
            account: grown.account(),
            role: Role::Admin,
        },
    );
    refused(
        &mut f,
        "head_behind_the_recorded_one",
        &[&grown],
        &behind,
        "head_behind",
        2,
    );
    let mut off_chain = genesis(&alice);
    let mut forged = alice.head();
    forged.head = Digest([1; 32]);
    off_chain.push(alice.first(), vec![forged], PolicyKind::AccountHeads);
    refused(
        &mut f,
        "head_off_the_accounts_chain",
        &[&alice],
        &off_chain,
        "unknown_head",
        2,
    );
    let mut foreign = base.clone();
    foreign.push(
        alice.first(),
        vec![alice.head(), bob.head()],
        PolicyKind::SetRole {
            account: bob.account(),
            role: Role::Viewer,
        },
    );
    refused(
        &mut f,
        "another_account_outside_add_member_accept_reset_and_account_heads",
        &[&alice, &bob],
        &foreign,
        "foreign_head",
        3,
    );
    let mut nothing_new = base.clone();
    nothing_new.push(alice.first(), vec![alice.head()], PolicyKind::AccountHeads);
    refused(
        &mut f,
        "account_heads_recording_nothing_new",
        &[&alice, &bob],
        &nothing_new,
        "nothing_new",
        3,
    );
    let mut repeat = base.clone();
    repeat.push(bob.first(), vec![bob.head()], PolicyKind::AccountHeads);
    accepted(
        &mut f,
        "own_device_records_its_head_once",
        &[&alice.chain, &bob.chain],
        &repeat,
        json!({}),
    );
    repeat.push(bob.first(), vec![bob.head()], PolicyKind::AccountHeads);
    refused(
        &mut f,
        "own_device_records_its_head_twice",
        &[&alice, &bob],
        &repeat,
        "nothing_new",
        4,
    );

    // Authority.
    let mut kit_owner = Person::new("alice");
    let kit = Actor::new("alice", "alice kit");
    f.actor(&kit);
    let signer = kit_owner.first().clone();
    kit_owner.chain.push(
        &signer,
        LinkKind::AddRecovery {
            card: sync_protocol::codec::Nested(kit.card()),
        },
    );
    let mut by_kit = genesis(&kit_owner);
    by_kit.push(&kit, vec![kit_owner.head()], PolicyKind::AccountHeads);
    refused(
        &mut f,
        "recovery_identity_signs_a_policy_change",
        &[&kit_owner],
        &by_kit,
        "recovery_signer",
        2,
    );
    let mut again = base.clone();
    add_member(&mut again, &alice, &bob, Role::Viewer);
    refused(
        &mut f,
        "add_member_for_a_member",
        &[&alice, &bob],
        &again,
        "invalid_target",
        3,
    );
    let three = || {
        let mut p = genesis(&alice);
        add_member(&mut p, &alice, &bob, Role::Admin);
        add_member(&mut p, &alice, &carol, Role::Editor);
        p
    };
    let people = [&alice, &bob, &carol, &dave];
    let mut editor_invites = three();
    add_member(&mut editor_invites, &carol, &dave, Role::Viewer);
    refused(
        &mut f,
        "editor_invites",
        &people,
        &editor_invites,
        "unauthorized",
        4,
    );
    let mut admin_adds_admin = three();
    add_member(&mut admin_adds_admin, &bob, &dave, Role::Admin);
    refused(
        &mut f,
        "admin_adds_an_admin",
        &people,
        &admin_adds_admin,
        "unauthorized",
        4,
    );
    let mut editor_removes_admin = three();
    editor_removes_admin.push(
        carol.first(),
        vec![carol.head()],
        PolicyKind::RemoveMember {
            account: bob.account(),
        },
    );
    refused(
        &mut f,
        "editor_removes_an_admin",
        &people,
        &editor_removes_admin,
        "unauthorized",
        4,
    );
    let mut owner_removed = three();
    owner_removed.push(
        bob.first(),
        vec![bob.head()],
        PolicyKind::RemoveMember {
            account: alice.account(),
        },
    );
    refused(
        &mut f,
        "owner_removed",
        &people,
        &owner_removed,
        "invalid_target",
        4,
    );
    let mut leaves = three();
    leaves.push(
        bob.first(),
        vec![bob.head()],
        PolicyKind::RemoveMember {
            account: bob.account(),
        },
    );
    let three_chains = people_chains(&people);
    let three_refs: Vec<&Chain> = three_chains.iter().collect();
    accepted(
        &mut f,
        "member_removes_itself",
        &three_refs,
        &leaves,
        json!({"devices": {"4": ids(&[alice.first(), carol.first()])}}),
    );
    let mut admin_promotes = three();
    admin_promotes.push(
        bob.first(),
        vec![bob.head()],
        PolicyKind::SetRole {
            account: carol.account(),
            role: Role::Admin,
        },
    );
    refused(
        &mut f,
        "admin_makes_an_admin",
        &people,
        &admin_promotes,
        "unauthorized",
        4,
    );
    let mut admin_transfers = three();
    admin_transfers.push(
        bob.first(),
        vec![bob.head()],
        PolicyKind::TransferOwner {
            account: carol.account(),
        },
    );
    refused(
        &mut f,
        "admin_transfers_ownership",
        &people,
        &admin_transfers,
        "unauthorized",
        4,
    );
    let mut no_manager = three();
    no_manager.push(
        bob.first(),
        vec![bob.head()],
        PolicyKind::CreateDomain {
            domain: restricted(),
            marker_rid: id("policy marker"),
            access: access(&[(&carol, Level::Manage)]),
        },
    );
    refused(
        &mut f,
        "create_domain_without_the_signer_as_manager",
        &people,
        &no_manager,
        "invalid_target",
        4,
    );
    let mut created = three();
    created.push(
        bob.first(),
        vec![bob.head()],
        PolicyKind::CreateDomain {
            domain: restricted(),
            marker_rid: id("policy marker"),
            access: access(&[(&bob, Level::Manage), (&carol, Level::Read)]),
        },
    );
    let mut owner_grants = created.clone();
    owner_grants.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::SetAccess {
            domain: restricted(),
            access: access(&[
                (&alice, Level::Read),
                (&bob, Level::Manage),
                (&carol, Level::Read),
            ]),
        },
    );
    refused(
        &mut f,
        "owner_who_doesnt_manage_grants",
        &people,
        &owner_grants,
        "unauthorized",
        5,
    );
    let mut owner_cuts = created.clone();
    owner_cuts.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::SetAccess {
            domain: restricted(),
            access: access(&[(&bob, Level::Manage)]),
        },
    );
    accepted(
        &mut f,
        "owner_who_doesnt_manage_removes_a_grant",
        &three_refs,
        &owner_cuts,
        json!({
            "rotations": {"5": [restricted().to_hex()]},
            "readers": {"4": {restricted().to_hex(): ids(&[bob.first(), carol.first()])}},
        }),
    );

    // Devices and card indices.
    let mut growing = Person::new("frank");
    let gina = Person::new("gina");
    f.actor(growing.first());
    f.actor(gina.first());
    let mut indices = genesis(&growing);
    add_member(&mut indices, &growing, &gina, Role::Editor);
    let frank_phone = growing.add("frank phone");
    f.actor(&frank_phone);
    indices.push(
        growing.first(),
        vec![growing.head()],
        PolicyKind::AccountHeads,
    );
    let signer = growing.first().clone();
    growing.chain.push(
        &signer,
        LinkKind::Remove {
            device: frank_phone.device(),
            keep: Sorted::default(),
        },
    );
    indices.push(
        growing.first(),
        vec![growing.head()],
        PolicyKind::AccountHeads,
    );
    indices.push(
        growing.first(),
        vec![growing.head()],
        PolicyKind::RemoveMember {
            account: gina.account(),
        },
    );
    add_member(&mut indices, &growing, &gina, Role::Editor);
    let mut card_indices = Map::new();
    card_indices.insert(growing.first().device().to_hex(), json!(1));
    card_indices.insert(gina.first().device().to_hex(), json!(2));
    card_indices.insert(frank_phone.device().to_hex(), json!(3));
    accepted(
        &mut f,
        "devices_before_and_after_a_removal_and_indices_kept_for_life",
        &[&growing.chain, &gina.chain],
        &indices,
        json!({
            "devices": {
                "3": ids(&[growing.first(), &frank_phone, gina.first()]),
                "4": ids(&[growing.first(), gina.first()]),
                "5": ids(&[growing.first()]),
                "6": ids(&[growing.first(), gina.first()]),
            },
            "card_indices": card_indices,
            "rotations": {"4": [ws_domain().to_hex()], "5": [ws_domain().to_hex()]},
        }),
    );
    let (two_chains, two_policy, two_indices) = two_accounts_one_change();
    let two_refs: Vec<&Chain> = two_chains.iter().collect();
    accepted(
        &mut f,
        "card_indices_across_two_accounts_at_one_change",
        &two_refs,
        &two_policy,
        json!({"card_indices": two_indices}),
    );

    // One device ID under one account.
    let stolen = {
        let mut a = Actor::new("mallory", "alice laptop");
        a.account = super::common::account_id("mallory");
        a
    };
    let mallory = Person {
        chain: Chain::genesis(&stolen),
        devices: vec![stolen],
    };
    let mut twice = genesis(&alice);
    add_member(&mut twice, &alice, &mallory, Role::Viewer);
    refused(
        &mut f,
        "one_device_id_under_two_accounts",
        &[&alice, &mallory],
        &twice,
        "device_under_two_accounts",
        2,
    );

    // Trust through the device's own account.
    let mut joiner = Person::new("hana");
    let hana_phone = joiner.add("hana phone");
    f.actor(joiner.first());
    f.actor(&hana_phone);
    let mut invited = genesis(&alice);
    add_member(&mut invited, &alice, &joiner, Role::Editor);
    f.case(json!({
        "kind": "anchored_by",
        "name": "a_log_that_nothing_anchors",
        "account_chains": chains(&[&alice.chain, &joiner.chain]),
        "changes": changes(&invited),
        "own_chain": joiner.chain.links.iter().map(|l| to_hex(&l.to_bytes())).collect::<Vec<_>>(),
        "reference_seqno": 1,
        "expect": null,
    }));
    invited.push(
        joiner.first(),
        vec![joiner.head()],
        PolicyKind::AccountHeads,
    );
    f.case(json!({
        "kind": "anchored_by",
        "name": "trusted_through_a_change_its_account_signed",
        "account_chains": chains(&[&alice.chain, &joiner.chain]),
        "changes": changes(&invited),
        "own_chain": joiner.chain.links.iter().map(|l| to_hex(&l.to_bytes())).collect::<Vec<_>>(),
        "reference_seqno": 1,
        "expect": 3,
    }));
    let mut later = joiner.chain.clone();
    let laptop = joiner.first().clone();
    later.push(
        &hana_phone,
        LinkKind::Remove {
            device: laptop.device(),
            keep: Sorted::default(),
        },
    );
    f.case(json!({
        "kind": "anchored_by",
        "name": "a_change_from_a_card_removed_since_confers_no_trust",
        "account_chains": chains(&[&alice.chain, &joiner.chain]),
        "changes": changes(&invited),
        "own_chain": later.links.iter().map(|l| to_hex(&l.to_bytes())).collect::<Vec<_>>(),
        "reference_seqno": 1,
        "expect": null,
    }));
    // Review of 0eef7d5: a reset device trusts a workspace its own request
    // didn't name through an account_heads change that its sibling, a
    // device it approved into the new chain, signed there once that
    // workspace accepted the reset.
    let ivan = Person::new("ivan");
    let mut ivan_reset = {
        let first = Actor::new("ivan", "ivan reset laptop");
        Person {
            chain: Chain::genesis(&first),
            devices: vec![first],
        }
    };
    let sibling = Actor::new("ivan", "ivan reset phone");
    let reset_laptop = ivan_reset.first().clone();
    ivan_reset.chain.add_device(&reset_laptop, &sibling);
    ivan_reset.devices.push(sibling.clone());
    f.actor(ivan.first());
    f.actor(ivan_reset.first());
    f.actor(&sibling);
    let mut second = Policy::new(id("policy second workspace"));
    second.push(
        alice.first(),
        vec![alice.head()],
        PolicyKind::Genesis {
            workspace_domain: id("policy second workspace domain"),
        },
    );
    add_member(&mut second, &alice, &ivan, Role::Editor);
    second.push(
        alice.first(),
        vec![alice.head(), ivan_reset.chain.head_at(1)],
        PolicyKind::AcceptReset {
            account: ivan.account(),
        },
    );
    second.push(&sibling, vec![ivan_reset.head()], PolicyKind::AccountHeads);
    f.case(json!({
        "kind": "anchored_by",
        "name": "a_reset_device_trusts_a_second_workspace_through_its_siblings_change",
        "account_chains": chains(&[&alice.chain, &ivan.chain, &ivan_reset.chain]),
        "changes": changes(&second),
        "own_chain": ivan_reset.chain.links.iter().map(|l| to_hex(&l.to_bytes())).collect::<Vec<_>>(),
        "reference_seqno": 1,
        "expect": 4,
    }));
    chain_reads(&mut f);
    f.set(
        "limits",
        json!({"readers_per_domain": 512, "card_indices": 16_777_215, "domains": 256}),
    );
    f
}

/// The chains a session reads through a workspace (§7): every chain the
/// policy records, up to the latest head recorded on it, a current
/// member's current chain in full to the workspace's devices, and, for a
/// device whose account `remove_member` removed, the chains up to the
/// heads recorded by its removal. A case with `approval_request` gives the
/// target account a waiting approval request that names an invitation to
/// this workspace, which lets the sessions of the workspace's member
/// accounts read its current chain in full.
fn chain_reads(f: &mut Fixture) {
    let mut kim = Person::new("kim");
    let mut lee = Person::new("lee");
    let max = Person::new("max");
    let ned = Person::new("ned");
    let mut oda = Person::new("oda");
    for person in [&kim, &lee, &max, &ned, &oda] {
        f.actor(person.first());
    }
    let oda_phone = oda.add("oda phone");
    f.actor(&oda_phone);
    let mut policy = genesis(&kim);
    add_member(&mut policy, &kim, &lee, Role::Editor);
    // An early change that the member signed.
    policy.push(lee.first(), vec![lee.head()], PolicyKind::AccountHeads);
    policy.push(
        kim.first(),
        vec![kim.head()],
        PolicyKind::RemoveMember {
            account: lee.account(),
        },
    );
    add_member(&mut policy, &kim, &max, Role::Viewer);
    // Both chains grow after the removal, and the workspace records only
    // the owner's new head.
    let kim_phone = kim.add("kim phone");
    let lee_phone = lee.add("lee phone");
    f.actor(&kim_phone);
    f.actor(&lee_phone);
    policy.push(kim.first(), vec![kim.head()], PolicyKind::AccountHeads);
    let case = |name: &str, reader: &Person, target: &Person, expect: Option<u64>| {
        json!({
            "kind": "chain_read",
            "name": name,
            "account_chains": chains(&[&kim.chain, &lee.chain, &max.chain, &oda.chain]),
            "changes": changes(&policy),
            "reader": reader.first().device().to_hex(),
            "reader_account": reader.account().to_hex(),
            "account": target.account().to_hex(),
            "genesis": target.chain.links[0].digest().to_hex(),
            "current_seqno": target.chain.links.len(),
            "expect": expect,
        })
    };
    let read =
        |f: &mut Fixture, name: &str, reader: &Person, target: &Person, expect: Option<u64>| {
            f.case(case(name, reader, target, expect));
        };
    let invited =
        |f: &mut Fixture, name: &str, reader: &Person, target: &Person, expect: Option<u64>| {
            let mut case = case(name, reader, target, expect);
            case["approval_request"] = json!({ "invitation": id("oda invitation").to_hex() });
            f.case(case);
        };
    read(
        f,
        "a_joining_device_reads_a_since_removed_members_chain_up_to_its_last_recorded_head",
        &max,
        &lee,
        Some(1),
    );
    read(
        f,
        "a_member_removed_with_remove_member_reads_the_removers_chain_up_to_its_removal",
        &lee,
        &kim,
        Some(1),
    );
    read(
        f,
        "a_current_members_current_chain_reaches_the_workspaces_devices_in_full",
        &max,
        &kim,
        Some(2),
    );
    read(
        f,
        "a_device_outside_the_workspace_reads_no_chain_through_it",
        &ned,
        &kim,
        None,
    );
    read(
        f,
        "a_member_reads_no_chain_of_an_account_the_workspace_never_recorded",
        &max,
        &oda,
        None,
    );
    invited(
        f,
        "a_member_reads_an_invitees_current_chain_in_full_while_its_approval_request_waits",
        &max,
        &oda,
        Some(2),
    );
    invited(
        f,
        "an_invitees_approval_request_opens_its_chain_to_no_removed_member",
        &lee,
        &oda,
        None,
    );
    invited(
        f,
        "an_invitees_approval_request_opens_its_chain_to_no_account_outside_the_workspace",
        &ned,
        &oda,
        None,
    );
}
