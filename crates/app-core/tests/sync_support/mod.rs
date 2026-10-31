//! Helpers for the sync tests at seam 4: the real server on SQLite or on a
//! throwaway PostgreSQL cluster, devices
//! whose cards and sessions go into its store as ticket 47's sign-in will
//! put them there, an in-memory credential store, and a proxy that can
//! tamper with what the server answers.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use app_core::sync::{DeviceCredentials, KeptHeads, Secrets, SyncError, SyncService};
use local_core::{CoreError, ErrorCategory, sync::SyncCredentials};
use sync_key_envelope::v2::{DeviceKeys, genesis_link, next_link};
use sync_protocol::codec::{Decode, Encode, Nested};
use sync_protocol::messages::Frame;
use sync_protocol::records::{AccountHead, AccountLink, LinkKind, Signed};
use sync_protocol::types::{Digest, Id, to_base64url};
use sync_store::{Backend, Session, Store};
use sync_store_postgres::PgBackend;
use sync_store_postgres::testing::{Cluster, TestDatabase};
use sync_store_sqlite::SqliteBackend;
use zeroize::Zeroizing;

/// A credential store in memory.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SyncCredentials for MemorySecrets {
    fn read(&self, reference: &str) -> local_core::Result<Zeroizing<String>> {
        self.read_optional(reference)?.ok_or_else(|| {
            CoreError::new(
                "sync_credentials_unavailable",
                ErrorCategory::Credential,
                "no such credential",
                "sync",
            )
        })
    }

    fn read_optional(&self, reference: &str) -> local_core::Result<Option<Zeroizing<String>>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .map(Zeroizing::new))
    }

    fn write(&self, reference: &str, value: &str) -> local_core::Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(reference.to_owned(), value.to_owned());
        Ok(())
    }
}

/// The real server, on SQLite in a temporary data directory or on a
/// PostgreSQL database.
pub struct Server<B: Backend = SqliteBackend> {
    pub base: String,
    pub store: Arc<Store<B>>,
    pub dir: tempfile::TempDir,
    /// The PostgreSQL database, kept alive for the test.
    pub postgres: Option<TestDatabase>,
    /// How to reach that database, for a second server instance.
    pub instance: Option<sync_server::Postgres>,
}

/// The database file in a SQLite server's data directory.
pub const DATABASE: &str = "sync.sqlite3";

impl Server<SqliteBackend> {
    pub async fn start() -> Self {
        Self::start_with(
            sync_store::CheckpointInterval::default(),
            sync_store::Retention::default(),
        )
        .await
    }

    /// The server on SQLite, with checkpoints falling due every `entries`
    /// log entries (§17).
    pub async fn start_with_interval(entries: u64) -> Self {
        Self::start_with_retention(entries, sync_store::Retention::default()).await
    }

    /// The server on SQLite, with checkpoints falling due every `entries`
    /// log entries and `retention`'s settings (§19, §20).
    pub async fn start_with_retention(entries: u64, retention: sync_store::Retention) -> Self {
        Self::start_with(
            sync_store::CheckpointInterval {
                entries,
                weight: u64::MAX,
            },
            retention,
        )
        .await
    }

    async fn start_with(
        interval: sync_store::CheckpointInterval,
        retention: sync_store::Retention,
    ) -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        let data = dir.path().join("data");
        sync_server::migrate(&data).await.expect("migrated");
        let store = sync_server::open_store(&sync_server::Config::new(
            data,
            "127.0.0.1:0".parse().expect("an address"),
        ))
        .await
        .expect("the store opens")
        .with_checkpoint_interval(interval)
        .with_retention(retention);
        Self::serve(store, dir, None).await
    }
}

impl Server<SqliteBackend> {
    /// Copies the running database with `backup`, into the server's
    /// temporary directory.
    pub async fn backup(&self, name: &str) -> PathBuf {
        let copy = self.dir.path().join(name);
        sync_server::restore::backup(&self.dir.path().join("data"), &copy)
            .await
            .expect("the backup");
        copy
    }

    /// Serves `copy` from a new data directory after `restore`, or, with
    /// `recorded` false, as a database put back by hand, which keeps its
    /// generation. Blobs stay in the one store. Returns the new server.
    pub async fn put_back(&self, copy: &Path, recorded: bool) -> Server {
        let dir = tempfile::tempdir().expect("temporary directory");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).expect("a directory");
        std::os::unix::fs::symlink(self.dir.path().join("blobs"), data.join("blobs"))
            .expect("the shared blob store");
        std::os::unix::fs::symlink(self.dir.path().join("blobs"), dir.path().join("blobs"))
            .expect("the shared blob store");
        if recorded {
            sync_server::restore::restore(
                &data,
                copy,
                None,
                &sync_server::BlobStoreConfig::Disk,
                |_| {},
            )
            .await
            .expect("the restore");
        } else {
            std::fs::copy(copy, data.join(DATABASE)).expect("a file copy");
            std::fs::copy(
                sync_server::restore::secret_beside(copy),
                data.join("server-secret"),
            )
            .expect("the secret");
            sync_store_sqlite::clear_stamp(&data.join(DATABASE))
                .await
                .expect("a file copy carries no stamp");
        }
        let store = sync_server::open_store(&sync_server::Config::new(
            data,
            "127.0.0.1:0".parse().expect("an address"),
        ))
        .await
        .expect("the store opens");
        Self::serve(store, dir, None).await
    }
}

