//! The terms of paid plans (§26 Billing): subscription details and their
//! reminders, orders, consents, ending a paid period with the refund it
//! owes, price changes and the queue of mail about plans, on both stores.

use std::time::Duration;

use sync_protocol::messages::OwnedState;

use super::World;
use super::plans::{another_workspace, sign_up, subscribe, subscription};
use crate::billing::CONSENT_DAYS;
use crate::plans::{DAY, FREE};
use crate::{
    AccountPlanRow, Backend, BillingEvent, BillingMail, ConsentRow, EarlyEnd, OrderRow,
    PriceChangeRow, Reminder, Store, SubscriptionState,
};

fn mail(subject: &str) -> BillingMail {
    BillingMail {
        recipient: "owner@example.com".into(),
        subject: subject.into(),
        body: format!("{subject}\n"),
    }
}

fn order(order_id: &str, paid_at: u64) -> OrderRow {
    OrderRow {
        subscription: "subscription-1".into(),
        order_id: order_id.into(),
        net: 9_600,
        currency: "usd".into(),
        paid_at,
    }
}

/// A yearly subscription with every detail a provider reports.
fn detailed(changed_at: u64) -> AccountPlanRow {
    AccountPlanRow {
        period_start: Some(1_790_000_000),
        started_at: Some(1_790_000_000),
        amount: Some(9_600),
        currency: Some("usd".into()),
        interval: Some("year".into()),
        canceling: false,
        country: Some("CA".into()),
        region: Some("QC".into()),
        ..subscription("plus_year", SubscriptionState::Active, changed_at)
    }
}

/// The subjects of the queued mail, oldest first.
async fn queued<B: Backend>(world: &World<B>) -> Vec<String> {
    world
        .store
        .billing_mail()
        .await
        .expect("queue")
        .into_iter()
        .map(|queued| queued.mail.subject)
        .collect()
}

/// A subscription's details round-trip, and the reminders the server
/// recorded survive the provider's later reports of the same
/// subscription, never another's.
pub async fn subscription_details_round_trip_and_keep_their_reminders<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    sign_up(&world).await;
    subscribe(&world, "e1", detailed(1)).await;
    let stored = world
        .store
        .account_plan(world.account)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(stored, detailed(1));
    assert!(stored.in_quebec() && !stored.in_eu_or_uk() && stored.yearly());

    let reminder = Reminder {
        reminded_for: Some(1_800_000_000),
        reminders: 1,
        reminded_day: Some(20_000),
    };
    assert!(
        world
            .store
            .record_reminder(
                world.account,
                "subscription-1".into(),
                reminder,
                mail("Reminder")
            )
            .await
            .expect("recorded")
    );
    assert!(
        !world
            .store
            .record_reminder(world.account, "other".into(), reminder, mail("Other"))
            .await
            .expect("checked"),
        "another subscription's reminder changes nothing"
    );
    assert_eq!(queued(&world).await, ["Reminder"]);

    // The provider reports the same subscription again.
    subscribe(
        &world,
        "e2",
        AccountPlanRow {
            canceling: true,
            ..detailed(2)
        },
    )
    .await;
    let stored = world
        .store
        .account_plan(world.account)
        .await
        .expect("read")
        .expect("stored");
    assert!(stored.canceling);
    assert_eq!(
        (stored.reminded_for, stored.reminders, stored.reminded_day),
        (Some(1_800_000_000), 1, Some(20_000))
    );

    // A new subscription starts with no reminders.
    subscribe(
        &world,
        "e3",
        AccountPlanRow {
            subscription: "subscription-2".into(),
            ..detailed(3)
        },
    )
    .await;
    let stored = world
        .store
        .account_plan(world.account)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!((stored.reminded_for, stored.reminders), (None, 0));

    // Paging through the subscriptions.
    let page = world.store.subscriptions(None, 10).await.expect("page");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].0, world.account);
    assert!(
        world
            .store
            .subscriptions(Some(world.account), 10)
            .await
            .expect("page")
            .is_empty()
    );
}

