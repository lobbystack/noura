//! The HTTP API at seam 4 (§26): limits and the restore generation header,
//! error bodies with the statuses `sync-v2-encoding.json` pins, the read
//! routes, and a pull that wakes through the event hub.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use reqwest::StatusCode;
use support::{Client, push_body};
use sync_protocol::codec::{Bytes, Decode, Encode, List, Sorted};
use sync_protocol::messages::{
    EntryRecord, EpochsResponse, Frame, HeadResponse, KeyRecord, KeysResponse, LimitsResponse,
    PolicyResponse, PullCursor, PullRequest, PullResponse, PushResponse, WorkspacesResponse,
};
use sync_protocol::records::{
    Cover, LinkKind, MAX_OPERATION_CIPHERTEXT, PolicyKind, Presence, Signed,
};
use sync_protocol::types::{Digest, Id, Nonce};
use sync_store::conformance::World;
use sync_store::{INITIAL_RESTORE_GENERATION, Store};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

async fn world(devices: usize) -> (World<SqliteBackend>, String, tempfile::TempDir) {
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
    let address = sync_server::spawn(world.store.clone(), "127.0.0.1:0")
        .await
        .expect("serving");
    (world, format!("http://{address}"), dir)
}

fn client(world: &World<SqliteBackend>, base: &str, device: usize) -> Client {
    Client::new(base.to_owned(), Some(world.devices[device].bearer()))
}

/// The code the realtime channel answers a presence record of the first
/// device with.
async fn presence_answer(world: &World<SqliteBackend>, base: &str) -> String {
    let url = format!(
        "{}/v2/workspaces/{}/realtime",
        base.replace("http://", "ws://"),
        world.workspace.to_base64url()
    );
    let mut request = url.into_client_request().expect("a request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", world.devices[0].bearer())
            .parse()
            .expect("a header"),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the channel opens");
    let record = Presence {
        workspace: world.workspace,
        domain: world.domain,
        epoch: 1,
        device: world.devices[0].id(),
        session: Id([5; 16]),
        sequence: 1,
        minute: 1,
        channel: Id([6; 16]),
        nonce: Nonce::ZERO,
        ciphertext: Bytes(vec![0x5a; 272]),
    };
    let frame = Frame::Presence {
        record: Signed::sign(record, world.devices[0].keys.signing()),
    };
    socket
        .send(Message::Binary(frame.to_bytes().into()))
        .await
        .expect("sent");
    while let Some(message) = socket.next().await {
        if let Message::Binary(bytes) = message.expect("the channel stays open")
            && let Ok(Frame::Error { code, .. }) = Frame::from_bytes(&bytes)
        {
            return code.0;
        }
    }
    panic!("no error frame");
}

fn encoding_fixture() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/sync-v2-encoding.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
}

