//! Replacing an epoch (seam 4, sync-v2.md §9, with the replacement and
//! rewrite cases of `sync-v2-keys.json`): a reader whose envelope of the
//! latest epoch fails signs a replacement that skips it, counts the
//! rotator's records under the skipped epoch as unreadable, and the rotator
//! writes its records again under the current epoch. When another device
//! used the skipped epoch, the server refuses the replacement with
//! `epoch_conflict`, the reader asks for a re-wrap and keeps the first
//! envelope that passes.

mod sync_support;

use std::sync::Arc;

use sync_protocol::codec::{Decode, Encode, ExactBytes};
use sync_protocol::messages::{KeyRecord, KeysResponse};
use sync_protocol::types::Id;
use sync_support::{Folder, Pair, Proxy, approve, pair};

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

/// A third device joins and the laptop removes it without recording the
/// removal, so the desktop records it and rotates to epoch 2.
async fn desktop_rotates(pair: &mut Pair<sync_store_sqlite::SqliteBackend>) {
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    pair.laptop
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_envelope_is_replaced_and_the_rotator_writes_again() {
    let mut pair = pair().await;
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
