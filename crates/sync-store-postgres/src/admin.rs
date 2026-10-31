//! What `sync-server migrate` and `serve` ask of a PostgreSQL database:
//! which migrations it lacks, the key check of the server secret it holds,
//! whether it dates commits, and running the migrations.

use sqlx::migrate::{Migrate, MigrateError, Migrator};
use sqlx::{Connection, PgConnection};

use crate::connect;

/// The migrations in `migrations/`, embedded at build time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// A database's migrations and secret check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseState {
    /// Migrations this build carries that the database hasn't run.
    pub pending: Vec<i64>,
    /// Migrations the database ran that this build lacks or carries with
    /// other contents.
    pub changed: Vec<i64>,
    /// The key check of the server secret, once the first `migrate` stored
    /// it.
    pub secret_check: Option<[u8; 32]>,
    /// The restore generation, once stored.
    pub restore_generation: Option<u64>,
}

async fn table_exists(connection: &mut PgConnection, name: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
             WHERE table_schema = current_schema() AND table_name = $1
           ) AS "exists!""#,
        name
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(row.exists)
}

fn migrate_error(error: MigrateError) -> sqlx::Error {
    match error {
        MigrateError::Execute(error) => error,
        other => sqlx::Error::Protocol(other.to_string()),
    }
}

/// Reads a database's state without changing it. An empty database has
/// every migration pending.
pub async fn inspect(url: &str) -> Result<DatabaseState, sqlx::Error> {
    let up = || {
        MIGRATOR
            .iter()
            .filter(|m| !m.migration_type.is_down_migration())
    };
    let mut connection = connect(url).await?;
    let applied = if table_exists(&mut connection, MIGRATIONS_TABLE).await? {
        connection
            .list_applied_migrations(MIGRATIONS_TABLE)
            .await
            .map_err(migrate_error)?
    } else {
        Vec::new()
    };
    let pending = up()
        .filter(|m| !applied.iter().any(|a| a.version == m.version))
        .map(|m| m.version)
        .collect();
    let changed = applied
        .iter()
        .filter(|a| {
            !up().any(|m| m.version == a.version && m.checksum.as_ref() == a.checksum.as_ref())
        })
        .map(|a| a.version)
        .collect();
    let (secret_check, restore_generation) = if table_exists(&mut connection, "server").await? {
        let row = sqlx::query!("SELECT restore_generation, secret_check FROM server WHERE id = 1")
            .fetch_optional(&mut connection)
            .await?;
        match row {
            Some(row) => (
                Some(row.secret_check.try_into().map_err(|_| {
                    sqlx::Error::Protocol("the stored secret check isn't 32 bytes".into())
                })?),
                Some(u64::try_from(row.restore_generation).map_err(|_| {
                    sqlx::Error::Protocol("the stored restore generation is negative".into())
                })?),
            ),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    connection.close().await?;
    Ok(DatabaseState {
        pending,
        changed,
        secret_check,
        restore_generation,
    })
}

/// Runs every pending migration under sqlx's migration lock, so instances
/// that migrate at once run each migration once. Returns the versions this
/// call ran.
pub async fn migrate(url: &str) -> Result<Vec<i64>, sqlx::Error> {
    let mut connection = connect(url).await?;
    let before: Vec<i64> = if table_exists(&mut connection, MIGRATIONS_TABLE).await? {
        connection
            .list_applied_migrations(MIGRATIONS_TABLE)
            .await
            .map_err(migrate_error)?
            .into_iter()
            .map(|m| m.version)
            .collect()
    } else {
        Vec::new()
    };
    MIGRATOR.run(&mut connection).await.map_err(migrate_error)?;
    let after = connection
        .list_applied_migrations(MIGRATIONS_TABLE)
        .await
        .map_err(migrate_error)?;
    connection.close().await?;
    Ok(after
        .into_iter()
        .map(|m| m.version)
        .filter(|v| !before.contains(v))
        .collect())
}

/// Stores the server secret's key check and the starting restore
/// generation, unless the database holds a key check already. Returns
/// whether it stored them.
pub async fn store_secret_check(
    url: &str,
    secret_check: [u8; 32],
    restore_generation: u64,
) -> Result<bool, sqlx::Error> {
    let mut connection = connect(url).await?;
    let check = &secret_check[..];
    let generation = i64::try_from(restore_generation)
        .map_err(|_| sqlx::Error::Protocol("the restore generation exceeds a BIGINT".into()))?;
    let done = sqlx::query!(
        "INSERT INTO server (id, restore_generation, secret_check) VALUES (1, $1, $2)
         ON CONFLICT (id) DO NOTHING",
        generation,
        check
    )
    .execute(&mut connection)
    .await?;
    connection.close().await?;
    Ok(done.rows_affected() == 1)
}

/// Whether the server records each commit's time, which would date every
/// record (§29).
pub async fn track_commit_timestamp(url: &str) -> Result<bool, sqlx::Error> {
    let mut connection = connect(url).await?;
    let row = sqlx::query!(r#"SELECT current_setting('track_commit_timestamp') AS "setting!""#)
        .fetch_one(&mut connection)
        .await?;
    connection.close().await?;
    Ok(row.setting == "on")
}

/// In one transaction, raises the restore generation to the larger of the
/// stored one plus 1 and `floor`, gives every completed blob that no
/// operation lists until `blobs_until`, so no collector takes one before
/// the check against the blob store reaches it, and returns the new
/// generation.
pub async fn raise_generation(url: &str, floor: u64, blobs_until: u64) -> Result<u64, sqlx::Error> {
    let floor = i64::try_from(floor)
        .map_err(|_| sqlx::Error::Protocol("the restore generation exceeds a BIGINT".into()))?;
    let blobs_until = i64::try_from(blobs_until)
        .map_err(|_| sqlx::Error::Protocol("the blob expiry exceeds a BIGINT".into()))?;
    let mut connection = connect(url).await?;
    let mut transaction = connection.begin().await?;
    let row = sqlx::query!(
        "UPDATE server SET restore_generation = GREATEST(restore_generation + 1, $1)
         WHERE id = 1 RETURNING restore_generation",
        floor
    )
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query!(
        "UPDATE blobs SET delete_after = $1
         WHERE complete AND NOT marked AND NOT EXISTS (
           SELECT 1 FROM blob_listings l
           WHERE l.workspace = blobs.workspace AND l.blob = blobs.blob
         )",
        blobs_until
    )
    .execute(&mut *transaction)
    .await?;
    // The caller checked that no lease is live.
    sqlx::query("DELETE FROM instances")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    connection.close().await?;
    u64::try_from(row.restore_generation)
        .map_err(|_| sqlx::Error::Protocol("the stored restore generation is negative".into()))
}

/// How many instances hold a lease past `now`.
pub async fn live_instances(url: &str, now: u64) -> Result<usize, sqlx::Error> {
    let mut connection = connect(url).await?;
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM instances WHERE lease_until > $1"#,
        now
    )
    .fetch_one(&mut connection)
    .await?;
    connection.close().await?;
    Ok(usize::try_from(row.count).unwrap_or(usize::MAX))
}
