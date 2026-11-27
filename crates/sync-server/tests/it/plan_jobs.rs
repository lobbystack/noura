//! The plan jobs of the maintenance pass (§20): notices about a workspace
//! over plan on its first day, day 60 and day 85, its close on day 90,
//! and the warnings and close of a `free` owner nobody used for a year, on
//! the store's clock with mail kept in an outbox.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sync_protocol::messages::OwnedState;
use sync_protocol::types::{Digest, Id};
use sync_server::accounts::mail::{Mail, MailTransport, SendFuture};
use sync_server::maintenance::{month_of, plan_jobs};
use sync_server::{Accounts, Allowlist, PublicOrigin};
use sync_store::{AccountPlanRow, Backend, Session, Store, SubscriptionState, WriteTxn};
use sync_store_sqlite::SqliteBackend;

use crate::support::{Account, push_body, seed_store};

const DAY: u64 = 86_400;

/// Keeps every message it sends.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<Mail>>>);

impl MailTransport for Outbox {
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a> {
        self.0.lock().unwrap().push(mail.clone());
        Box::pin(async { Ok(()) })
    }
}

impl Outbox {
    fn take(&self) -> Vec<Mail> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct World {
    store: Arc<Store<SqliteBackend>>,
    accounts: Accounts,
    outbox: Outbox,
    _dir: tempfile::TempDir,
}

async fn world() -> World {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = Arc::new(
        sync_server::open_store(&sync_server::Config::new(
            data,
            "127.0.0.1:0".parse().expect("an address"),
        ))
        .await
        .expect("the store opens")
        .with_billing(true),
    );
    let outbox = Outbox::default();
    let accounts = Accounts::new(
        PublicOrigin::parse("https://sync.example.com").expect("an origin"),
        Allowlist::default(),
        "Noura <noura@example.com>".into(),
        Box::new(outbox.clone()),
        None,
    );
    World {
        store,
        accounts,
        outbox,
        _dir: dir,
    }
}

impl World {
    /// An account with one device and `email`.
    async fn owner(&self, email: &'static str) -> Account {
        let account = seed_store(&self.store, 1).await;
        let id = account.account;
        self.store
            .backend()
            .write(Box::new(
                move |txn: &mut <SqliteBackend as Backend>::Write| {
                    Box::pin(async move { txn.insert_account(id, email).await })
                },
            ))
            .await
            .expect("signed up");
        account
    }

    async fn subscribe(&self, account: Id, event: &str, state: SubscriptionState, at: u64) {
        let row = AccountPlanRow {
            provider: "polar".into(),
            customer: "cus".into(),
            subscription: "sub".into(),
            plan: "plus_year".into(),
            state,
            changed_at: at,
            ..AccountPlanRow::default()
        };
        self.store
            .apply_billing_event(sync_store::BillingEvent {
                provider: "polar".into(),
                id: event.into(),
                change: Some((account, row)),
                ..Default::default()
            })
            .await
            .expect("applied");
    }

    async fn create(&self, owner: &Account) -> Id {
        let created = owner.creation();
        self.store
            .create_workspace(
                Session::new(owner.account, owner.keys[0].device()),
                created.body,
            )
            .await
            .expect("created");
        created.workspace
    }

    async fn run(&self) -> sync_server::maintenance::PlanJobs {
        plan_jobs(&self.store, &self.accounts)
            .await
            .expect("the jobs run")
    }

    fn advance_days(&self, days: u64) {
        self.store.clock().advance(Duration::from_secs(days * DAY));
    }

    fn month(&self) -> u64 {
        month_of(self.store.clock().now())
    }

