//! The realtime channel at seam 4 (sync-v2.md §23), on SQLite and on
//! PostgreSQL: `GET /v2/workspaces/{w}/realtime` serves only devices active
//! in the workspace, tells every connected reader about a committed push in
//! a records frame or a notify, and never about one that rolled back, and
//! carries the restore generation, key stream, account chain and error
//! frames. Two PostgreSQL instances deliver the same frames, with the
//! notification sent after commit.

mod support;

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use support::{Account, Created, push_body, seed_store};
use sync_key_envelope::v2::{DeviceKeys, next_link};
use sync_protocol::codec::{Bytes, Decode, Encode, List, Nested};
use sync_protocol::messages::{
    EntryRecord, Frame, FrameAction, PullCursor, PullRequest, PullResponse, PushResponse,
    RewrapBody, records_frame_action,
};
use sync_protocol::records::{LinkKind, Operation, Presence, Signed};
use sync_protocol::types::{Digest, Id, Nonce};
use sync_server::{ChannelLimits, RateGate};
use sync_store::conformance::head_of;
use sync_store::{Backend, Store};
use sync_store_postgres::PgBackend;
use sync_store_postgres::testing::{Cluster, TestDatabase};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// An open realtime channel.
struct Channel(Socket);

impl Channel {
    /// The next frame within `wait`, or `None`.
    async fn next(&mut self, wait: Duration) -> Option<Frame> {
        loop {
            let message = tokio::time::timeout(wait, self.0.next()).await.ok()??;
            match message.expect("the channel stays open") {
                Message::Binary(bytes) => {
                    assert!(bytes.len() <= 65_536 + 5, "a frame within 64 KiB");
                    return Some(Frame::from_bytes(&bytes).expect("a frame"));
                }
                Message::Text(_) => panic!("frames are binary"),
                _ => {}
            }
        }
    }

    async fn expect(&mut self) -> Frame {
        self.next(Duration::from_secs(10))
            .await
            .expect("a frame arrives")
    }

    async fn send(&mut self, frame: &Frame) {
        self.0
            .send(Message::Binary(frame.to_bytes().into()))
            .await
            .expect("the channel takes the frame");
    }

    /// Asserts that no frame arrives for a while.
    async fn quiet(&mut self) {
        let frame = self.next(Duration::from_millis(600)).await;
        assert!(frame.is_none(), "no frame, got {frame:?}");
    }
}

/// Opens the channel of `workspace` at `base` with `token`, or returns the
/// HTTP status that refused it.
async fn open(base: &str, workspace: Id, token: Option<&str>) -> Result<Channel, u16> {
    let url = format!(
        "{}/v2/workspaces/{}/realtime",
        base.replace("http://", "ws://"),
        workspace.to_base64url()
    );
    let mut request = url.into_client_request().expect("a request");
    if let Some(token) = token {
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("a header"),
        );
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => Ok(Channel(socket)),
        Err(tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
        Err(error) => panic!("the upgrade failed: {error}"),
    }
}

/// A server on a store, its address, and an account of two devices that
/// created a workspace.
struct World<B: Backend> {
    store: Arc<Store<B>>,
    base: String,
    account: Account,
    created: Created,
}

impl<B: Backend> World<B> {
    async fn new(store: Store<B>) -> Self {
        Self::with_gate(store, RateGate::unlimited()).await
    }

    async fn with_gate(store: Store<B>, gate: RateGate) -> Self {
        let store = Arc::new(store);
        let account = seed_store(&store, 2).await;
        let address = sync_server::spawn_limited(
            store.clone(),
            "127.0.0.1:0",
            sync_server::Accounts::local,
            Arc::new(sync_server::blobs::Objects::memory()),
            gate,
        )
        .await
        .expect("serving");
        let base = format!("http://localhost:{}", address.port());
        let created = account.creation();
        let world = Self {
            store,
            base,
            account,
            created,
        };
        let _: PushResponse = world
            .client(0)
            .post("/v2/workspaces", world.created.body.clone())
            .await
            .decode();
        world
    }

