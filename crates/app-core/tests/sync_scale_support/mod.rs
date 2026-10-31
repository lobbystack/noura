//! What the scale test and the soak share: a counting allocator that keeps
//! client and server memory apart, a seed that reproduces a failed run, a
//! digest of a folder, a report the workflow publishes, and the database's
//! size on either store.
//!
//! The server runs on a runtime of its own, whose threads count as the
//! server's, as do the threads its SQLite driver starts where a thread's
//! name can be read without allocating. Once the run starts its load, the
//! test reaches the store only from that runtime ([`on_server`]), so each
//! side frees what it allocated. The counts cover Rust's heap: SQLite's page
//! cache and PostgreSQL's processes stay outside them.

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use sync_protocol::types::{Digest, Id};
use sync_store::Backend;

use crate::sync_support::{DATABASE, Server};

const CLIENT: usize = 0;
const SERVER: usize = 1;
/// A thread that hasn't allocated yet.
const UNKNOWN: usize = 2;

static CURRENT: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];
static PEAK: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];

thread_local! {
    /// The side this thread allocates for. It holds no destructor, so the
    /// allocator can read it at any point of a thread's life.
    static SIDE: Cell<usize> = const { Cell::new(UNKNOWN) };
}

/// The side a thread belongs to by its name, which needs no allocation.
#[cfg(unix)]
fn side_by_name() -> usize {
    let mut name = [0 as libc::c_char; 32];
    // SAFETY: the buffer outlives the call and its length is right.
    let read =
        unsafe { libc::pthread_getname_np(libc::pthread_self(), name.as_mut_ptr(), name.len()) };
    // SAFETY: on success the name ends in a NUL inside the buffer.
    let bytes = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_bytes();
    if read == 0 && bytes.starts_with(b"sqlx-sqlite") {
        SERVER
    } else {
        CLIENT
    }
}

/// A platform with no name that reads without allocating counts every
/// thread it can't place as the client's.
#[cfg(not(unix))]
fn side_by_name() -> usize {
    CLIENT
}

fn side() -> usize {
    SIDE.with(|side| {
        if side.get() == UNKNOWN {
            side.set(side_by_name());
        }
        side.get()
    })
}

/// Wraps the system allocator and counts each side's heap.
pub struct Counting;

fn grew(size: usize) {
    let side = side();
    let now = CURRENT[side].fetch_add(size as i64, Ordering::Relaxed) + size as i64;
    PEAK[side].fetch_max(now, Ordering::Relaxed);
}

fn shrank(size: usize) {
    CURRENT[side()].fetch_sub(size as i64, Ordering::Relaxed);
}

// SAFETY: every call goes to the system allocator unchanged; the counters
// only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            grew(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        shrank(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            shrank(layout.size());
            grew(new_size);
        }
        moved
    }
}

/// The runtime the test's server runs on, whose threads count as the
/// server's.
pub fn server_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .on_thread_start(|| SIDE.with(|side| side.set(SERVER)))
        .build()
        .expect("a runtime")
}

/// Runs `start` with this thread counting as the server's, for what the
/// server allocates as it starts and frees on its own threads later.
pub fn as_server<T>(start: impl FnOnce() -> T) -> T {
    SIDE.with(|side| side.set(SERVER));
    let result = start();
    SIDE.with(|side| side.set(CLIENT));
    result
}

/// The runtime the test's clients run on. Each client's journal write waits
/// for the disk on its worker, so a fleet needs far more workers than cores.
pub fn client_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(CLIENT_WORKERS)
        .enable_all()
        .build()
        .expect("a runtime")
}

const CLIENT_WORKERS: usize = 64;

/// Runs `work` on the server's runtime, so what the store allocates for it
/// counts as the server's and is freed there.
pub async fn on_server<T: Send + 'static>(
    runtime: &tokio::runtime::Handle,
    work: impl Future<Output = T> + Send + 'static,
) -> T {
    runtime.spawn(work).await.expect("the server's task")
}

/// The server's heap now, in bytes.
pub fn server_heap() -> u64 {
    CURRENT[SERVER].load(Ordering::Relaxed).max(0) as u64
}

