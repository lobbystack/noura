//! The sync server's store on SQLite.

use sqlx::Connection;

/// Returns the version of the SQLite library sqlx bundles, read from an
/// in-memory database.
///
/// Trial S0 calls this so the static server binary really links SQLite.
/// Ticket 44 replaces it with the store.
pub async fn sqlite_version() -> Result<String, sqlx::Error> {
    let mut connection = sqlx::SqliteConnection::connect("sqlite::memory:").await?;
    let version = sqlx::query_scalar("select sqlite_version()")
        .fetch_one(&mut connection)
        .await?;
    connection.close().await?;
    Ok(version)
}
