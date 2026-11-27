//! Plans (§20): storage an owner's workspaces share, the owned-workspace
//! limit, the largest file, the member limit, the over-plan state and the
//! webhook events that set an account's subscription, on both stores.

use sync_protocol::blob::blob_ciphertext_len;
use sync_protocol::codec::Decode;
use sync_protocol::messages::{FileTooLargeDetail, OwnedState, SubscriptionStatus};
use sync_protocol::records::{Card, Operation, PolicyKind, Role, Signed};
use sync_protocol::types::{Digest, Id};

use super::blobs::blob_for;
use super::invitations::Invite;
use super::{World, creation, push_failure, refusal};
use crate::plans::{DAY, FREE, OPERATOR, OVER_PLAN_DAYS};
use crate::{AccountPlanRow, Backend, BlobCreation, Store, SubscriptionState, WriteTxn};

const GB: u64 = 1_000_000_000;

/// Gives the world's account an email, as sign-up does, so webhook
/// events can name it.
async fn sign_up<B: Backend>(world: &World<B>) {
    let account = world.account;
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.insert_account(account, "owner@example.com").await })
        }))
        .await
        .expect("signed up");
}

/// A subscription row for `plan` in `state`, changed at `changed_at`.
fn subscription(plan: &str, state: SubscriptionState, changed_at: u64) -> AccountPlanRow {
    AccountPlanRow {
        provider: "polar".into(),
        customer: "customer-1".into(),
        subscription: "subscription-1".into(),
        plan: plan.into(),
        state,
        period_end: Some(1_800_000_000),
        changed_at,
    }
}

/// Applies a webhook event that sets the world's subscription.
async fn subscribe<B: Backend>(world: &World<B>, event: &str, row: AccountPlanRow) -> bool {
    world
        .store
        .apply_billing_event("polar", event.into(), Some((world.account, row)))
        .await
        .expect("applied")
}

/// Creates another workspace of the world's account from its first device,
/// and returns it with its workspace domain.
async fn another_workspace<B: Backend>(world: &World<B>) -> crate::Result<(Id, Id)> {
    let cards: Vec<Card> = world
        .devices
        .iter()
        .map(|d| d.keys.card(world.account))
        .collect();
    let (body, genesis, _) = creation(&world.devices[0].keys, &world.links, &cards);
    world
        .store
        .create_workspace(world.devices[0].session, body)
        .await?;
    let PolicyKind::Genesis { workspace_domain } = genesis.body.kind else {
        unreachable!("a genesis");
    };
    Ok((genesis.body.workspace, workspace_domain))
}

/// The world's next operation after `previous`, at `counter`.
fn after<B: Backend>(
    world: &World<B>,
    previous: &Signed<Operation>,
    counter: u64,
) -> Signed<Operation> {
    world.op(0, |op| {
        op.counter = counter;
        op.previous = previous.digest();
    })
}

/// Adds `bytes` of usage to the world's workspace domain.
async fn add_usage<B: Backend>(world: &World<B>, bytes: i64) {
    let (workspace, domain) = (world.workspace, world.domain);
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.add_usage(workspace, domain, bytes, 0).await })
        }))
        .await
        .expect("usage added");
}

/// On a server that sells no plans, every account keeps the operator's
/// settings: no allowance, 100 owned workspaces and no other limit.
pub async fn a_server_without_billing_keeps_the_operator_settings<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let report = world
        .store
        .account(world.devices[0].session)
        .await
        .expect("report");
    assert_eq!(report.plan.as_str(), OPERATOR);
    assert_eq!(report.status, SubscriptionStatus::None);
    assert!(!report.billing);
    assert_eq!(report.owned_workspaces, 100);
    assert_eq!(
        (report.storage_bytes, report.max_file_bytes, report.members),
        (None, None, None)
    );
    assert_eq!(report.workspaces.0.len(), 1);
    assert!(
        report.used_bytes > 0,
        "the workspace's policy and keys count"
    );
    another_workspace(&world)
        .await
        .expect("a second workspace without billing");
    // A subscription changes nothing while the server sells no plans.
    sign_up(&world).await;
    assert!(
        subscribe(
            &world,
            "e1",
            subscription("free", SubscriptionState::Ended, 1)
        )
        .await
    );
    assert_eq!(
        world
            .store
            .account(world.devices[0].session)
            .await
            .expect("report")
            .plan
            .as_str(),
        OPERATOR
    );
}