impl Server<PgBackend> {
    /// The server on a fresh database in the test binary's throwaway
    /// cluster under the target directory.
    pub async fn start_postgres() -> Self {
        Self::start_postgres_with(
            sync_store::CheckpointInterval::default(),
            sync_store::Retention::default(),
        )
        .await
    }

    /// The server on PostgreSQL, with checkpoints falling due every
    /// `entries` log entries (§17).
    pub async fn start_postgres_with_interval(entries: u64) -> Self {
        Self::start_postgres_with_retention(entries, sync_store::Retention::default()).await
    }

    /// The server on PostgreSQL, with checkpoints falling due every
    /// `entries` log entries and `retention`'s settings (§19, §20).
    pub async fn start_postgres_with_retention(
        entries: u64,
        retention: sync_store::Retention,
    ) -> Self {
        Self::start_postgres_with(
            sync_store::CheckpointInterval {
                entries,
                weight: u64::MAX,
            },
            retention,
        )
        .await
    }

    async fn start_postgres_with(
        interval: sync_store::CheckpointInterval,
        retention: sync_store::Retention,
    ) -> Self {
        let database = Cluster::shared(Path::new(env!("CARGO_TARGET_TMPDIR")))
            .database()
            .await;
        let postgres = sync_server::Postgres {
            url: database.url.clone(),
            secret: Some(sync_protocol::types::SecretKey::random().expect("random")),
        };
        sync_server::postgres::migrate(&postgres)
            .await
            .expect("migrated");
        let store = sync_server::postgres::open_store(&postgres)
            .await
            .expect("the store opens")
            .with_checkpoint_interval(interval)
            .with_retention(retention);
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut server = Self::serve(store, dir, Some(database)).await;
        server.instance = Some(postgres);
        server
    }

    /// A second server instance on the same database and blobs: its base
    /// URL.
    pub async fn second_instance(&self) -> String {
        let config = self.instance.as_ref().expect("a PostgreSQL server");
        let store = sync_server::postgres::open_store(config)
            .await
            .expect("the store opens");
        let blobs = sync_server::blobs::DiskStore::open(&self.dir.path().join("blobs"))
            .expect("the blob store opens");
        let address = sync_server::spawn_with_blobs(
            Arc::new(store),
            "127.0.0.1:0",
            sync_server::Accounts::local,
            Arc::new(blobs),
        )
        .await
        .expect("serving");
        format!("http://localhost:{}", address.port())
    }
}

impl Server<PgBackend> {
    /// The server on a PostgreSQL cluster of its own, which
    /// [`Self::fail_over`] can copy.
    pub async fn start_postgres_alone() -> Self {
        let cluster = Arc::new(Cluster::start(Path::new(env!("CARGO_TARGET_TMPDIR")), &[]));
        let database = cluster.database().await;
        let postgres = sync_server::Postgres {
            url: database.url.clone(),
            secret: Some(sync_protocol::types::SecretKey::random().expect("random")),
        };
        sync_server::postgres::migrate(&postgres)
            .await
            .expect("migrated");
        let store = sync_server::postgres::open_store(&postgres)
            .await
            .expect("the store opens");
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut server = Self::serve(store, dir, Some(database)).await;
        server.instance = Some(postgres);
        server
    }

    /// A copy of the database as an asynchronous standby holds it now.
    pub fn standby(&self) -> PathBuf {
        self.postgres
            .as_ref()
            .expect("a PostgreSQL server")
            .cluster
            .base_backup()
    }

    /// Serves the copy `standby` after a failover, on the same secret and
    /// blobs. With `recorded`, the operator ran `mark-restored` first.
    pub async fn fail_over(&self, standby: &Path, recorded: bool) -> Self {
        let database = self.postgres.as_ref().expect("a PostgreSQL server");
        let cluster = Arc::new(database.cluster.promoted(standby));
        let database = database.on(cluster);
        let config = sync_server::Postgres {
            url: database.url.clone(),
            secret: self.instance.as_ref().expect("a config").secret.clone(),
        };
        let dir = tempfile::tempdir().expect("temporary directory");
        std::os::unix::fs::symlink(self.dir.path().join("blobs"), dir.path().join("blobs"))
            .expect("the shared blob store");
        if recorded {
            sync_server::restore::mark_restored_postgres(
                &config,
                dir.path(),
                &sync_server::BlobStoreConfig::Disk,
                |_| {},
            )
            .await
            .expect("marked");
        }
        let store = sync_server::postgres::open_store(&config)
            .await
            .expect("the store opens");
        let mut server = Self::serve(store, dir, Some(database)).await;
        server.instance = Some(config);
        server
    }
}

impl<B: Backend> Server<B> {
    async fn serve(
        store: Store<B>,
        dir: tempfile::TempDir,
        postgres: Option<TestDatabase>,
    ) -> Self {
        let store = Arc::new(store);
        let blobs = sync_server::blobs::DiskStore::open(&dir.path().join("blobs"))
            .expect("the blob store opens");
        let address = sync_server::spawn_with_blobs(
            store.clone(),
            "127.0.0.1:0",
            sync_server::Accounts::local,
            Arc::new(blobs),
        )
        .await
        .expect("serving");
        Self {
            base: format!("http://localhost:{}", address.port()),
            store,
            dir,
            postgres,
            instance: None,
        }
    }

    /// The blob segments the server stored.
    pub fn blob_files(&self) -> Vec<PathBuf> {
        files(&self.dir.path().join("blobs"))
            .into_iter()
            .filter(|f| !f.to_string_lossy().contains(".partial"))
            .collect()
    }

