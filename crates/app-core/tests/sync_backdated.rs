//! A server that commits what an honest one refuses after a removal (seam
//! 4, sync-v2.md §9, §30): a record from the removed device placed after
//! the change that ended its write access reports
//! `sync_writer_not_authorized`, and an epoch record the removed device
//! signed at an older revision, committed after the removal, gives no
//! reader a key; the remover rotates past it with a replacement.

mod sync_support;

use std::collections::BTreeSet;

use sync_protocol::codec::{Bytes, Encode, Sorted};
use sync_protocol::crypto::random_nonce;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::KeyRecord;
use sync_protocol::records::{Operation, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};
use sync_store::{Backend as _, Session};
use sync_support::{Desktop, Pair, approve, pair};

/// The third device's key of epoch 1, which it unwraps from its envelope.
async fn epoch_one_key(
    pair: &Pair<sync_store_sqlite::SqliteBackend>,
    third: &Desktop,
) -> SecretKey {
    let session = Session::new(third.credentials.account, third.id());
    pair.server
        .store
        .current_keys(session, pair.sync_b.workspace(), third.id())
        .await
        .unwrap()
        .entries
        .0
        .into_iter()
        .find_map(|e| match e.record {
            KeyRecord::Envelope(envelope) if envelope.body.recipient == third.id() => {
                Some(envelope)
            }
            _ => None,
        })
        .map(|envelope| {
            sync_key_envelope::v2::unwrap(
                &envelope.body,
                &third.credentials.keys,
                &third.credentials.card(),
            )
            .unwrap()
        })
        .expect("the third device's envelope")
}

