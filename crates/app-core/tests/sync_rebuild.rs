//! A folder that syncs without this device's journal (seam 4, sync-v2.md
//! §9, §15): after a lost, refused or rolled-back journal, or when you
//! connect a copied folder, the client matches local files to objects and
//! holds every file that differs from the live state for your review
//! instead of uploading it, holds the files beneath a local
//! `.noura-domain` file, and keeps listing the old review store.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::marks::Mark;
use app_core::sync::registry::Registry;
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::workspace::rebuild::HeldChoice;
use sync_support::{Folder, Pair, approve, pair};

/// Removes `device`'s journal from `folder`, as a lost journal would.
fn lose_journal(folder: &Folder, device: sync_protocol::types::Id) {
    std::fs::remove_dir_all(folder.path(".noura/sync").join(device.to_hex())).unwrap();
}

fn journal_file(folder: &Folder, device: sync_protocol::types::Id) -> std::path::PathBuf {
    folder
        .path(".noura/sync")
        .join(device.to_hex())
        .join("journal.json")
}

/// A start raises the reservation before anything signs in the new block,
/// so losing the journal again can't sign one of its counters twice (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_without_a_journal_stores_its_new_block() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        service: _service,
        a: _a,
        sync_a: _sync_a,
        mut desktop,
        mut b,
        sync_b,
    } = pair().await;
    let (workspace, domain) = (sync_b.workspace(), sync_b.domain());
    let reserved = |desktop: &sync_support::Desktop| {
        Mark::read(&*desktop.secrets(), &desktop.id(), &workspace)
            .unwrap()
            .unwrap()
            .domains[&domain]
            .reserved
    };
    let before = reserved(&desktop);
    drop(sync_b);
    lose_journal(&b, desktop.id());
    b.reopen();
    let _sync = WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
        .await
        .expect("the folder syncs again");
    assert!(reserved(&desktop) > before, "the store holds the new block");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_journal_matches_files_and_holds_the_ones_that_differ() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        mut b,
        mut sync_b,
    } = pair().await;
    a.write("Notes/Same.md", b"# Same\n");
    a.write("Other.png", b"png bytes");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files(), a.files());

    // The desktop quits, loses its journal, and its files change meanwhile.
    drop(sync_b);
    lose_journal(&b, desktop.id());
    b.write("Plan.md", b"# Plan, changed without a journal\n");
    std::fs::create_dir_all(b.path("Moved")).unwrap();
    std::fs::rename(b.path("Other.png"), b.path("Moved/Other.png")).unwrap();
    b.write("New.md", b"# New on the desktop\n");
    b.reopen();
    let error = WorkspaceSync::open(
        b.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &b.registry,
    )
    .expect_err("no journal");
    assert_eq!(error.code(), Some("sync_journal_lost"));
    let mut sync_b =
        WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
            .await
            .expect("the folder syncs again");
    let mut held: Vec<String> = sync_b.held().into_iter().map(|(_, p)| p).collect();
    held.sort();
    assert_eq!(held, ["Moved/Other.png", "Plan.md"]);
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    // The held versions didn't upload; the new file did, and the file that
    // matched stays as it was.
    assert_eq!(a.files()["Plan.md"], b"# Plan\n");
    assert_eq!(a.files()["Other.png"], b"png bytes");
    assert_eq!(a.files()["New.md"], b"# New on the desktop\n");
    assert_eq!(b.files()["Notes/Same.md"], b"# Same\n");

    // Keep this device's version of one, and the live state of the other.
    let held = sync_b.held();
    let plan = held.iter().find(|(_, p)| p == "Plan.md").unwrap().0;
    let other = held.iter().find(|(_, p)| p == "Moved/Other.png").unwrap().0;
    sync_b
        .resolve_held(plan, HeldChoice::KeepMine)
        .expect("kept mine");
    sync_b
        .resolve_held(other, HeldChoice::KeepLive)
        .expect("kept the live state");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(a.files()["Plan.md"], b"# Plan, changed without a journal\n");
    assert_eq!(b.files(), a.files());
    assert!(!b.files().contains_key("Moved/Other.png"));
    assert_eq!(sync_b.status().held, 0);
    assert_eq!(sync_a.status().conflicts, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeping_both_saves_a_managed_copy_with_a_new_id() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        mut b,
        sync_b,
    } = pair().await;
    let note = b"---\nid: note_01k00000000000000000000007\ntype: note\n---\n\n# Note\n";
    a.write("Note.md", note);
    sync_a.round(&mut service, 0).await.expect("a round");
    let mut sync_b = sync_b;
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    drop(sync_b);
    lose_journal(&b, desktop.id());
    let edited = b"---\nid: note_01k00000000000000000000007\ntype: note\n---\n\n# Note, edited\n";
    b.write("Note.md", edited);
    b.reopen();
    let mut sync_b =
        WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
            .await
            .expect("the folder syncs again");
    let (rid, path) = sync_b.held().pop().expect("the edited note is held");
    assert_eq!(path, "Note.md");
    sync_b
        .resolve_held(rid, HeldChoice::KeepBoth)
        .expect("kept both");
    assert_eq!(b.files()["Note.md"], note);
    let copy = String::from_utf8(b.files()["Note (2).md"].clone()).unwrap();
    assert!(copy.contains("# Note, edited"));
    assert!(!copy.contains("note_01k00000000000000000000007"), "{copy}");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_or_rolled_back_journal_is_refused() {
    let Pair {
        server,
        proxy: _proxy,
        mut laptop,
        mut service,
        a: _a,
        mut sync_a,
        mut desktop,
        mut b,
        mut sync_b,
    } = pair().await;
    let file = journal_file(&b, desktop.id());
    let old = std::fs::read(&file).unwrap();
    // A third device joins, so the workspace's policy moves on, and the
    // desktop accepts it.
    let _third = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().unwrap()),
    )
    .await;
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    let accepted = sync_b.policy_revision();
    assert!(accepted > 2);
    drop(sync_b);

    // A journal another program changed fails its keyed hash.
    let mut changed = std::fs::read(&file).unwrap();
    let last = changed.len() - 2;
    changed[last] ^= 1;
    std::fs::write(&file, &changed).unwrap();
    b.reopen();
    let error = WorkspaceSync::open(
        b.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &b.registry,
    )
    .expect_err("refused");
    assert_eq!(error.code(), Some("sync_journal_rollback"));

    // An older journal, as a restored backup would leave, lies below the
    // credential store's mark.
    std::fs::write(&file, &old).unwrap();
    let error = WorkspaceSync::open(
        b.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &b.registry,
    )
    .expect_err("refused");
    assert_eq!(error.code(), Some("sync_journal_rollback"));

    // The folder syncs again as after a lost journal, below the mark nothing.
    let mut sync_b =
        WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
            .await
            .expect("the folder syncs again");
    assert_eq!(sync_b.policy_revision(), accepted);
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(sync_b.status().held, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copied_folder_waits_until_you_connect_it_and_keeps_the_old_review() {
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
    // A conflict that waits on the desktop.
    a.write("Plan.md", b"# Plan, the laptop's\n");
    b.write("Plan.md", b"# Plan, the desktop's\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(sync_b.status().conflicts, 1);
    drop(sync_b);

    // You copy the folder in Finder and connect the copy.
    let copy = tempfile::tempdir().unwrap();
    copy_tree(b.dir.path(), copy.path());
    let engine = std::sync::Arc::new(
        local_core::WorkspaceEngine::open_with_app_data(copy.path(), b.app_data.path()).unwrap(),
    );
    let mut registry = Registry::open(b.app_data.path()).unwrap();
    let error = WorkspaceSync::open(
        engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &registry,
    )
    .expect_err("the copy isn't connected");
    assert!(matches!(error, SyncError::Invalid(_)), "{error:?}");
    let entry = registry.folders()[0].clone();
    registry
        .connect(&entry.server, entry.account, entry.workspace, copy.path())
        .unwrap();
    let error = WorkspaceSync::open(
        engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &registry,
    )
    .expect_err("its journal holds the old replica ID");
    assert_eq!(error.code(), Some("sync_folder_not_connected"));

    let mut sync_c = WorkspaceSync::rebuild(&mut desktop.service, engine.clone(), &mut registry)
        .await
        .expect("connected");
    // The old journal's conflict branch stays listed for review.
    let archived = sync_c.archived().to_vec();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].path, "Plan.md");
    assert!(
        copy.path()
            .join(".noura/sync/archive")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
    sync_c.resolve_archived(0, true).expect("restored");
    assert!(sync_c.archived().is_empty());
    let restored = std::fs::read(copy.path().join("Plan (2).md")).unwrap();
    assert_eq!(restored, b"# Plan, the laptop's\n");
    sync_c
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_beneath_a_local_marker_wait_until_you_confirm() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        mut b,
        sync_b,
    } = pair().await;
    drop(sync_b);
    lose_journal(&b, desktop.id());
    b.write("Private/.noura-domain", b"domain: AAAAAAAAAAAAAAAAAAAAAA\n");
    b.write("Private/Secret.md", b"# Secret\n");
    b.write("Open.md", b"# Open\n");
    b.reopen();
    let mut sync_b =
        WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
            .await
            .expect("the folder syncs again");
    let roots = sync_b.held_roots();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].0, "Private");
    assert_eq!(roots[0].1, vec!["Private/Secret.md".to_owned()]);
    assert_eq!(roots[0].2, 1, "one account would see it");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(a.files().contains_key("Open.md"));
    assert!(!a.files().contains_key("Private/Secret.md"));

    sync_b.confirm_held_root("Private").expect("confirmed");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(a.files()["Private/Secret.md"], b"# Secret\n");
    assert!(
        !a.files().contains_key("Private/.noura-domain"),
        "the marker file itself never uploads"
    );
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else if entry.file_name() != "lock" {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A held version stays in its file while other devices keep editing: what
/// arrives waits for review, and each choice still works on your version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_that_arrives_leaves_a_held_version_in_place() {
    for (choice, ending, copy) in [
        (HeldChoice::KeepMine, &b"# Mine\n"[..], false),
        (HeldChoice::KeepLive, &b"# Theirs\n"[..], false),
        (HeldChoice::KeepBoth, &b"# Theirs\n"[..], true),
    ] {
        let Pair {
            server: _server,
            proxy: _proxy,
            laptop: _laptop,
            mut service,
            a,
            mut sync_a,
            mut desktop,
            mut b,
            sync_b,
        } = pair().await;
        drop(sync_b);
        lose_journal(&b, desktop.id());
        b.write("Plan.md", b"# Mine\n");
        b.reopen();
        let mut sync_b =
            WorkspaceSync::rebuild(&mut desktop.service, b.engine.clone(), &mut b.registry)
                .await
                .expect("the folder syncs again");
        sync_b
            .round(&mut desktop.service, 0)
            .await
            .expect("a round");
        let (rid, path) = sync_b.held().pop().expect("the file is held");
        assert_eq!(path, "Plan.md");
        let store = b
            .path(".noura/sync")
            .join(desktop.id().to_hex())
            .join("held")
            .join(rid.to_hex());
        let kept = sync_protocol::crypto::hash(b"# Mine\n").to_hex();
        assert_eq!(std::fs::read(store.join(&kept)).unwrap(), b"# Mine\n");

        a.write("Plan.md", b"# Theirs\n");
        sync_a.round(&mut service, 0).await.expect("a round");
        sync_b
            .round(&mut desktop.service, 0)
            .await
            .expect("a round");
        assert_eq!(b.files()["Plan.md"], b"# Mine\n", "your version stays");
        assert_eq!(sync_b.status().held, 1);

        sync_b.resolve_held(rid, choice).expect("you choose");
        assert_eq!(b.files()["Plan.md"], ending);
        assert!(!store.join(&kept).exists(), "the copy goes once you chose");
        if copy {
            assert_eq!(b.files()["Plan (2).md"], b"# Mine\n");
        }
        for _ in 0..2 {
            sync_b
                .round(&mut desktop.service, 0)
                .await
                .expect("a round");
            sync_a.round(&mut service, 0).await.expect("a round");
        }
        assert_eq!(a.files(), b.files());
        assert_eq!(a.files()["Plan.md"], ending);
    }
}

