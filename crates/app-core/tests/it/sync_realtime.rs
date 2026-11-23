//! The desktop client on the realtime channel at seam 4 (sync-v2.md §23),
//! against the real server on SQLite and on PostgreSQL: records frames
//! apply exactly as pulled pages do, at each cursor position around
//! `previous_seq` and inside the frame, and a gap pulls; a tampering relay
//! that forges or drops frames delays sync and changes nothing the device
//! accepts; policy, key and account chain hints fetch what they name; the
//! app pings, reconnects with backoff, pulls once a minute while the
//! channel is up and long-polls while it is down, and catches up after a
//! dropped connection; and a note that couldn't enter text mode asks
//! again 2 seconds on, with the channel up or down.

use crate::sync_support;
use std::time::Duration;

use app_core::sync::SyncService;
use app_core::sync::realtime::{Backoff, Channel, Live};
use app_core::sync::workspace::{FrameOutcome, WorkspaceSync};
use sync_protocol::codec::{Bytes, Sorted};
use sync_protocol::messages::{Entry, EntryRecord, Frame, PullCursor};
use sync_protocol::records::Signed;
use sync_store::Backend;
use sync_support::{Desktop, Folder, Pair, Server, approve, first_desktop, pair};

/// The next frame within ten seconds.
async fn next(channel: &mut Channel) -> Frame {
    tokio::time::timeout(Duration::from_secs(10), channel.next())
        .await
        .expect("a frame arrives")
        .expect("the channel stays open")
}

/// The next records frame, handing every other frame to `sync` first.
async fn records(
    channel: &mut Channel,
    sync: &mut WorkspaceSync,
    service: &mut SyncService,
) -> Frame {
    loop {
        let frame = next(channel).await;
        if matches!(frame, Frame::Records { .. }) {
            return frame;
        }
        sync.on_frame(service, &frame).await.expect("handled");
    }
}

/// Opens `sync`'s channel and hands it the `restored` frame it opens with.
async fn open(sync: &mut WorkspaceSync, service: &mut SyncService) -> Channel {
    let mut channel = Channel::open(service.client(), sync.workspace())
        .await
        .expect("the channel opens");
    let restored = next(&mut channel).await;
    assert!(matches!(restored, Frame::Restored { .. }), "{restored:?}");
    assert_eq!(
        sync.on_frame(service, &restored).await.unwrap(),
        FrameOutcome::default()
    );
    channel
}

fn entries_of(frame: &Frame) -> Vec<Entry> {
    let Frame::Records { entries, .. } = frame else {
        panic!("a records frame");
    };
    entries.to_vec()
}

/// `frame` with only its first `count` entries.
fn first_entries(frame: &Frame, count: usize) -> Frame {
    let Frame::Records {
        domain,
        previous_seq,
        checkpoint_due,
        entries,
    } = frame
    else {
        panic!("a records frame");
    };
    Frame::Records {
        domain: *domain,
        previous_seq: *previous_seq,
        checkpoint_due: *checkpoint_due,
        entries: Sorted::new(entries.iter().take(count).cloned().collect()).unwrap(),
    }
}

/// Two desktops of one account that hold a synced workspace.
struct Two {
    laptop: Desktop,
    a: Folder,
    sync_a: WorkspaceSync,
    desktop: Desktop,
    b: Folder,
    sync_b: WorkspaceSync,
}

