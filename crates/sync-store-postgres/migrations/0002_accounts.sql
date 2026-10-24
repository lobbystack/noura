-- Accounts, sign-in and enrolled cards on PostgreSQL
-- (docs/workspace-format/sync-v2.md §7, §14, §29). They mirror
-- sync-store-sqlite's. Tokens and challenges are stored only as their
-- BLAKE3 hashes. Sessions carry no expiry; magic links, passkey sign-ups
-- awaiting their email, passkey ceremonies, enrollment challenges and
-- approval requests carry the expiry §29 allows, in Unix seconds, and the
-- server deletes each when it's used or 10 minutes after it issued it.

-- A signed-up account: its ID derives from this email under the server's
-- secret (§14), and no route changes the email.
CREATE TABLE account_emails (
    account BYTEA PRIMARY KEY,
    email TEXT NOT NULL UNIQUE
);

-- Passkeys, as webauthn-rs serializes them, by credential ID.
CREATE TABLE passkeys (
    credential_id BYTEA PRIMARY KEY,
    account BYTEA NOT NULL,
    passkey BYTEA NOT NULL
);

CREATE INDEX passkeys_by_account ON passkeys (account, credential_id);

-- Sessions of the account pages, ended by signing out.
CREATE TABLE web_sessions (
    token_hash BYTEA PRIMARY KEY,
    account BYTEA NOT NULL
);

CREATE TABLE magic_links (
    token_hash BYTEA PRIMARY KEY,
    email TEXT NOT NULL,
    expires_at BIGINT NOT NULL
);

-- A passkey registered at sign-up, waiting for its email to be confirmed.
CREATE TABLE pending_passkeys (
    token_hash BYTEA PRIMARY KEY,
    email TEXT NOT NULL,
    credential_id BYTEA NOT NULL,
    passkey BYTEA NOT NULL,
    expires_at BIGINT NOT NULL
);

-- A passkey ceremony's state between its two requests, so a request may
-- land on another server instance.
CREATE TABLE ceremonies (
    id BYTEA PRIMARY KEY,
    kind SMALLINT NOT NULL,
    account BYTEA,
    email TEXT,
    state BYTEA NOT NULL,
    expires_at BIGINT NOT NULL
);

-- Enrollment challenges (§7), which recovery proofs answer too (§12), by
-- their BLAKE3 hash (§29).
CREATE TABLE challenges (
    challenge_hash BYTEA PRIMARY KEY,
    account BYTEA NOT NULL,
    expires_at BIGINT NOT NULL
);

-- Approval requests (§10) until they expire: the account and card that
-- posted each, and, for one that names an invitation, the invitation and
-- the workspace it invites to, so the chain read route serves that
-- account's current chain to the workspace's members (§7). Tickets 47 and
-- 50 add the rest of a request.
CREATE TABLE approval_requests (
    commit_digest BYTEA PRIMARY KEY,
    account BYTEA NOT NULL,
    device BYTEA NOT NULL,
    invitation BYTEA,
    workspace BYTEA,
    expires_at BIGINT NOT NULL
);

CREATE INDEX approval_requests_by_account ON approval_requests (account, commit_digest);

-- The expiring rows, which the server deletes every 10 seconds.
CREATE INDEX magic_links_by_expiry ON magic_links (expires_at);
CREATE INDEX pending_passkeys_by_expiry ON pending_passkeys (expires_at);
CREATE INDEX ceremonies_by_expiry ON ceremonies (expires_at);
CREATE INDEX challenges_by_expiry ON challenges (expires_at);
CREATE INDEX approval_requests_by_expiry ON approval_requests (expires_at);

-- Cards that sessions of an account enrolled (§7). Whether a card is active
-- comes from the account chain.
CREATE TABLE cards (
    device BYTEA PRIMARY KEY,
    account BYTEA NOT NULL,
    card BYTEA NOT NULL
);

-- The accounts whose heads each workspace's policy records, so the chain
-- read route finds the workspaces two accounts share (§7).
CREATE TABLE recorded_accounts (
    workspace BYTEA NOT NULL,
    account BYTEA NOT NULL,
    PRIMARY KEY (workspace, account)
);

CREATE INDEX recorded_accounts_by_account ON recorded_accounts (account, workspace);

-- The recovery identity a session proved it holds (§12), for the rest of
-- that session.
ALTER TABLE sessions ADD COLUMN recovery BYTEA;