    /// The files that hold what the server stored, blob segments included.
    pub async fn stored_files(&self) -> Vec<PathBuf> {
        let mut stored = self.database_files().await;
        stored.extend(self.blob_files());
        stored
    }

    async fn database_files(&self) -> Vec<PathBuf> {
        match &self.postgres {
            None => {
                let data = self.dir.path().join("data");
                vec![data.join(DATABASE), data.join(format!("{DATABASE}-wal"))]
            }
            Some(database) => {
                // A checkpoint writes every page to the data files.
                database.cluster.execute("CHECKPOINT").await;
                database.cluster.data_dir().map_or_else(Vec::new, files)
            }
        }
    }

    /// A device of `account` whose card and session the store holds.
    pub async fn device(&self, account: Id) -> Device {
        let keys = DeviceKeys::generate().expect("random");
        self.store
            .enroll_card(keys.card(account))
            .await
            .expect("the card enrolls");
        let token: [u8; 32] = sync_protocol::crypto::random_array().expect("random");
        self.store
            .create_session(token, Session::new(account, keys.device()))
            .await
            .expect("the session is stored");
        Device {
            account,
            keys,
            token: to_base64url(&token),
            state: tempfile::tempdir().expect("an application data folder"),
            secrets: Arc::new(MemorySecrets::default()),
        }
    }
}

/// A device: its keys, its session, its application data folder and its
/// credential store.
pub struct Device {
    pub account: Id,
    pub keys: DeviceKeys,
    pub token: String,
    pub state: tempfile::TempDir,
    pub secrets: Arc<MemorySecrets>,
}

impl Device {
    pub fn id(&self) -> Id {
        self.keys.device()
    }

    /// The folder the device keeps its account heads in.
    pub fn heads_dir(&self) -> PathBuf {
        self.state.path().join("sync")
    }

    pub fn secrets(&self) -> Secrets {
        self.secrets.clone()
    }

    pub fn heads(&self) -> Result<KeptHeads, SyncError> {
        KeptHeads::open(&self.heads_dir(), self.account, self.id(), &*self.secrets)
    }

    pub fn credentials(&self, reference: AccountHead) -> DeviceCredentials {
        DeviceCredentials {
            account: self.account,
            keys: self.keys.clone(),
            session_token: self.token.clone(),
            reference_head: reference,
        }
    }

    /// Connects to `base` with this device's kept heads.
    pub async fn connect(&self, base: &str, reference: AccountHead) -> SyncService {
        SyncService::connect(
            base,
            self.credentials(reference),
            self.heads().expect("the kept heads open"),
            self.secrets(),
        )
        .await
        .expect("connects")
    }
}

/// Every file under `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

pub fn head_of(links: &[Signed<AccountLink>]) -> AccountHead {
    let last = links.last().expect("links");
    AccountHead {
        account: last.body.account,
        genesis: links[0].digest(),
        seqno: last.body.seqno,
        head: last.digest(),
    }
}

/// An account whose first device signed its genesis through the chain
/// route.
pub struct Account {
    pub id: Id,
    pub devices: Vec<Device>,
    pub links: Vec<Signed<AccountLink>>,
}

impl Account {
    pub async fn new<B: Backend>(server: &Server<B>) -> Self {
        let id = Id::random().expect("random");
        let first = server.device(id).await;
        let genesis = genesis_link(&first.keys, id);
        let mut client = app_core::sync::SyncClient::connect(&server.base, &first.token)
            .await
            .expect("connects");
        client
            .append_chain(vec![genesis.clone()])
            .await
            .expect("the genesis");
        Self {
            id,
            devices: vec![first],
            links: vec![genesis],
        }
    }

    /// The head the first device verified its chain from: its genesis.
    pub fn reference(&self) -> AccountHead {
        head_of(&self.links[..1])
    }

    pub fn head(&self) -> AccountHead {
        head_of(&self.links)
    }

    /// A link the device at `signer` signs after the head.
    pub fn link(&self, signer: usize, kind: LinkKind) -> Signed<AccountLink> {
        next_link(&self.devices[signer].keys, &self.head(), kind)
    }

    /// Enrolls a new device and adds it through the first device's session.
    pub async fn add_device<B: Backend>(&mut self, server: &Server<B>) -> usize {
        let device = server.device(self.id).await;
        let link = self.link(
            0,
            LinkKind::AddDevice {
                card: Nested(device.keys.card(self.id)),
                offer: Digest([9; 32]),
            },
        );
        self.devices.push(device);
        self.send(server, 0, link).await;
        self.devices.len() - 1
    }

    /// Sends a link from the session of the device at `sender`.
    pub async fn send<B: Backend>(
        &mut self,
        server: &Server<B>,
        sender: usize,
        link: Signed<AccountLink>,
    ) {
        let mut client =
            app_core::sync::SyncClient::connect(&server.base, &self.devices[sender].token)
                .await
                .expect("connects");
        client
            .append_chain(vec![link.clone()])
            .await
            .expect("the link is appended");
        self.links.push(link);
    }
}

/// What the proxy does to a response: it gets the route's path and the
/// response body and returns the body to send.
pub type Tamper = Box<dyn Fn(&str, Vec<u8>) -> Vec<u8> + Send + Sync>;

/// Answers a request in the server's place: given the method and path, the
/// status and body to send, or `None` to forward the request.
pub type Forge = Box<dyn Fn(&str, &str) -> Option<(u16, Vec<u8>)> + Send + Sync>;

