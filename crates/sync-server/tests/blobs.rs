//! The blob routes at seam 4 (§21): tus creation, resumable segment
//! uploads, digest checks, termination and range reads, over HTTP against a
//! disk blob store.

mod emulator;

use std::io;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use emulator::Emulator;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures::future::BoxFuture;
use reqwest::{Method, StatusCode};
use sync_protocol::blob::{SEGMENT_LEN, decrypt_blob, encrypt_blob};
use sync_protocol::codec::{Decode, Sorted};
use sync_protocol::messages::ErrorBody;
use sync_protocol::records::BlobRef;
use sync_protocol::types::{Id, SecretKey};
use sync_server::blobs::{DiskStore, SegmentStore};
use sync_store::conformance::World;
use sync_store::{Backend, INITIAL_RESTORE_GENERATION, ReadTxn, Store, UPLOAD_TTL};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

struct Fixture {
    world: World<SqliteBackend>,
    base: String,
    blobs: Arc<DiskStore>,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

async fn fixture(devices: usize) -> Fixture {
    fixture_with(devices, |blobs| blobs).await
}

/// A fixture whose server reaches its disk store through `wrap`.
async fn fixture_with<S: SegmentStore>(
    devices: usize,
    wrap: impl FnOnce(Arc<DiskStore>) -> Arc<S>,
) -> Fixture {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join(DATABASE_FILE);
    sync_store_sqlite::migrate(&path).await.expect("migrated");
    sync_store_sqlite::store_secret_check(&path, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    let store = Store::open(SqliteBackend::open(&path).await.expect("opened"))
        .await
        .expect("store");
    let world = World::new(store, devices).await;
    let blobs = Arc::new(DiskStore::open(&dir.path().join("blobs")).expect("blob store"));
    let address = sync_server::spawn_with_blobs(
        world.store.clone(),
        "127.0.0.1:0",
        sync_server::Accounts::local,
        wrap(blobs.clone()),
    )
    .await
    .expect("serving");
    Fixture {
        world,
        base: format!("http://{address}"),
        blobs,
        http: reqwest::Client::new(),
        _dir: dir,
    }
}

fn file(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 256) as u8).collect()
}

impl Fixture {
    fn request(&self, device: usize, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.world.devices[device].bearer())
            .header("Tus-Resumable", "1.0.0")
    }

    fn blobs_path(&self) -> String {
        format!(
            "/v2/workspaces/{}/blobs",
            self.world.workspace.to_base64url()
        )
    }

    async fn create(&self, device: usize, blob_ref: &BlobRef, domain: Id) -> reqwest::Response {
        let metadata = format!(
            "blob {},digest {},domain {}",
            STANDARD.encode(blob_ref.blob.0),
            STANDARD.encode(blob_ref.ciphertext_digest.0),
            STANDARD.encode(domain.0)
        );
        self.request(device, Method::POST, &self.blobs_path())
            .header("Upload-Length", blob_ref.ciphertext_length)
            .header("Upload-Metadata", metadata)
            .send()
            .await
            .expect("creation")
    }

    async fn patch(
        &self,
        device: usize,
        location: &str,
        offset: u64,
        bytes: Vec<u8>,
    ) -> reqwest::Response {
        self.request(device, Method::PATCH, location)
            .header("Upload-Offset", offset)
            .header("Content-Type", "application/offset+octet-stream")
            .body(bytes)
            .send()
            .await
            .expect("patch")
    }

    async fn offset(&self, device: usize, location: &str) -> u64 {
        let response = self
            .request(device, Method::HEAD, location)
            .send()
            .await
            .expect("head");
        assert_eq!(response.status(), StatusCode::OK);
        header(&response, "Upload-Offset").parse().expect("offset")
    }
}

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("{name} is missing"))
        .to_str()
        .expect("text")
        .to_owned()
}

