-- Share links (docs/workspace-format/sync-v2.md §24, §29): one row per link,
-- keyed by the token's hash, naming the workspace, the domain, the creating
-- device, the expiry and the signed padded snapshot. The server never holds
-- the token, the link key or the verify key.
CREATE TABLE shares (
    token_hash BLOB NOT NULL PRIMARY KEY,
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    device BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    share BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX shares_by_workspace ON shares (workspace, expires_at);