/// Picks, by method and path, a request whose answer the proxy loses: it
/// forwards the request, then answers 502 with no error body, as a relay
/// that drops the connection after the server commits.
pub type Lose = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// Rewrites a request's path and query before the proxy forwards it.
pub type Reroute = Box<dyn Fn(&str) -> String + Send + Sync>;

/// Holds the next request whose path ends with `suffix` until the test
/// releases it: the proxy signals `arrived`, then waits for `release`.
struct Hold {
    suffix: String,
    arrived: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

/// Rewrites each realtime frame the server sends: the frames to send in
/// its place, none to drop it.
pub type FrameTamper = Box<dyn Fn(Frame) -> Vec<Frame> + Send + Sync>;

/// A proxy between a client and the server that forwards every request
/// and lets a test rewrite responses, or answer in the server's place, as
/// a malicious server or relay would. It bridges realtime channels too,
/// and can rewrite or drop their frames, count the client's pings and
/// close every channel.
pub struct Proxy {
    pub base: String,
    tamper: Arc<Mutex<Option<Tamper>>>,
    forge: Arc<Mutex<Option<Forge>>>,
    lose: Arc<Mutex<Option<Lose>>>,
    reroute: Arc<Mutex<Option<Reroute>>>,
    hold: Arc<Mutex<Option<Hold>>>,
    upstream: Arc<Mutex<String>>,
    /// Every request the proxy received: its method, path with query,
    /// headers and body.
    requests: Arc<Mutex<Vec<Captured>>>,
    /// Every response body and realtime frame the proxy passed on, with
    /// the path it answered or `frame`.
    received: Received,
    frames: Arc<Mutex<Option<FrameTamper>>>,
    pings: Arc<AtomicUsize>,
    closing: tokio::sync::watch::Sender<u64>,
}

/// The response bodies and frames a proxy passed on, each with the path it
/// answered or `frame`.
type Received = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// One request a proxy received.
#[derive(Clone, Debug)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
}

