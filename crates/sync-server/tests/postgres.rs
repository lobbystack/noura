//! The binary in PostgreSQL mode at seam 4: a database URL switches
//! `migrate` and `serve` to PostgreSQL, every instance takes the server
//! secret from `NOURA_SERVER_SECRET` or a mounted file and checks it
//! against the key check the database holds (§14), `serve` refuses a
//! PostgreSQL that dates commits (§29), reaches one that requires TLS, and
//! two instances on one database serve the same log and wake each other's
//! pulls only after a commit (§15).

mod emulator;
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::{Connection, PgConnection};
use support::{Account, BIN, Client, Created, Server, push_body, seed_store};
use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::{Encode, List};
use sync_protocol::ids::secret_check;
use sync_protocol::messages::{
    EntryRecord, LimitsResponse, PullCursor, PullRequest, PullResponse, PushResponse,
};
use sync_protocol::records::{Operation, Signed};
use sync_protocol::types::{Digest, SecretKey, to_hex};
use sync_store_postgres::testing::{Cluster, TestDatabase};

fn tmp() -> &'static Path {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A command of the binary with no secret variable but those given.
fn command(args: &[&str], data: &Path) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .arg("--data")
        .arg(data)
        .env_remove("NOURA_SERVER_SECRET")
        .env_remove("NOURA_SERVER_SECRET_FILE")
        .env_remove("NOURA_DATABASE_URL");
    command
}

/// A command in PostgreSQL mode, with `secret` in `NOURA_SERVER_SECRET`.
fn postgres(args: &[&str], data: &Path, url: &str, secret: Option<&SecretKey>) -> Command {
    let mut command = command(args, data);
    command.arg("--database-url").arg(url);
    if let Some(secret) = secret {
        command.env("NOURA_SERVER_SECRET", to_hex(secret.expose()));
    }
    command
}

fn output(command: &mut Command) -> std::process::Output {
    command.output().expect("the binary runs")
}

fn random_secret() -> SecretKey {
    SecretKey::random().expect("random")
}

/// A fresh database, migrated with `secret`.
async fn migrated(secret: &SecretKey) -> (TestDatabase, tempfile::TempDir) {
    let database = Cluster::shared(tmp()).database().await;
    let root = tempfile::tempdir().expect("temporary directory");
    let migrate = output(&mut postgres(
        &["migrate"],
        root.path(),
        &database.url,
        Some(secret),
    ));
    assert!(migrate.status.success(), "{}", stderr(&migrate));
    (database, root)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_then_serve_on_an_empty_postgres_database() {
    let cluster = Cluster::shared(tmp());
    let database = cluster.database().await;
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    let secret = random_secret();

    // No secret: migrate refuses and creates none.
    let refused = output(&mut postgres(&["migrate"], &data, &database.url, None));
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("no server secret"),
        "{}",
        stderr(&refused)
    );

    let first = output(&mut postgres(
        &["migrate"],
        &data,
        &database.url,
        Some(&secret),
    ));
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(
        stderr(&first).contains("ran 7 migration(s)"),
        "{}",
        stderr(&first)
    );
    assert!(
        !data.join("server-secret").exists(),
        "PostgreSQL mode writes no secret file"
    );

    // The database holds the key check, never the secret.
    let mut connection = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    let (check, generation): (Vec<u8>, i64) =
        sqlx::query_as("SELECT secret_check, restore_generation FROM server")
            .fetch_one(&mut connection)
            .await
            .expect("the server row");
    assert_eq!(check, secret_check(&secret).expose().to_vec());
    assert_ne!(check, secret.expose().to_vec());
    assert_eq!(generation, 1);
    let applied: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .expect("migrations");
    if let Some(dir) = cluster.data_dir() {
        sqlx::query("CHECKPOINT")
            .execute(&mut connection)
            .await
            .expect("checkpoint");
        let hex = to_hex(secret.expose());
        for file in files(dir) {
            let bytes = std::fs::read(&file).unwrap_or_default();
            for needle in [&secret.expose()[..], hex.as_bytes()] {
                assert!(
                    !bytes.windows(needle.len()).any(|w| w == needle),
                    "{} holds the secret",
                    file.display()
                );
            }
        }
    }

    // A second migrate changes nothing.
    let second = output(&mut postgres(
        &["migrate"],
        &data,
        &database.url,
        Some(&secret),
    ));
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(stderr(&second).contains("ran 0 migration(s)"));
    let again: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .expect("migrations");
    let (check_again,): (Vec<u8>,) = sqlx::query_as("SELECT secret_check FROM server")
        .fetch_one(&mut connection)
        .await
        .expect("the server row");
    connection.close().await.expect("closed");
    assert_eq!((again, check_again), (applied, check));

    // `serve` then answers.
    let server = Server::start_with(&mut postgres(
        &["serve"],
        &data,
        &database.url,
        Some(&secret),
    ));
    let limits: LimitsResponse = Client::new(server.base(), None)
        .get("/v2/limits")
        .await
        .decode();
    assert_eq!(limits.protocol, 2);
    // Its log goes to the data directory.
    assert!(data.join("logs").is_dir());
}

