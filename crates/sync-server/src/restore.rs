//! `backup`, `restore` and `mark-restored` (§14, §19). `backup` copies a
//! running SQLite database and stamps the copy so `serve` refuses it. The
//! other two raise the restore generation, clear the stamp and check the
//! blob store against the restored database.
//!
//! Both list the blob store before they raise the generation, while the
//! instance leases show that nothing serves the database, so the pass never
//! takes an object that a server started afterwards stored. `serve` may
//! start as soon as the command reports the new generation through its
//! `raised` callback, before the pass ends: the pass changes each blob's row
//! under the collector's locks, and looks at the store again before it
//! calls a blob lost.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sync_protocol::ids::secret_check;
use sync_protocol::types::{Id, SecretKey, from_hex};
use sync_store::{Backend, Store, UPLOAD_TTL};
use sync_store_postgres::PgBackend;
use sync_store_sqlite::SqliteBackend;

use crate::blobs::{SegmentStore, segment_key};
use crate::data::{self, CommandError, SECRET_FILE, database_path, read_secret};
use crate::postgres::Postgres;
use crate::{BlobStoreConfig, blobs};

/// The key prefix that holds objects the database doesn't know.
pub const QUARANTINE: &str = "quarantine";
/// How long a quarantined object stays before the collector deletes it.
pub const QUARANTINE_DAYS: u64 = 7;
const QUARANTINE_SECONDS: u64 = QUARANTINE_DAYS * 24 * 60 * 60;
/// How long a lease lasts past its last refresh.
pub const LEASE_SECONDS: u64 = 90;
/// How often a running instance refreshes its lease.
pub const LEASE_PERIOD: Duration = Duration::from_secs(30);
/// The extension of the secret file `backup` writes beside a copy.
pub const SECRET_EXTENSION: &str = "secret";

fn fail(message: impl Into<String>) -> CommandError {
    CommandError(message.into())
}

/// The current Unix time in seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The secret file beside the backup `copy`.
pub fn secret_beside(copy: &Path) -> PathBuf {
    let mut name = copy.as_os_str().to_owned();
    name.push(".");
    name.push(SECRET_EXTENSION);
    PathBuf::from(name)
}

fn matches(secret: &SecretKey, check: &[u8; 32]) -> bool {
    secret_check(secret).expose() == check
}

/// The directory that holds `path`: `.` for a bare file name.
pub(crate) fn directory_of(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn sync_parent(path: &Path) -> Result<(), CommandError> {
    #[cfg(unix)]
    {
        let parent = directory_of(path);
        fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| fail(format!("can't flush {}: {e}", parent.display())))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// What `restore` and `mark-restored` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// The new restore generation.
    pub generation: u64,
    /// The result of the check against the blob store.
    pub blobs: BlobCheck,
    /// Where `restore` kept an old database it couldn't read.
    pub set_aside: Option<PathBuf>,
}

/// What the check against the blob store did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlobCheck {
    /// Objects it moved under the quarantine prefix.
    pub quarantined: u64,
    /// Objects it moved back from quarantine.
    pub returned: u64,
    /// Completed blobs that no operation lists, given a fresh 24 hours.
    pub refreshed: u64,
    /// Listed blobs whose objects are gone, now uploads at offset 0.
    pub missing: u64,
}

