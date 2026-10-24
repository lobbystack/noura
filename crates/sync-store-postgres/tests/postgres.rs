//! What PostgreSQL mode adds to the conformance suite (§15): queued pushes
//! into one domain commit together with a savepoint each; pushes into
//! different workspaces, and into different domains of one workspace,
//! commit in parallel while a policy change waits; the policy lock writes
//! no row; no push holds or waits for the notification lock, or for a
//! connection that does; a push reads the state a policy change it waited
//! for left, whatever the database's default isolation; and a pull waiting
//! on one instance wakes when another commits, and only then.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::{Connection, PgConnection};
use sync_protocol::codec::List;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{EntryRecord, PullCursor, PullRequest};
use sync_protocol::records::{PolicyChange, PolicyKind, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};
use sync_store::conformance::{World, head_of, manager};
use sync_store::{Backend, INITIAL_RESTORE_GENERATION, LockPlan, Store, WriteTxn};
use sync_store_postgres::testing::{Cluster, TestDatabase};
use sync_store_postgres::{CONNECTIONS, PgBackend};

fn tmp() -> &'static Path {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
}

async fn migrated(cluster: &Arc<Cluster>) -> TestDatabase {
    let database = cluster.database().await;
    sync_store_postgres::migrate(&database.url)
        .await
        .expect("migrated");
    sync_store_postgres::store_secret_check(&database.url, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    database
}

async fn instance(url: &str) -> Store<PgBackend> {
    Store::open(PgBackend::open(url).await.expect("opened"))
        .await
        .expect("store")
}

/// The row versions of a workspace's row and key-sequence row.
async fn versions(connection: &mut PgConnection, workspace: &[u8]) -> (String, String) {
    sqlx::query_as(
        "SELECT w.xmin::text, k.xmin::text FROM workspaces w
         JOIN key_sequences k ON k.workspace = w.workspace WHERE w.workspace = $1",
    )
    .bind(workspace)
    .fetch_one(connection)
    .await
    .expect("the rows")
}

async fn open() -> (Store<PgBackend>, TestDatabase) {
    let database = migrated(&Cluster::shared(tmp())).await;
    (instance(&database.url).await, database)
}

/// A change that creates a restricted domain, which the world's account
/// manages, and the domain's epoch 1.
fn create_domain(
    world: &World<PgBackend>,
) -> (
    Id,
    Signed<PolicyChange>,
    Signed<sync_protocol::records::Epoch>,
) {
    let domain = Id::random().expect("random");
    let change = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::CreateDomain {
            domain,
            marker_rid: Id::random().expect("random"),
            access: manager(world.account),
        },
    );
    let creator = &world.devices[0];
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: world.workspace,
                domain,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: change.body.revision,
                policy_head: change.digest(),
                rotating_device: creator.id(),
                recipients: std::collections::BTreeSet::from([creator.id()]),
            },
            &SecretKey::random().expect("random"),
            None,
            Nonce::ZERO,
        ),
        creator.keys.signing(),
    );
    (domain, change, epoch)
}

