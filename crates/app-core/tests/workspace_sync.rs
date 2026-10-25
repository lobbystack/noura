//! The desktop sync client against the real server on SQLite and on
//! PostgreSQL (seam 4, sync-v2.md §13 to §16, §25): two desktops of one
//! account keep a workspace's files in step on temporary folders.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::workspace::{WorkspaceSync, synced_workspaces};
use sync_protocol::messages::EntryRecord;
use sync_store::Backend;
use sync_support::{Folder, Pair, Server, approve, cut_off, first_desktop, pair};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edits_made_offline_wait_in_the_outbox_across_a_relaunch() {
    let Pair {
        server: _server,
        proxy,
        laptop,
        mut service,
        mut a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let workspace = sync_a.workspace();
    let domain = sync_a.remote_mut().unwrap().domain;

    cut_off(&proxy);
    a.write("Plan.md", b"# Plan, written offline\n");
    let failed = sync_a.round(&mut service, 0).await;
    assert!(matches!(failed, Err(SyncError::Network(_))), "{failed:?}");
    let first = sync_a.outbox();
    assert_eq!(first.len(), 1, "the edit waits in the outbox");
    a.write("Second.md", b"# Second\n");
    assert!(sync_a.round(&mut service, 0).await.is_err());
    let both = sync_a.outbox();
    assert_eq!(both.len(), 2);
    assert_eq!(both[0], first[0], "a retry sends the same bytes");

    // Quit and relaunch, still offline: the outbox survives, and the next
    // record starts the block the new start reserved.
    drop(sync_a);
    a.reopen();
    let mut sync_a = WorkspaceSync::open(
        a.engine.clone(),
        laptop.credentials.clone(),
        laptop.secrets(),
        &a.registry,
    )
    .expect("the journal opens");
    assert_eq!(sync_a.outbox(), both);
    a.write("Third.md", b"# Third\n");
    assert_eq!(sync_a.capture().expect("captured"), 1);
    let all = sync_a.outbox();
    assert_eq!(all.len(), 3);
    assert_eq!(&all[..2], &both[..]);

    proxy.honest();
    let round = sync_a.round(&mut service, 0).await.expect("back online");
    assert_eq!(round.pushed, 3);
    assert!(sync_a.outbox().is_empty());
    let device = laptop.id();
    let head = service
        .client()
        .chain_head(workspace, domain, device)
        .await
        .expect("the head")
        .expect("one");
    assert_eq!(head.counter, 1_025, "a relaunch skips to the next block");
    assert_eq!(head.record_digest, all[2]);
    // The server holds the records in order, byte for byte.
    let mut stored = Vec::new();
    let mut counter = 1;
    while let Some(entry) = service
        .client()
        .chain_position(workspace, domain, device, counter)
        .await
        .expect("a position")
    {
        let EntryRecord::Operation(op) = entry.record else {
            panic!("an operation");
        };
        counter = op.body.counter + 1;
        stored.push(op.digest());
    }
    assert_eq!(&stored[stored.len() - 3..], &all[..]);

    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the copy catches up");
    assert_eq!(b.files(), a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pause_holds_across_a_relaunch_and_resume_uploads_what_queued() {
    let Pair {
        server: _server,
        proxy,
        laptop,
        mut service,
        mut a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    sync_a.set_paused(&mut a.registry, true).expect("paused");
    // Nothing goes out while paused, even when the server is gone.
    cut_off(&proxy);
    a.write("Paused.md", b"# Written while paused\n");
    let round = sync_a.round(&mut service, 0).await.expect("a paused round");
    assert_eq!(round.captured, 1);
    assert_eq!(round.pushed, 0);
    assert_eq!(sync_a.status().pending, 1);

    drop(sync_a);
    a.reopen();
    let mut sync_a = WorkspaceSync::open(
        a.engine.clone(),
        laptop.credentials.clone(),
        laptop.secrets(),
        &a.registry,
    )
    .expect("the journal opens");
    assert!(sync_a.paused(), "the pause holds across relaunch");
    proxy.honest();
    sync_a.round(&mut service, 0).await.expect("still paused");
    assert_eq!(sync_a.status().pending, 1);

    sync_a.set_paused(&mut a.registry, false).expect("resumed");
    let round = sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(round.pushed, 1);
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files(), a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_sync_off_stops_it_and_leaves_the_files() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop,
        mut service,
        mut a,
        sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let before = a.files();
    sync_a.turn_off(&mut a.registry).expect("sync turns off");
    assert_eq!(a.files(), before, "every file stays");
    // The files stay editable, and nothing reaches the other desktop.
    a.write("Plan.md", b"# Plan, edited with sync off\n");
    a.engine
        .write_synced("Local.md", b"# Local\n", &local_core::SyncedExpect::Absent)
        .expect("still writable");
    assert!(
        WorkspaceSync::open(
            a.engine.clone(),
            laptop.credentials.clone(),
            laptop.secrets(),
            &a.registry
        )
        .is_err(),
        "a folder whose sync is off doesn't sync at the next start"
    );
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files()["Plan.md"], b"# Plan\n");

    // Turning it on again continues the same workspace.
    let mut sync_a = WorkspaceSync::turn_on_again(
        a.engine.clone(),
        laptop.credentials.clone(),
        laptop.secrets(),
        &mut a.registry,
    )
    .expect("sync turns on again");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files(), a.files());
}
