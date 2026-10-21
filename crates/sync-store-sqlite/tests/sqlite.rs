//! What SQLite mode adds to the conformance suite (§15): write-ahead
//! logging with fully synchronous commits, one writer that commits queued
//! pushes in groups and checkpoints the log itself, and pulls that hold no
//! read transaction while they wait.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use sync_protocol::codec::List;
use sync_protocol::messages::{PullCursor, PullRequest};
use sync_protocol::types::Digest;
use sync_store::conformance::World;
use sync_store::{INITIAL_RESTORE_GENERATION, Store};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

async fn open() -> (Store<SqliteBackend>, PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join(DATABASE_FILE);
    sync_store_sqlite::migrate(&path).await.expect("migrated");
    sync_store_sqlite::store_secret_check(&path, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    let store = Store::open(SqliteBackend::open(&path).await.expect("opened"))
        .await
        .expect("store");
    (store, path, dir)
}

fn wal_size(path: &Path) -> u64 {
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    std::fs::metadata(wal).map_or(0, |m| m.len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_log_ahead_and_commit_fully_synchronously() {
    let (store, _, _dir) = open().await;
    let settings = store.backend().settings().await.expect("settings");
    assert_eq!(settings.journal_mode, "wal");
    assert_eq!(settings.synchronous, 2, "FULL");
    assert_eq!(settings.wal_autocheckpoint, 0, "the writer checkpoints");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_pushes_commit_together_with_a_savepoint_each() {
    let (store, _, _dir) = open().await;
    let world = Arc::new(World::new(store, 12).await);
    let before = world.store.backend().writer_counts();
    let mut tasks = Vec::new();
    for device in 0..12 {
        let world = world.clone();
        tasks.push(tokio::spawn(async move {
            let mut previous = Digest::ZERO;
            for counter in 1..=10u64 {
                let good = world.op(device, |op| {
                    op.counter = counter;
                    op.previous = previous;
                });
                // Every device's third push carries a record that fails
                // after a good one; that push commits nothing.
                if counter % 3 == 0 {
                    let bad = world.op(device, |op| {
                        op.counter = counter + 1;
                        op.previous = good.digest();
                        op.epoch = 9;
                    });
                    world
                        .push(device, &[good.clone(), bad])
                        .await
                        .expect_err("the failing record rolls its push back");
                }
                world
                    .push(device, std::slice::from_ref(&good))
                    .await
                    .expect("committed");
                previous = good.digest();
            }
        }));
    }
    for task in tasks {
        task.await.expect("finished");
    }
    let after = world.store.backend().writer_counts();
    let jobs = after.jobs - before.jobs;
    let transactions = after.transactions - before.transactions;
    assert_eq!(jobs, 12 * 13, "every push went through the one writer");
    assert!(
        transactions < jobs,
        "{jobs} pushes in {transactions} transactions: queued pushes commit in groups"
    );
    // Each device's ten good records committed, and none of the failures.
    let page = world.pull(0, 0).await;
    assert_eq!(page.head, 2 + 12 * 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_writer_keeps_the_write_ahead_log_bounded() {
    let (store, path, _dir) = open().await;
    let world = World::new(store, 1).await;
    let mut previous = Digest::ZERO;
    for counter in 1..=400u64 {
        let op = world.op(0, |op| {
            op.counter = counter;
            op.previous = previous;
            op.rid = sync_protocol::types::Id([7; 16]);
            op.supersedes = if counter == 1 {
                Default::default()
            } else {
                sync_protocol::codec::Sorted::new(vec![sync_protocol::records::Cover {
                    device: world.devices[0].id(),
                    counter: counter - 1,
                }])
                .expect("sorted")
            };
        });
        previous = op.digest();
        world.push(0, &[op]).await.expect("pushed");
    }
    let counts = world.store.backend().writer_counts();
    assert!(counts.checkpoints > 0, "the writer ran checkpoints");
    let size = wal_size(&path);
    assert!(
        size < 512 * 1024,
        "400 commits left a {size}-byte write-ahead log"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_pull_holds_no_read_transaction() {
    let (store, path, _dir) = open().await;
    let world = Arc::new(World::new(store, 1).await);
    world.push(0, &[world.op(0, |_| {})]).await.expect("pushed");
    let waiting = {
        let world = world.clone();
        tokio::spawn(async move {
            world
                .store
                .pull(
                    world.devices[0].session,
                    world.workspace,
                    PullRequest {
                        cursors: List(vec![PullCursor {
                            domain: world.domain,
                            after: 3,
                        }]),
                        wait_seconds: 4,
                    },
                )
                .await
                .expect("pull")
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!waiting.is_finished(), "the pull waits");
    // A truncating checkpoint resets the log only when no reader holds a
    // snapshot in it.
    let mut connection = SqliteConnectOptions::new()
        .filename(&path)
        .busy_timeout(Duration::from_secs(1))
        .connect()
        .await
        .expect("connects");
    let (busy, _, _): (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut connection)
        .await
        .expect("checkpoint");
    connection.close().await.expect("closed");
    assert_eq!(busy, 0, "no read transaction stays open while a pull waits");
    assert_eq!(wal_size(&path), 0);
    let response = waiting.await.expect("the pull returns");
    assert!(response.pages[0].entries.is_empty());
}