/// The latest order replaces an older one and never a newer one, an
/// event's mail goes in the queue once, and consents stay 3 years.
pub async fn orders_mail_and_consents_keep_to_their_rules<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    sign_up(&world).await;
    let paid = |id: &str, order_id: &str, paid_at: u64, subject: &str| BillingEvent {
        provider: "polar".into(),
        id: id.into(),
        order: Some((world.account, order(order_id, paid_at))),
        mail: vec![mail(subject)],
        ..BillingEvent::default()
    };
    assert!(
        world
            .store
            .apply_billing_event(paid("o1", "order-2", 200, "Contract"))
            .await
            .expect("applied")
    );
    assert!(
        !world
            .store
            .apply_billing_event(paid("o1", "order-2", 200, "Contract"))
            .await
            .expect("applied"),
        "a retry queues nothing"
    );
    world
        .store
        .apply_billing_event(paid("o2", "order-1", 100, "Late"))
        .await
        .expect("applied");
    assert_eq!(
        world
            .store
            .latest_order(world.account)
            .await
            .expect("read")
            .map(|row| row.order_id),
        Some("order-2".into()),
        "an older order arriving late stays out"
    );
    world
        .store
        .apply_billing_event(paid("o3", "order-3", 300, "Renewal"))
        .await
        .expect("applied");
    assert_eq!(
        world.store.latest_order(world.account).await.expect("read"),
        Some(order("order-3", 300))
    );
    assert_eq!(queued(&world).await, ["Contract", "Late", "Renewal"]);
    let first = world.store.billing_mail().await.expect("queue")[0].clone();
    assert_eq!(first.mail, mail("Contract"));
    world
        .store
        .sent_billing_mail(first.id)
        .await
        .expect("deleted");
    assert_eq!(queued(&world).await, ["Late", "Renewal"]);

    // Consents: the latest wins, a repeat in the same second is one row,
    // and none outlives 3 years.
    let now = world.store.clock().now();
    let consent = |accepted_at: u64, plan: &str| ConsentRow {
        accepted_at,
        plan: plan.into(),
        version: "2026-11-27".into(),
        immediate: true,
        renewal: true,
    };
    for row in [
        consent(now - 10, "plus_month"),
        consent(now, "plus_year"),
        consent(now, "plus_year"),
    ] {
        world
            .store
            .record_consent(world.account, row)
            .await
            .expect("recorded");
    }
    assert_eq!(
        world
            .store
            .latest_consent(world.account)
            .await
            .expect("read"),
        Some(consent(now, "plus_year"))
    );
    assert_eq!(world.store.forget_consents().await.expect("pruned"), 0);
    world
        .store
        .clock()
        .advance(Duration::from_secs(CONSENT_DAYS * DAY + 1));
    assert_eq!(world.store.forget_consents().await.expect("pruned"), 2);
    assert_eq!(
        world
            .store
            .latest_consent(world.account)
            .await
            .expect("read"),
        None
    );
}

