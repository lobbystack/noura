-- Server restores and manual recovery (docs/workspace-format/sync-v2.md §7,
-- §14). `backup_stamp` marks a backup copy, which `serve` refuses until
-- `restore` or `mark-restored` clears it.
ALTER TABLE server ADD COLUMN backup_stamp INTEGER NOT NULL DEFAULT 0;

-- Each serving instance's lease, which it refreshes while it runs, so
-- `restore` and `mark-restored` can refuse a database that is still served.
CREATE TABLE instances (
    instance BLOB PRIMARY KEY,
    lease_until INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- Each account's pending reset code, as its BLAKE3 hash. A new code replaces
-- the old one, and using it or letting it expire deletes the row.
CREATE TABLE reset_codes (
    account BLOB PRIMARY KEY,
    code_hash BLOB NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX reset_codes_by_expiry ON reset_codes (expires_at);

-- A workspace its owner closed: every route answers `not_found` and the
-- collector reclaims its domains.
ALTER TABLE workspaces ADD COLUMN closed INTEGER NOT NULL DEFAULT 0;
