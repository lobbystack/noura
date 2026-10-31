//! The soak of sync-v2.md §32 at seam 4: simulated devices in the workspace
//! domain and two restricted domains write at collaboration volume, the 500
//! ms tick, while the retention floors move, each domain's storage stays
//! within the bound of §19, server memory stays level, one device loses its
//! connection and catches up, and every device ends with every operation
//! exactly once.
//!
//! The fleet has two kinds of device, both the real client. An *editor* holds
//! notes in text mode and follows the log with a round every second, so it
//! verifies everything and signs the checkpoints. A *writer* changes its own
//! note each tick and pushes it, and catches up only at the end: 200 devices
//! that each verified 400 records a second would measure the test machine,
//! not the server. Each edit carries a token that must end up in the note
//! once on every editor.
//!
//! A plain test run takes seconds with a few devices. The scheduled job sets
//! `NOURA_SOAK_DEVICES=200`, `NOURA_SOAK_SECONDS=86400`,
//! `NOURA_SOAK_RETAINED_BYTES=67108864`, `NOURA_SOAK_WARMUP_SECONDS=600` and
//! `NOURA_SOAK_HOLD_RATE=1`. `NOURA_SOAK_SEED` repeats a run's choices, and
//! `NOURA_SCALE_REPORT` names the file the numbers go to.

mod sync_scale_support;
mod sync_support;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::sync::SyncError;
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::workspace::text::{Entered, IDLE_ANY_WRITER, IDLE_PATCHES};
use collab_sandbox::SandboxDocument;
use editor_core::text_mode::{CollabDocument, Utf16Splice};
use sync_protocol::records::OpClass;
use sync_protocol::types::Id;
use sync_scale_support::{
    Counting, Random, Report, Seed, as_server, client_runtime, difference, digests, domain_row,
    number, postgres_size, server_heap, server_runtime, sqlite_size,
};
use sync_store::{Backend, CheckpointInterval, Retention};
use sync_support::{DATABASE, Desktop, Folder, Proxy, Server, approve, cut_off, first_desktop};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// §22's tick.
const TICK: Duration = Duration::from_millis(500);
/// How often an editor pulls.
const FOLLOW_EVERY: Duration = Duration::from_secs(1);
/// §17's bytes a checkpoint's record keeps for good.
const CHECKPOINT_RECORD: u64 = 376;
/// A checkpoint's bucket hashes, in the log's weight (§19).
const CHECKPOINT_WEIGHT: u64 = 131_072;
/// Edits an editor makes to one note before it moves to the next, which
/// keeps a day's edits under the note size limit.
const EDITS_PER_NOTE: usize = 64;
const ROOTS: [&str; 3] = ["Shared", "Pay", "Legal"];

#[derive(Clone, Copy, Debug)]
struct Config {
    devices: usize,
    editors: usize,
    length: Duration,
    warmup: Duration,
    retained: u64,
    /// Whether the run must hold 400 pushes a second.
    hold_rate: bool,
}

impl Config {
    fn from_environment() -> Self {
        let devices = number("NOURA_SOAK_DEVICES", 6) as usize;
        Self {
            devices,
            editors: (number("NOURA_SOAK_EDITORS", 3) as usize).min(devices),
            length: Duration::from_secs(number("NOURA_SOAK_SECONDS", 20)),
            warmup: Duration::from_secs(number("NOURA_SOAK_WARMUP_SECONDS", 3)),
            retained: number("NOURA_SOAK_RETAINED_BYTES", 1 << 10),
            hold_rate: number("NOURA_SOAK_HOLD_RATE", 0) == 1,
        }
    }

    /// A short run makes checkpoints due far more often than the full one.
    fn interval(&self) -> CheckpointInterval {
        if self.retained >= 64 << 20 {
            CheckpointInterval::default()
        } else {
            CheckpointInterval {
                entries: 8,
                weight: 256 << 10,
            }
        }
    }
}

