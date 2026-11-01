//! Records that pass every check but that a reader can't use (seam 4,
//! sync-v2.md §9, §13, with `sync-v2-operation.json`'s rule for a delete
//! flag that doesn't match the payload): a record that doesn't open counts
//! as applied, the file keeps its last good state and the review lists an
//! unreadable version from its signer; an object at a `.noura-domain` path
//! that no domain names as its marker never reaches the disk.

mod sync_support;

use sync_support::{Pair, pair};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_doesnt_open_counts_as_applied_and_keeps_the_file() {
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
    let rid = sync_b.rid_at("Plan.md").expect("the plan syncs");
    sync_b.sign_unreadable(rid).expect("signed");
    sync_b
        .push(&mut desktop.service)
        .await
        .expect("the server can't see the problem");
    let round = sync_a.round(&mut service, 0).await.expect("applied");
    assert_eq!(round.applied, 1);
    assert_eq!(
        a.files()["Plan.md"],
        b"# Plan\n",
        "the last good state stays"
    );
    let unreadable = sync_a.unreadable();
    assert_eq!(unreadable.len(), 1);
    assert_eq!(unreadable[0].0, rid);
    assert_eq!(unreadable[0].1, desktop.id(), "attributed to its signer");
    assert_eq!(sync_a.status().unreadable, 1);
    // The cursor moved past it: the next round brings only new records.
    let round = sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(round.applied, 0);

    // A later state operation supersedes it.
    b.write("Plan.md", b"# Plan, readable again\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(a.files()["Plan.md"], b"# Plan, readable again\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_at_a_marker_path_that_no_domain_names_stays_off_disk() {
    let Pair {
        server: _server,
        proxy: _proxy,
        laptop: _laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b: _b,
        mut sync_b,
    } = pair().await;
    let rid = sync_b
        .sign_file_at(
            "Projects/.noura-domain",
            b"domain: AAAAAAAAAAAAAAAAAAAAAA\n",
        )
        .expect("signed");
    sync_b.push(&mut desktop.service).await.expect("pushed");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert!(!a.path("Projects/.noura-domain").exists());
    assert!(!a.path("Projects").exists());
    assert_eq!(sync_a.refused_objects(), &[rid]);
    assert_eq!(sync_a.status().refused, 1);
}
