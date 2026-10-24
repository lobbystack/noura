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
    connection.close().await?;
    Ok(DatabaseState {
        exists: true,
        pending,
        changed,
        secret_check,
        restore_generation,
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