/// `sync-server backup`: copies the database to `to` with `VACUUM INTO`
/// beside a running `serve`, stamps the copy and writes the secret beside
/// it. Returns the secret file's path.
pub async fn backup(data: &Path, to: &Path) -> Result<PathBuf, CommandError> {
    crate::data::check_ready(data).await?;
    let path = secret_beside(to);
    for taken in [to, path.as_path()] {
        if taken.exists() {
            return Err(fail(format!("{} already exists", taken.display())));
        }
    }
    let secret = read_secret(&data.join(SECRET_FILE))?
        .ok_or_else(|| fail("the data directory holds no server secret"))?;
    // The copy keeps a temporary name until it is stamped, so a failure
    // never leaves a file that looks like a finished backup.
    let mut partial = to.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let _ = fs::remove_file(&partial);
    if let Err(error) = sync_store_sqlite::backup_into(&database_path(data), &partial).await {
        let _ = fs::remove_file(&partial);
        return Err(fail(format!("the copy failed: {error}")));
    }
    if let Err(error) = data::write_secret_at(&path, &secret) {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    if let Err(error) = fs::rename(&partial, to) {
        let _ = fs::remove_file(&partial);
        let _ = fs::remove_file(&path);
        return Err(fail(format!("can't name the copy: {error}")));
    }
    sync_parent(to)?;
    Ok(path)
}

/// `sync-server backup` in PostgreSQL mode, which takes no copy.
pub fn backup_postgres() -> CommandError {
    fail(
        "in PostgreSQL mode, back the database up with its own tools, such as pg_dump or \
         pg_basebackup, and keep the server secret with each backup",
    )
}

/// A secret file's contents: 32 raw bytes as the data directory holds, or
/// 64 hexadecimal digits as PostgreSQL mode takes.
fn secret_file(path: &Path) -> Result<SecretKey, CommandError> {
    let bytes =
        fs::read(path).map_err(|e| fail(format!("can't read the server secret file: {e}")))?;
    if let Ok(raw) = <[u8; 32]>::try_from(bytes.as_slice()) {
        return Ok(SecretKey(raw));
    }
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| from_hex(text.trim()))
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .map(SecretKey)
        .ok_or_else(|| fail("the server secret file holds no 32-byte secret"))
}

