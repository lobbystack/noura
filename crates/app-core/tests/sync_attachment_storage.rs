//! Seam 1: the AGENTS.md storage matrix for applying attachments that sync
//! as blobs (sync-v2.md §21), against real workspace folders: an index
//! that refuses writes or is stale or gone, copies of an attachment, a
//! device that lost its journal and its file, and durable bytes before the
//! round returns. Every check reads the disk as soon as the call returns.
//! Malformed downloads, moves and two edits of one attachment are in
//! `sync_attachments.rs`; outside edits, path traversal and a changed
//! source at the write itself are in `local-core`'s `synced_files.rs`.

mod sync_support;

use std::path::PathBuf;

use app_core::sync::workspace::WorkspaceSync;
use sync_support::{Folder, Pair, pair};

/// `len` bytes that start with `head`, different for each `seed`.
fn attachment(head: &[u8], len: usize, seed: u8) -> Vec<u8> {
    let mut bytes = head.to_vec();
    let mut i: usize = seed.into();
    while bytes.len() < len {
        bytes.push((i.wrapping_mul(31).wrapping_add(seed.into()) % 251) as u8);
        i += 1;
    }
    bytes.truncate(len);
    bytes
}

/// The journal folder of `device` in `folder`.
fn journal(folder: &Folder, device: sync_protocol::types::Id) -> PathBuf {
    folder.path(".noura/sync").join(device.to_hex())
}

