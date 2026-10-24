//! The sync service against the real server on SQLite and on PostgreSQL
//! (seam 4): one client creates a workspace and pushes an encrypted v2
//! operation, and a fresh client with the same device credentials fetches
//! the policy and keys, pulls the operation, checks its signature and
//! chain, and decrypts it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use app_core::sync::{DeviceCredentials, SyncError, SyncService};
use sync_key_envelope::v2::{DeviceKeys, genesis_link};
use sync_protocol::codec::Encode;
use sync_protocol::messages::LimitsResponse;
use sync_protocol::records::AccountHead;
use sync_protocol::types::{Id, to_base64url};
use sync_store::{Backend, Session, Store};
use sync_store_postgres::testing::{Cluster, TestDatabase};

/// Where the server keeps its state, kept alive for the test.
enum Database {
    /// The data directory holds the SQLite database.
    Sqlite(tempfile::TempDir),
    /// A PostgreSQL database.
    Postgres(TestDatabase),
}

impl Database {
    /// The files that hold what the server stored.
    async fn files(&self) -> Vec<PathBuf> {
        match self {
            Database::Sqlite(dir) => {
                let data = dir.path().join("data");
                vec![data.join(DATABASE), data.join(format!("{DATABASE}-wal"))]
            }
            Database::Postgres(database) => {
                // A checkpoint writes every page to the data files.
                database.cluster.execute("CHECKPOINT").await;
                database
                    .cluster
                    .data_dir()
                    .map_or_else(Vec::new, |dir| files(dir))
            }
        }
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

async fn sqlite() -> (Store<sync_store_sqlite::SqliteBackend>, Database) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = sync_server::open_store(&sync_server::Config {
        data_dir: data,
        listen: "127.0.0.1:0".parse().expect("an address"),
        postgres: None,
    })
    .await
    .expect("the store opens");
    (store, Database::Sqlite(dir))
}

async fn postgres() -> (Store<sync_store_postgres::PgBackend>, Database) {
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
        .expect("the store opens");
    (store, Database::Postgres(database))
}

async fn server<B: Backend>(store: Store<B>) -> (String, DeviceCredentials) {
    // Until ticket 46 adds sign-in and the chain routes, the account chain
    // and the session go straight into the store.
    let account = Id::random().expect("random");
    let keys = DeviceKeys::generate().expect("random");
    let genesis = genesis_link(&keys, account);
    store
        .append_account_links(vec![genesis.clone()])
        .await
        .expect("the chain is stored");
    let token: [u8; 32] = sync_protocol::crypto::random_array().expect("random");
    store
        .create_session(
            token,
            Session {
                account,
                device: keys.device(),
            },
        )
        .await
        .expect("the session is stored");
    let address = sync_server::spawn(Arc::new(store), "127.0.0.1:0")
        .await
        .expect("serving");
    let credentials = DeviceCredentials {
        account,
        keys,
        session_token: to_base64url(&token),
        account_chain: vec![genesis.clone()],
        reference_head: AccountHead {
            account,
            genesis: genesis.digest(),
            seqno: 1,
            head: genesis.digest(),
        },
    };
    (format!("http://{address}"), credentials)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_client_pulls_and_decrypts_what_another_pushed() {
    let (store, database) = sqlite().await;
    push_and_pull(server(store).await, database).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_client_pulls_and_decrypts_what_another_pushed_on_postgres() {
    let (store, database) = postgres().await;
    push_and_pull(server(store).await, database).await;
}

async fn push_and_pull((base, credentials): (String, DeviceCredentials), database: Database) {
    let note = b"# Field notes\n\nThe first synced note.\n";

    let mut writer = SyncService::connect(&base, credentials.clone())
        .await
        .expect("connects");
    let mut created = writer
        .create_workspace("Field notes")
        .await
        .expect("the workspace is created");
    let receipt = writer
        .push_file(&mut created, "Notes/First.md", note)
        .await
        .expect("the operation is pushed");
    assert_eq!(receipt.domain, created.domain);
    assert_eq!(receipt.seq, 3, "after the genesis and epoch entries");

    let mut reader = SyncService::connect(&base, credentials.clone())
        .await
        .expect("connects");
    let mut remote = reader
        .open_workspace(created.workspace)
        .await
        .expect("the policy and keys verify");
    assert_eq!(remote.domain, created.domain);
    assert_eq!(remote.policy_revision(), 1);
    let files = reader
        .pull_files(&mut remote)
        .await
        .expect("the pull verifies");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "Notes/First.md");
    assert_eq!(files[0].bytes, note);
    assert_eq!(files[0].device, credentials.keys.device());
    assert!(
        reader
            .pull_files(&mut remote)
            .await
            .expect("a second pull")
            .is_empty()
    );

    // The server stored ciphertext: neither the note, its path nor the
    // workspace name appears in its database.
    for file in database.files().await {
        let bytes = std::fs::read(&file).unwrap_or_default();
        for plaintext in [
            &b"The first synced note"[..],
            b"Notes/First.md",
            b"Field notes",
        ] {
            assert!(
                !bytes.windows(plaintext.len()).any(|w| w == plaintext),
                "{} holds plaintext",
                file.display()
            );
        }
    }
}

/// The database file in the server's data directory.
const DATABASE: &str = "sync.sqlite3";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_refuses_a_server_without_protocol_2() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("an address");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("a connection");
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let body = LimitsResponse {
            protocol: 1,
            restore_generation: 1,
            max_body: 1_048_576,
            max_inline: 262_144,
            max_blob: 2_147_483_648,
            max_covers: 256,
            max_recipients: 512,
            presence_per_second: 2,
            retained_bytes: 1 << 30,
        }
        .to_bytes();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/vnd.noura.sync.v2\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body);
    });
    let keys = DeviceKeys::generate().expect("random");
    let account = Id::random().expect("random");
    let genesis = genesis_link(&keys, account);
    let credentials = DeviceCredentials {
        account,
        keys,
        session_token: to_base64url(&[1; 32]),
        account_chain: vec![genesis.clone()],
        reference_head: AccountHead {
            account,
            genesis: genesis.digest(),
            seqno: 1,
            head: genesis.digest(),
        },
    };
    let result = SyncService::connect(&format!("http://{address}"), credentials).await;
    assert!(
        matches!(result, Err(SyncError::ProtocolUnsupported(1))),
        "the client refuses a v1 server"
    );
}