/// Approves a third device, which both desktops take, and returns it with
/// the policy revision and head at which it was a device.
async fn third_device(pair: &mut Pair<sync_store_sqlite::SqliteBackend>) -> (Desktop, u64, Digest) {
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
    let (revision, head) = pair.sync_a.remote_mut().unwrap().policy().head();
    (third, revision, head)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_after_the_change_that_ended_its_writer_is_refused() {
    let mut pair = pair().await;
    let (third, revision, head) = third_device(&mut pair).await;
    // The laptop removes the third device and records it, which rotates.
    pair.laptop
        .service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(pair.sync_b.remote_mut().unwrap()),
        )
        .await
        .expect("removed");
    // The server places after the removal a record the third device signed
    // while it still could write.
    let (workspace, domain) = (
        pair.sync_b.workspace(),
        pair.sync_b.remote_mut().unwrap().domain,
    );
    let op = Signed::sign(
        Operation {
            workspace,
            domain,
            device: third.id(),
            counter: 1,
            previous: Digest::ZERO,
            epoch: 1,
            policy_revision: revision,
            policy_head: head,
            previous_attempt: Digest::ZERO,
            generation: pair.server.store.restore_generation(),
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid: Id::random().unwrap(),
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(vec![0; 272]),
        },
        third.credentials.keys.signing(),
    );
    let bytes = op.to_bytes();
    pair.server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                use sync_store::WriteTxn as _;
                txn.append_entry(workspace, domain, 1, &bytes).await?;
                Ok(())
            })
        }))
        .await
        .unwrap();
    let before = pair.a.files();
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_writer_not_authorized")
    );
    assert_eq!(pair.a.files(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_backdated_epoch_after_a_removal_is_replaced() {
    let mut pair = pair().await;
    let (third, revision, head) = third_device(&mut pair).await;
    let first_key = epoch_one_key(&pair, &third).await;
    let (workspace, domain) = (
        pair.sync_b.workspace(),
        pair.sync_b.remote_mut().unwrap().domain,
    );
    // The laptop removes the third device and records the removal, but the
    // server refuses its rotation.
    let pushes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = pushes.clone();
    pair.proxy.forge(Box::new(move |method, path| {
        if method != "POST" || !path.ends_with("/ops") {
            return None;
        }
        (count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1).then(|| (503, Vec::new()))
    }));
    let removal = pair
        .service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
        )
        .await;
    assert!(removal.is_err(), "the rotation didn't commit");
    pair.proxy.honest();

    // The server then commits an epoch the removed device signed at the
    // revision before the removal, with envelopes for the other two.
    let latest = pair
        .server
        .store
        .epochs(
            Session::new(pair.laptop.credentials.account, pair.laptop.id()),
            workspace,
            domain,
            1,
        )
        .await
        .unwrap()
        .epochs
        .0
        .remove(0)
        .0;
    let readers: BTreeSet<Id> = [pair.laptop.id(), pair.desktop.id(), third.id()]
        .into_iter()
        .collect();
    let key = SecretKey::random().unwrap();
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace,
                domain,
                epoch: 2,
                previous: latest.digest(),
                policy_revision: revision,
                policy_head: head,
                rotating_device: third.id(),
                recipients: readers,
            },
            &key,
            Some(&first_key),
            random_nonce().unwrap(),
        ),
        third.credentials.keys.signing(),
    );
    let envelopes: Vec<_> = [&pair.laptop, &pair.desktop]
        .into_iter()
        .map(|reader| {
            sync_key_envelope::v2::wrap(
                &sync_key_envelope::v2::EnvelopeHeader {
                    workspace,
                    domain,
                    epoch: 2,
                    wrapping_device: third.id(),
                },
                &key,
                &reader.credentials.card(),
                &third.credentials.keys,
            )
            .unwrap()
        })
        .collect();
    let digest = epoch.digest();
    let previous = epoch.body.previous;
    let bytes = epoch.to_bytes();
    pair.server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                use sync_store::WriteTxn as _;
                let seq = txn.append_entry(workspace, domain, 8, &bytes).await?;
                txn.insert_epoch(
                    workspace,
                    domain,
                    &sync_store::EpochRow {
                        epoch: 2,
                        seq,
                        digest,
                        previous,
                        record: bytes.clone(),
                    },
                    revision,
                    &bytes,
                )
                .await?;
                txn.append_key_entry(workspace, 3, domain, 2, None, &bytes)
                    .await?;
                for envelope in envelopes {
                    txn.append_key_entry(
                        workspace,
                        4,
                        domain,
                        2,
                        Some(envelope.body.recipient),
                        &envelope.to_bytes(),
                    )
                    .await?;
                }
                Ok(())
            })
        }))
        .await
        .unwrap();

    // The laptop takes no key from the removed device and rotates past its
    // epoch with a replacement.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 3);
    pair.a.write("Plan.md", b"# Plan, after the replacement\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.sync_b.remote_mut().unwrap().epoch(), 3);
    assert_eq!(
        pair.b.files()["Plan.md"],
        b"# Plan, after the replacement\n"
    );
}

/// A writer that never learned of a removal keeps signing under the old
/// epoch, and the server places its record after the rotation (§30): the
/// device that accepted the removal reports the stale epoch and changes no
/// file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_under_an_old_epoch_after_a_rotation_is_caught() {
    let mut pair = pair().await;
    let (third, revision, head) = third_device(&mut pair).await;
    pair.laptop
        .service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(pair.sync_b.remote_mut().unwrap()),
        )
        .await
        .expect("removed");
    let (workspace, domain) = (
        pair.sync_b.workspace(),
        pair.sync_b.remote_mut().unwrap().domain,
    );
    let op = Signed::sign(
        Operation {
            workspace,
            domain,
            device: pair.desktop.id(),
            counter: 1,
            previous: Digest::ZERO,
            epoch: 1,
            policy_revision: revision,
            policy_head: head,
            previous_attempt: Digest::ZERO,
            generation: pair.server.store.restore_generation(),
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid: Id::random().unwrap(),
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(vec![0; 272]),
        },
        pair.desktop.credentials.keys.signing(),
    );
    let bytes = op.to_bytes();
    pair.server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                use sync_store::WriteTxn as _;
                txn.append_entry(workspace, domain, 1, &bytes).await?;
                Ok(())
            })
        }))
        .await
        .unwrap();
    let before = pair.a.files();
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_epoch_stale")
    );
    assert_eq!(pair.a.files(), before);
}
