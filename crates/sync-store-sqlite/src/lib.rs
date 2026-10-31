//! The sync server's store on SQLite (docs/workspace-format/sync-v2.md §15).
//!
//! The database runs with write-ahead logging and fully synchronous commits.
//! One writer task owns the only write connection: it takes queued pushes,
//! which the caller has already decoded and checked, and commits them in
//! groups inside one `BEGIN IMMEDIATE` transaction with a savepoint per push,
//! so one push's failure rolls back only that push and one fsync covers the
//! group. It runs the WAL checkpoints itself. Reads use a pool of read-only
//! connections, each read in one transaction, so a pull sees one snapshot.
//!
//! Every query is checked at compile time against `migrations/`, through the
//! offline data in `.sqlx`.

#![forbid(unsafe_code)]

mod admin;
mod txn;

use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use sync_store::{Backend, BoxFut, Error, Limiter, MemoryLimiter, ReadJob, Result, WriteJob};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

pub use admin::{
    DatabaseState, MIGRATOR, backup_into, clear_stamp, inspect, live_instances, migrate,
    raise_generation, store_secret_check,
};
pub use txn::SqliteTxn;

/// The database file's name in the server's data directory.
pub const DATABASE_FILE: &str = "sync.sqlite3";

/// The most pushes one write transaction commits.
const MAX_GROUP: usize = 64;
/// Groups committed between WAL checkpoints while pushes keep queueing.
const CHECKPOINT_EVERY: u64 = 32;
/// How long a connection waits on a lock before it reports busy. The one
/// writer never waits on another, so only a checkpoint or an outside tool
/// can make it wait.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
/// Read connections.
const READERS: u32 = 8;

/// The options every connection uses.
fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(BUSY_TIMEOUT)
        .foreign_keys(false)
        // The writer runs checkpoints itself.
        .pragma("wal_autocheckpoint", "0")
        // Temporary tables and sorts stay in memory, so the server writes
        // nothing outside its data directory.
        .pragma("temp_store", "MEMORY")
        // Statements hold no record bodies, but the server's log keeps only
        // what §29 allows, so no statement reaches it.
        .disable_statement_logging()
}

/// Counters the tests read to see the writer at work.
#[derive(Debug, Default)]
pub struct WriterStats {
    transactions: AtomicU64,
    jobs: AtomicU64,
    checkpoints: AtomicU64,
}

/// A snapshot of [`WriterStats`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriterCounts {
    /// Write transactions committed or rolled back.
    pub transactions: u64,
    /// Jobs the writer ran.
    pub jobs: u64,
    /// WAL checkpoints the writer ran.
    pub checkpoints: u64,
}

/// The settings every store connection runs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `wal`.
    pub journal_mode: String,
    /// 2, `FULL`.
    pub synchronous: i64,
    /// 0: SQLite runs no checkpoint on its own.
    pub wal_autocheckpoint: i64,
}

/// One queued write: its job, then its result once the group's commit is
/// known.
trait Queued: Send {
    /// Runs the job; returns whether its writes stay.
    fn run<'t>(&'t mut self, txn: &'t mut SqliteTxn) -> BoxFut<'t, bool>;
    /// Delivers the result: `commit` says whether the group committed.
    fn finish(self: Box<Self>, commit: std::result::Result<(), String>);
}

struct Request<R> {
    job: Option<WriteJob<SqliteTxn, R>>,
    result: Option<Result<R>>,
    reply: oneshot::Sender<Result<R>>,
}

impl<R: Send + 'static> Queued for Request<R> {
    fn run<'t>(&'t mut self, txn: &'t mut SqliteTxn) -> BoxFut<'t, bool> {
        let job = self.job.take();
        Box::pin(async move {
            let Some(job) = job else {
                return false;
            };
            let result = job(txn).await;
            let keep = result.is_ok();
            self.result = Some(result);
            keep
        })
    }

    fn finish(self: Box<Self>, commit: std::result::Result<(), String>) {
        let result = match (commit, self.result) {
            (Ok(()), Some(result)) => result,
            (Err(_), Some(Err(error))) => Err(error),
            (Err(error), Some(Ok(_))) => Err(Error::Internal(error)),
            (Err(error), None) => Err(Error::Internal(error)),
            (Ok(()), None) => Err(Error::internal("the write job never ran")),
        };
        let _ = self.reply.send(result);
    }
}

/// The store's SQLite database.
pub struct SqliteBackend {
    reads: SqlitePool,
    writer: StdMutex<Option<mpsc::Sender<Box<dyn Queued>>>>,
    task: Mutex<Option<JoinHandle<()>>>,
    stats: Arc<WriterStats>,
    // One instance serves the database, so its counters live in memory.
    limiter: Arc<MemoryLimiter>,
}

impl SqliteBackend {
    /// Opens a migrated database.
    pub async fn open(path: &Path) -> Result<Self> {
        let writer = options(path)
            .create_if_missing(false)
            .connect()
            .await
            .map_err(Error::internal)?;
        let reads = SqlitePoolOptions::new()
            .max_connections(READERS)
            .connect_with(
                options(path)
                    .create_if_missing(false)
                    .pragma("query_only", "ON"),
            )
            .await
            .map_err(Error::internal)?;
        let stats = Arc::new(WriterStats::default());
        let (sender, receiver) = mpsc::channel(1_024);
        let task = tokio::spawn(write_loop(
            SqliteTxn::writer(writer),
            receiver,
            stats.clone(),
        ));
        Ok(Self {
            reads,
            writer: StdMutex::new(Some(sender)),
            task: Mutex::new(Some(task)),
            stats,
            limiter: Arc::new(MemoryLimiter::default()),
        })
    }

