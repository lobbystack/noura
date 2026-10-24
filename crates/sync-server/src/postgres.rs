//! PostgreSQL mode (§14, §15, §29): a database URL in place of the data
//! directory's SQLite file. Every instance gets the same server secret
//! through `NOURA_SERVER_SECRET` or a mounted file that
//! `NOURA_SERVER_SECRET_FILE` names, and the database holds only its key
//! check. `migrate` refuses to run without the secret, stores its key check
//! on an empty database and refuses a different secret afterwards; `serve`
//! also refuses pending migrations and a PostgreSQL that dates commits.

use std::ffi::OsString;
use std::path::PathBuf;

use sync_protocol::ids::secret_check;
use sync_protocol::types::{SecretKey, from_hex};
use sync_store::{INITIAL_RESTORE_GENERATION, Store};
use sync_store_postgres::PgBackend;

use crate::data::{CommandError, Migrated};

/// The variable that holds the server secret.
pub const SECRET_VARIABLE: &str = "NOURA_SERVER_SECRET";
/// The variable that names a file holding the server secret.
pub const SECRET_FILE_VARIABLE: &str = "NOURA_SERVER_SECRET_FILE";

/// A PostgreSQL database and the secret every instance shares.
#[derive(Clone, Debug)]
pub struct Postgres {
    /// The database's URL, such as
    /// `postgres://noura@db.internal:5432/sync?sslmode=require`.
    pub url: String,
    /// The server secret, when the instance was given one.
    pub secret: Option<SecretKey>,
}

fn fail(message: impl Into<String>) -> CommandError {
    CommandError(message.into())
}

const HOW: &str = "set NOURA_SERVER_SECRET to 64 hexadecimal digits, such as the output of \
                   `openssl rand -hex 32`, or NOURA_SERVER_SECRET_FILE to a file that holds them";

/// The server secret from its text form: 64 hexadecimal digits, with
/// surrounding whitespace ignored.
pub fn parse_secret(text: &str) -> Result<SecretKey, CommandError> {
    from_hex(text.trim())
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(SecretKey)
        .ok_or_else(|| {
            fail(format!(
                "the server secret isn't 64 hexadecimal digits; {HOW}"
            ))
        })
}

/// The server secret that `NOURA_SERVER_SECRET` or the file that
/// `NOURA_SERVER_SECRET_FILE` names holds, read through `variable`. Setting
/// both is refused, so no instance picks a different one than the others.
pub fn secret_from(
    variable: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<SecretKey>, CommandError> {
    let text = variable(SECRET_VARIABLE).filter(|v| !v.is_empty());
    let file = variable(SECRET_FILE_VARIABLE).filter(|v| !v.is_empty());
    match (text, file) {
        (Some(_), Some(_)) => Err(fail(format!(
            "set {SECRET_VARIABLE} or {SECRET_FILE_VARIABLE}, not both"
        ))),
        (Some(text), None) => {
            let text = text
                .into_string()
                .map_err(|_| fail(format!("{SECRET_VARIABLE} isn't text; {HOW}")))?;
            parse_secret(&text).map(Some)
        }
        (None, Some(path)) => {
            let path = PathBuf::from(path);
            let text = std::fs::read_to_string(&path).map_err(|e| {
                fail(format!(
                    "can't read the server secret from {}: {e}",
                    path.display()
                ))
            })?;
            parse_secret(&text).map(Some)
        }
        (None, None) => Ok(None),
    }
}

fn matches(secret: &SecretKey, check: &[u8; 32]) -> bool {
    secret_check(secret).expose() == check
}

fn the_secret(postgres: &Postgres) -> Result<&SecretKey, CommandError> {
    postgres
        .secret
        .as_ref()
        .ok_or_else(|| fail(format!("PostgreSQL mode got no server secret; {HOW}")))
}

fn unreachable(error: impl std::fmt::Display) -> CommandError {
    fail(format!("can't read the database: {error}"))
}

/// `sync-server migrate` in PostgreSQL mode: runs pending migrations, then
/// stores the secret's key check on a database that holds none. It refuses
/// a missing secret, and a secret that doesn't match the stored check.
/// Instances that migrate at once run each migration once, and only one
/// key check is ever stored.
pub async fn migrate(postgres: &Postgres) -> Result<Migrated, CommandError> {
    let secret = the_secret(postgres)?;
    let state = sync_store_postgres::inspect(&postgres.url)
        .await
        .map_err(unreachable)?;
    if !state.changed.is_empty() {
        return Err(fail(format!(
            "the database ran migrations this build doesn't carry: {:?}",
            state.changed
        )));
    }
    if let Some(check) = &state.secret_check
        && !matches(secret, check)
    {
        return Err(fail(
            "the server secret doesn't match the key check the database holds",
        ));
    }
    let applied = sync_store_postgres::migrate(&postgres.url)
        .await
        .map_err(|e| fail(format!("migration failed: {e}")))?;
    if state.secret_check.is_none() {
        sync_store_postgres::store_secret_check(
            &postgres.url,
            *secret_check(secret).expose(),
            INITIAL_RESTORE_GENERATION,
        )
        .await
        .map_err(|e| fail(format!("can't store the secret's key check: {e}")))?;
        // Another instance may have stored its own check first.
        let stored = sync_store_postgres::inspect(&postgres.url)
            .await
            .map_err(unreachable)?
            .secret_check;
        if !stored.is_some_and(|check| matches(secret, &check)) {
            return Err(fail(
                "the server secret doesn't match the key check the database holds",
            ));
        }
    }
    Ok(Migrated {
        applied,
        created_secret: false,
    })
}

/// The checks `serve` runs before it starts in PostgreSQL mode: a migrated
/// database with no pending migration, the secret its key check names, and
/// `track_commit_timestamp` off.
pub async fn check_ready(postgres: &Postgres) -> Result<(), CommandError> {
    let secret = the_secret(postgres)?;
    let state = sync_store_postgres::inspect(&postgres.url)
        .await
        .map_err(unreachable)?;
    let Some(check) = state.secret_check else {
        return Err(fail(
            "the database holds no migrated store; run `sync-server migrate` first",
        ));
    };
    if !state.changed.is_empty() {
        return Err(fail(format!(
            "the database ran migrations this build doesn't carry: {:?}",
            state.changed
        )));
    }
    if !state.pending.is_empty() {
        return Err(fail(format!(
            "migrations {:?} are pending; run `sync-server migrate` first",
            state.pending
        )));
    }
    if !matches(secret, &check) {
        return Err(fail(
            "the server secret doesn't match the key check the database holds",
        ));
    }
    let dates_commits = sync_store_postgres::track_commit_timestamp(&postgres.url)
        .await
        .map_err(unreachable)?;
    if dates_commits {
        return Err(fail(
            "PostgreSQL's track_commit_timestamp is on, which dates every record the server \
             stores; turn it off and restart PostgreSQL",
        ));
    }
    Ok(())
}

/// Opens the store on a ready PostgreSQL database, with the server secret
/// that account IDs derive from.
pub async fn open_store(postgres: &Postgres) -> Result<Store<PgBackend>, CommandError> {
    check_ready(postgres).await?;
    let secret = the_secret(postgres)?.clone();
    let backend = PgBackend::open(&postgres.url)
        .await
        .map_err(|e| fail(format!("can't open the database: {e}")))?;
    Ok(Store::open(backend)
        .await
        .map_err(|e| fail(format!("can't open the store: {e}")))?
        .with_secret(secret))
}
