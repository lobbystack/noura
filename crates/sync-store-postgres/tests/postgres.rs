//! What PostgreSQL mode adds to the conformance suite (§15): queued pushes
//! into one domain commit together with a savepoint each; pushes into
//! different workspaces, and into different domains of one workspace,
//! commit in parallel while a policy change waits; the policy lock writes
//! no row; no push holds or waits for the notification lock, or for a
//! connection that does; a push reads the state a policy change it waited
//! for left, whatever the database's default isolation; a pull waiting on
//! one instance wakes when another commits, and only then; a push of a new
//! record looks up no position in `operations`; a grouped job keeps its
//! locks only once its savepoint is released; and an operation updates its
//! head row once.

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
use sync_store::{
    Backend, Error, INITIAL_RESTORE_GENERATION, LockPlan, ReadTxn, Store, WriteGroup, WriteJob,
    WriteTxn,
};
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

/// A record above its device's chain head can't be a retry, so its push
/// looks nothing up by position. Until ANALYZE first reaches `operations`,
/// the planner costs every index on its workspace and domain alike and may
/// serve that lookup from a narrower one than `operations_by_position`,
/// which walks the domain's whole log under its head row on each push. The
/// scan counters show the lookup whichever plan the database picked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_record_looks_up_no_position() {
    const PUSHES: i64 = 4;
    let (store, database) = open().await;
    let world = World::new(store, 1).await;
    let mut previous = Digest::ZERO;
    for counter in 1..=PUSHES as u64 {
        let op = world.op(0, |op| {
            op.counter = counter;
            op.previous = previous;
        });
        world
            .push(0, std::slice::from_ref(&op))
            .await
            .expect("pushed");
        previous = op.digest();
    }
    // A backend hands in its counters as it exits.
    world.store.backend().close().await;
    let mut connection = PgConnection::connect(&database.url)
        .await
        .expect("connected");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (inserted, scans): (i64, i64) = sqlx::query_as(
            "SELECT t.n_tup_ins, COALESCE(SUM(i.idx_scan), 0)::bigint
             FROM pg_stat_user_tables t
             LEFT JOIN pg_stat_user_indexes i ON i.relid = t.relid
               AND i.indexrelname <> 'operations_by_rid'
             WHERE t.relname = 'operations' GROUP BY t.n_tup_ins",
        )
        .fetch_one(&mut connection)
        .await
        .expect("the counters");
        if inserted == PUSHES {
            assert_eq!(scans, 0, "a push read `operations` by position");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{inserted} inserts counted of {PUSHES}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether another connection finds the head row of `domain` locked.
async fn head_row_locked(url: &str, workspace: Id, domain: Id) -> bool {
    let mut connection = PgConnection::connect(url).await.expect("connected");
    let locked =
        sqlx::query("SELECT 1 FROM domains WHERE workspace = $1 AND domain = $2 FOR UPDATE NOWAIT")
            .bind(&workspace.0[..])
            .bind(&domain.0[..])
            .fetch_optional(&mut connection)
            .await;
    match locked {
        Ok(_) => false,
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => true,
        Err(error) => panic!("the probe failed: {error}"),
    }
}

/// A group's later jobs skip the lock statements only for locks a released
/// savepoint left with the transaction. On this one-thread runtime the
/// group's worker starts after all four jobs queued, so they share one
/// transaction.
#[tokio::test]
async fn a_rolled_back_job_leaves_the_next_one_no_locks() {
    type Job = WriteJob<<PgBackend as Backend>::Write, (bool, bool)>;
    let (store, database) = open().await;
    let world = World::new(store, 1).await;
    let (workspace, domain) = (world.workspace, world.domain);
    let plan = LockPlan {
        workspace: Some(workspace),
        domains: vec![domain],
        ..LockPlan::default()
    };
    let job = |lock: bool, keep: bool| -> Job {
        let (plan, url) = (plan.clone(), database.url.clone());
        Box::new(move |txn| {
            Box::pin(async move {
                let found = head_row_locked(&url, workspace, domain).await;
                if lock {
                    txn.lock(&plan).await?;
                }
                let held = head_row_locked(&url, workspace, domain).await;
                if keep {
                    Ok((found, held))
                } else {
                    Err(Error::internal(format!("rolled back, row held: {held}")))
                }
            })
        })
    };
    let backend = world.store.backend();
    let group = WriteGroup { workspace, domain };
    let before = backend.writer_counts();
    let (first, second, third, fourth) = tokio::join!(
        // Rolled back, so its locks go with its savepoint.
        backend.write_grouped(group, job(true, false)),
        // Found nothing locked; takes the locks and keeps them.
        backend.write_grouped(group, job(true, true)),
        // Plans the same locks, sends no statement, and rolls back.
        backend.write_grouped(group, job(true, false)),
        // The transaction still holds what the second job kept.
        backend.write_grouped(group, job(false, true)),
    );
    let after = backend.writer_counts();
    assert_eq!(after.transactions - before.transactions, 1, "one group");
    let held = "rolled back, row held: true";
    assert!(matches!(first, Err(Error::Internal(ref e)) if e == held));
    assert_eq!(second.expect("kept"), (false, true), "the row came back");
    assert!(matches!(third, Err(Error::Internal(ref e)) if e == held));
    assert_eq!(fourth.expect("kept"), (true, true), "the row stayed held");
    assert!(
        !head_row_locked(&database.url, workspace, domain).await,
        "the commit let go of the row"
    );
}

/// A group reads the workspace's row once, and sees its own quota change.
#[tokio::test]
async fn a_group_rereads_the_workspace_after_a_quota_change() {
    type Job = WriteJob<<PgBackend as Backend>::Write, bool>;
    let (store, _database) = open().await;
    let world = World::new(store, 1).await;
    let (workspace, domain) = (world.workspace, world.domain);
    let plan = LockPlan {
        workspace: Some(workspace),
        domains: vec![domain],
        ..LockPlan::default()
    };
    let job = |raise: bool| -> Job {
        let plan = plan.clone();
        Box::new(move |txn| {
            Box::pin(async move {
                txn.lock(&plan).await?;
                txn.workspace(workspace).await?;
                if raise {
                    txn.set_quota_pressure(workspace, true).await?;
                }
                let row = txn.workspace(workspace).await?.expect("a workspace");
                Ok(row.quota_pressure)
            })
        })
    };
    let backend = world.store.backend();
    let group = WriteGroup { workspace, domain };
    let (first, second, third) = tokio::join!(
        backend.write_grouped(group, job(false)),
        backend.write_grouped(group, job(true)),
        backend.write_grouped(group, job(false)),
    );
    assert!(!first.expect("kept"), "no pressure yet");
    assert!(second.expect("kept"), "the job sees its own write");
    assert!(third.expect("kept"), "the next job sees it too");
}

/// A push's head row changes go in one update, so a busy domain's row grows
/// one version per operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operation_updates_its_head_row_once() {
    const PUSHES: i64 = 20;
    let (store, database) = open().await;
    let world = World::new(store, 1).await;
    let mut previous = Digest::ZERO;
    for counter in 1..=PUSHES as u64 {
        let op = world.op(0, |op| {
            op.counter = counter;
            op.previous = previous;
        });
        world
            .push(0, std::slice::from_ref(&op))
            .await
            .expect("pushed");
        previous = op.digest();
    }
    // A backend hands in its counters as it exits.
    world.store.backend().close().await;
    let mut connection = PgConnection::connect(&database.url)
        .await
        .expect("connected");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (inserted, updated): (i64, i64) = sqlx::query_as(
            "SELECT o.n_tup_ins, d.n_tup_upd
             FROM pg_stat_user_tables o, pg_stat_user_tables d
             WHERE o.relname = 'operations' AND d.relname = 'domains'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("the counters");
        if inserted == PUSHES {
            // The workspace's creation updated it a few times too.
            assert!(
                updated < 2 * PUSHES,
                "{updated} updates of the head row in {PUSHES} pushes"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{inserted} inserts counted of {PUSHES}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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

/// Raising the restore generation gives each completed blob no operation
/// lists its fresh time in the same transaction, and leaves listed, marked
/// and incomplete blobs alone (§19).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raising_the_generation_refreshes_unlisted_blobs() {
    let cluster = Cluster::shared(tmp());
    let database = migrated(&cluster).await;
    let mut connection = PgConnection::connect(&database.url)
        .await
        .expect("connected");
    let workspace = &[7u8; 16][..];
    for (blob, complete, marked) in [
        (1u8, true, false),
        (2, true, false),
        (3, true, true),
        (4, false, false),
    ] {
        sqlx::query(
            "INSERT INTO blobs (workspace, blob, domain, device, length, digest, upload_offset,
                                complete, delete_after, marked, live_listings)
             VALUES ($1, $2, $1, $1, 4096, $1, 0, $3, 5, $4, 0)",
        )
        .bind(workspace)
        .bind(&[blob; 16][..])
        .bind(complete)
        .bind(marked)
        .execute(&mut connection)
        .await
        .expect("a blob");
    }
    sqlx::query("INSERT INTO blob_listings (workspace, blob, domain, seq) VALUES ($1, $2, $1, 1)")
        .bind(workspace)
        .bind(&[2u8; 16][..])
        .execute(&mut connection)
        .await
        .expect("a listing");

    sync_store_postgres::raise_generation(&database.url, 1, 900)
        .await
        .expect("raised");

    let rows: Vec<(Vec<u8>, Option<i64>)> =
        sqlx::query_as("SELECT blob, delete_after FROM blobs ORDER BY blob")
            .fetch_all(&mut connection)
            .await
            .expect("read");
    let times: Vec<_> = rows.into_iter().map(|(_, at)| at).collect();
    assert_eq!(times, vec![Some(900), Some(5), Some(5), Some(5)]);
}
