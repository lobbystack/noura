//! Helpers for the sync tests at seam 4: the real server on SQLite, devices
//! whose cards and sessions go into its store as ticket 47's sign-in will
//! put them there, an in-memory credential store, and a proxy that can
//! tamper with what the server answers.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use app_core::sync::{DeviceCredentials, KeptHeads, Secrets, SyncError, SyncService};
use local_core::{CoreError, ErrorCategory, sync::SyncCredentials};
use sync_key_envelope::v2::{DeviceKeys, genesis_link, next_link};
use sync_protocol::codec::{Encode, Nested};
use sync_protocol::records::{AccountHead, AccountLink, LinkKind, Signed};
use sync_protocol::types::{Digest, Id, to_base64url};
use sync_store::{Session, Store};
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

/// The real server on SQLite, in a temporary data directory.
pub struct Server {
    pub base: String,
    pub store: Arc<Store<SqliteBackend>>,
    pub dir: tempfile::TempDir,
}

impl Server {
    pub async fn start() -> Self {
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
        let address = sync_server::spawn(store.clone(), "127.0.0.1:0")
            .await
            .expect("serving");
        Self {
            base: format!("http://localhost:{}", address.port()),
            store,
            dir,
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
    pub async fn new(server: &Server) -> Self {
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
    pub async fn add_device(&mut self, server: &Server) -> usize {
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
    pub async fn send(&mut self, server: &Server, sender: usize, link: Signed<AccountLink>) {
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

/// A proxy between a client and the server that forwards every request
/// and lets a test rewrite responses, as a malicious server or relay
/// would.
pub struct Proxy {
    pub base: String,
    tamper: Arc<Mutex<Option<Tamper>>>,
}

impl Proxy {
    pub async fn start(upstream: &str) -> Self {
        let tamper: Arc<Mutex<Option<Tamper>>> = Arc::new(Mutex::new(None));
        let shared = tamper.clone();
        let upstream = upstream.to_owned();
        let http = reqwest::Client::new();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let shared = shared.clone();
            let upstream = upstream.clone();
            let http = http.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, 2 << 20)
                    .await
                    .unwrap_or_default();
                let path = parts
                    .uri
                    .path_and_query()
                    .map_or("/", |p| p.as_str())
                    .to_owned();
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
                let headers = response.headers().clone();
                let mut bytes = response.bytes().await.expect("a body").to_vec();
                if status.is_success()
                    && let Some(tamper) = shared.lock().unwrap().as_ref()
                {
                    bytes = tamper(parts.uri.path(), bytes);
                }
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
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { base, tamper }
    }

    /// Rewrites every later response with `tamper`.
    pub fn tamper(&self, tamper: Tamper) {
        *self.tamper.lock().unwrap() = Some(tamper);
    }

    /// Forwards every later response unchanged.
    pub fn honest(&self) {
        *self.tamper.lock().unwrap() = None;
    }
}

/// The bytes of a response body type, for tampering.
pub fn encode(value: &impl Encode) -> Vec<u8> {
    value.to_bytes()
}