/// Bridges a client's realtime channel to the server's, passing each
/// frame the server sends through `frames`.
async fn bridge(
    client: axum::extract::ws::WebSocket,
    server: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    frames: Arc<Mutex<Option<FrameTamper>>>,
    pings: Arc<AtomicUsize>,
    mut closing: tokio::sync::watch::Receiver<u64>,
    received: Received,
) {
    use axum::extract::ws::Message as Client;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Server;
    let (mut to_client, mut from_client) = client.split();
    let (mut to_server, mut from_server) = server.split();
    loop {
        tokio::select! {
            _ = closing.changed() => return,
            message = from_client.next() => match message {
                Some(Ok(Client::Binary(bytes))) => {
                    if to_server.send(Server::Binary(bytes)).await.is_err() {
                        return;
                    }
                }
                Some(Ok(Client::Ping(_))) => {
                    pings.fetch_add(1, Ordering::SeqCst);
                }
                Some(Ok(Client::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
            message = from_server.next() => match message {
                Some(Ok(Server::Binary(bytes))) => {
                    let frame = Frame::from_bytes(&bytes).expect("the server sends frames");
                    received.lock().unwrap().push(("frame".to_owned(), bytes.to_vec()));
                    let sent = match frames.lock().unwrap().as_ref() {
                        Some(tamper) => tamper(frame),
                        None => vec![frame],
                    };
                    for frame in sent {
                        if to_client.send(Client::Binary(frame.to_bytes().into())).await.is_err() {
                            return;
                        }
                    }
                }
                Some(Ok(Server::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

impl Proxy {
    pub async fn start(upstream: &str) -> Self {
        let proxy = Self::listen().await;
        *proxy.upstream.lock().unwrap() = upstream.to_owned();
        proxy
    }

    /// The real server on SQLite behind a proxy whose address the server
    /// takes as its origin, as a relay in front of it would: the device
    /// page and enrollment name the proxy's address, and the server's
    /// `base` is the proxy's, so every client goes through it.
    pub async fn in_front() -> (Server, Self) {
        let proxy = Self::listen().await;
        let dir = tempfile::tempdir().expect("temporary directory");
        let data = dir.path().join("data");
        sync_server::migrate(&data).await.expect("migrated");
        let store = Arc::new(
            sync_server::open_store(&sync_server::Config::new(
                data,
                "127.0.0.1:0".parse().expect("an address"),
            ))
            .await
            .expect("the store opens"),
        );
        let origin = sync_server::PublicOrigin::parse(&proxy.base).expect("an origin");
        let address = sync_server::spawn_with(store.clone(), "127.0.0.1:0", move |_| {
            sync_server::Accounts::local(origin)
        })
        .await
        .expect("serving");
        *proxy.upstream.lock().unwrap() = format!("http://localhost:{}", address.port());
        // Every client reaches the server through the proxy.
        let server = Server {
            base: proxy.base.clone(),
            store,
            dir,
            postgres: None,
            instance: None,
        };
        (server, proxy)
    }

    async fn listen() -> Self {
        let tamper: Arc<Mutex<Option<Tamper>>> = Arc::new(Mutex::new(None));
        let forge: Arc<Mutex<Option<Forge>>> = Arc::new(Mutex::new(None));
        let reroute: Arc<Mutex<Option<Reroute>>> = Arc::new(Mutex::new(None));
        let lose: Arc<Mutex<Option<Lose>>> = Arc::new(Mutex::new(None));
        let lost = lose.clone();
        let hold: Arc<Mutex<Option<Hold>>> = Arc::new(Mutex::new(None));
        let held = hold.clone();
        let upstream: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let requests: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let shared = tamper.clone();
        let forged = forge.clone();
        let rerouted = reroute.clone();
        let target = upstream.clone();
        let frames: Arc<Mutex<Option<FrameTamper>>> = Arc::new(Mutex::new(None));
        let pings = Arc::new(AtomicUsize::new(0));
        let (closing, _) = tokio::sync::watch::channel(0u64);
        let received: Received = Arc::new(Mutex::new(Vec::new()));
        let recording = received.clone();
        let bridged = (frames.clone(), pings.clone(), closing.clone());
        let http = reqwest::Client::new();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let shared = shared.clone();
            let forged = forged.clone();
            let rerouted = rerouted.clone();
            let lost = lost.clone();
            let held = held.clone();
            let upstream = target.lock().unwrap().clone();
            let http = http.clone();
            let captured = captured.clone();
            let (frames, pings, closing) = bridged.clone();
            let recording = recording.clone();
            async move {
                let (mut parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, 2 << 20)
                    .await
                    .unwrap_or_default();
                captured.lock().unwrap().push(Captured {
                    method: parts.method.to_string(),
                    path: parts
                        .uri
                        .path_and_query()
                        .map_or("/", |p| p.as_str())
                        .to_owned(),
                    headers: parts
                        .headers
                        .iter()
                        .map(|(n, v)| (n.to_string(), v.as_bytes().to_vec()))
                        .collect(),
                    body: body.to_vec(),
                });
                let answer = forged
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|forge| forge(parts.method.as_str(), parts.uri.path()));
                if let Some((status, bytes)) = answer {
                    return axum::response::Response::builder()
                        .status(status)
                        .header("content-type", "application/octet-stream")
                        .body(axum::body::Body::from(bytes))
                        .unwrap();
                }
                let waiting = {
                    let mut hold = held.lock().unwrap();
                    if hold
                        .as_ref()
                        .is_some_and(|h| parts.uri.path().ends_with(&h.suffix))
                    {
                        hold.take()
                    } else {
                        None
                    }
                };
                if let Some(hold) = waiting {
                    hold.arrived.notify_one();
                    hold.release.notified().await;
                }
                if parts.uri.path().ends_with("/realtime") {
                    use axum::extract::FromRequestParts;
                    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
                    let upgrade =
                        axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &())
                            .await
                            .expect("a WebSocket upgrade");
                    let url = format!(
                        "{}{}",
                        upstream.replace("http://", "ws://"),
                        parts.uri.path()
                    );
                    let mut request = url.into_client_request().unwrap();
                    if let Some(token) = parts.headers.get("authorization") {
                        request.headers_mut().insert("authorization", token.clone());
                    }
                    return match tokio_tungstenite::connect_async(request).await {
                        Ok((server, _)) => {
                            let closing = closing.subscribe();
                            upgrade.on_upgrade(move |client| {
                                bridge(client, server, frames, pings, closing, recording)
                            })
                        }
                        Err(tokio_tungstenite::tungstenite::Error::Http(refused)) => {
                            let (head, body) = refused.into_parts();
                            axum::response::Response::builder()
                                .status(head.status.as_u16())
                                .header("content-type", "application/octet-stream")
                                .body(axum::body::Body::from(body.unwrap_or_default()))
                                .unwrap()
                        }
                        Err(error) => panic!("the server's channel didn't open: {error}"),
                    };
                }
                let path = parts
                    .uri
                    .path_and_query()
                    .map_or("/", |p| p.as_str())
                    .to_owned();
                let path = match rerouted.lock().unwrap().as_ref() {
                    Some(reroute) => reroute(&path),
                    None => path,
                };
                let mut forward = http.request(parts.method.clone(), format!("{upstream}{path}"));
                for (name, value) in &parts.headers {
                    if name != "host" {
                        forward = forward.header(name, value);
                    }
                }
                let response = forward
                    .body(body.to_vec())
                    .send()
                    .await
                    .expect("upstream answers");
                let status = response.status();
                if lost
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|lose| lose(parts.method.as_str(), parts.uri.path()))
                {
                    return axum::response::Response::builder()
                        .status(502)
                        .body(axum::body::Body::empty())
                        .unwrap();
                }
                let headers = response.headers().clone();
                let mut bytes = response.bytes().await.expect("a body").to_vec();
                if status.is_success()
                    && let Some(tamper) = shared.lock().unwrap().as_ref()
                {
                    bytes = tamper(parts.uri.path(), bytes);
                }
                recording
                    .lock()
                    .unwrap()
                    .push((parts.uri.path().to_owned(), bytes.clone()));
                let mut reply = axum::response::Response::builder().status(status.as_u16());
                for (name, value) in &headers {
                    if name != "content-length" && name != "transfer-encoding" {
                        reply = reply.header(name, value);
                    }
                }
                reply.body(axum::body::Body::from(bytes)).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let base = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            tamper,
            forge,
            lose,
            reroute,
            hold,
            upstream,
            requests,
            received,
            frames,
            pings,
            closing,
        }
    }

    /// Every response body and realtime frame the proxy passed on so far,
    /// each with the path it answered or `frame`.
    pub fn received(&self) -> Vec<(String, Vec<u8>)> {
        self.received.lock().unwrap().clone()
    }

    /// Every request the proxy received so far.
    pub fn requests(&self) -> Vec<Captured> {
        self.requests.lock().unwrap().clone()
    }

    /// Rewrites or drops every later realtime frame with `tamper`.
    pub fn tamper_frames(&self, tamper: FrameTamper) {
        *self.frames.lock().unwrap() = Some(tamper);
    }

    /// The pings the clients sent on their realtime channels.
    pub fn pings(&self) -> usize {
        self.pings.load(Ordering::SeqCst)
    }

    /// Closes every realtime channel open through the proxy, as a lost
    /// connection would.
    pub fn close_channels(&self) {
        self.closing.send_modify(|n| *n += 1);
    }

    /// Answers every later request that `forge` picks in the server's
    /// place.
    pub fn forge(&self, forge: Forge) {
        *self.forge.lock().unwrap() = Some(forge);
    }

    /// Rewrites every later response with `tamper`.
    pub fn tamper(&self, tamper: Tamper) {
        *self.tamper.lock().unwrap() = Some(tamper);
    }

    /// Loses the answer to every later request that `lose` picks, after
    /// the server handled it.
    pub fn lose(&self, lose: Lose) {
        *self.lose.lock().unwrap() = Some(lose);
    }

    /// Sends every later request to the path `reroute` gives it.
    pub fn reroute(&self, reroute: Reroute) {
        *self.reroute.lock().unwrap() = Some(reroute);
    }

    /// Holds the next request whose path ends with `suffix` before the
    /// proxy forwards it. Returns the signal that it arrived and the one
    /// that releases it.
    pub fn hold(&self, suffix: &str) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let arrived = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *self.hold.lock().unwrap() = Some(Hold {
            suffix: suffix.to_owned(),
            arrived: arrived.clone(),
            release: release.clone(),
        });
        (arrived, release)
    }

    /// Forwards every later request and response unchanged.
    /// Sends every later request to `upstream`.
    pub fn retarget(&self, upstream: &str) {
        *self.upstream.lock().unwrap() = upstream.to_owned();
    }

    pub fn honest(&self) {
        *self.frames.lock().unwrap() = None;
        *self.tamper.lock().unwrap() = None;
        *self.forge.lock().unwrap() = None;
        *self.lose.lock().unwrap() = None;
        *self.reroute.lock().unwrap() = None;
    }
}

/// The bytes of a response body type, for tampering.
pub fn encode(value: &impl Encode) -> Vec<u8> {
    value.to_bytes()
}

/// Approves a device grant's code on the server once the app first waits,
/// and moves the server's clock by each wait, as time passing would.
pub struct Approving<B: Backend> {
    store: Arc<Store<B>>,
    account: Id,
    code: Mutex<Option<String>>,
}

impl<B: Backend> app_core::sync::device::Pause for Approving<B> {
    fn pause(
        &self,
        seconds: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.store
                .clock()
                .advance(std::time::Duration::from_secs(seconds));
            let code = self.code.lock().unwrap().take();
            if let Some(code) = code {
                assert!(
                    self.store
                        .decide_device_grant(self.account, &code, true)
                        .await
                        .expect("decided"),
                    "the code waits"
                );
            }
        })
    }
}

impl<B: Backend> Server<B> {
    /// Signs a person up with `email` and returns their account.
    pub async fn person(&self, email: &str) -> Id {
        let link = self.store.issue_magic_link(email).await.expect("a link");
        self.store
            .sign_in_with_magic_link(&link)
            .await
            .expect("used")
            .expect("a current link")
            .0
    }

    /// Signs a desktop in to `account` through the device grant at `base`,
    /// approving its code as you would on the device page.
    pub async fn sign_in(&self, account: Id, base: &str) -> app_core::sync::device::SignedIn {
        let grant = app_core::sync::device::DeviceSignIn::start(base, &|_| Ok(()))
            .await
            .expect("the grant starts");
        let pause = Approving {
            store: self.store.clone(),
            account,
            code: Mutex::new(Some(grant.user_code().to_owned())),
        };
        grant.finish(&pause).await.expect("signed in")
    }
}

/// A signed-in device's sync service, with its own application data folder
/// and credential store.
pub struct Desktop {
    pub service: SyncService,
    pub credentials: DeviceCredentials,
    pub secrets: Arc<MemorySecrets>,
    pub state: tempfile::TempDir,
}

impl Desktop {
    /// The service of a device that joined its account's chain.
    pub async fn connect(
        base: &str,
        credentials: DeviceCredentials,
        secrets: Arc<MemorySecrets>,
        state: tempfile::TempDir,
    ) -> Self {
        let heads = KeptHeads::open(
            state.path(),
            credentials.account,
            credentials.keys.device(),
            &*secrets,
        )
        .expect("the kept heads open");
        let service = SyncService::connect(base, credentials.clone(), heads, secrets.clone())
            .await
            .expect("connects");
        Self {
            service,
            credentials,
            secrets,
            state,
        }
    }

    pub fn id(&self) -> Id {
        self.credentials.keys.device()
    }

    /// Another connection of this desktop's device, through `base`, such as
    /// a proxy.
    pub async fn connect_through(&self, base: &str) -> SyncService {
        let heads = KeptHeads::open(
            self.state.path(),
            self.credentials.account,
            self.credentials.keys.device(),
            &*self.secrets,
        )
        .expect("the kept heads open");
        SyncService::connect(base, self.credentials.clone(), heads, self.secrets.clone())
            .await
            .expect("connects")
    }

    pub fn secrets(&self) -> Secrets {
        self.secrets.clone()
    }
}

/// The first device of a new account: it signs in through the device
/// grant, enrolls and signs the genesis.
pub async fn first_desktop<B: Backend>(server: &Server<B>, email: &str) -> Desktop {
    let account = server.person(email).await;
    let signed_in = server.sign_in(account, &server.base).await;
    let secrets = Arc::new(MemorySecrets::default());
    let credentials = match app_core::sync::device::join(&signed_in, &*secrets)
        .await
        .expect("enrolled")
    {
        app_core::sync::device::Joined::First(credentials) => credentials,
        _ => panic!("a first device"),
    };
    Desktop::connect(
        &server.base,
        credentials,
        secrets,
        tempfile::tempdir().expect("a folder"),
    )
    .await
}

/// A candidate device of `account`: signed in and enrolled, with its
/// approval request posted.
pub struct NewDesktop {
    pub candidate: app_core::sync::approval::Candidate,
    pub secrets: Arc<MemorySecrets>,
    pub state: tempfile::TempDir,
    pub heads: KeptHeads,
}

/// Signs a new device of `account` in, then posts its approval request and
/// polls for offers through `base`, such as a tampering proxy.
pub async fn new_desktop<B: Backend>(server: &Server<B>, account: Id, base: &str) -> NewDesktop {
    let signed_in = server.sign_in(account, &server.base).await;
    let secrets = Arc::new(MemorySecrets::default());
    let (keys, client) = match app_core::sync::device::join(&signed_in, &*secrets)
        .await
        .expect("enrolled")
    {
        app_core::sync::device::Joined::NeedsApproval { keys, client } => (keys, client),
        _ => panic!("the account has a chain"),
    };
    let client = if base == server.base {
        client
    } else {
        app_core::sync::SyncClient::connect(base, &signed_in.session_token)
            .await
            .expect("connects")
    };
    let device = keys.device();
    let candidate = app_core::sync::approval::Candidate::request(client, keys, account)
        .await
        .expect("the request");
    let state = tempfile::tempdir().expect("a folder");
    let heads = KeptHeads::open(state.path(), account, device, &*secrets).expect("heads");
    NewDesktop {
        candidate,
        secrets,
        state,
        heads,
    }
}

/// Approves a new device of `approver`'s account through the whole flow of
/// §10, recording its head in `workspaces`, and returns its service.
pub async fn approve<B: Backend>(
    server: &Server<B>,
    approver: &mut Desktop,
    workspaces: &mut [app_core::sync::RemoteWorkspace],
) -> Desktop {
    let account = approver.credentials.account;
    let mut new = new_desktop(server, account, &server.base).await;
    let fingerprint = new.candidate.fingerprint();
    let pending = approver
        .service
        .pending_devices()
        .await
        .expect("listed")
        .into_iter()
        .find(|p| p.fingerprint == fingerprint)
        .expect("the request");
    let mut offer = approver
        .service
        .offer_approval(&pending, workspaces)
        .await
        .expect("offered");
    let app_core::sync::approval::CandidateStep::Code(code) =
        new.candidate.poll().await.expect("polled")
    else {
        panic!("a code");
    };
    assert_eq!(
        approver.service.approval_code(&mut offer).await.unwrap(),
        Some(code)
    );
    new.candidate.confirm();
    approver
        .service
        .confirm_approval(offer, workspaces)
        .await
        .expect("committed");
    new.joined(&server.base).await
}

impl NewDesktop {
    /// The approved device's service, once its approval committed.
    pub async fn joined(mut self, base: &str) -> Desktop {
        let credentials = self
            .candidate
            .joined(&mut self.heads, &*self.secrets)
            .await
            .expect("the approval verifies")
            .expect("the approver committed");
        drop(self.heads);
        Desktop::connect(base, credentials, self.secrets, self.state).await
    }
}

/// A workspace folder on one desktop, with its engine and the app data
/// that holds the registry of synced folders.
pub struct Folder {
    pub dir: tempfile::TempDir,
    pub app_data: tempfile::TempDir,
    pub engine: Arc<local_core::WorkspaceEngine>,
    pub registry: app_core::sync::registry::Registry,
}

impl Folder {
    /// A new workspace named `name` in an empty folder.
    pub fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("a folder");
        let app_data = tempfile::tempdir().expect("app data");
        let engine = Arc::new(
            local_core::WorkspaceEngine::create_with_app_data(dir.path(), name, app_data.path())
                .expect("the workspace opens"),
        );
        let registry =
            app_core::sync::registry::Registry::open(app_data.path()).expect("the registry");
        Self {
            dir,
            app_data,
            engine,
            registry,
        }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    /// Writes a file as another app would.
    pub fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Every file outside `.noura`, with its bytes.
    pub fn files(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        files(self.dir.path())
            .into_iter()
            .filter_map(|path| {
                let relative = path
                    .strip_prefix(self.dir.path())
                    .ok()?
                    .to_str()?
                    .replace('\\', "/");
                (!relative.starts_with(".noura/"))
                    .then(|| (relative, std::fs::read(&path).unwrap()))
            })
            .collect()
    }

    /// Opens the folder's engine again, as a relaunch does.
    pub fn reopen(&mut self) {
        self.engine = Arc::new(
            local_core::WorkspaceEngine::open_with_app_data(self.dir.path(), self.app_data.path())
                .expect("the workspace opens again"),
        );
        self.registry =
            app_core::sync::registry::Registry::open(self.app_data.path()).expect("the registry");
    }
}

/// A laptop whose sync goes through a proxy that can cut it off, a
/// workspace it synced, and a second desktop of the account that holds a
/// copy.
pub struct Pair<B: Backend> {
    pub server: Server<B>,
    pub proxy: Proxy,
    pub laptop: Desktop,
    pub service: app_core::sync::SyncService,
    pub a: Folder,
    pub sync_a: app_core::sync::workspace::WorkspaceSync,
    pub desktop: Desktop,
    pub b: Folder,
    pub sync_b: app_core::sync::workspace::WorkspaceSync,
}

pub async fn pair() -> Pair<SqliteBackend> {
    pair_on(Server::start().await).await
}

/// A [`Pair`] on `server`.
pub async fn pair_on<B: Backend>(server: Server<B>) -> Pair<B> {
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut service = laptop.connect_through(&proxy.base).await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", b"# Plan\n");
    let mut sync_a = app_core::sync::workspace::WorkspaceSync::turn_on(
        &mut service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.round(&mut service, 0).await.expect("a round");
    let mut desktop = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut b = Folder::new("Copy");
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
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
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(b.files(), a.files());
    Pair {
        server,
        proxy,
        laptop,
        service,
        a,
        sync_a,
        desktop,
        b,
        sync_b,
    }
}

/// A [`Pair`] on `server` whose desktop belongs to another account, a
/// member the laptop's account invited as an admin.
pub async fn pair_of_accounts_on<B: Backend>(server: Server<B>) -> Pair<B> {
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut service = laptop.connect_through(&proxy.base).await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", b"# Plan\n");
    let mut sync_a = app_core::sync::workspace::WorkspaceSync::turn_on(
        &mut service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.round(&mut service, 0).await.expect("a round");
    let mut book =
        app_core::sync::invitations::InvitationBook::open(laptop.state.path(), laptop.id())
            .expect("the book opens");
    let mut desktop = first_desktop(&server, "ben@example.com").await;
    let (mut sync_b, b) = invite_and_join(
        &mut laptop,
        &mut sync_a,
        &mut book,
        &mut desktop,
        sync_protocol::records::Role::Admin,
    )
    .await;
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a round");
    sync_a.round(&mut service, 0).await.expect("a round");
    assert_eq!(b.files(), a.files());
    Pair {
        server,
        proxy,
        laptop,
        service,
        a,
        sync_a,
        desktop,
        b,
        sync_b,
    }
}

/// Answers every request with a 503, as a server you can't reach would.
pub fn cut_off(proxy: &Proxy) {
    proxy.forge(Box::new(|_, _| Some((503, Vec::new()))));
}

/// The body of a `not_found` error, for a forged answer.
pub fn not_found_body() -> Vec<u8> {
    use sync_protocol::codec::Text;
    sync_protocol::messages::ErrorBody {
        code: Text::new("not_found").unwrap(),
        message: Text::new("not found").unwrap(),
        retry_after_seconds: None,
        detail: None,
    }
    .to_bytes()
}

/// Invites `member`'s account to `inviter`'s synced workspace with `role`,
/// through the whole flow of §10 and §11: the link, acceptance, the
/// invited request, the offer, the matching codes and the `add_member`.
/// The member's device then downloads the workspace into a new folder and
/// records its own head there, as §10 step 6 says. Returns its sync and
/// folder.
pub async fn invite_and_join(
    inviter: &mut Desktop,
    inviter_sync: &mut app_core::sync::workspace::WorkspaceSync,
    book: &mut app_core::sync::invitations::InvitationBook,
    member: &mut Desktop,
    role: sync_protocol::records::Role,
) -> (app_core::sync::workspace::WorkspaceSync, Folder) {
    use app_core::sync::approval::CandidateStep;
    use app_core::sync::invitations::{InvitationLink, NewInvitation};

    let workspace = inviter_sync.workspace();
    let created = inviter
        .service
        .create_invitation(
            book,
            NewInvitation {
                workspace,
                role,
                workspace_name: "Field notes",
                inviter_name: "Ana",
            },
        )
        .await
        .expect("invited");
    let link = InvitationLink::parse(&created.url).expect("the link reads");
    let opened = member
        .service
        .open_invitation(&link)
        .await
        .expect("it opens");
    member
        .service
        .accept_invitation(&link)
        .await
        .expect("accepted");
    let mut candidate = member
        .service
        .request_invited_approval(&link, &opened)
        .await
        .expect("the request");
    let requests = inviter
        .service
        .invited_requests(book, workspace)
        .await
        .expect("listed");
    let request = requests
        .iter()
        .find(|r| r.commit == candidate.commit())
        .expect("the request");
    let mut offer = inviter
        .service
        .offer_invited(request, inviter_sync.remote_mut().expect("connected"))
        .await
        .expect("offered");
    let CandidateStep::Code(code) = member
        .service
        .poll_invited(&mut candidate)
        .await
        .expect("polled")
    else {
        panic!("a code");
    };
    assert_eq!(
        inviter.service.approval_code(&mut offer).await.unwrap(),
        Some(code)
    );
    candidate.confirm();
    inviter_sync
        .approve_member(&mut inviter.service, offer)
        .await
        .expect("the approval commits");
    assert!(
        member
            .service
            .join_invited(&candidate)
            .await
            .expect("joined")
    );
    let mut folder = Folder::new("Member's copy");
    let mut sync = app_core::sync::workspace::WorkspaceSync::download(
        &mut member.service,
        folder.engine.clone(),
        workspace,
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    sync.round(&mut member.service, 0).await.expect("a round");
    let remote = sync.remote_mut().expect("connected");
    let own = member
        .service
        .pending_heads(remote)
        .await
        .expect("pending")
        .into_iter()
        .find(|p| p.own);
    if let Some(own) = own {
        member
            .service
            .push_pending_head(remote, &own)
            .await
            .expect("recorded");
    }
    sync.round(&mut member.service, 0).await.expect("a round");
    (sync, folder)
}
