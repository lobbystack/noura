//! The sync server's store on PostgreSQL (docs/workspace-format/sync-v2.md
//! §15), which several instances can serve at once.
//!
//! Every push takes the locks of §15 in one order: the workspace's policy
//! lock, a transaction-level advisory lock in the one-key form keyed by the
//! first 8 bytes of the workspace ID, shared without a policy change and
//! exclusive with one; the key-sequence row; the domains' head rows in
//! domain-ID order. It reads policy, epoch, rotation and checkpoint state
//! only under those locks, at READ COMMITTED, and assigns sequences under
//! the head rows. Pushes into one domain queue behind its head row, so a
//! worker per domain commits the queued ones in one transaction with a
//! savepoint each, as the SQLite store does: one push's failure rolls back
//! only that push, and one WAL flush covers the group. Pulls and the other
//! reads each take one read-only REPEATABLE READ snapshot.
//!
//! After a push commits, the instance hands the workspace, domain and new
//! head to its announcer, which sends `NOTIFY` from a connection of its own
//! outside the request pool, one transaction at a time, with every head
//! that waits. A push that rolls back wakes nobody, and no push holds or
//! waits for PostgreSQL's notification lock, or for a connection that
//! does. Each instance `LISTEN`s on another connection of its own and wakes
//! its waiting pulls on the others' notifications.
//!
//! Every query is checked at compile time against `migrations/`, through the
//! offline data in `.sqlx`.

#![forbid(unsafe_code)]

mod admin;
mod limiter;
#[cfg(feature = "testing")]
pub mod testing;
mod txn;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPool, PgPoolOptions};
use sqlx::{ConnectOptions, PgConnection};
use sync_protocol::types::Id;
use sync_store::{
    Backend, BoxFut, Error, EventHub, Limiter, ReadJob, Relayed, Result, WriteGroup, WriteJob,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub use admin::{
    DatabaseState, MIGRATOR, inspect, live_instances, migrate, raise_generation,
    store_secret_check, track_commit_timestamp,
};
pub use limiter::PgLimiter;
pub use txn::PgTxn;

/// The channel every instance notifies and listens on.
pub const CHANNEL: &str = "noura_sync_v2";

/// The most pushes one write transaction commits.
const MAX_GROUP: usize = 64;
/// Connections per instance for requests. The listener and the announcer
/// each hold one more of their own.
pub const CONNECTIONS: u32 = 32;
/// How long a request waits for a free connection.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the listener and the announcer wait before they try again
/// after a failure.
const RETRY: Duration = Duration::from_secs(1);
/// The most domains whose heads wait for the announcer; a commit into
/// another domain meanwhile goes unannounced, which costs a waiting pull at
/// most its wait.
const MAX_PENDING: usize = 4_096;

/// The most presence frames that wait for their `NOTIFY`; another one
/// meanwhile is dropped, and the session's next record replaces it.
const PRESENCE_BACKLOG: usize = 1_024;
/// The most bytes PostgreSQL takes in a `NOTIFY` payload.
pub const NOTIFY_LIMIT: usize = 7_999;
/// Marks a presence notification, in place of a workspace ID.
const PRESENCE_MARK: &str = "p";

/// The options every connection of the store uses: the URL's, with no
/// statement in the server's log (§29).
pub fn connect_options(url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    Ok(PgConnectOptions::from_str(url)?
        .application_name("noura-sync-server")
        .disable_statement_logging())
}

/// Opens a single connection, for the admin commands.
pub(crate) async fn connect(url: &str) -> Result<PgConnection, sqlx::Error> {
    connect_options(url)?.connect().await
}

/// Counters the tests read to see the writers at work.
#[derive(Debug, Default)]
struct WriterStats {
    transactions: AtomicU64,
    jobs: AtomicU64,
}

/// A snapshot of the writers' counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriterCounts {
    /// Write transactions committed or rolled back.
    pub transactions: u64,
    /// Write jobs run.
    pub jobs: u64,
}

/// One queued write: its job, then its result once the transaction's
/// commit is known.
trait Queued: Send {
    /// Runs the job; returns whether its writes stay.
    fn run<'t>(&'t mut self, txn: &'t mut PgTxn) -> BoxFut<'t, bool>;
    /// Delivers the result: `commit` says whether the transaction
    /// committed.
    fn finish(self: Box<Self>, commit: std::result::Result<(), String>);
}

