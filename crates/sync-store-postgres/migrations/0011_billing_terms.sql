-- What the terms of a paid plan need (docs/workspace-format/sync-v2.md §26
-- Billing, §29): the subscription's start, period, price and billing
-- country for reminders, refunds and mail, and the reminders sent for its
-- period.
ALTER TABLE account_plans ADD COLUMN period_start BIGINT;
ALTER TABLE account_plans ADD COLUMN started_at BIGINT;
ALTER TABLE account_plans ADD COLUMN amount BIGINT;
ALTER TABLE account_plans ADD COLUMN currency TEXT;
ALTER TABLE account_plans ADD COLUMN billing_interval TEXT;
ALTER TABLE account_plans ADD COLUMN canceling BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE account_plans ADD COLUMN country TEXT;
ALTER TABLE account_plans ADD COLUMN region TEXT;
ALTER TABLE account_plans ADD COLUMN reminded_for BIGINT;
ALTER TABLE account_plans ADD COLUMN reminders BIGINT NOT NULL DEFAULT 0;
ALTER TABLE account_plans ADD COLUMN reminded_day BIGINT;

-- The latest paid order of each account's subscription, which a refund
-- refunds.
CREATE TABLE latest_orders (
    account BYTEA PRIMARY KEY,
    subscription TEXT NOT NULL,
    order_id TEXT NOT NULL,
    net BIGINT NOT NULL,
    currency TEXT NOT NULL,
    paid_at BIGINT NOT NULL
);

-- The consent an account gave before each checkout, kept 3 years.
CREATE TABLE consents (
    account BYTEA NOT NULL,
    accepted_at BIGINT NOT NULL,
    plan TEXT NOT NULL,
    version TEXT NOT NULL,
    immediate BOOLEAN NOT NULL,
    renewal BOOLEAN NOT NULL,
    PRIMARY KEY (account, accepted_at, plan)
);
CREATE INDEX consents_by_time ON consents (accepted_at);

-- Price changes the operator announced, and the notices each subscriber
-- got about each.
CREATE TABLE price_changes (
    change BIGSERIAL PRIMARY KEY,
    plan TEXT NOT NULL,
    effective_day BIGINT NOT NULL,
    old_en TEXT NOT NULL,
    new_en TEXT NOT NULL,
    old_fr TEXT NOT NULL,
    new_fr TEXT NOT NULL
);
CREATE TABLE price_notices (
    change BIGINT NOT NULL,
    account BYTEA NOT NULL,
    notices BIGINT NOT NULL,
    PRIMARY KEY (change, account)
);

-- The refunds and revocations the server owes the payment provider, until
-- the provider accepts each.
CREATE TABLE refunds_owed (
    id BIGSERIAL PRIMARY KEY,
    account BYTEA NOT NULL,
    provider TEXT NOT NULL,
    subscription TEXT NOT NULL,
    order_id TEXT NOT NULL,
    amount BIGINT NOT NULL,
    revoke BOOLEAN NOT NULL
);

-- Mail about plans that waits to go out; the server deletes each message
-- once it's sent.
CREATE TABLE billing_mail (
    id BIGSERIAL PRIMARY KEY,
    recipient TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL
);

-- The language an account reads the pages in, for its mail.
ALTER TABLE account_activity ADD COLUMN lang TEXT;