/// What a device does in the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Editor,
    /// An editor that goes away right after its last edit.
    GoneEditor,
    Writer,
}

/// One device of the fleet.
struct Member {
    desktop: Desktop,
    folder: Folder,
    sync: WorkspaceSync,
}

/// What the store-specific part of the run does.
trait Database<B: Backend> {
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
    /// Checks that no read transaction stays open while pulls wait, and
    /// returns the bytes of the write-ahead log when it reports one.
    fn no_reader_across_a_wait(server: &Server<B>) -> impl Future<Output = ()>;
    /// The write-ahead log's size now, for the numbers.
    fn wal_bytes(server: &Server<B>) -> u64;
}

struct Sqlite;

impl Sqlite {
    fn file(server: &Server<sync_store_sqlite::SqliteBackend>) -> std::path::PathBuf {
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

    async fn no_reader_across_a_wait(server: &Server<sync_store_sqlite::SqliteBackend>) {
        // A reader holding a snapshot keeps the log from truncating.
        let (busy, frames): (i64, i64) = Self::connection(server)
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("a checkpoint of the log");
        assert_eq!((busy, frames), (0, 0), "a read stayed open across a wait");
    }

    fn wal_bytes(server: &Server<sync_store_sqlite::SqliteBackend>) -> u64 {
        let mut name = Self::file(server).into_os_string();
        name.push("-wal");
        std::fs::metadata(name).map_or(0, |metadata| metadata.len())
    }
}

struct Postgres;

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

    async fn no_reader_across_a_wait(_: &Server<sync_store_postgres::PgBackend>) {}

    fn wal_bytes(_: &Server<sync_store_postgres::PgBackend>) -> u64 {
        0
    }
}

/// What the sampler tracks of one domain's checkpoint intervals.
#[derive(Clone, Copy, Debug, Default)]
struct Interval {
    /// The heaviest stretch between two checkpoints.
    largest: u64,
    /// The weight when the current stretch fell due.
    due_at: Option<u64>,
    /// The most weight appended between a due point and its checkpoint.
    during_wait: u64,
    last: u64,
}

impl Interval {
    fn sample(&mut self, row: &sync_store::DomainRow) {
        let weight = row.weight_since_checkpoint;
        self.largest = self.largest.max(weight);
        if weight < self.last {
            self.due_at = None;
        }
        self.last = weight;
        if row.checkpoint_due != 0 {
            let due_at = *self.due_at.get_or_insert(weight);
            self.during_wait = self.during_wait.max(weight.saturating_sub(due_at));
        }
    }
}

/// One domain as the soak watches it.
struct Watched {
    root: &'static str,
    id: Id,
    interval: Interval,
    floor_after_warmup: Option<u64>,
    floor: u64,
    /// The heaviest entry the samples saw: a checkpoint's weight counts its
    /// bucket hashes.
    largest_entry: u64,
}

/// Checks the bound of §19 in the form given there.
fn within_the_bound(
    watched: &Watched,
    limits: (u64, u64),
    row: &sync_store::DomainRow,
    below: u64,
    above: u64,
    servable: u64,
    taken: u64,
) -> Result<(), String> {
    let (retained, threshold) = limits;
    let largest_entry = watched.largest_entry;
    let interval = threshold + largest_entry + watched.interval.during_wait;
    if watched.interval.largest > interval {
        return Err(format!(
            "{}: an interval of {} above {interval}",
            watched.root, watched.interval.largest
        ));
    }
    let log = above + servable * CHECKPOINT_WEIGHT;
    let log_bound = retained + 2 * interval;
    if log > log_bound {
        return Err(format!(
            "{}: {log} of log weight above the bound {log_bound}",
            watched.root
        ));
    }
    // The live set at the floor exceeds today's only by what was replaced
    // since, which the log above the floor covers.
    let live_bound = row.usage_bytes + above + largest_entry;
    if below > live_bound {
        return Err(format!(
            "{}: {below} bytes at or below the floor, above {live_bound}",
            watched.root
        ));
    }
    let total = below + log + taken * CHECKPOINT_RECORD;
    println!(
        "{}: floor {} storage {total} log {log} live {} checkpoints {taken}",
        watched.root, row.retention_floor, row.usage_bytes
    );
    Ok(())
}

