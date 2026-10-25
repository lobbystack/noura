//! What the server stores after a full sync (seam 4, sync-v2.md §29), on
//! SQLite and on PostgreSQL: no row holds an object type, a local object
//! ID, plaintext, an unpadded size or a per-record timestamp, sessions hold
//! no expiry, and PostgreSQL runs with `track_commit_timestamp` off.

mod sync_support;

use app_core::sync::workspace::WorkspaceSync;
use sync_protocol::codec::Decode;
use sync_protocol::messages::Entry;
use sync_protocol::padding::is_padded_ciphertext_length;
use sync_protocol::records::{Operation, Signed};
use sync_store::Backend;
use sync_support::{Folder, Server, approve, first_desktop};

const NAME: &str = "Stored data check";
const OBJECT_ID: &str = "note_01k0000000000000000000000s";
const PATH: &str = "Journal/Distinct name.md";
const BODY: &str = "Seventeen quiet herons crossed the estuary at dawn.";
const SIZES: [usize; 2] = [1_337, 4_093];

/// The tables whose rows only live minutes, until a sign-in or approval
/// step uses or drops them, and so may carry an expiry (§29).
const TRANSIENT: &[&str] = &[
    "device_grants",
    "approval_requests",
    "approval_offers",
    "magic_links",
    "pending_passkeys",
    "ceremonies",
    "challenges",
    "rate_counts",
    "rate_limits",
];

/// A value the server stores.
enum Value {
    Bytes(Vec<u8>),
    Text(String),
    Int(i64),
}

struct Column {
    table: String,
    name: String,
    timestamp_type: bool,
    values: Vec<Value>,
}

/// The managed note, padded to exactly 1,337 bytes.
fn note() -> Vec<u8> {
    let head = format!("---\nid: {OBJECT_ID}\ntype: note\n---\n\n# Herons\n\n{BODY}\n");
    let mut bytes = head.into_bytes();
    bytes.resize(SIZES[0] - 1, b'.');
    bytes.push(b'\n');
    bytes
}

