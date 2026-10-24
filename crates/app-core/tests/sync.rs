//! The sync service against the real server on SQLite and on PostgreSQL
//! (seam 4): one client creates a workspace and pushes an encrypted v2
//! operation, and a fresh client with the same device credentials takes its
//! account chain from the server, fetches the policy and keys, pulls the
//! operation, checks its signature and chain, and decrypts it.

mod sync_support;

use std::io::{Read, Write};
use std::net::TcpListener;

use app_core::sync::{DeviceCredentials, KeptHeads, SyncError, SyncService};
use sync_key_envelope::v2::{DeviceKeys, genesis_link};
use sync_protocol::codec::Encode;
use sync_protocol::messages::LimitsResponse;
use sync_protocol::records::AccountHead;
use sync_protocol::types::{Id, to_base64url};
use sync_store::Backend;
use sync_support::{Account, MemorySecrets, Server};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_client_pulls_and_decrypts_what_another_pushed() {
    push_and_pull(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_client_pulls_and_decrypts_what_another_pushed_on_postgres() {
    push_and_pull(Server::start_postgres().await).await;
}

async fn push_and_pull<B: Backend>(server: Server<B>) {
    let account = Account::new(&server).await;
    let device = &account.devices[0];    let note = b"# Field notes\n\nThe first synced note.\n";

    let mut writer = device.connect(&server.base, account.reference()).await;
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

    // A fresh client: the same device credentials, no kept heads yet.
    let fresh = tempfile::tempdir().unwrap();
    let secrets = std::sync::Arc::new(MemorySecrets::default());
    let heads = KeptHeads::open(fresh.path(), account.id, device.id(), &*secrets).unwrap();
    let mut reader = SyncService::connect(
        &server.base,
        device.credentials(account.reference()),
        heads,
        secrets,
    )
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
    assert_eq!(files[0].device, device.id());
    assert!(
        reader
            .pull_files(&mut remote)
            .await
            .expect("a second pull")
            .is_empty()
    );
    // The reader keeps its account's head now.
    assert_eq!(
        reader.kept_heads().kept_of(&account.id),
        vec![account.head()]
    );

    // The server stored ciphertext: neither the note, its path nor the
    // workspace name appears in its database.
    for file in server.stored_files().await {
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
            );        }
    }
}

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
    let device = keys.device();
    let credentials = DeviceCredentials {
        account,
        keys,
        session_token: to_base64url(&[1; 32]),
        reference_head: AccountHead {
            account,
            genesis: genesis.digest(),
            seqno: 1,
            head: genesis.digest(),
        },
    };
    let dir = tempfile::tempdir().unwrap();
    let secrets = std::sync::Arc::new(MemorySecrets::default());
    let heads = KeptHeads::open(dir.path(), account, device, &*secrets).unwrap();
    let result =
        SyncService::connect(&format!("http://{address}"), credentials, heads, secrets).await;
    assert!(
        matches!(result, Err(SyncError::ProtocolUnsupported(1))),
        "the client refuses a v1 server"
    );
}
