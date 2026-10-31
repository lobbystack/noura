//! What `sync-server migrate` and `serve` ask of the database: which
//! migrations it lacks, the key check of the server secret it holds, and
//! running the migrations.

use std::path::Path;

use sqlx::migrate::{Migrate, MigrateError, Migrator};
use sqlx::{Connection, SqliteConnection};

use crate::connect;

/// The migrations in `migrations/`, embedded at build time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// A database's migrations and secret check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseState {
    /// Whether the database file exists.
    pub exists: bool,
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
    /// Whether `backup` stamped this database, which `serve` then refuses.
    pub stamped: bool,
}

async fn table_exists(connection: &mut SqliteConnection, name: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM sqlite_master WHERE type = 'table' AND name = ?1"#,
        name
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(row.count > 0)
}

async fn column_exists(
    connection: &mut SqliteConnection,
    table: &str,
    column: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM pragma_table_info(?1) WHERE name = ?2"#,
        table,
        column
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(row.count > 0)
}

fn migrate_error(error: MigrateError) -> sqlx::Error {
    match error {
        MigrateError::Execute(error) => error,
        other => sqlx::Error::Protocol(other.to_string()),
    }
}

/// Reads a database's state without changing it. A missing file has every
/// migration pending.
pub async fn inspect(path: &Path) -> Result<DatabaseState, sqlx::Error> {
    let up = || {
        MIGRATOR
            .iter()
            .filter(|m| !m.migration_type.is_down_migration())
    };
    if !path.exists() {
        return Ok(DatabaseState {
            exists: false,
            pending: up().map(|m| m.version).collect(),
            changed: Vec::new(),
            secret_check: None,
            restore_generation: None,
            stamped: false,
        });
    }
    let mut connection = connect(path, false).await?;
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
    let stamped = if column_exists(&mut connection, "server", "backup_stamp").await? {
        sqlx::query!("SELECT backup_stamp FROM server WHERE id = 1")
            .fetch_optional(&mut connection)
            .await?
            .is_some_and(|row| row.backup_stamp != 0)
    } else {
        false
    };
    connection.close().await?;
    Ok(DatabaseState {
        exists: true,
        pending,
        changed,
        secret_check,
        restore_generation,
        stamped,
    })
}

/// Creates the database when it is missing and runs every pending
/// migration. Returns the versions it ran.
pub async fn migrate(path: &Path) -> Result<Vec<i64>, sqlx::Error> {
    let pending = inspect(path).await?.pending;
    let mut connection = connect(path, true).await?;
    MIGRATOR.run(&mut connection).await.map_err(migrate_error)?;
    connection.close().await?;
    Ok(pending)
}

/// Stores the server secret's key check and the starting restore
/// generation, unless the database holds a key check already. Returns
/// whether it stored them.
pub async fn store_secret_check(
    path: &Path,
    secret_check: [u8; 32],
    restore_generation: u64,
) -> Result<bool, sqlx::Error> {
    let mut connection = connect(path, false).await?;
    let check = &secret_check[..];
    let generation = i64::try_from(restore_generation)
        .map_err(|_| sqlx::Error::Protocol("the restore generation exceeds an INTEGER".into()))?;
    let done = sqlx::query!(
        "INSERT INTO server (id, restore_generation, secret_check) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO NOTHING",
        generation,
        check
    )
    .execute(&mut connection)
    .await?;
    connection.close().await?;
    Ok(done.rows_affected() == 1)
}

/// Copies the database to `target` with `VACUUM INTO`, which reads one
/// snapshot beside a running server, stamps the copy and flushes it. It then
/// truncates the write-ahead log, which grew during the copy because no
/// checkpoint can pass the copy's snapshot. A busy server just leaves the
/// log for its own next checkpoint.
pub async fn backup_into(database: &Path, target: &Path) -> Result<(), sqlx::Error> {
    let mut connection = connect(database, false).await?;
    let path = target.to_string_lossy().into_owned();
    sqlx::query("VACUUM INTO ?1")
        .bind(path)
        .execute(&mut connection)
        .await?;
    let mut copy = connect(target, false).await?;
    sqlx::query!("UPDATE server SET backup_stamp = 1 WHERE id = 1")
        .execute(&mut copy)
        .await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut copy)
        .await?;
    sqlx::query("PRAGMA journal_mode = DELETE")
        .execute(&mut copy)
        .await?;
    copy.close().await?;
    std::fs::File::open(target)
        .and_then(|file| file.sync_all())
        .map_err(sqlx::Error::Io)?;
    let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut connection)
        .await;
    connection.close().await
}

/// Raises the restore generation to the larger of the stored one plus 1
/// and `floor`, clears the stamp and returns the new generation.
pub async fn raise_generation(path: &Path, floor: u64) -> Result<u64, sqlx::Error> {
    let floor = i64::try_from(floor)
        .map_err(|_| sqlx::Error::Protocol("the restore generation exceeds an INTEGER".into()))?;
    let mut connection = connect(path, false).await?;
    let row = sqlx::query!(
        "UPDATE server SET restore_generation = MAX(restore_generation + 1, ?1), backup_stamp = 0
         WHERE id = 1 RETURNING restore_generation",
        floor
    )
    .fetch_one(&mut connection)
    .await?;
    connection.close().await?;
    u64::try_from(row.restore_generation)
        .map_err(|_| sqlx::Error::Protocol("the stored restore generation is negative".into()))
}

/// How many instances hold a lease past `now`. A missing database or one
/// from before leases holds none.
pub async fn live_instances(path: &Path, now: u64) -> Result<usize, sqlx::Error> {
    if !path.exists() {
        return Ok(0);
    }
    let mut connection = connect(path, false).await?;
    let count = if table_exists(&mut connection, "instances").await? {
        let now = i64::try_from(now).unwrap_or(i64::MAX);
        sqlx::query!(
            r#"SELECT COUNT(*) AS "count!: i64" FROM instances WHERE lease_until > ?1"#,
            now
        )
        .fetch_one(&mut connection)
        .await?
        .count
    } else {
        0
    };
    connection.close().await?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}
