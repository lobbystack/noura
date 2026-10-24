//! Helpers for the server tests: the binary, an account written into a data
//! directory, and an HTTP client that speaks the v2 encoding.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::{Bytes, Decode, Encode, List, Sorted};
use sync_protocol::messages::{ErrorBody, PushFailureDetail, PushRequest, RecordRef};
use sync_protocol::records::{AccountLink, Card, Operation, PolicyChange, PolicyKind, Signed};
use sync_protocol::types::{Digest, Id, Nonce, to_base64url};
use sync_store::Session;
use sync_store::conformance::{account, creation};

pub const BIN: &str = env!("CARGO_BIN_EXE_sync-server");

/// Runs a server command to completion.
pub fn run(args: &[&str], data: &Path) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .arg("--data")
        .arg(data)
        .output()
        .expect("the binary runs")
}

/// A running `sync-server serve`.
pub struct Server {
    pub child: Child,
    pub address: SocketAddr,
}

impl Server {
    /// Starts `serve` on a free loopback port and waits until it listens.
    pub fn start(data: &Path) -> Self {
        Self::start_with(Command::new(BIN).arg("serve").arg("--data").arg(data))
    }

    /// Starts a prepared `serve` command.
    pub fn start_with(command: &mut Command) -> Self {
        let mut child = command
            .arg("--listen")
            .arg("127.0.0.1:0")
            .stderr(Stdio::piped())
            .spawn()
            .expect("serve starts");
        let stderr = child.stderr.take().expect("stderr");
        let mut lines = BufReader::new(stderr).lines();
        let address = loop {
            let line = lines
                .next()
                .expect("serve prints its address")
                .expect("a line");
            if let Some(address) = line.strip_prefix("sync-server listening on ") {
                break address.parse().expect("an address");
            }
        };
        // Keep draining stderr so the server never blocks on it.
        std::thread::spawn(move || for _ in lines {});
        Self { child, address }
    }

    pub fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Stops the server at once, as a crash or `kill -9` would.
    pub fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An account written into a store, with signed-in devices.
pub struct Account {
    pub account: Id,
    pub keys: Vec<DeviceKeys>,
    pub links: Vec<Signed<AccountLink>>,
    pub tokens: Vec<[u8; 32]>,
}

impl Account {
    pub fn cards(&self) -> Vec<Card> {
        self.keys.iter().map(|k| k.card(self.account)).collect()
    }

    pub fn token(&self, device: usize) -> String {
        to_base64url(&self.tokens[device])
    }

    /// The creation request of a workspace the first device creates.
    pub fn creation(&self) -> Created {
        let (body, genesis, epoch_key) = creation(&self.keys[0], &self.links, &self.cards());
        let PolicyKind::Genesis { workspace_domain } = genesis.body.kind else {
            unreachable!("a genesis");
        };
        Created {
            body,
            workspace: genesis.body.workspace,
            domain: workspace_domain,
            genesis,
            epoch_key,
        }
    }
}

/// A workspace creation request and what it names.
pub struct Created {
    pub body: Vec<u8>,
    pub workspace: Id,
    pub domain: Id,
    pub genesis: Signed<PolicyChange>,
    pub epoch_key: sync_protocol::types::SecretKey,
}

impl Created {
    /// An operation of `keys` at `counter` after `previous`, on a new
    /// object, with a 272-byte ciphertext.
    pub fn op(
        &self,
        keys: &DeviceKeys,
        counter: u64,
        previous: Digest,
        edit: impl FnOnce(&mut Operation),
    ) -> Signed<Operation> {
        let mut body = Operation {
            workspace: self.workspace,
            domain: self.domain,
            device: keys.device(),
            counter,
            previous,
            epoch: 1,
            policy_revision: 1,
            policy_head: self.genesis.digest(),
            previous_attempt: Digest::ZERO,
            generation: sync_store::INITIAL_RESTORE_GENERATION,
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid: Id::random().expect("random"),
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(vec![0x5a; 272]),
        };
        edit(&mut body);
        Signed::sign(body, keys.signing())
    }
}

/// The body of a push of `ops`.
pub fn push_body(ops: &[Signed<Operation>]) -> Vec<u8> {
    PushRequest {
        records: List(ops.iter().cloned().map(RecordRef::Operation).collect()),
    }
    .to_bytes()
}

/// Writes an account of `devices` devices and a session for each into the
/// data directory's database, with the server stopped.
pub async fn seed(data: &Path, devices: usize) -> Account {
    let store = sync_server::open_store(&sync_server::Config {
        data_dir: data.to_owned(),
        listen: "127.0.0.1:0".parse().expect("an address"),
        postgres: None,
    })
    .await
    .expect("the data directory is ready");
    let seeded = seed_store(&store, devices).await;
    store.backend().close().await;
    seeded
}

/// Writes an account of `devices` devices and a session for each.
pub async fn seed_store<B: sync_store::Backend>(
    store: &sync_store::Store<B>,
    devices: usize,
) -> Account {
    let (account, keys, links) = account(devices);
    store
        .append_account_links(links.clone())
        .await
        .expect("the chain is stored");
    let mut tokens = Vec::new();
    for key in &keys {
        let token: [u8; 32] = sync_protocol::crypto::random_array().expect("random");
        store
            .create_session(
                token,
                Session {
                    account,
                    device: key.device(),
                },
            )
            .await
            .expect("the session is stored");
        tokens.push(token);
    }
    Account {
        account,
        keys,
        links,
        tokens,
    }
}

/// A response's status, headers and body.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    /// The error body, which every refusal carries.
    pub fn error(&self) -> ErrorBody {
        ErrorBody::from_bytes(&self.body).expect("an error body")
    }

    /// A push refusal's code and failing record index.
    pub fn push_failure(&self) -> (String, u32) {
        let error = self.error();
        let detail = PushFailureDetail::from_bytes(&error.detail.expect("a push detail").0)
            .expect("a push failure detail");
        assert_eq!(detail.code.as_str(), error.code.as_str());
        (error.code.0, detail.record_index)
    }

    pub fn generation(&self) -> Option<&str> {
        self.headers
            .get(sync_protocol::messages::RESTORE_GENERATION_HEADER)
            .and_then(|v| v.to_str().ok())
    }

    pub fn decode<T: Decode>(&self) -> T {
        assert_eq!(
            self.status,
            StatusCode::OK,
            "{:?}",
            ErrorBody::from_bytes(&self.body)
        );
        T::from_bytes(&self.body).expect("the response decodes")
    }
}

/// An HTTP client with a session token.
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    pub fn new(base: String, token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base,
            token,
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Reply {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request.send().await.expect("the server answers");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.expect("a body").to_vec();
        Reply {
            status,
            headers,
            body,
        }
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.send(self.http.get(format!("{}{path}", self.base)))
            .await
    }

    pub async fn post(&self, path: &str, body: Vec<u8>) -> Reply {
        self.send(
            self.http
                .post(format!("{}{path}", self.base))
                .header("content-type", sync_protocol::messages::MEDIA_TYPE)
                .body(body),
        )
        .await
    }
}
