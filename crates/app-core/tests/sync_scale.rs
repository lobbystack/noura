//! The worked example of sync-v2.md §28 at seam 4: a workspace of notes and
//! attachments syncs up and bootstraps onto a fresh device from a
//! checkpoint, with identical files and IDs; no request or response goes
//! over 1 MiB; one edit crosses with no walk and nothing unchanged resent;
//! and removing a device re-encrypts no object.
//!
//! The server runs at the default rate limits of §26, so the numbers show
//! the pacing a real client gets: the client waits out `rate_limited`.
//!
//! A plain test run builds a small workspace. The scheduled job sets the
//! sizes of §28: `NOURA_SCALE_NOTES=95000`, `NOURA_SCALE_ATTACHMENTS=5000`,
//! `NOURA_SCALE_DEVICES=197`. Every device belongs to one account, so each
//! reads all 3 domains, and a removal uploads 199 envelopes per domain where
//! §28's 2 restricted domains hold 20 devices. `NOURA_SCALE_SEED` repeats a
//! run, and `NOURA_SCALE_REPORT` names the file
//! the numbers go to. The counting allocator serves the whole binary, so
//! each store's run takes this binary's process alone: CI runs one test at
//! a time, as the cargo commands in the workflow show.

mod sync_scale_support;
mod sync_support;

use std::collections::BTreeSet;
use std::future::Future;
use std::time::{Duration, Instant};

use app_core::sync::SyncService;
use app_core::sync::workspace::WorkspaceSync;
use sync_protocol::codec::Decode;
use sync_protocol::messages::{MAX_BODY, PushRequest, RecordRef};
use sync_protocol::types::Id;
use sync_scale_support::{
    Counting, Database, Postgres, Random, Report, Seed, Sqlite, as_server, client_runtime,
    difference, digests, number, server_runtime,
};
use sync_server::{RateGate, RateLimits};
use sync_store::{Backend, CheckpointInterval, Limit, Retention};
use sync_support::{Desktop, Folder, Proxy, Server, approve, domain_row, first_desktop};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many notes and attachments go into each of the two restricted
/// folders; the rest stay in the workspace domain.
const RESTRICTED_SHARE: usize = 50;
const NOTE_BYTES: usize = 3 << 10;
const ATTACHMENT_BYTES: usize = 2 << 20;

#[derive(Clone, Copy)]
struct Sizes {
    notes: usize,
    attachments: usize,
    attachment_bytes: usize,
    /// Devices beside the first, the fresh one and the removed one.
    other_devices: usize,
}

impl Sizes {
    fn from_environment() -> Self {
        Self {
            notes: number("NOURA_SCALE_NOTES", 300) as usize,
            attachments: number("NOURA_SCALE_ATTACHMENTS", 4) as usize,
            attachment_bytes: number("NOURA_SCALE_ATTACHMENT_BYTES", ATTACHMENT_BYTES as u64)
                as usize,
            other_devices: number("NOURA_SCALE_DEVICES", 1) as usize,
        }
    }

    fn objects(&self) -> usize {
        self.notes + self.attachments
    }
}

/// Where the `index`th object goes, spread over 256 folders, with a share
/// in each restricted folder.
fn folder_of(index: usize) -> String {
    let bucket = index % 256;
    match index % RESTRICTED_SHARE {
        0 => format!("Pay/{bucket:02x}"),
        1 => format!("Legal/{bucket:02x}"),
        _ => format!("Notes/{bucket:02x}"),
    }
}

fn note_path(index: usize) -> String {
    format!("{}/Note {index}.md", folder_of(index))
}

/// A managed note of about 3 KiB, its ID drawn from `random`.
fn note(random: &mut Random) -> String {
    format!(
        "---\nid: note_01k{:016x}000000000\ntype: note\n---\n\n# Note\n\n{}\n",
        random.next(),
        random.text(NOTE_BYTES - 64),
    )
}

