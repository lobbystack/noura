//! Removing a device and signing out (seam 4, sync-v2.md §7, §9): one
//! `remove` link takes down the card and the cards it added unless you keep
//! one, the remover records the new head in every workspace and rotates, a
//! removed device answers `device_removed`, verifies who removed it and
//! can't read what comes after, and a device that signs out removes
//! itself, deletes its secrets and leaves the others to rotate.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::device::StoredDevice;
use sync_protocol::messages::{EntryRecord, PullCursor, PullRequest};
use sync_protocol::records::OpClass;
use sync_store::{Backend, Session};
use sync_support::{Server, approve, first_desktop};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_device_takes_down_its_cards_and_rotates() {
    removal(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_device_takes_down_its_cards_and_rotates_on_postgres() {
    removal(Server::start_postgres().await).await;
}

async fn removal<B: Backend>(server: Server<B>) {
    let mut laptop = first_desktop(&server, "ada@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let mut phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut phone_remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .expect("the phone reads the workspace");
    // The phone approves a tablet, whose card's parent is the phone.
    let tablet = approve(&server, &mut phone, std::slice::from_mut(&mut phone_remote)).await;
    laptop.service.refresh_policy(&mut workspace).await.unwrap();

    let preview = laptop
        .service
        .removal_preview(phone.id())
        .await
        .expect("a preview");
    assert_eq!(preview.target.device, phone.id());
    assert_eq!(
        preview.with_it.iter().map(|c| c.device).collect::<Vec<_>>(),
        vec![tablet.id()],
        "the tablet goes with the phone that added it"
    );
    assert_eq!(preview.keepable, vec![tablet.id()]);

    let removal = laptop
        .service
        .remove_device(phone.id(), &[], std::slice::from_mut(&mut workspace))
        .await
        .expect("removed");
    let mut removed = removal.removed.clone();
    removed.sort();
    let mut expected = vec![phone.id(), tablet.id()];
    expected.sort();
    assert_eq!(removed, expected);
    assert!(removal.changes.is_empty(), "they signed only account_heads");
    assert_eq!(workspace.epoch(), 2, "the remover rotated the domain");
    let devices = workspace.policy().head_snapshot().devices.clone();
    assert!(!devices.contains_key(&phone.id()) && !devices.contains_key(&tablet.id()));

    // The phone's session answers device_removed, and the phone verifies
    // from its chain that the laptop removed it.
    let refused = phone
        .service
        .pull_files(&mut phone_remote)
        .await
        .expect_err("removed");
    let SyncError::Refused { code, detail, .. } = refused else {
        panic!("a refusal");
    };
    assert_eq!(code, "device_removed");
    let by = phone
        .service
        .verify_removal(&detail.expect("a detail"))
        .await
        .expect("the removal verifies");
    assert_eq!(by.signer, laptop.id());
    assert_eq!(
        by.signer_fingerprint,
        laptop.credentials.card().fingerprint()
    );

    // What the laptop writes next is under the new epoch, whose key the
    // phone never got.
    laptop
        .service
        .push_file(&mut workspace, "After.md", b"# After the removal\n")
        .await
        .expect("written under epoch 2");
    let page = server
        .store
        .pull(
            Session::new(laptop.credentials.account, laptop.id()),
            workspace.workspace,
            PullRequest {
                cursors: sync_protocol::codec::List(vec![PullCursor {
                    domain: workspace.domain,
                    after: 0,
                }]),
                wait_seconds: 0,
            },
        )
        .await
        .unwrap()
        .pages
        .0
        .remove(0);
    let op = page
        .entries
        .iter()
        .rev()
        .find_map(|e| match &e.record {
            EntryRecord::Operation(op) => Some(op.clone()),
            _ => None,
        })
        .expect("the new operation");
    assert_eq!(op.body.epoch, 2);
    assert!(
        !phone_remote.holds_key(2),
        "the removed phone holds no key of epoch 2"
    );
    assert!(
        server
            .store
            .current_keys(
                Session::new(phone.credentials.account, phone.id()),
                workspace.workspace,
                phone.id()
            )
            .await
            .is_err(),
        "the server serves the removed card no key"
    );
    let _ = OpClass::State;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_device_stays_and_signing_out_removes_only_this_device() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "bea@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    assert!(
        laptop.service.is_last_device().await.unwrap(),
        "warns before the last sign-out"
    );
    let mut phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut phone_remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();
    let tablet = approve(&server, &mut phone, std::slice::from_mut(&mut phone_remote)).await;
    laptop.service.refresh_policy(&mut workspace).await.unwrap();
    assert!(!laptop.service.is_last_device().await.unwrap());

    // Keeping the tablet takes down the phone alone.
    let removal = laptop
        .service
        .remove_device(
            phone.id(),
            &[tablet.id()],
            std::slice::from_mut(&mut workspace),
        )
        .await
        .expect("removed");
    assert_eq!(removal.removed, vec![phone.id()]);
    assert!(
        workspace
            .policy()
            .head_snapshot()
            .devices
            .contains_key(&tablet.id())
    );

    // The tablet signs out: a remove link naming itself, then its secrets
    // leave the credential store.
    let mut tablet = tablet;
    let mut tablet_remote = tablet
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();
    let tablet_id = tablet.id();
    let tablet_secrets = tablet.secrets.clone();
    assert!(
        StoredDevice::load(&*tablet_secrets, &server.base)
            .unwrap()
            .is_some()
    );
    tablet
        .service
        .sign_out(std::slice::from_mut(&mut tablet_remote), &*tablet_secrets)
        .await
        .expect("signed out");
    assert!(
        StoredDevice::load(&*tablet_secrets, &server.base)
            .unwrap()
            .is_none(),
        "the keys and token are gone"
    );
    let chain = laptop.service.own_chain().await.unwrap();
    assert!(!chain.is_active_at(&tablet_id, chain.head_seqno()));
    assert!(chain.is_active_at(&laptop.id(), chain.head_seqno()));

    // The laptop records the head that takes the tablet out, then rotates.
    let pending = laptop.service.pending_heads(&workspace).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].own);
    assert!(pending[0].wait.end <= std::time::Duration::from_secs(60));
    assert!(
        laptop
            .service
            .push_pending_head(&mut workspace, &pending[0])
            .await
            .unwrap()
    );
    assert!(
        !workspace
            .policy()
            .head_snapshot()
            .devices
            .contains_key(&tablet_id)
    );
    assert_eq!(workspace.epoch(), 3, "a rotation after each removal");
    assert!(laptop.service.is_last_device().await.unwrap());
}