async fn full_sync<B: Backend>(server: &Server<B>) {
    let mut laptop = first_desktop(server, "ana@example.com").await;
    let mut a = Folder::new(NAME);
    a.write(PATH, &note());
    a.write("Data/blob.bin", &vec![7u8; SIZES[1]]);
    let mut sync_a =
        WorkspaceSync::turn_on(&mut laptop.service, a.engine.clone(), NAME, &mut a.registry)
            .await
            .expect("sync turns on");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    let mut desktop = approve(
        server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut b = Folder::new("Copy");
    let mut sync_b = WorkspaceSync::download(
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
    assert_eq!(b.files(), a.files());
}

fn check(columns: &[Column]) {
    let mut markers: Vec<Vec<u8>> = vec![
        NAME.as_bytes().to_vec(),
        OBJECT_ID.as_bytes().to_vec(),
        PATH.as_bytes().to_vec(),
        b"Distinct name".to_vec(),
        BODY.as_bytes().to_vec(),
        b"type: note".to_vec(),
    ];
    markers.extend(markers.clone().iter().map(|m| hex(m).into_bytes()));
    let mut operations = 0;
    for column in columns {
        let named_for_time = [
            "time", "date", "_at", "created", "updated", "expir", "clock",
        ]
        .iter()
        .any(|word| column.name.contains(word));
        if (named_for_time || column.timestamp_type) && !TRANSIENT.contains(&column.table.as_str())
        {
            panic!("{}.{} dates records", column.table, column.name);
        }
        if column.table == "sessions" {
            assert!(!named_for_time, "sessions hold no expiry");
        }
        for value in &column.values {
            match value {
                Value::Bytes(bytes) => {
                    for marker in &markers {
                        assert!(
                            !bytes.windows(marker.len()).any(|w| w == marker.as_slice()),
                            "{}.{} holds plaintext",
                            column.table,
                            column.name
                        );
                    }
                    if let Ok(entry) = Entry::from_bytes(bytes)
                        && let sync_protocol::messages::EntryRecord::Operation(op) = entry.record
                    {
                        operations += padded(&op);
                    }
                    if let Ok(op) = Signed::<Operation>::from_bytes(bytes) {
                        operations += padded(&op);
                    }
                }
                Value::Text(text) => {
                    for marker in &markers {
                        assert!(
                            !text
                                .as_bytes()
                                .windows(marker.len())
                                .any(|w| w == marker.as_slice()),
                            "{}.{} holds plaintext",
                            column.table,
                            column.name
                        );
                    }
                    assert!(
                        !["note", "task", "project", "chat", "chat-message"]
                            .contains(&text.as_str()),
                        "{}.{} names an object type",
                        column.table,
                        column.name
                    );
                }
                Value::Int(number) => {
                    for size in SIZES {
                        assert_ne!(
                            *number, size as i64,
                            "{}.{} holds a file's unpadded size",
                            column.table, column.name
                        );
                    }
                }
            }
        }
    }
    assert!(operations >= 2, "the check read the stored operations");
}

/// Checks that `op`'s ciphertext has a padded length, and counts it.
fn padded(op: &Signed<Operation>) -> usize {
    assert!(
        is_padded_ciphertext_length(op.body.ciphertext.len() as u64),
        "an operation with an unpadded ciphertext"
    );
    1
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sqlite_store_keeps_no_type_id_plaintext_size_or_time() {
    let server = Server::start().await;
    full_sync(&server).await;
    let database = rusqlite::Connection::open_with_flags(
        server.dir.path().join("data").join(sync_support::DATABASE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the database opens");
    let tables: Vec<String> = database
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(tables.contains(&"operations".to_owned()));
    let mut columns = Vec::new();
    for table in tables {
        let names: Vec<(String, String)> = database
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (name, declared) in names {
            let mut statement = database
                .prepare(&format!("SELECT \"{name}\" FROM \"{table}\""))
                .unwrap();
            let values = statement
                .query_map([], |row| {
                    Ok(match row.get_ref(0)? {
                        rusqlite::types::ValueRef::Blob(b) => Some(Value::Bytes(b.to_vec())),
                        rusqlite::types::ValueRef::Text(t) => {
                            Some(Value::Text(String::from_utf8_lossy(t).into_owned()))
                        }
                        rusqlite::types::ValueRef::Integer(i) => Some(Value::Int(i)),
                        _ => None,
                    })
                })
                .unwrap()
                .filter_map(|v| v.unwrap())
                .collect();
            columns.push(Column {
                table: table.clone(),
                timestamp_type: declared.to_lowercase().contains("time")
                    || declared.to_lowercase().contains("date"),
                name,
                values,
            });
        }
    }
    check(&columns);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_postgres_store_keeps_no_type_id_plaintext_size_or_time() {
    let server = Server::start_postgres().await;
    full_sync(&server).await;
    let database = server.postgres.as_ref().expect("on PostgreSQL");
    assert!(
        !database.cluster.dates_commits().await,
        "track_commit_timestamp is off"
    );
    let columns: Vec<Column> = database
        .columns()
        .await
        .into_iter()
        .filter(|c| !c.table.starts_with("_sqlx"))
        .map(|c| Column {
            timestamp_type: c.data_type.contains("timestamp")
                || c.data_type.contains("date")
                || c.data_type.contains("time"),
            table: c.table,
            name: c.column,
            values: c
                .values
                .into_iter()
                .filter_map(|v| match v {
                    sync_store_postgres::testing::Stored::Bytes(b) => Some(Value::Bytes(b)),
                    sync_store_postgres::testing::Stored::Text(t) => Some(Value::Text(t)),
                    sync_store_postgres::testing::Stored::Int(i) => Some(Value::Int(i)),
                    sync_store_postgres::testing::Stored::Other => None,
                })
                .collect(),
        })
        .collect();
    assert!(columns.iter().any(|c| c.table == "operations"));
    check(&columns);
}