/// Builds the workspace in `folder`, in order, so a seed names its bytes.
fn build(folder: &Folder, sizes: Sizes, seed: u64) {
    let mut random = Random::new(seed);
    for index in 0..sizes.notes {
        folder.write(&note_path(index), note(&mut random).as_bytes());
    }
    for index in 0..sizes.attachments {
        let path = format!("{}/Attachment {index}.bin", folder_of(index));
        folder.write(&path, &random.bytes(sizes.attachment_bytes));
    }
}

/// Rounds until `sync` has nothing left to send, its cursor reached the
/// log's head, and two rounds in a row did nothing.
async fn settle<B: Backend>(
    server: &Server<B>,
    sync: &mut WorkspaceSync,
    service: &mut SyncService,
    what: &str,
) {
    let (workspace, domain) = (sync.workspace(), sync.domain());
    let mut quiet = 0;
    for _ in 0..50_000 {
        let round = sync.round(service, 0).await.expect(what);
        let busy = round.applied + round.captured + round.pushed > 0 || round.waiting_for_key;
        quiet = if busy { 0 } else { quiet + 1 };
        if quiet >= 2
            && sync.status().pending == 0
            && sync.cursor() >= domain_row(&server.store, workspace, domain).await.head_seq
        {
            return;
        }
    }
    panic!("{what}: never settled");
}

fn bodies_stayed_small(proxy: &Proxy) -> (usize, usize) {
    let traffic = proxy.traffic();
    let oversized = traffic.oversized.load(std::sync::atomic::Ordering::SeqCst);
    let request = traffic
        .largest_request
        .load(std::sync::atomic::Ordering::SeqCst);
    let response = traffic
        .largest_response
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(oversized, 0, "a request over the proxy's limit");
    assert!(request <= MAX_BODY, "a request of {request} bytes");
    assert!(response <= MAX_BODY, "a response of {response} bytes");
    (request, response)
}

/// What the records pushed since `proxy` last forgot hold.
#[derive(Debug, Default)]
struct Pushed {
    pushes: usize,
    operations: usize,
    envelopes: usize,
    epochs: usize,
    policy: usize,
    checkpoints: usize,
    /// The domains of the epoch bundles in each push that carried any.
    rotations: Vec<Vec<Id>>,
}

fn pushed(proxy: &Proxy) -> Pushed {
    let mut pushed = Pushed::default();
    for request in proxy.requests() {
        if request.method != "POST" || !request.path.ends_with("/ops") {
            continue;
        }
        let body = PushRequest::from_bytes(&request.body).expect("a push");
        pushed.pushes += 1;
        let mut rotated = Vec::new();
        for record in body.records.iter() {
            match record {
                RecordRef::Operation(_) => pushed.operations += 1,
                RecordRef::Envelope(_) => pushed.envelopes += 1,
                RecordRef::EpochBundle(bundle) => {
                    pushed.epochs += 1;
                    rotated.push(bundle.epoch.0.body.domain);
                }
                RecordRef::Policy(_) => pushed.policy += 1,
                RecordRef::CheckpointBundle(_) => pushed.checkpoints += 1,
                _ => {}
            }
        }
        if !rotated.is_empty() {
            pushed.rotations.push(rotated);
        }
    }
    pushed
}

fn blob_requests(proxy: &Proxy) -> u64 {
    proxy
        .traffic()
        .routes()
        .iter()
        .filter(|(name, _)| name.ends_with(" blob"))
        .map(|(_, route)| route.requests)
        .sum()
}

/// The range requests that downloaded attachments, which uploads don't
/// count toward.
fn download_ranges(proxy: &Proxy) -> u64 {
    proxy
        .traffic()
        .routes()
        .get("GET blob")
        .map_or(0, |route| route.requests)
}

/// The requests the server answered `429`, which the client waited out.
fn rate_limited(proxy: &Proxy) -> u64 {
    proxy
        .traffic()
        .routes()
        .values()
        .map(|route| route.limited)
        .sum()
}

