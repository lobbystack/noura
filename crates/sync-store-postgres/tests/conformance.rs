//! The store conformance suite on PostgreSQL: every case of ticket 44's
//! suite, `sync-v2-live.json` included, each in a fresh migrated database,
//! and the cases that race two instances on one database, each instance
//! waiting 200 ms after a write's policy read, before its commit. A missing
//! PostgreSQL fails these tests; they never skip.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sync_store::{INITIAL_RESTORE_GENERATION, Store};
use sync_store_postgres::PgBackend;
use sync_store_postgres::testing::{Cluster, TestDatabase};

/// A fresh, migrated database.
async fn database() -> TestDatabase {
    let database = Cluster::shared(Path::new(env!("CARGO_TARGET_TMPDIR")))
        .database()
        .await;
    sync_store_postgres::migrate(&database.url)
        .await
        .expect("migrated");
    sync_store_postgres::store_secret_check(&database.url, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    database
}

async fn instance(url: &str) -> Store<PgBackend> {
    Store::open(PgBackend::open(url).await.expect("opened"))
        .await
        .expect("store")
}

async fn open() -> (Store<PgBackend>, TestDatabase) {
    let database = database().await;
    (instance(&database.url).await, database)
}

/// Two instances on one database, with delays injected between each
/// push's policy read and its commit.
async fn open_pair() -> (Arc<Store<PgBackend>>, Arc<Store<PgBackend>>, TestDatabase) {
    let database = database().await;
    let first = instance(&database.url).await;
    let second = instance(&database.url).await;
    for store in [&first, &second] {
        store
            .backend()
            .inject_policy_read_delay(Some(Duration::from_millis(200)));
    }
    (Arc::new(first), Arc::new(second), database)
}

sync_store::conformance_tests!(open);
sync_store::instance_tests!(open_pair);
