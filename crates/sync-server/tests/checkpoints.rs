//! The checkpoint routes at seam 4 (§17, §26): `checkpoint`,
//! `checkpoints/{digest}`, `checkpoint-chain`, `live` and `manifest`, their
//! query parameters and the statuses of their refusals.

mod support;

use reqwest::StatusCode;
use support::Client;
use sync_protocol::manifest::{bucket_of, verify_buckets};
use sync_protocol::messages::{
    CheckpointChainResponse, CheckpointResponse, Entry, EntryRecord, LiveResponse,
    ManifestBucketsResponse, ManifestLeavesResponse, ManifestNodeResponse,
};
use sync_store::conformance::{World, commit_checkpoints};
use sync_store::{CheckpointInterval, INITIAL_RESTORE_GENERATION, Store};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

async fn world() -> (World<SqliteBackend>, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join(DATABASE_FILE);
    sync_store_sqlite::migrate(&path).await.expect("migrated");
    sync_store_sqlite::store_secret_check(&path, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    let store = Store::open(SqliteBackend::open(&path).await.expect("opened"))
        .await
        .expect("store")
        .with_checkpoint_interval(CheckpointInterval {
            entries: 3,
            weight: u64::MAX,
        });
    let world = World::new(store, 2).await;
    let address = sync_server::spawn(world.store.clone(), "127.0.0.1:0")
        .await
        .expect("serving");
    (world, format!("http://{address}"), dir)
}

#[track_caller]
fn refused(reply: &support::Reply, status: StatusCode, code: &str) {
    assert_eq!((reply.status, reply.error().code.as_str()), (status, code));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checkpoint_routes_parse_their_queries() {
    let (world, base, _dir) = world().await;
    let device = Client::new(base, Some(world.devices[1].bearer()));
    let w = world.workspace.to_base64url();
    let d = world.domain.to_base64url();
    let route = |rest: &str| format!("/v2/workspaces/{w}/domains/{d}/{rest}");

    refused(
        &device.get(&route("checkpoint")).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    let committed = commit_checkpoints(&world, 3).await;
    let (latest, latest_seq) = committed.last().expect("three").clone();
    let first = &committed[0].0;

    let response: CheckpointResponse = device.get(&route("checkpoint")).await.decode();
    assert_eq!(
        (response.seq, &response.bundle.checkpoint.0),
        (latest_seq, &latest)
    );
    let entry: Entry = device
        .get(&route(&format!(
            "checkpoints/{}",
            first.digest().to_base64url()
        )))
        .await
        .decode();
    assert!(matches!(&entry.record, EntryRecord::Checkpoint(b) if &b.checkpoint.0 == first));
    refused(
        &device.get(&route("checkpoints/not-a-digest")).await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );

    let (from, to) = (
        latest.digest().to_base64url(),
        first.digest().to_base64url(),
    );
    let chain: CheckpointChainResponse = device
        .get(&route(&format!("checkpoint-chain?from={from}&to={to}")))
        .await
        .decode();
    let numbers: Vec<u64> = chain.checkpoints.iter().map(|c| c.0.body.number).collect();
    assert_eq!(numbers, vec![3, 2, 1]);
    for query in [format!("from={from}"), format!("to={to}"), String::new()] {
        refused(
            &device
                .get(&route(&format!("checkpoint-chain?{query}")))
                .await,
            StatusCode::BAD_REQUEST,
            "bad_request",
        );
    }

    let at = latest.body.covered_seq;
    let live: LiveResponse = device.get(&route(&format!("live?at={at}"))).await.decode();
    assert_eq!(live.entries.len() as u64, latest.body.live_objects);
    refused(
        &device.get(&route("live")).await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );
    refused(
        &device
            .get(&route(&format!("live?at={at}&after=nonsense")))
            .await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );
    let compacted = device.get(&route(&format!("live?at={}", at - 1))).await;
    assert_eq!(compacted.error().code.as_str(), "cursor_compacted");

    let buckets: ManifestBucketsResponse = device
        .get(&route(&format!("manifest?at={at}&buckets=all")))
        .await
        .decode();
    verify_buckets(&buckets.buckets, &latest.body.manifest_root).expect("the root");
    refused(
        &device
            .get(&route(&format!("manifest?at={at}&buckets=7")))
            .await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );
    let top: ManifestNodeResponse = device
        .get(&route(&format!("manifest?at={at}&level=0&index=0")))
        .await
        .decode();
    assert_eq!(top.children.len(), 16);
    let EntryRecord::Operation(op) = &live.entries[0].record else {
        panic!("an operation");
    };
    let index = bucket_of(&op.body.rid);
    let leaves: ManifestLeavesResponse = device
        .get(&route(&format!("manifest?at={at}&level=3&index={index}")))
        .await
        .decode();
    assert!(leaves.leaves.iter().any(|l| l.rid == op.body.rid));
    for query in [
        format!("at={at}&level=4&index=0"),
        format!("at={at}&level=256&index=0"),
        format!("at={at}&level=1&index=16"),
        format!("at={at}&index=0"),
        format!("at={at}&level=0"),
        "level=0&index=0".to_owned(),
    ] {
        refused(
            &device.get(&route(&format!("manifest?{query}"))).await,
            StatusCode::BAD_REQUEST,
            "bad_request",
        );
    }
}
