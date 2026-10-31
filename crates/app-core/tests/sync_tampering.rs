//! A tampering proxy between the sync client and the server (seam 4,
//! sync-v2.md §30): a server that drops, reorders, replays or forks a
//! device's chain, withholds this device's own newest records, serves an
//! older state, forges a record, serves an older policy chain or another
//! change at a policy entry, or leaves a policy entry out of the log gets
//! caught with the error §30 names; the client changes no file and holds
//! its cursor, so the next honest round applies everything. A server that
//! delays the key route costs time and drops no record.

mod sync_support;

use sync_protocol::codec::{Decode, Encode, List, Sorted};
use sync_protocol::messages::{
    Entry, EntryRecord, KeyRecord, KeysResponse, Page, PolicyResponse, PullResponse,
};
use sync_protocol::records::Signed;
use sync_protocol::types::{Digest, Id};
use sync_support::{Pair, Proxy, approve, pair};

/// Rewrites every page of every pull through `proxy` with `change`.
fn tamper_pulls(proxy: &Proxy, change: impl Fn(&mut Page) + Send + Sync + 'static) {
    proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            change(page);
        }
        response.to_bytes()
    }));
}

fn entries(page: &Page) -> Vec<Entry> {
    page.entries.to_vec()
}

fn set_entries(page: &mut Page, entries: Vec<Entry>) {
    page.entries = Sorted::new(entries).expect("sorted by sequence");
}

/// The page's operations signed by `device`, as (index, op).
fn ops_of(page: &Page, device: Id) -> Vec<usize> {
    page.entries
        .iter()
        .enumerate()
        .filter(
            |(_, e)| matches!(&e.record, EntryRecord::Operation(op) if op.body.device == device),
        )
        .map(|(i, _)| i)
        .collect()
}

/// The desktop writes three files in one round, which the laptop hasn't
/// pulled yet.
async fn three_from_the_desktop(pair: &mut Pair<sync_store_sqlite::SqliteBackend>) {
    pair.b.write("One.md", b"# One\n");
    pair.b.write("Two.md", b"# Two\n");
    pair.b.write("Three.md", b"# Three\n");
    let round = pair
        .sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(round.pushed, 3);
}

/// The laptop's round fails with `code` and changes no file; once the
/// proxy is honest, the next round applies everything.
async fn caught(pair: &mut Pair<sync_store_sqlite::SqliteBackend>, code: &str) {
    let before = pair.a.files();
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result.as_ref().err().and_then(|e| e.code()),
        Some(code),
        "{result:?}"
    );
    assert_eq!(pair.a.files(), before, "no file changed");
    pair.proxy.honest();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("an honest round");
    assert_eq!(
        pair.a.files(),
        pair.b.files(),
        "the held cursor lost nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_record_is_a_gap() {
    let mut pair = pair().await;
    three_from_the_desktop(&mut pair).await;
    let desktop = pair.desktop.id();
    tamper_pulls(&pair.proxy, move |page| {
        let ops = ops_of(page, desktop);
        if ops.len() == 3 {
            let mut entries = entries(page);
            entries.remove(ops[1]);
            set_entries(page, entries);
        }
    });
    caught(&mut pair, "sync_chain_gap").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reordered_record_is_caught() {
    let mut pair = pair().await;
    three_from_the_desktop(&mut pair).await;
    let desktop = pair.desktop.id();
    // The second record's real place, which the proxy moves after the
    // third, here and in the chain-position answers.
    let moved: std::sync::Arc<std::sync::Mutex<Option<(Digest, u64)>>> = Default::default();
    let seen = moved.clone();
    pair.proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/pull") {
            let mut response = PullResponse::from_bytes(&body).unwrap();
            for page in response.pages.0.iter_mut() {
                let ops = ops_of(page, desktop);
                if ops.len() == 3 {
                    let mut entries = entries(page);
                    let (second, third) = (entries[ops[1]].seq, entries[ops[2]].seq);
                    let record = entries[ops[1]].record.clone();
                    let EntryRecord::Operation(op) = &record else {
                        unreachable!()
                    };
                    *seen.lock().unwrap() = Some((op.digest(), third));
                    entries[ops[1]].record = entries[ops[2]].record.clone();
                    entries[ops[1]].seq = second;
                    entries[ops[2]].record = record;
                    entries[ops[2]].seq = third;
                    set_entries(page, entries);
                }
            }
            return response.to_bytes();
        }
        if path.contains("/chain/") {
            let mut entry = Entry::from_bytes(&body).unwrap();
            if let (EntryRecord::Operation(op), Some((digest, seq))) =
                (&entry.record, *seen.lock().unwrap())
                && op.digest() == digest
            {
                entry.seq = seq;
            }
            return entry.to_bytes();
        }
        body
    }));
    drop(moved);
    caught(&mut pair, "sync_chain_reordered").await;
}

