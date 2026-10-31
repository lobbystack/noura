//! What the scale test and the soak share: a counting allocator that keeps
//! client and server memory apart, a seed that reproduces a failed run, a
//! digest of a folder, a report the workflow publishes, and the database's
//! size on either store.
//!
//! The server runs on a runtime of its own, whose threads count as the
//! server's. Nothing crosses between the two sides but sockets, so each
//! side frees what it allocated. The counts cover Rust's heap: SQLite's page
//! cache and PostgreSQL's processes stay outside them.

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use sync_protocol::types::Digest;

const CLIENT: usize = 0;
const SERVER: usize = 1;

static CURRENT: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];
static PEAK: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];

thread_local! {
    /// The side this thread allocates for. It holds no destructor, so the
    /// allocator can read it at any point of a thread's life.
    static SIDE: Cell<usize> = const { Cell::new(CLIENT) };
}

/// Wraps the system allocator and counts each side's heap.
pub struct Counting;

fn grew(size: usize) {
    let side = SIDE.with(Cell::get);
    let now = CURRENT[side].fetch_add(size as i64, Ordering::Relaxed) + size as i64;
    PEAK[side].fetch_max(now, Ordering::Relaxed);
}

fn shrank(size: usize) {
    CURRENT[SIDE.with(Cell::get)].fetch_sub(size as i64, Ordering::Relaxed);
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

/// The runtime the test's clients run on.
pub fn client_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime")
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
/// one JSON object per line, and to the test's output.
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

    /// Adds the peak heaps and writes the report out.
    pub fn finish(mut self) {
        let (client, server) = peaks();
        self.set("peak_client_heap_bytes", client);
        self.set("peak_server_heap_bytes", server);
        let line = serde_json::Value::Object(self.0).to_string();
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

/// A domain's head row in the server's store.
pub async fn domain_row<B: sync_store::Backend>(
    store: &sync_store::Store<B>,
    workspace: sync_protocol::types::Id,
    domain: sync_protocol::types::Id,
) -> sync_store::DomainRow {
    use sync_store::ReadTxn as _;
    store
        .backend()
        .read(Box::new(move |txn: &mut B::Read| {
            Box::pin(async move { txn.domain(workspace, domain).await })
        }))
        .await
        .expect("a read")
        .expect("the domain")
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