/// A failed rebuild keeps the review list: the network work that can fail
/// first runs before the journal moves aside, and a failure after that
/// reads the list back from the archive (§15).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebuild_that_fails_keeps_the_review_list() {
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
    // A conflict that waits on the laptop.
    a.write("Plan.md", b"# Plan, the laptop's\n");
    b.write("Plan.md", b"# Plan, the desktop's\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(sync_a.status().conflicts, 1);
    drop(sync_a);

    // The server can't be read: the journal stays where it was.
    proxy.lose(Box::new(|_, path| path.contains("/policy")));
    WorkspaceSync::rebuild(&mut service, a.engine.clone(), &mut a.registry)
        .await
        .expect_err("the policy didn't arrive");
    assert!(journal_file(&a, laptop.id()).exists(), "nothing moved");

    // The pull fails after the journal moved aside.
    proxy.honest();
    proxy.lose(Box::new(|_, path| path.contains("/pull")));
    WorkspaceSync::rebuild(&mut service, a.engine.clone(), &mut a.registry)
        .await
        .expect_err("the pull didn't arrive");

    proxy.honest();
    let mut sync_a = WorkspaceSync::rebuild(&mut service, a.engine.clone(), &mut a.registry)
        .await
        .expect("the folder syncs again");
    assert_eq!(sync_a.archived().len(), 1, "the review stays listed");
    sync_a.resolve_archived(0, false).expect("discarded");
    drop(sync_a);

    // A version you resolved stays resolved through another rebuild.
    let sync_a = WorkspaceSync::rebuild(&mut service, a.engine.clone(), &mut a.registry)
        .await
        .expect("the folder syncs again");
    assert!(sync_a.archived().is_empty());
}
