//! `sync-v2-account.json`: account chain links and their verification
//! from the links alone (§7).

use serde_json::{Value, json};
use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::records::{AccountLink, LinkKind, Signed};
use sync_protocol::types::{Digest, Id, to_hex};

use super::common::{Actor, Fixture};
use super::world::{Chain, IntoDigest};

pub struct Family {
    pub laptop: Actor,
    pub phone: Actor,
    pub tablet: Actor,
    pub watch: Actor,
    pub kit: Actor,
    pub kit2: Actor,
}

pub fn family() -> Family {
    Family {
        laptop: Actor::new("carol", "carol laptop"),
        phone: Actor::new("carol", "carol phone"),
        tablet: Actor::new("carol", "carol tablet"),
        watch: Actor::new("carol", "carol watch"),
        kit: Actor::new("carol", "carol kit"),
        kit2: Actor::new("carol", "carol kit 2"),
    }
}

pub fn remove(target: &Actor, keep: &[&Actor]) -> LinkKind {
    LinkKind::Remove {
        device: target.device(),
        keep: Sorted::new(keep.iter().map(|a| a.device()).collect()).unwrap(),
    }
}

fn add_recovery(chain: &mut Chain, signer: &Actor, identity: &Actor) {
    chain.push(
        signer,
        LinkKind::AddRecovery {
            card: Nested(identity.card()),
        },
    );
}

fn recover(chain: &mut Chain, identity: &Actor, new: &Actor) {
    chain.push(
        identity,
        LinkKind::RecoverDevice {
            card: Nested(new.card()),
        },
    );
}

fn links(chain: &Chain) -> Vec<Value> {
    chain
        .links
        .iter()
        .map(|l| json!(to_hex(&l.to_bytes())))
        .collect()
}

fn ids(actors: &[&Actor]) -> Vec<String> {
    let mut ids: Vec<Id> = actors.iter().map(|a| a.device()).collect();
    ids.sort();
    ids.iter().map(Id::to_hex).collect()
}

fn ok(f: &mut Fixture, name: &str, chain: &Chain, active: &[&Actor]) {
    f.case(json!({
        "kind": "account_chain",
        "name": name,
        "links": links(chain),
        "listed_elsewhere": [],
        "expect": {
            "active": ids(active),
            "head_seqno": chain.links.len(),
            "fingerprint": chain.links[0].digest().to_hex(),
        },
    }));
}

fn refused(f: &mut Fixture, name: &str, chain: &Chain, error: &str, seqno: u64) {
    refused_with(f, name, chain, &[], error, seqno);
}

fn refused_with(
    f: &mut Fixture,
    name: &str,
    chain: &Chain,
    listed: &[&Actor],
    error: &str,
    seqno: u64,
) {
    f.case(json!({
        "kind": "account_chain",
        "name": name,
        "links": links(chain),
        "listed_elsewhere": ids(listed),
        "expect": {"error": error, "seqno": seqno},
    }));
}

fn resign(
    link: &Signed<AccountLink>,
    signer: &Actor,
    edit: impl FnOnce(&mut AccountLink),
) -> Signed<AccountLink> {
    let mut body = link.body.clone();
    edit(&mut body);
    Signed::sign(body, signer.keys.signing())
}