/// Ending a paid period early moves the account to `free` at once, puts
/// its extra workspaces over plan, records the refund and revocation owed
/// and queues the confirmation, all once; a scheduled cancellation keeps
/// the plan.
pub async fn ending_a_paid_period_moves_the_account_to_free_and_owes_the_refund<B: Backend>(
    store: Store<B>,
) {
    let world = World::new(store.with_billing(true), 1).await;
    sign_up(&world).await;
    subscribe(&world, "e1", detailed(1)).await;
    let (second, _) = another_workspace(&world).await.expect("second");
    let session = world.devices[0].session;

    assert!(
        world
            .store
            .mark_canceling(
                world.account,
                "subscription-1".into(),
                vec![mail("Canceled")]
            )
            .await
            .expect("marked")
    );
    let row = world
        .store
        .account_plan(world.account)
        .await
        .expect("read")
        .expect("stored");
    assert!(row.canceling && row.state.grants());
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

    let end = |subscription: &str| EarlyEnd {
        subscription: subscription.into(),
        refund: Some(("order-1".into(), 4_800)),
        mail: vec![mail("Refunded")],
    };
    assert_eq!(
        world
            .store
            .end_paid_period(world.account, end("someone-else"))
            .await
            .expect("checked"),
        None
    );
    let owed = world
        .store
        .end_paid_period(world.account, end("subscription-1"))
        .await
        .expect("ended")
        .expect("owed");
    assert_eq!(
        (
            owed.account,
            owed.provider.as_str(),
            owed.subscription.as_str(),
            owed.order_id.as_str(),
            owed.amount,
            owed.revoke
        ),
        (
            world.account,
            "polar",
            "subscription-1",
            "order-1",
            4_800,
            true
        )
    );
    let report = world.store.account(session).await.expect("report");
    assert_eq!(report.plan.as_str(), FREE);
    let state = |id| {
        report
            .workspaces
            .0
            .iter()
            .find(|w| w.workspace == id)
            .map(|w| w.state)
    };
    assert_eq!(state(world.workspace), Some(OwnedState::Active));
    assert_eq!(state(second), Some(OwnedState::OverPlan));
    assert_eq!(
        world
            .store
            .end_paid_period(world.account, end("subscription-1"))
            .await
            .expect("checked"),
        None,
        "an ended period ends once"
    );
    assert!(
        !world
            .store
            .mark_canceling(world.account, "subscription-1".into(), vec![mail("Again")])
            .await
            .expect("checked")
    );
    assert_eq!(queued(&world).await, ["Canceled", "Refunded"]);

    // The provider's late report of the active subscription stays out.
    subscribe(&world, "e2", detailed(2)).await;
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

    // What's owed shrinks as the provider accepts each part.
    assert_eq!(
        world.store.refunds_owed().await.expect("owed"),
        std::slice::from_ref(&owed)
    );
    world
        .store
        .settle_refund(owed.id, 0, true)
        .await
        .expect("refunded");
    let left = world.store.refunds_owed().await.expect("owed");
    assert_eq!((left[0].amount, left[0].revoke), (0, true));
    world
        .store
        .settle_refund(owed.id, 0, false)
        .await
        .expect("revoked");
    assert!(world.store.refunds_owed().await.expect("owed").is_empty());
}

/// A price change keeps its notices per subscriber, each recorded once,
/// and the account's language waits for its activity row.
pub async fn price_notices_and_languages_record_once<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_billing(true), 1).await;
    sign_up(&world).await;
    let change = PriceChangeRow {
        change: 0,
        plan: "plus_year".into(),
        effective_day: 21_000,
        old_en: "USD 96 a year".into(),
        new_en: "USD 108 a year".into(),
        old_fr: "96 $ US par année".into(),
        new_fr: "108 $ US par année".into(),
    };
    let number = world
        .store
        .announce_price_change(change.clone())
        .await
        .expect("announced");
    let second = world
        .store
        .announce_price_change(change.clone())
        .await
        .expect("announced");
    assert!(second > number);
    let changes = world.store.price_changes().await.expect("changes");
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[0],
        PriceChangeRow {
            change: number,
            ..change
        }
    );
    assert_eq!(
        world
            .store
            .price_notices(number, world.account)
            .await
            .expect("read"),
        0
    );
    let record = |notices| {
        world
            .store
            .record_price_notice(number, world.account, notices, mail("Price"))
    };
    assert!(record(1).await.expect("recorded"));
    assert!(!record(1).await.expect("checked"), "one first notice");
    assert!(record(2).await.expect("recorded"));
    assert_eq!(
        world
            .store
            .price_notices(number, world.account)
            .await
            .expect("read"),
        2
    );
    assert_eq!(
        world
            .store
            .price_notices(second, world.account)
            .await
            .expect("read"),
        0
    );
    assert_eq!(queued(&world).await, ["Price", "Price"]);

    world
        .store
        .set_account_lang(world.account, "fr")
        .await
        .expect("set");
    assert_eq!(
        world.store.account_lang(world.account).await.expect("read"),
        None,
        "no activity row yet"
    );
    world
        .store
        .note_activity(world.account, 24_330)
        .await
        .expect("noted");
    world
        .store
        .set_account_lang(world.account, "fr")
        .await
        .expect("set");
    assert_eq!(
        world
            .store
            .account_lang(world.account)
            .await
            .expect("read")
            .as_deref(),
        Some("fr")
    );
    world
        .store
        .queue_billing_mail(mail("Direct"))
        .await
        .expect("queued");
    assert_eq!(queued(&world).await, ["Price", "Price", "Direct"]);
}
