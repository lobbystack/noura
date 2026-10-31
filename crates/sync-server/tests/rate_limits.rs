//! The rate limits of §26 at seam 4: a request over a limit gets
//! `rate_limited` with `retry_after_seconds`, each class of traffic counts
//! apart per device, uploads and pushes never share a budget, sign-in counts
//! per client IP read only from a configured header, and in PostgreSQL mode
//! the limits hold across two instances.

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderName;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use reqwest::header::{CONTENT_SECURITY_POLICY, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use support::{Account, BIN, Client, Created, Reply, Server, push_body, seed_store};
use sync_protocol::blob::encrypt_blob;
use sync_protocol::codec::Decode;
use sync_protocol::messages::{Frame, PushResponse};
use sync_protocol::records::BlobRef;
use sync_protocol::types::{Digest, Id, SecretKey};
use sync_server::blobs::{Objects, SegmentStore};
use sync_server::{Accounts, RateGate, RateLimits};
use sync_store::{Backend, Limit, Store};
use sync_store_postgres::testing::Cluster;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// An allowance in a window far longer than any test.
fn hour(allowance: u64) -> Limit {
    Limit {
        allowance,
        window: Duration::from_secs(3600),
    }
}

/// Limits no test reaches unless it lowers one.
fn wide() -> RateLimits {
    RateLimits {
        pushes: hour(1 << 20),
        push_bytes: hour(1 << 40),
        upload_bytes: hour(1 << 40),
        read_bytes: hour(1 << 40),
        bootstrap_bytes: hour(1 << 40),
        requests: hour(1 << 20),
        sign_in: hour(1 << 20),
    }
}

struct World<B: Backend> {
    base: String,
    account: Account,
    created: Created,
    store: Arc<Store<B>>,
    blobs: Arc<dyn SegmentStore>,
    _dir: tempfile::TempDir,
}

impl<B: Backend> World<B> {
    fn client(&self, device: usize) -> Client {
        Client::new(self.base.clone(), Some(self.account.token(device)))
    }

    fn workspace(&self) -> String {
        self.created.workspace.to_base64url()
    }

    /// Pushes one new operation of `device` at `counter`.
    async fn push(&self, device: usize, counter: u64, previous: Digest) -> (Reply, Digest) {
        let op = self
            .created
            .op(&self.account.keys[device], counter, previous, |_| {});
        let path = format!("/v2/workspaces/{}/ops", self.workspace());
        let reply = self
            .client(device)
            .post(&path, push_body(std::slice::from_ref(&op)))
            .await;
        (reply, op.digest())
    }

    fn tus(&self, device: usize, method: Method, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.account.token(device))
            .header("Tus-Resumable", "1.0.0")
    }

    /// Creates an upload of `blob_ref` and returns its location.
    async fn create_blob(&self, device: usize, blob_ref: &BlobRef) -> String {
        let metadata = format!(
            "blob {},digest {},domain {}",
            STANDARD.encode(blob_ref.blob.0),
            STANDARD.encode(blob_ref.ciphertext_digest.0),
            STANDARD.encode(self.created.domain.0)
        );
        let created = self
            .tus(
                device,
                Method::POST,
                &format!("/v2/workspaces/{}/blobs", self.workspace()),
            )
            .header("Upload-Length", blob_ref.ciphertext_length)
            .header("Upload-Metadata", metadata)
            .send()
            .await
            .expect("creation");
        assert_eq!(created.status(), StatusCode::CREATED);
        created.headers()["Location"]
            .to_str()
            .expect("text")
            .to_owned()
    }

    async fn patch(
        &self,
        device: usize,
        location: &str,
        offset: u64,
        bytes: &[u8],
    ) -> reqwest::Response {
        self.tus(device, Method::PATCH, location)
            .header("Upload-Offset", offset)
            .header("Content-Type", "application/offset+octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .expect("patch")
    }

    /// An upload of `len` bytes of file, with its ciphertext.
    fn blob(len: usize) -> (Vec<u8>, BlobRef) {
        encrypt_blob(
            &vec![7u8; len],
            Id::random().expect("random"),
            SecretKey::random().expect("random"),
        )
        .expect("encrypts")
    }
}

async fn world_on<B: Backend>(
    store: Arc<Store<B>>,
    rate: RateGate,
    dir: tempfile::TempDir,
) -> World<B> {
    let blobs: Arc<dyn SegmentStore> = Arc::new(Objects::memory());
    let address = sync_server::spawn_limited(
        store.clone(),
        "127.0.0.1:0",
        Accounts::local,
        blobs.clone(),
        rate,
    )
    .await
    .expect("serving");
    let account = seed_store(&store, 2).await;
    let created = account.creation();
    let base = format!("http://{address}");
    let reply = Client::new(base.clone(), Some(account.token(0)))
        .post("/v2/workspaces", created.body.clone())
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    World {
        base,
        account,
        created,
        store,
        blobs,
        _dir: dir,
    }
}

async fn sqlite_world(rate: RateGate) -> World<sync_store_sqlite::SqliteBackend> {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = sync_server::open_store(&sync_server::Config::new(
        data,
        "127.0.0.1:0".parse().expect("an address"),
    ))
    .await
    .expect("the store opens");
    world_on(Arc::new(store), rate, dir).await
}

async fn limited(limits: RateLimits) -> World<sync_store_sqlite::SqliteBackend> {
    sqlite_world(RateGate::new(limits, None)).await
}

/// Asserts the refusal of §26: 429, `rate_limited` and a wait no longer
/// than the window.
fn assert_limited(reply: &Reply) {
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    let body = reply.error();
    assert_eq!(body.code.as_str(), "rate_limited");
    let wait = body.retry_after_seconds.expect("a wait");
    assert!((1..=3600).contains(&wait), "waits {wait} seconds");
    assert_eq!(reply.headers[RETRY_AFTER], wait.to_string().as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_over_its_pushes_waits_and_another_device_keeps_pushing() {
    let world = limited(RateLimits {
        pushes: hour(2),
        ..wide()
    })
    .await;
    let (first, digest) = world.push(0, 1, Digest::ZERO).await;
    let _: PushResponse = first.decode();
    let (second, digest) = world.push(0, 2, digest).await;
    let _: PushResponse = second.decode();
    assert_limited(&world.push(0, 3, digest).await.0);

    let (other, _) = world.push(1, 1, Digest::ZERO).await;
    let _: PushResponse = other.decode();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_bodies_count_in_bytes() {
    let world = limited(RateLimits {
        push_bytes: hour(1_000),
        ..wide()
    })
    .await;
    let (first, digest) = world.push(0, 1, Digest::ZERO).await;
    let _: PushResponse = first.decode();
    assert_limited(&world.push(0, 2, digest).await.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_count_their_responses_and_bootstrap_reads_have_their_own_allowance() {
    let world = limited(RateLimits {
        read_bytes: hour(3 * sync_server::rate::MIN_CHARGE),
        bootstrap_bytes: hour(100_000),
        ..wide()
    })
    .await;
    let client = world.client(0);
    let policy = format!("/v2/workspaces/{}/policy", world.workspace());

    // A large blob's range overdraws the bootstrap allowance once.
    let (ciphertext, blob_ref) = World::<sync_store_sqlite::SqliteBackend>::blob(300_000);
    let location = world.create_blob(0, &blob_ref).await;
    let patched = world.patch(0, &location, 0, &ciphertext).await;
    assert_eq!(patched.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        client.get(&location).await.status,
        StatusCode::PARTIAL_CONTENT
    );
    assert_limited(&client.get(&location).await);

    // Reads go on from their own allowance, until each small answer has
    // cost the least a read costs.
    for _ in 0..3 {
        assert_eq!(client.get(&policy).await.status, StatusCode::OK);
    }
    assert_limited(&client.get(&policy).await);

    // The other device's allowances are its own.
    assert_eq!(world.client(1).get(&policy).await.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_sync_requests_count_per_device_and_the_limits_route_never_does() {
    // Creating the workspace took the first request.
    let world = limited(RateLimits {
        requests: hour(3),
        ..wide()
    })
    .await;
    let client = world.client(0);
    for _ in 0..2 {
        assert_eq!(client.get("/v2/workspaces").await.status, StatusCode::OK);
    }
    assert_limited(&client.get("/v2/workspaces").await);
    for _ in 0..5 {
        assert_eq!(client.get("/v2/limits").await.status, StatusCode::OK);
    }
    assert_eq!(
        world.client(1).get("/v2/workspaces").await.status,
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_that_uploads_keeps_pushing_within_the_push_budget() {
    let world = limited(RateLimits {
        push_bytes: hour(64 * 1024),
        upload_bytes: hour(1_200_000),
        ..wide()
    })
    .await;
    let (ciphertext, blob_ref) = World::<sync_store_sqlite::SqliteBackend>::blob(2_500_000);
    let location = world.create_blob(0, &blob_ref).await;
    let mut chunks = ciphertext.chunks(1024 * 1024);

    // One segment fits the upload budget, and the next doesn't.
    let first = chunks.next().expect("a segment");
    let patched = world.patch(0, &location, 0, first).await;
    assert_eq!(patched.status(), StatusCode::NO_CONTENT);
    let refused = world
        .patch(
            0,
            &location,
            first.len() as u64,
            chunks.next().expect("a segment"),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()["Tus-Resumable"], "1.0.0");
    let body = refused.bytes().await.expect("a body");
    let error = sync_protocol::messages::ErrorBody::from_bytes(&body).expect("an error");
    assert_eq!(error.code.as_str(), "rate_limited");
    assert!(error.retry_after_seconds.is_some());

    // With its upload budget spent, the device's edits go through.
    let mut previous = Digest::ZERO;
    for counter in 1..=20 {
        let (reply, digest) = world.push(0, counter, previous).await;
        let _: PushResponse = reply.decode();
        previous = digest;
    }
}

/// `POST /v2/device/authorize` from a client that sets `header` to `value`.
async fn authorize(base: &str, header: (&str, &str)) -> Reply {
    let response = reqwest::Client::new()
        .post(format!("{base}/v2/device/authorize"))
        .header(header.0, header.1)
        .send()
        .await
        .expect("the server answers");
    Reply {
        status: response.status(),
        headers: response.headers().clone(),
        body: response.bytes().await.expect("a body").to_vec(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sign_in_counts_per_ip_from_the_trusted_header() {
    let world = sqlite_world(RateGate::new(
        RateLimits {
            sign_in: hour(2),
            ..wide()
        },
        Some(HeaderName::from_static("x-real-ip")),
    ))
    .await;
    for _ in 0..2 {
        let reply = authorize(&world.base, ("x-real-ip", "203.0.113.9")).await;
        assert_eq!(reply.status, StatusCode::OK);
    }
    assert_limited(&authorize(&world.base, ("x-real-ip", "203.0.113.9")).await);

    // Another client behind the proxy has its own count.
    let reply = authorize(&world.base, ("x-real-ip", "203.0.113.10")).await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_trusted_header_a_client_cannot_name_its_own_ip() {
    let world = limited(RateLimits {
        sign_in: hour(2),
        ..wide()
    })
    .await;
    for address in ["203.0.113.1", "203.0.113.2"] {
        let reply = authorize(&world.base, ("x-real-ip", address)).await;
        assert_eq!(reply.status, StatusCode::OK);
    }
    assert_limited(&authorize(&world.base, ("x-real-ip", "203.0.113.3")).await);
    assert_limited(&authorize(&world.base, ("x-forwarded-for", "203.0.113.4")).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_kind_of_sign_in_shares_the_count_and_pages_stay_readable() {
    let world = limited(RateLimits {
        sign_in: hour(1),
        ..wide()
    })
    .await;
    let http = reqwest::Client::new();
    let post = |path: &str| http.post(format!("{}{path}", world.base)).send();
    assert_ne!(
        post("/v2/device/token").await.expect("answers").status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    // A form gets a page, with the headers of every account page.
    let page = post("/account/email-link").await.expect("answers");
    assert_eq!(page.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(page.headers().contains_key(RETRY_AFTER));
    assert!(page.headers().contains_key(CONTENT_SECURITY_POLICY));
    assert!(
        page.text()
            .await
            .expect("a page")
            .contains("Too many sign-in attempts")
    );

    // A passkey ceremony gets JSON.
    let passkey = post("/account/passkeys/sign-in/start")
        .await
        .expect("answers");
    assert_eq!(passkey.status(), StatusCode::TOO_MANY_REQUESTS);
    let value: serde_json::Value =
        serde_json::from_slice(&passkey.bytes().await.expect("a body")).expect("JSON");
    assert_eq!(value["error"], "rate_limited");
    assert!(value["retry_after_seconds"].as_u64().is_some());

    // A reset code counts, through its route.
    let client = world.client(0);
    assert_limited(&client.post("/v2/account/reset-code", Vec::new()).await);
    // Reading a page isn't a sign-in request.
    let read = http
        .get(format!("{}/account/sign-in", world.base))
        .send()
        .await
        .expect("answers");
    assert_eq!(read.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_on_the_realtime_channel_cost_nothing() {
    let world = limited(RateLimits {
        read_bytes: hour(sync_server::rate::MIN_CHARGE),
        ..wide()
    })
    .await;
    let reader = world.client(1);
    let policy = format!("/v2/workspaces/{}/policy", world.workspace());
    assert_eq!(reader.get(&policy).await.status, StatusCode::OK);
    assert_limited(&reader.get(&policy).await);

    let url = format!(
        "{}/v2/workspaces/{}/realtime",
        world.base.replacen("http", "ws", 1),
        world.workspace()
    );
    let mut request = url.into_client_request().expect("a request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", world.account.token(1))
            .parse()
            .expect("a header"),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the channel opens");

    let (pushed, _) = world.push(0, 1, Digest::ZERO).await;
    let _: PushResponse = pushed.decode();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a frame in time")
            .expect("the channel stays open")
            .expect("a message");
        if let Message::Binary(bytes) = message
            && matches!(Frame::from_bytes(&bytes), Ok(Frame::Records { .. }))
        {
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_limits_hold_across_two_postgres_instances() {
    let database = Cluster::shared(Path::new(env!("CARGO_TARGET_TMPDIR")))
        .database()
        .await;
    let postgres = sync_server::Postgres {
        url: database.url.clone(),
        secret: Some(SecretKey::random().expect("random")),
    };
    sync_server::postgres::migrate(&postgres)
        .await
        .expect("migrated");
    let open = async || {
        Arc::new(
            sync_server::postgres::open_store(&postgres)
                .await
                .expect("the store opens"),
        )
    };
    let limits = RateLimits {
        pushes: hour(2),
        read_bytes: hour(2 * sync_server::rate::MIN_CHARGE),
        // Creating the workspace takes the first request.
        requests: hour(3),
        sign_in: hour(2),
        ..wide()
    };
    let gate = || RateGate::new(limits, Some(HeaderName::from_static("x-real-ip")));
    let dir = tempfile::tempdir().expect("temporary directory");
    let one = world_on(open().await, gate(), dir).await;
    let second = sync_server::spawn_limited(
        open().await,
        "127.0.0.1:0",
        Accounts::local,
        one.blobs.clone(),
        gate(),
    )
    .await
    .expect("serving");
    let two = format!("http://{second}");
    assert_ne!(one.store.restore_generation(), 0);

    // Each instance serves half of an allowance, and neither serves more.
    let (first, digest) = one.push(0, 1, Digest::ZERO).await;
    let _: PushResponse = first.decode();
    let op = one.created.op(&one.account.keys[0], 2, digest, |_| {});
    let path = format!("/v2/workspaces/{}/ops", one.workspace());
    let on_two = Client::new(two.clone(), Some(one.account.token(0)));
    let _: PushResponse = on_two
        .post(&path, push_body(std::slice::from_ref(&op)))
        .await
        .decode();
    let op = one.created.op(&one.account.keys[0], 3, op.digest(), |_| {});
    assert_limited(
        &one.client(0)
            .post(&path, push_body(std::slice::from_ref(&op)))
            .await,
    );
    assert_limited(&on_two.post(&path, push_body(&[op])).await);

    let policy = format!("/v2/workspaces/{}/policy", one.workspace());
    assert_eq!(one.client(0).get(&policy).await.status, StatusCode::OK);
    assert_eq!(on_two.get(&policy).await.status, StatusCode::OK);
    assert_limited(&one.client(0).get(&policy).await);
    assert_limited(&on_two.get(&policy).await);

    assert_eq!(
        one.client(0).get("/v2/workspaces").await.status,
        StatusCode::OK
    );
    assert_eq!(on_two.get("/v2/workspaces").await.status, StatusCode::OK);
    assert_limited(&one.client(0).get("/v2/workspaces").await);
    assert_limited(&on_two.get("/v2/workspaces").await);

    let client = ("x-real-ip", "203.0.113.9");
    assert_eq!(authorize(&one.base, client).await.status, StatusCode::OK);
    assert_eq!(authorize(&two, client).await.status, StatusCode::OK);
    assert_limited(&authorize(&one.base, client).await);
    assert_limited(&authorize(&two, client).await);
}

#[test]
fn the_binary_reads_its_limits_and_trusted_header_from_the_environment() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    assert!(support::run(&["migrate"], &data).status.success());
    let server = Server::start_with(
        Command::new(BIN)
            .arg("serve")
            .arg("--data")
            .arg(&data)
            .env("NOURA_RATE_SIGN_IN_PER_MINUTE", "1")
            .env("NOURA_TRUSTED_PROXY_HEADER", "x-real-ip"),
    );
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async {
        let base = server.base();
        let first = authorize(&base, ("x-real-ip", "203.0.113.1")).await;
        assert_eq!(first.status, StatusCode::OK);
        let second = authorize(&base, ("x-real-ip", "203.0.113.2")).await;
        assert_eq!(second.status, StatusCode::OK);
        // The count runs by the minute, so a retry may land in a new window.
        let mut limited = false;
        for _ in 0..3 {
            limited |= authorize(&base, ("x-real-ip", "203.0.113.1")).await.status
                == StatusCode::TOO_MANY_REQUESTS;
        }
        assert!(limited, "the second request from one address is refused");
    });
}

#[test]
fn the_binary_refuses_a_limit_or_header_it_cannot_read() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    assert!(support::run(&["migrate"], &data).status.success());
    for (name, value, says) in [
        ("NOURA_RATE_PUSHES_PER_SECOND", "many", "whole number"),
        ("NOURA_TRUSTED_PROXY_HEADER", "not a header", "header name"),
    ] {
        let output = Command::new(BIN)
            .arg("serve")
            .arg("--data")
            .arg(&data)
            .arg("--listen")
            .arg("127.0.0.1:0")
            .env(name, value)
            .output()
            .expect("the binary runs");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(says), "{stderr}");
    }
}

#[test]
fn the_defaults_are_the_ones_section_26_names() {
    const MIB: u64 = 1024 * 1024;
    let limits = RateLimits::default();
    let minute = Duration::from_secs(60);
    assert_eq!(
        limits.pushes,
        Limit {
            allowance: 10,
            window: Duration::from_secs(1)
        }
    );
    for (limit, allowance) in [
        (limits.push_bytes, 64 * MIB),
        (limits.upload_bytes, 512 * MIB),
        (limits.read_bytes, 128 * MIB),
        (limits.bootstrap_bytes, 512 * MIB),
        (limits.requests, 600),
        (limits.sign_in, 30),
    ] {
        assert_eq!(
            limit,
            Limit {
                allowance,
                window: minute
            }
        );
    }
}