/// Moves a database `restore` can't read, with its write-ahead log files,
/// to a side name, and returns that name.
fn set_aside(database: &Path) -> Result<PathBuf, CommandError> {
    let named = |suffix: &str| {
        let mut name = database.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let side = named(&format!(".damaged-{}", unix_now()));
    fs::rename(database, &side)
        .map_err(|e| fail(format!("can't set the damaged database aside: {e}")))?;
    for suffix in ["-wal", "-shm"] {
        let mut kept = side.as_os_str().to_owned();
        kept.push(suffix);
        match fs::rename(named(suffix), PathBuf::from(kept)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(fail(format!("can't set the write-ahead log aside: {e}"))),
        }
    }
    Ok(side)
}

/// The restore generation of the database `restore` replaces, after
/// refusing one a server holds. `None` in the second place says that the
/// file can't be read at all, so it shows no lease and names no generation.
async fn old_database(database: &Path) -> Result<(Option<u64>, bool), CommandError> {
    match sync_store_sqlite::live_instances(database, unix_now()).await {
        Ok(live) => refuse_serving(live)?,
        Err(error) if sync_store_sqlite::is_damaged(&error) => return Ok((None, true)),
        Err(error) => return Err(fail(format!("can't read the database: {error}"))),
    }
    match sync_store_sqlite::inspect(database).await {
        Ok(state) => Ok((state.restore_generation, false)),
        Err(error) if sync_store_sqlite::is_damaged(&error) => Ok((None, true)),
        Err(error) => Err(fail(format!("can't read the old database: {error}"))),
    }
}

async fn refuse_serving_sqlite(database: &Path) -> Result<(), CommandError> {
    let live = sync_store_sqlite::live_instances(database, unix_now())
        .await
        .map_err(|e| fail(format!("can't read the database: {e}")))?;
    refuse_serving(live)
}

fn refuse_serving(live: usize) -> Result<(), CommandError> {
    if live == 0 {
        return Ok(());
    }
    Err(fail(format!(
        "{live} running instance(s) hold a lease on this database; stop `serve` first. A lease \
         from an instance that crashed ends within {LEASE_SECONDS} seconds"
    )))
}

/// `sync-server restore`: takes the backup `from` and its secret into the
/// data directory `data`, the old one or a fresh one. It refuses a missing
/// secret or one that doesn't match the check the backup holds, and a
/// directory whose server still runs.
pub async fn restore(
    data: &Path,
    from: &Path,
    secret: Option<&Path>,
    blob_store: &BlobStoreConfig,
    raised: impl FnOnce(u64),
) -> Result<Restored, CommandError> {
    let beside = secret_beside(from);
    let secret_path = secret.unwrap_or(&beside);
    if !secret_path.exists() {
        return Err(fail(format!(
            "no server secret at {}; a restore needs the secret its backup was made with",
            secret_path.display()
        )));
    }
    let secret = secret_file(secret_path)?;
    let state = sync_store_sqlite::inspect(from)
        .await
        .map_err(|e| fail(format!("can't read the backup: {e}")))?;
    let (true, Some(check)) = (state.exists, state.secret_check) else {
        return Err(fail(format!("{} isn't a backup", from.display())));
    };
    if !matches(&secret, &check) {
        return Err(fail(
            "the server secret doesn't match the key check the backup holds",
        ));
    }
    fs::create_dir_all(data).map_err(|e| fail(format!("can't create the data directory: {e}")))?;
    let held = read_secret(&data.join(SECRET_FILE))?;
    if held.as_ref().is_some_and(|held| !matches(held, &check)) {
        return Err(fail(
            "the data directory holds another server secret than the backup's",
        ));
    }
    let database = database_path(data);
    let (previous, mut damaged) = old_database(&database).await?;

    let blobs = blob_store.open(data)?;
    let keys = blobs
        .list()
        .await
        .map_err(|e| fail(format!("can't list the blob store: {e}")))?;

    // The copy takes its new generation before it replaces the database,
    // and the old database folds its write-ahead log first, so a crash
    // leaves the whole old database or a finished one. Rerunning the command
    // repeats the job.
    let incoming = database.with_extension("restoring");
    let _ = fs::remove_file(&incoming);
    fs::copy(from, &incoming).map_err(|e| fail(format!("can't copy the backup: {e}")))?;
    let floor = unix_now().max(previous.map_or(0, |g| g.saturating_add(1)));
    let generation = sync_store_sqlite::raise_generation(&incoming, floor, unix_now() + UPLOAD_TTL)
        .await
        .map_err(|e| fail(format!("can't raise the restore generation: {e}")))?;
    fs::File::open(&incoming)
        .and_then(|file| file.sync_all())
        .map_err(|e| fail(format!("can't flush the copy: {e}")))?;
    if !damaged {
        match sync_store_sqlite::fold_log(&database).await {
            Ok(()) => {}
            Err(error) if sync_store_sqlite::is_damaged(&error) => damaged = true,
            Err(error) => {
                return Err(fail(format!("can't fold the old write-ahead log: {error}")));
            }
        }
    }
    let set_aside = if damaged {
        Some(set_aside(&database)?)
    } else {
        None
    };
    for suffix in ["-wal", "-shm"] {
        let mut name = database.as_os_str().to_owned();
        name.push(suffix);
        match fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(fail(format!("can't remove the write-ahead log: {e}"))),
        }
    }
    fs::rename(&incoming, &database)
        .map_err(|e| fail(format!("can't put the backup in place: {e}")))?;
    sync_parent(&database)?;
    if held.is_none() {
        data::write_secret_at(&data.join(SECRET_FILE), &secret)?;
    }
    raised(generation);

    let backend = SqliteBackend::open(&database)
        .await
        .map_err(|e| fail(format!("can't open the database: {e}")))?;
    let store = Store::open(backend)
        .await
        .map_err(|e| fail(format!("can't open the store: {e}")))?;
    let checked = check_blobs(&store, blobs.as_ref(), keys).await;
    store.backend().close().await;
    Ok(Restored {
        generation,
        blobs: checked?,
        set_aside,
    })
}

/// `sync-server mark-restored` on the SQLite database in `data`, which the
/// operator put back by hand or whose blob store lost objects.
pub async fn mark_restored(
    data: &Path,
    blob_store: &BlobStoreConfig,
    raised: impl FnOnce(u64),
) -> Result<Restored, CommandError> {
    let database = database_path(data);
    let state = sync_store_sqlite::inspect(&database)
        .await
        .map_err(|e| fail(format!("can't read the database: {e}")))?;
    let Some(check) = state.secret_check else {
        return Err(fail(
            "the data directory holds no migrated database; run `sync-server restore` on a backup",
        ));
    };
    match read_secret(&data.join(SECRET_FILE))? {
        Some(secret) if matches(&secret, &check) => {}
        Some(_) => {
            return Err(fail(
                "the server secret doesn't match the key check the database holds",
            ));
        }
        None => return Err(fail("the data directory holds no server secret")),
    }
    refuse_serving_sqlite(&database).await?;
    let blobs = blob_store.open(data)?;
    let keys = blobs
        .list()
        .await
        .map_err(|e| fail(format!("can't list the blob store: {e}")))?;
    let generation =
        sync_store_sqlite::raise_generation(&database, unix_now(), unix_now() + UPLOAD_TTL)
            .await
            .map_err(|e| fail(format!("can't raise the restore generation: {e}")))?;
    raised(generation);
    let backend = SqliteBackend::open(&database)
        .await
        .map_err(|e| fail(format!("can't open the database: {e}")))?;
    let store = Store::open(backend)
        .await
        .map_err(|e| fail(format!("can't open the store: {e}")))?;
    let checked = check_blobs(&store, blobs.as_ref(), keys).await;
    store.backend().close().await;
    Ok(Restored {
        generation,
        blobs: checked?,
        set_aside: None,
    })
}