async fn code(response: reqwest::Response) -> (StatusCode, String) {
    let status = response.status();
    let body = response.bytes().await.expect("body");
    let error = ErrorBody::from_bytes(&body).expect("an error body");
    (status, error.code.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_resume_complete_and_read_back_in_ranges() {
    let f = fixture(2).await;
    let options = f
        .request(0, Method::OPTIONS, &f.blobs_path())
        .send()
        .await
        .expect("options");
    assert_eq!(header(&options, "Tus-Version"), "1.0.0");
    assert_eq!(
        header(&options, "Tus-Extension"),
        "creation,termination,expiration"
    );
    assert_eq!(header(&options, "Tus-Max-Size"), "2215133237");

    let plaintext = file(2_600_000);
    let blob = Id::random().expect("random");
    let (ciphertext, blob_ref) =
        encrypt_blob(&plaintext, blob, SecretKey::random().expect("random")).expect("encrypts");
    let created = f.create(0, &blob_ref, f.world.domain).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert!(created.headers().contains_key("Upload-Expires"));
    let location = header(&created, "Location");
    assert_eq!(
        location,
        format!("{}/{}", f.blobs_path(), blob.to_base64url())
    );
    let retried = f.create(0, &blob_ref, f.world.domain).await;
    assert_eq!(
        retried.status(),
        StatusCode::CREATED,
        "a retry gets the original result"
    );
    let mut other = blob_ref.clone();
    other.ciphertext_digest = sync_protocol::types::Digest([3; 32]);
    assert_eq!(
        code(f.create(0, &other, f.world.domain).await).await,
        (StatusCode::CONFLICT, "id_conflict".to_owned())
    );

    // The first segment.
    let seg = SEGMENT_LEN;
    let response = f.patch(0, &location, 0, ciphertext[..seg].to_vec()).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&response, "Upload-Offset"), seg.to_string());
    // The segment is on disk, as its own object named by its hash.
    let keys = f.blobs.list().await.expect("list");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        sync_server::blobs::key_hash(&keys[0]),
        Some(sync_protocol::crypto::hash(&ciphertext[..seg]))
    );

    // A connection cut midway through the second segment changes nothing.
    {
        use tokio::io::AsyncWriteExt;
        let address = f.base.trim_start_matches("http://");
        let mut socket = tokio::net::TcpStream::connect(address)
            .await
            .expect("connects");
        let request = format!(
            "PATCH {location} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\n\
             Tus-Resumable: 1.0.0\r\nUpload-Offset: {seg}\r\n\
             Content-Type: application/offset+octet-stream\r\nContent-Length: {seg}\r\n\r\n",
            f.world.devices[0].bearer()
        );
        socket.write_all(request.as_bytes()).await.expect("headers");
        socket
            .write_all(&ciphertext[seg..seg + 1000])
            .await
            .expect("a part of the body");
        drop(socket);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        f.offset(0, &location).await,
        seg as u64,
        "resumes from the stored offset"
    );

    // A wrong offset conflicts, and a short segment that isn't the last is refused.
    assert_eq!(
        code(f.patch(0, &location, 0, ciphertext[..seg].to_vec()).await).await,
        (StatusCode::CONFLICT, "blob_offset_conflict".to_owned())
    );
    assert_eq!(
        code(
            f.patch(0, &location, seg as u64, ciphertext[seg..seg + 10].to_vec())
                .await
        )
        .await,
        (StatusCode::BAD_REQUEST, "bad_request".to_owned())
    );
    for (index, chunk) in ciphertext.chunks(seg).enumerate().skip(1) {
        let response = f
            .patch(0, &location, (index * seg) as u64, chunk.to_vec())
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    assert_eq!(f.offset(0, &location).await, ciphertext.len() as u64);

    // Only the uploader reads it before an operation lists it.
    let get = |device: usize, range: &str| {
        f.request(device, Method::GET, &location)
            .header("Range", range.to_owned())
            .send()
    };
    assert_eq!(
        get(1, "bytes=0-").await.expect("get").status(),
        StatusCode::NOT_FOUND
    );
    let listing = f.world.op(0, |o| {
        o.blobs = Sorted::new(vec![blob]).expect("sorted");
    });
    f.world.push(0, &[listing]).await.expect("listed");

    // Each response carries at most 1 MiB, and the ranges rebuild the blob.
    let mut downloaded = Vec::new();
    while downloaded.len() < ciphertext.len() {
        let response = get(1, &format!("bytes={}-", downloaded.len()))
            .await
            .expect("get");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let range = header(&response, "Content-Range");
        let body = response.bytes().await.expect("body");
        assert!(body.len() <= SEGMENT_LEN);
        assert_eq!(
            range,
            format!(
                "bytes {}-{}/{}",
                downloaded.len(),
                downloaded.len() + body.len() - 1,
                ciphertext.len()
            )
        );
        downloaded.extend_from_slice(&body);
    }
    assert_eq!(downloaded, ciphertext);
    assert_eq!(
        decrypt_blob(
            &downloaded,
            &blob_ref,
            Some(&sync_protocol::crypto::hash(&plaintext))
        )
        .expect("decrypts"),
        plaintext
    );
    // A range across a segment boundary.
    let response = get(1, "bytes=1048000-1049000").await.expect("get");
    assert_eq!(
        response.bytes().await.expect("body")[..],
        ciphertext[1_048_000..=1_049_000]
    );
    let past = get(1, &format!("bytes={}-", ciphertext.len()))
        .await
        .expect("get");
    assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatched_digests_and_abandoned_uploads_leave_nothing() {
    let f = fixture(1).await;
    let blob = Id::random().expect("random");
    let (mut ciphertext, blob_ref) =
        encrypt_blob(&file(1_100_000), blob, SecretKey::random().expect("random"))
            .expect("encrypts");
    let location = header(&f.create(0, &blob_ref, f.world.domain).await, "Location");
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 1;
    let seg = SEGMENT_LEN;
    f.patch(0, &location, 0, ciphertext[..seg].to_vec()).await;
    assert_eq!(
        code(
            f.patch(0, &location, seg as u64, ciphertext[seg..].to_vec())
                .await
        )
        .await,
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "blob_digest_mismatch".to_owned()
        )
    );
    assert!(
        f.blobs.list().await.expect("list").is_empty(),
        "the segments go"
    );
    let head = f
        .request(0, Method::HEAD, &location)
        .send()
        .await
        .expect("head");
    assert_eq!(head.status(), StatusCode::NOT_FOUND);

    // An abandoned upload deletes its segments.
    let location = header(&f.create(0, &blob_ref, f.world.domain).await, "Location");
    f.patch(0, &location, 0, ciphertext[..seg].to_vec()).await;
    assert_eq!(f.blobs.list().await.expect("list").len(), 1);
    let deleted = f
        .request(0, Method::DELETE, &location)
        .send()
        .await
        .expect("delete");
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(f.blobs.list().await.expect("list").is_empty());

    // Creation needs every metadata key, in standard base64.
    let bad = f
        .request(0, Method::POST, &f.blobs_path())
        .header("Upload-Length", blob_ref.ciphertext_length)
        .header(
            "Upload-Metadata",
            format!("blob {}", STANDARD.encode(blob.0)),
        )
        .send()
        .await
        .expect("creation");
    assert_eq!(
        code(bad).await,
        (StatusCode::BAD_REQUEST, "bad_request".to_owned())
    );
}