    fn client(&self, device: usize) -> support::Client {
        support::Client::new(self.base.clone(), Some(self.account.token(device)))
    }

    async fn channel(&self, device: usize) -> Channel {
        let mut channel = open(
            &self.base,
            self.created.workspace,
            Some(&self.account.token(device)),
        )
        .await
        .expect("the channel opens");
        assert_eq!(
            channel.expect().await,
            Frame::Restored {
                restore_generation: self.store.restore_generation()
            },
            "the channel opens with the restore generation"
        );
        channel
    }

    async fn push(&self, device: usize, ops: &[Signed<Operation>]) -> support::Reply {
        self.client(device)
            .post(
                &format!(
                    "/v2/workspaces/{}/ops",
                    self.created.workspace.to_base64url()
                ),
                push_body(ops),
            )
            .await
    }
}

async fn sqlite() -> (Store<sync_store_sqlite::SqliteBackend>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = sync_server::open_store(&sync_server::Config::new(
        data,
        "127.0.0.1:0".parse().expect("an address"),
    ))
    .await
    .expect("the store opens");
    (store, dir)
}

async fn postgres_database() -> (TestDatabase, sync_server::Postgres) {
    let database = Cluster::shared(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")))
        .database()
        .await;
    let postgres = sync_server::Postgres {
        url: database.url.clone(),
        secret: Some(sync_protocol::types::SecretKey::random().expect("random")),
    };
    sync_server::postgres::migrate(&postgres)
        .await
        .expect("migrated");
    (database, postgres)
}

async fn postgres() -> (Store<PgBackend>, TestDatabase) {
    let (database, postgres) = postgres_database().await;
    let store = sync_server::postgres::open_store(&postgres)
        .await
        .expect("the store opens");
    (store, database)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_channel_serves_only_devices_active_in_the_workspace() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let workspace = world.created.workspace;
    assert_eq!(open(&world.base, workspace, None).await.err(), Some(401));
    // An account that shares no workspace with this one.
    let stranger = seed_store(&world.store, 1).await;
    assert_eq!(
        open(&world.base, workspace, Some(&stranger.token(0)))
            .await
            .err(),
        Some(403)
    );
    assert_eq!(
        open(
            &world.base,
            Id::random().unwrap(),
            Some(&world.account.token(0))
        )
        .await
        .err(),
        Some(404)
    );
    // A plain GET without the upgrade gets the error body of §26.
    let plain = world
        .client(0)
        .get(&format!(
            "/v2/workspaces/{}/realtime",
            workspace.to_base64url()
        ))
        .await;
    assert_eq!(plain.error().code.as_str(), "bad_request");
    world.channel(0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_push_reaches_every_reader_in_a_records_frame() {
    let (store, _dir) = sqlite().await;
    records_after_commit(World::new(store).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_push_reaches_every_reader_in_a_records_frame_on_postgres() {
    let (store, _database) = postgres().await;
    records_after_commit(World::new(store).await).await;
}

async fn records_after_commit<B: Backend>(world: World<B>) {
    let mut pusher = world.channel(0).await;
    let mut reader = world.channel(1).await;
    let keys = &world.account.keys[0];
    let first = world.created.op(keys, 1, Digest::ZERO, |_| {});

    // A push that rolls back sends nothing.
    let bad = world.created.op(keys, 2, Digest([7; 32]), |_| {});
    let refused = world.push(0, &[first.clone(), bad]).await;
    assert_eq!(refused.error().code.as_str(), "chain_conflict");
    pusher.quiet().await;
    reader.quiet().await;

    // The creation wrote entries 1 and 2; the push commits entry 3, which
    // every reader gets, the pushing device included.
    let _: PushResponse = world.push(0, std::slice::from_ref(&first)).await.decode();
    for channel in [&mut pusher, &mut reader] {
        let Frame::Records {
            domain,
            previous_seq,
            checkpoint_due,
            entries,
        } = channel.expect().await
        else {
            panic!("a records frame");
        };
        assert_eq!(domain, world.created.domain);
        assert_eq!(previous_seq, 2);
        assert_eq!(checkpoint_due, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, 3);
        assert!(matches!(&entries[0].record, EntryRecord::Operation(op) if *op == first));
    }

    // Two operations in one push arrive together, after entry 3.
    let second = world.created.op(keys, 2, first.digest(), |_| {});
    let third = world.created.op(keys, 3, second.digest(), |_| {});
    let _: PushResponse = world.push(0, &[second, third]).await.decode();
    let Frame::Records {
        previous_seq,
        entries,
        ..
    } = reader.expect().await
    else {
        panic!("a records frame");
    };
    assert_eq!(previous_seq, 3);
    let seqs: Vec<u64> = entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![4, 5]);
    pusher.expect().await;
    reader.quiet().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_beyond_one_frame_send_a_notify_at_most_every_250_ms() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let mut reader = world.channel(1).await;
    let keys = &world.account.keys[0];
    let mut previous = Digest::ZERO;
    let mut counter = 0;
    let mut batch = |size: usize| {
        (0..size)
            .map(|_| {
                counter += 1;
                let op = world.created.op(keys, counter, previous, |_| {});
                previous = op.digest();
                op
            })
            .collect::<Vec<_>>()
    };
    // 150 operations of about 650 bytes each pass 64 KiB.
    let batches: Vec<Vec<Signed<Operation>>> = (0..4).map(|_| batch(150)).collect();
    let started = tokio::time::Instant::now();
    // The reader takes frames as they come, while the pushes commit.
    let listening = tokio::spawn(async move {
        let mut frames = Vec::new();
        while let Some(frame) = reader.next(Duration::from_secs(2)).await {
            frames.push((tokio::time::Instant::now(), frame));
        }
        frames
    });
    let mut head = 0;
    for ops in &batches {
        let pushed: PushResponse = world.push(0, ops).await.decode();
        head = pushed.receipts.iter().map(|r| r.seq).max().unwrap();
    }
    let mut notifies = Vec::new();
    for (at, frame) in listening.await.unwrap() {
        let Frame::Notify {
            domain,
            head: told,
            checkpoint_due,
        } = frame
        else {
            panic!("a notify, got {frame:?}");
        };
        assert_eq!(domain, world.created.domain);
        assert_eq!(checkpoint_due, 0);
        notifies.push((at, told));
    }
    assert!(notifies.len() >= 2, "a notify for each wave: {notifies:?}");
    assert_eq!(
        notifies.last().unwrap().1,
        head,
        "the last notify names the head"
    );
    for pair in notifies.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= Duration::from_millis(240),
            "notifies 250 ms apart: {:?}",
            notifies
                .iter()
                .map(|(at, h)| (*at - started, *h))
                .collect::<Vec<_>>()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_channel_tells_of_key_and_account_chain_changes() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let mut reader = world.channel(0).await;

    // A re-wrap request joins the key stream.
    let path = format!(
        "/v2/workspaces/{}/domains/{}/rewrap",
        world.created.workspace.to_base64url(),
        world.created.domain.to_base64url()
    );
    let reply = world
        .client(1)
        .post(&path, RewrapBody { epoch: 1 }.to_bytes())
        .await;
    assert!(reply.status.is_success(), "{:?}", reply.error());
    assert_eq!(reader.expect().await, Frame::KeysChanged);

    // A member account's chain gets a link.
    let account = &world.account;
    let added = DeviceKeys::generate().unwrap();
    let link = next_link(
        &account.keys[0],
        &head_of(&account.links),
        LinkKind::AddDevice {
            card: Nested(added.card(account.account)),
            offer: Digest::ZERO,
        },
    );
    let mut links = account.links.clone();
    links.push(link.clone());
    world
        .store
        .append_account_links(vec![link])
        .await
        .expect("the link is stored");
    assert_eq!(
        reader.expect().await,
        Frame::AccountChanged {
            account: account.account,
            genesis: account.links[0].digest(),
            seqno: 3,
        }
    );
    reader.quiet().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_frame_gets_an_error_frame() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let mut channel = world.channel(0).await;
    // A subscribe within the limit takes no answer; one past it is refused.
    channel.send(&subscribe(&[Id::random().unwrap()])).await;
    channel.quiet().await;
    let mut past_the_limit = vec![7];
    past_the_limit.extend_from_slice(&257u32.to_le_bytes());
    past_the_limit.extend_from_slice(&[1; 257 * 16]);
    channel
        .0
        .send(Message::Binary(past_the_limit.into()))
        .await
        .unwrap();
    let Frame::Error {
        rejected_type,
        domain,
        session,
        sequence,
        code,
        ..
    } = channel.expect().await
    else {
        panic!("an error frame");
    };
    assert_eq!(rejected_type, 7);
    assert_eq!((domain.0, session.0, sequence), ([0; 16], [0; 16], 0));
    assert_eq!(code.as_str(), "bad_request");
    // Bytes that aren't a frame, and a frame only the server sends.
    for (bytes, rejected) in [
        (vec![0x0b, 0, 0, 0, 0], 0x0b),
        (Frame::KeysChanged.to_bytes(), 5),
    ] {
        channel.0.send(Message::Binary(bytes.into())).await.unwrap();
        let Frame::Error { rejected_type, .. } = channel.expect().await else {
            panic!("an error frame");
        };
        assert_eq!(rejected_type, rejected);
    }
}

fn subscribe(channels: &[Id]) -> Frame {
    Frame::Subscribe {
        channels: List(channels.to_vec()),
    }
}

/// A presence record of `device` in the world's workspace, with a padded
/// ciphertext the server can't read.
fn presence<B: Backend>(
    world: &World<B>,
    device: usize,
    session: Id,
    sequence: u64,
    channel: Id,
    edit: impl FnOnce(&mut Presence),
) -> Frame {
    let keys = &world.account.keys[device];
    let mut body = Presence {
        workspace: world.created.workspace,
        domain: world.created.domain,
        epoch: 1,
        device: keys.device(),
        session,
        sequence,
        minute: 29_000_000,
        channel,
        nonce: Nonce::ZERO,
        ciphertext: Bytes(vec![0x5a; 272]),
    };
    edit(&mut body);
    Frame::Presence {
        record: Signed::sign(body, keys.signing()),
    }
}

/// The server's error code for a presence record, read from `channel`.
async fn refused_with(channel: &mut Channel, frame: &Frame) -> String {
    channel.send(frame).await;
    let Frame::Error {
        rejected_type,
        code,
        ..
    } = channel.expect().await
    else {
        panic!("an error frame");
    };
    assert_eq!(rejected_type, 2);
    code.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_reaches_subscribers_and_goes_when_the_connection_ends() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    presence_flow(world, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_crosses_postgres_instances_with_notify() {
    let (_database, config) = postgres_database().await;
    let first = sync_server::postgres::open_store(&config).await.unwrap();
    let second = Arc::new(sync_server::postgres::open_store(&config).await.unwrap());
    let world = World::new(first).await;
    let second_base = format!(
        "http://localhost:{}",
        sync_server::spawn(second.clone(), "127.0.0.1:0")
            .await
            .unwrap()
            .port()
    );
    presence_flow(world, Some(second_base)).await;
}

/// Device 0 sends presence on `world`'s server, and device 1 listens on
/// `other`, or on the same server.
async fn presence_flow<B: Backend>(world: World<B>, other: Option<String>) {
    let note = Id::random().unwrap();
    let mut sender = world.channel(0).await;
    let base = other.as_ref().unwrap_or(&world.base);
    let mut listener = open(base, world.created.workspace, Some(&world.account.token(1)))
        .await
        .unwrap();
    assert!(matches!(listener.expect().await, Frame::Restored { .. }));
    listener.send(&subscribe(&[note])).await;
    // The subscribe reaches the other instance's connection first.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let session = Id::random().unwrap();
    let first = presence(&world, 0, session, 1, note, |_| {});
    sender.send(&first).await;
    assert_eq!(listener.expect().await, first, "the record arrives whole");
    // The sender doesn't hear itself, and another channel stays silent.
    sender.quiet().await;
    let other_note = presence(&world, 0, session, 2, Id::random().unwrap(), |_| {});
    sender.send(&other_note).await;
    listener.quiet().await;

    // The largest record fits one notification.
    let largest = presence(&world, 0, session, 3, note, |body| {
        body.ciphertext = Bytes(vec![0x5a; 4_096 + 16]);
    });
    sender.send(&largest).await;
    assert_eq!(listener.expect().await, largest);

    drop(sender);
    assert_eq!(
        listener.expect().await,
        Frame::PresenceGone {
            device: world.account.keys[0].device(),
            session,
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_refuses_presence_it_may_not_carry() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let note = Id::random().unwrap();
    let session = Id::random().unwrap();
    let mut channel = world.channel(0).await;
    let stale = presence(&world, 0, session, 1, note, |body| body.epoch = 2);
    assert_eq!(refused_with(&mut channel, &stale).await, "epoch_stale");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let elsewhere = presence(&world, 0, session, 2, note, |body| {
        body.domain = Id([9; 16]);
    });
    assert_eq!(refused_with(&mut channel, &elsewhere).await, "forbidden");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let someone_else = presence(&world, 1, session, 3, note, |_| {});
    assert_eq!(refused_with(&mut channel, &someone_else).await, "forbidden");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let unpadded = presence(&world, 0, session, 4, note, |body| {
        body.ciphertext = Bytes(vec![0x5a; 273]);
    });
    assert_eq!(
        refused_with(&mut channel, &unpadded).await,
        "padding_invalid"
    );
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    // Two records a second pass, and the third doesn't.
    channel
        .send(&presence(&world, 0, session, 5, note, |_| {}))
        .await;
    channel
        .send(&presence(&world, 0, session, 6, note, |_| {}))
        .await;
    let third = presence(&world, 0, session, 7, note, |_| {});
    assert_eq!(refused_with(&mut channel, &third).await, "rate_limited");
}

/// A request naming a workspace that nobody can see leaves nothing in the
/// event hub, and an open channel leaves nothing once it closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_event_hub_holds_only_workspaces_someone_waits_on() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let hub = world.store.hub().clone();
    for _ in 0..16 {
        let unknown = Id::random().expect("random");
        assert_eq!(
            open(&world.base, unknown, Some(&world.account.token(0)))
                .await
                .err(),
            Some(404)
        );
        let pull = PullRequest {
            cursors: List(Vec::new()),
            wait_seconds: 0,
        };
        let reply = world
            .client(0)
            .post(
                &format!("/v2/workspaces/{}/pull", unknown.to_base64url()),
                pull.to_bytes(),
            )
            .await;
        assert_eq!(reply.status.as_u16(), 404);
    }
    assert_eq!(hub.entries(), (0, 0));

    let channel = world.channel(0).await;
    assert_eq!(hub.entries(), (1, 1));
    drop(channel);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hub.entries() != (0, 0) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(hub.entries(), (0, 0));
}

/// A server on a store with `channels` as its channel limits.
async fn with_channels<B: Backend>(store: Store<B>, channels: ChannelLimits) -> World<B> {
    World::with_gate(store, RateGate::unlimited().with_channels(channels)).await
}

/// The presence limit counts the device, however many channels it opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_presence_limit_counts_a_device_across_its_channels() {
    let (store, _dir) = sqlite().await;
    let world = World::new(store).await;
    let mut one = world.channel(0).await;
    let mut two = world.channel(0).await;
    let (session, note) = (Id([0x61; 16]), Id([0x62; 16]));
    let frame = |sequence| presence(&world, 0, session, sequence, note, |_| {});
    let mut sequence = 0;
    for _ in 0..5 {
        // Two records each, in the first half of a second, so the four
        // share one of the limiter's windows.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a time");
        tokio::time::sleep(Duration::from_millis(u64::from(
            1_050 - now.subsec_millis(),
        )))
        .await;
        for channel in [&mut one, &mut two] {
            for _ in 0..2 {
                sequence += 1;
                channel.send(&frame(sequence)).await;
            }
        }
        let refused = two.next(Duration::from_millis(300)).await;
        if let Some(Frame::Error { code, .. }) = refused {
            assert_eq!(code.as_str(), "rate_limited");
            return;
        }
    }
    panic!("a device got twice its presence limit");
}

/// A device holds a bounded number of channels to one workspace, and a
/// closed channel gives its place back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_holds_a_bounded_number_of_channels() {
    let (store, _dir) = sqlite().await;
    let channels = ChannelLimits {
        per_device: 2,
        ..ChannelLimits::default()
    };
    let world = with_channels(store, channels).await;
    let first = world.channel(0).await;
    let _second = world.channel(0).await;
    let token = world.account.token(0);
    let refused = open(&world.base, world.created.workspace, Some(&token)).await;
    assert_eq!(refused.err(), Some(429));
    let _other_device = world.channel(1).await;
    drop(first);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match open(&world.base, world.created.workspace, Some(&token)).await {
            Ok(_) => break,
            Err(429) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(status) => panic!("the place stays taken: {status}"),
        }
    }
}

/// A channel whose client stays silent closes, and a ping keeps one open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_channel_closes_and_a_ping_keeps_one_open() {
    let (store, _dir) = sqlite().await;
    let channels = ChannelLimits {
        idle: Duration::from_millis(400),
        ..ChannelLimits::default()
    };
    let world = with_channels(store, channels).await;
    let mut silent = world.channel(0).await;
    let mut pinging = world.channel(1).await;
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        pinging
            .0
            .send(Message::Ping(Vec::new().into()))
            .await
            .expect("the channel stays open");
        // Reading lets the client answer and notice a close.
        let _ = tokio::time::timeout(Duration::from_millis(10), pinging.0.next()).await;
    }
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match silent.0.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the silent channel closed");
    let frame = pinging.next(Duration::from_millis(50)).await;
    assert_eq!(frame, None, "the pinging channel stays quiet and open");
    pinging
        .0
        .send(Message::Ping(Vec::new().into()))
        .await
        .expect("still open");
}

/// What a client that follows the channel holds of a domain's log: it
/// applies a records frame that follows straight on from its cursor and
/// pulls on a gap, a frame inside its cursor, or a notify.
struct Follower {
    client: support::Client,
    channel: Channel,
    cursor: u64,
    seen: Vec<u64>,
}

impl Follower {
    async fn pull(&mut self, workspace: Id, domain: Id) {
        loop {
            let response: PullResponse = self
                .client
                .post(
                    &format!("/v2/workspaces/{}/pull", workspace.to_base64url()),
                    PullRequest {
                        cursors: List(vec![PullCursor {
                            domain,
                            after: self.cursor,
                        }]),
                        wait_seconds: 0,
                    }
                    .to_bytes(),
                )
                .await
                .decode();
            let page = &response.pages[0];
            self.seen.extend(page.entries.iter().map(|e| e.seq));
            self.cursor = page.complete_through;
            if self.cursor >= page.head {
                return;
            }
        }
    }

    /// Follows frames until its cursor reaches `head`.
    async fn follow(&mut self, workspace: Id, domain: Id, head: u64) {
        while self.cursor < head {
            match self.channel.expect().await {
                Frame::Records {
                    previous_seq,
                    entries,
                    ..
                } => {
                    let first = entries.first().unwrap().seq;
                    let last = entries.last().unwrap().seq;
                    match records_frame_action(self.cursor, previous_seq, first, last) {
                        FrameAction::Apply => {
                            self.seen.extend(entries.iter().map(|e| e.seq));
                            self.cursor = last;
                        }
                        FrameAction::Ignore => {}
                        FrameAction::Pull => self.pull(workspace, domain).await,
                    }
                }
                Frame::Notify { head, .. } if head > self.cursor => {
                    self.pull(workspace, domain).await
                }
                _ => {}
            }
        }
    }
}

/// Two instances on one PostgreSQL database race pushes into one domain,
/// one of them with a delay inside each push's transaction: every client,
/// on either instance, receives every record, and a push waiting in its
/// transaction holds back no other commit's notification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_postgres_instances_deliver_every_record_to_every_client() {
    let (_database, config) = postgres_database().await;
    let first = sync_server::postgres::open_store(&config).await.unwrap();
    let second = Arc::new(sync_server::postgres::open_store(&config).await.unwrap());
    let world = World::new(first).await;
    let second_base = format!(
        "http://localhost:{}",
        sync_server::spawn(second.clone(), "127.0.0.1:0")
            .await
            .unwrap()
            .port()
    );
    let workspace = world.created.workspace;
    let domain = world.created.domain;
    let follower = |base: String, device: usize| {
        let token = world.account.token(device);
        async move {
            let mut channel = open(&base, workspace, Some(&token)).await.unwrap();
            assert!(matches!(channel.expect().await, Frame::Restored { .. }));
            Follower {
                client: support::Client::new(base, Some(token)),
                channel,
                cursor: 2,
                seen: Vec::new(),
            }
        }
    };
    let mut followers = vec![
        follower(world.base.clone(), 0).await,
        follower(world.base.clone(), 1).await,
        follower(second_base.clone(), 0).await,
        follower(second_base.clone(), 1).await,
    ];
    world
        .store
        .backend()
        .inject_policy_read_delay(Some(Duration::from_millis(20)));
    let push_chain = |base: String, device: usize| {
        let client = support::Client::new(base, Some(world.account.token(device)));
        let keys = world.account.keys[device].clone();
        let created = &world.created;
        async move {
            let path = format!("/v2/workspaces/{}/ops", workspace.to_base64url());
            let mut previous = Digest::ZERO;
            for counter in 1..=20 {
                let op = created.op(&keys, counter, previous, |_| {});
                previous = op.digest();
                let _: PushResponse = client
                    .post(&path, push_body(std::slice::from_ref(&op)))
                    .await
                    .decode();
            }
        }
    };
    tokio::join!(
        push_chain(world.base.clone(), 0),
        push_chain(second_base.clone(), 1)
    );
    for follower in &mut followers {
        follower.follow(workspace, domain, 42).await;
        let mut seen = follower.seen.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, (3..=42).collect::<Vec<u64>>(), "every record arrives");
    }

    // A push held in its transaction on the first instance holds back no
    // other commit's notification: a push into another workspace through
    // the second instance reaches the first instance's clients meanwhile.
    let other = world.account.creation();
    let _: PushResponse = world
        .client(0)
        .post("/v2/workspaces", other.body.clone())
        .await
        .decode();
    let mut watcher = open(&world.base, other.workspace, Some(&world.account.token(1)))
        .await
        .unwrap();
    assert!(matches!(watcher.expect().await, Frame::Restored { .. }));
    world
        .store
        .backend()
        .inject_policy_read_delay(Some(Duration::from_secs(4)));
    let held = {
        let client = world.client(0);
        let keys = world.account.keys[0].clone();
        let op = world.created.op(&keys, 21, Digest::ZERO, |_| {});
        let path = format!("/v2/workspaces/{}/ops", workspace.to_base64url());
        tokio::spawn(async move { client.post(&path, push_body(&[op])).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = tokio::time::Instant::now();
    let op = other.op(&world.account.keys[1], 1, Digest::ZERO, |_| {});
    let _: PushResponse = support::Client::new(second_base, Some(world.account.token(1)))
        .post(
            &format!("/v2/workspaces/{}/ops", other.workspace.to_base64url()),
            push_body(std::slice::from_ref(&op)),
        )
        .await
        .decode();
    let frame = watcher.expect().await;
    assert!(matches!(frame, Frame::Records { .. }), "{frame:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the notification waited {:?} for the held push",
        started.elapsed()
    );
    assert!(
        !held.is_finished(),
        "the held push is still in its transaction"
    );
    world.store.backend().inject_policy_read_delay(None);
    let _ = held.await;
}
