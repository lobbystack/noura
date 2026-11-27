-- Plans (docs/workspace-format/sync-v2.md §20, §29). Each workspace keeps
-- the order the server created it in, whether its owner chose to keep it
-- active, and while it's over plan the day it went over and the notices
-- sent. Days count from the Unix epoch.
ALTER TABLE workspaces ADD COLUMN ordinal INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workspaces ADD COLUMN kept INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workspaces ADD COLUMN over_plan_since INTEGER;
ALTER TABLE workspaces ADD COLUMN over_plan_notices INTEGER NOT NULL DEFAULT 0;
UPDATE workspaces SET ordinal = (
    SELECT COUNT(*) FROM workspaces AS earlier WHERE earlier.workspace <= workspaces.workspace
);
CREATE INDEX workspaces_over_plan ON workspaces (over_plan_since) WHERE over_plan_since IS NOT NULL;

-- An account's subscription with its payment provider. The provider keeps
-- the name, address and payment details.
CREATE TABLE account_plans (
    account BLOB PRIMARY KEY,
    provider TEXT NOT NULL,
    customer TEXT NOT NULL,
    subscription TEXT NOT NULL,
    plan TEXT NOT NULL,
    state INTEGER NOT NULL,
    period_end INTEGER,
    changed_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- The webhook events the server applied, so a retried delivery changes
-- nothing; the server deletes a row 100 days after its day.
CREATE TABLE billing_events (
    provider TEXT NOT NULL,
    event TEXT NOT NULL,
    day INTEGER NOT NULL,
    PRIMARY KEY (provider, event)
) STRICT, WITHOUT ROWID;
CREATE INDEX billing_events_by_day ON billing_events (day);

-- The month each account last sent an authenticated request, counted from
-- January of year 0, and the inactivity warnings it got since.
CREATE TABLE account_activity (
    account BLOB PRIMARY KEY,
    month INTEGER NOT NULL,
    warnings INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;
CREATE INDEX account_activity_by_month ON account_activity (month);