/// Adds `entry` to the page after its last sequence.
fn append(page: &mut Page, record: EntryRecord) {
    let seq = page.head.max(page.complete_through) + 1;
    let mut entries = entries(page);
    entries.push(Entry { seq, record });
    set_entries(page, entries);
    page.head = seq;
    page.complete_through = seq;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_record_is_caught() {
    let mut pair = pair().await;
    three_from_the_desktop(&mut pair).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop applies them");
    let desktop = pair.desktop.id();
    let copy: std::sync::Arc<std::sync::Mutex<Option<EntryRecord>>> = Default::default();
    // The first record comes back at a later sequence.
    let stored = copy.clone();
    let workspace = pair.sync_a.workspace();
    let domain = pair.sync_a.remote_mut().unwrap().domain;
    let first = pair
        .service
        .client()
        .chain_position(workspace, domain, desktop, 1)
        .await
        .unwrap()
        .unwrap();
    *stored.lock().unwrap() = Some(first.record);
    pair.b.write("Four.md", b"# Four\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    tamper_pulls(&pair.proxy, move |page| {
        if let Some(record) = copy.lock().unwrap().clone()
            && !page.entries.is_empty()
        {
            append(page, record);
        }
    });
    caught(&mut pair, "sync_chain_replayed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forked_chain_is_caught_and_a_forged_record_is_refused() {
    let mut pair = pair().await;
    three_from_the_desktop(&mut pair).await;
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    let workspace = pair.sync_a.workspace();
    let domain = pair.sync_a.remote_mut().unwrap().domain;
    let desktop = pair.desktop.id();
    let first = pair
        .service
        .client()
        .chain_position(workspace, domain, desktop, 1)
        .await
        .unwrap()
        .unwrap();
    let EntryRecord::Operation(op) = first.record else {
        panic!("an operation");
    };
    // Another record at the same position, signed with the desktop's key,
    // as a device that forks its own chain would.
    let mut body = op.body.clone();
    body.rid = Id::random().unwrap();
    let fork = Signed::sign(body.clone(), pair.desktop.credentials.keys.signing());
    // And one whose signature the server made up.
    let mut forged = Signed::sign(body, pair.desktop.credentials.keys.signing());
    forged.sig.0[0] ^= 1;

    pair.b.write("Four.md", b"# Four\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    let forking = std::sync::Arc::new(fork);
    let record = forking.clone();
    tamper_pulls(&pair.proxy, move |page| {
        if !page.entries.is_empty() {
            append(page, EntryRecord::Operation((*record).clone()));
        }
    });
    caught(&mut pair, "sync_chain_forked").await;

    pair.b.write("Five.md", b"# Five\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    let forged = std::sync::Arc::new(forged);
    tamper_pulls(&pair.proxy, move |page| {
        if !page.entries.is_empty() {
            append(page, EntryRecord::Operation((*forged).clone()));
        }
    });
    caught(&mut pair, "sync_writer_not_authorized").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_state_and_withheld_own_records_are_caught() {
    let mut pair = pair().await;
    // An older but consistent state: a head below what the server showed.
    three_from_the_desktop(&mut pair).await;
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    tamper_pulls(&pair.proxy, |page| {
        let older = page.after.saturating_sub(2);
        page.head = older;
        page.complete_through = page.after;
        set_entries(page, Vec::new());
    });
    let before = pair.a.files();
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
    assert_eq!(pair.a.files(), before);
    // The rollback holds the device until the generation changes (§14).
    pair.proxy.honest();
    let held = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        held.err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn withheld_own_records_are_caught() {
    let mut pair = pair().await;
    three_from_the_desktop(&mut pair).await;
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();

    // The laptop's own record, which the server acknowledged, left out of
    // the page that claims to cover it.
    let laptop = pair.laptop.id();
    tamper_pulls(&pair.proxy, move |page| {
        let mut entries = entries(page);
        entries.retain(
            |e| !matches!(&e.record, EntryRecord::Operation(op) if op.body.device == laptop),
        );
        set_entries(page, entries);
    });
    pair.a.write("Mine.md", b"# Mine\n");
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_chain_gap")
    );
    pair.proxy.honest();
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_or_forked_policy_and_a_missing_policy_entry_are_caught() {
    let mut pair = pair().await;
    // A third device joins through the desktop, so the policy moves on
    // without the laptop.
    let _third = approve(
        &pair.server,
        &mut pair.desktop,
        std::slice::from_mut(pair.sync_b.remote_mut().unwrap()),
    )
    .await;

    // The policy entry of the new change, with another change's digest.
    tamper_pulls(&pair.proxy, |page| {
        let mut entries = entries(page);
        for entry in entries.iter_mut() {
            if let EntryRecord::Policy(policy) = &mut entry.record {
                policy.head = Digest([7; 32]);
            }
        }
        set_entries(page, entries);
    });
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_policy_forked")
    );

    // The same log without that entry, while the policy names the change.
    tamper_pulls(&pair.proxy, |page| {
        let mut entries = entries(page);
        entries.retain(|e| !matches!(e.record, EntryRecord::Policy(_)));
        set_entries(page, entries);
    });
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_chain_gap")
    );
    pair.proxy.honest();
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    let accepted = pair.sync_a.policy_revision();

    // An older policy chain, one change short of the accepted head.
    pair.proxy.tamper(Box::new(|path, body| {
        if path.ends_with("/policy") {
            let mut response = PolicyResponse::from_bytes(&body).unwrap();
            if response.head_revision > 1 {
                response.head_revision -= 1;
                let mut changes = response.changes.0;
                changes.pop();
                response.changes = List(changes);
            }
            return response.to_bytes();
        }
        if path.ends_with("/pull") {
            // A page that names a newer policy, so the laptop asks for it.
            let mut response = PullResponse::from_bytes(&body).unwrap();
            for page in response.pages.0.iter_mut() {
                page.policy_revision += 1;
            }
            return response.to_bytes();
        }
        body
    }));
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_policy_rollback")
    );
    // The rollback holds the device until the generation changes (§14).
    pair.proxy.honest();
    let held = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        held.err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_policy_rollback")
    );
    assert_eq!(pair.sync_a.policy_revision(), accepted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delayed_key_route_costs_time_and_drops_nothing() {
    let mut pair = pair().await;
    // The laptop approves a third device and removes it, which rotates the
    // workspace domain to epoch 2.
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    pair.laptop
        .service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
        )
        .await
        .expect("removed");
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 2);
    // The desktop writes under the new epoch: its first push meets
    // epoch_stale, and it re-seals once it holds the new key.
    pair.b.write("Under epoch 2.md", b"# New epoch\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop re-seals and pushes");
    assert!(pair.sync_b.outbox().is_empty());

    // The key route leaves out the laptop's envelopes for a while.
    let laptop = pair.laptop.id();
    pair.proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/keys/current") || path.contains("/keys?") || path.ends_with("/keys") {
            let mut response = KeysResponse::from_bytes(&body).unwrap();
            response.entries.0.retain(
                |e| !matches!(&e.record, KeyRecord::Envelope(env) if env.body.recipient == laptop),
            );
            return response.to_bytes();
        }
        body
    }));
    let before = pair.a.files();
    for _ in 0..3 {
        let round = pair
            .sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("waiting isn't a failure");
        assert!(round.waiting_for_key);
        assert_eq!(pair.a.files(), before);
    }
    pair.proxy.honest();
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert!(!round.waiting_for_key);
    assert_eq!(pair.a.files(), pair.b.files());
}