/// The highest each side's heap reached: client, then server.
pub fn peaks() -> (u64, u64) {
    let peak = |side: usize| PEAK[side].load(Ordering::Relaxed).max(0) as u64;
    (peak(CLIENT), peak(SERVER))
}

/// The run's seed: `variable` when set, else a new one, printed at once.
/// When the run fails, the guard prints how to run it again.
pub struct Seed {
    pub value: u64,
    variable: &'static str,
}

impl Seed {
    pub fn take(variable: &'static str) -> Self {
        let value = std::env::var(variable)
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("after 1970")
                    .subsec_nanos()
                    .into()
            });
        println!("{variable}={value}");
        Self { value, variable }
    }
}

impl Drop for Seed {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "This run failed. Reproduce it with {}={}.",
                self.variable, self.value
            );
            println!(
                "::error::Reproduce this failure with {}={}",
                self.variable, self.value
            );
        }
    }
}

/// A small generator, so a seed yields the same run on every machine.
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    /// `len` bytes of text that don't compress.
    pub fn text(&mut self, len: usize) -> String {
        const WORD: &[u8] = b"abcdefghijklmnopqrstuvwxyz \n";
        (0..len)
            .map(|_| WORD[self.below(WORD.len())] as char)
            .collect()
    }

    /// `len` bytes that don't compress.
    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(len + 8);
        while bytes.len() < len {
            bytes.extend_from_slice(&self.next().to_le_bytes());
        }
        bytes.truncate(len);
        bytes
    }
}

/// Every file under `root` outside `.noura`, with its length and digest,
/// read one at a time so a large folder stays out of memory.
pub fn digests(root: &Path) -> BTreeMap<String, (u64, Digest)> {
    let mut found = BTreeMap::new();
    let mut waiting: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(folder) = waiting.pop() {
        for entry in std::fs::read_dir(&folder).expect("a folder") {
            let entry = entry.expect("an entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("beneath the root")
                .to_str()
                .expect("UTF-8")
                .replace('\\', "/");
            if relative == ".noura" {
                continue;
            }
            if entry.file_type().expect("a type").is_dir() {
                waiting.push(path);
            } else {
                let bytes = std::fs::read(&path).expect("a file");
                let digest = sync_protocol::crypto::hash(&bytes);
                found.insert(relative, (bytes.len() as u64, digest));
            }
        }
    }
    found
}

/// The names of the paths that differ between two digests, for a failure
/// message that stays readable: lost, added and diverged files.
pub fn difference(
    expected: &BTreeMap<String, (u64, Digest)>,
    found: &BTreeMap<String, (u64, Digest)>,
) -> Vec<String> {
    let mut differ = Vec::new();
    for (path, entry) in expected {
        match found.get(path) {
            None => differ.push(format!("lost {path}")),
            Some(other) if other != entry => differ.push(format!("diverged {path}")),
            Some(_) => {}
        }
    }
    for path in found.keys() {
        if !expected.contains_key(path) {
            differ.push(format!("added {path}"));
        }
    }
    differ
}

/// One run's numbers, which go to the file `NOURA_SCALE_REPORT` names, as
/// one JSON object per line, and to the test's output. A run that fails
/// still writes what it measured, marked `failed`.
#[derive(Default)]
pub struct Report(serde_json::Map<String, serde_json::Value>);

impl Report {
    pub fn new(name: &str, store: &str, seed: u64) -> Self {
        let mut report = Self::default();
        report.set("run", name);
        report.set("store", store);
        report.set("seed", seed);
        report
    }

    pub fn set(&mut self, key: &str, value: impl Into<serde_json::Value>) {
        self.0.insert(key.to_owned(), value.into());
    }

    pub fn seconds(&mut self, key: &str, since: std::time::Instant) {
        self.set(key, (since.elapsed().as_secs_f64() * 10.0).round() / 10.0);
    }

    /// Ends the run: dropping the report adds the peak heaps and writes it
    /// out. Only a run that got here counts as passed.
    pub fn finish(mut self) {
        self.set("passed", true);
    }
}

impl Drop for Report {
    fn drop(&mut self) {
        let (client, server) = peaks();
        self.set("peak_client_heap_bytes", client);
        self.set("peak_server_heap_bytes", server);
        let passed = self.0.contains_key("passed");
        self.set("failed", !passed);
        self.0.remove("passed");
        let line = serde_json::Value::Object(std::mem::take(&mut self.0)).to_string();
        println!("{line}");
        if let Ok(path) = std::env::var("NOURA_SCALE_REPORT") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("the report file");
            writeln!(file, "{line}").expect("written");
        }
    }
}