pub fn fixture() -> Fixture {
    let c = family();
    let mut f = Fixture::new(
        "sync-v2-account.json",
        "Account chains (sync-v2.md §7): each link kind, and verification from the links alone: seqno and previous links, who may sign each kind, cards, the 256-card and one-recovery-identity limits, removal cascades with keep, active cards at a head, the fingerprint, and kept heads.",
    );
    for a in [&c.laptop, &c.phone, &c.tablet, &c.watch, &c.kit, &c.kit2] {
        f.actor(a);
    }

    // Each link kind.
    let mut chain = Chain::genesis(&c.laptop);
    let add = chain.add_device(&c.laptop, &c.phone);
    add_recovery(&mut chain, &c.laptop, &c.kit);
    let add_recovery_link = chain.links.last().unwrap().clone();
    recover(&mut chain, &c.kit, &c.tablet);
    let recover_link = chain.links.last().unwrap().clone();
    let remove_link = chain.push(&c.laptop, remove(&c.kit, &[&c.tablet]));
    f.accept_signed("genesis", "account_link", &chain.links[0], &c.laptop);
    f.accept_signed("add_device", "account_link", &add, &c.laptop);
    f.accept_signed(
        "add_recovery",
        "account_link",
        &add_recovery_link,
        &c.laptop,
    );
    f.accept_signed("recover_device", "account_link", &recover_link, &c.kit);
    f.accept_signed("remove_with_keep", "account_link", &remove_link, &c.laptop);
    ok(
        &mut f,
        "every_kind",
        &chain,
        &[&c.laptop, &c.phone, &c.tablet],
    );
    let mut unknown_kind = add.to_bytes();
    let at = 21 + 16 + 8 + 32 + 16;
    unknown_kind[at] = 6;
    f.reject(
        "link_unknown_kind",
        "account_link",
        &unknown_kind,
        json!({"decode": "unknown_value"}),
    );

    // Links.
    let mut chain = Chain::genesis(&c.laptop);
    chain.add_device(&c.laptop, &c.phone);
    let mut skipped = chain.clone();
    skipped.links[1] = resign(&chain.links[1], &c.laptop, |b| b.seqno = 3);
    refused(&mut f, "seqno_skips", &skipped, "seqno", 3);
    let mut unlinked = chain.clone();
    unlinked.links[1] = resign(&chain.links[1], &c.laptop, |b| b.previous = Digest([1; 32]));
    refused(&mut f, "previous_not_the_head", &unlinked, "previous", 2);
    let mut other_account = chain.clone();
    other_account.links[1] = resign(&chain.links[1], &c.laptop, |b| b.account = Id([0x0b; 16]));
    refused(
        &mut f,
        "link_names_another_account",
        &other_account,
        "account",
        2,
    );
    let mut bad_signature = chain.clone();
    bad_signature.links[1].sig.0[0] ^= 1;
    refused(&mut f, "bad_signature", &bad_signature, "signature", 2);

    // Genesis.
    let mut genesis = Chain::genesis(&c.laptop);
    genesis.links[0] = resign(&genesis.links[0], &c.phone, |b| b.signer = c.phone.device());
    refused(
        &mut f,
        "genesis_signed_by_another_card",
        &genesis,
        "genesis_signer",
        1,
    );
    let mut second = Chain::genesis(&c.laptop);
    second.push(
        &c.laptop,
        LinkKind::Genesis {
            card: Nested(c.phone.card()),
        },
    );
    refused(&mut f, "second_genesis", &second, "genesis_position", 2);

    // Authority.
    let mut removed_signs = Chain::genesis(&c.laptop);
    removed_signs.add_device(&c.laptop, &c.phone);
    removed_signs.push(&c.laptop, remove(&c.phone, &[]));
    removed_signs.add_device(&c.phone, &c.tablet);
    refused(
        &mut f,
        "link_from_a_removed_card",
        &removed_signs,
        "signer_not_active",
        4,
    );
    let mut kit_adds = Chain::genesis(&c.laptop);
    add_recovery(&mut kit_adds, &c.laptop, &c.kit);
    kit_adds.add_device(&c.kit, &c.phone);
    refused(
        &mut f,
        "recovery_identity_signs_add_device",
        &kit_adds,
        "recovery_signs",
        3,
    );
    let mut kit_policy = Chain::genesis(&c.laptop);
    add_recovery(&mut kit_policy, &c.laptop, &c.kit);
    kit_policy.push(&c.kit, remove(&c.laptop, &[]));
    refused(
        &mut f,
        "recovery_identity_signs_remove",
        &kit_policy,
        "recovery_signs",
        3,
    );
    let mut twice = Chain::genesis(&c.laptop);
    add_recovery(&mut twice, &c.laptop, &c.kit);
    recover(&mut twice, &c.kit, &c.phone);
    recover(&mut twice, &c.kit, &c.tablet);
    refused(
        &mut f,
        "second_recover_device_from_one_identity",
        &twice,
        "recovery_used",
        4,
    );
    let mut not_kit = Chain::genesis(&c.laptop);
    recover(&mut not_kit, &c.laptop, &c.phone);
    refused(
        &mut f,
        "recover_device_from_a_device",
        &not_kit,
        "recovery_not_signer",
        2,
    );
    let mut two_kits = Chain::genesis(&c.laptop);
    add_recovery(&mut two_kits, &c.laptop, &c.kit);
    add_recovery(&mut two_kits, &c.laptop, &c.kit2);
    refused(
        &mut f,
        "second_active_recovery_identity",
        &two_kits,
        "second_recovery",
        3,
    );
    let mut relisted = Chain::genesis(&c.laptop);
    relisted.add_device(&c.laptop, &c.phone);
    relisted.push(&c.laptop, remove(&c.phone, &[]));
    relisted.add_device(&c.laptop, &c.phone);
    refused(
        &mut f,
        "add_device_of_a_device_id_listed_before",
        &relisted,
        "device_listed",
        4,
    );
    let mut after_reset = Chain::genesis(&c.laptop);
    after_reset.add_device(&c.laptop, &c.phone);
    refused_with(
        &mut f,
        "add_device_of_a_device_id_another_chain_listed",
        &after_reset,
        &[&c.phone],
        "device_listed",
        2,
    );
    let stranger = Actor::new("mallory", "mallory laptop");
    f.actor(&stranger);
    let mut foreign = Chain::genesis(&c.laptop);
    foreign.add_device(&c.laptop, &stranger);
    refused(
        &mut f,
        "card_naming_another_account",
        &foreign,
        "card_account",
        2,
    );
    let mut gone = Chain::genesis(&c.laptop);
    gone.add_device(&c.laptop, &c.phone);
    gone.push(&c.laptop, remove(&c.phone, &[]));
    gone.push(&c.laptop, remove(&c.phone, &[]));
    refused(
        &mut f,
        "remove_of_an_inactive_card",
        &gone,
        "remove_target",
        4,
    );

    // Removal cascades.
    let mut deep = Chain::genesis(&c.laptop);
    deep.add_device(&c.laptop, &c.phone);
    deep.add_device(&c.phone, &c.tablet);
    deep.add_device(&c.tablet, &c.watch);
    let mut two_levels = deep.clone();
    two_levels.push(&c.laptop, remove(&c.phone, &[]));
    ok(&mut f, "removal_two_levels_deep", &two_levels, &[&c.laptop]);
    let mut kept = deep.clone();
    kept.push(&c.laptop, remove(&c.phone, &[&c.tablet]));
    ok(
        &mut f,
        "keep_spares_a_device_and_its_cards",
        &kept,
        &[&c.laptop, &c.tablet, &c.watch],
    );
    let mut inside = deep.clone();
    inside.push(&c.tablet, remove(&c.phone, &[]));
    ok(
        &mut f,
        "signer_inside_the_removed_subtree_stays_with_its_cards",
        &inside,
        &[&c.laptop, &c.tablet, &c.watch],
    );
    let mut own = deep.clone();
    own.push(&c.phone, remove(&c.phone, &[]));
    ok(
        &mut f,
        "self_removal_leaves_the_devices_cards_active",
        &own,
        &[&c.laptop, &c.tablet, &c.watch],
    );
    let mut keep_kit = Chain::genesis(&c.laptop);
    keep_kit.add_device(&c.laptop, &c.phone);
    add_recovery(&mut keep_kit, &c.phone, &c.kit);
    keep_kit.push(&c.laptop, remove(&c.phone, &[&c.kit]));
    refused(
        &mut f,
        "keep_naming_a_recovery_identity",
        &keep_kit,
        "keep",
        4,
    );
    let mut stray = Chain::genesis(&c.laptop);
    stray.add_device(&c.laptop, &c.phone);
    stray.add_device(&c.laptop, &c.tablet);
    stray.push(&c.laptop, remove(&c.phone, &[&c.tablet]));
    refused(
        &mut f,
        "keep_naming_a_card_the_link_keeps_anyway",
        &stray,
        "keep",
        4,
    );
    let mut replaced = Chain::genesis(&c.laptop);
    add_recovery(&mut replaced, &c.laptop, &c.kit);
    recover(&mut replaced, &c.kit, &c.phone);
    replaced.push(&c.laptop, remove(&c.kit, &[]));
    add_recovery(&mut replaced, &c.laptop, &c.kit2);
    ok(
        &mut f,
        "kit_replacement_takes_down_the_device_it_recovered",
        &replaced,
        &[&c.laptop, &c.kit2],
    );
    let mut replaced_kept = Chain::genesis(&c.laptop);
    add_recovery(&mut replaced_kept, &c.laptop, &c.kit);
    recover(&mut replaced_kept, &c.kit, &c.phone);
    replaced_kept.push(&c.laptop, remove(&c.kit, &[&c.phone]));
    ok(
        &mut f,
        "kit_replacement_keeping_the_recovered_device",
        &replaced_kept,
        &[&c.laptop, &c.phone],
    );
    let mut recovery = Chain::genesis(&c.laptop);
    recovery.add_device(&c.laptop, &c.tablet);
    add_recovery(&mut recovery, &c.laptop, &c.kit);
    recover(&mut recovery, &c.kit, &c.phone);
    recovery.push(&c.phone, remove(&c.laptop, &[]));
    ok(
        &mut f,
        "recovery_that_removes_every_other_card",
        &recovery,
        &[&c.phone],
    );

    // Active cards at any head.
    let mut history = Chain::genesis(&c.laptop);
    history.add_device(&c.laptop, &c.phone);
    history.push(&c.laptop, remove(&c.phone, &[]));
    f.case(json!({
        "kind": "active_at",
        "name": "active_cards_at_each_head",
        "links": links(&history),
        "expect": {
            "1": ids(&[&c.laptop]),
            "2": ids(&[&c.laptop, &c.phone]),
            "3": ids(&[&c.laptop]),
        },
    }));

    // Kept heads.
    let mut kept_chain = Chain::genesis(&c.laptop);
    kept_chain.add_device(&c.laptop, &c.phone);
    kept_chain.add_device(&c.laptop, &c.tablet);
    let kept_head = kept_chain.head_at(2);
    let kept_value =
        json!({"genesis": kept_head.genesis.to_hex(), "seqno": 2, "head": kept_head.head.to_hex()});
    f.case(json!({"kind": "kept_head", "name": "chain_extends_the_kept_head", "links": links(&kept_chain), "kept": kept_value, "recorded": null, "expect": "ok"}));
    let mut fork = Chain::genesis(&c.laptop);
    fork.add_device(&c.laptop, &c.watch);
    fork.add_device(&c.laptop, &c.tablet);
    f.case(json!({"kind": "kept_head", "name": "fork_at_one_seqno", "links": links(&fork), "kept": kept_value, "recorded": null, "expect": "sync_account_forked"}));
    let short = Chain::genesis(&c.laptop);
    f.case(json!({"kind": "kept_head", "name": "chain_cut_back_below_the_kept_head", "links": links(&short), "kept": kept_value, "recorded": null, "expect": "sync_account_forked"}));
    f.case(json!({
        "kind": "kept_head",
        "name": "head_recorded_in_a_workspace_off_the_kept_chain",
        "links": links(&kept_chain),
        "kept": kept_value,
        "recorded": serde_json::to_value(fork.head()).unwrap(),
        "expect": "sync_account_forked",
    }));

    // Links a device takes after its kept head: a `previous` that names
    // another digest at any seqno is a fork, and any other broken rule is
    // unauthorized; both stop the workspaces the device shares with the
    // account, or every workspace for its own.
    let extend = |f: &mut Fixture,
                  name: &str,
                  served: &Chain,
                  own: bool,
                  outcome: &str,
                  head_seqno: Option<usize>| {
        let stops = match (outcome, own) {
            ("ok", _) => Value::Null,
            (_, true) => json!("every_workspace"),
            (_, false) => json!("shared_with_account"),
        };
        f.case(json!({
            "kind": "kept_head_extend",
            "name": name,
            "links": links(served),
            "kept": kept_value,
            "own_account": own,
            "expect": {"outcome": outcome, "head_seqno": head_seqno, "stops": stops},
        }));
    };
    extend(
        &mut f,
        "links_that_extend_the_kept_head",
        &kept_chain,
        false,
        "ok",
        Some(3),
    );
    let mut other_previous = kept_chain.clone();
    other_previous.links[2] = resign(&kept_chain.links[2], &c.laptop, |b| {
        b.previous = Digest([1; 32])
    });
    extend(
        &mut f,
        "a_link_after_the_kept_head_naming_another_previous",
        &other_previous,
        false,
        "sync_account_forked",
        None,
    );
    extend(
        &mut f,
        "the_own_accounts_link_after_the_kept_head_naming_another_previous",
        &other_previous,
        true,
        "sync_account_forked",
        None,
    );
    let mut later = kept_chain.clone();
    later.add_device(&c.laptop, &c.watch);
    later.links[3] = resign(&later.links[3], &c.laptop, |b| b.previous = Digest([2; 32]));
    extend(
        &mut f,
        "a_later_link_naming_another_previous",
        &later,
        false,
        "sync_account_forked",
        None,
    );
    let mut unauthorized = kept_chain.clone();
    unauthorized.links[2] = resign(&kept_chain.links[2], &c.watch, |b| {
        b.signer = c.watch.device()
    });
    extend(
        &mut f,
        "a_link_after_the_kept_head_from_a_card_that_isnt_active",
        &unauthorized,
        false,
        "sync_account_unauthorized",
        None,
    );
    extend(
        &mut f,
        "the_own_accounts_link_after_the_kept_head_from_a_card_that_isnt_active",
        &unauthorized,
        true,
        "sync_account_unauthorized",
        None,
    );
    let mut skips = kept_chain.clone();
    skips.links[2] = resign(&kept_chain.links[2], &c.laptop, |b| b.seqno = 4);
    extend(
        &mut f,
        "a_link_after_the_kept_head_that_skips_a_seqno",
        &skips,
        false,
        "sync_account_unauthorized",
        None,
    );
    let _ = id_offer();
    f
}

fn id_offer() -> Digest {
    super::common::id("approval offer").into_digest()
}