#[test]
fn every_command_refuses_a_missing_or_different_secret_on_postgres() {
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let secret = random_secret();
    let (database, root) = runtime.block_on(migrated(&secret));
    let data = root.path();
    let url = &database.url;
    for args in [&["migrate"][..], &["serve", "--listen", "127.0.0.1:0"][..]] {
        let missing = output(&mut postgres(args, data, url, None));
        assert!(
            !missing.status.success(),
            "{args:?} runs without the secret"
        );
        assert!(
            stderr(&missing).contains("no server secret"),
            "{}",
            stderr(&missing)
        );
        let other = output(&mut postgres(args, data, url, Some(&random_secret())));
        assert!(!other.status.success(), "{args:?} runs with another secret");
        assert!(
            stderr(&other).contains("doesn't match"),
            "{}",
            stderr(&other)
        );
    }

    // A mounted secret file works like the variable; both at once don't.
    let file = data.join("mounted-secret");
    std::fs::write(&file, format!("{}\n", to_hex(secret.expose()))).expect("written");
    let mut mounted = postgres(&["migrate"], data, url, None);
    mounted.env("NOURA_SERVER_SECRET_FILE", &file);
    let mounted = output(&mut mounted);
    assert!(mounted.status.success(), "{}", stderr(&mounted));
    let mut both = postgres(&["migrate"], data, url, Some(&secret));
    both.env("NOURA_SERVER_SECRET_FILE", &file);
    let both = output(&mut both);
    assert!(!both.status.success());
    assert!(stderr(&both).contains("not both"), "{}", stderr(&both));
    let malformed = output(
        command(&["migrate", "--database-url", url], data).env("NOURA_SERVER_SECRET", "abc"),
    );
    assert!(!malformed.status.success());
    assert!(
        stderr(&malformed).contains("64 hexadecimal digits"),
        "{}",
        stderr(&malformed)
    );

    // Without a database URL the variables would go unused.
    let sqlite = output(
        command(&["migrate"], &data.join("sqlite"))
            .env("NOURA_SERVER_SECRET", to_hex(secret.expose())),
    );
    assert!(!sqlite.status.success());
    assert!(
        stderr(&sqlite).contains("apply to PostgreSQL mode"),
        "{}",
        stderr(&sqlite)
    );
    assert!(!data.join("sqlite").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_refuses_an_unmigrated_database_and_pending_migrations() {
    let secret = random_secret();
    let database = Cluster::shared(tmp()).database().await;
    let root = tempfile::tempdir().expect("temporary directory");
    let unmigrated = output(&mut postgres(
        &["serve"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    assert!(!unmigrated.status.success());
    assert!(
        stderr(&unmigrated).contains("run `sync-server migrate` first"),
        "{}",
        stderr(&unmigrated)
    );
    let migrate = output(&mut postgres(
        &["migrate"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    assert!(migrate.status.success(), "{}", stderr(&migrate));
    let mut connection = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&mut connection)
        .await
        .expect("a migration forgotten");
    connection.close().await.expect("closed");
    let pending = output(&mut postgres(
        &["serve"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    assert!(!pending.status.success());
    assert!(
        stderr(&pending).contains("are pending"),
        "{}",
        stderr(&pending)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_refuses_a_postgres_that_dates_commits() {
    let cluster = Arc::new(Cluster::start(tmp(), &[("track_commit_timestamp", "on")]));
    let database = cluster.database().await;
    let root = tempfile::tempdir().expect("temporary directory");
    let secret = random_secret();
    let migrate = output(&mut postgres(
        &["migrate"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    assert!(migrate.status.success(), "{}", stderr(&migrate));
    let serve = output(&mut postgres(
        &["serve"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    assert!(!serve.status.success(), "serve starts with commit times on");
    assert!(
        stderr(&serve).contains("track_commit_timestamp is on"),
        "{}",
        stderr(&serve)
    );
}

/// A self-signed certificate and its key, in PEM.
fn self_signed() -> (Vec<u8>, Vec<u8>) {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::ec::{EcGroup, EcKey};
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::x509::{X509, X509NameBuilder};

    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).expect("P-256");
    let key = PKey::from_ec_key(EcKey::generate(&group).expect("a key")).expect("a key");
    let mut name = X509NameBuilder::new().expect("a name");
    name.append_entry_by_text("CN", "localhost").expect("CN");
    let name = name.build();
    let mut builder = X509::builder().expect("a certificate");
    builder.set_version(2).expect("v3");
    builder
        .set_serial_number(
            &BigNum::from_u32(1)
                .expect("1")
                .to_asn1_integer()
                .expect("1"),
        )
        .expect("serial");
    builder.set_subject_name(&name).expect("subject");
    builder.set_issuer_name(&name).expect("issuer");
    builder.set_pubkey(&key).expect("key");
    builder
        .set_not_before(&Asn1Time::days_from_now(0).expect("now"))
        .expect("not before");
    builder
        .set_not_after(&Asn1Time::days_from_now(2).expect("later"))
        .expect("not after");
    builder.sign(&key, MessageDigest::sha256()).expect("signed");
    (
        builder.build().to_pem().expect("PEM"),
        key.private_key_to_pem_pkcs8().expect("PEM"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_reaches_a_postgres_that_requires_tls() {
    let (certificate, key) = self_signed();
    let cluster = Arc::new(
        Cluster::start_with(
            tmp(),
            &[
                ("ssl", "on"),
                ("ssl_cert_file", "'server.crt'"),
                ("ssl_key_file", "'server.key'"),
            ],
            |data| {
                std::fs::write(data.join("server.crt"), &certificate).expect("certificate");
                std::fs::write(data.join("server.key"), &key).expect("key");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        data.join("server.key"),
                        std::fs::Permissions::from_mode(0o600),
                    )
                    .expect("the key is private");
                }
                // Every TCP connection but one over TLS is refused.
                std::fs::write(
                    data.join("pg_hba.conf"),
                    "hostssl all all 127.0.0.1/32 trust\nhostnossl all all 127.0.0.1/32 reject\n",
                )
                .expect("pg_hba.conf");
            },
        )
        .with_url_options("sslmode=require"),
    );
    let database = cluster.database().await;
    let root = tempfile::tempdir().expect("temporary directory");
    let secret = random_secret();
    let plain = database.url.replace("sslmode=require", "sslmode=disable");
    let refused = output(&mut postgres(
        &["migrate"],
        root.path(),
        &plain,
        Some(&secret),
    ));
    assert!(!refused.status.success(), "a plain connection is refused");

    let migrate = output(&mut postgres(
        &["migrate"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    assert!(migrate.status.success(), "{}", stderr(&migrate));
    let server = Server::start_with(&mut postgres(
        &["serve"],
        root.path(),
        &database.url,
        Some(&secret),
    ));
    let limits: LimitsResponse = Client::new(server.base(), None)
        .get("/v2/limits")
        .await
        .decode();
    assert_eq!(limits.protocol, 2);
}

/// Two `serve` instances on one migrated database, and an account of
/// `devices` devices written through a third.
async fn two_instances(
    devices: usize,
) -> (Server, Server, Account, TestDatabase, tempfile::TempDir) {
    let secret = random_secret();
    let (database, root) = migrated(&secret).await;
    let store = sync_server::postgres::open_store(&sync_server::Postgres {
        url: database.url.clone(),
        secret: Some(secret.clone()),
    })
    .await
    .expect("the store opens");
    let account = seed_store(&store, devices).await;
    sync_store::Backend::close(store.backend()).await;
    let first = Server::start_with(&mut postgres(
        &["serve"],
        &root.path().join("first"),
        &database.url,
        Some(&secret),
    ));
    let second = Server::start_with(&mut postgres(
        &["serve"],
        &root.path().join("second"),
        &database.url,
        Some(&secret),
    ));
    (first, second, account, database, root)
}

/// Every operation of `domain` after `after` that `client` pulls, in log
/// order, with its sequence.
async fn pull_all(
    client: &Client,
    workspace: &str,
    domain: sync_protocol::types::Id,
) -> Vec<(u64, Signed<Operation>)> {
    let mut found = Vec::new();
    let mut after = 0;
    loop {
        let response: PullResponse = client
            .post(
                &format!("/v2/workspaces/{workspace}/pull"),
                PullRequest {
                    cursors: List(vec![PullCursor { domain, after }]),
                    wait_seconds: 0,
                }
                .to_bytes(),
            )
            .await
            .decode();
        let page = &response.pages[0];
        for entry in page.entries.iter() {
            if let EntryRecord::Operation(op) = &entry.record {
                found.push((entry.seq, op.clone()));
            }
        }
        after = page.complete_through;
        if after >= page.head {
            return found;
        }
    }
}

/// Pushes 20 operations of `keys`' chain through `client`, one at a time.
async fn push_chain(
    client: &Client,
    created: &Created,
    keys: &DeviceKeys,
) -> Vec<Signed<Operation>> {
    let path = format!("/v2/workspaces/{}/ops", created.workspace.to_base64url());
    let mut previous = Digest::ZERO;
    let mut pushed = Vec::new();
    for counter in 1..=20 {
        let op = created.op(keys, counter, previous, |_| {});
        previous = op.digest();
        let _: PushResponse = client
            .post(&path, push_body(std::slice::from_ref(&op)))
            .await
            .decode();
        pushed.push(op);
    }
    pushed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_instances_take_pushes_from_different_clients_and_serve_one_order() {
    let (first, second, account, _database, _root) = two_instances(2).await;
    let created = account.creation();
    let owner = Client::new(first.base(), Some(account.token(0)));
    let _: PushResponse = owner
        .post("/v2/workspaces", created.body.clone())
        .await
        .decode();
    let other = Client::new(second.base(), Some(account.token(1)));
    let workspace = created.workspace.to_base64url();
    let (mine, theirs) = tokio::join!(
        push_chain(&owner, &created, &account.keys[0]),
        push_chain(&other, &created, &account.keys[1])
    );
    let from_first = pull_all(&owner, &workspace, created.domain).await;
    let from_second = pull_all(&other, &workspace, created.domain).await;
    assert_eq!(from_first, from_second, "both instances serve one order");
    assert_eq!(from_first.len(), 40);
    let seqs: Vec<u64> = from_first.iter().map(|(seq, _)| *seq).collect();
    assert_eq!(seqs, (3..=42).collect::<Vec<u64>>());
    for pushed in [mine, theirs] {
        let order: Vec<&Signed<Operation>> = from_first
            .iter()
            .map(|(_, op)| op)
            .filter(|op| op.body.device == pushed[0].body.device)
            .collect();
        assert_eq!(order, pushed.iter().collect::<Vec<_>>());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_on_one_instance_wakes_when_the_other_commits_and_only_then() {
    let (first, second, account, _database, _root) = two_instances(2).await;
    let created = account.creation();
    let owner = Client::new(first.base(), Some(account.token(0)));
    let _: PushResponse = owner
        .post("/v2/workspaces", created.body.clone())
        .await
        .decode();
    let reader = Arc::new(Client::new(second.base(), Some(account.token(1))));
    let workspace = created.workspace.to_base64url();
    let wait = |seconds: u32| {
        let reader = reader.clone();
        let path = format!("/v2/workspaces/{workspace}/pull");
        let body = PullRequest {
            cursors: List(vec![PullCursor {
                domain: created.domain,
                after: 2,
            }]),
            wait_seconds: seconds,
        }
        .to_bytes();
        tokio::spawn(async move {
            let started = Instant::now();
            let response: PullResponse = reader.post(&path, body).await.decode();
            (started.elapsed(), response)
        })
    };
    let ops = format!("/v2/workspaces/{workspace}/ops");

    // A push that rolls back wakes nobody.
    let waiting = wait(3);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let good = created.op(&account.keys[0], 1, Digest::ZERO, |_| {});
    let bad = created.op(&account.keys[0], 2, Digest([7; 32]), |_| {});
    let refused = owner.post(&ops, push_body(&[good.clone(), bad])).await;
    assert_eq!(refused.error().code.as_str(), "chain_conflict");
    let (elapsed, response) = waiting.await.expect("the pull returns");
    assert!(response.pages[0].entries.is_empty());
    assert!(
        elapsed >= Duration::from_millis(2_900),
        "a rolled-back push woke the pull after {elapsed:?}"
    );

    // A push that commits on the first instance wakes the second's pull.
    let waiting = wait(20);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _: PushResponse = owner
        .post(&ops, push_body(std::slice::from_ref(&good)))
        .await
        .decode();
    let (elapsed, response) = waiting.await.expect("the pull returns");
    assert!(elapsed < Duration::from_secs(10), "woke after {elapsed:?}");
    let entries = &response.pages[0].entries;
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].record, EntryRecord::Operation(op) if *op == good));
}

/// On PostgreSQL with S3 storage, an upload whose segments reach two
/// different instances completes, and either instance serves it (§21).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_across_two_instances_completes_on_postgres_and_s3() {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use sync_protocol::blob::{SEGMENT_LEN, decrypt_blob, encrypt_blob};
    use sync_protocol::codec::Sorted;
    use sync_protocol::types::Id;

    let secret = random_secret();
    let (database, _root) = migrated(&secret).await;
    let config = sync_server::Postgres {
        url: database.url.clone(),
        secret: Some(secret.clone()),
    };
    let first = Arc::new(
        sync_server::postgres::open_store(&config)
            .await
            .expect("opens"),
    );
    let second = Arc::new(
        sync_server::postgres::open_store(&config)
            .await
            .expect("opens"),
    );
    let world = sync_store::conformance::World::on(first.clone(), 1).await;
    let emulator = emulator::Emulator::start().await;
    let mut bases = Vec::new();
    for store in [first, second] {
        let address = sync_server::spawn_with_blobs(
            store,
            "127.0.0.1:0",
            sync_server::Accounts::local,
            Arc::new(emulator.store()),
        )
        .await
        .expect("serving");
        bases.push(format!("http://{address}"));
    }

    let plaintext: Vec<u8> = (0..2_500_000u32).map(|i| (i % 249) as u8).collect();
    let blob = Id::random().expect("random");
    let (ciphertext, blob_ref) =
        encrypt_blob(&plaintext, blob, SecretKey::random().expect("random")).expect("encrypts");
    let http = reqwest::Client::new();
    let token = world.devices[0].bearer();
    let created = http
        .post(format!(
            "{}/v2/workspaces/{}/blobs",
            bases[0],
            world.workspace.to_base64url()
        ))
        .bearer_auth(&token)
        .header("Tus-Resumable", "1.0.0")
        .header("Upload-Length", blob_ref.ciphertext_length)
        .header(
            "Upload-Metadata",
            format!(
                "blob {},digest {},domain {}",
                STANDARD.encode(blob.0),
                STANDARD.encode(blob_ref.ciphertext_digest.0),
                STANDARD.encode(world.domain.0)
            ),
        )
        .send()
        .await
        .expect("creation");
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let location = created.headers()["Location"]
        .to_str()
        .expect("text")
        .to_owned();
    for (index, chunk) in ciphertext.chunks(SEGMENT_LEN).enumerate() {
        let response = http
            .patch(format!("{}{location}", bases[index % 2]))
            .bearer_auth(&token)
            .header("Tus-Resumable", "1.0.0")
            .header("Upload-Offset", index * SEGMENT_LEN)
            .header("Content-Type", "application/offset+octet-stream")
            .body(chunk.to_vec())
            .send()
            .await
            .expect("patch");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::NO_CONTENT,
            "segment {index}"
        );
    }
    let listing = world.op(0, |o| {
        o.blobs = Sorted::new(vec![blob]).expect("sorted");
    });
    world.push(0, &[listing]).await.expect("listed");
    for base in &bases {
        let mut downloaded = Vec::new();
        while downloaded.len() < ciphertext.len() {
            let response = http
                .get(format!("{base}{location}"))
                .bearer_auth(&token)
                .header("Range", format!("bytes={}-", downloaded.len()))
                .send()
                .await
                .expect("get");
            assert_eq!(response.status(), reqwest::StatusCode::PARTIAL_CONTENT);
            downloaded.extend_from_slice(&response.bytes().await.expect("body"));
        }
        assert_eq!(
            decrypt_blob(&downloaded, &blob_ref, None).expect("decrypts"),
            plaintext
        );
    }
}
