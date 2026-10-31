//! `backup`, `restore` and `mark-restored` at seam 4 (§14, §19): the stamp,
//! the generation, the secret, the lease and the check against the blob
//! store, on a real data directory with the real binary.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use bytes::Bytes;
use sqlx::{ConnectOptions, Connection};
use support::{BIN, Client, Server, push_body, run, seed};
use sync_protocol::messages::{MissingBlobsResponse, PushResponse};
use sync_protocol::types::{Digest, Id, to_base64url};
use sync_server::blobs::{DiskStore, SegmentStore, segment_key};
use sync_server::restore::{QUARANTINE, check_blobs, collect_quarantine, secret_beside, unix_now};

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn command(args: &[&str], data: &Path) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .arg("--data")
        .arg(data)
        .output()
        .expect("runs")
}

fn text(path: &Path) -> &str {
    path.to_str().expect("a path")
}

async fn database(path: &Path) -> sqlx::SqliteConnection {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .connect()
        .await
        .expect("the database opens")
}

async fn generation(path: &Path) -> u64 {
    let mut connection = database(path).await;
    let (generation,): (i64,) = sqlx::query_as("SELECT restore_generation FROM server")
        .fetch_one(&mut connection)
        .await
        .expect("the server row");
    connection.close().await.expect("closed");
    generation as u64
}

fn db_file(data: &Path) -> PathBuf {
    data.join(sync_store_sqlite::DATABASE_FILE)
}

fn fresh() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    assert!(run(&["migrate"], &data).status.success());
    (root, data)
}

fn workspace_id() -> Id {
    Id([7; 16])
}

/// A blob row the database knows, with its one segment stored or not.
struct Row {
    blob: Id,
    bytes: Vec<u8>,
    hash: Digest,
}

impl Row {
    fn new(byte: u8) -> Self {
        let bytes = vec![byte; 4096];
        Self {
            blob: Id([byte; 16]),
            hash: sync_protocol::crypto::hash(&bytes),
            bytes,
        }
    }

    fn key(&self) -> String {
        segment_key(&workspace_id(), &self.blob, 0, &self.hash)
    }

    async fn store(&self, blobs: &DiskStore) {
        blobs
            .put(&self.key(), Bytes::from(self.bytes.clone()))
            .await
            .expect("stored");
    }

    /// Writes the blob's rows, as a finished upload that `listed` says an
    /// operation lists.
    async fn insert(&self, database_path: &Path, listed: bool) {
        let mut connection = database(database_path).await;
        let workspace = &workspace_id().0[..];
        let domain = &[9u8; 16][..];
        sqlx::query(
            "INSERT INTO blobs (workspace, blob, domain, device, length, digest, upload_offset,
                                complete, delete_after, marked, live_listings)
             VALUES (?1, ?2, ?3, ?3, 4096, ?4, 4096, 1, ?5, 0, 0)",
        )
        .bind(workspace)
        .bind(&self.blob.0[..])
        .bind(domain)
        .bind(&self.hash.0[..])
        .bind(if listed { None } else { Some(1_i64) })
        .execute(&mut connection)
        .await
        .expect("the blob row");
        sqlx::query(
            "INSERT INTO blob_segments (workspace, blob, idx, hash) VALUES (?1, ?2, 0, ?3)",
        )
        .bind(workspace)
        .bind(&self.blob.0[..])
        .bind(&self.hash.0[..])
        .execute(&mut connection)
        .await
        .expect("the segment row");
        if listed {
            sqlx::query(
                "INSERT INTO blob_listings (workspace, blob, domain, seq) VALUES (?1, ?2, ?3, 1)",
            )
            .bind(workspace)
            .bind(&self.blob.0[..])
            .bind(domain)
            .execute(&mut connection)
            .await
            .expect("the listing");
        }
        connection.close().await.expect("closed");
    }
}

