//! The store conformance suite on SQLite.

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

sync_store::conformance_tests!(open);
