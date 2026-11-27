-- Plans (docs/workspace-format/sync-v2.md §20, §29). Each workspace keeps
-- the order the server created it in, whether its owner chose to keep it
-- active, and while it's over plan the day it went over and the notices
-- sent. Days count from the Unix epoch.
CREATE SEQUENCE workspace_order;
ALTER TABLE workspaces ADD COLUMN ordinal BIGINT NOT NULL DEFAULT nextval('workspace_order');
ALTER TABLE workspaces ADD COLUMN kept BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE workspaces ADD COLUMN over_plan_since BIGINT;
ALTER TABLE workspaces ADD COLUMN over_plan_notices BIGINT NOT NULL DEFAULT 0;
CREATE INDEX workspaces_over_plan ON workspaces (over_plan_since) WHERE over_plan_since IS NOT NULL;

-- An account's subscription with its payment provider. The provider keeps
-- the name, address and payment details.
CREATE TABLE account_plans (
    account BYTEA PRIMARY KEY,
    provider TEXT NOT NULL,
    customer TEXT NOT NULL,
    subscription TEXT NOT NULL,
    plan TEXT NOT NULL,
    state SMALLINT NOT NULL,
    period_end BIGINT,
    changed_at BIGINT NOT NULL
);

-- The webhook events the server applied, so a retried delivery changes
-- nothing; the server deletes a row 100 days after its day.
CREATE TABLE billing_events (
    provider TEXT NOT NULL,
    event TEXT NOT NULL,
    day BIGINT NOT NULL,
    PRIMARY KEY (provider, event)
);
CREATE INDEX billing_events_by_day ON billing_events (day);

-- The month each account last sent an authenticated request, counted from
-- January of year 0, and the inactivity warnings it got since.
CREATE TABLE account_activity (
    account BYTEA PRIMARY KEY,
    month BIGINT NOT NULL,
    warnings BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX account_activity_by_month ON account_activity (month);
