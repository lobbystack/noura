//! The credential store's mark as an anchor (seam 4, sync-v2.md §8, §15):
//! a device that trusted a workspace only through a sibling's change, and
//! whose journal is lost after a link removed that sibling, trusts the
//! workspace again through the policy head its mark keeps. Without that
//! mark it reports `sync_workspace_unverified`.

mod sync_support;

use app_core::sync::workspace::WorkspaceSync;
use local_core::sync::SyncCredentials;
use sync_support::{Folder, Server, approve, first_desktop};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_journal_trusts_the_workspace_through_the_mark() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", b"# Plan\n");
    let mut sync_a = WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");

    // The desktop joins with no workspace in its offer, and trusts the
    // workspace through the laptop's change that records its head.
    let mut desktop = approve(&server, &mut laptop, &mut []).await;
    laptop
        .service
        .record_heads(sync_a.remote_mut().unwrap(), &[])
        .await
        .expect("the laptop records the desktop's head");
    let mut b = Folder::new("Copy");
    let mut sync_b = WorkspaceSync::download(
        &mut desktop.service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the desktop trusts the workspace through the laptop");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");

    // A third device removes the laptop, keeping the desktop, and records
    // nothing in the workspace yet.
    let mut tablet = approve(&server, &mut laptop, &mut []).await;
    tablet
        .service
        .remove_device(laptop.id(), &[desktop.id()], &mut [])
        .await
        .expect("removed");

    // The desktop loses its journal.
    drop(sync_b);
    std::fs::remove_dir_all(b.path(".noura/sync").join(desktop.id().to_hex())).unwrap();
    b.reopen();
    let workspace = sync_a.workspace();
    let mark_item = format!(
        "noura.sync.v2.mark.{}.{}",
        desktop.id().to_hex(),
        workspace.to_hex()
    );
    let mark = desktop.secrets.read_optional(&mark_item).unwrap().unwrap();

    // Without the mark, nothing anchors the workspace.
    desktop.secrets.write(&mark_item, "").unwrap();
    let error = WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
        .await
        .expect_err("unanchored");
    assert_eq!(error.code(), Some("sync_workspace_unverified"));

    // With it, the folder syncs again.
    desktop.secrets.write(&mark_item, &mark).unwrap();
    let mut sync_b =
        WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
            .await
            .expect("the mark anchors the workspace");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files()["Plan.md"], b"# Plan\n");
}