/// Adds a restricted domain with its epoch 1, which the world's account
/// manages.
async fn add_domain(world: &mut World<PgBackend>) -> Id {
    let (domain, change, epoch) = create_domain(world);
    let existing = vec![world.domain];
    world
        .inject_policy(change, existing, Some((domain, epoch)))
        .await;
    domain
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_pushes_into_one_domain_commit_together_with_a_savepoint_each() {
    let (store, _database) = open().await;
    let world = Arc::new(World::new(store, 12).await);
    let before = world.store.backend().writer_counts();
    let mut tasks = Vec::new();
    for device in 0..12 {
        let world = world.clone();
        tasks.push(tokio::spawn(async move {
            let mut previous = Digest::ZERO;
            for counter in 1..=10u64 {
                let good = world.op(device, |op| {
                    op.counter = counter;
                    op.previous = previous;
                });
                // Every device's third push carries a record that fails
                // after a good one; that push commits nothing.
                if counter % 3 == 0 {
                    let bad = world.op(device, |op| {
                        op.counter = counter + 1;
                        op.previous = good.digest();
                        op.epoch = 9;
                    });
                    world
                        .push(device, &[good.clone(), bad])
                        .await
                        .expect_err("the failing record rolls its push back");
                }
                world
                    .push(device, std::slice::from_ref(&good))
                    .await
                    .expect("committed");
                previous = good.digest();
            }
        }));
    }
    for task in tasks {
        task.await.expect("finished");
    }
    let after = world.store.backend().writer_counts();
    let jobs = after.jobs - before.jobs;
    let transactions = after.transactions - before.transactions;
    assert_eq!(jobs, 12 * 13, "every push ran");
    assert!(
        transactions < jobs,
        "{jobs} pushes in {transactions} transactions: queued pushes commit in groups"
    );
    let page = world.pull(0, 0).await;
    assert_eq!(page.head, 2 + 12 * 10, "the good records and no failed one");
}

/// Runs `push` and `other` at once and returns how long both took.
async fn together(
    push: impl std::future::Future<Output = ()>,
    other: impl std::future::Future<Output = ()>,
) -> Duration {
    let started = Instant::now();
    tokio::join!(push, other);
    started.elapsed()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pushes_commit_in_parallel_unless_one_changes_the_policy() {
    let database = migrated(&Cluster::shared(tmp())).await;
    let store = Arc::new(instance(&database.url).await);
    let mut first = World::on(store.clone(), 1).await;
    let restricted = add_domain(&mut first).await;
    let second = World::on(store.clone(), 1).await;
    // Every write now holds its locks for 600 ms after its policy read.
    let delay = Duration::from_millis(600);
    store.backend().inject_policy_read_delay(Some(delay));
    let serial = delay * 2 - Duration::from_millis(100);

    let a = first.op(0, |_| {});
    let b = second.op(0, |_| {});
    let elapsed = together(
        async {
            drop(
                first
                    .push(0, std::slice::from_ref(&a))
                    .await
                    .expect("pushed"),
            )
        },
        async { drop(second.push(0, &[b]).await.expect("pushed")) },
    )
    .await;
    assert!(
        elapsed < serial,
        "pushes into two workspaces took {elapsed:?}"
    );

    let c = first.op(0, |op| {
        op.counter = 2;
        op.previous = a.digest();
    });
    let d = first.op(0, |op| op.domain = restricted);
    let elapsed = together(
        async {
            drop(
                first
                    .push(0, std::slice::from_ref(&c))
                    .await
                    .expect("pushed"),
            )
        },
        async { drop(first.push(0, &[d]).await.expect("pushed")) },
    )
    .await;
    assert!(
        elapsed < serial,
        "pushes into two domains of one workspace took {elapsed:?}"
    );

    // A policy change waits for the push that holds the shared lock.
    let e = first.op(0, |op| {
        op.counter = 3;
        op.previous = c.digest();
    });
    let workspace = first.workspace;
    let pushes = [e];
    let (pushed, waited) = tokio::join!(first.push(0, &pushes), async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let started = Instant::now();
        store
            .backend()
            .write(Box::new(move |txn: &mut sync_store_postgres::PgTxn| {
                Box::pin(async move {
                    txn.lock(&LockPlan {
                        workspace: Some(workspace),
                        policy_exclusive: true,
                        ..LockPlan::default()
                    })
                    .await
                })
            }))
            .await
            .expect("the policy lock");
        started.elapsed()
    });
    pushed.expect("pushed");
    assert!(
        waited >= Duration::from_millis(300),
        "the policy change waited {waited:?} for the push in flight"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_policy_lock_writes_no_row() {
    let (store, database) = open().await;
    let world = World::new(store, 1).await;
    let mut connection = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    let workspace = world.workspace.0.to_vec();
    let before = versions(&mut connection, &workspace).await;
    let mut previous = Digest::ZERO;
    for counter in 1..=5 {
        let op = world.op(0, |op| {
            op.counter = counter;
            op.previous = previous;
        });
        previous = op.digest();
        world.push(0, &[op]).await.expect("pushed");
    }
    assert_eq!(
        versions(&mut connection, &workspace).await,
        before,
        "pushes leave the workspace and key-sequence rows unwritten"
    );
    connection.close().await.expect("closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_push_holds_the_notification_lock() {
    // Sessions that ask for synchronous commit wait for a standby that
    // never comes, holding the notification lock if their transaction sent
    // a notification; the store's sessions commit locally. The blocker
    // writes a row, so its commit has a record to wait on.
    let cluster = Arc::new(Cluster::start(
        tmp(),
        &[
            ("synchronous_standby_names", "'nobody'"),
            ("synchronous_commit", "local"),
        ],
    ));
    let database = migrated(&cluster).await;
    let world = World::new(instance(&database.url).await, 1).await;

    let mut blocker = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    let (pid,): (i32,) = sqlx::query_as("SELECT pg_backend_pid()")
        .fetch_one(&mut blocker)
        .await
        .expect("pid");
    let blocked = tokio::spawn(async move {
        sqlx::raw_sql(
            "CREATE TABLE blocker (x INT); SET synchronous_commit = on;
             BEGIN; INSERT INTO blocker VALUES (1); NOTIFY noura_blocker; COMMIT;",
        )
        .execute(&mut blocker)
        .await
        .map(|_| ())
    });
    let mut observer = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // The notification lock: an object lock on database 0 of class
        // pg_database.
        let (holding,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
               SELECT 1 FROM pg_locks
               WHERE pid = $1 AND locktype = 'object' AND database = 0
                 AND classid = 'pg_database'::regclass AND objid = 0
                 AND mode = 'AccessExclusiveLock' AND granted
             )",
        )
        .bind(pid)
        .fetch_one(&mut observer)
        .await
        .expect("locks");
        if holding {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the blocker never held the notification lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A transaction that notifies can't commit now.
    let mut control = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    let refused = sqlx::raw_sql("SET lock_timeout = '300ms'; BEGIN; NOTIFY noura_control; COMMIT;")
        .execute(&mut control)
        .await;
    assert!(
        refused.is_err(),
        "a notification inside a transaction waits for the notification lock"
    );

    // Pushes commit and answer all the same, more of them than the
    // instance has connections, while its announcer waits for the lock.
    let mut previous = Digest::ZERO;
    let pushes = CONNECTIONS as u64 + 8;
    for counter in 1..=pushes {
        let op = world.op(0, |op| {
            op.counter = counter;
            op.previous = previous;
        });
        previous = op.digest();
        tokio::time::timeout(Duration::from_secs(5), world.push(0, &[op]))
            .await
            .expect("the push waits neither for the notification lock nor for a connection")
            .expect("pushed");
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (waiting,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
               SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid
               WHERE a.application_name = 'noura-sync-server'
                 AND l.locktype = 'object' AND l.database = 0
                 AND l.classid = 'pg_database'::regclass AND l.objid = 0
                 AND NOT l.granted
             )",
        )
        .fetch_one(&mut observer)
        .await
        .expect("locks");
        if waiting {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the announcer never waited for the notification lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let page = tokio::time::timeout(Duration::from_secs(5), world.pull(0, 0))
        .await
        .expect("a pull gets a connection");
    assert_eq!(page.head, 2 + pushes);
    tokio::time::timeout(Duration::from_secs(5), world.store.backend().close())
        .await
        .expect("the backend closes while the lock is held");

    sqlx::query("SELECT pg_cancel_backend($1)")
        .bind(pid)
        .execute(&mut observer)
        .await
        .expect("the blocker is cancelled");
    let _ = blocked.await;
    observer.close().await.expect("closed");
}

/// How many advisory lock requests wait in the connection's database.
async fn waiting_advisory_locks(connection: &mut PgConnection) -> i64 {
    let (waiting,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_locks
         WHERE locktype = 'advisory' AND NOT granted
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(connection)
    .await
    .expect("locks");
    waiting
}

async fn until_waiting(connection: &mut PgConnection, count: i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while waiting_advisory_locks(connection).await < count {
        assert!(
            Instant::now() < deadline,
            "{count} requests never waited for the policy lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_reads_the_state_a_policy_change_it_waited_for_left() {
    // With a default of REPEATABLE READ, a push that names no isolation
    // takes its snapshot before it waits for the policy lock, and then
    // meets the head row the change rewrote. The store names READ
    // COMMITTED, so the push reads after the lock.
    let database = Cluster::shared(tmp()).database().await;
    let mut observer = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    sqlx::raw_sql(
        "DO $$ BEGIN EXECUTE format(
           'ALTER DATABASE %I SET default_transaction_isolation = %L',
           current_database(), 'repeatable read'); END $$",
    )
    .execute(&mut observer)
    .await
    .expect("the default changes");
    sync_store_postgres::migrate(&database.url)
        .await
        .expect("migrated");
    sync_store_postgres::store_secret_check(&database.url, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    let world = Arc::new(World::new(instance(&database.url).await, 1).await);

    // Another session holds the workspace's policy lock, so a change
    // queues for it, then a push behind the change.
    let mut holder = PgConnection::connect(&database.url)
        .await
        .expect("connects");
    let mut key = [0; 8];
    key.copy_from_slice(&world.workspace.0[..8]);
    sqlx::raw_sql("BEGIN")
        .execute(&mut holder)
        .await
        .expect("begun");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(i64::from_be_bytes(key))
        .execute(&mut holder)
        .await
        .expect("the policy lock");
    let (domain, change, epoch) = create_domain(&world);
    let changing = {
        let store = world.store.clone();
        let existing = vec![world.domain];
        tokio::spawn(async move {
            sync_store::conformance::inject_change(&store, change, existing, Some((domain, epoch)))
                .await
        })
    };
    until_waiting(&mut observer, 1).await;
    let pushing = {
        let world = world.clone();
        tokio::spawn(async move { world.push(0, &[world.op(0, |_| {})]).await })
    };
    until_waiting(&mut observer, 2).await;
    sqlx::raw_sql("COMMIT")
        .execute(&mut holder)
        .await
        .expect("committed");

    changing
        .await
        .expect("the change ends")
        .expect("the change commits");
    let pushed = pushing
        .await
        .expect("the push ends")
        .expect("the push commits after the change");
    // The genesis and epoch entries, the change's policy entry, then the
    // push's record.
    assert_eq!(pushed.receipts[0].seq, 4);
    assert_eq!(pushed.policy_revision, 2);
    holder.close().await.expect("closed");
    observer.close().await.expect("closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_on_one_instance_wakes_when_another_commits_and_only_then() {
    let database = migrated(&Cluster::shared(tmp())).await;
    let first = Arc::new(instance(&database.url).await);
    let second = Arc::new(instance(&database.url).await);
    let world = Arc::new(World::on(first, 2).await);
    let wait_on_second = |after: u64, seconds: u32| {
        let world = world.clone();
        let second = second.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let response = second
                .pull(
                    world.devices[1].session,
                    world.workspace,
                    PullRequest {
                        cursors: List(vec![PullCursor {
                            domain: world.domain,
                            after,
                        }]),
                        wait_seconds: seconds,
                    },
                )
                .await
                .expect("pull");
            (started.elapsed(), response)
        })
    };

    // A push that rolls back wakes nobody.
    let waiting = wait_on_second(2, 3);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let good = world.op(0, |_| {});
    let bad = world.op(0, |op| {
        op.counter = 2;
        op.previous = Digest([7; 32]);
    });
    world
        .push(0, &[good.clone(), bad])
        .await
        .expect_err("the push rolls back");
    let (elapsed, response) = waiting.await.expect("the pull returns");
    assert!(response.pages[0].entries.is_empty());
    assert!(
        elapsed >= Duration::from_millis(2_900),
        "a rolled-back push woke the pull after {elapsed:?}"
    );

    // A push that commits on the first instance wakes the second's pull.
    let waiting = wait_on_second(2, 20);
    tokio::time::sleep(Duration::from_millis(300)).await;
    world
        .push(0, std::slice::from_ref(&good))
        .await
        .expect("pushed");
    let (elapsed, response) = waiting.await.expect("the pull returns");
    assert!(elapsed < Duration::from_secs(10), "woke after {elapsed:?}");
    let entries = &response.pages[0].entries;
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].record, EntryRecord::Operation(op) if *op == good));
}