/// Uploads `plaintext` as a listed blob through the fixture's server and
/// returns its location, ciphertext and `blob_ref`.
async fn upload_listed(f: &Fixture, plaintext: &[u8]) -> (String, Vec<u8>, BlobRef) {
    let blob = Id::random().expect("random");
    let (ciphertext, blob_ref) =
        encrypt_blob(plaintext, blob, SecretKey::random().expect("random")).expect("encrypts");
    let location = header(&f.create(0, &blob_ref, f.world.domain).await, "Location");
    for (index, chunk) in ciphertext.chunks(SEGMENT_LEN).enumerate() {
        let response = f
            .patch(0, &location, (index * SEGMENT_LEN) as u64, chunk.to_vec())
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let listing = f.world.op(0, |o| {
        o.blobs = Sorted::new(vec![blob]).expect("sorted");
    });
    f.world.push(0, &[listing]).await.expect("listed");
    (location, ciphertext, blob_ref)
}

/// Reads a whole blob in ranges from the server at `base`.
async fn download(f: &Fixture, base: &str, location: &str, len: usize) -> Vec<u8> {
    let mut downloaded = Vec::new();
    while downloaded.len() < len {
        let response = f
            .http
            .get(format!("{base}{location}"))
            .bearer_auth(f.world.devices[0].bearer())
            .header("Range", format!("bytes={}-", downloaded.len()))
            .send()
            .await
            .expect("get");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        downloaded.extend_from_slice(&response.bytes().await.expect("body"));
    }
    downloaded
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blobs_migrate_to_s3_and_read_the_same_afterwards() {
    let f = fixture(1).await;
    let plaintext = file(2_200_000);
    let (location, ciphertext, blob_ref) = upload_listed(&f, &plaintext).await;
    let emulator = Emulator::start().await;
    let data = f._dir.path().to_owned();
    let endpoint = emulator.endpoint();
    let migrate = move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_sync-server"))
            .args(["blobs", "migrate-to-s3", "--data"])
            .arg(&data)
            .env("NOURA_S3_BUCKET", emulator::BUCKET)
            .env("NOURA_S3_ENDPOINT", &endpoint)
            .env("AWS_ACCESS_KEY_ID", emulator::ACCESS_KEY)
            .env("AWS_SECRET_ACCESS_KEY", emulator::SECRET_KEY)
            .env("AWS_REGION", "us-east-1")
            .output()
            .expect("the binary runs")
    };
    let output = tokio::task::spawn_blocking(migrate.clone())
        .await
        .expect("joins");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        f.blobs.list().await.expect("list").is_empty(),
        "the local copies go"
    );
    assert_eq!(emulator.store().list().await.expect("list").len(), 3);
    let rerun = tokio::task::spawn_blocking(migrate).await.expect("joins");
    assert!(rerun.status.success(), "a rerun has nothing left to do");

    // A server on the bucket serves the same bytes.
    let address = sync_server::spawn_with_blobs(
        f.world.store.clone(),
        "127.0.0.1:0",
        sync_server::Accounts::local,
        Arc::new(emulator.store()),
    )
    .await
    .expect("serving");
    let downloaded = download(
        &f,
        &format!("http://{address}"),
        &location,
        ciphertext.len(),
    )
    .await;
    assert_eq!(downloaded, ciphertext);
    assert_eq!(
        decrypt_blob(&downloaded, &blob_ref, None).expect("decrypts"),
        plaintext
    );
}