/// Tells `folder`'s engine that `path` changed, as the native watcher does.
fn heard(folder: &Folder, path: &str) {
    let root = folder.engine.root().to_path_buf();
    folder.engine.inject_watch_event(vec![root.join(path)]);
    folder
        .engine
        .poll_external_changes(Duration::from_millis(1))
        .expect("the watcher reports");
}

/// Takes what the native watcher still reports of the run so far, as the
/// app's watch thread would have.
fn watcher_quiet(folder: &Folder) {
    let mut quiet = 0;
    while quiet < 3 {
        let poll = folder
            .engine
            .poll_watcher(Duration::from_millis(200))
            .expect("the watcher reports");
        quiet = if poll.changed.is_empty() && !poll.rescanned {
            quiet + 1
        } else {
            0
        };
    }
}

/// One device with its folder.
struct Member {
    desktop: Desktop,
    folder: Folder,
    sync: WorkspaceSync,
}

/// A new device of the first's account with a folder of its own.
async fn join<B: Backend>(
    server: &Server<B>,
    first: &mut Desktop,
    sync: &mut WorkspaceSync,
    name: &str,
) -> Member {
    let mut desktop = approve(
        server,
        first,
        std::slice::from_mut(sync.remote_mut().expect("connected")),
    )
    .await;
    let mut folder = Folder::new(name);
    let sync_new = WorkspaceSync::download(
        &mut desktop.service,
        folder.engine.clone(),
        sync.workspace(),
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    Member {
        desktop,
        folder,
        sync: sync_new,
    }
}

/// Starts the server on its own runtime, at the default limits of §26, and
/// drives the run on another.
fn scale<B, D, S, F>(store: &str, start: S)
where
    B: Backend,
    D: Database<B>,
    S: FnOnce(CheckpointInterval, Retention, RateGate) -> F,
    F: Future<Output = Server<B>>,
{
    let sizes = Sizes::from_environment();
    // A small run makes checkpoints due as the full one does at 10,000.
    let interval = CheckpointInterval {
        entries: (sizes.objects() as u64 / 4).clamp(8, 10_000),
        ..CheckpointInterval::default()
    };
    let server_side = server_runtime();
    // Every device here signs in from one address, which no real fleet does,
    // so only that limit stays out of the way.
    let wide = Limit {
        allowance: 1 << 20,
        window: Duration::from_secs(60),
    };
    let limits = RateGate::new(
        RateLimits {
            sign_in: wide,
            ..RateLimits::default()
        },
        None,
    );
    let server = as_server(|| server_side.block_on(start(interval, Retention::default(), limits)));
    client_runtime().block_on(drive::<B, D>(store, sizes, server));
    server_side.shutdown_background();
}

async fn drive<B: Backend, D: Database<B>>(store: &str, sizes: Sizes, server: Server<B>) {
    let seed = Seed::take("NOURA_SCALE_SEED");
    let mut report = Report::new("scale", store, seed.value);
    report.set("notes", sizes.notes);
    report.set("attachments", sizes.attachments);
    let started = Instant::now();

    let proxy = Proxy::start(&server.base).await;
    proxy
        .traffic()
        .quiet
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // The first device restricts two folders, then syncs everything up.
    let mut first = first_desktop(&server, "ana@example.com").await;
    let mut service = first.connect_through(&proxy.base).await;
    let mut a = Folder::new("Field notes");
    let mut sync_a = WorkspaceSync::turn_on(
        &mut service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.set_checkpoint_wait(Duration::ZERO);
    sync_a.round(&mut service, 0).await.expect("a round");
    let mut restricted = BTreeSet::new();
    for name in ["Pay", "Legal"] {
        restricted.insert(
            sync_a
                .restrict_folder(&mut service, name, &[])
                .await
                .expect("restricted"),
        );
    }
    build(&a, sizes, seed.value);
    report.seconds("build_seconds", started);

    let upload = Instant::now();
    settle(&server, &mut sync_a, &mut service, "the upload").await;
    report.seconds("upload_seconds", upload);
    report.set("upload_rate_limited", rate_limited(&proxy));
    assert!(
        sync_a.latest_checkpoint().is_some(),
        "the upload made a checkpoint due: {:?} {:?} {}",
        sync_a.checkpoint_failures(),
        sync_a.status(),
        sync_a.cursor(),
    );
    let expected = digests(a.dir.path());
    assert_eq!(
        expected.keys().filter(|p| p.ends_with(".md")).count(),
        sizes.notes
    );

    // A fresh device joins, bootstrapping from the checkpoint at §26's
    // limits. §28's numbers and ticket 52's 20 minutes describe this step.
    let joining = Instant::now();
    let limited_before = rate_limited(&proxy);
    let mut fresh = join(&server, &mut first, &mut sync_a, "Fresh").await;
    fresh.sync.set_checkpoint_wait(Duration::ZERO);
    fresh.desktop.service = fresh.desktop.connect_through(&proxy.base).await;
    let live_before = proxy
        .traffic()
        .routes()
        .get("GET live")
        .map_or(0, |r| r.requests);
    settle(
        &server,
        &mut fresh.sync,
        &mut fresh.desktop.service,
        "the bootstrap",
    )
    .await;
    report.seconds("join_seconds", joining);
    report.set("join_rate_limited", rate_limited(&proxy) - limited_before);
    let live_pages = proxy
        .traffic()
        .routes()
        .get("GET live")
        .map_or(0, |r| r.requests);
    assert!(
        live_pages > live_before,
        "it bootstrapped from a checkpoint"
    );
    report.set("live_pages", live_pages - live_before);
    report.set("attachment_range_requests", download_ranges(&proxy));

    // Both devices end with identical files and IDs.
    let found = digests(fresh.folder.dir.path());
    let differ = difference(&expected, &found);
    assert!(
        differ.is_empty(),
        "{}: {:?}",
        differ.len(),
        &differ[..differ.len().min(20)]
    );
    for path in expected.keys() {
        if path.ends_with(".noura-domain") {
            continue;
        }
        assert_eq!(
            sync_a.rid_at(path),
            fresh.sync.rid_at(path),
            "the object at {path}"
        );
    }
    let (request, response) = bodies_stayed_small(&proxy);
    report.set("largest_request_bytes", request as u64);
    report.set("largest_response_bytes", response as u64);

    // One edit crosses with no walk and nothing unchanged resent.
    let edit = Instant::now();
    sync_a.follow_changes();
    fresh.sync.follow_changes();
    watcher_quiet(&a);
    watcher_quiet(&fresh.folder);
    settle(&server, &mut sync_a, &mut service, "the first walk").await;
    settle(
        &server,
        &mut fresh.sync,
        &mut fresh.desktop.service,
        "the first walk",
    )
    .await;
    sync_a.set_walk_every(Duration::from_secs(u64::MAX >> 8));
    fresh
        .sync
        .set_walk_every(Duration::from_secs(u64::MAX >> 8));
    let walks = (sync_a.walks(), fresh.sync.walks());
    proxy
        .traffic()
        .quiet
        .store(false, std::sync::atomic::Ordering::SeqCst);
    proxy.forget_recorded();
    let blobs_before = blob_requests(&proxy);
    let mut random = Random::new(seed.value ^ 1);
    let target = note_path(random.below(sizes.notes));
    let edited = note(&mut random);
    let keeps = std::fs::read_to_string(a.path(&target)).expect("the note");
    let keeps = keeps.split("\n\n").next().expect("its frontmatter");
    let edited = format!(
        "{keeps}\n\n{}",
        edited.split_once("\n\n").expect("a body").1
    );
    a.write(&target, edited.as_bytes());
    heard(&a, &target);
    let one_way = Instant::now();
    sync_a
        .round(&mut service, 0)
        .await
        .expect("the edit goes up");
    fresh
        .sync
        .round(&mut fresh.desktop.service, 0)
        .await
        .expect("the edit comes down");
    report.seconds("edit_seconds", one_way);
    assert_eq!(
        std::fs::read_to_string(fresh.folder.path(&target)).expect("the note"),
        edited,
        "the edit crossed"
    );
    assert_eq!((sync_a.walks(), fresh.sync.walks()), walks, "a walk ran");
    let crossed = pushed(&proxy);
    assert_eq!(crossed.operations, 1, "{crossed:?}");
    assert_eq!(blob_requests(&proxy), blobs_before, "no attachment moved");
    report.seconds("edit_total_seconds", edit);

    // Removing a device re-encrypts no object.
    // The other devices only need their cards in the policy: each takes
    // its envelopes in the rotation.
    let approving = Instant::now();
    let doomed = approve(
        &server,
        &mut first,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut others = Vec::new();
    for index in 0..sizes.other_devices {
        others.push(
            approve(
                &server,
                &mut first,
                std::slice::from_mut(sync_a.remote_mut().expect("connected")),
            )
            .await,
        );
        // Answered requests count against a cap until they expire.
        if index % 16 == 15 {
            D::clear_requests(&server).await;
        }
    }
    report.seconds("approvals_seconds", approving);
    settle(&server, &mut sync_a, &mut service, "the approvals").await;
    settle(
        &server,
        &mut fresh.sync,
        &mut fresh.desktop.service,
        "the approvals",
    )
    .await;
    let domains = 1 + restricted.len();
    let devices = 3 + sizes.other_devices;
    report.set("devices", devices);
    proxy.forget_recorded();
    let blobs_before = blob_requests(&proxy);
    let removal = Instant::now();
    service
        .remove_device(
            doomed.id(),
            &[],
            std::slice::from_mut(sync_a.remote_mut().expect("connected")),
        )
        .await
        .expect("removed");
    sync_a.set_rotation_wait(Duration::ZERO);
    for domain in &restricted {
        while sync_a.domain_epoch(domain) < 2 {
            sync_a.round(&mut service, 0).await.expect("a rotation");
        }
    }
    settle(&server, &mut sync_a, &mut service, "the rotations").await;
    report.seconds("removal_seconds", removal);
    let rotation = pushed(&proxy);
    assert_eq!(
        rotation.operations, 0,
        "no object re-encrypted: {rotation:?}"
    );
    assert_eq!(blob_requests(&proxy), blobs_before, "no attachment moved");
    assert!(
        rotation.envelopes <= devices * domains,
        "{devices} devices, {domains} domains: {rotation:?}"
    );
    // One push per domain carries that domain's rotation, and the
    // `account_heads` change goes in a push of its own.
    let rotated: BTreeSet<Id> = rotation.rotations.iter().flatten().copied().collect();
    assert!(
        rotation.rotations.len() == domains
            && rotation.rotations.iter().all(|push| push.len() == 1)
            && rotated.len() == domains,
        "one rotation push per domain: {rotation:?}"
    );
    assert_eq!(rotation.policy, 1, "one account_heads change: {rotation:?}");
    let bytes: u64 = proxy
        .requests()
        .iter()
        .filter(|r| r.path.ends_with("/ops"))
        .map(|r| r.body.len() as u64)
        .sum();
    report.set("removal_bytes", bytes);
    report.set("removal_envelopes", rotation.envelopes as u64);
    report.set("removal_pushes", rotation.pushes as u64);
    bodies_stayed_small(&proxy);

    report.set("database_bytes", D::size(&server).await);
    report.seconds("duration_seconds", started);
    report.finish();
    drop(others);
    drop(seed);
}

#[test]
fn a_large_workspace_syncs_up_and_bootstraps_on_sqlite() {
    scale::<_, Sqlite, _, _>("sqlite", Server::start_at);
}

#[test]
fn a_large_workspace_syncs_up_and_bootstraps_on_postgres() {
    scale::<_, Postgres, _, _>("postgres", Server::start_postgres_at);
}
