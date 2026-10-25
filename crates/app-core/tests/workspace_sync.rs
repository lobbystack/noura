//! The desktop sync client against the real server on SQLite and on
//! PostgreSQL (seam 4, sync-v2.md §13 to §16, §25): two desktops of one
//! account keep a workspace's files in step on temporary folders.

mod sync_support;

use app_core::sync::workspace::{WorkspaceSync, synced_workspaces};
use sync_store::Backend;
use sync_support::{Folder, Server, approve, first_desktop};

const PLAN: &[u8] = b"---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\ncreated: 2026-10-24T12:00:00Z\nupdated: 2026-10-24T12:00:00Z\n---\n# Plan\n\nThe first draft.\n";
const PLAN_EDITED: &[u8] = b"---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\ncreated: 2026-10-24T12:00:00Z\nupdated: 2026-10-24T12:05:00Z\n---\n# Plan\n\nThe second draft, longer than the first.\n";
const PLAN_ID: &str = "note_01j9z3y7q8r2s4t6v8w0x2y4z6";
const PHOTO: &[u8] = &[0x89, b'P', b'N', b'G', 1, 2, 3, 4, 5];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_desktops_keep_a_workspace_in_step() {
    two_desktops(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_desktops_keep_a_workspace_in_step_on_postgres() {
    two_desktops(Server::start_postgres().await).await;
}

async fn two_desktops<B: Backend>(server: Server<B>) {
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("Notes/Plan.md", PLAN);
    a.write("photo.png", PHOTO);
    a.write("Notes/Old.md", b"# Old\n");

    let mut sync_a = WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    let round = sync_a
        .round(&mut laptop.service, 0)
        .await
        .expect("the first round uploads the folder");
    assert_eq!(round.captured, 3);
    assert_eq!(round.pushed, 3);
    assert_eq!(sync_a.status().pending, 0);

    // A second desktop of the account, approved from the laptop.
    let mut desktop = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let listed = synced_workspaces(&mut desktop.service)
        .await
        .expect("the workspaces list");
    assert_eq!(listed, vec![(sync_a.workspace(), "Field notes".to_owned())]);

    let mut b = Folder::new("Downloaded");
    let mut sync_b = WorkspaceSync::download(
        &mut desktop.service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    assert_eq!(sync_b.name(), "Field notes");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the download pulls every file");
    assert_eq!(b.files(), a.files());
    let note = b
        .engine
        .get_object(PLAN_ID)
        .unwrap()
        .expect("the index holds the note");
    assert_eq!(note.relative_path, "Notes/Plan.md");

    // Creates, edits, moves, renames and deletes on the laptop.
    a.write("Notes/Plan.md", PLAN_EDITED);
    std::fs::create_dir_all(a.path("Pictures")).unwrap();
    std::fs::rename(a.path("photo.png"), a.path("Pictures/photo.png")).unwrap();
    std::fs::remove_file(a.path("Notes/Old.md")).unwrap();
    a.write("Notes/New.md", b"# New\n");
    let round = sync_a.round(&mut laptop.service, 0).await.expect("a round");
    assert_eq!(round.captured, 4, "an edit, a move, a delete and a create");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files(), a.files());

    // A managed note moved and renamed keeps its object.
    std::fs::rename(a.path("Notes/Plan.md"), a.path("Archive.md")).unwrap();
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files(), a.files());
    let note = b.engine.get_object(PLAN_ID).unwrap().expect("indexed");
    assert_eq!(note.relative_path, "Archive.md");

    // And the other way.
    b.write("Notes/From the desktop.md", b"# Hello\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    assert_eq!(a.files(), b.files());
    assert_eq!(sync_a.status().conflicts, 0);
    assert_eq!(sync_b.status().conflicts, 0);
    b.reopen();
}