/// The files left in a journal's `downloads/` folder.
fn downloads(folder: &Folder, device: sync_protocol::types::Id) -> Vec<PathBuf> {
    std::fs::read_dir(journal(folder, device).join("downloads"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect()
}

/// The workspace's index files in the folder's app data.
fn index_files(folder: &Folder) -> Vec<PathBuf> {
    let id = folder.engine.manifest().id.clone();
    let dir = folder.app_data.path().join("workspaces").join(id);
    ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"]
        .into_iter()
        .map(|name| dir.join(name))
        .collect()
}

/// Whether the index lists `path`.
fn indexed(folder: &Folder, path: &str) -> bool {
    let connection = rusqlite::Connection::open(&index_files(folder)[0]).unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM files WHERE relative_path = ?1",
            [path],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_applies_while_the_index_refuses_writes_and_after_it_goes_stale() {
    let Pair {
        server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    // A copy of the desktop's index from before the attachment arrives.
    let stale = tempfile::tempdir().unwrap();
    for file in index_files(&b) {
        if file.exists() {
            std::fs::copy(&file, stale.path().join(file.file_name().unwrap())).unwrap();
        }
    }
    // The index refuses every change to its file list, as a full disk or a
    // broken index would, so its update stops midway.
    let refusing = rusqlite::Connection::open(&index_files(&b)[0]).unwrap();
    refusing
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();

    // A note travels with the attachment, so the index has something to
    // follow and fails midway.
    let pdf = attachment(b"%PDF-1.7\n", 700_000, 1);
    a.write("Docs/Report.pdf", &pdf);
    a.write("Docs/Notes.md", b"# Notes on the report\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the files apply though the index can't follow");
    assert!(b.files()["Docs/Report.pdf"] == pdf);
    assert_eq!(b.files()["Docs/Notes.md"], b"# Notes on the report\n");
    assert!(!indexed(&b, "Docs/Notes.md"), "the index update stopped");
    assert!(
        downloads(&b, desktop.id()).is_empty(),
        "no plaintext left behind"
    );
    refusing
        .execute_batch("DROP TRIGGER refuse_insert; DROP TRIGGER refuse_update;")
        .unwrap();
    drop(refusing);

    // The desktop quits, and its index goes back to the stale copy: on the
    // next launch the index finds the file again.
    let files = index_files(&b);
    drop(sync_b);
    let Folder {
        dir,
        app_data,
        engine,
        registry,
    } = b;
    drop(engine);
    drop(registry);
    for file in &files {
        let _ = std::fs::remove_file(file);
        let copy = stale.path().join(file.file_name().unwrap());
        if copy.exists() {
            std::fs::copy(&copy, file).unwrap();
        }
    }
    let mut b = reopened(dir, app_data);
    let mut sync_b = WorkspaceSync::open(
        b.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &b.registry,
    )
    .expect("the journal opens");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round over the stale index");
    assert!(indexed(&b, "Docs/Notes.md"), "the stale index caught up");
    assert!(b.files()["Docs/Report.pdf"] == pdf);
    assert_eq!(server.blob_files().len(), 1, "nothing uploaded again");
    drop(sync_b);

    // With no index at all, the rebuilt one lists it too, and sync goes on.
    drop(b.engine);
    let Folder { dir, app_data, .. } = b;
    for file in &files {
        let _ = std::fs::remove_file(file);
    }
    b = reopened(dir, app_data);
    assert!(indexed(&b, "Docs/Notes.md"), "the rebuilt index lists it");
    let mut sync_b = WorkspaceSync::open(
        b.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &b.registry,
    )
    .expect("the journal opens");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(b.files()["Docs/Report.pdf"] == pdf);
    assert_eq!(sync_b.status().conflicts, 0);
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(a.files() == b.files());
    assert_eq!(server.blob_files().len(), 1, "nothing uploaded again");
}

fn reopened(dir: tempfile::TempDir, app_data: tempfile::TempDir) -> Folder {
    let engine = std::sync::Arc::new(
        local_core::WorkspaceEngine::open_with_app_data(dir.path(), app_data.path())
            .expect("the workspace opens again"),
    );
    let registry = app_core::sync::registry::Registry::open(app_data.path()).expect("the registry");
    Folder {
        dir,
        app_data,
        engine,
        registry,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_of_an_attachment_reaches_a_device_that_holds_the_original() {
    let Pair {
        server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let pdf = attachment(b"%PDF-1.7\n", 600_000, 2);
    a.write("Report.pdf", &pdf);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(b.files()["Report.pdf"] == pdf);

    // Two more objects with the same bytes: a copy of the original, and the
    // same file added again under another name.
    a.write("Copies/Report copy.pdf", &pdf);
    a.write("Again.pdf", &pdf);
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(server.blob_files().len(), 3, "each new object has its blob");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the copies apply from the original's bytes");
    assert!(b.files()["Copies/Report copy.pdf"] == pdf);
    assert!(b.files()["Again.pdf"] == pdf);
    assert!(b.files()["Report.pdf"] == pdf);

    // Another round on each side changes nothing.
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(a.files() == b.files());
    assert!(downloads(&b, desktop.id()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_applies_when_the_original_changed_outside_sync() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let pdf = attachment(b"%PDF-1.7\n", 600_000, 3);
    a.write("Report.pdf", &pdf);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");

    // The laptop copies the file; meanwhile another app rewrites the
    // desktop's original with bytes of the same size and times, which the
    // desktop's walk can't tell from the original.
    a.write("Copy.pdf", &pdf);
    sync_a.round(&mut service, 0).await.expect("a round");
    let original = b.path("Report.pdf");
    let times = std::fs::metadata(&original).unwrap().modified().unwrap();
    let rewritten = attachment(b"%PDF-1.7\n", 600_000, 4);
    std::fs::write(&original, &rewritten).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&original)
        .unwrap()
        .set_modified(times)
        .unwrap();
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the copy downloads instead");
    assert!(b.files()["Copy.pdf"] == pdf);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_that_lost_its_journal_and_its_file_gets_its_own_attachment_back() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop,
        mut service,
        mut a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let png = attachment(b"\x89PNG\r\n\x1a\n", 1_200_000, 5);
    a.write("Images/Photo.png", &png);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");

    // The laptop quits, loses its journal and the file, and syncs again.
    drop(sync_a);
    std::fs::remove_dir_all(journal(&a, laptop.id())).unwrap();
    std::fs::remove_file(a.path("Images/Photo.png")).unwrap();
    a.reopen();
    let mut sync_a = WorkspaceSync::rebuild(&mut service, a.engine.clone(), &mut a.registry)
        .await
        .expect("the folder syncs again");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(
        a.files()["Images/Photo.png"] == png,
        "its own blob downloads"
    );
    assert!(downloads(&a, laptop.id()).is_empty());
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(a.files() == b.files());
}

#[test]
fn an_applied_attachment_and_its_folders_are_on_disk_before_the_round_returns() {
    // One thread runs everything, so the probe sees every folder the
    // round syncs.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = runtime.block_on(pair());
    let pdf = attachment(b"%PDF-1.7\n", 900_000, 6);
    a.write("Deep/Docs/Report.pdf", &pdf);
    runtime
        .block_on(sync_a.round(&mut service, 0))
        .expect("a round");
    let (result, synced) = local_core::durability_probe::record(|| {
        runtime.block_on(sync_b.round(&mut desktop.service, 0))
    });
    result.expect("a round");
    assert!(b.files()["Deep/Docs/Report.pdf"] == pdf);
    let root = b.engine.root().to_owned();
    for folder in [root.clone(), root.join("Deep"), root.join("Deep/Docs")] {
        assert!(
            synced.contains(&folder),
            "{} wasn't synced: {synced:?}",
            folder.display()
        );
    }
}