/// The owned-workspace limit follows the plan: `free` and the standard
/// plans own one, `plus` ten (§20).
pub async fn the_plan_bounds_owned_workspaces<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    sign_up(&world).await;
    let report = world
        .store
        .account(world.devices[0].session)
        .await
        .expect("report");
    assert_eq!(report.plan.as_str(), FREE);
    assert!(report.billing);
    assert_eq!(report.owned_workspaces, 1);
    assert_eq!(report.storage_bytes, Some(100_000_000));
    assert_eq!(
        refusal(another_workspace(&world).await).code,
        "workspace_limit"
    );
    assert!(
        subscribe(
            &world,
            "e1",
            subscription("plus_year", SubscriptionState::Active, 10)
        )
        .await
    );
    let report = world
        .store
        .account(world.devices[0].session)
        .await
        .expect("report");
    assert_eq!(report.plan.as_str(), "plus_year");
    assert_eq!(report.status, SubscriptionStatus::Active);
    assert_eq!(report.period_end, Some(1_800_000_000));
    assert_eq!(report.owned_workspaces, 10);
    another_workspace(&world)
        .await
        .expect("plus owns a second workspace");
}

/// Storage counts against the owner across every workspace it owns, and
/// against nobody else: a push that raises usage while the owner stands
/// above its allowance gets `quota_exceeded` in any of its workspaces.
pub async fn owned_workspaces_share_the_allowance<B: Backend>(store: Store<B>) {
    let store = store.with_billing(true);
    let world = World::new(store, 1).await;
    sign_up(&world).await;
    subscribe(
        &world,
        "e1",
        subscription("standard_month", SubscriptionState::Active, 1),
    )
    .await;
    let first = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&first))
        .await
        .expect("fits");

    // Another account's workspace on the same store.
    let other = World::on(world.store.clone(), 1).await;

    // The first workspace alone takes the owner past 1 GB.
    add_usage(&world, GB as i64).await;
    let report = world
        .store
        .account(world.devices[0].session)
        .await
        .expect("report");
    assert!(report.used_bytes > GB);
    let op = after(&world, &first, 2);
    assert_eq!(push_failure(world.push(0, &[op]).await).0, "quota_exceeded");

    // Another account's allowance is its own.
    other
        .push(0, &[other.op(0, |_| {})])
        .await
        .expect("the other owner has room");
    let theirs = other
        .store
        .account(other.devices[0].session)
        .await
        .expect("report");
    assert!(theirs.used_bytes < GB);

    // The allowance holds across the owner's workspaces: a second workspace
    // on plus shares the 10 GB.
    subscribe(
        &world,
        "e2",
        subscription("plus_month", SubscriptionState::Active, 2),
    )
    .await;
    add_usage(&world, 9 * GB as i64).await;
    let (second, _) = another_workspace(&world).await.expect("plus owns two");
    let (_, upload) = blob_for(300_000, world.domain);
    assert_eq!(
        refusal(
            world
                .store
                .create_blob(world.devices[0].session, world.workspace, upload)
                .await
        )
        .code,
        "quota_exceeded"
    );
    let report = world
        .store
        .account(world.devices[0].session)
        .await
        .expect("report");
    assert_eq!(report.workspaces.0.len(), 2);
    assert!(report.workspaces.0.iter().any(|w| w.workspace == second));

    // Freeing space lets writes through again.
    add_usage(&world, -(10 * GB as i64)).await;
    world
        .push(0, &[after(&world, &first, 3)])
        .await
        .expect("room again");
}

