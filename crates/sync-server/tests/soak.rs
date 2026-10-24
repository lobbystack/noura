//! The PostgreSQL push soak (§15, review item M20): 200 devices each push a
//! record every 500 ms into one workspace domain through two instances on
//! one database, 400 pushes a second in all, and every push answers within
//! its tick. `NOURA_SOAK_SECONDS` sets how long the devices push and makes
//! the run hold that rate and latency. Without it the workspace tests push
//! for 5 seconds, report the rate and check only that both instances serve
//! every record in one order; CI's soak job sets it to 300.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::{Client, push_body, seed_store};
use sync_protocol::codec::{Encode, List};
use sync_protocol::messages::{EntryRecord, PullCursor, PullRequest, PullResponse, PushResponse};
use sync_protocol::types::{Digest, SecretKey};
use sync_store_postgres::testing::Cluster;

const DEVICES: usize = 200;
const TICK: Duration = Duration::from_millis(500);

fn seconds() -> u64 {
    std::env::var("NOURA_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_instances_sustain_400_pushes_a_second_into_one_domain() {
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
    let first = Arc::new(
        sync_server::postgres::open_store(&postgres)
            .await
            .expect("the first instance"),
    );
    let second = Arc::new(
        sync_server::postgres::open_store(&postgres)
            .await
            .expect("the second instance"),
    );
    let account = Arc::new(seed_store(&first, DEVICES).await);
    let bases = [
        format!(
            "http://{}",
            sync_server::spawn(first.clone(), "127.0.0.1:0")
                .await
                .expect("serving")
        ),
        format!(
            "http://{}",
            sync_server::spawn(second.clone(), "127.0.0.1:0")
                .await
                .expect("serving")
        ),
    ];
    let created = Arc::new(account.creation());
    let _: PushResponse = Client::new(bases[0].clone(), Some(account.token(0)))
        .post("/v2/workspaces", created.body.clone())
        .await
        .decode();

    let length = Duration::from_secs(seconds());
    let started = tokio::time::Instant::now();
    let mut devices = Vec::new();
    for device in 0..DEVICES {
        let account = account.clone();
        let created = created.clone();
        let client = Client::new(bases[device % 2].clone(), Some(account.token(device)));
        devices.push(tokio::spawn(async move {
            let path = format!("/v2/workspaces/{}/ops", created.workspace.to_base64url());
            // The devices' ticks spread over the 500 ms.
            let mut tick = started + TICK * device as u32 / DEVICES as u32;
            let mut previous = Digest::ZERO;
            let mut latencies = Vec::new();
            let mut counter = 0;
            while tick < started + length {
                tokio::time::sleep_until(tick).await;
                counter += 1;
                let op = created.op(&account.keys[device], counter, previous, |_| {});
                let sent = Instant::now();
                let reply = client
                    .post(&path, push_body(std::slice::from_ref(&op)))
                    .await;
                latencies.push(sent.elapsed());
                let _: PushResponse = reply.decode();
                previous = op.digest();
                tick += TICK;
            }
            latencies
        }));
    }
    let mut latencies = Vec::new();
    for device in devices {
        latencies.extend(device.await.expect("the device ends"));
    }
    let elapsed = started.elapsed();
    latencies.sort();
    let pushes = latencies.len();
    let rate = pushes as f64 / elapsed.as_secs_f64();
    let p99 = latencies[pushes * 99 / 100];
    let slowest = latencies[pushes - 1];
    let counts = [
        first.backend().writer_counts(),
        second.backend().writer_counts(),
    ];
    eprintln!(
        "soak: {pushes} pushes from {DEVICES} devices in {elapsed:?}, {rate:.0} a second; \
         p99 {p99:?}, slowest {slowest:?}; {} write jobs in {} transactions",
        counts[0].jobs + counts[1].jobs,
        counts[0].transactions + counts[1].transactions
    );
    assert_eq!(
        pushes as u64,
        DEVICES as u64 * length.as_millis() as u64 / 500
    );
    // The short run in the workspace tests shares the computer with other
    // test binaries, so only a run that sets its length holds the rate.
    if std::env::var_os("NOURA_SOAK_SECONDS").is_some() {
        assert!(rate >= 380.0, "{rate:.0} pushes a second");
        assert!(p99 < TICK, "the 99th percentile push took {p99:?}");
    }

    // Both instances serve every record, in one order.
    let mut logs = Vec::new();
    for (index, base) in bases.iter().enumerate() {
        let client = Client::new(base.clone(), Some(account.token(index)));
        let mut seqs = Vec::new();
        let mut after = 0;
        loop {
            let response: PullResponse = client
                .post(
                    &format!("/v2/workspaces/{}/pull", created.workspace.to_base64url()),
                    PullRequest {
                        cursors: List(vec![PullCursor {
                            domain: created.domain,
                            after,
                        }]),
                        wait_seconds: 0,
                    }
                    .to_bytes(),
                )
                .await
                .decode();
            let page = &response.pages[0];
            for entry in page.entries.iter() {
                if let EntryRecord::Operation(op) = &entry.record {
                    seqs.push((entry.seq, op.digest()));
                }
            }
            after = page.complete_through;
            if after >= page.head {
                break;
            }
        }
        logs.push(seqs);
    }
    assert_eq!(logs[0], logs[1], "both instances serve one order");
    assert_eq!(logs[0].len(), pushes);
}