/// A number from the environment, or `default`.
pub fn number(variable: &str, default: u64) -> u64 {
    std::env::var(variable)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(default)
}

/// The size of a SQLite database with its write-ahead log.
pub fn sqlite_size(file: &Path) -> u64 {
    ["", "-wal"]
        .iter()
        .filter_map(|suffix| {
            let mut name = file.as_os_str().to_owned();
            name.push(suffix);
            std::fs::metadata(name).ok()
        })
        .map(|metadata| metadata.len())
        .sum()
}

/// The size of the PostgreSQL database at `url`, with its indexes.
pub async fn postgres_size(url: &str) -> u64 {
    use sqlx::{Connection, Row};
    let mut connection = sqlx::PgConnection::connect(url).await.expect("connected");
    let row = sqlx::query("SELECT pg_database_size(current_database())")
        .fetch_one(&mut connection)
        .await
        .expect("the size");
    row.get::<i64, _>(0).max(0) as u64
}

/// Whether something kept the write-ahead log of the SQLite database at
/// `file` from truncating for all of `window`. A snapshot held open blocks a
/// `TRUNCATE` until it ends, so a connection that waited for it would report
/// success afterwards; this one never waits, and tries again until one try
/// finds no reader and no writer. A short read or a commit only costs a try.
///
/// A reader that started on an empty log blocks nothing, so the log must
/// hold frames when the read starts, as it does under load.
pub fn reader_stayed_open(file: &Path, window: Duration) -> bool {
    let connection = rusqlite::Connection::open(file).expect("the database");
    connection
        .busy_timeout(Duration::ZERO)
        .expect("no busy timeout");
    let started = Instant::now();
    loop {
        let busy = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(1);
        if busy == 0 {
            return false;
        }
        if started.elapsed() >= window {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What the store-specific part of the run does.
pub trait Database<B: Backend>: 'static {
    /// Forgets answered approval requests, which count against a cap until
    /// they expire.
    fn clear_requests(server: &Server<B>) -> impl Future<Output = ()>;
    /// The database's size.
    fn size(server: &Server<B>) -> impl Future<Output = u64>;
    /// The bytes of `domain`'s log entries at or below `floor`, the bytes
    /// above it, and the longest entry.
    fn log_bytes(
        server: &Server<B>,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> impl Future<Output = (u64, u64, u64)>;
    /// The checkpoints the domain ever took, and those still servable.
    fn checkpoints(
        server: &Server<B>,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> impl Future<Output = (u64, u64)>;
    /// Whether a read transaction stayed open for all of `window`.
    fn reader_stayed_open(server: &Server<B>, window: Duration) -> bool;
    /// The write-ahead log's size now, for the numbers.
    fn wal_bytes(server: &Server<B>) -> u64;
}

pub struct Sqlite;

impl Sqlite {
    pub fn file(server: &Server<sync_store_sqlite::SqliteBackend>) -> std::path::PathBuf {
        server.dir.path().join("data").join(DATABASE)
    }

    fn connection(server: &Server<sync_store_sqlite::SqliteBackend>) -> rusqlite::Connection {
        rusqlite::Connection::open(Self::file(server)).expect("the database")
    }
}

impl Database<sync_store_sqlite::SqliteBackend> for Sqlite {
    async fn clear_requests(server: &Server<sync_store_sqlite::SqliteBackend>) {
        Self::connection(server)
            .execute("DELETE FROM approval_requests", [])
            .expect("cleared");
    }

    async fn size(server: &Server<sync_store_sqlite::SqliteBackend>) -> u64 {
        sqlite_size(&Self::file(server))
    }

    async fn log_bytes(
        server: &Server<sync_store_sqlite::SqliteBackend>,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> (u64, u64, u64) {
        Self::connection(server)
            .query_row(
                "SELECT COALESCE(SUM(CASE WHEN seq <= ?3 THEN LENGTH(record) END), 0),
                        COALESCE(SUM(CASE WHEN seq > ?3 THEN LENGTH(record) END), 0),
                        COALESCE(MAX(LENGTH(record)), 0)
                 FROM entries WHERE workspace = ?1 AND domain = ?2",
                rusqlite::params![workspace.0.as_slice(), domain.0.as_slice(), floor as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, i64>(2)? as u64,
                    ))
                },
            )
            .expect("the log's size")
    }

    async fn checkpoints(
        server: &Server<sync_store_sqlite::SqliteBackend>,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> (u64, u64) {
        Self::connection(server)
            .query_row(
                "SELECT COUNT(*), COUNT(CASE WHEN covered_seq >= ?3 THEN 1 END)
                 FROM checkpoints WHERE workspace = ?1 AND domain = ?2",
                rusqlite::params![workspace.0.as_slice(), domain.0.as_slice(), floor as i64],
                |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
            )
            .expect("the checkpoints")
    }

    fn reader_stayed_open(
        server: &Server<sync_store_sqlite::SqliteBackend>,
        window: Duration,
    ) -> bool {
        reader_stayed_open(&Self::file(server), window)
    }

    fn wal_bytes(server: &Server<sync_store_sqlite::SqliteBackend>) -> u64 {
        let mut name = Self::file(server).into_os_string();
        name.push("-wal");
        std::fs::metadata(name).map_or(0, |metadata| metadata.len())
    }
}

pub struct Postgres;

impl Postgres {
    async fn connection(server: &Server<sync_store_postgres::PgBackend>) -> sqlx::PgConnection {
        use sqlx::Connection;
        let url = &server.postgres.as_ref().expect("a database").url;
        sqlx::PgConnection::connect(url).await.expect("connected")
    }
}

impl Database<sync_store_postgres::PgBackend> for Postgres {
    async fn clear_requests(server: &Server<sync_store_postgres::PgBackend>) {
        sqlx::raw_sql("DELETE FROM approval_requests")
            .execute(&mut Self::connection(server).await)
            .await
            .expect("cleared");
    }

    async fn size(server: &Server<sync_store_postgres::PgBackend>) -> u64 {
        postgres_size(&server.postgres.as_ref().expect("a database").url).await
    }

    async fn log_bytes(
        server: &Server<sync_store_postgres::PgBackend>,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> (u64, u64, u64) {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT COALESCE(SUM(CASE WHEN seq <= $3 THEN LENGTH(record) END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN seq > $3 THEN LENGTH(record) END), 0)::BIGINT,
                    COALESCE(MAX(LENGTH(record)), 0)::BIGINT
             FROM entries WHERE workspace = $1 AND domain = $2",
        )
        .bind(workspace.0.as_slice())
        .bind(domain.0.as_slice())
        .bind(floor as i64)
        .fetch_one(&mut Self::connection(server).await)
        .await
        .expect("the log's size");
        (
            row.get::<i64, _>(0) as u64,
            row.get::<i64, _>(1) as u64,
            row.get::<i64, _>(2) as u64,
        )
    }

    async fn checkpoints(
        server: &Server<sync_store_postgres::PgBackend>,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> (u64, u64) {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT COUNT(*)::BIGINT, COUNT(CASE WHEN covered_seq >= $3 THEN 1 END)::BIGINT
             FROM checkpoints WHERE workspace = $1 AND domain = $2",
        )
        .bind(workspace.0.as_slice())
        .bind(domain.0.as_slice())
        .bind(floor as i64)
        .fetch_one(&mut Self::connection(server).await)
        .await
        .expect("the checkpoints");
        (row.get::<i64, _>(0) as u64, row.get::<i64, _>(1) as u64)
    }

    fn reader_stayed_open(_: &Server<sync_store_postgres::PgBackend>, _: Duration) -> bool {
        false
    }

    fn wal_bytes(_: &Server<sync_store_postgres::PgBackend>) -> u64 {
        0
    }
}
