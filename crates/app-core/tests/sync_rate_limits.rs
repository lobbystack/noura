//! The real sync client against the server's rate limits (seam 4, §26): it
//! waits out `rate_limited` for as long as the server says and finishes the
//! request, on the core routes and on blob transfers alike, and it hands a
//! wait of more than a minute to its caller.

use std::sync::Arc;
use std::time::{Duration, Instant};

use app_core::sync::{SyncClient, SyncError};
use sync_protocol::types::{Id, SecretKey};
use sync_server::{Accounts, RateGate, RateLimits};
use sync_store::Limit;
use sync_store::conformance::World;

/// A server at `limits` and a world of one device on it.
async fn serve(
    limits: RateLimits,
) -> (
    World<sync_store_sqlite::SqliteBackend>,
    String,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = sync_server::open_store(&sync_server::Config::new(
        data,
        "127.0.0.1:0".parse().expect("an address"),
    ))
    .await
    .expect("the store opens");
    let world = World::new(store, 1).await;
    let address = sync_server::spawn_limited(
        world.store.clone(),
        "127.0.0.1:0",
        Accounts::local,
        Arc::new(sync_server::blobs::Objects::memory()),
        RateGate::new(limits, None),
    )
    .await
    .expect("serving");
    (world, format!("http://{address}"), dir)
}

fn wide() -> Limit {
    Limit {
        allowance: 1 << 40,
        window: Duration::from_secs(3600),
    }
}

fn limits(requests: Limit, bootstrap_bytes: Limit) -> RateLimits {
    RateLimits {
        pushes: wide(),
        push_bytes: wide(),
        upload_bytes: wide(),
        read_bytes: wide(),
        bootstrap_bytes,
        requests,
        sign_in: wide(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_waits_out_a_rate_limit_and_finishes() {
    let requests = Limit {
        allowance: 1,
        window: Duration::from_secs(1),
    };
    let (world, base, _dir) = serve(limits(requests, wide())).await;
    let mut client = SyncClient::connect(&base, &world.devices[0].bearer())
        .await
        .expect("connects");
    let started = Instant::now();
    for _ in 0..3 {
        client.workspaces().await.expect("the list comes back");
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the client waited one window, not more"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blob_upload_and_download_wait_out_their_budgets() {
    let bootstrap = Limit {
        allowance: 1 << 20,
        window: Duration::from_secs(1),
    };
    let mut rates = limits(wide(), bootstrap);
    rates.upload_bytes = bootstrap;
    let (world, base, dir) = serve(rates).await;
    let mut client = SyncClient::connect(&base, &world.devices[0].bearer())
        .await
        .expect("connects");

    // Three segments at 1 MiB a second, so each transfer spans windows.
    let plaintext = vec![9u8; 2_500_000];
    let (ciphertext, blob_ref) = sync_protocol::blob::encrypt_blob(
        &plaintext,
        Id::random().expect("random"),
        SecretKey::random().expect("random"),
    )
    .expect("encrypts");
    let file = dir.path().join("blob");
    std::fs::write(&file, &ciphertext).expect("written");
    client
        .upload_blob(
            world.workspace,
            world.domain,
            &app_core::sync::workspace::state::BlobMeta::of(&blob_ref),
            &file,
        )
        .await
        .expect("uploads");
    let read = client
        .download_blob(world.workspace, &blob_ref, None, dir.path())
        .await
        .expect("downloads");
    assert_eq!(std::fs::read(read).expect("read back"), plaintext);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_wait_reaches_the_caller() {
    // Windows end at fixed times counted from 1970, so an hour's can end
    // between the two requests. A thousand years' first ends in 2970.
    let requests = Limit {
        allowance: 1,
        window: Duration::from_secs(1000 * 365 * 24 * 3600),
    };
    let (world, base, _dir) = serve(limits(requests, wide())).await;
    let mut client = SyncClient::connect(&base, &world.devices[0].bearer())
        .await
        .expect("connects");
    client.workspaces().await.expect("the first request fits");
    let refused = client.workspaces().await;
    assert!(matches!(
        refused,
        Err(SyncError::Refused { code, status: 429, .. }) if code == "rate_limited"
    ));
}