/// What the fleet tells the sampler and the end of the run.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    pushes: AtomicU64,
    failures: Mutex<Vec<String>>,
}

impl Shared {
    fn failed(&self, what: String) {
        self.failures.lock().unwrap().push(what);
    }
}

/// An editor's note in progress.
struct Note {
    path: String,
    document: SandboxDocument,
    length: usize,
    edits: usize,
}

/// What an editor wrote into each of its notes.
type Written = BTreeMap<String, Vec<String>>;

const NEW_NOTE: &str = "# Note\n";

async fn start_note(member: &mut Member, path: String) -> Result<Note, SyncError> {
    member.folder.write(&path, NEW_NOTE.as_bytes());
    loop {
        member.sync.round(&mut member.desktop.service, 0).await?;
        let (entered, opened) = member.sync.open_text(&path)?;
        if entered == Entered::Yes {
            return Ok(Note {
                path,
                document: opened.expect("a document"),
                length: NEW_NOTE.len(),
                edits: 0,
            });
        }
    }
}

/// Runs one editor until the fleet stops, and returns the device with what
/// it wrote. `offline` says when the proxy has cut this device off.
async fn run_editor(
    mut member: Member,
    index: usize,
    shared: Arc<Shared>,
    offline: Arc<AtomicBool>,
    seed: u64,
) -> (Member, Written) {
    let mut written = Written::new();
    let mut random = Random::new(seed ^ (index as u64 + 1));
    let root = ROOTS[index % ROOTS.len()];
    let mut number = 0u64;
    let mut note = None;
    let mut tick = tokio::time::interval(TICK);
    let mut followed = Instant::now();
    while !shared.stop.load(Ordering::SeqCst) {
        tick.tick().await;
        if note.is_none() {
            number += 1;
            let path = format!("{root}/Editor {index} note {number}.md");
            match start_note(&mut member, path).await {
                Ok(started) => note = Some(started),
                Err(_) if offline.load(Ordering::SeqCst) => continue,
                Err(error) => {
                    shared.failed(format!("editor {index}: {error:?}"));
                    break;
                }
            }
        }
        let current = note.as_mut().expect("a note");
        let token = format!("[{index}.{number}.{}]", current.edits);
        current.document.edit(
            vec![Utf16Splice {
                // Either end of the body, so no edit splits a token.
                start: if random.below(2) == 0 {
                    NEW_NOTE.len()
                } else {
                    current.length
                },
                removed: 0,
                inserted: token.clone(),
            }],
            true,
        );
        current.length += token.encode_utf16().count();
        current.edits += 1;
        written.entry(current.path.clone()).or_default().push(token);
        match member.sync.text_step(&mut member.desktop.service).await {
            Ok(_) => {
                shared.pushes.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) if offline.load(Ordering::SeqCst) => {}
            Err(error) => {
                shared.failed(format!("editor {index} step: {error:?}"));
                break;
            }
        }
        if followed.elapsed() >= FOLLOW_EVERY {
            followed = Instant::now();
            match member.sync.round(&mut member.desktop.service, 0).await {
                Ok(_) => {}
                Err(_) if offline.load(Ordering::SeqCst) => {}
                Err(error) => {
                    shared.failed(format!("editor {index} round: {error:?}"));
                    break;
                }
            }
        }
        let finished = note.as_ref().is_some_and(|n| n.edits >= EDITS_PER_NOTE);
        if finished {
            let done = note.take().expect("a note");
            drop(done.document);
            // Every other note ends as an editor that goes away right after
            // its last edit, without closing it.
            if number.is_multiple_of(2) {
                let _ = member.sync.close_text(&done.path);
            }
        }
    }
    // The last edit goes out. An even editor closes its note; an odd one
    // goes away right after, and the others take over (§22).
    if let Some(last) = note.take() {
        let path = last.path.clone();
        let _ = member.sync.text_step(&mut member.desktop.service).await;
        drop(last);
        if index.is_multiple_of(2) {
            let _ = member.sync.close_text(&path);
        }
    }
    // Nothing it signed stays behind on the way out.
    let _ = member.sync.push(&mut member.desktop.service).await;
    (member, written)
}

