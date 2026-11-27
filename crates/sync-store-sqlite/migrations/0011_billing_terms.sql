-- What the terms of a paid plan need (docs/workspace-format/sync-v2.md §26
-- Billing, §29): the subscription's start, period, price and billing
-- country for reminders, refunds and mail, and the reminders sent for its
-- period.
ALTER TABLE account_plans ADD COLUMN period_start INTEGER;
ALTER TABLE account_plans ADD COLUMN started_at INTEGER;
ALTER TABLE account_plans ADD COLUMN amount INTEGER;
ALTER TABLE account_plans ADD COLUMN currency TEXT;
ALTER TABLE account_plans ADD COLUMN billing_interval TEXT;
ALTER TABLE account_plans ADD COLUMN canceling INTEGER NOT NULL DEFAULT 0;
ALTER TABLE account_plans ADD COLUMN country TEXT;
ALTER TABLE account_plans ADD COLUMN region TEXT;
ALTER TABLE account_plans ADD COLUMN reminded_for INTEGER;
ALTER TABLE account_plans ADD COLUMN reminders INTEGER NOT NULL DEFAULT 0;
ALTER TABLE account_plans ADD COLUMN reminded_day INTEGER;

-- The latest paid order of each account's subscription, which a refund
-- refunds.
CREATE TABLE latest_orders (
    account BLOB PRIMARY KEY,
    subscription TEXT NOT NULL,
    order_id TEXT NOT NULL,
    net INTEGER NOT NULL,
    currency TEXT NOT NULL,
    paid_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- The consent an account gave before each checkout, kept 3 years.
CREATE TABLE consents (
    account BLOB NOT NULL,
    accepted_at INTEGER NOT NULL,
    plan TEXT NOT NULL,
    version TEXT NOT NULL,
    immediate INTEGER NOT NULL,
    renewal INTEGER NOT NULL,
    PRIMARY KEY (account, accepted_at, plan)
) STRICT, WITHOUT ROWID;
CREATE INDEX consents_by_time ON consents (accepted_at);

-- Price changes the operator announced, and the notices each subscriber
-- got about each.
CREATE TABLE price_changes (
    change INTEGER PRIMARY KEY,
    plan TEXT NOT NULL,
    effective_day INTEGER NOT NULL,
    old_en TEXT NOT NULL,
    new_en TEXT NOT NULL,
    old_fr TEXT NOT NULL,
    new_fr TEXT NOT NULL
) STRICT;
CREATE TABLE price_notices (
    change INTEGER NOT NULL,
    account BLOB NOT NULL,
    notices INTEGER NOT NULL,
    PRIMARY KEY (change, account)
) STRICT, WITHOUT ROWID;

-- The refunds and revocations the server owes the payment provider, until
-- the provider accepts each.
CREATE TABLE refunds_owed (
    id INTEGER PRIMARY KEY,
    account BLOB NOT NULL,
    provider TEXT NOT NULL,
    subscription TEXT NOT NULL,
    order_id TEXT NOT NULL,
    amount INTEGER NOT NULL,
    revoke INTEGER NOT NULL
) STRICT;

-- Mail about plans that waits to go out; the server deletes each message
-- once it's sent.
CREATE TABLE billing_mail (
    id INTEGER PRIMARY KEY,
    recipient TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL
) STRICT;

-- The language an account reads the pages in, for its mail.
ALTER TABLE account_activity ADD COLUMN lang TEXT;
