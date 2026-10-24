//! The PostgreSQL push soak (§15, review item M20): 200 devices each push a
//! record every 500 ms into one workspace domain through two instances on
//! one database, 400 pushes a second in all, and every push answers within
//! its tick. Beside them, one device pushes into a second domain of that
//! workspace and one into a second workspace at the same pace, and the
//! soak reports their latencies apart from the hot domain's: their commits
//! run beside the hot domain's, not behind them. `NOURA_SOAK_SECONDS` sets
//! how long the devices push and makes the run hold that rate and latency
//! in every domain. Without it the workspace tests push for 5 seconds,
//! report the rate and check only that every push commits and both
//! instances serve every record in one order; CI's soak job sets it to 300.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::{Account, Client, Created, push_body, seed_store};
use sync_protocol::codec::{Encode, List};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{EntryRecord, PullCursor, PullRequest, PullResponse, PushResponse};
use sync_protocol::records::{Epoch, PolicyChange, PolicyKind, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};
use sync_store::conformance::{head_of, inject_change, manager};
use sync_store_postgres::testing::Cluster;

/// The devices that push into the hot domain.
const DEVICES: usize = 200;
/// The device that pushes into the workspace's second domain, then the one
/// that pushes into the second workspace.
const OTHER_DOMAIN: usize = DEVICES;
const OTHER_WORKSPACE: usize = DEVICES + 1;
const TICK: Duration = Duration::from_millis(500);

/// A change that creates a second domain of `created`'s workspace, which
/// the account manages, and that domain's epoch 1.
fn second_domain(
    account: &Account,
    created: &Created,
) -> (Id, Signed<PolicyChange>, Signed<Epoch>) {
    let creator = &account.keys[0];
    let domain = Id::random().expect("random");
    let change = Signed::sign(
        PolicyChange {
            workspace: created.workspace,
            revision: 2,
            previous: created.genesis.digest(),
            signer: creator.device(),
            heads: sync_protocol::codec::Sorted::new(vec![head_of(&account.links)])
                .expect("one head"),
            kind: PolicyKind::CreateDomain {
                domain,
                marker_rid: Id::random().expect("random"),
                access: manager(account.account),
            },
        },
        creator.signing(),
    );
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: created.workspace,
                domain,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: 2,
                policy_head: change.digest(),
                rotating_device: creator.device(),
                recipients: std::collections::BTreeSet::from([creator.device()]),
            },
            &SecretKey::random().expect("random"),
            None,
            Nonce::ZERO,
        ),
        creator.signing(),
    );
    (domain, change, epoch)
}

/// The 99th percentile of `latencies`, which it sorts.
fn p99(latencies: &mut [Duration]) -> Duration {
    latencies.sort();
    latencies[latencies.len() * 99 / 100]
}

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
    let account = Arc::new(seed_store(&first, DEVICES + 2).await);
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
    let other = Arc::new(account.creation());
    for workspace in [&created, &other] {
        let _: PushResponse = Client::new(bases[0].clone(), Some(account.token(0)))
            .post("/v2/workspaces", workspace.body.clone())
            .await
            .decode();
    }
    // Until tickets 47 and 50 add policy pushes, the change goes straight
    // into the store.
    let (domain, change, epoch) = second_domain(&account, &created);
    let policy_head = change.digest();
    inject_change(&first, change, vec![created.domain], Some((domain, epoch)))
        .await
        .expect("the second domain");
    // Each instance verifies each workspace's policy chain once before the
    // devices start, so the soak measures pushes rather than that check.
    for base in &bases {
        for workspace in [&created, &other] {
            let reply = Client::new(base.clone(), Some(account.token(0)))
                .get(&format!(
                    "/v2/workspaces/{}/policy?after=0",
                    workspace.workspace.to_base64url()
                ))
                .await;
            let _: sync_protocol::messages::PolicyResponse = reply.decode();
        }
    }

    let length = Duration::from_secs(seconds());
    let started = tokio::time::Instant::now();
    let mut devices = Vec::new();
    for device in 0..DEVICES + 2 {
        let account = account.clone();
        let workspace = if device == OTHER_WORKSPACE {
            other.clone()
        } else {
            created.clone()
        };
        let client = Client::new(bases[device % 2].clone(), Some(account.token(device)));
        devices.push(tokio::spawn(async move {
            let path = format!("/v2/workspaces/{}/ops", workspace.workspace.to_base64url());
            // The devices' ticks spread over the 500 ms.
            let mut tick = started + TICK * device as u32 / (DEVICES + 2) as u32;
            let mut previous = Digest::ZERO;
            let mut latencies = Vec::new();
            let mut counter = 0;
            while tick < started + length {
                tokio::time::sleep_until(tick).await;
                counter += 1;
                let op = workspace.op(&account.keys[device], counter, previous, |op| {
                    if device == OTHER_DOMAIN {
                        op.domain = domain;
                        op.policy_revision = 2;
                        op.policy_head = policy_head;
                    }
                });
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
    let mut ended = Vec::new();
    for device in devices {
        ended.push(device.await.expect("the device ends"));
    }
    let mut other_workspace = ended.pop().expect("the second workspace's device");
    let mut other_domain = ended.pop().expect("the second domain's device");
    for device in ended {
        latencies.extend(device);
    }
    let elapsed = started.elapsed();
    let pushes = latencies.len();
    let rate = pushes as f64 / elapsed.as_secs_f64();
    let hot = p99(&mut latencies);
    let slowest = latencies[pushes - 1];
    let (beside, apart) = (p99(&mut other_domain), p99(&mut other_workspace));
    let counts = [
        first.backend().writer_counts(),
        second.backend().writer_counts(),
    ];
    eprintln!(
        "soak: {pushes} pushes from {DEVICES} devices in {elapsed:?}, {rate:.0} a second; \
         p99 {hot:?}, slowest {slowest:?}; {} write jobs in {} transactions; \
         p99 in the workspace's second domain {beside:?}, in the second workspace {apart:?}",
        counts[0].jobs + counts[1].jobs,
        counts[0].transactions + counts[1].transactions
    );
    let ticks = length.as_millis() as u64 / 500;
    assert_eq!(pushes as u64, DEVICES as u64 * ticks);
    assert_eq!(other_domain.len() as u64, ticks);
    assert_eq!(other_workspace.len() as u64, ticks);
    // The short run in the workspace tests shares the computer with other
    // test binaries, so only a run that sets its length holds the rate.
    if std::env::var_os("NOURA_SOAK_SECONDS").is_some() {
        assert!(rate >= 380.0, "{rate:.0} pushes a second");
        assert!(hot < TICK, "the 99th percentile push took {hot:?}");
        assert!(
            beside < TICK && apart < TICK,
            "the 99th percentile push took {beside:?} in the second domain \
             and {apart:?} in the second workspace"
        );
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