async fn blob_state(data: &Path, blob: Id) -> (i64, i64, Option<i64>) {
    let mut connection = database(&db_file(data)).await;
    let row: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT upload_offset, complete, delete_after FROM blobs WHERE workspace = ?1 AND blob = ?2",
    )
    .bind(&workspace_id().0[..])
    .bind(&blob.0[..])
    .fetch_one(&mut connection)
    .await
    .expect("the row");
    connection.close().await.expect("closed");
    row
}

fn disk(data: &Path) -> DiskStore {
    DiskStore::open(&data.join("blobs")).expect("the blob store")
}

/// `backup --to` with a bare file name writes into the working directory,
/// and a secret already there stops it before the copy, with nothing left
/// behind, so the rerun works.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_takes_a_bare_file_name_and_leaves_nothing_when_it_fails() {
    let (root, data) = fresh();
    let backup = || {
        Command::new(BIN)
            .args(["backup", "--to", "b.sqlite3", "--data"])
            .arg(&data)
            .current_dir(root.path())
            .output()
            .expect("runs")
    };
    let secret = root.path().join("b.sqlite3.secret");
    let copy = root.path().join("b.sqlite3");
    std::fs::write(&secret, b"in the way").expect("written");
    let refused = backup();
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("b.sqlite3.secret"),
        "{}",
        stderr(&refused)
    );
    assert!(!root.path().join("b.sqlite3.partial").exists());
    assert!(!copy.exists());