/// `sync-server mark-restored` in PostgreSQL mode, after the operator
/// restored the database or failed over to a standby that may lack commits.
pub async fn mark_restored_postgres(
    postgres: &Postgres,
    data: &Path,
    blob_store: &BlobStoreConfig,
    raised: impl FnOnce(u64),
) -> Result<Restored, CommandError> {
    let secret = postgres
        .secret
        .as_ref()
        .ok_or_else(|| fail("PostgreSQL mode got no server secret"))?;
    let state = sync_store_postgres::inspect(&postgres.url)
        .await
        .map_err(|e| fail(format!("can't read the database: {e}")))?;
    let Some(check) = state.secret_check else {
        return Err(fail("the database holds no migrated store"));
    };
    if !matches(secret, &check) {
        return Err(fail(
            "the server secret doesn't match the key check the database holds",
        ));
    }
    let live = sync_store_postgres::live_instances(&postgres.url, unix_now())
        .await
        .map_err(|e| fail(format!("can't read the database: {e}")))?;
    refuse_serving(live)?;
    let blobs = blob_store.open(data)?;
    let keys = blobs
        .list()
        .await
        .map_err(|e| fail(format!("can't list the blob store: {e}")))?;
    let generation =
        sync_store_postgres::raise_generation(&postgres.url, unix_now(), unix_now() + UPLOAD_TTL)
            .await
            .map_err(|e| fail(format!("can't raise the restore generation: {e}")))?;
    raised(generation);
    let backend = PgBackend::open(&postgres.url)
        .await
        .map_err(|e| fail(format!("can't open the database: {e}")))?;
    let store = Store::open(backend)
        .await
        .map_err(|e| fail(format!("can't open the store: {e}")))?;
    let checked = check_blobs(&store, blobs.as_ref(), keys).await;
    store.backend().close().await;
    Ok(Restored {
        generation,
        blobs: checked?,
        set_aside: None,
    })
}

/// The workspace, blob, index and hash a segment key names.
fn parse_key(key: &str) -> Option<(Id, Id, u64, sync_protocol::types::Digest)> {
    let mut parts = key.split('/');
    let workspace = parts.next()?;
    let blob = parts.next()?;
    let segment = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let id = |text: &str| -> Option<Id> { <[u8; 16]>::try_from(from_hex(text)?).ok().map(Id) };
    let (index, _) = segment.split_once('-')?;
    Some((
        id(workspace)?,
        id(blob)?,
        index.parse().ok()?,
        blobs::key_hash(key)?,
    ))
}

/// The key of `key` in quarantine, dated `now`.
fn quarantine_key(now: u64, key: &str) -> String {
    format!("{QUARANTINE}/{now}/{key}")
}

/// The date and the original key of a quarantined object.
fn parse_quarantined(key: &str) -> Option<(u64, &str)> {
    let rest = key.strip_prefix(QUARANTINE)?.strip_prefix('/')?;
    let (date, original) = rest.split_once('/')?;
    Some((date.parse().ok()?, original))
}

fn store_error(error: impl std::fmt::Display) -> CommandError {
    fail(format!("the blob check stopped: {error}"))
}

async fn move_object(store: &dyn SegmentStore, from: &str, to: &str) -> Result<(), CommandError> {
    let size = store.size(from).await.map_err(store_error)?;
    let bytes = store.get(from, 0..size).await.map_err(store_error)?;
    store.put(to, bytes).await.map_err(store_error)?;
    store.delete(from).await.map_err(store_error)
}

