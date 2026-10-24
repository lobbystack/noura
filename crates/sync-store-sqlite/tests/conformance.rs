//! The store conformance suite on SQLite. One instance serves a SQLite
//! database, so the cases that race two instances race it with itself.

use std::sync::Arc;

use sync_store::{INITIAL_RESTORE_GENERATION, Store};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

/// A fresh migrated database in a temporary directory.
async fn open() -> (Store<SqliteBackend>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join(DATABASE_FILE);
    sync_store_sqlite::migrate(&path).await.expect("migrated");
    sync_store_sqlite::store_secret_check(&path, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check stored");
    let backend = SqliteBackend::open(&path).await.expect("opened");
    let store = Store::open(backend).await.expect("store");
    (store, dir)
}

/// The one instance, twice.
async fn open_pair() -> (
    Arc<Store<SqliteBackend>>,
    Arc<Store<SqliteBackend>>,
    tempfile::TempDir,
) {
    let (store, dir) = open().await;
    let store = Arc::new(store);
    (store.clone(), store, dir)
}

sync_store::conformance_tests!(open);
sync_store::instance_tests!(open_pair);