/// Runs an editor on a thread of its own, since a document stays with the
/// thread that holds it.
fn on_a_thread(
    member: Member,
    index: usize,
    shared: Arc<Shared>,
    offline: Arc<AtomicBool>,
    seed: u64,
) -> tokio::task::JoinHandle<(Member, Written)> {
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(run_editor(member, index, shared, offline, seed))
    })
}

/// Runs one writer until the fleet stops.
async fn run_writer(mut member: Member, index: usize, shared: Arc<Shared>, seed: u64) -> Member {
    let mut random = Random::new(seed ^ (index as u64 + 1) << 16);
    let root = ROOTS[index % ROOTS.len()];
    let path = format!("{root}/Writer {index}.md");
    let mut tick = tokio::time::interval(TICK);
    // The fleet spreads over the tick.
    tokio::time::sleep(Duration::from_millis(random.below(500) as u64)).await;
    let mut count = 0u64;
    while !shared.stop.load(Ordering::SeqCst) {
        tick.tick().await;
        count += 1;
        member.folder.write(
            &path,
            format!("# Writer {index}\n\nEdit {count}\n").as_bytes(),
        );
        let pushed = match member.sync.capture() {
            Ok(_) => member.sync.push(&mut member.desktop.service).await,
            Err(error) => Err(error),
        };
        match pushed {
            Ok(_) => {
                shared.pushes.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                shared.failed(format!("writer {index}: {error:?}"));
                break;
            }
        }
    }
    member
}

/// Rounds until the device has nothing left to apply or send.
async fn settle(member: &mut Member, what: &str) {
    let mut quiet = 0;
    for _ in 0..10_000 {
        let round = member
            .sync
            .round(&mut member.desktop.service, 0)
            .await
            .unwrap_or_else(|error| panic!("{what}: {error:?}"));
        let busy = round.applied + round.captured + round.pushed > 0 || round.waiting_for_key;
        quiet = if busy { 0 } else { quiet + 1 };
        if quiet == 2 && member.sync.status().pending == 0 {
            return;
        }
    }
    panic!("{what}: never settled");
}

fn soak<B, D, S, F>(store: &str, start: S)
where
    B: Backend,
    D: Database<B>,
    S: FnOnce(CheckpointInterval, Retention) -> F,
    F: Future<Output = Server<B>>,
{
    let config = Config::from_environment();
    let retention = Retention {
        retained_bytes: config.retained,
        ..Retention::default()
    };
    let server_side = server_runtime();
    let server = as_server(|| server_side.block_on(start(config.interval(), retention)));
    client_runtime().block_on(drive::<B, D>(store, config, server));
    server_side.shutdown_background();
}

