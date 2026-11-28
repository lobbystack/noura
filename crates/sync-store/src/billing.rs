//! The terms of paid plans (docs/workspace-format/sync-v2.md §26 Billing):
//! the latest paid order of each subscription, the consents given before a
//! checkout, the refunds and revocations the server owes the payment
//! provider, renewal reminders, price changes and their notices, and the
//! queue of mail about plans. Each change that the server reports by mail
//! queues the message in its own transaction, so the change and its mail
//! commit together.

use sync_protocol::types::Id;

use crate::plans::{AccountPlanRow, DAY, SubscriptionState, reconcile};
use crate::{Backend, ReadTxn, Result, Store, WriteTxn};

/// How long the store keeps a consent: 3 years, counted generously.
pub const CONSENT_DAYS: u64 = 3 * 366;

/// A message about plans, before it's queued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BillingMail {
    /// The address.
    pub recipient: String,
    /// The subject.
    pub subject: String,
    /// The plain-text body.
    pub body: String,
}

/// A queued message about plans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedMail {
    /// Its number, which the store assigns.
    pub id: u64,
    /// The message.
    pub mail: BillingMail,
}

/// The latest paid order of an account's subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderRow {
    /// The provider's subscription ID.
    pub subscription: String,
    /// The provider's order ID.
    pub order_id: String,
    /// Its amount before tax, in the currency's smallest unit.
    pub net: u64,
    /// Its currency, as a lower-case ISO 4217 code.
    pub currency: String,
    /// When it was paid, in Unix seconds.
    pub paid_at: u64,
}

/// The consent an account gave before a checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRow {
    /// When, in Unix seconds.
    pub accepted_at: u64,
    /// The plan the account chose.
    pub plan: String,
    /// The version of the text it agreed to.
    pub version: String,
    /// Whether it asked for the service to start during the withdrawal
    /// period.
    pub immediate: bool,
    /// Whether it agreed to the automatic renewal.
    pub renewal: bool,
}

/// A price change the operator announced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriceChangeRow {
    /// Its number, which the store assigns.
    pub change: u64,
    /// The plan it changes.
    pub plan: String,
    /// The day, from the Unix epoch, it takes effect.
    pub effective_day: u64,
    /// The clause before the change, in English.
    pub old_en: String,
    /// The clause after the change, in English.
    pub new_en: String,
    /// The clause before the change, in French.
    pub old_fr: String,
    /// The clause after the change, in French.
    pub new_fr: String,
}

/// A refund, a revocation or both that the server owes the provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefundOwed {
    /// Its number, which the store assigns.
    pub id: u64,
    /// The account.
    pub account: Id,
    /// The provider's name.
    pub provider: String,
    /// The provider's subscription ID, to revoke.
    pub subscription: String,
    /// The provider's order ID, to refund.
    pub order_id: String,
    /// The amount still to refund before tax, or 0.
    pub amount: u64,
    /// Whether the subscription is still to revoke.
    pub revoke: bool,
}

/// A webhook event in the store's terms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BillingEvent {
    /// The provider's name.
    pub provider: String,
    /// The provider's event ID, the same on every retry.
    pub id: String,
    /// The account's subscription as the event reports it.
    pub change: Option<(Id, AccountPlanRow)>,
    /// The account's paid order as the event reports it.
    pub order: Option<(Id, OrderRow)>,
    /// The mail the event sends, queued only the first time it applies.
    pub mail: Vec<BillingMail>,
}

/// How a paid period ends early.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarlyEnd {
    /// The subscription it ends, which must still be the account's.
    pub subscription: String,
    /// The order to refund, and the amount before tax, or `None`.
    pub refund: Option<(String, u64)>,
    /// The mail that confirms it.
    pub mail: Vec<BillingMail>,
}

async fn queue_all(txn: &mut impl WriteTxn, mail: &[BillingMail]) -> Result<()> {
    for message in mail {
        txn.queue_billing_mail(message).await?;
    }
    Ok(())
}

