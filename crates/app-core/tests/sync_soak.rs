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
//! The run ends with a bounded catch-up. The editors and a sample of writers
//! catch up and are compared in full. The other writers would each cost a
//! bootstrap, so their part is checked instead by what they signed: the
//! server's chain head counts every record they pushed, and the verified
//! devices hold each one's last edit. An overrun fails the run with its seed.
//!
//! A plain test run takes seconds with a few devices. The scheduled job sets
//! `NOURA_SOAK_DEVICES=200`, `NOURA_SOAK_SECONDS=86400`,
//! `NOURA_SOAK_RETAINED_BYTES=67108864`, `NOURA_SOAK_WARMUP_SECONDS=600`,
//! `NOURA_SOAK_VERIFIED_WRITERS=16` and `NOURA_SOAK_HOLD_RATE=1`.
//! `NOURA_SOAK_SEED` repeats a run's choices, and `NOURA_SCALE_REPORT` names
//! the file the numbers go to.

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
    Counting, Database, Postgres, Random, Report, Seed, Sqlite, as_server, client_runtime,
    difference, digests, number, on_server, reader_stayed_open, server_heap, server_runtime,
};
use sync_store::{Backend, CheckpointInterval, Retention};
use sync_support::{
    Desktop, Folder, Proxy, Server, approve, chain_head, cut_off, domain_row, first_desktop,
};

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
/// How long the quiet device's pull waits, in seconds.
const QUIET_WAIT: u32 = 10;
/// How often the run looks for a read held across that wait.
const READ_CHECK_EVERY: Duration = Duration::from_secs(13);
/// How long one look lasts: under the wait, and short enough that most
/// looks fall inside one wait.
const READ_CHECK_WINDOW: Duration = Duration::from_secs(3);
/// How many devices catch up at once at the end.
const CATCH_UP_WORKERS: usize = 8;
/// How many heap samples stand for the run's start and its end.
const HEAP_SAMPLES: usize = 5;

#[derive(Clone, Copy, Debug)]
struct Config {
    devices: usize,
    editors: usize,
    length: Duration,
    warmup: Duration,
    retained: u64,
    /// Whether the run must hold 400 pushes a second.
    hold_rate: bool,
    /// Writers that catch up at the end and are compared in full.
    verified_writers: usize,
    /// What the end of the run may take before it counts as failed.
    catch_up_limit: Duration,
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
            verified_writers: number("NOURA_SOAK_VERIFIED_WRITERS", devices as u64) as usize,
            catch_up_limit: Duration::from_secs(number("NOURA_SOAK_CATCH_UP_SECONDS", 3600)),
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
    /// Pushes that carried records.
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
            Ok(signed) => {
                shared
                    .pushes
                    .fetch_add(u64::from(signed > 0), Ordering::Relaxed);
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

/// Runs `work` on a thread of its own: a device's rounds aren't `Send`, and
/// a document stays with the thread that holds it.
fn on_a_thread<T, F>(work: impl FnOnce() -> F + Send + 'static) -> tokio::task::JoinHandle<T>
where
    T: Send + 'static,
    F: Future<Output = T>,
{
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(work())
    })
}

/// A writer at the end of the run: the device, its note, the domain's root
/// and the records the server acknowledged.
struct Finished {
    member: Member,
    path: String,
    root: &'static str,
    acknowledged: u64,
}

/// Runs one writer until the fleet stops.
async fn run_writer(mut member: Member, index: usize, shared: Arc<Shared>, seed: u64) -> Finished {
    let mut random = Random::new(seed ^ (index as u64 + 1) << 16);
    let root = ROOTS[index % ROOTS.len()];
    let path = format!("{root}/Writer {index}.md");
    let mut tick = tokio::time::interval(TICK);
    // The fleet spreads over the tick.
    tokio::time::sleep(Duration::from_millis(random.below(500) as u64)).await;
    let mut count = 0u64;
    let mut acknowledged = 0u64;
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
            Ok(pushed) => {
                acknowledged += pushed as u64;
                shared
                    .pushes
                    .fetch_add(u64::from(pushed > 0), Ordering::Relaxed);
            }
            Err(error) => {
                shared.failed(format!("writer {index}: {error:?}"));
                break;
            }
        }
    }
    Finished {
        member,
        path,
        root,
        acknowledged,
    }
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