async fn drive<B: Backend, D: Database<B>>(store: &str, config: Config, server: Server<B>) {
    let seed = Seed::take("NOURA_SOAK_SEED");
    let mut report = Report::new("soak", store, seed.value);
    report.set("devices", config.devices);
    report.set("seconds", config.length.as_secs());
    let started = Instant::now();
    let server = Arc::new(server);

    // The fleet joins, one device at a time.
    let mut first = first_desktop(&server, "ana@example.com").await;
    let mut folder = Folder::new("Soak");
    let mut sync = WorkspaceSync::turn_on(
        &mut first.service,
        folder.engine.clone(),
        "Soak",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.set_checkpoint_wait(Duration::ZERO);
    sync.round(&mut first.service, 0).await.expect("a round");
    let workspace = sync.workspace();
    let mut domains = vec![(ROOTS[0], sync.domain())];
    for root in &ROOTS[1..] {
        let domain = sync
            .restrict_folder(&mut first.service, root, &[])
            .await
            .expect("restricted");
        domains.push((*root, domain));
    }
    sync.round(&mut first.service, 0).await.expect("a round");

    let proxy = Proxy::start(&server.base).await;
    proxy.traffic().quiet.store(true, Ordering::SeqCst);
    let mut others = Vec::new();
    for index in 1..config.devices {
        let mut desktop = approve(
            &server,
            &mut first,
            std::slice::from_mut(sync.remote_mut().expect("connected")),
        )
        .await;
        sync.round(&mut first.service, 0).await.expect("a round");
        let mut folder = Folder::new(&format!("Device {index}"));
        let joined = WorkspaceSync::download(
            &mut desktop.service,
            folder.engine.clone(),
            workspace,
            &mut folder.registry,
        )
        .await
        .expect("the download starts");
        // The first editors after the laptop go through the proxy, so the
        // run can cut one off.
        if index == 1 {
            desktop.service = desktop.connect_through(&proxy.base).await;
        }
        let mut member = Member {
            desktop,
            folder,
            sync: joined,
        };
        member.sync.set_checkpoint_wait(Duration::ZERO);
        settle(&mut member, "joining").await;
        others.push(member);
        if index % 16 == 15 {
            D::clear_requests(&server).await;
        }
    }
    report.seconds("join_seconds", started);

    // The fleet writes.
    let shared = Arc::new(Shared::default());
    let offline = Arc::new(AtomicBool::new(false));
    let cut_device = config.editors > 1;
    let mut editors = Vec::new();
    let mut writers = Vec::new();
    editors.push(on_a_thread(
        Member {
            desktop: first,
            folder,
            sync,
        },
        0,
        shared.clone(),
        Arc::new(AtomicBool::new(false)),
        seed.value,
    ));
    for (position, member) in others.into_iter().enumerate() {
        let index = position + 1;
        if index < config.editors {
            let offline = if index == 1 {
                offline.clone()
            } else {
                Arc::new(AtomicBool::new(false))
            };
            editors.push(on_a_thread(
                member,
                index,
                shared.clone(),
                offline,
                seed.value,
            ));
        } else {
            writers.push(tokio::spawn(run_writer(
                member,
                index,
                shared.clone(),
                seed.value,
            )));
        }
    }

    let load = Instant::now();
    let mut watched: Vec<Watched> = domains
        .iter()
        .map(|(root, id)| Watched {
            root,
            id: *id,
            interval: Interval::default(),
            floor_after_warmup: None,
            floor: 0,
            largest_entry: 0,
        })
        .collect();
    let threshold = config.interval().weight;
    let cut_at = config.length.mul_f64(0.5);
    let back_at = cut_at + Duration::from_secs(number("NOURA_SOAK_CUT_SECONDS", 2));
    let mut cut_state = 0;
    let mut heap_after_warmup = None;
    let mut wal_peak = 0;
    let mut pushes_at_warmup = 0;
    let mut warmed = None;
    let every = Duration::from_secs(number("NOURA_SOAK_SAMPLE_SECONDS", 1));
    let mut bound_failure = None;
    while load.elapsed() < config.length && shared.failures.lock().unwrap().is_empty() {
        tokio::time::sleep(every).await;
        if cut_device && cut_state == 0 && load.elapsed() >= cut_at {
            offline.store(true, Ordering::SeqCst);
            cut_off(&proxy);
            cut_state = 1;
        }
        if cut_state == 1 && load.elapsed() >= back_at {
            proxy.honest();
            // A request the cut answered may still be on its way back.
            tokio::time::sleep(Duration::from_secs(1)).await;
            offline.store(false, Ordering::SeqCst);
            cut_state = 2;
        }
        server.store.maintain().await.expect("a background pass");
        wal_peak = wal_peak.max(D::wal_bytes(&server));
        if heap_after_warmup.is_none() && load.elapsed() >= config.warmup {
            heap_after_warmup = Some(server_heap());
            pushes_at_warmup = shared.pushes.load(Ordering::Relaxed);
            warmed = Some(Instant::now());
        }
        for domain in &mut watched {
            let row = domain_row(&server.store, workspace, domain.id).await;
            domain.interval.sample(&row);
            domain.floor = row.retention_floor;
            if heap_after_warmup.is_some() {
                domain.floor_after_warmup.get_or_insert(row.retention_floor);
            }
            let (below, above, longest) =
                D::log_bytes(&server, workspace, domain.id, row.retention_floor).await;
            domain.largest_entry = domain.largest_entry.max(longest + CHECKPOINT_WEIGHT);
            let (taken, servable) =
                D::checkpoints(&server, workspace, domain.id, row.retention_floor).await;
            let checked = within_the_bound(
                domain,
                (config.retained, threshold),
                &row,
                below,
                above,
                servable,
                taken,
            );
            if let Err(problem) = checked {
                bound_failure.get_or_insert(problem);
            }
        }
    }
    shared.stop.store(true, Ordering::SeqCst);
    let measured = warmed.map(|at| at.elapsed());
    let pushes_held = shared.pushes.load(Ordering::Relaxed) - pushes_at_warmup;
    let rate = measured.map_or(0.0, |length| pushes_held as f64 / length.as_secs_f64());
    report.set("pushes", shared.pushes.load(Ordering::Relaxed));
    report.set("pushes_per_second", (rate * 10.0).round() / 10.0);
    report.set("wal_peak_bytes", wal_peak);
    proxy.honest();
    offline.store(false, Ordering::SeqCst);

    let mut members = Vec::new();
    let mut written = Written::new();
    for (position, editor) in editors.into_iter().enumerate() {
        let (member, wrote) = editor.await.expect("an editor");
        let role = if position % 2 == 1 {
            Role::GoneEditor
        } else {
            Role::Editor
        };
        members.push((member, role));
        written.extend(wrote);
    }
    for writer in writers {
        members.push((writer.await.expect("a writer"), Role::Writer));
    }
    let failures = shared.failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "{failures:?}");
    if let Some(problem) = bound_failure {
        panic!("{problem}");
    }
    if config.hold_rate {
        assert!(
            rate >= 2.0 * config.devices as f64 * 0.98,
            "{rate} pushes a second"
        );
    }

    // No read stays open while pulls wait.
    let pulling = {
        let waiter = members
            .iter_mut()
            .find(|(_, role)| *role == Role::Editor)
            .expect("one");
        settle(&mut waiter.0, "before the wait").await;
        let round = waiter.0.sync.round(&mut waiter.0.desktop.service, 2);
        let check = async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            D::no_reader_across_a_wait(&server).await;
        };
        tokio::join!(round, check).0
    };
    pulling.expect("the pull ends");

    // Everyone but the editors that went away catches up, the writers from
    // a checkpoint if the floor passed them. Any writer then takes over the
    // notes those editors left, one device at a time, as the 5-minute rule
    // does when one device's snapshot reaches the others first.
    let catch_up = Instant::now();
    let (away, mut here): (Vec<_>, Vec<_>) = members
        .into_iter()
        .partition(|(_, role)| *role == Role::GoneEditor);
    for _ in 0..3 {
        for (member, _) in &mut here {
            settle(member, "catching up").await;
        }
        server.store.maintain().await.expect("a background pass");
    }
    let later = Instant::now() + IDLE_ANY_WRITER + Duration::from_secs(1);
    for (member, _) in &mut here {
        settle(member, "before the idle rule").await;
        member.sync.text_tick(later).expect("the idle rule");
        settle(member, "after the idle rule").await;
    }
    let mut members = here;
    members.extend(away);
    // The run skipped the 5 minutes those editors stayed away, so their last
    // pull counts as that old.
    for (member, role) in &mut members {
        if *role == Role::GoneEditor {
            member.sync.set_pulled_within(Duration::ZERO);
        }
    }
    for (member, _) in &mut members {
        settle(member, "after the idle rule").await;
    }
    for (member, _) in &mut members {
        settle(member, "after the idle rule").await;
    }
    report.seconds("catch_up_seconds", catch_up);

    // Every device holds every operation, once, and the same files.
    let expected = digests(members[0].0.folder.dir.path());
    assert!(expected.len() > 3, "the run wrote notes");
    for (member, _) in &members {
        let found = digests(member.folder.dir.path());
        let differ = difference(&expected, &found);
        assert!(
            differ.is_empty(),
            "{}: {:?}",
            differ.len(),
            &differ[..differ.len().min(20)]
        );
    }
    let mut tokens = 0;
    for (path, wrote) in &written {
        let text = std::fs::read_to_string(members[0].0.folder.path(path)).expect("the note");
        for token in wrote {
            tokens += 1;
            assert_eq!(text.matches(token.as_str()).count(), 1, "{token} in {path}");
        }
    }
    report.set("edits_checked", tokens);

    // A note editing left holds one snapshot and at most 8 live updates.
    let head = {
        let (member, _) = &members[0];
        let mut checked = 0;
        let mut largest = 0usize;
        for path in written.keys() {
            let Some(rid) = member.sync.rid_at(path) else {
                continue;
            };
            let domain = member
                .sync
                .object_domain(&rid)
                .unwrap_or_else(|| member.sync.domain());
            let row = domain_row(&server.store, workspace, domain).await;
            let live = server
                .store
                .live_at(workspace, rid, row.head_seq)
                .await
                .expect("the live set");
            let patches = live
                .iter()
                .filter(|(_, class)| *class == OpClass::Patch)
                .count();
            let states = live.len() - patches;
            assert!(patches <= IDLE_PATCHES, "{path}: {patches} live updates");
            assert!(
                states <= 1,
                "{path}: {states} snapshots: {:?}",
                live.iter()
                    .map(|(op, class)| (op.device, op.counter, op.seq, *class))
                    .collect::<Vec<_>>()
            );
            largest = largest.max(live.len());
            checked += 1;
        }
        report.set("notes_checked", checked);
        largest
    };
    report.set("largest_live_records_per_note", head);

    // The floors moved, and server memory came back.
    for domain in &watched {
        let after_warmup = domain.floor_after_warmup.unwrap_or(0);
        assert!(
            domain.floor > after_warmup || (config.editors <= domain_index(domain)),
            "{}: the floor stayed at {after_warmup}",
            domain.root
        );
        report.set(&format!("floor_{}", domain.root), domain.floor);
        report.set(
            &format!("largest_interval_{}", domain.root),
            domain.interval.largest,
        );
    }
    let end_heap = server_heap();
    report.set("server_heap_end_bytes", end_heap);
    if let Some(warm) = heap_after_warmup {
        report.set("server_heap_after_warmup_bytes", warm);
        // A plain run ends before the caches settle.
        if config.length >= Duration::from_secs(600) {
            assert!(
                end_heap as f64 <= warm as f64 * 1.1,
                "the server's heap went from {warm} to {end_heap}"
            );
        }
    }
    report.set("database_bytes", D::size(&server).await);
    report.seconds("duration_seconds", started);
    report.finish();
    drop(members);
    drop(seed);
}

/// The domain's place in the fleet's order, which says who writes there.
fn domain_index(domain: &Watched) -> usize {
    ROOTS
        .iter()
        .position(|root| *root == domain.root)
        .unwrap_or(0)
}

#[test]
fn the_fleet_soaks_on_sqlite() {
    soak::<_, Sqlite, _, _>("sqlite", Server::start_with);
}

#[test]
fn the_fleet_soaks_on_postgres() {
    soak::<_, Postgres, _, _>("postgres", Server::start_postgres_with);
}