/// A blob larger than the plan's largest file gets `file_too_large`, with
/// the size in its detail (§20, §21).
pub async fn uploads_past_the_largest_file_are_too_large<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    let session = world.devices[0].session;
    let fits = BlobCreation {
        blob: Id::random().expect("random"),
        length: blob_ciphertext_len(5_000_000),
        digest: Digest([1; 32]),
        domain: world.domain,
    };
    world
        .store
        .create_blob(session, world.workspace, fits)
        .await
        .expect("a 5 MB file fits free");
    let over = BlobCreation {
        blob: Id::random().expect("random"),
        length: blob_ciphertext_len(5_000_000 + 1_048_576),
        ..fits
    };
    let refused = refusal(
        world
            .store
            .create_blob(session, world.workspace, over)
            .await,
    );
    assert_eq!(refused.code, "file_too_large");
    let detail = FileTooLargeDetail::from_bytes(refused.detail.as_deref().expect("a detail"))
        .expect("decodes");
    assert_eq!(detail.max_file_bytes, 5_000_000);

    // Plus takes 200 MB.
    sign_up(&world).await;
    subscribe(
        &world,
        "e1",
        subscription("plus_year", SubscriptionState::PastDue, 1),
    )
    .await;
    world
        .store
        .create_blob(session, world.workspace, over)
        .await
        .expect("plus takes it while its payment is retried");
}

/// Members and open invitations together stay within the owner's plan:
/// `free` counts four member accounts, the owner included (§11, §20).
pub async fn invitations_stop_at_the_member_limit<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    let owner = world.devices[0].session;
    for _ in 0..3 {
        let invite = Invite::new(Role::Editor);
        world
            .store
            .create_invitation(owner, world.workspace, invite.creation(world.workspace))
            .await
            .expect("within four");
    }
    let invite = Invite::new(Role::Editor);
    assert_eq!(
        refusal(
            world
                .store
                .create_invitation(owner, world.workspace, invite.creation(world.workspace))
                .await
        )
        .code,
        "member_limit"
    );
    sign_up(&world).await;
    subscribe(
        &world,
        "e1",
        subscription("standard_year", SubscriptionState::Active, 1),
    )
    .await;
    world
        .store
        .create_invitation(owner, world.workspace, invite.creation(world.workspace))
        .await
        .expect("standard takes twenty");
}

