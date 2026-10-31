//! The data directory: the SQLite database, the server secret beside it and
//! the log files (§14). `migrate` creates the secret on an empty database
//! and stores only its key check inside; `serve` and `migrate` refuse a
//! missing or different secret, and `serve` refuses pending migrations.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sync_protocol::ids::secret_check;
use sync_protocol::types::SecretKey;
use sync_store_sqlite::DATABASE_FILE;

/// The server secret's file in the data directory.
pub const SECRET_FILE: &str = "server-secret";
/// The log directory inside the data directory.
pub const LOG_DIR: &str = "logs";

/// Why a command refused to run.
#[derive(Debug)]
pub struct CommandError(pub String);

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CommandError {}

fn fail(message: impl Into<String>) -> CommandError {
    CommandError(message.into())
}

/// The database file in `data`.
pub fn database_path(data: &Path) -> PathBuf {
    data.join(DATABASE_FILE)
}

pub(crate) fn read_secret(path: &Path) -> Result<Option<SecretKey>, CommandError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(format!("can't read the server secret: {error}"))),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| fail(format!("can't read the server secret: {e}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| fail("the server secret file doesn't hold 32 bytes"))?;
    Ok(Some(SecretKey(bytes)))
}

/// Writes a new secret, readable by its owner alone, durably, and never
/// over an existing file.
fn write_secret(data: &Path, secret: &SecretKey) -> Result<(), CommandError> {
    write_secret_at(&data.join(SECRET_FILE), secret)
}

/// Writes a new secret file at `path`, readable by its owner alone,
/// durably, and never over an existing file.
pub(crate) fn write_secret_at(path: &Path, secret: &SecretKey) -> Result<(), CommandError> {
    let data = crate::restore::directory_of(path);
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let _ = fs::remove_file(&partial);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = (|| -> io::Result<()> {
        let mut file = options.open(&partial)?;
        file.write_all(secret.expose())?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a secret file appeared",
            ));
        }
        fs::rename(&partial, path)?;
        #[cfg(unix)]
        File::open(data)?.sync_all()?;
        Ok(())
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&partial);
        fail(format!("can't write the server secret: {e}"))
    })
}

fn matches(secret: &SecretKey, check: &[u8; 32]) -> bool {
    secret_check(secret).expose() == check
}

/// What `migrate` did.
#[derive(Debug, PartialEq, Eq)]
pub struct Migrated {
    /// The migrations it ran.
    pub applied: Vec<i64>,
    /// Whether it created the server secret.
    pub created_secret: bool,
}

/// `sync-server migrate`: creates the data directory and database when
/// missing and runs pending migrations. On a database without a key check
/// it first takes the secret file in the data directory, writing a new
/// secret when there is none, and stores the secret's key check. On a
/// database with a key check it refuses a missing or different secret.
pub async fn migrate(data: &Path) -> Result<Migrated, CommandError> {
    fs::create_dir_all(data).map_err(|e| fail(format!("can't create the data directory: {e}")))?;
    let database = database_path(data);
    let state = sync_store_sqlite::inspect(&database)
        .await
        .map_err(|e| fail(format!("can't read the database: {e}")))?;
    if !state.changed.is_empty() {
        return Err(fail(format!(
            "the database ran migrations this build doesn't carry: {:?}",
            state.changed
        )));
    }
    let secret = read_secret(&data.join(SECRET_FILE))?;
    let created_secret = match (&state.secret_check, secret) {
        (Some(check), Some(secret)) if matches(&secret, check) => false,
        (Some(_), Some(_)) => {
            return Err(fail(
                "the server secret doesn't match the key check the database holds",
            ));
        }
        (Some(_), None) => {
            return Err(fail(
                "the database holds a key check but the data directory holds no server secret",
            ));
        }
        (None, Some(_)) => false,
        (None, None) => {
            let secret = SecretKey::random().map_err(|e| fail(format!("no randomness: {e}")))?;
            write_secret(data, &secret)?;
            true
        }
    };
    let applied = sync_store_sqlite::migrate(&database)
        .await
        .map_err(|e| fail(format!("migration failed: {e}")))?;
    if state.secret_check.is_none() {
        let secret = read_secret(&data.join(SECRET_FILE))?
            .ok_or_else(|| fail("the server secret disappeared"))?;
        sync_store_sqlite::store_secret_check(
            &database,
            *secret_check(&secret).expose(),
            crate::restore::unix_now(),
        )
        .await
        .map_err(|e| fail(format!("can't store the secret's key check: {e}")))?;
    }
    Ok(Migrated {
        applied,
        created_secret,
    })
}

/// The checks `serve` runs before it starts: a migrated database with no
/// pending migration, and the secret its key check names.
pub async fn check_ready(data: &Path) -> Result<(), CommandError> {
    let state = sync_store_sqlite::inspect(&database_path(data))
        .await
        .map_err(|e| fail(format!("can't read the database: {e}")))?;
    let Some(check) = state.secret_check.filter(|_| state.exists) else {
        return Err(fail(
            "the data directory holds no migrated database; run `sync-server migrate` first",
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
    if state.stamped {
        return Err(fail(
            "this database is a backup copy; run `sync-server restore` or `mark-restored` first",
        ));
    }
    match read_secret(&data.join(SECRET_FILE))? {
        Some(secret) if matches(&secret, &check) => Ok(()),
        Some(_) => Err(fail(
            "the server secret doesn't match the key check the database holds",
        )),
        None => Err(fail("the data directory holds no server secret")),
    }
}