async fn two<B: Backend>(server: &Server<B>) -> Two {
    let mut laptop = first_desktop(server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", b"# Plan\n");
    let mut sync_a = WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    let mut desktop = approve(
        server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut b = Folder::new("Copy");
    let mut sync_b = WorkspaceSync::download(
        &mut desktop.service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    assert_eq!(a.files(), b.files());
    Two {
        laptop,
        a,
        sync_a,
        desktop,
        b,
        sync_b,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_frames_apply_like_pulled_pages() {
    let server = Server::start().await;
    records_frames(&server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_frames_apply_like_pulled_pages_on_postgres() {
    let server = Server::start_postgres().await;
    records_frames(&server).await;
}

async fn records_frames<B: Backend>(server: &Server<B>) {
    let Two {
        mut laptop,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = two(server).await;
    let service = &mut laptop.service;
    let mut channel = open(&mut sync_a, service).await;

    // Two files in one push reach the laptop in one frame.
    b.write("One.md", b"# One\n");
    b.write("Two.md", b"# Two\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    let both = records(&mut channel, &mut sync_a, service).await;
    let entries = entries_of(&both);
    assert_eq!(entries.len(), 2);
    let (first, last) = (entries[0].seq, entries[1].seq);
    let Frame::Records { previous_seq, .. } = &both else {
        unreachable!()
    };
    assert_eq!(*previous_seq, sync_a.cursor(), "the frame follows on");

    // At `previous_seq`: the first entry alone applies, with no pull, and
    // the cursor moves to it once its file is on disk.
    let outcome = sync_a
        .on_frame(service, &first_entries(&both, 1))
        .await
        .unwrap();
    assert_eq!(
        outcome,
        FrameOutcome {
            applied: 1,
            pulled: false
        }
    );
    assert_eq!(sync_a.cursor(), first);
    assert_eq!(a.files().len() + 1, b.files().len());

    // Inside the frame: a pull instead.
    let outcome = sync_a.on_frame(service, &both).await.unwrap();
    assert!(outcome.pulled);
    assert_eq!(sync_a.cursor(), last);
    assert_eq!(a.files(), b.files());

    // A frame the cursor covers changes nothing.
    assert_eq!(
        sync_a.on_frame(service, &both).await.unwrap(),
        FrameOutcome::default()
    );
    assert_eq!(sync_a.cursor(), last);

    // A gap: the later frame arrives first, and the device pulls.
    b.write("Three.md", b"# Three\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    let third = records(&mut channel, &mut sync_a, service).await;
    b.write("Four.md", b"# Four\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    let fourth = records(&mut channel, &mut sync_a, service).await;
    let outcome = sync_a.on_frame(service, &fourth).await.unwrap();
    assert!(outcome.pulled, "a gap pulls");
    assert_eq!(sync_a.cursor(), entries_of(&fourth)[0].seq);
    assert_eq!(a.files(), b.files());
    assert_eq!(
        sync_a.on_frame(service, &third).await.unwrap(),
        FrameOutcome::default()
    );

    // A frame that follows straight on applies with no pull at all.
    b.write("Five.md", b"# Five\n");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    let fifth = records(&mut channel, &mut sync_a, service).await;
    let outcome = sync_a.on_frame(service, &fifth).await.unwrap();
    assert_eq!(
        outcome,
        FrameOutcome {
            applied: 1,
            pulled: false
        }
    );
    assert_eq!(a.files(), b.files());

    // A domain's first entry comes with `previous_seq` 0: a new workspace
    // whose cursor is still 0 applies it.
    let c = Folder::new("Fresh");
    let mut registry = c.registry;
    let mut sync_c = WorkspaceSync::turn_on(service, c.engine.clone(), "Fresh", &mut registry)
        .await
        .expect("sync turns on");
    assert_eq!(sync_c.cursor(), 0);
    let domain = sync_c.remote_mut().unwrap().domain;
    let page = service
        .client()
        .pull(sync_c.workspace(), vec![PullCursor { domain, after: 0 }], 0)
        .await
        .unwrap()
        .pages
        .0
        .remove(0);
    let generation = service.client().restore_generation();
    sync_c
        .on_frame(
            service,
            &Frame::Restored {
                restore_generation: generation,
            },
        )
        .await
        .unwrap();
    let opening = Frame::Records {
        domain,
        previous_seq: 0,
        checkpoint_due: 0,
        entries: Sorted::new(vec![page.entries[0].clone()]).unwrap(),
    };
    assert_eq!(page.entries[0].seq, 1);
    let outcome = sync_c.on_frame(service, &opening).await.unwrap();
    assert!(!outcome.pulled, "{outcome:?}");
    assert_eq!(sync_c.cursor(), 1);
}

/// The laptop of `pair`, which reaches the server through the proxy,
/// opens its channel there.
async fn laptop_channel(pair: &mut Pair<sync_store_sqlite::SqliteBackend>) -> Channel {
    open(&mut pair.sync_a, &mut pair.service).await
}

/// Changes one byte of every operation's ciphertext in a records frame.
fn forge_records(frame: Frame) -> Vec<Frame> {
    let Frame::Records {
        domain,
        previous_seq,
        checkpoint_due,
        entries,
    } = frame
    else {
        return vec![frame];
    };
    let forged = entries
        .iter()
        .map(|entry| {
            let mut entry = entry.clone();
            if let EntryRecord::Operation(op) = &mut entry.record {
                let mut body = op.body.clone();
                let mut bytes = body.ciphertext.0.clone();
                bytes[0] ^= 1;
                body.ciphertext = Bytes(bytes);
                *op = Signed { body, sig: op.sig };
            }
            entry
        })
        .collect();
    vec![Frame::Records {
        domain,
        previous_seq,
        checkpoint_due,
        entries: Sorted::new(forged).unwrap(),
    }]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_that_forges_or_drops_frames_only_delays_sync() {
    let mut pair = pair().await;
    let mut channel = laptop_channel(&mut pair).await;

    // A forged records frame fails the checks: the laptop applies nothing
    // from it, keeps its cursor and pulls the real records.
    pair.proxy.tamper_frames(Box::new(forge_records));
    pair.b.write("Forged.md", b"# The real text\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    let forged = records(&mut channel, &mut pair.sync_a, &mut pair.service).await;
    let outcome = pair
        .sync_a
        .on_frame(&mut pair.service, &forged)
        .await
        .unwrap();
    assert!(outcome.pulled, "a forged frame sends the device to pull");
    assert_eq!(outcome.applied, 1, "the pull brings the real record");
    assert_eq!(pair.a.files(), pair.b.files());

    // Frames the relay makes up from nothing change nothing either.
    let cursor = pair.sync_a.cursor();
    let before = pair.a.files();
    let domain = pair.sync_a.remote_mut().unwrap().domain;
    let mut made_up = entries_of(&forged);
    for (i, entry) in made_up.iter_mut().enumerate() {
        entry.seq = cursor + 1 + i as u64;
    }
    for frame in [
        Frame::Records {
            domain,
            previous_seq: cursor,
            checkpoint_due: 0,
            entries: Sorted::new(made_up).unwrap(),
        },
        Frame::Notify {
            domain,
            head: cursor + 100,
            checkpoint_due: 0,
        },
        Frame::Restored {
            restore_generation: u64::MAX,
        },
    ] {
        let outcome = pair.sync_a.on_frame(&mut pair.service, &frame).await;
        assert!(outcome.is_ok(), "{frame:?}: {outcome:?}");
        assert_eq!(pair.sync_a.cursor(), cursor);
        assert_eq!(pair.a.files(), before);
        assert_eq!(pair.sync_a.stopped(), None);
    }

    // After a forged generation, a real frame isn't applied from the
    // channel: the device pulls it instead.
    pair.proxy.honest();
    pair.b.write("After.md", b"# After\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    let real = records(&mut channel, &mut pair.sync_a, &mut pair.service).await;
    let outcome = pair
        .sync_a
        .on_frame(&mut pair.service, &real)
        .await
        .unwrap();
    assert!(outcome.pulled);
    assert_eq!(pair.a.files(), pair.b.files());
    drop(channel);

    // A relay that drops every frame: the minute's pull still brings the
    // change, here after a second.
    pair.proxy.tamper_frames(Box::new(|_| Vec::new()));
    let mut live = Live::new(Duration::from_secs(1), Duration::from_secs(20));
    let wake = tokio::sync::Notify::new();
    live.step(&mut pair.sync_a, &mut pair.service, &wake)
        .await
        .expect("the first step connects and catches up");
    assert!(live.connected());
    pair.b.write("Dropped.md", b"# Dropped\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    let started = tokio::time::Instant::now();
    while pair.a.files() != pair.b.files() {
        assert!(started.elapsed() < Duration::from_secs(10), "the pull came");
        live.step(&mut pair.sync_a, &mut pair.service, &wake)
            .await
            .expect("a step");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_app_pings_reconnects_and_catches_up() {
    let mut pair = pair().await;
    let wake = tokio::sync::Notify::new();
    let mut live = Live::new(Duration::from_secs(60), Duration::from_millis(200));
    let step = live
        .step(&mut pair.sync_a, &mut pair.service, &wake)
        .await
        .expect("connects");
    assert!(step.connected && step.pulled, "it pulls once it connects");

    // While it waits for frames, the app pings, and the desktop's change
    // arrives in a records frame, with no pull.
    let pings = pair.proxy.pings();
    let following = async {
        loop {
            let step = live
                .step(&mut pair.sync_a, &mut pair.service, &wake)
                .await
                .expect("a step");
            assert!(step.connected && !step.pulled, "{step:?}");
            if step.applied > 0 {
                return step;
            }
        }
    };
    let (step, ()) = tokio::join!(following, async {
        tokio::time::sleep(Duration::from_millis(900)).await;
        pair.b.write("Live.md", b"# Live\n");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
    });
    assert_eq!(step.applied, 1);
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.proxy.pings() >= pings + 3, "pings every 200 ms");

    // The connection drops while the desktop writes; the app reconnects
    // and catches up with no loss.
    pair.proxy.close_channels();
    pair.b.write("While down.md", b"# While down\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    let started = tokio::time::Instant::now();
    let mut was_down = false;
    while pair.a.files() != pair.b.files() || !live.connected() {
        assert!(started.elapsed() < Duration::from_secs(20), "caught up");
        let step = live
            .step(&mut pair.sync_a, &mut pair.service, &wake)
            .await
            .expect("a step");
        was_down |= !step.connected;
    }
    assert!(was_down, "the channel went down");

    // While the channel can't open, the pull long-polls: it returns as
    // soon as the desktop's change commits.
    pair.proxy.forge(Box::new(|_, path| {
        path.ends_with("/realtime").then(|| (503, Vec::new()))
    }));
    live.disconnect();
    let (step, ()) = tokio::join!(
        live.step(&mut pair.sync_a, &mut pair.service, &wake),
        async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            pair.b.write("Polled.md", b"# Polled\n");
            pair.sync_b
                .round(&mut pair.desktop.service, 0)
                .await
                .expect("a round");
        }
    );
    let step = step.expect("a step");
    assert!(!step.connected && step.pulled);
    assert_eq!(step.applied, 1, "the long poll brought the change");
    assert_eq!(pair.a.files(), pair.b.files());
}

#[test]
fn the_backoff_stays_between_one_and_thirty_seconds_and_grows() {
    for _ in 0..50 {
        let mut backoff = Backoff::default();
        let waits: Vec<Duration> = (0..10).map(|_| backoff.next_wait()).collect();
        for wait in &waits {
            assert!(*wait >= Duration::from_secs(1) && *wait <= Duration::from_secs(30));
        }
        assert!(waits[0] <= Duration::from_secs(2));
        assert!(waits[9] >= Duration::from_secs(15), "{waits:?}");
        backoff.reset();
        assert!(backoff.next_wait() <= Duration::from_secs(2));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hints_fetch_the_policy_keys_and_account_chain() {
    let server = Server::start().await;
    let Two {
        mut laptop,
        a: _a,
        mut sync_a,
        mut desktop,
        b: _b,
        mut sync_b,
    } = two(&server).await;
    let mut channel = open(&mut sync_b, &mut desktop.service).await;
    let account = laptop.credentials.account;
    let genesis = laptop.credentials.reference_head.genesis;
    let seqno =
        |service: &SyncService| service.kept_heads().kept(&account, &genesis).unwrap().seqno;
    let kept = seqno(&desktop.service);
    let revision = sync_b.policy_revision();

    // A third device: its link moves the account chain, and recording it
    // changes the policy and adds an envelope to the key stream.
    let _phone = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut seen = Vec::new();
    while !(seen.contains(&4) && seen.contains(&5) && seen.contains(&10)) {
        let frame = next(&mut channel).await;
        seen.push(frame.code());
        if let Frame::AccountChanged {
            account: changed,
            genesis: of,
            seqno,
        } = &frame
        {
            assert_eq!((*changed, *of), (account, genesis));
            assert!(*seqno > kept);
        }
        sync_b
            .on_frame(&mut desktop.service, &frame)
            .await
            .expect("handled");
    }
    assert!(
        seqno(&desktop.service) > kept,
        "the chain's later head is kept"
    );
    assert!(
        sync_b.policy_revision() > revision,
        "the policy arrived before any push"
    );
    assert_eq!(
        sync_b.policy_revision(),
        sync_a.remote_mut().unwrap().policy_revision()
    );
}

/// How many requests `proxy` took so far whose path ends with `suffix`.
fn requests(proxy: &sync_support::Proxy, suffix: &str) -> usize {
    proxy
        .requests()
        .iter()
        .filter(|r| {
            r.path
                .split('?')
                .next()
                .unwrap_or_default()
                .ends_with(suffix)
        })
        .count()
}

/// A step's wait ends by the moment it's given, and with the channel up
/// that costs no request (§22, §23). With the channel down, the pull waits
/// no longer than that, even while the reconnect backoff is long.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_ends_by_the_moment_it_is_given() {
    use tokio::time::Instant;

    let mut pair = pair().await;
    let wake = tokio::sync::Notify::new();
    let mut live = Live::new(Duration::from_secs(60), Duration::from_secs(20));
    let step = live
        .step(&mut pair.sync_a, &mut pair.service, &wake)
        .await
        .expect("connects");
    assert!(step.connected && step.pulled, "it pulls once it connects");

    // Up: the step stops waiting and sends nothing. The channel's first
    // frames may end a step before then.
    let before = requests(&pair.proxy, "/pull");
    let started = Instant::now();
    let until = started + Duration::from_secs(1);
    while Instant::now() < until {
        let step = live
            .step_until(&mut pair.sync_a, &mut pair.service, &wake, Some(until))
            .await
            .expect("a step");
        assert!(step.connected && !step.pulled, "{step:?}");
    }
    let took = started.elapsed();
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(requests(&pair.proxy, "/pull"), before, "no pull");

    // Down, with a backoff of 15 to 30 seconds: the long poll ends by the
    // moment given instead.
    pair.proxy.forge(Box::new(|_, path| {
        path.ends_with("/realtime").then(|| (503, Vec::new()))
    }));
    for _ in 0..6 {
        live.disconnect();
    }
    let before = requests(&pair.proxy, "/pull");
    let started = Instant::now();
    let until = started + Duration::from_secs(2);
    let step = live
        .step_until(&mut pair.sync_a, &mut pair.service, &wake, Some(until))
        .await
        .expect("a step");
    let took = started.elapsed();
    assert!(!step.connected && step.pulled, "{step:?}");
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(8),
        "{took:?}"
    );
    assert_eq!(
        requests(&pair.proxy, "/pull"),
        before + 1,
        "one pull, which waited"
    );
}

/// Makes a file unreadable until the guard goes, as a file another program
/// holds for a moment can be.
#[cfg(unix)]
struct Unreadable(std::path::PathBuf);

#[cfg(unix)]
impl Unreadable {
    fn new(path: std::path::PathBuf) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(
            std::fs::read(&path).is_err(),
            "the file stays readable: the test can't run as root"
        );
        Self(path)
    }
}

#[cfg(unix)]
impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o644));
    }
}

/// The app, as the desktop runs it with the channel up: a note whose
/// first request to enter text mode gets "not yet" asks again 2 seconds
/// on, and enters as soon as it can, rather than at the minute's pull
/// (§22, §23). Here the note can't be read for a moment, which no file
/// event or frame follows, so only the retry's timer ends the wait.
#[cfg(unix)]
#[test]
fn a_note_refused_while_the_channel_is_up_asks_again_within_seconds() {
    use app_core::account::live::{ACCOUNT_FILE, LiveAccount};
    use app_core::account::{AccountBackend, AccountPhase};
    use app_core::folder_sync::{FolderSyncBackend, FolderSyncPhase};
    use app_core::sync::account_heads::KEPT_HEADS_FILE;
    use app_core::sync::device::StoredDevice;
    use std::sync::Arc;
    use std::time::Instant;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let Pair {
        server,
        proxy,
        laptop,
        service,
        a,
        sync_a,
        ..
    } = runtime.block_on(pair());
    drop((service, sync_a));
    let state = a.app_data.path().join("sync").join(laptop.id().to_hex());
    std::fs::create_dir_all(&state).unwrap();
    std::fs::copy(
        laptop.state.path().join(KEPT_HEADS_FILE),
        state.join(KEPT_HEADS_FILE),
    )
    .unwrap();
    std::fs::write(
        a.app_data.path().join(ACCOUNT_FILE),
        format!(r#"{{"server":"{}"}}"#, proxy.base),
    )
    .unwrap();
    let mut stored = StoredDevice::load(&*laptop.secrets, &server.base)
        .unwrap()
        .expect("the laptop's device");
    stored.server = proxy.base.clone();
    stored.save(&*laptop.secrets).unwrap();
    let account = LiveAccount::start(
        a.app_data.path().to_owned(),
        laptop.secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    FolderSyncBackend::attach(&account, Some(a.engine.clone()));
    let bridge = FolderSyncBackend::text_bridge(&account).expect("the bridge");

    let until = |what: &str, limit: Duration, done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + limit;
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let channels = || requests(&proxy, "/realtime");
    until(
        "the folder syncs on the channel",
        Duration::from_secs(30),
        &|| {
            matches!(
                AccountBackend::screen(&account).phase,
                AccountPhase::SignedIn(_)
            ) && FolderSyncBackend::screen(&account).phase == FolderSyncPhase::On
                && channels() > 0
        },
    );

    // Startup's last rounds come a couple of seconds after the channel
    // opens. Once nothing goes to the server for 3 seconds, which is
    // longer than the thread rests between rounds, it waits on the
    // channel.
    let seen = std::cell::Cell::new((proxy.requests().len(), Instant::now()));
    until("the sync thread rests", Duration::from_secs(30), &|| {
        let count = proxy.requests().len();
        let (before, since) = seen.get();
        if count != before {
            seen.set((count, Instant::now()));
            return false;
        }
        since.elapsed() >= Duration::from_secs(3)
    });
    // The note can't be read when you first type in it, so it stays out
    // of text mode for now. It stays unreadable through the next try, so
    // that try comes from the sync thread's wait on the channel either way.
    let text = "# Plan\n";
    let unreadable = Unreadable::new(a.path("Plan.md"));
    bridge.open("Plan.md");
    until("the request is answered", Duration::from_secs(10), &|| {
        !bridge.answering("Plan.md")
    });
    assert!(bridge.take_document("Plan.md", text).is_none(), "refused");
    std::thread::sleep(Duration::from_secs(3));
    assert!(bridge.take_document("Plan.md", text).is_none(), "refused");

    // Readable again: the next try enters text mode within seconds.
    drop(unreadable);
    until(
        "the note entered text mode within 10 seconds",
        Duration::from_secs(10),
        &|| bridge.take_document("Plan.md", text).is_some(),
    );
    assert_eq!(channels(), 1, "the channel stayed up");
    drop(account);
    drop(runtime);
}