struct Request<R> {
    job: Option<WriteJob<PgTxn, R>>,
    result: Option<Result<R>>,
    reply: oneshot::Sender<Result<R>>,
}

impl<R: Send + 'static> Queued for Request<R> {
    fn run<'t>(&'t mut self, txn: &'t mut PgTxn) -> BoxFut<'t, bool> {
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

/// Heads that wait for the announcer, the highest per domain, with no
/// domain for a change that moved no domain's head, such as an envelope or
/// a re-wrap request.
type Heads = BTreeMap<(Id, Option<Id>), u64>;

/// Adds `heads` to `pending`, keeping the highest head of each domain and
/// at most [`MAX_PENDING`] domains.
fn merge(pending: &mut Heads, heads: impl IntoIterator<Item = ((Id, Option<Id>), u64)>) {
    for (domain, head) in heads {
        let full = pending.len() >= MAX_PENDING;
        match pending.entry(domain) {
            Entry::Occupied(mut entry) => {
                let highest = entry.get_mut();
                *highest = (*highest).max(head);
            }
            Entry::Vacant(entry) if !full => {
                entry.insert(head);
            }
            Entry::Vacant(_) => {}
        }
    }
}

/// The instance's announcer: heads wait in `pending` until its task sends
/// them.
struct Announcer {
    pending: Arc<StdMutex<Heads>>,
    wake: mpsc::Sender<()>,
    task: StdMutex<Option<JoinHandle<()>>>,
}

/// Sends what waits in `pending` from one connection of its own, one
/// transaction at a time. That session commits without waiting for a
/// flush or a standby: a notification lost in a crash costs a waiting pull
/// at most its wait, and the notification lock never waits on a disk or a
/// round trip for it. While another session holds that lock, heads keep
/// merging into `pending`, so nothing piles up.
async fn announce_loop(
    options: PgConnectOptions,
    pending: Arc<StdMutex<Heads>>,
    mut wake: mpsc::Receiver<()>,
    instance: String,
) {
    let mut connection: Option<PgConnection> = None;
    while wake.recv().await.is_some() {
        loop {
            let heads =
                std::mem::take(&mut *pending.lock().unwrap_or_else(PoisonError::into_inner));
            if heads.is_empty() {
                break;
            }
            let payloads: Vec<String> = heads
                .iter()
                .map(|((workspace, domain), head)| match domain {
                    Some(domain) => format!(
                        "{instance}.{}.{}.{head}",
                        workspace.to_hex(),
                        domain.to_hex()
                    ),
                    None => format!("{instance}.{}", workspace.to_hex()),
                })
                .collect();
            let mut session = match connection.take() {
                Some(session) => Some(session),
                None => options.connect().await.ok(),
            };
            let sent = match session.as_mut() {
                Some(session) => sqlx::query!(
                    r#"SELECT COUNT(*) AS "sent!" FROM unnest($2::text[]) AS payload,
                   LATERAL pg_notify($1, payload)"#,
                    CHANNEL,
                    &payloads[..]
                )
                .fetch_one(session)
                .await
                .is_ok(),
                None => false,
            };
            if sent {
                connection = session;
                continue;
            }
            // The heads wait for the next try, unless the backend is gone.
            merge(
                &mut pending.lock().unwrap_or_else(PoisonError::into_inner),
                heads,
            );
            if Arc::strong_count(&pending) == 1 {
                return;
            }
            tokio::time::sleep(RETRY).await;
        }
    }
}

struct Inner {
    /// The connections requests use.
    pool: PgPool,
    /// The listener's own connection.
    listen_pool: PgPool,
    announcer: Announcer,
    /// Where presence frames wait for their `NOTIFY`.
    relay: mpsc::Sender<(Id, Relayed)>,
    /// The task that sends them.
    relay_task: StdMutex<Option<JoinHandle<()>>>,
    /// Each domain's queue of pushes, while its worker runs.
    groups: StdMutex<HashMap<WriteGroup, mpsc::UnboundedSender<Box<dyn Queued>>>>,
    /// Identifies this instance's notifications, which it skips.
    instance: String,
    policy_read_delay: StdMutex<Option<Duration>>,
    stats: WriterStats,
    listener: StdMutex<Option<JoinHandle<()>>>,
    limiter: Arc<PgLimiter>,
}

impl Inner {
    fn policy_read_delay(&self) -> Option<Duration> {
        *self
            .policy_read_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `jobs` in one READ COMMITTED transaction, each in its own
    /// savepoint when there are several, and commits what stays.
    async fn run(&self, jobs: &mut [Box<dyn Queued>]) -> std::result::Result<(), String> {
        self.stats
            .jobs
            .fetch_add(jobs.len() as u64, Ordering::Relaxed);
        self.stats.transactions.fetch_add(1, Ordering::Relaxed);
        // Named, not the database's default: a write reads the policy only
        // after its locks, in a snapshot taken after it waited for them.
        let transaction = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await
            .map_err(|e| e.to_string())?;
        let mut txn = PgTxn::writer(transaction, self.policy_read_delay());
        let outcome = self.run_in(&mut txn, jobs).await;
        if outcome.is_err() {
            txn.rollback().await;
        }
        outcome
    }

    async fn run_in(
        &self,
        txn: &mut PgTxn,
        jobs: &mut [Box<dyn Queued>],
    ) -> std::result::Result<(), String> {
        if let [job] = jobs {
            if !job.run(txn).await {
                return Err("the write rolled back".into());
            }
        } else {
            // One round trip closes a job's savepoint and opens the next
            // one's: a busy domain's other pushes wait for this transaction.
            let last = jobs.len().saturating_sub(1);
            txn.execute("SAVEPOINT push").await?;
            for (index, job) in jobs.iter_mut().enumerate() {
                let kept = job.run(txn).await;
                txn.execute(match (kept, index == last) {
                    (true, false) => "RELEASE SAVEPOINT push; SAVEPOINT push",
                    (true, true) => "RELEASE SAVEPOINT push",
                    (false, false) => {
                        "ROLLBACK TO SAVEPOINT push; RELEASE SAVEPOINT push; SAVEPOINT push"
                    }
                    (false, true) => "ROLLBACK TO SAVEPOINT push; RELEASE SAVEPOINT push",
                })
                .await?;
                txn.settle_locks(kept);
            }
        }
        txn.commit().await
    }
}

/// A domain's worker: commits its queued pushes in groups until the queue
/// stays empty, then leaves.
async fn group_loop(
    inner: Arc<Inner>,
    group: WriteGroup,
    mut queue: mpsc::UnboundedReceiver<Box<dyn Queued>>,
) {
    loop {
        let mut jobs = Vec::new();
        while jobs.len() < MAX_GROUP {
            match queue.try_recv() {
                Ok(job) => jobs.push(job),
                Err(_) => break,
            }
        }
        if jobs.is_empty() {
            // Senders queue under the same lock, so nothing arrives between
            // this check and the removal.
            let mut groups = inner.groups.lock().unwrap_or_else(PoisonError::into_inner);
            if queue.is_empty() {
                groups.remove(&group);
                return;
            }
            continue;
        }
        let outcome = inner.run(&mut jobs).await;
        for job in jobs {
            job.finish(outcome.clone());
        }
    }
}

/// The store's PostgreSQL database, as one instance sees it.
pub struct PgBackend {
    inner: Arc<Inner>,
}

impl PgBackend {
    /// Connects to a migrated database at `url`.
    pub async fn open(url: &str) -> Result<Self> {
        let options = connect_options(url).map_err(Error::internal)?;
        let pool = PgPoolOptions::new()
            .max_connections(CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(options.clone())
            .await
            .map_err(Error::internal)?;
        let listen_pool = PgPoolOptions::new()
            .max_connections(1)
            .max_lifetime(None)
            .idle_timeout(None)
            .connect_lazy_with(options.clone());
        let instance = Id::random().map_err(Error::internal)?.to_hex();
        let pending = Arc::new(StdMutex::new(Heads::new()));
        let (wake, woken) = mpsc::channel(1);
        let options = options.options([("synchronous_commit", "off")]);
        let (relay, frames) = mpsc::channel(PRESENCE_BACKLOG);
        let relay_task = tokio::spawn(relay_loop(options.clone(), frames, instance.clone()));
        let task = tokio::spawn(announce_loop(
            options,
            pending.clone(),
            woken,
            instance.clone(),
        ));
        Ok(Self {
            inner: Arc::new(Inner {
                limiter: Arc::new(PgLimiter::new(pool.clone())),
                pool,
                listen_pool,
                announcer: Announcer {
                    pending,
                    wake,
                    task: StdMutex::new(Some(task)),
                },
                relay,
                relay_task: StdMutex::new(Some(relay_task)),
                groups: StdMutex::new(HashMap::new()),
                instance,
                policy_read_delay: StdMutex::new(None),
                stats: WriterStats::default(),
                listener: StdMutex::new(None),
            }),
        })
    }

    /// The connection pool, for tests that inspect the database.
    pub fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    /// The writers' counters.
    pub fn writer_counts(&self) -> WriterCounts {
        WriterCounts {
            transactions: self.inner.stats.transactions.load(Ordering::Relaxed),
            jobs: self.inner.stats.jobs.load(Ordering::Relaxed),
        }
    }

    /// A test seam: every write transaction waits `delay` right after it
    /// reads a workspace's policy head, so it holds its locks through a
    /// wide window between the policy read and the commit, and races meet
    /// those locks.
    #[cfg(feature = "testing")]
    pub fn inject_policy_read_delay(&self, delay: Option<Duration>) {
        *self
            .inner
            .policy_read_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = delay;
    }

    fn spawn_write<R: Send + 'static>(
        &self,
        job: WriteJob<PgTxn, R>,
    ) -> oneshot::Receiver<Result<R>> {
        let (reply, result) = oneshot::channel();
        let inner = self.inner.clone();
        // The write runs in its own task, so a caller that goes away leaves
        // it to commit or roll back whole.
        tokio::spawn(async move {
            let mut jobs: Vec<Box<dyn Queued>> = vec![Box::new(Request {
                job: Some(job),
                result: None,
                reply,
            })];
            let outcome = inner.run(&mut jobs).await;
            for job in jobs {
                job.finish(outcome.clone());
            }
        });
        result
    }
}

async fn received<R>(result: oneshot::Receiver<Result<R>>) -> Result<R> {
    result
        .await
        .map_err(|_| Error::internal("the writer dropped a write"))?
}

/// Relays presence from one connection of its own, which, like the
/// announcer's, commits without waiting for a flush. A failed notification
/// only loses that frame.
async fn relay_loop(
    options: PgConnectOptions,
    mut frames: mpsc::Receiver<(Id, Relayed)>,
    instance: String,
) {
    let mut connection: Option<PgConnection> = None;
    while let Some((workspace, relayed)) = frames.recv().await {
        let payload = presence_payload(&instance, workspace, &relayed);
        if payload.len() > NOTIFY_LIMIT {
            continue;
        }
        let session = match connection.take() {
            Some(session) => Some(session),
            None => options.connect().await.ok(),
        };
        if let Some(mut session) = session {
            let sent = sqlx::query("SELECT pg_notify($1, $2)")
                .bind(CHANNEL)
                .bind(&payload)
                .execute(&mut session)
                .await;
            if sent.is_ok() {
                connection = Some(session);
            }
        }
    }
}

/// A presence notification: `instance.p.workspace.frame`, the frame in
/// base64 (§23).
fn presence_payload(instance: &str, workspace: Id, relayed: &Relayed) -> String {
    format!(
        "{instance}.{PRESENCE_MARK}.{}.{}",
        workspace.to_hex(),
        STANDARD.encode(relayed.to_bytes())
    )
}

/// Reads [`presence_payload`], for a notification of another instance.
fn notified_presence(payload: &str, own: &str) -> Option<(Id, Relayed)> {
    let mut parts = payload.split('.');
    let instance = parts.next()?;
    if parts.next()? != PRESENCE_MARK || instance == own {
        return None;
    }
    let workspace = Id::from_hex(parts.next()?)?;
    let bytes = STANDARD.decode(parts.next()?).ok()?;
    Some((workspace, Relayed::from_bytes(&bytes)?))
}

/// Parses `instance.workspace.domain.head`, or `instance.workspace` for a
/// change that moved no domain's head.
fn notified_workspace(payload: &str, own: &str) -> Option<Id> {
    let mut parts = payload.split('.');
    let instance = parts.next()?;
    let workspace = Id::from_hex(parts.next()?)?;
    (instance != own).then_some(workspace)
}

async fn listen_loop(mut listener: PgListener, hub: EventHub, own: String) {
    loop {
        match listener.recv().await {
            Ok(notification) => {
                let payload = notification.payload();
                if let Some((workspace, relayed)) = notified_presence(payload, &own) {
                    hub.relay(workspace, relayed);
                } else if let Some(workspace) = notified_workspace(payload, &own) {
                    hub.notify(workspace);
                }
            }
            Err(sqlx::Error::PoolClosed) => return,
            Err(_) => tokio::time::sleep(RETRY).await,
        }
    }
}

impl Backend for PgBackend {
    type Read = PgTxn;
    type Write = PgTxn;

    fn read<R: Send + 'static>(&self, job: ReadJob<PgTxn, R>) -> BoxFut<'_, Result<R>> {
        Box::pin(async move {
            // A dropped transaction rolls back, so a cancelled request never
            // leaves a snapshot open.
            let transaction = self
                .inner
                .pool
                .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .await
                .map_err(Error::internal)?;
            let mut txn = PgTxn::new(transaction);
            let result = job(&mut txn).await;
            txn.rollback().await;
            result
        })
    }

    fn write<R: Send + 'static>(&self, job: WriteJob<PgTxn, R>) -> BoxFut<'_, Result<R>> {
        let result = self.spawn_write(job);
        Box::pin(received(result))
    }

    fn write_grouped<R: Send + 'static>(
        &self,
        group: WriteGroup,
        job: WriteJob<PgTxn, R>,
    ) -> BoxFut<'_, Result<R>> {
        let (reply, result) = oneshot::channel();
        let request: Box<dyn Queued> = Box::new(Request {
            job: Some(job),
            result: None,
            reply,
        });
        {
            let mut groups = self
                .inner
                .groups
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let request = match groups.get(&group) {
                Some(queue) => queue.send(request).err().map(|e| e.0),
                None => Some(request),
            };
            if let Some(request) = request {
                let (queue, receiver) = mpsc::unbounded_channel();
                let _ = queue.send(request);
                groups.insert(group, queue);
                tokio::spawn(group_loop(self.inner.clone(), group, receiver));
            }
        }
        Box::pin(received(result))
    }

    fn attach(&self, hub: EventHub) -> BoxFut<'_, Result<()>> {
        Box::pin(async move {
            let mut listener = PgListener::connect_with(&self.inner.listen_pool)
                .await
                .map_err(Error::internal)?;
            listener.listen(CHANNEL).await.map_err(Error::internal)?;
            let task = tokio::spawn(listen_loop(listener, hub, self.inner.instance.clone()));
            let old = self
                .inner
                .listener
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .replace(task);
            if let Some(old) = old {
                old.abort();
            }
            Ok(())
        })
    }

    fn announce(&self, workspace: Id, heads: Vec<(Id, u64)>) {
        // The push's response doesn't wait: the announcer sends the heads
        // after this push committed, from its own transaction.
        let announcer = &self.inner.announcer;
        let heads: Vec<((Id, Option<Id>), u64)> = if heads.is_empty() {
            vec![((workspace, None), 0)]
        } else {
            heads
                .into_iter()
                .map(|(domain, head)| ((workspace, Some(domain)), head))
                .collect()
        };
        merge(
            &mut announcer
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            heads,
        );
        // A full channel already holds a wake that the announcer hasn't
        // taken, and it reads `pending` after taking it.
        let _ = announcer.wake.try_send(());
    }

    fn relay_presence(&self, workspace: Id, relayed: &Relayed) {
        // A full queue drops the frame: presence is only ever the latest
        // cursor.
        let _ = self.inner.relay.try_send((workspace, relayed.clone()));
    }

    fn parallel_writes(&self) -> bool {
        true
    }

    fn limiter(&self) -> Arc<dyn Limiter> {
        self.inner.limiter.clone()
    }

    fn close(&self) -> BoxFut<'_, ()> {
        Box::pin(async move {
            // Queued pushes finish first: each worker holds a connection
            // only while it commits, and close waits for every connection.
            loop {
                let busy = !self
                    .inner
                    .groups
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_empty();
                if !busy {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // The announcer may be waiting for the notification lock. Its
            // heads go unannounced, and its connection closes with it.
            let announcer = self
                .inner
                .announcer
                .task
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(announcer) = announcer {
                announcer.abort();
                let _ = announcer.await;
            }
            let relay = self
                .inner
                .relay_task
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(relay) = relay {
                relay.abort();
                let _ = relay.await;
            }
            self.inner.pool.close().await;
            self.inner.listen_pool.close().await;
            let listener = self
                .inner
                .listener
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(listener) = listener {
                let _ = listener.await;
            }
        })
    }
}