/// Lets every one of `members` settle, a few at a time on threads of their
/// own, and gives them back in order.
async fn settle_all<R: Send + 'static>(
    members: Vec<(Member, R)>,
    what: &'static str,
) -> Vec<(Member, R)> {
    let room = Arc::new(tokio::sync::Semaphore::new(CATCH_UP_WORKERS));
    let mut settling = Vec::new();
    for (mut member, role) in members {
        let room = room.clone();
        settling.push(tokio::spawn(async move {
            let _turn = room.acquire_owned().await.expect("open");
            on_a_thread(move || async move {
                settle(&mut member, what).await;
                (member, role)
            })
            .await
            .expect("a device settled")
        }));
    }
    let mut settled = Vec::new();
    for member in settling {
        settled.push(member.await.expect("a device settled"));
    }
    settled
}

/// A device of another account in a workspace nothing writes to, so its
/// pulls really wait while the fleet loads the same database.
struct Quiet {
    desktop: Desktop,
    _folder: Folder,
    sync: WorkspaceSync,
}

async fn quiet_device<B: Backend>(server: &Server<B>) -> Quiet {
    let mut desktop = first_desktop(server, "bo@example.com").await;
    let mut folder = Folder::new("Quiet");
    let mut sync = WorkspaceSync::turn_on(
        &mut desktop.service,
        folder.engine.clone(),
        "Quiet",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.round(&mut desktop.service, 0).await.expect("a round");
    sync.round(&mut desktop.service, 0).await.expect("a round");
    Quiet {
        desktop,
        _folder: folder,
        sync,
    }
}

/// The quiet device's pulls until the fleet stops, with how many of them
/// waited.
async fn wait_quietly(mut quiet: Quiet, shared: Arc<Shared>) -> (Quiet, u64) {
    let mut waited = 0;
    while !shared.stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let round = quiet
            .sync
            .round(&mut quiet.desktop.service, QUIET_WAIT)
            .await;
        match round {
            Ok(_) => waited += u64::from(started.elapsed().as_secs() >= u64::from(QUIET_WAIT)),
            Err(error) => {
                shared.failed(format!("the quiet device: {error:?}"));
                break;
            }
        }
    }
    (quiet, waited)
}

