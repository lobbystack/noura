//! Replacing an epoch (seam 4, sync-v2.md §9, with the replacement and
//! rewrite cases of `sync-v2-keys.json`): a reader whose envelope of the
//! latest epoch fails signs a replacement that skips it, counts the
//! rotator's records under the skipped epoch as unreadable, and the rotator
//! writes its records again under the current epoch. When another device
//! used the skipped epoch, the server refuses the replacement with
//! `epoch_conflict`, the reader asks for a re-wrap and keeps the first
//! envelope that passes. Every other device that read the skipped epoch
//! writes again, after a random wait, the records it can read under it and
//! the profile, so a rotator that is gone loses nothing.

mod sync_support;

use std::sync::Arc;
use std::time::Duration;

use app_core::sync::upkeep::Task;
use app_core::sync::workspace::WorkspaceSync;
use sync_protocol::codec::{Bytes, Decode, Encode, ExactBytes, Sorted, Text};
use sync_protocol::messages::{
    EntryRecord, ErrorBody, KeyRecord, KeysResponse, PullResponse, PushFailureDetail,
};
use sync_protocol::records::{Access, Level, Role};
use sync_protocol::types::Id;
use sync_support::{
    Folder, Pair, Proxy, Server, approve, pair, pair_of_accounts_on, pair_of_accounts_with,
};

/// A laptop and a desktop of two accounts: a device doesn't replace an epoch
/// its own account rotated (§9).
async fn of_accounts() -> Pair<sync_store_sqlite::SqliteBackend> {
    pair_of_accounts_on(Server::start().await).await
}

/// Damages, on their way to `reader`, the envelopes that `wrapper` signed,
/// as a malicious member's or server's would arrive.
fn damage_envelopes(proxy: &Proxy, reader: Id, wrapper: Id) {
    proxy.tamper(Box::new(move |path, body| {
        if !(path.ends_with("/keys/current") || path.ends_with("/keys")) {
            return body;
        }
        let mut keys = KeysResponse::from_bytes(&body).unwrap();
        for entry in keys.entries.0.iter_mut() {
            if let KeyRecord::Envelope(envelope) = &mut entry.record
                && envelope.body.recipient == reader
                && envelope.body.wrapping_device == wrapper
            {
                let mut bytes = envelope.body.ciphertext.as_slice().to_vec();
                bytes[0] ^= 1;
                envelope.body.ciphertext = ExactBytes::new(bytes).unwrap();
            }
        }
        keys.to_bytes()
    }));
}

/// A third device of the desktop's account joins, and the desktop removes
/// it, records the removal and rotates to epoch 2.
async fn desktop_rotates(pair: &mut Pair<sync_store_sqlite::SqliteBackend>) {
    let third = approve(
        &pair.server,
        &mut pair.desktop,
        std::slice::from_mut(pair.sync_b.remote_mut().unwrap()),
    )
    .await;
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    pair.desktop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    pair.desktop
        .service
        .record_heads(pair.sync_b.remote_mut().unwrap(), &[])
        .await
        .expect("the desktop records the head and rotates");
    assert_eq!(pair.sync_b.remote_mut().unwrap().epoch(), 2);
}

