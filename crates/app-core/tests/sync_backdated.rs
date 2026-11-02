//! A server that commits what an honest one refuses after a removal (seam
//! 4, sync-v2.md §9, §30): a record from the removed device placed after
//! the change that ended its write access reports
//! `sync_writer_not_authorized`, and an epoch record the removed device
//! signed at an older revision, committed after the removal, gives no
//! reader a key: recipients refuse its envelopes with `sync_key_mismatch`,
//! and the remover rotates past it with a replacement. A checkpoint that
//! lists another final head for the removed device fails with
//! `sync_manifest_mismatch`.

mod sync_support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_core::sync::SyncError;
use app_core::sync::workspace::WorkspaceSync;
use sync_protocol::codec::{Bytes, Decode, Encode, Nested, Sorted};
use sync_protocol::crypto::random_nonce;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::manifest::heads_digest;
use sync_protocol::messages::{EntryRecord, KeyRecord, PullResponse};
use sync_protocol::records::{CheckpointBundle, Operation, Ref, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};
use sync_store::{Backend as _, Session};
use sync_support::{Desktop, Folder, Pair, Server, approve, pair, pair_on, rounds};

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

    // A recipient refuses its envelopes: the wrapper couldn't read the
    // domain at the latest policy entry before the epoch's entry.
    let policy = pair.sync_a.remote_mut().unwrap().policy().clone();
    let refused = pair
        .desktop
        .service
        .check_keys_after_entry(workspace, domain, &policy, policy.head().0)
        .await
        .expect_err("the removed device's envelope");
    assert!(
        matches!(refused, SyncError::Check("sync_key_mismatch")),
        "{refused:?}"
    );

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

/// A split view (§30): the server places the removed device's last record
/// before the removal's entry for the laptop, and the first checkpoint that
/// covers the entry comes from a side that never saw it, so it lists the
/// head one record lower. The proxy stands in for that side, re-signing the
/// desktop's checkpoint under the desktop's key with the lower head. The
/// laptop reports `sync_manifest_mismatch`, accepts nothing and keeps going.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_listing_another_final_head_is_reported() {
    let never = Duration::from_secs(1 << 40);
    let mut pair = pair_on(Server::start_with_interval(6).await).await;
    pair.sync_a.set_checkpoint_wait(never);
    pair.sync_b.set_checkpoint_wait(never);
    let (mut third, _, _) = third_device(&mut pair).await;
    let (workspace, domain) = (pair.sync_b.workspace(), pair.sync_b.domain());

    // The third device writes two records, which both desktops apply.
    let mut folder = Folder::new("Third");
    let mut sync = WorkspaceSync::download(
        &mut third.service,
        folder.engine.clone(),
        workspace,
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    sync.round(&mut third.service, 0).await.expect("a round");
    for name in ["One.md", "Two.md"] {
        folder.write(name, b"# Third\n");
        sync.round(&mut third.service, 0).await.expect("a round");
    }
    rounds(&mut pair).await;

    // The desktop removes it, then signs what falls due.
    pair.desktop
        .service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(pair.sync_b.remote_mut().unwrap()),
        )
        .await
        .expect("removed");
    let removal = pair.sync_b.remote_mut().unwrap().policy().head().0;
    let client = pair.desktop.service.client();
    let (one, two) = (
        client
            .chain_position(workspace, domain, third.id(), 1)
            .await
            .expect("a read")
            .expect("its first record"),
        client
            .chain_position(workspace, domain, third.id(), 2)
            .await
            .expect("a read")
            .expect("its second record"),
    );
    assert!(two.seq > one.seq);
    let lower = Ref {
        device: third.id(),
        counter: 1,
        seq: one.seq,
        op_digest: match &one.record {
            EntryRecord::Operation(op) => op.digest(),
            other => panic!("not an operation: {other:?}"),
        },
    };
    let signer = pair.desktop.credentials.keys.signing().clone();
    let forged_digest = Arc::new(Mutex::new(None));
    let noted = forged_digest.clone();
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            let entries: Vec<_> = page
                .entries
                .iter()
                .cloned()
                .map(|mut entry| {
                    if let EntryRecord::Checkpoint(bundle) = &mut entry.record
                        && bundle.checkpoint.0.body.policy_revision >= removal
                    {
                        let heads: Vec<Ref> = bundle
                            .heads
                            .iter()
                            .map(|head| {
                                if head.device == lower.device {
                                    lower.clone()
                                } else {
                                    head.clone()
                                }
                            })
                            .collect();
                        let mut forged = bundle.checkpoint.0.body.clone();
                        forged.heads_digest = heads_digest(&heads);
                        let forged = Signed::sign(forged, &signer);
                        noted.lock().unwrap().get_or_insert(forged.digest());
                        *bundle = CheckpointBundle {
                            checkpoint: Nested(forged),
                            heads: Sorted::new(heads).expect("sorted"),
                        };
                    }
                    entry
                })
                .collect();
            page.entries = Sorted::new(entries).expect("sorted");
        }
        response.to_bytes()
    }));

    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    let files = |pair: &Pair<_>| (pair.a.files(), pair.b.files());
    for index in 0..40 {
        if !pair.sync_a.checkpoint_failures().is_empty() {
            break;
        }
        pair.b.write(&format!("Note {index}.md"), b"# Note\n");
        rounds(&mut pair).await;
    }
    let failures = pair.sync_a.checkpoint_failures().to_vec();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(Some(failures[0].digest), *forged_digest.lock().unwrap());
    // Which check answers depends on whether an honest checkpoint from
    // before the removal was accepted first.
    assert!(
        ["sync_manifest_mismatch", "sync_manifest_rollback"].contains(&failures[0].code.as_str()),
        "{failures:?}"
    );
    assert_eq!(pair.sync_a.restore_lost(), None, "the workspace goes on");
    assert_ne!(
        pair.sync_a.latest_checkpoint().map(|c| c.digest()),
        Some(failures[0].digest),
        "a failed checkpoint never becomes the latest accepted one"
    );
    assert!(pair.sync_b.checkpoint_failures().is_empty());
    let (a, b) = files(&pair);
    assert_eq!(a, b);
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
