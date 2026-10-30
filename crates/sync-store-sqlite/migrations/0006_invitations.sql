-- Invitations (docs/workspace-format/sync-v2.md §11, §29) while they're
-- open: the ID the inviting device chose, the token's hash, the workspace,
-- the role, the inviting account, the accepting account once known, the
-- expiry and the padded context. A row whose accepted_by is set is
-- accepted, otherwise pending. The server never holds the token or the
-- secret, and deletes the row once the invitation is approved, revoked or
-- expired.
CREATE TABLE invitations (
    invitation BLOB NOT NULL PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    workspace BLOB NOT NULL,
    role INTEGER NOT NULL,
    inviter BLOB NOT NULL,
    accepted_by BLOB,
    expires_at INTEGER NOT NULL,
    invite_context BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX invitations_by_workspace ON invitations (workspace, expires_at);

-- The IDs of invitations approved or revoked before their expiry, kept
-- until that expiry, so a late retry of a creation can't open the link
-- again (§26). Nothing else of the invitation stays.
CREATE TABLE ended_invitations (
    invitation BLOB NOT NULL PRIMARY KEY,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- The approval requests that name a workspace, for pending-devices (§26).
CREATE INDEX approval_requests_by_workspace ON approval_requests (workspace, commit_digest);