/// The HTTP status `sync-v2-encoding.json` gives each server error code.
fn fixture_statuses() -> BTreeMap<String, u16> {
    encoding_fixture()["server_errors"]
        .as_array()
        .expect("server errors")
        .iter()
        .map(|e| {
            (
                e["code"].as_str().expect("a code").to_owned(),
                e["status"].as_u64().expect("a status") as u16,
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn limits_report_protocol_2_and_every_response_carries_the_generation() {
    let (world, base, _dir) = world(1).await;
    let anonymous = Client::new(base.clone(), None);
    let reply = anonymous.get("/v2/limits").await;
    assert_eq!(reply.generation(), Some("1"));
    assert_eq!(
        reply
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some(sync_protocol::messages::MEDIA_TYPE)
    );
    let limits: LimitsResponse = reply.decode();
    assert_eq!(
        limits,
        LimitsResponse {
            protocol: 2,
            restore_generation: 1,
            max_body: 1_048_576,
            max_inline: 262_144,
            max_blob: 2_147_483_648,
            max_covers: 256,
            max_recipients: 512,
            presence_per_second: 2,
            retained_bytes: 1 << 30,
        }
    );

    // No /v1 route, unknown routes and methods: not_found, with the header.
    for reply in [
        anonymous.get("/v1/limits").await,
        anonymous.get("/v1/workspaces").await,
        anonymous.post("/v2/limits", Vec::new()).await,
    ] {
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        assert_eq!(reply.error().code.as_str(), "not_found");
        assert_eq!(reply.generation(), Some("1"));
    }
    // No session: unauthorized, with the header.
    let reply = anonymous.get("/v2/workspaces").await;
    assert_eq!(
        (reply.status, reply.error().code.as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    assert_eq!(reply.generation(), Some("1"));
    let forged = Client::new(base.clone(), Some(Id([1; 16]).to_base64url()));
    assert_eq!(
        forged.get("/v2/workspaces").await.status,
        StatusCode::UNAUTHORIZED
    );
    // A malformed ID in a path.
    let owner = client(&world, &base, 0);
    let reply = owner.post("/v2/workspaces/not-an-id/ops", Vec::new()).await;
    assert_eq!(
        (reply.status, reply.error().code.as_str()),
        (StatusCode::BAD_REQUEST, "bad_request")
    );
    // A body above 1 MiB.
    let path = format!("/v2/workspaces/{}/ops", world.workspace.to_base64url());
    let reply = owner.post(&path, vec![0; 1_048_577]).await;
    assert_eq!(
        (reply.status, reply.error().code.as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
    assert_eq!(reply.generation(), Some("1"));
}

/// A fixture record's bytes: its own hex, or another record's with the
/// mutation applied.
fn fixture_bytes(records: &[serde_json::Value], record: &serde_json::Value) -> Vec<u8> {
    let hex = |text: &serde_json::Value| {
        sync_protocol::types::from_hex(text.as_str().expect("hex")).expect("hex bytes")
    };
    if let Some(text) = record.get("hex") {
        return hex(text);
    }
    let from = record["from"].as_str().expect("a source record");
    let source = records
        .iter()
        .find(|r| r["name"] == from)
        .expect("the source record");
    let mut bytes = hex(&source["hex"]);
    let mutation = &record["mutation"];
    if let Some(count) = mutation.get("truncate") {
        let count = count.as_u64().expect("a count") as usize;
        bytes.truncate(bytes.len() - count);
    }
    if let Some(tail) = mutation.get("append") {
        bytes.extend(hex(tail));
    }
    bytes
}

/// A `list` of one element.
fn list_of_one(element: &[u8]) -> Vec<u8> {
    let mut bytes = 1u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(element);
    bytes
}

/// The code §26 gives a body that fails to decode this way.
fn decode_code(kind: &str) -> &'static str {
    match kind {
        "too_long" => "too_large",
        "unsupported_version" => "version_unsupported",
        _ => "bad_request",
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_bodies_get_the_codes_of_their_decode_errors() {
    let statuses = fixture_statuses();
    let doc = encoding_fixture();
    let records = doc["records"].as_array().expect("records");
    let (world, base, _dir) = world(1).await;
    let owner = client(&world, &base, 0);
    let w = world.workspace.to_base64url();
    let ops = format!("/v2/workspaces/{w}/ops");
    let pull = format!("/v2/workspaces/{w}/pull");
    let create = "/v2/workspaces".to_owned();
    let mut sent = 0;
    for record in records {
        let Some(kind) = record["expect"]["decode"].as_str() else {
            continue;
        };
        let name = record["name"].as_str().expect("a name");
        let bytes = fixture_bytes(records, record);
        let requests: Vec<(&String, Vec<u8>)> = match record["type"].as_str() {
            Some("operation") => {
                // A push of one operation record.
                let mut record_ref = vec![1];
                record_ref.extend((bytes.len() as u32).to_be_bytes());
                record_ref.extend(&bytes);
                vec![(&ops, list_of_one(&record_ref))]
            }
            Some("record_ref") => vec![(&ops, list_of_one(&bytes)), (&create, list_of_one(&bytes))],
            Some("push_request") => vec![(&ops, bytes.clone()), (&create, bytes)],
            Some("pull_request") => vec![(&pull, bytes)],
            _ => continue,
        };
        let code = decode_code(kind);
        for (route, body) in requests {
            let reply = owner.post(route, body).await;
            assert_eq!(
                (reply.status.as_u16(), reply.error().code.as_str()),
                (statuses[code], code),
                "{name} at {route}"
            );
            assert_eq!(reply.generation(), Some("1"), "{name} at {route}");
            sent += 1;
        }
    }
    assert!(sent >= 13, "the fixture's push and pull cases: {sent}");

    // A version 1 body, such as a v1 JSON tuple, decodes as nothing v2
    // defines.
    let v1 = records
        .iter()
        .find(|r| r["name"] == "v1_json_tuple")
        .expect("the v1 case");
    for route in [&ops, &create, &pull] {
        let reply = owner.post(route, fixture_bytes(records, v1)).await;
        assert_eq!(
            (reply.status.as_u16(), reply.error().code.as_str()),
            (statuses["bad_request"], "bad_request"),
            "a v1 body at {route}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_that_isnt_utf8_gets_an_error_body() {
    let (world, base, _dir) = world(1).await;
    let owner = client(&world, &base, 0);
    let w = world.workspace.to_base64url();
    let d = world.domain.to_base64url();
    let device = world.devices[0].id().to_base64url();
    for path in [
        "/v2/workspaces/%FF/policy".to_owned(),
        format!("/v2/workspaces/{w}/domains/{d}/heads/%C3%28"),
        format!("/v2/workspaces/{w}/domains/{d}/chain/{device}/%FF"),
        format!("/v2/workspaces/{w}/domains/%FF/epochs/1"),
    ] {
        let reply = owner.get(&path).await;
        assert_eq!(
            (reply.status, reply.error().code.as_str()),
            (StatusCode::BAD_REQUEST, "bad_request"),
            "{path}"
        );
        assert_eq!(reply.generation(), Some("1"));
    }
    let reply = owner.post("/v2/workspaces/%FF/ops", Vec::new()).await;
    assert_eq!(reply.error().code.as_str(), "bad_request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operation_refusals_carry_the_status_the_fixture_pins() {
    let statuses = fixture_statuses();
    let (mut world, base, _dir) = world(2).await;
    let owner = client(&world, &base, 0);
    let ops = format!("/v2/workspaces/{}/ops", world.workspace.to_base64url());
    let base_op = world.op(0, |_| {});
    let _: PushResponse = owner
        .post(&ops, push_body(std::slice::from_ref(&base_op)))
        .await
        .decode();
    let patch = world.op(1, |op| op.rid = base_op.body.rid);
    let _: PushResponse = client(&world, &base, 1)
        .post(&ops, push_body(&[patch]))
        .await
        .decode();
    let other = world.devices[1].id();
    let next = |world: &World<SqliteBackend>,
                edit: &dyn Fn(&mut sync_protocol::records::Operation)| {
        world.op(0, |op| {
            op.counter = 2;
            op.previous = base_op.digest();
            edit(op);
        })
    };
    let cases = vec![
        ("policy_stale", next(&world, &|op| op.policy_revision = 2)),
        (
            "policy_conflict",
            next(&world, &|op| op.policy_head = Digest([9; 32])),
        ),
        ("policy_regressed", next(&world, &|op| op.generation = 2)),
        ("epoch_stale", next(&world, &|op| op.epoch = 2)),
        (
            "checkpoint_unknown",
            next(&world, &|op| {
                op.checkpoint_seq = 2;
                op.checkpoint = Digest([5; 32]);
            }),
        ),
        (
            "chain_conflict",
            next(&world, &|op| op.previous = Digest([1; 32])),
        ),
        (
            "supersedes_unknown",
            next(&world, &|op| {
                op.supersedes = Sorted::new(vec![Cover {
                    device: op.device,
                    counter: 9,
                }])
                .expect("sorted");
            }),
        ),
        (
            "supersedes_incomplete",
            next(&world, &|op| {
                op.rid = base_op.body.rid;
                op.supersedes = Sorted::new(vec![Cover {
                    device: other,
                    counter: 1,
                }])
                .expect("sorted");
            }),
        ),
        (
            "padding_invalid",
            next(&world, &|op| op.ciphertext = Bytes(vec![0; 300])),
        ),
        (
            "too_large",
            next(&world, &|op| {
                op.ciphertext = Bytes(vec![0; MAX_OPERATION_CIPHERTEXT + 1])
            }),
        ),
        ("signature_invalid", {
            let mut op = next(&world, &|_| {});
            op.sig.0[0] ^= 1;
            op
        }),
    ];
    for (code, op) in cases {
        let reply = owner.post(&ops, push_body(&[op])).await;
        assert_eq!(reply.push_failure(), (code.to_owned(), 0), "{code}");
        assert_eq!(reply.status.as_u16(), statuses[code], "{code}");
        assert_eq!(reply.generation(), Some("1"));
    }

    // A rotation the removal of the second device requires.
    let removed = world.devices[1].id();
    let head = world
        .append_link(
            0,
            LinkKind::Remove {
                device: removed,
                keep: Sorted::default(),
            },
        )
        .await;
    let change = world.change(0, vec![head], PolicyKind::AccountHeads);
    let domain = world.domain;
    world.inject_policy(change, vec![domain], None).await;
    let reply = owner.post(&ops, push_body(&[next(&world, &|_| {})])).await;
    assert_eq!(reply.push_failure(), ("rotation_pending".to_owned(), 0));
    assert_eq!(reply.status.as_u16(), statuses["rotation_pending"]);
    // A domain that waits for its rotation takes no presence either.
    assert_eq!(presence_answer(&world, &base).await, "rotation_pending");
    // The link that removed the second device ended its session (§7).
    let reply = client(&world, &base, 1)
        .post(&ops, push_body(&[world.op(1, |_| {})]))
        .await;
    assert_eq!(reply.error().code.as_str(), "device_removed");
    assert_eq!(reply.status.as_u16(), statuses["device_removed"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_routes_answer_with_their_bodies() {
    let (world, base, _dir) = world(2).await;
    let device = client(&world, &base, 1);
    let w = world.workspace.to_base64url();
    let d = world.domain.to_base64url();
    let workspaces: WorkspacesResponse = device.get("/v2/workspaces").await.decode();
    assert_eq!(workspaces.workspaces[0].workspace, world.workspace);
    let policy: PolicyResponse = device
        .get(&format!("/v2/workspaces/{w}/policy?after=0"))
        .await
        .decode();
    assert_eq!((policy.head_revision, policy.changes.len()), (1, 1));
    let recipient = world.devices[1].id().to_base64url();
    let keys: KeysResponse = device
        .get(&format!(
            "/v2/workspaces/{w}/keys?after=0&recipient={recipient}"
        ))
        .await
        .decode();
    assert_eq!(keys.entries.len(), 2);
    assert!(matches!(keys.entries[0].record, KeyRecord::Epoch(_)));
    assert!(
        matches!(&keys.entries[1].record, KeyRecord::Envelope(e) if e.body.recipient == world.devices[1].id())
    );
    let epochs: EpochsResponse = device
        .get(&format!("/v2/workspaces/{w}/domains/{d}/epochs/1"))
        .await
        .decode();
    assert_eq!(epochs.epochs.len(), 1);

    let writer = world.devices[0].id().to_base64url();
    let reply = device
        .get(&format!("/v2/workspaces/{w}/domains/{d}/heads/{writer}"))
        .await;
    assert_eq!(
        (reply.status, reply.error().code.as_str()),
        (StatusCode::NOT_FOUND, "not_found")
    );
    let op = world.op(0, |_| {});
    let _: PushResponse = client(&world, &base, 0)
        .post(
            &format!("/v2/workspaces/{w}/ops"),
            push_body(std::slice::from_ref(&op)),
        )
        .await
        .decode();
    let head: HeadResponse = device
        .get(&format!("/v2/workspaces/{w}/domains/{d}/heads/{writer}"))
        .await
        .decode();
    assert_eq!(
        (head.counter, head.record_digest, head.seq),
        (1, op.digest(), 3)
    );
    let entry: sync_protocol::messages::Entry = device
        .get(&format!("/v2/workspaces/{w}/domains/{d}/chain/{writer}/1"))
        .await
        .decode();
    assert!(matches!(&entry.record, EntryRecord::Operation(o) if *o == op));
    let reply = device
        .get(&format!("/v2/workspaces/{w}/domains/{d}/chain/{writer}/01"))
        .await;
    assert_eq!(
        reply.error().code.as_str(),
        "bad_request",
        "integers carry no leading zeros"
    );
    let reply = device
        .get(&format!("/v2/workspaces/{w}/keys?after=0&after=1"))
        .await;
    assert_eq!(reply.error().code.as_str(), "bad_request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_pull_wakes_when_another_client_pushes() {
    let (world, base, _dir) = world(2).await;
    let w = world.workspace.to_base64url();
    let request = PullRequest {
        cursors: List(vec![PullCursor {
            domain: world.domain,
            after: 2,
        }]),
        wait_seconds: 25,
    };
    let reader = client(&world, &base, 1);
    let path = format!("/v2/workspaces/{w}/pull");
    let started = Instant::now();
    let waiting = tokio::spawn(async move { reader.post(&path, request.to_bytes()).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let op = world.op(0, |_| {});
    let _: PushResponse = client(&world, &base, 0)
        .post(
            &format!("/v2/workspaces/{w}/ops"),
            push_body(std::slice::from_ref(&op)),
        )
        .await
        .decode();
    let response: PullResponse = waiting.await.expect("the pull returns").decode();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "woke after {:?}",
        started.elapsed()
    );
    let page = &response.pages[0];
    assert_eq!(page.entries.len(), 1);
    assert!(matches!(&page.entries[0].record, EntryRecord::Operation(o) if *o == op));
}
