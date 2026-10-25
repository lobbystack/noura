//! Conflicts and collisions between two desktops (seam 4, sync-v2.md §16,
//! with the live-set rules of `sync-v2-live.json`): when both change one
//! file, neither version is overwritten, the other one waits in each
//! device's review store and status counts the conflict until you choose.
//! Identical branches aren't a conflict, a path-only change and a
//! content-only change merge, an edit against a delete keeps the edit and
//! asks, and two objects whose paths collide end at distinct names.

mod sync_support;

use std::path::PathBuf;

use sync_protocol::types::Id;
use sync_support::{Folder, Pair, pair};

/// Every file in `device`'s review store of conflict branches.
fn review_store(folder: &Folder, device: Id) -> Vec<Vec<u8>> {
    let dir: PathBuf = folder
        .path(".noura/sync")
        .join(device.to_hex())
        .join("conflicts");
    let mut found = Vec::new();
    let mut stack = vec![dir];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                found.push(std::fs::read(entry.path()).unwrap());
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_edits_of_one_file_keep_both_until_you_choose() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    a.write("Plan.md", b"# Plan, as the laptop wrote it\n");
    b.write("Plan.md", b"# Plan, as the desktop wrote it\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");

    // Neither file got overwritten, and each device keeps the other one.
    assert_eq!(a.files()["Plan.md"], b"# Plan, as the laptop wrote it\n");
    assert_eq!(b.files()["Plan.md"], b"# Plan, as the desktop wrote it\n");
    assert_eq!(
        review_store(&a, laptop.id()),
        vec![b"# Plan, as the desktop wrote it\n".to_vec()]
    );
    assert_eq!(
        review_store(&b, desktop.id()),
        vec![b"# Plan, as the laptop wrote it\n".to_vec()]
    );
    assert_eq!(sync_a.status().conflicts, 1);
    assert_eq!(sync_b.status().conflicts, 1);

    // Another edit on the laptop covers only its own branch: the conflict
    // stays.
    a.write("Plan.md", b"# Plan, the laptop again\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(sync_b.status().conflicts, 1);
    assert_eq!(b.files()["Plan.md"], b"# Plan, as the desktop wrote it\n");

    // Choosing the desktop's version on the laptop covers every branch.
    let conflict = sync_a.conflicts().pop().expect("the conflict");
    let theirs = conflict
        .branches
        .iter()
        .find(|branch| branch.device == desktop.id())
        .expect("the desktop's branch")
        .key;
    sync_a.choose(conflict.rid, theirs).expect("chosen");
    assert_eq!(a.files()["Plan.md"], b"# Plan, as the desktop wrote it\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(sync_a.status().conflicts, 0);
    assert_eq!(sync_b.status().conflicts, 0);
    assert_eq!(a.files(), b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identical_edits_are_no_conflict_and_the_next_edit_covers_both() {
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
    a.write("Plan.md", b"# Plan, the same on both\n");
    b.write("Plan.md", b"# Plan, the same on both\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(sync_a.status().conflicts, 0);
    assert_eq!(sync_b.status().conflicts, 0);

    a.write("Plan.md", b"# Plan, then the laptop\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(b.files()["Plan.md"], b"# Plan, then the laptop\n");
    assert_eq!(sync_b.status().conflicts, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_and_an_edit_merge() {
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
    std::fs::rename(a.path("Plan.md"), a.path("Renamed.md")).unwrap();
    b.write("Plan.md", b"# Plan, edited on the desktop\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");

    let merged = std::collections::BTreeMap::from([(
        "Renamed.md".to_owned(),
        b"# Plan, edited on the desktop\n".to_vec(),
    )]);
    assert_eq!(a.files(), merged);
    assert_eq!(b.files(), merged);
    assert_eq!(sync_a.status().conflicts, 0);
    assert_eq!(sync_b.status().conflicts, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_against_a_delete_keeps_the_edit_and_asks() {
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
    std::fs::remove_file(a.path("Plan.md")).unwrap();
    b.write("Plan.md", b"# Plan, still wanted\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");

    assert_eq!(b.files()["Plan.md"], b"# Plan, still wanted\n");
    assert_eq!(
        a.files()["Plan.md"],
        b"# Plan, still wanted\n",
        "the edit comes back on the device that deleted it"
    );
    let conflict = sync_a.conflicts().pop().expect("the conflict asks");
    assert_eq!(conflict.branches.len(), 2);
    assert!(conflict.branches.iter().any(|b| b.path.is_none()));
    assert_eq!(sync_b.status().conflicts, 1);

    // Keeping the delete removes the file on both.
    let delete = conflict
        .branches
        .iter()
        .find(|b| b.path.is_none())
        .unwrap()
        .key;
    sync_a.choose(conflict.rid, delete).expect("chosen");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert!(a.files().is_empty());
    assert!(b.files().is_empty());
    assert_eq!(sync_b.status().conflicts, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_new_files_whose_paths_collide_end_at_distinct_names() {
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
    a.write("Notes/Idea.md", b"# The laptop's idea\n");
    b.write("notes/idea.md", b"# The desktop's idea\n");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");

    // Each device keeps its own folder's case, as a file system that
    // ignores case does; the files are the same.
    let folded = |files: std::collections::BTreeMap<String, Vec<u8>>| {
        files
            .into_iter()
            .map(|(path, bytes)| (path.to_lowercase(), bytes))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let files = a.files();
    assert_eq!(folded(files.clone()), folded(b.files()));
    // Another round changes nothing: no device moves a file back and forth.
    let before = (a.files(), b.files());
    let round = sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(round.captured, 0);
    let round = sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(round.captured, 0);
    assert_eq!((a.files(), b.files()), before);
    let ideas: Vec<&Vec<u8>> = files
        .iter()
        .filter(|(path, _)| path.to_lowercase().contains("idea"))
        .map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(ideas.len(), 2, "{:?}", files.keys().collect::<Vec<_>>());
    assert!(
        files.keys().any(|path| path.ends_with("dea (2).md")),
        "{:?}",
        files.keys().collect::<Vec<_>>()
    );
}
