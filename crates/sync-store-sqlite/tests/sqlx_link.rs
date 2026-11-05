//! Link check for trial S0: sqlx runs a query on its bundled SQLite.

#[tokio::test]
async fn reports_the_bundled_sqlite_version() {
    let version = sync_store_sqlite::sqlite_version()
        .await
        .expect("in-memory database answers");

    assert!(version.starts_with("3."), "unexpected SQLite {version}");
}