/// Holds the first two `put`s until both arrive, so two `PATCH`es pass the
/// offset check before either records its segment.
struct MeetingPuts {
    inner: Arc<DiskStore>,
    gate: tokio::sync::Barrier,
    puts: AtomicUsize,
}

impl MeetingPuts {
    fn around(inner: Arc<DiskStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            gate: tokio::sync::Barrier::new(2),
            puts: AtomicUsize::new(0),
        })
    }
}

impl SegmentStore for MeetingPuts {
    fn put(&self, key: &str, bytes: Bytes) -> BoxFuture<'_, io::Result<()>> {
        let key = key.to_owned();
        Box::pin(async move {
            if self.puts.fetch_add(1, Ordering::SeqCst) < 2 {
                self.gate.wait().await;
            }
            self.inner.put(&key, bytes).await
        })
    }
    fn get(&self, key: &str, range: Range<u64>) -> BoxFuture<'_, io::Result<Bytes>> {
        self.inner.get(key, range)
    }
    fn size(&self, key: &str) -> BoxFuture<'_, io::Result<u64>> {
        self.inner.size(key)
    }
    fn delete(&self, key: &str) -> BoxFuture<'_, io::Result<()>> {
        self.inner.delete(key)
    }
    fn list(&self) -> BoxFuture<'_, io::Result<Vec<String>>> {
        self.inner.list()
    }
}

/// Two `PATCH`es at one offset race: `first` and `second` are their bodies.
/// Returns the fixture, the upload, its plaintext's ciphertext, and the
/// location, with one `PATCH` answered 204 and the other 409.
async fn racing_patches(
    first: impl FnOnce(&[u8]) -> Vec<u8>,
    second: impl FnOnce(&[u8]) -> Vec<u8>,
) -> (Fixture, Vec<u8>, String) {
    let f = fixture_with(1, MeetingPuts::around).await;
    let blob = Id::random().expect("random");
    let (ciphertext, blob_ref) =
        encrypt_blob(&file(2_600_000), blob, SecretKey::random().expect("random"))
            .expect("encrypts");
    let created = f.create(0, &blob_ref, f.world.domain).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let location = header(&created, "Location");
    let (a, b) = tokio::join!(
        f.patch(0, &location, 0, first(&ciphertext)),
        f.patch(0, &location, 0, second(&ciphertext)),
    );
    let mut statuses = [a.status(), b.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::NO_CONTENT, StatusCode::CONFLICT]);
    assert_eq!(f.offset(0, &location).await, SEGMENT_LEN as u64);
    (f, ciphertext, location)
}

/// Segment keys are content-addressed, so a losing `PATCH` of the same
/// bytes names the winner's object and leaves it alone: the upload finishes
/// and reads back whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_losing_patch_of_the_same_bytes_keeps_the_winners_segment() {
    let segment = |ciphertext: &[u8]| ciphertext[..SEGMENT_LEN].to_vec();
    let (f, ciphertext, location) = racing_patches(segment, segment).await;
    let keys = f.blobs.list().await.expect("list");
    assert_eq!(keys.len(), 1, "the recorded segment's object stays");
    for (index, chunk) in ciphertext.chunks(SEGMENT_LEN).enumerate().skip(1) {
        let response = f
            .patch(0, &location, (index * SEGMENT_LEN) as u64, chunk.to_vec())
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let mut read = Vec::new();
    while read.len() < ciphertext.len() {
        let response = f
            .request(0, Method::GET, &location)
            .header("Range", format!("bytes={}-", read.len()))
            .send()
            .await
            .expect("a read");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        read.extend_from_slice(&response.bytes().await.expect("bytes"));
    }
    assert_eq!(read, ciphertext);
}

