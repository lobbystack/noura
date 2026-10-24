//! Keys at seam 4 (sync-v2.md §8, §9): a device whose envelope failed asks
//! for a re-wrap, another reader answers with a lone envelope, and the
//! requester's next fetch closes the request; a newly approved device
//! takes its keys from `keys/current`; an `account_heads` change whose
//! envelopes wouldn't fit one push records the head in steps; and a
//! tampered envelope or an epoch for the wrong recipients fails with
//! `sync_key_mismatch`.

mod sync_support;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use app_core::sync::SyncError;
use app_core::sync::heads::RewrapAnswer;
use app_core::sync::upkeep::{KEY_HOLDER_WAIT, Task, Upkeep};
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{KeyRecord, KeysResponse};
use sync_protocol::records::{LinkKind, Signed};
use sync_protocol::types::{Id, SecretKey};
use sync_store::{Backend as _, Session};
use sync_support::{Desktop, Proxy, Server, approve, first_desktop};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rewrap_reaches_the_readers_and_closes_once_fetched() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "ada@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let mut phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();

    phone.service.request_rewrap(&remote).await.expect("asked");
    let answers = laptop
        .service
        .answer_rewraps(&mut workspace)
        .await
        .expect("answered");
    assert_eq!(answers, vec![RewrapAnswer::Answered(phone.id())]);
    let answers = laptop.service.answer_rewraps(&mut workspace).await.unwrap();
    assert_eq!(
        answers,
        vec![RewrapAnswer::Refused(phone.id())],
        "one answer per wrapping device"
    );
    phone
        .service
        .refresh_keys(&mut remote)
        .await
        .expect("the new envelope");
    let open = server
        .store
        .current_keys(
            Session::new(laptop.credentials.account, laptop.id()),
            workspace.workspace,
            laptop.id(),
        )
        .await
        .unwrap();
    assert!(
        !open
            .entries
            .iter()
            .any(|e| matches!(e.record, KeyRecord::Rewrap(_))),
        "the requester's fetch closed the request"
    );
}

/// The upkeep that waits its turn (§8, §9): a head of the device's own
/// account falls due within 60 seconds and a re-wrap answer within 60, and
/// once a removal's rotation committed, another key holder finds no
/// rotation pending and skips it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upkeep_waits_its_turn_then_records_heads_and_answers_rewraps() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "cy@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let mut phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();
    assert!(
        laptop
            .service
            .upkeep_waiting(&mut workspace)
            .await
            .unwrap()
            .is_empty(),
        "nothing waits"
    );

    // A tablet joins the chain while the workspace lags behind, and the
    // phone asks for a re-wrap.
    let mut none = Vec::new();
    let tablet = approve(&server, &mut laptop, &mut none).await;
    phone.service.request_rewrap(&remote).await.unwrap();
    let id = workspace.workspace;
    let epoch = workspace.epoch();
    let waiting = laptop.service.upkeep_waiting(&mut workspace).await.unwrap();
    assert_eq!(
        waiting,
        vec![
            (
                Task::Head {
                    workspace: id,
                    account: laptop.credentials.account,
                },
                Duration::ZERO..Duration::from_secs(60),
            ),
            (
                Task::Rewrap {
                    workspace: id,
                    device: phone.id(),
                    epoch,
                },
                KEY_HOLDER_WAIT,
            ),
        ]
    );
    let mut upkeep = Upkeep::default();
    let start = Instant::now();
    let mut due: BTreeSet<Task> = upkeep.plan(waiting.clone(), start).into_iter().collect();
    due.extend(upkeep.plan(waiting, start + Duration::from_secs(60)));
    assert_eq!(due.len(), 2, "each falls due within its 60 seconds");
    for task in &due {
        laptop
            .service
            .run_upkeep(&mut workspace, task)
            .await
            .expect("ran");
    }
    assert!(
        workspace
            .policy()
            .head_snapshot()
            .devices
            .contains_key(&tablet.id()),
        "the head is recorded"
    );
    phone
        .service
        .refresh_keys(&mut remote)
        .await
        .expect("the re-wrapped envelope");

    // The laptop removes the tablet: its upkeep records the head and
    // rotates, so the phone, which holds the key too, has nothing to do.
    let chain = laptop.service.own_chain().await.unwrap();
    let removal = sync_key_envelope::v2::next_link(
        &laptop.credentials.keys,
        &chain.head(),
        LinkKind::Remove {
            device: tablet.id(),
            keep: Default::default(),
        },
    );
    laptop.service.append_links(vec![removal]).await.unwrap();
    let waiting = laptop.service.upkeep_waiting(&mut workspace).await.unwrap();
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    laptop
        .service
        .run_upkeep(&mut workspace, &waiting[0].0)
        .await
        .expect("recorded and rotated");
    assert!(workspace.epoch() > epoch, "the removal rotated the domain");
    let waiting = phone.service.upkeep_waiting(&mut remote).await.unwrap();
    assert!(
        waiting.is_empty(),
        "the rotation committed, so none is pending: {waiting:?}"
    );
}

