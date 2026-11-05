-- Server restores and manual recovery (docs/workspace-format/sync-v2.md §7,
-- §14).

-- Each serving instance's lease, which it refreshes while it runs, so
-- `mark-restored` can refuse a database that is still served.
CREATE TABLE instances (
    instance BYTEA PRIMARY KEY,
    lease_until BIGINT NOT NULL
);

-- Each account's pending reset code, as its BLAKE3 hash. A new code replaces
-- the old one, and using it or letting it expire deletes the row.
CREATE TABLE reset_codes (
    account BYTEA PRIMARY KEY,
    code_hash BYTEA NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE INDEX reset_codes_by_expiry ON reset_codes (expires_at);

-- A workspace its owner closed: every route answers `not_found` and the
-- collector reclaims its domains.
ALTER TABLE workspaces ADD COLUMN closed SMALLINT NOT NULL DEFAULT 0;