/// A database too damaged to read holds no lease anyone can see, so
/// `restore` keeps it under a side name and takes the backup in its place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_sets_a_damaged_database_aside() {
    let (root, data) = fresh();
    let copy = root.path().join("copy.sqlite3");
    let output = command(&["backup", "--to", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    let before = generation(&db_file(&data)).await;
    std::fs::write(db_file(&data), vec![0x5a; 8192]).expect("damaged");

    let output = command(&["restore", "--from", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(generation(&db_file(&data)).await > before);
    let side: Vec<_> = std::fs::read_dir(&data)
        .expect("listed")
        .map(|entry| entry.expect("an entry").file_name())
        .filter(|name| name.to_string_lossy().contains(".damaged-"))
        .collect();
    assert_eq!(side.len(), 1, "{side:?}");
    assert_eq!(
        std::fs::read(data.join(&side[0])).expect("kept"),
        vec![0x5a; 8192]
    );
    assert!(stderr(&output).contains(".damaged-"), "{}", stderr(&output));
    let server = Server::start(&data);
    assert!(
        Client::new(server.base(), None)
            .get("/v2/limits")
            .await
            .status
            .is_success()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_starts_the_generation_at_the_current_time() {
    let before = unix_now();
    let (_root, data) = fresh();
    let started = generation(&db_file(&data)).await;
    assert!(started >= before && started <= unix_now());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_copies_a_serving_database_and_serve_refuses_the_copy() {
    let (root, data) = fresh();
    let account = seed(&data, 1).await;
    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(0)));
    let created = account.creation();
    client.post("/v2/workspaces", created.body.clone()).await;
    let copy = root.path().join("copy.sqlite3");

    // Pushes keep landing while the copy runs.
    let writer = {
        let base = server.base();
        let token = account.token(0);
        let op_keys = account.keys[0].clone();
        let workspace = created.workspace;
        let mut ops = Vec::new();
        let mut previous = Digest::ZERO;
        for counter in 1..=40 {
            let op = created.op(&op_keys, counter, previous, |_| {});
            previous = op.digest();
            ops.push(op);
        }
        tokio::spawn(async move {
            let client = Client::new(base, Some(token));
            for op in ops {
                let reply = client
                    .post(
                        &format!("/v2/workspaces/{}/ops", to_base64url(&workspace.0)),
                        push_body(&[op]),
                    )
                    .await;
                reply.decode::<PushResponse>();
            }
        })
    };
    let output = command(&["backup", "--to", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    writer.await.expect("every push lands");

    let secret = std::fs::read(secret_beside(&copy)).expect("the secret beside the copy");
    assert_eq!(
        secret,
        std::fs::read(data.join("server-secret")).expect("secret")
    );
    let state = sync_store_sqlite::inspect(&copy).await.expect("inspects");
    assert!(state.stamped, "the copy carries the stamp");

    // A second backup never overwrites the first.
    let again = command(&["backup", "--to", text(&copy)], &data);
    assert!(!again.status.success());

    // `serve` refuses a data directory that holds the copy as it stands.
    let other = root.path().join("other");
    std::fs::create_dir_all(&other).expect("a directory");
    std::fs::copy(&copy, db_file(&other)).expect("a file copy");
    std::fs::write(other.join("server-secret"), &secret).expect("the secret");
    let refused = Command::new(BIN)
        .arg("serve")
        .arg("--data")
        .arg(&other)
        .output()
        .expect("runs");
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("backup copy"),
        "{}",
        stderr(&refused)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_ahead_log_grows_during_the_copy_and_shrinks_after_it() {
    let (root, data) = fresh();
    let path = db_file(&data);
    let wal = {
        let mut name = path.as_os_str().to_owned();
        name.push("-wal");
        PathBuf::from(name)
    };
    // A second connection writes while a read transaction stands in for
    // the copy's snapshot, which no checkpoint can pass.
    let mut reader = database(&path).await;
    let mut writer = database(&path).await;
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(&mut writer)
        .await
        .expect("WAL");
    sqlx::query("CREATE TABLE IF NOT EXISTS padding (bytes BLOB)")
        .execute(&mut writer)
        .await
        .expect("a table");
    let mut snapshot = reader.begin().await.expect("a read");
    sqlx::query("SELECT COUNT(*) FROM server")
        .execute(&mut *snapshot)
        .await
        .expect("the snapshot");
    for _ in 0..64 {
        sqlx::query("INSERT INTO padding VALUES (zeroblob(65536))")
            .execute(&mut writer)
            .await
            .expect("a write");
    }
    sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
        .execute(&mut writer)
        .await
        .expect("a checkpoint that can't pass the snapshot");
    let grown = std::fs::metadata(&wal).expect("the log").len();
    assert!(grown > 4 * 1024 * 1024, "the log grew to {grown}");
    snapshot.rollback().await.expect("ended");
    reader.close().await.expect("closed");

    let copy = root.path().join("copy.sqlite3");
    let output = command(&["backup", "--to", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    let after = std::fs::metadata(&wal).map_or(0, |m| m.len());
    assert!(after < grown / 4, "the log stayed at {after} of {grown}");
    writer.close().await.expect("closed");
}

#[test]
fn backup_in_postgres_mode_points_to_the_database_tools() {
    let root = tempfile::tempdir().expect("temporary directory");
    let output = Command::new(BIN)
        .args([
            "backup",
            "--to",
            "x",
            "--database-url",
            "postgres://nowhere/x",
        ])
        .arg("--data")
        .arg(root.path())
        .env("NOURA_SERVER_SECRET", "00".repeat(32))
        .output()
        .expect("runs");
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("pg_dump"), "{message}");
    assert!(message.contains("keep the server secret"), "{message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_takes_a_backup_into_a_fresh_or_the_old_directory() {
    let (root, data) = fresh();
    let copy = root.path().join("copy.sqlite3");
    assert!(
        command(&["backup", "--to", text(&copy)], &data)
            .status
            .success()
    );
    let stored = generation(&copy).await;

    // A fresh directory, with the secret the backup wrote.
    let fresh_dir = root.path().join("fresh");
    let output = command(&["restore", "--from", text(&copy)], &fresh_dir);
    assert!(output.status.success(), "{}", stderr(&output));
    let restored = generation(&db_file(&fresh_dir)).await;
    assert!(restored > stored);
    assert_eq!(
        std::fs::read(fresh_dir.join("server-secret")).expect("secret"),
        std::fs::read(data.join("server-secret")).expect("secret")
    );
    assert!(
        !sync_store_sqlite::inspect(&db_file(&fresh_dir))
            .await
            .expect("inspects")
            .stamped
    );
    let server = Server::start(&fresh_dir);
    let reply = Client::new(server.base(), None).get("/v2/limits").await;
    assert_eq!(reply.generation(), Some(restored.to_string().as_str()));
    drop(server);

    // The old directory, whose write-ahead log goes.
    let wal = {
        let mut name = db_file(&data).into_os_string();
        name.push("-wal");
        PathBuf::from(name)
    };
    std::fs::write(&wal, b"stale").expect("a stale log");
    let output = command(&["restore", "--from", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!wal.exists());
    // A second restore of the same backup never repeats a generation, even
    // within the same second.
    let first = generation(&db_file(&data)).await;
    let output = command(&["restore", "--from", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(generation(&db_file(&data)).await > first);
}

#[test]
fn restore_refuses_a_missing_or_different_secret() {
    let (root, data) = fresh();
    let copy = root.path().join("copy.sqlite3");
    assert!(
        command(&["backup", "--to", text(&copy)], &data)
            .status
            .success()
    );
    let secret = secret_beside(&copy);
    let held = std::fs::read(&secret).expect("secret");
    let target = root.path().join("target");

    std::fs::remove_file(&secret).expect("removed");
    let missing = command(&["restore", "--from", text(&copy)], &target);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("no server secret"));

    std::fs::write(&secret, [0xee; 32]).expect("another secret");
    let different = command(&["restore", "--from", text(&copy)], &target);
    assert!(!different.status.success());
    assert!(stderr(&different).contains("doesn't match"));
    assert!(!db_file(&target).exists());

    // The right secret from anywhere works, in raw or hexadecimal form.
    let elsewhere = root.path().join("kept-secret");
    std::fs::write(&elsewhere, sync_protocol::types::to_hex(&held)).expect("written");
    let output = command(
        &[
            "restore",
            "--from",
            text(&copy),
            "--secret",
            text(&elsewhere),
        ],
        &target,
    );
    assert!(output.status.success(), "{}", stderr(&output));

    // A directory that holds another secret stays as it is.
    let other = root.path().join("other");
    assert!(run(&["migrate"], &other).status.success());
    let mismatch = command(
        &[
            "restore",
            "--from",
            text(&copy),
            "--secret",
            text(&elsewhere),
        ],
        &other,
    );
    assert!(!mismatch.status.success());
    assert!(stderr(&mismatch).contains("another server secret"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_restored_raises_the_generation_and_refuses_a_bad_secret() {
    let (_root, data) = fresh();
    let before = generation(&db_file(&data)).await;
    let output = command(&["mark-restored"], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    let raised = generation(&db_file(&data)).await;
    assert!(raised > before);

    let path = data.join("server-secret");
    let secret = std::fs::read(&path).expect("secret");
    std::fs::write(&path, [0xee; 32]).expect("another secret");
    let different = command(&["mark-restored"], &data);
    assert!(!different.status.success());
    std::fs::remove_file(&path).expect("removed");
    let missing = command(&["mark-restored"], &data);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("no server secret"));
    std::fs::write(&path, secret).expect("restored");
    assert_eq!(generation(&db_file(&data)).await, raised);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_and_mark_restored_refuse_a_running_server() {
    let (root, data) = fresh();
    let copy = root.path().join("copy.sqlite3");
    assert!(
        command(&["backup", "--to", text(&copy)], &data)
            .status
            .success()
    );
    let server = Server::start(&data);
    let marked = command(&["mark-restored"], &data);
    assert!(!marked.status.success());
    assert!(
        stderr(&marked).contains("stop `serve`"),
        "{}",
        stderr(&marked)
    );
    let restored = command(&["restore", "--from", text(&copy)], &data);
    assert!(!restored.status.success());
    assert!(stderr(&restored).contains("stop `serve`"));
    drop(server);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_restore_pass_checks_the_blob_store_against_the_database() {
    let (_root, data) = fresh();
    let path = db_file(&data);
    let blobs = disk(&data);

    let unknown = Row::new(1);
    unknown.store(&blobs).await;
    let unlisted = Row::new(2);
    unlisted.store(&blobs).await;
    unlisted.insert(&path, false).await;
    let lost = Row::new(3);
    lost.insert(&path, true).await;
    let kept = Row::new(4);
    kept.store(&blobs).await;
    kept.insert(&path, true).await;

    let now = unix_now();
    let output = command(&["mark-restored"], &data);
    assert!(output.status.success(), "{}", stderr(&output));

    // The unknown object sits under the quarantine prefix.
    let quarantined: Vec<String> = blobs
        .list_under(QUARANTINE)
        .await
        .expect("listed")
        .into_iter()
        .collect();
    assert_eq!(quarantined.len(), 1);
    assert!(quarantined[0].ends_with(&unknown.key()));
    assert!(blobs.size(&unknown.key()).await.is_err());

    // The completed blob no operation lists gets a fresh 24 hours.
    let (_, complete, delete_after) = blob_state(&data, unlisted.blob).await;
    assert_eq!(complete, 1);
    assert!(delete_after.expect("a time") >= (now + 24 * 3600) as i64);

    // The listed blob whose object went is an upload at offset 0.
    assert_eq!(blob_state(&data, lost.blob).await, (0, 0, None));
    // The whole one stays.
    assert_eq!(blob_state(&data, kept.blob).await, (4096, 1, None));
    assert!(blobs.size(&kept.key()).await.is_ok());

    // A second pass leaves the quarantine alone.
    assert!(command(&["mark-restored"], &data).status.success());
    assert_eq!(
        blobs.list_under(QUARANTINE).await.expect("listed").len(),
        1,
        "the pass skips the quarantine prefix"
    );

    // Seven days later, and no sooner, the collector deletes it.
    assert_eq!(
        collect_quarantine(&blobs, now + 6 * 24 * 3600)
            .await
            .expect("collected"),
        0
    );
    assert_eq!(
        collect_quarantine(&blobs, now + 8 * 24 * 3600)
            .await
            .expect("collected"),
        1
    );
    assert!(
        blobs
            .list_under(QUARANTINE)
            .await
            .expect("listed")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_right_backup_after_a_wrong_one_brings_its_blobs_back() {
    let (root, data) = fresh();
    let wrong = root.path().join("wrong.sqlite3");
    assert!(
        command(&["backup", "--to", text(&wrong)], &data)
            .status
            .success()
    );

    let blobs = disk(&data);
    let row = Row::new(5);
    row.store(&blobs).await;
    row.insert(&db_file(&data), true).await;
    let right = root.path().join("right.sqlite3");
    assert!(
        command(&["backup", "--to", text(&right)], &data)
            .status
            .success()
    );

    assert!(
        command(&["restore", "--from", text(&wrong)], &data)
            .status
            .success()
    );
    assert!(
        blobs.size(&row.key()).await.is_err(),
        "the wrong backup lacks it"
    );
    assert_eq!(blobs.list_under(QUARANTINE).await.expect("listed").len(), 1);

    assert!(
        command(&["restore", "--from", text(&right)], &data)
            .status
            .success()
    );
    assert!(blobs.size(&row.key()).await.is_ok(), "the object came back");
    assert!(
        blobs
            .list_under(QUARANTINE)
            .await
            .expect("listed")
            .is_empty()
    );
    assert_eq!(blob_state(&data, row.blob).await, (4096, 1, None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_blob_shows_in_the_route_and_any_writer_can_upload_it() {
    let (_root, data) = fresh();
    let account = seed(&data, 2).await;
    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(0)));
    let created = account.creation();
    let reply = client.post("/v2/workspaces", created.body.clone()).await;
    assert!(reply.status.is_success());
    drop(server);

    let lost = Row::new(6);
    lost.insert(&db_file(&data), true).await;
    let mut connection = database(&db_file(&data)).await;
    // The killed server's lease would last 90 seconds.
    sqlx::query("DELETE FROM instances")
        .execute(&mut connection)
        .await
        .expect("no lease");
    sqlx::query("UPDATE blobs SET workspace = ?1, domain = ?3 WHERE blob = ?2")
        .bind(&created.workspace.0[..])
        .bind(&lost.blob.0[..])
        .bind(&created.domain.0[..])
        .execute(&mut connection)
        .await
        .expect("moved");
    sqlx::query("UPDATE blob_segments SET workspace = ?1 WHERE blob = ?2")
        .bind(&created.workspace.0[..])
        .bind(&lost.blob.0[..])
        .execute(&mut connection)
        .await
        .expect("moved");
    sqlx::query("UPDATE blob_listings SET workspace = ?1, domain = ?3 WHERE blob = ?2")
        .bind(&created.workspace.0[..])
        .bind(&lost.blob.0[..])
        .bind(&created.domain.0[..])
        .execute(&mut connection)
        .await
        .expect("moved");
    connection.close().await.expect("closed");
    assert!(command(&["mark-restored"], &data).status.success());

    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(1)));
    let route = format!(
        "/v2/workspaces/{}/missing-blobs",
        to_base64url(&created.workspace.0)
    );
    let missing: MissingBlobsResponse = client.get(&route).await.decode();
    assert_eq!(missing.blobs.0, vec![lost.blob]);
}

type Fut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// A store whose listing misses what a running server stored after it.
struct Stale(DiskStore);

impl SegmentStore for Stale {
    fn put(&self, key: &str, bytes: Bytes) -> Fut<'_, std::io::Result<()>> {
        self.0.put(key, bytes)
    }
    fn get(&self, key: &str, range: std::ops::Range<u64>) -> Fut<'_, std::io::Result<Bytes>> {
        self.0.get(key, range)
    }
    fn size(&self, key: &str) -> Fut<'_, std::io::Result<u64>> {
        self.0.size(key)
    }
    fn delete(&self, key: &str) -> Fut<'_, std::io::Result<()>> {
        self.0.delete(key)
    }
    fn list(&self) -> Fut<'_, std::io::Result<Vec<String>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn list_under(&self, prefix: &str) -> Fut<'_, std::io::Result<Vec<String>>> {
        self.0.list_under(prefix)
    }
}

/// A server started during the pass stores a blob the pass's listing
/// doesn't hold: the pass looks at the store again, and a blob whose
/// segments changed since it read them stays as it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pass_leaves_a_blob_a_running_server_took() {
    let (_root, data) = fresh();
    let path = db_file(&data);
    let blobs = disk(&data);
    let taken = Row::new(5);
    taken.store(&blobs).await;
    taken.insert(&path, true).await;
    let lost = Row::new(6);
    lost.insert(&path, true).await;

    let backend = sync_store_sqlite::SqliteBackend::open(&path)
        .await
        .expect("the database");
    let store = sync_store::Store::open(backend).await.expect("the store");
    let check = check_blobs(&store, &Stale(disk(&data)), Vec::new())
        .await
        .expect("the pass");
    assert_eq!(check.missing, 1, "only the blob without its object");
    assert_eq!(blob_state(&data, taken.blob).await.0, 4096);
    assert_eq!(blob_state(&data, lost.blob).await.0, 0);

    let changed = store
        .lose_blob(workspace_id(), Id([9; 16]), taken.blob, Vec::new())
        .await
        .expect("the step");
    assert_eq!(
        changed, None,
        "the segments moved since the caller read them"
    );
    assert_eq!(blob_state(&data, taken.blob).await.0, 4096);
    store.backend().close().await;
}

/// `serve` starts as soon as the command says the generation rose, while
/// the pass still works through a large store, which it finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_may_start_while_the_pass_runs_over_a_large_store() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let (_root, data) = fresh();
    let mut connection = database(&db_file(&data)).await;
    let domain = &[9u8; 16][..];
    let hash = sync_protocol::crypto::hash(b"gone");
    let count: u16 = 4_000;
    let mut transaction = sqlx::Connection::begin(&mut connection)
        .await
        .expect("a transaction");
    for number in 0..count {
        let mut blob = [0u8; 16];
        blob[..2].copy_from_slice(&number.to_be_bytes());
        for statement in [
            "INSERT INTO blobs (workspace, blob, domain, device, length, digest, upload_offset,
                                complete, delete_after, marked, live_listings)
             VALUES (?1, ?2, ?3, ?3, 4096, ?4, 4096, 1, NULL, 0, 0)",
            "INSERT INTO blob_segments (workspace, blob, idx, hash) VALUES (?1, ?2, 0, ?4)",
            "INSERT INTO blob_listings (workspace, blob, domain, seq) VALUES (?1, ?2, ?3, 1)",
        ] {
            sqlx::query(statement)
                .bind(&workspace_id().0[..])
                .bind(&blob[..])
                .bind(domain)
                .bind(&hash.0[..])
                .execute(&mut *transaction)
                .await
                .expect("a row");
        }
    }
    transaction.commit().await.expect("committed");
    connection.close().await.expect("closed");

    let mut marking = Command::new(BIN)
        .arg("mark-restored")
        .arg("--data")
        .arg(&data)
        .stderr(Stdio::piped())
        .spawn()
        .expect("runs");
    let mut lines = BufReader::new(marking.stderr.take().expect("stderr")).lines();
    let first = lines.next().expect("a line").expect("a line");
    assert!(first.contains("serve may start"), "{first}");
    assert_ne!(
        marking.try_wait().expect("polled").map(|s| s.success()),
        Some(false)
    );

    let server = Server::start(&data);
    let client = Client::new(server.base(), None);
    assert!(client.get("/v2/limits").await.generation().is_some());

    let last = lines.last().expect("a line").expect("a line");
    assert!(last.contains("4000 blob(s) wait"), "{last}");
    assert!(marking.wait().expect("ends").success());
    drop(server);

    let mut connection = database(&db_file(&data)).await;
    let (reset,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM blobs WHERE upload_offset = 0 AND complete = 0")
            .fetch_one(&mut connection)
            .await
            .expect("counted");
    connection.close().await.expect("closed");
    assert_eq!(reset, i64::from(count));
}

/// A backup taken beside a running server carries no lease into the copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_backup_of_a_serving_database_restores_at_once() {
    let (root, data) = fresh();
    let server = Server::start(&data);
    let copy = root.path().join("copy.sqlite3");
    let output = command(&["backup", "--to", text(&copy)], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!root.path().join("copy.sqlite3.partial").exists());
    drop(server);
    let elsewhere = root.path().join("elsewhere");
    let output = command(&["restore", "--from", text(&copy)], &elsewhere);
    assert!(output.status.success(), "{}", stderr(&output));
    let output = command(&["mark-restored"], &elsewhere);
    assert!(output.status.success(), "{}", stderr(&output));
}

/// A stray file in the blob store doesn't stop the pass after the
/// generation rose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stray_file_in_the_blob_store_stays_where_it_is() {
    let (_root, data) = fresh();
    let blobs = disk(&data);
    Row::new(7).store(&blobs).await;
    let stray = data.join("blobs").join("notes.txt");
    std::fs::write(&stray, b"not a segment").expect("written");
    let output = command(&["mark-restored"], &data);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stray.exists());
}

#[test]
fn restore_takes_no_database_url() {
    let root = tempfile::tempdir().expect("temporary directory");
    let output = command(
        &[
            "restore",
            "--from",
            "copy.sqlite3",
            "--database-url",
            "postgres://localhost/none",
        ],
        &root.path().join("data"),
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("mark-restored"),
        "{}",
        stderr(&output)
    );
}