    /// Moves the clock to the first day `months` months after `from`.
    fn advance_to_month(&self, from: u64, months: u64) {
        while self.month() < from + months {
            self.advance_days(1);
        }
    }
}

#[tokio::test]
async fn a_workspace_over_plan_gets_three_notices_then_closes() {
    let world = world().await;
    let owner = world.owner("owner@example.com").await;
    world
        .subscribe(owner.account, "e1", SubscriptionState::Active, 1)
        .await;
    let kept = world.create(&owner).await;
    let over = world.create(&owner).await;
    world
        .subscribe(owner.account, "e2", SubscriptionState::Ended, 2)
        .await;
    let report = world
        .store
        .account_report(owner.account)
        .await
        .expect("report");
    let state = |id: Id| {
        report
            .workspaces
            .0
            .iter()
            .find(|w| w.workspace == id)
            .map(|w| w.state)
    };
    assert_eq!(state(kept), Some(OwnedState::Active));
    assert_eq!(state(over), Some(OwnedState::OverPlan));

    // Day 0: one notice to the owner, once.
    assert_eq!(world.run().await.notices, 1);
    let mail = world.outbox.take();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].to, "owner@example.com");
    assert_eq!(mail[0].subject, "A workspace you sync is read-only");
    assert!(mail[0].text.contains("https://sync.example.com/account"));
    assert!(mail[0].text.contains("Settings > Account"));
    assert_eq!(world.run().await.notices, 0);
    assert!(world.outbox.take().is_empty());

    // Day 60 and day 85 each send one more.
    world.advance_days(59);
    assert_eq!(world.run().await.notices, 0);
    world.advance_days(1);
    assert_eq!(world.run().await.notices, 1);
    let mail = world.outbox.take();
    assert!(
        mail[0]
            .subject
            .starts_with("A workspace you sync closes on")
    );
    world.advance_days(25);
    assert_eq!(world.run().await.notices, 1);
    assert_eq!(world.outbox.take().len(), 1);
    assert_eq!(world.run().await.notices, 0);

    // Day 90 closes it; the other stays.
    world.advance_days(4);
    assert_eq!(world.run().await.closed_over_plan, 0);
    world.advance_days(1);
    assert_eq!(world.run().await.closed_over_plan, 1);
    let report = world
        .store
        .account_report(owner.account)
        .await
        .expect("report");
    assert_eq!(report.workspaces.0.len(), 1);
    assert_eq!(report.workspaces.0[0].workspace, kept);
    assert_eq!(report.workspaces.0[0].state, OwnedState::Active);
}

#[tokio::test]
async fn an_upgrade_stops_the_notices() {
    let world = world().await;
    let owner = world.owner("owner@example.com").await;
    world
        .subscribe(owner.account, "e1", SubscriptionState::Active, 1)
        .await;
    world.create(&owner).await;
    world.create(&owner).await;
    world
        .subscribe(owner.account, "e2", SubscriptionState::Ended, 2)
        .await;
    assert_eq!(world.run().await.notices, 1);
    world
        .subscribe(owner.account, "e3", SubscriptionState::Active, 3)
        .await;
    world.advance_days(95);
    let report = world.run().await;
    assert_eq!((report.notices, report.closed_over_plan), (0, 0));
    let account = world
        .store
        .account_report(owner.account)
        .await
        .expect("report");
    assert_eq!(account.workspaces.0.len(), 2);
}