/// A downgrade below the workspaces an owner holds puts the newest over
/// plan, read-only, until the owner chooses others, upgrades or closes one
/// (§20, Over plan).
pub async fn a_downgrade_puts_the_newest_workspaces_over_plan<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    sign_up(&world).await;
    subscribe(
        &world,
        "e1",
        subscription("plus_year", SubscriptionState::Active, 1),
    )
    .await;
    let (second, second_domain) = another_workspace(&world).await.expect("second");
    let (third, _) = another_workspace(&world).await.expect("third");
    let session = world.devices[0].session;

    // The subscription ends: free keeps one workspace, the oldest.
    subscribe(
        &world,
        "e2",
        subscription("plus_year", SubscriptionState::Ended, 2),
    )
    .await;
    let report = world.store.account(session).await.expect("report");
    assert_eq!(report.plan.as_str(), FREE);
    assert_eq!(report.status, SubscriptionStatus::Ended);
    assert_eq!(report.period_end, None);
    let states: Vec<(Id, OwnedState)> = report
        .workspaces
        .0
        .iter()
        .map(|w| (w.workspace, w.state))
        .collect();
    assert_eq!(
        states,
        vec![
            (world.workspace, OwnedState::Active),
            (second, OwnedState::OverPlan),
            (third, OwnedState::OverPlan),
        ]
    );
    let today = world.store.clock().now() / DAY;
    assert_eq!(
        report.workspaces.0[1].closes_at,
        Some((today + OVER_PLAN_DAYS) * DAY)
    );
    assert_eq!(report.workspaces.0[0].closes_at, None);

    // The oldest still takes writes.
    let first = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&first))
        .await
        .expect("active");

    // An over-plan workspace takes no blob upload, share or invitation.
    let (_, upload) = blob_for(300_000, second_domain);
    assert_eq!(
        refusal(world.store.create_blob(session, second, upload).await).code,
        "over_plan"
    );
    let invite = Invite::new(Role::Editor);
    assert_eq!(
        refusal(
            world
                .store
                .create_invitation(session, third, invite.creation(third))
                .await
        )
        .code,
        "over_plan"
    );

    // The owner keeps the third instead.
    assert_eq!(
        refusal(
            world
                .store
                .set_active_workspaces(session, vec![second, third])
                .await
        )
        .code,
        "bad_request"
    );
    assert_eq!(
        refusal(
            world
                .store
                .set_active_workspaces(session, vec![Id::random().expect("random")])
                .await
        )
        .code,
        "not_found"
    );
    world
        .store
        .set_active_workspaces(session, vec![third])
        .await
        .expect("chosen");
    let report = world.store.account(session).await.expect("report");
    let state = |id: Id| {
        report
            .workspaces
            .0
            .iter()
            .find(|w| w.workspace == id)
            .map(|w| w.state)
    };
    assert_eq!(state(third), Some(OwnedState::Active));
    assert_eq!(state(world.workspace), Some(OwnedState::OverPlan));
    assert_eq!(state(second), Some(OwnedState::OverPlan));
    let op = after(&world, &first, 2);
    assert_eq!(push_failure(world.push(0, &[op]).await).0, "over_plan");

    // Closing the kept one brings the oldest back.
    world
        .store
        .close_workspace(session, third)
        .await
        .expect("closed");
    let report = world.store.account(session).await.expect("report");
    assert_eq!(report.workspaces.0.len(), 2);
    assert_eq!(report.workspaces.0[0].workspace, world.workspace);
    assert_eq!(report.workspaces.0[0].state, OwnedState::Active);
    assert_eq!(report.workspaces.0[1].state, OwnedState::OverPlan);

    // An upgrade makes every workspace active again.
    subscribe(
        &world,
        "e3",
        subscription("plus_month", SubscriptionState::Active, 3),
    )
    .await;
    let report = world.store.account(session).await.expect("report");
    assert!(
        report
            .workspaces
            .0
            .iter()
            .all(|w| w.state == OwnedState::Active)
    );
    world
        .push(0, &[after(&world, &first, 3)])
        .await
        .expect("active again");
}

/// Each webhook event applies once, an event that reports an older change
/// than the stored one changes nothing, and an event for an account the
/// store doesn't hold stores nothing (§26, Billing).
pub async fn billing_events_apply_once_and_in_order<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    let session = world.devices[0].session;
    // No email, no account: nothing to store, but the event counts.
    assert!(
        subscribe(
            &world,
            "e0",
            subscription("plus_year", SubscriptionState::Active, 1)
        )
        .await
    );
    assert!(
        world
            .store
            .account_plan(world.account)
            .await
            .expect("row")
            .is_none()
    );

    sign_up(&world).await;
    assert!(
        subscribe(
            &world,
            "e1",
            subscription("plus_year", SubscriptionState::Active, 10)
        )
        .await
    );
    assert!(
        !subscribe(
            &world,
            "e1",
            subscription("plus_year", SubscriptionState::Ended, 99)
        )
        .await,
        "a retried delivery changes nothing"
    );
    assert_eq!(
        world
            .store
            .account(session)
            .await
            .expect("report")
            .plan
            .as_str(),
        "plus_year"
    );
    // An older change arriving late.
    subscribe(
        &world,
        "e2",
        subscription("plus_year", SubscriptionState::Ended, 9),
    )
    .await;
    assert_eq!(
        world
            .store
            .account(session)
            .await
            .expect("report")
            .plan
            .as_str(),
        "plus_year"
    );
    // A product no plan maps to never grants anything.
    subscribe(
        &world,
        "e3",
        subscription("gold", SubscriptionState::Active, 11),
    )
    .await;
    assert_eq!(
        world
            .store
            .account(session)
            .await
            .expect("report")
            .plan
            .as_str(),
        FREE
    );
    // Events older than 100 days go.
    assert_eq!(
        world.store.forget_billing_events().await.expect("pruned"),
        0
    );
    world
        .store
        .clock()
        .advance(std::time::Duration::from_secs(101 * DAY));
    assert_eq!(
        world.store.forget_billing_events().await.expect("pruned"),
        4
    );
}