/// A losing `PATCH` of other bytes stranded its own object, which goes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_losing_patch_of_other_bytes_deletes_its_own_object() {
    let (f, _, _) = racing_patches(
        |ciphertext| ciphertext[..SEGMENT_LEN].to_vec(),
        |ciphertext| ciphertext[SEGMENT_LEN..2 * SEGMENT_LEN].to_vec(),
    )
    .await;
    assert_eq!(f.blobs.list().await.expect("list").len(), 1);
}

/// A disk store whose deletes fail while `broken`.
struct BrokenDeletes {
    inner: Arc<DiskStore>,
    broken: AtomicBool,
}

impl SegmentStore for BrokenDeletes {
    fn put(&self, key: &str, bytes: Bytes) -> BoxFuture<'_, io::Result<()>> {
        self.inner.put(key, bytes)
    }
    fn get(&self, key: &str, range: Range<u64>) -> BoxFuture<'_, io::Result<Bytes>> {
        self.inner.get(key, range)
    }
    fn size(&self, key: &str) -> BoxFuture<'_, io::Result<u64>> {
        self.inner.size(key)
    }
    fn delete(&self, key: &str) -> BoxFuture<'_, io::Result<()>> {
        if self.broken.load(Ordering::SeqCst) {
            return Box::pin(async { Err(io::Error::other("the store is down")) });
        }
        self.inner.delete(key)
    }
    fn list(&self) -> BoxFuture<'_, io::Result<Vec<String>>> {
        self.inner.list()
    }
}

/// The collector end to end (§19): a server pass takes an expired upload's
/// objects from the store and then its row. A failed store delete leaves a
/// marked row that a later pass finishes, and the blob reads as gone
/// meanwhile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_maintenance_pass_collects_an_expired_upload_and_finishes_after_a_failed_delete() {
    let f = fixture(1).await;
    let store = BrokenDeletes {
        inner: f.blobs.clone(),
        broken: AtomicBool::new(true),
    };
    let world = &f.world;
    let (workspace, session) = (world.workspace, world.devices[0].session);

    let blob = Id::random().expect("random");
    let (ciphertext, blob_ref) =
        encrypt_blob(&file(2_600_000), blob, SecretKey::random().expect("random"))
            .expect("encrypts");
    let location = header(&f.create(0, &blob_ref, world.domain).await, "Location");
    for (index, chunk) in ciphertext.chunks(SEGMENT_LEN).enumerate() {
        let response = f
            .patch(0, &location, (index * SEGMENT_LEN) as u64, chunk.to_vec())
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let segments = f.blobs.list().await.expect("list").len();
    assert_eq!(segments, ciphertext.chunks(SEGMENT_LEN).count());

    // Inside its 24 hours a pass leaves it.
    sync_server::maintenance::run_once(&world.store, &store)
        .await
        .expect("a pass");
    assert_eq!(f.offset(0, &location).await, ciphertext.len() as u64);

    world
        .store
        .clock()
        .advance(Duration::from_secs(UPLOAD_TTL + 1));
    let row = async || {
        world
            .store
            .backend()
            .read(Box::new(
                move |txn: &mut <SqliteBackend as Backend>::Read| {
                    Box::pin(async move { txn.blob(workspace, blob).await })
                },
            ))
            .await
            .expect("read")
    };

    // The row is marked, and the objects outlive the failed deletes.
    sync_server::maintenance::run_once(&world.store, &store)
        .await
        .expect("a pass");
    assert!(row().await.expect("a row").marked);
    assert_eq!(f.blobs.list().await.expect("list").len(), segments);
    let head = f
        .request(0, Method::HEAD, &location)
        .send()
        .await
        .expect("head");
    assert_eq!(head.status(), StatusCode::NOT_FOUND);
    assert!(
        world
            .store
            .blob_upload(session, workspace, blob)
            .await
            .is_err()
    );

    // The next pass finishes: objects first, then the row.
    store.broken.store(false, Ordering::SeqCst);
    sync_server::maintenance::run_once(&world.store, &store)
        .await
        .expect("a pass");
    assert!(f.blobs.list().await.expect("list").is_empty());
    assert_eq!(row().await, None);
    assert!(
        world
            .store
            .collectable_blobs()
            .await
            .expect("due")
            .is_empty()
    );
}