/// After a restore, a device whose latest epoch a replacement ended asks
/// for that epoch's record. A failed request keeps the comparison pending,
/// so the check runs again instead of passing (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_comparison_that_cant_read_a_replaced_epoch_stays_pending() {
    let mut pair = of_accounts().await;
    desktop_rotates(&mut pair).await;
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop takes epoch 2");
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();
    let copy = pair.server.backup("before.sqlite3").await;
    let restored = pair.server.put_back(&copy, true).await;
    sync_support::reconnect(&mut pair, &restored.base).await;

    pair.proxy.forge(Box::new(|_, path| {
        path.ends_with("/epochs/2").then(|| (503, Vec::new()))
    }));
    let cut_short = pair.sync_b.round(&mut pair.desktop.service, 0).await;
    assert!(cut_short.is_err(), "{cut_short:?}");
    pair.proxy.honest();
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the comparison runs again");
    assert_eq!(pair.sync_b.restore_lost(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_envelope_is_replaced_and_the_rotator_writes_again() {
    let mut pair = of_accounts().await;
    desktop_rotates(&mut pair).await;
    // The desktop writes under epoch 2.
    pair.b.write("Plan.md", b"# Plan, under epoch 2\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop takes epoch 2 and pushes");

    // The laptop's envelope of epoch 2 fails: it replaces the epoch.
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();
    let epoch = pair.sync_a.remote_mut().unwrap().epoch();
    assert_eq!(epoch, 3, "the replacement skips epoch 2");
    // The rotator's record under the skipped epoch is unreadable here, and
    // the file keeps its last good state.
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan\n");
    assert!(
        pair.sync_a
            .unreadable()
            .iter()
            .any(|(_, device, _)| *device == pair.desktop.id())
    );

    // The desktop accepts the replacement and writes its record again.
    let round = pair
        .sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(round.captured, 1, "the rewrite");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan, under epoch 2\n");
    assert_eq!(pair.sync_a.status().conflicts, 0);
    assert_eq!(pair.sync_b.status().conflicts, 0);
}

/// A reader of a restricted domain whose envelope fails stops only there.
/// A key holder that reaches the upkeep answers its re-wrap request (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rewrap_in_a_restricted_domain_is_answered() {
    let mut pair = pair_of_accounts_with(Server::start().await, Role::Admin).await;
    let grant = Access {
        account: pair.laptop.credentials.account,
        level: Level::Read,
    };
    let domain = pair
        .sync_b
        .restrict_folder(&mut pair.desktop.service, "Pay", &[grant])
        .await
        .expect("restricted");
    pair.b.write("Pay/March.md", b"# March\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");

    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.a.write("Open.md", b"# Open\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the workspace domain goes on");
    assert!(pair.sync_a.key_stopped());
    assert!(!pair.a.files().contains_key("Pay/March.md"));

    // The key upkeep of a device that holds the domain's key sees the
    // request and answers it.
    let remote = pair.sync_b.remote_mut().unwrap();
    let waiting = pair
        .desktop
        .service
        .upkeep_waiting(remote)
        .await
        .expect("the upkeep reads");
    let task = waiting
        .iter()
        .map(|(task, _)| *task)
        .find(|task| matches!(task, Task::Rewrap { domain: d, .. } if *d == domain))
        .expect("the request");
    pair.desktop
        .service
        .run_upkeep(remote, &task)
        .await
        .expect("answered");

    pair.proxy.honest();
    for _ in 0..3 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round");
    }
    assert!(!pair.sync_a.key_stopped());
    assert_eq!(pair.a.files()["Pay/March.md"], b"# March\n");
}

/// A device that quits with a record under an epoch a replacement just
/// ended still reaches that epoch's key, so its next push re-seals (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_of_an_ended_epoch_survives_a_restart() {
    let mut pair = of_accounts().await;
    desktop_rotates(&mut pair).await;
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop takes epoch 2");
    pair.b.write("Waits.md", b"# Waits under epoch 2\n");
    pair.sync_b.capture().expect("signed under epoch 2");

    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 3);

    pair.sync_b
        .refresh_for_testing(&mut pair.desktop.service)
        .await
        .expect("the desktop accepts the replacement");
    pair.sync_b.lose_saves();
    pair.sync_b = WorkspaceSync::open(
        pair.b.engine.clone(),
        pair.desktop.credentials.clone(),
        pair.desktop.secrets(),
        &pair.b.registry,
    )
    .expect("the folder opens again");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the record goes out under the current epoch");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.a.files()["Waits.md"], b"# Waits under epoch 2\n");
}

/// An entry of an epoch a replacement ended names the rotator whose records
/// count as unreadable, so a forged one holds the cursor (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_entry_of_an_ended_epoch_holds_the_cursor() {
    let mut pair = of_accounts().await;
    desktop_rotates(&mut pair).await;
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop takes epoch 2");
    let (reader, wrapper) = (pair.laptop.id(), pair.desktop.id());
    pair.proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/pull") {
            let mut response = PullResponse::from_bytes(&body).unwrap();
            for page in response.pages.0.iter_mut() {
                let mut entries = page.entries.to_vec();
                for entry in entries.iter_mut() {
                    if let EntryRecord::Epoch(epoch) = &mut entry.record
                        && epoch.body.epoch == 2
                    {
                        epoch.body.rotating_device = reader;
                    }
                }
                page.entries = Sorted::new(entries).unwrap();
            }
            return response.to_bytes();
        }
        if !(path.ends_with("/keys/current") || path.ends_with("/keys")) {
            return body;
        }
        let mut keys = KeysResponse::from_bytes(&body).unwrap();
        for entry in keys.entries.0.iter_mut() {
            if let KeyRecord::Envelope(envelope) = &mut entry.record
                && envelope.body.recipient == reader
                && envelope.body.wrapping_device == wrapper
            {
                let mut bytes = envelope.body.ciphertext.as_slice().to_vec();
                bytes[0] ^= 1;
                envelope.body.ciphertext = ExactBytes::new(bytes).unwrap();
            }
        }
        keys.to_bytes()
    }));
    let cursor = pair.sync_a.cursor();
    let forged = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        forged.err().and_then(|e| e.code().map(str::to_owned)),
        Some("sync_key_mismatch".to_owned())
    );
    assert_eq!(pair.sync_a.cursor(), cursor, "the cursor holds");
    pair.proxy.honest();
}