impl<B: Backend> Store<B> {
    /// The latest paid order of the account's subscription.
    pub async fn latest_order(&self, account: Id) -> Result<Option<OrderRow>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.latest_order(account).await })
            }))
            .await
    }

    /// Records the consent `account` gave before a checkout.
    pub async fn record_consent(&self, account: Id, row: ConsentRow) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_consent(account, &row).await })
            }))
            .await
    }

    /// The latest consent `account` gave, for any plan.
    pub async fn latest_consent(&self, account: Id) -> Result<Option<ConsentRow>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.latest_consent(account).await })
            }))
            .await
    }

    /// Deletes the consents given more than [`CONSENT_DAYS`] ago, and
    /// returns how many.
    pub async fn forget_consents(&self) -> Result<u64> {
        let before = self
            .clock()
            .now()
            .saturating_sub(CONSENT_DAYS.saturating_mul(DAY));
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.delete_consents_before(before).await })
            }))
            .await
    }

    /// Records the language `account` reads the account pages in, `en` or
    /// `fr`, on an account the activity table holds.
    pub async fn set_account_lang(&self, account: Id, lang: &'static str) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.set_account_lang(account, lang).await })
            }))
            .await
    }

    /// The language `account` reads the account pages in, when known.
    pub async fn account_lang(&self, account: Id) -> Result<Option<String>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.account_lang(account).await })
            }))
            .await
    }

    /// Records that the account's subscription ends when its period ends,
    /// and queues `mail`. Returns false when the account's subscription is
    /// no longer `subscription` or no longer grants a plan.
    pub async fn mark_canceling(
        &self,
        account: Id,
        subscription: String,
        mail: Vec<BillingMail>,
    ) -> Result<bool> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    let Some(mut row) = txn.account_plan(account).await? else {
                        return Ok(false);
                    };
                    if row.subscription != subscription || !row.state.grants() {
                        return Ok(false);
                    }
                    row.canceling = true;
                    txn.put_account_plan(account, &row).await?;
                    queue_all(txn, &mail).await?;
                    Ok(true)
                })
            }))
            .await
    }

    /// Ends the account's paid period now: the subscription ends, the
    /// account moves to `free` and its workspaces beyond that plan go over
    /// plan, the store records the refund and the revocation the server
    /// owes the provider, and `end`'s mail goes in the queue, all in one
    /// transaction (§26, Refunds and withdrawal). Returns what the server
    /// owes, or `None` when the account's subscription is no longer
    /// `end.subscription` or no longer grants a plan.
    pub async fn end_paid_period(&self, account: Id, end: EarlyEnd) -> Result<Option<RefundOwed>> {
        let billing = self.billing;
        let now = self.clock().now();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    let Some(mut row) = txn.account_plan(account).await? else {
                        return Ok(None);
                    };
                    if row.subscription != end.subscription || !row.state.grants() {
                        return Ok(None);
                    }
                    row.state = SubscriptionState::Ended;
                    row.canceling = true;
                    row.changed_at = row.changed_at.max(now);
                    txn.put_account_plan(account, &row).await?;
                    let (order_id, amount) = end.refund.unwrap_or_default();
                    let mut owed = RefundOwed {
                        id: 0,
                        account,
                        provider: row.provider.clone(),
                        subscription: row.subscription.clone(),
                        order_id,
                        amount,
                        revoke: true,
                    };
                    owed.id = txn.insert_refund_owed(&owed).await?;
                    queue_all(txn, &end.mail).await?;
                    reconcile(txn, account, billing, now / DAY).await?;
                    Ok(Some(owed))
                })
            }))
            .await
    }

    /// Up to 256 refunds and revocations the server owes, oldest first.
    pub async fn refunds_owed(&self) -> Result<Vec<RefundOwed>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.refunds_owed(256).await })
            }))
            .await
    }

    /// Records what's left of a refund owed: the amount still to refund
    /// and whether the revocation is still to do. Nothing left deletes it.
    pub async fn settle_refund(&self, id: u64, amount: u64, revoke: bool) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    if amount == 0 && !revoke {
                        txn.delete_refund_owed(id).await
                    } else {
                        txn.set_refund_owed(id, amount, revoke).await
                    }
                })
            }))
            .await
    }

    /// Up to `limit` stored subscriptions after the account `after`, by
    /// account.
    pub async fn subscriptions(
        &self,
        after: Option<Id>,
        limit: u32,
    ) -> Result<Vec<(Id, AccountPlanRow)>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.account_plans_after(after, limit).await })
            }))
            .await
    }

    /// Records a renewal reminder for the account's subscription and
    /// queues it. Returns false when the account's subscription is no
    /// longer `subscription`.
    pub async fn record_reminder(
        &self,
        account: Id,
        subscription: String,
        reminder: Reminder,
        mail: BillingMail,
    ) -> Result<bool> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    let same = txn
                        .account_plan(account)
                        .await?
                        .is_some_and(|row| row.subscription == subscription);
                    if !same {
                        return Ok(false);
                    }
                    txn.set_plan_reminders(
                        account,
                        reminder.reminded_for,
                        reminder.reminders,
                        reminder.reminded_day,
                    )
                    .await?;
                    txn.queue_billing_mail(&mail).await?;
                    Ok(true)
                })
            }))
            .await
    }

    /// Stores a price change the operator announced and returns its
    /// number.
    pub async fn announce_price_change(&self, row: PriceChangeRow) -> Result<u64> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_price_change(&row).await })
            }))
            .await
    }

    /// Every price change, oldest first.
    pub async fn price_changes(&self) -> Result<Vec<PriceChangeRow>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.price_changes().await })
            }))
            .await
    }

    /// How many notices the account got about a price change.
    pub async fn price_notices(&self, change: u64, account: Id) -> Result<u8> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.price_notices(change, account).await })
            }))
            .await
    }

    /// Records the account's `notices`-th notice about a price change and
    /// queues it. Returns false when another pass recorded it first.
    pub async fn record_price_notice(
        &self,
        change: u64,
        account: Id,
        notices: u8,
        mail: BillingMail,
    ) -> Result<bool> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    if txn.price_notices(change, account).await? >= notices {
                        return Ok(false);
                    }
                    txn.set_price_notices(change, account, notices).await?;
                    txn.queue_billing_mail(&mail).await?;
                    Ok(true)
                })
            }))
            .await
    }

    /// Queues one message about plans.
    pub async fn queue_billing_mail(&self, mail: BillingMail) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.queue_billing_mail(&mail).await })
            }))
            .await
    }

    /// Up to 256 queued messages about plans, oldest first.
    pub async fn billing_mail(&self) -> Result<Vec<QueuedMail>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.billing_mail(256).await })
            }))
            .await
    }

    /// Deletes a queued message that went out.
    pub async fn sent_billing_mail(&self, id: u64) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.delete_billing_mail(id).await })
            }))
            .await
    }
}

/// The renewal reminders a subscription got, as
/// [`Store::record_reminder`] stores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reminder {
    /// The period end the reminders went out for.
    pub reminded_for: Option<u64>,
    /// How many went out for that period.
    pub reminders: u8,
    /// The day a monthly plan's last yearly reminder went out.
    pub reminded_day: Option<u64>,
}