    /// The writer's counters.
    pub fn writer_counts(&self) -> WriterCounts {
        WriterCounts {
            transactions: self.stats.transactions.load(Ordering::Relaxed),
            jobs: self.stats.jobs.load(Ordering::Relaxed),
            checkpoints: self.stats.checkpoints.load(Ordering::Relaxed),
        }
    }

    /// The journal mode, `synchronous` level and automatic checkpoint
    /// threshold of the store's connections, which all share one set of
    /// options.
    pub async fn settings(&self) -> Result<Settings> {
        let mut connection = self.reads.acquire().await.map_err(Error::internal)?;
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *connection)
            .await
            .map_err(Error::internal)?;
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut *connection)
            .await
            .map_err(Error::internal)?;
        let wal_autocheckpoint: i64 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
            .fetch_one(&mut *connection)
            .await
            .map_err(Error::internal)?;
        Ok(Settings {
            journal_mode,
            synchronous,
            wal_autocheckpoint,
        })
    }

    /// Finishes the queued writes, checkpoints the write-ahead log into the
    /// database and closes every connection.
    pub async fn close(&self) {
        // The writer stops once its queue closes and drains.
        drop(self.sender().take());
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
        self.reads.close().await;
    }

    fn sender(&self) -> std::sync::MutexGuard<'_, Option<mpsc::Sender<Box<dyn Queued>>>> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Backend for SqliteBackend {
    type Read = SqliteTxn;
    type Write = SqliteTxn;

    fn read<R: Send + 'static>(&self, job: ReadJob<SqliteTxn, R>) -> BoxFut<'_, Result<R>> {
        Box::pin(async move {
            // A dropped transaction rolls back, so a cancelled request never
            // leaves a snapshot open.
            let transaction = self.reads.begin().await.map_err(Error::internal)?;
            let mut txn = SqliteTxn::read(transaction);
            let result = job(&mut txn).await;
            txn.finish_read().await?;
            result
        })
    }

    fn write<R: Send + 'static>(&self, job: WriteJob<SqliteTxn, R>) -> BoxFut<'_, Result<R>> {
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            let request = Request {
                job: Some(job),
                result: None,
                reply,
            };
            let sender = self
                .sender()
                .clone()
                .ok_or_else(|| Error::internal("the store is closed"))?;
            sender
                .send(Box::new(request))
                .await
                .map_err(|_| Error::internal("the writer stopped"))?;
            result
                .await
                .map_err(|_| Error::internal("the writer dropped a write"))?
        })
    }

    fn limiter(&self) -> Arc<dyn Limiter> {
        self.limiter.clone()
    }

    fn close(&self) -> BoxFut<'_, ()> {
        Box::pin(SqliteBackend::close(self))
    }
}

/// The writer: takes queued writes in groups, commits each group in one
/// immediate transaction with a savepoint per write, then delivers each
/// result, and checkpoints the write-ahead log.
async fn write_loop(
    mut txn: SqliteTxn,
    mut queue: mpsc::Receiver<Box<dyn Queued>>,
    stats: Arc<WriterStats>,
) {
    let mut since_checkpoint = 0;
    while let Some(first) = queue.recv().await {
        let mut group = vec![first];
        while group.len() < MAX_GROUP {
            match queue.try_recv() {
                Ok(next) => group.push(next),
                Err(_) => break,
            }
        }
        stats.jobs.fetch_add(group.len() as u64, Ordering::Relaxed);
        let outcome = commit_group(&mut txn, &mut group).await;
        stats.transactions.fetch_add(1, Ordering::Relaxed);
        for queued in group {
            queued.finish(outcome.clone());
        }
        since_checkpoint += 1;
        if since_checkpoint >= CHECKPOINT_EVERY || queue.is_empty() {
            if txn.checkpoint("PASSIVE").await.is_ok() {
                stats.checkpoints.fetch_add(1, Ordering::Relaxed);
            }
            since_checkpoint = 0;
        }
    }
    let _ = txn.checkpoint("TRUNCATE").await;
    if let Some(connection) = txn.into_writer() {
        let _ = connection.close().await;
    }
}

async fn commit_group(
    txn: &mut SqliteTxn,
    group: &mut [Box<dyn Queued>],
) -> std::result::Result<(), String> {
    txn.execute("BEGIN IMMEDIATE").await?;
    for queued in group.iter_mut() {
        if let Err(error) = txn.execute("SAVEPOINT push").await {
            let _ = txn.execute("ROLLBACK").await;
            return Err(error);
        }
        let keep = queued.run(txn).await;
        let end = if keep {
            txn.execute("RELEASE push").await
        } else {
            match txn.execute("ROLLBACK TO push").await {
                Ok(()) => txn.execute("RELEASE push").await,
                Err(error) => Err(error),
            }
        };
        if let Err(error) = end {
            let _ = txn.execute("ROLLBACK").await;
            return Err(error);
        }
    }
    match txn.execute("COMMIT").await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = txn.execute("ROLLBACK").await;
            Err(error)
        }
    }
}

/// The version of the SQLite library sqlx bundles, read from an in-memory
/// database. `sync-server version` prints it.
pub async fn sqlite_version() -> Result<String, sqlx::Error> {
    let mut connection = SqliteConnectOptions::from_str("sqlite::memory:")?
        .connect()
        .await?;
    let version = sqlx::query_scalar("select sqlite_version()")
        .fetch_one(&mut connection)
        .await?;
    connection.close().await?;
    Ok(version)
}

/// Opens a single connection, for the admin commands.
pub(crate) async fn connect(path: &Path, create: bool) -> Result<SqliteConnection, sqlx::Error> {
    options(path).create_if_missing(create).connect().await
}