/// Checks the blob store against the database (§19). `listed` is the store
/// as the command listed it before any server could start. Objects the
/// database doesn't know move under the quarantine prefix, and known ones
/// come back from it; a completed blob that no operation lists gets a fresh
/// 24 hours; a blob whose objects are gone becomes an upload at offset 0.
pub async fn check_blobs<B: Backend>(
    store: &Store<B>,
    blobs: &dyn SegmentStore,
    listed: Vec<String>,
) -> Result<BlobCheck, CommandError> {
    let mut check = BlobCheck::default();
    let now = store.clock().now();
    let quarantine_prefix = format!("{QUARANTINE}/");

    for key in &listed {
        if key.starts_with(&quarantine_prefix) {
            continue;
        }
        // A stray file isn't a segment: the pass leaves it where it is.
        let Some((workspace, blob, index, hash)) = parse_key(key) else {
            tracing::warn!(key = %key, "the blob store holds an object that isn't a segment");
            continue;
        };
        let known = store
            .knows_segment(workspace, blob, index, hash)
            .await
            .map_err(store_error)?;
        if !known {
            move_object(blobs, key, &quarantine_key(now, key)).await?;
            check.quarantined += 1;
        }
    }

    for key in &listed {
        let Some((_, original)) = parse_quarantined(key) else {
            continue;
        };
        let Some((workspace, blob, index, hash)) = parse_key(original) else {
            continue;
        };
        if store
            .knows_segment(workspace, blob, index, hash)
            .await
            .map_err(store_error)?
        {
            move_object(blobs, key, original).await?;
            check.returned += 1;
        }
    }

    let held: BTreeSet<String> = blobs
        .list()
        .await
        .map_err(store_error)?
        .into_iter()
        .collect();
    let mut after = None;
    loop {
        let page = store.blob_rows(after).await.map_err(store_error)?;
        let Some((workspace, last)) = page.last() else {
            break;
        };
        after = Some((*workspace, last.blob));
        for (workspace, row) in page {
            if row.marked {
                continue;
            }
            let segments = store
                .stored_segments(workspace, row.blob)
                .await
                .map_err(store_error)?;
            let mut whole = true;
            for (index, hash) in segments.iter().enumerate() {
                let key = segment_key(&workspace, &row.blob, index as u64, hash);
                // A server started during the pass may have stored it since
                // the listing.
                if !held.contains(&key) && blobs.size(&key).await.is_err() {
                    whole = false;
                    break;
                }
            }
            if !whole {
                let listed = store
                    .lose_blob(workspace, row.domain, row.blob, segments)
                    .await
                    .map_err(store_error)?;
                if listed == Some(true) {
                    check.missing += 1;
                }
            } else if row.complete
                && store
                    .refresh_blob(workspace, row.domain, row.blob)
                    .await
                    .map_err(store_error)?
            {
                check.refreshed += 1;
            }
        }
    }
    Ok(check)
}

/// Deletes quarantined objects older than 7 days and returns how many.
pub async fn collect_quarantine(blobs: &dyn SegmentStore, now: u64) -> std::io::Result<u64> {
    let mut deleted = 0;
    for key in blobs.list_under(QUARANTINE).await? {
        let Some((date, _)) = parse_quarantined(&key) else {
            continue;
        };
        if date.saturating_add(QUARANTINE_SECONDS) <= now {
            blobs.delete(&key).await?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Holds this instance's lease every `period` until the task is aborted, or
/// until the database reports another restore generation than the one this
/// process serves: an instance that lost its database through a failover
/// lets its lease lapse, so `mark-restored` passes it by. The task then
/// fires `moved` and ends, and the process restarts on the new generation.
pub fn spawn_lease<B: Backend>(
    store: Arc<Store<B>>,
    instance: Id,
    period: Duration,
    moved: tokio::sync::oneshot::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(period);
        loop {
            every.tick().await;
            let until = store.clock().now() + LEASE_SECONDS;
            match store.hold_lease(instance, until).await {
                Ok(stored) if stored != store.restore_generation() => {
                    tracing::error!("the database moved to another restore generation");
                    let _ = moved.send(());
                    return;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(cause = %error, "can't refresh the instance's lease");
                }
            }
        }
    })
}