/// Replacements don't loop (§9, §30): the server refuses a replacement of
/// a replacement, so the device asks for a re-wrap and the epoch stands
/// until an honest rotation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replacement_of_a_replacement_asks_for_a_rewrap() {
    let mut pair = of_accounts().await;
    desktop_rotates(&mut pair).await;
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 3);

    // Now the desktop's envelope of that replacement fails.
    let mut via = pair.desktop.connect_through(&pair.proxy.base).await;
    damage_envelopes(&pair.proxy, pair.desktop.id(), pair.laptop.id());
    let _ = pair.sync_b.round(&mut via, 0).await;
    pair.proxy.honest();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(
        pair.sync_a.remote_mut().unwrap().epoch(),
        3,
        "no second replacement committed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replacement_another_device_used_the_epoch_against_asks_for_a_rewrap() {
    let mut pair = pair().await;
    // A fourth device, which syncs too.
    let mut fourth = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    let mut d = Folder::new("Fourth");
    let mut sync_d = app_core::sync::workspace::WorkspaceSync::download(
        &mut fourth.service,
        d.engine.clone(),
        pair.sync_a.workspace(),
        &mut d.registry,
    )
    .await
    .expect("the fourth device downloads");
    sync_d.round(&mut fourth.service, 0).await.unwrap();
    desktop_rotates(&mut pair).await;
    // The fourth device writes under epoch 2.
    d.write("Fourth.md", b"# From the fourth device\n");
    sync_d
        .round(&mut fourth.service, 0)
        .await
        .expect("the fourth device takes epoch 2 and pushes");

    // The laptop's envelope fails; its replacement meets epoch_conflict,
    // so it asks for a re-wrap and waits.
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("waiting isn't a failure");
    assert!(round.waiting_for_key);
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 1);
    // The fourth device answers with an envelope of its own.
    let answers = fourth
        .service
        .answer_rewraps(sync_d.remote_mut().unwrap())
        .await
        .expect("answered");
    assert_eq!(answers.len(), 1);
    // The laptop keeps the first envelope that passes and reads epoch 2.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 2);
    assert_eq!(pair.a.files()["Fourth.md"], b"# From the fourth device\n");
    let _ = Arc::new(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_of_the_skipped_epoch_writes_a_gone_rotators_record_and_profile_again() {
    let mut pair = of_accounts().await;
    let mut fourth = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    let mut d = Folder::new("Fourth");
    let mut sync_d = app_core::sync::workspace::WorkspaceSync::download(
        &mut fourth.service,
        d.engine.clone(),
        pair.sync_a.workspace(),
        &mut d.registry,
    )
    .await
    .expect("the fourth device downloads");
    sync_d.round(&mut fourth.service, 0).await.unwrap();
    desktop_rotates(&mut pair).await;

    // The desktop writes and renames under epoch 2, then goes away.
    pair.b.write("Plan.md", b"# Plan, under epoch 2\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop pushes under epoch 2");
    pair.sync_b
        .rename(&mut pair.desktop.service, "Garden notes")
        .await
        .expect("renamed under epoch 2");
    let round = sync_d.round(&mut fourth.service, 0).await.unwrap();
    assert!(!round.waiting_for_key);
    assert_eq!(d.files()["Plan.md"], b"# Plan, under epoch 2\n");
    sync_d.connect(&mut fourth.service).await.unwrap();
    assert_eq!(sync_d.name(), "Garden notes");

    // The laptop's envelope of epoch 2 fails, and it replaces the epoch.
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 3);
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan\n");

    // The fourth device read epoch 2: after its wait it writes the
    // desktop's record and the profile again under epoch 3.
    sync_d.set_rewrite_wait(Duration::ZERO);
    let round = sync_d.round(&mut fourth.service, 0).await.expect("a round");
    assert_eq!(round.captured, 1, "the rewrite of the desktop's record");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan, under epoch 2\n");
    assert_eq!(pair.sync_a.status().conflicts, 0);
    pair.sync_a
        .connect(&mut pair.service)
        .await
        .expect("the laptop takes the profile signed again");
    assert_eq!(pair.sync_a.name(), "Garden notes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rewrite_whose_cover_the_server_cant_return_writes_nothing_and_waits_for_review() {
    let mut pair = of_accounts().await;
    desktop_rotates(&mut pair).await;
    pair.b.write("Plan.md", b"# Plan, under epoch 2\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop pushes under epoch 2");
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();

    // The server answers supersedes_unknown for the record the rewrite
    // covers, as it does once it pruned that record.
    let mut via = pair.desktop.connect_through(&pair.proxy.base).await;
    pair.proxy.forge(Box::new(|method, path| {
        if method != "POST" || !path.ends_with("/ops") {
            return None;
        }
        let detail = PushFailureDetail {
            record_index: 0,
            code: Text("supersedes_unknown".to_owned()),
            record_detail: None,
        };
        let body = ErrorBody {
            code: Text("supersedes_unknown".to_owned()),
            message: Text("refused".to_owned()),
            retry_after_seconds: None,
            detail: Some(Bytes(detail.to_bytes())),
        };
        Some((409, body.to_bytes()))
    }));
    pair.sync_b
        .round(&mut via, 0)
        .await
        .expect("a refused rewrite isn't an error");
    pair.proxy.honest();
    assert!(
        pair.sync_b.outbox().is_empty(),
        "the rewrite writes nothing"
    );
    let plan = pair.sync_b.rid_at("Plan.md").expect("synced");
    assert!(
        pair.sync_b.held().iter().any(|(rid, _)| *rid == plan),
        "the version waits for review"
    );
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan, under epoch 2\n");
    // Nothing more goes out for it.
    let round = pair
        .sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(round.pushed, 0);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_envelope_whose_wrapper_lost_access_after_the_epochs_entry_passes() {
    let mut pair = pair().await;
    let mut fourth = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    let mut d = Folder::new("Fourth");
    let mut sync_d = app_core::sync::workspace::WorkspaceSync::download(
        &mut fourth.service,
        d.engine.clone(),
        pair.sync_a.workspace(),
        &mut d.registry,
    )
    .await
    .expect("the fourth device downloads");
    sync_d.round(&mut fourth.service, 0).await.unwrap();

    // The desktop rotates to epoch 2, wrapping it for every reader, and
    // writes under it. The laptop takes epoch 2.
    desktop_rotates(&mut pair).await;
    pair.b.write("Plan.md", b"# Plan, under epoch 2\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop pushes under epoch 2");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop takes epoch 2");

    // The laptop removes the desktop after epoch 2's entry. The change
    // commits, and the rotation it makes due doesn't yet.
    let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = posts.clone();
    pair.proxy.forge(Box::new(move |method, path| {
        if method != "POST" || !path.ends_with("/ops") {
            return None;
        }
        (seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1).then(|| (503, Vec::new()))
    }));
    let desktop = pair.desktop.id();
    let before = pair.sync_a.remote_mut().unwrap().policy().head().0;
    let removed = pair
        .service
        .remove_device(
            desktop,
            &[],
            std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
        )
        .await;
    pair.proxy.honest();
    assert!(removed.is_err(), "the rotation didn't go out");
    assert_eq!(posts.load(std::sync::atomic::Ordering::SeqCst), 2);
    let removal = pair
        .service
        .client()
        .policy(pair.sync_a.workspace(), 0)
        .await
        .unwrap()
        .head_revision;
    assert!(removal > before, "the removal committed");
    let latest = pair
        .service
        .client()
        .current_keys(pair.sync_a.workspace(), pair.laptop.id())
        .await
        .unwrap();
    assert!(latest.entries.0.iter().any(|e| matches!(
        &e.record,
        KeyRecord::Epoch(epoch) if epoch.body.epoch == 2
    )));

    // The fourth device meets epoch 2's entry before the removal: its
    // envelope from the desktop passes the test at the latest policy entry
    // before that entry, so it reads the desktop's record and replaces
    // nothing.
    let round = sync_d.round(&mut fourth.service, 0).await.expect("a round");
    assert!(!round.waiting_for_key);
    assert!(sync_d.policy_revision() >= removal);
    assert!(sync_d.remote_mut().unwrap().epoch() >= 2);
    assert_eq!(d.files()["Plan.md"], b"# Plan, under epoch 2\n");
    assert!(sync_d.unreadable().is_empty());
}