/// Looks for a read held across the quiet device's wait while the fleet
/// runs. A look can land between two waits, so one that finds nothing proves
/// little; one that finds a read held for all of its window fails the run.
async fn watch_reads<B: Backend, D: Database<B>>(
    server: Arc<Server<B>>,
    shared: Arc<Shared>,
) -> u64 {
    let mut looks = 0;
    loop {
        for _ in 0..READ_CHECK_EVERY.as_secs() {
            if shared.stop.load(Ordering::SeqCst) {
                return looks;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let held = {
            let server = server.clone();
            tokio::task::spawn_blocking(move || D::reader_stayed_open(&server, READ_CHECK_WINDOW))
                .await
                .expect("a look at the log")
        };
        looks += 1;
        if held {
            shared.failed(format!(
                "a read stayed open for {READ_CHECK_WINDOW:?} during a pull's wait"
            ));
        }
    }
}

/// The middle value of `samples`, which a stray sample leaves alone.
fn middle(samples: &[u64]) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
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
    let server_handle = server_side.handle().clone();
    client_runtime().block_on(drive::<B, D>(store, config, server, server_handle));
    server_side.shutdown_background();
}

async fn drive<B: Backend, D: Database<B>>(
    store: &str,
    config: Config,
    server: Server<B>,
    server_side: tokio::runtime::Handle,
) {
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

    // The fleet writes, while a device elsewhere waits on the same database.
    let shared = Arc::new(Shared::default());
    let quiet = {
        let (quiet, shared) = (quiet_device(&server).await, shared.clone());
        on_a_thread(move || wait_quietly(quiet, shared))
    };
    let read_checks = tokio::spawn(watch_reads::<B, D>(server.clone(), shared.clone()));
    let offline = Arc::new(AtomicBool::new(false));
    let cut_device = config.editors > 1;
    let mut editors = Vec::new();
    let mut writers = Vec::new();
    let first_editor = Member {
        desktop: first,
        folder,
        sync,
    };
    {
        let (shared, seed) = (shared.clone(), seed.value);
        let offline = Arc::new(AtomicBool::new(false));
        editors.push(on_a_thread(move || {
            run_editor(first_editor, 0, shared, offline, seed)
        }));
    }
    for (position, member) in others.into_iter().enumerate() {
        let index = position + 1;
        if index < config.editors {
            let offline = if index == 1 {
                offline.clone()
            } else {
                Arc::new(AtomicBool::new(false))
            };
            let (shared, seed) = (shared.clone(), seed.value);
            editors.push(on_a_thread(move || {
                run_editor(member, index, shared, offline, seed)
            }));
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
    let mut wal_peak = 0;
    let mut pushes_at_warmup = 0;
    let mut warmed = None;
    let mut heaps = Vec::new();
    let heads = |watched: &[Watched]| {
        let store = server.store.clone();
        let ids: Vec<Id> = watched.iter().map(|domain| domain.id).collect();
        let server_side = server_side.clone();
        async move {
            on_server(&server_side, async move {
                let mut sum = 0;
                for id in ids {
                    sum += domain_row(&store, workspace, id).await.head_seq;
                }
                sum
            })
            .await
        }
    };
    let mut entries_at_warmup = 0;
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
        maintain(&server, &server_side).await;
        wal_peak = wal_peak.max(D::wal_bytes(&server));
        if warmed.is_none() && load.elapsed() >= config.warmup {
            pushes_at_warmup = shared.pushes.load(Ordering::Relaxed);
            entries_at_warmup = heads(&watched).await;
            warmed = Some(Instant::now());
        }
        if warmed.is_some() {
            heaps.push(server_heap());
        }
        for domain in &mut watched {
            let row = {
                let store = server.store.clone();
                let id = domain.id;
                on_server(&server_side, async move {
                    domain_row(&store, workspace, id).await
                })
                .await
            };
            domain.interval.sample(&row);
            domain.floor = row.retention_floor;
            if warmed.is_some() {
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
    let measured = warmed.map_or(Duration::ZERO, |at| at.elapsed());
    let pushes_held = shared.pushes.load(Ordering::Relaxed) - pushes_at_warmup;
    let per_second = |count: u64| count as f64 / measured.as_secs_f64().max(0.001);
    let rate = per_second(pushes_held);
    let entries = heads(&watched).await.saturating_sub(entries_at_warmup);
    println!("{rate:.1} pushes a second after the warmup");
    report.set("pushes", shared.pushes.load(Ordering::Relaxed));
    report.set("pushes_per_second", (rate * 10.0).round() / 10.0);
    report.set(
        "log_entries_per_second",
        (per_second(entries) * 10.0).round() / 10.0,
    );
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
    let mut unverified = Vec::new();
    let mut writer_notes = Vec::new();
    for (position, writer) in writers.into_iter().enumerate() {
        let writer = writer.await.expect("a writer");
        writer_notes.push(writer.path.clone());
        if position < config.verified_writers {
            members.push((writer.member, Role::Writer));
        } else {
            unverified.push(writer);
        }
    }
    let (quiet, quiet_waits) = quiet.await.expect("the quiet device");
    report.set("quiet_waits", quiet_waits);
    report.set("reads_checked", read_checks.await.expect("the read checks"));
    let failures = shared.failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "{failures:?}");

    // No read stays open across a wait on a quiet database either. A write
    // first gives the log frames, since a read on an empty log blocks
    // nothing.
    let mut quiet = quiet;
    let writer = members
        .iter_mut()
        .map(|(member, _)| member)
        .next()
        .expect("a device");
    writer.folder.write("Before the wait.md", b"# Wait\n");
    settle(writer, "before the wait").await;
    let pulling = {
        let round = quiet.sync.round(&mut quiet.desktop.service, 3);
        let check = async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let server = server.clone();
            tokio::task::spawn_blocking(move || {
                D::reader_stayed_open(&server, Duration::from_secs(2))
            })
            .await
            .expect("a look at the log")
        };
        let (round, held) = tokio::join!(round, check);
        assert!(!held, "a read stayed open across a wait");
        round
    };
    pulling.expect("the pull ends");

    // Everyone verified but the editors that went away catches up, the
    // writers from a checkpoint if the floor passed them. Any writer then
    // takes over the notes those editors left, one device at a time, as the
    // 5-minute rule does when one device's snapshot reaches the others
    // first.
    let catch_up = Instant::now();
    let members = tokio::time::timeout(
        config.catch_up_limit,
        catch_up_fleet::<B>(members, &server, &server_side),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "catching up took more than {:?}: the seed above reruns it",
            config.catch_up_limit
        )
    });
    report.seconds("catch_up_seconds", catch_up);
    report.set("devices_verified", members.len());

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

    // A writer that didn't catch up lost nothing: the server's chain head
    // counts each record it pushed, and its last edit reached the others.
    let mut records = 0;
    let domain_of = |root: &str| {
        domains
            .iter()
            .find(|(name, _)| *name == root)
            .map(|(_, id)| *id)
            .expect("a domain")
    };
    for writer in &unverified {
        let device = writer.member.desktop.id();
        let store = server.store.clone();
        let domain = domain_of(writer.root);
        let head = on_server(&server_side, async move {
            chain_head(&store, workspace, domain, device).await
        })
        .await;
        assert_eq!(
            head.map(|head| head.counter),
            Some(writer.acknowledged),
            "{}: the chain head against what it pushed",
            writer.path
        );
        records += writer.acknowledged;
    }
    report.set("unverified_writers", unverified.len());
    report.set("unverified_records", records);
    for writer in &unverified {
        let here = std::fs::read(writer.member.folder.path(&writer.path)).expect("its note");
        let there = std::fs::read(members[0].0.folder.path(&writer.path)).expect("its copy");
        assert_eq!(here, there, "{}: the last edit crossed", writer.path);
    }
    report.set("writer_notes", writer_notes.len());

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
            let store = server.store.clone();
            let live = on_server(&server_side, async move {
                let row = domain_row(&store, workspace, domain).await;
                store
                    .live_at(workspace, rid, row.head_seq)
                    .await
                    .expect("the live set")
            })
            .await;
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
    // The same sample at either end, under load: the first after the warmup
    // against the last before the fleet stopped.
    let warm = middle(&heaps[..heaps.len().min(HEAP_SAMPLES)]);
    let end = middle(&heaps[heaps.len().saturating_sub(HEAP_SAMPLES)..]);
    report.set("server_heap_after_warmup_bytes", warm);
    report.set("server_heap_end_bytes", end);
    report.set("server_heap_after_catch_up_bytes", server_heap());
    report.set("database_bytes", D::size(&server).await);
    report.seconds("duration_seconds", started);

    // A run that missed a bar still verified everything above, and its
    // numbers go out with it.
    if let Some(problem) = bound_failure {
        panic!("{problem}");
    }
    if config.hold_rate {
        // One tick's pushes can fall outside the measured time.
        let seconds = measured.as_secs();
        let devices = config.devices as u64;
        assert!(
            pushes_held + devices >= 2 * devices * seconds,
            "{pushes_held} pushes in {seconds} seconds, under 2 for each of {devices} devices"
        );
    }
    // A plain run ends before the caches settle.
    if config.length >= Duration::from_secs(600) {
        assert!(
            end as f64 <= warm as f64 * 1.1,
            "the server's heap went from {warm} to {end}"
        );
    }
    report.finish();
    drop(members);
    drop(unverified);
    drop(seed);
}

/// A background pass, on the server's own runtime.
async fn maintain<B: Backend>(server: &Server<B>, server_side: &tokio::runtime::Handle) {
    let store = server.store.clone();
    on_server(server_side, async move {
        store.maintain().await.expect("a background pass");
    })
    .await;
}

/// Catches `members` up, and has the writers among them take over what the
/// editors that went away left.
async fn catch_up_fleet<B: Backend>(
    members: Vec<(Member, Role)>,
    server: &Server<B>,
    server_side: &tokio::runtime::Handle,
) -> Vec<(Member, Role)> {
    let (away, mut here): (Vec<_>, Vec<_>) = members
        .into_iter()
        .partition(|(_, role)| *role == Role::GoneEditor);
    for _ in 0..3 {
        here = settle_all(here, "catching up").await;
        maintain(server, server_side).await;
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
    for _ in 0..2 {
        members = settle_all(members, "after the idle rule").await;
    }
    members
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

/// The wait check finds a read held open, which a checkpoint that waits for
/// it wouldn't, and passes once the read ends.
#[test]
fn a_read_held_across_a_wait_fails_the_check() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let file = dir.path().join("wait.sqlite3");
    let writer = rusqlite::Connection::open(&file).expect("the database");
    writer
        .execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE t (n); INSERT INTO t VALUES (1);")
        .expect("written");
    let window = Duration::from_millis(300);
    assert!(!reader_stayed_open(&file, window), "nothing reads yet");

    writer
        .execute_batch("INSERT INTO t VALUES (2);")
        .expect("written");
    let reader = rusqlite::Connection::open(&file).expect("the database");
    reader
        .execute_batch("BEGIN; SELECT COUNT(*) FROM t;")
        .expect("a snapshot");
    writer
        .execute_batch("INSERT INTO t VALUES (3);")
        .expect("written");
    assert!(reader_stayed_open(&file, window), "the snapshot is open");

    reader.execute_batch("COMMIT").expect("ended");
    assert!(!reader_stayed_open(&file, window), "the snapshot ended");
}
