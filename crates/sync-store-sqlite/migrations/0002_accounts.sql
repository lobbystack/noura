-- Accounts, sign-in and enrolled cards (docs/workspace-format/sync-v2.md §7,
-- §14, §29). Tokens and challenges are stored only as their BLAKE3 hashes.
-- Sessions carry no expiry; magic links, passkey sign-ups awaiting their
-- email, passkey ceremonies, enrollment challenges and approval requests
-- carry the expiry §29 allows, and the server deletes each when it's used
-- or 10 minutes after it issued it.

-- A signed-up account: its ID derives from this email under the server's
-- secret (§14), and no route changes the email.
CREATE TABLE account_emails (
    account BLOB PRIMARY KEY,
    email TEXT NOT NULL UNIQUE
) STRICT, WITHOUT ROWID;

-- Passkeys, as webauthn-rs serializes them, by credential ID.
CREATE TABLE passkeys (
    credential_id BLOB PRIMARY KEY,
    account BLOB NOT NULL,
    passkey BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX passkeys_by_account ON passkeys (account, credential_id);

-- Sessions of the account pages, ended by signing out.
CREATE TABLE web_sessions (
    token_hash BLOB PRIMARY KEY,
    account BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE magic_links (
    token_hash BLOB PRIMARY KEY,
    email TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- A passkey registered at sign-up, waiting for its email to be confirmed.
CREATE TABLE pending_passkeys (
    token_hash BLOB PRIMARY KEY,
    email TEXT NOT NULL,
    credential_id BLOB NOT NULL,
    passkey BLOB NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- A passkey ceremony's state between its two requests, so a request may
-- land on another server instance.
CREATE TABLE ceremonies (
    id BLOB PRIMARY KEY,
    kind INTEGER NOT NULL,
    account BLOB,
    email TEXT,
    state BLOB NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- Enrollment challenges (§7), which recovery proofs answer too (§12), by
-- their BLAKE3 hash (§29).
CREATE TABLE challenges (
    challenge_hash BLOB PRIMARY KEY,
    account BLOB NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- Approval requests (§10) until they expire: the account and card that
-- posted each, and, for one that names an invitation, the invitation and
-- the workspace it invites to, so the chain read route serves that
-- account's current chain to the workspace's members (§7). Tickets 47 and
-- 50 add the rest of a request.
CREATE TABLE approval_requests (
    commit_digest BLOB PRIMARY KEY,
    account BLOB NOT NULL,
    device BLOB NOT NULL,
    invitation BLOB,
    workspace BLOB,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX approval_requests_by_account ON approval_requests (account, commit_digest);

-- Cards that sessions of an account enrolled (§7). Whether a card is active
-- comes from the account chain.
CREATE TABLE cards (
    device BLOB PRIMARY KEY,
    account BLOB NOT NULL,
    card BLOB NOT NULL
) STRICT, WITHOUT ROWID;

-- The accounts whose heads each workspace's policy records, so the chain
-- read route finds the workspaces two accounts share (§7).
CREATE TABLE recorded_accounts (
    workspace BLOB NOT NULL,
    account BLOB NOT NULL,
    PRIMARY KEY (workspace, account)
) STRICT, WITHOUT ROWID;

CREATE INDEX recorded_accounts_by_account ON recorded_accounts (account, workspace);

-- The recovery identity a session proved it holds (§12), for the rest of
-- that session.
ALTER TABLE sessions ADD COLUMN recovery BLOB;