/// The code of a refused push.
fn refused_code<T: std::fmt::Debug>(result: Result<T, sync_store::Error>) -> String {
    match result {
        Err(sync_store::Error::Refused(refusal)) => refusal.code.to_string(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_upgrade_on_day_70_reopens_the_workspaces_and_cancels_the_close() {
    let world = world().await;
    let owner = world.owner("owner@example.com").await;
    world
        .subscribe(owner.account, "e1", SubscriptionState::Active, 1)
        .await;
    let session = Session::new(owner.account, owner.keys[0].device());
    let mut created = Vec::new();
    for _ in 0..3 {
        let workspace = owner.creation();
        world
            .store
            .create_workspace(session, workspace.body.clone())
            .await
            .expect("created");
        created.push(workspace);
    }
    // The subscription ends: Free keeps one workspace, and the owner chose
    // the newest.
    world
        .subscribe(owner.account, "e2", SubscriptionState::Ended, 2)
        .await;
    world
        .store
        .set_active_workspaces(session, vec![created[2].workspace])
        .await
        .expect("chosen");
    let push = |index: usize, counter: u64| {
        let op = created[index].op(&owner.keys[0], counter, Digest::ZERO, |_| {});
        world.store.push(
            session,
            created[index].workspace,
            push_body(std::slice::from_ref(&op)),
        )
    };
    assert_eq!(refused_code(push(0, 1).await), "over_plan");

    // Day 0 and day 60 send their notices.
    assert_eq!(world.run().await.notices, 2);
    world.advance_days(60);
    assert_eq!(world.run().await.notices, 2);
    world.outbox.take();

    // Day 70: the owner upgrades, and every workspace takes writes again.
    world.advance_days(10);
    world
        .subscribe(owner.account, "e3", SubscriptionState::Active, 3)
        .await;
    let report = world
        .store
        .account_report(owner.account)
        .await
        .expect("report");
    assert_eq!(report.workspaces.0.len(), 3);
    for workspace in &report.workspaces.0 {
        assert_eq!(workspace.state, OwnedState::Active);
        assert_eq!(workspace.closes_at, None);
    }
    push(0, 1).await.expect("the oldest takes writes again");
    push(1, 1).await.expect("so does the second");

    // Days 85 and 90 pass with no notice and no close.
    for days in [15, 5, 10] {
        world.advance_days(days);
        let jobs = world.run().await;
        assert_eq!((jobs.notices, jobs.closed_over_plan), (0, 0));
    }
    assert!(world.outbox.take().is_empty());
    let report = world
        .store
        .account_report(owner.account)
        .await
        .expect("report");
    assert_eq!(report.workspaces.0.len(), 3);
}

#[tokio::test]
async fn a_free_owner_nobody_used_for_a_year_gets_two_warnings_then_closes() {
    let world = world().await;
    let owner = world.owner("idle@example.com").await;
    let workspace = world.create(&owner).await;
    let start = world.month();
    world
        .store
        .note_activity(owner.account, start)
        .await
        .expect("noted");

    // Ten months on, nothing yet.
    world.advance_to_month(start, 10);
    assert_eq!(world.run().await.warnings, 0);

    // The 11th and 12th months each send one warning, once.
    world.advance_to_month(start, 11);
    assert_eq!(world.run().await.warnings, 1);
    assert_eq!(world.run().await.warnings, 0);
    let mail = world.outbox.take();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].to, "idle@example.com");
    assert!(mail[0].text.contains("11 months"));
    world.advance_to_month(start, 12);
    assert_eq!(world.run().await.warnings, 1);
    assert!(world.outbox.take()[0].text.contains("12 months"));

    // The 13th closes the owner's workspaces.
    world.advance_to_month(start, 13);
    assert_eq!(world.run().await.closed_inactive, 1);
    let report = world
        .store
        .account_report(owner.account)
        .await
        .expect("report");
    assert!(report.workspaces.0.iter().all(|w| w.workspace != workspace));
}

#[tokio::test]
async fn activity_or_a_paid_plan_keeps_an_idle_owners_workspaces() {
    let world = world().await;
    let returning = world.owner("back@example.com").await;
    let paying = world.owner("paying@example.com").await;
    world.create(&returning).await;
    world.create(&paying).await;
    world
        .subscribe(paying.account, "e1", SubscriptionState::Active, 1)
        .await;
    let start = world.month();
    for account in [returning.account, paying.account] {
        world
            .store
            .note_activity(account, start)
            .await
            .expect("noted");
    }
    world.advance_to_month(start, 11);
    assert_eq!(world.run().await.warnings, 1, "only the free owner");
    // The owner comes back: the warnings clear and the clock starts over.
    world
        .store
        .note_activity(returning.account, world.month())
        .await
        .expect("noted");
    world.advance_to_month(start, 14);
    let report = world.run().await;
    assert_eq!((report.warnings, report.closed_inactive), (0, 0));
    for account in [returning.account, paying.account] {
        let owned = world.store.account_report(account).await.expect("report");
        assert_eq!(owned.workspaces.0.len(), 1);
    }
}