/// A lagging head whose envelopes wouldn't fit one push, ending in a
/// removal, goes in steps of one card-adding link (§8, §20).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lagging_head_goes_in_steps_when_its_envelopes_wont_fit() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "bea@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    // Three devices join the chain while the workspace lags behind, and
    // the last one leaves again.
    let mut none = Vec::new();
    let phone = approve(&server, &mut laptop, &mut none).await;
    let tablet = approve(&server, &mut laptop, &mut none).await;
    let watch = approve(&server, &mut laptop, &mut none).await;
    let chain = laptop.service.own_chain().await.unwrap();
    let removal = sync_key_envelope::v2::next_link(
        &laptop.credentials.keys,
        &chain.head(),
        LinkKind::Remove {
            device: watch.id(),
            keep: Default::default(),
        },
    );
    laptop.service.append_links(vec![removal]).await.unwrap();
    // One envelope fits a push of this size; two don't.
    let limit = 2_000;
    let before = workspace.policy_revision();
    laptop
        .service
        .record_heads_within(&mut workspace, &[], limit)
        .await
        .expect("recorded in steps");
    assert_eq!(
        workspace.policy_revision(),
        before + 2,
        "the phone's link, then the head that ends in the removal"
    );
    let first = workspace.policy().change(before + 1).unwrap();
    assert_eq!(first.body.heads.iter().next().unwrap().seqno, 2);
    let snapshot = workspace.policy().head_snapshot();
    assert!(snapshot.devices.contains_key(&phone.id()));
    assert!(snapshot.devices.contains_key(&tablet.id()));
    assert!(!snapshot.devices.contains_key(&watch.id()));
    assert_eq!(
        snapshot.members[&laptop.credentials.account].head,
        laptop.service.own_chain().await.unwrap().head(),
        "the workspace reaches the latest head"
    );
}

/// A server that tampers with a recipient's envelope, or a rotator that
/// lists a recipient the policy doesn't authorize, fails the client's key
/// checks with `sync_key_mismatch` (§9, §30).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tampered_envelope_or_a_wrong_recipient_list_fails_the_key_check() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "cai@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut phone = Desktop::connect(
        &proxy.base,
        phone.credentials.clone(),
        phone.secrets.clone(),
        phone.state,
    )
    .await;
    let mut remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();

    // The proxy flips a byte of the phone's envelope.
    proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/keys/current") {
            return body;
        }
        let mut keys = KeysResponse::from_bytes(&body).unwrap();
        for entry in keys.entries.0.iter_mut() {
            if let KeyRecord::Envelope(envelope) = &mut entry.record {
                let mut bytes = envelope.body.ciphertext.as_slice().to_vec();
                bytes[0] ^= 1;
                envelope.body.ciphertext = sync_protocol::codec::ExactBytes::new(bytes).unwrap();
            }
        }
        keys.to_bytes()
    }));
    let refused = phone
        .service
        .refresh_keys(&mut remote)
        .await
        .expect_err("tampered");
    assert!(
        matches!(refused, SyncError::Check("sync_key_mismatch")),
        "{refused:?}"
    );
    proxy.honest();

    // An epoch whose recipients name a device the policy doesn't count, as
    // a rotator colluding with the server would commit it.
    let readers: BTreeSet<Id> = [laptop.id(), phone.id(), Id([7; 16])].into_iter().collect();
    let latest = server
        .store
        .epochs(
            Session::new(laptop.credentials.account, laptop.id()),
            workspace.workspace,
            workspace.domain,
            1,
        )
        .await
        .unwrap()
        .epochs
        .0
        .remove(0)
        .0;
    let laptop_session = Session::new(laptop.credentials.account, laptop.id());
    let first_key = server
        .store
        .current_keys(laptop_session, workspace.workspace, laptop.id())
        .await
        .unwrap()
        .entries
        .0
        .into_iter()
        .find_map(|e| match e.record {
            KeyRecord::Envelope(envelope) => Some(envelope),
            _ => None,
        })
        .map(|envelope| {
            sync_key_envelope::v2::unwrap(
                &envelope.body,
                &laptop.credentials.keys,
                &laptop.credentials.card(),
            )
            .unwrap()
        })
        .expect("the laptop's envelope");
    let (revision, head) = workspace.policy().head();
    let key = SecretKey::random().unwrap();
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: workspace.workspace,
                domain: workspace.domain,
                epoch: 2,
                previous: latest.digest(),
                policy_revision: revision,
                policy_head: head,
                rotating_device: laptop.id(),
                recipients: readers.clone(),
            },
            &key,
            Some(&first_key),
            sync_protocol::crypto::random_nonce().unwrap(),
        ),
        laptop.credentials.keys.signing(),
    );
    let envelope = sync_key_envelope::v2::wrap(
        &sync_key_envelope::v2::EnvelopeHeader {
            workspace: workspace.workspace,
            domain: workspace.domain,
            epoch: 2,
            wrapping_device: laptop.id(),
        },
        &key,
        &phone.credentials.card(),
        &laptop.credentials.keys,
    )
    .unwrap();
    let (w, d) = (workspace.workspace, workspace.domain);
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                use sync_store::WriteTxn as _;
                let bytes = epoch.to_bytes();
                let seq = txn.append_entry(w, d, 8, &bytes).await?;
                txn.insert_epoch(
                    w,
                    d,
                    &sync_store::EpochRow {
                        epoch: 2,
                        seq,
                        digest: epoch.digest(),
                        previous: epoch.body.previous,
                        record: bytes.clone(),
                    },
                    revision,
                    &bytes,
                )
                .await?;
                txn.append_key_entry(w, 3, d, 2, None, &bytes).await?;
                txn.append_key_entry(
                    w,
                    4,
                    d,
                    2,
                    Some(envelope.body.recipient),
                    &envelope.to_bytes(),
                )
                .await?;
                Ok(())
            })
        }))
        .await
        .unwrap();
    let refused = phone
        .service
        .refresh_keys(&mut remote)
        .await
        .expect_err("recipients the policy doesn't authorize");
    assert!(
        matches!(refused, SyncError::Check("sync_key_mismatch")),
        "{refused:?}"
    );
}
