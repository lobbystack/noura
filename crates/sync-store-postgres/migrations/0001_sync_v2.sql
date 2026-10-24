-- The sync server's tables for protocol v2 on PostgreSQL
-- (docs/workspace-format/sync-v2.md). They mirror sync-store-sqlite's.
-- IDs, digests and records are BYTEA. A chain counter is chosen by a client
-- and compared in queries, so it is stored as 8 big-endian bytes, whose byte
-- order is its unsigned order. Server-assigned numbers (sequences, key
-- sequences, revisions) are BIGINTs. No table holds a per-record time, and
-- `serve` refuses to run with `track_commit_timestamp` on (§29).

-- The server's own state (§14): its restore generation and the key check of
-- its secret, which every instance gets outside the database.
CREATE TABLE server (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    restore_generation BIGINT NOT NULL,
    secret_check BYTEA NOT NULL
);

-- Account chains (§7). `accounts` names each account's current chain; an
-- append locks only its account's row (§15).
CREATE TABLE accounts (
    account BYTEA PRIMARY KEY,
    genesis BYTEA NOT NULL
);

CREATE TABLE account_links (
    account BYTEA NOT NULL,
    genesis BYTEA NOT NULL,
    seqno BIGINT NOT NULL,
    digest BYTEA NOT NULL,
    link BYTEA NOT NULL,
    PRIMARY KEY (account, genesis, seqno)
);

-- Sessions hold only the token's BLAKE3 hash, and no expiry (§7).
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    account BYTEA NOT NULL,
    device BYTEA NOT NULL
);

-- No push writes this row: the policy lock is an advisory lock (§15).
CREATE TABLE workspaces (
    workspace BYTEA PRIMARY KEY,
    owner BYTEA NOT NULL,
    creation_digest BYTEA NOT NULL,
    creation_response BYTEA NOT NULL,
    policy_revision BIGINT NOT NULL,
    policy_head BYTEA NOT NULL
);

CREATE INDEX workspaces_by_owner ON workspaces (owner);

-- Each workspace's key-sequence row (§15), which a push that carries an
-- epoch bundle or envelope locks.
CREATE TABLE key_sequences (
    workspace BYTEA PRIMARY KEY,
    key_seq BIGINT NOT NULL
);

CREATE TABLE members (
    workspace BYTEA NOT NULL,
    account BYTEA NOT NULL,
    role SMALLINT NOT NULL,
    PRIMARY KEY (workspace, account)
);

CREATE INDEX members_by_account ON members (account, workspace);

CREATE TABLE policy_changes (
    workspace BYTEA NOT NULL,
    revision BIGINT NOT NULL,
    digest BYTEA NOT NULL,
    record BYTEA NOT NULL,
    PRIMARY KEY (workspace, revision)
);

CREATE TABLE profiles (
    workspace BYTEA NOT NULL,
    profile_revision BIGINT NOT NULL,
    record BYTEA NOT NULL,
    PRIMARY KEY (workspace, profile_revision)
);

-- Each domain's head row (§15): its latest sequence and its latest epoch
-- entry. A push assigns the domain's sequences while it holds this row.
CREATE TABLE domains (
    workspace BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    head_seq BIGINT NOT NULL,
    epoch BIGINT NOT NULL,
    epoch_digest BYTEA NOT NULL,
    epoch_policy_revision BIGINT NOT NULL,
    -- The highest retention floor the server published (§19); ticket 59
    -- raises it.
    retention_floor BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace, domain)
);

-- Each domain's log: operations, checkpoint bundles, epoch records and
-- policy entries, by sequence.
CREATE TABLE entries (
    workspace BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    seq BIGINT NOT NULL,
    record_type SMALLINT NOT NULL,
    record BYTEA NOT NULL,
    PRIMARY KEY (workspace, domain, seq)
);

-- The cleartext fields of each operation that chains and live sets need
-- (§13, §14). The record itself is the entry at the same sequence.
CREATE TABLE operations (
    workspace BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    seq BIGINT NOT NULL,
    device BYTEA NOT NULL,
    counter BYTEA NOT NULL,
    rid BYTEA NOT NULL,
    digest BYTEA NOT NULL,
    is_delete BOOLEAN NOT NULL,
    class SMALLINT NOT NULL,
    weight BIGINT NOT NULL,
    dropped BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (workspace, domain, seq)
);

CREATE UNIQUE INDEX operations_by_position ON operations (workspace, domain, device, counter);
CREATE INDEX operations_by_rid ON operations (workspace, rid, device, counter);
CREATE INDEX operations_deletes ON operations (workspace, domain, is_delete, seq);

-- One row per cover an operation makes: every operation on `rid` from
-- `device` with a counter at or below `counter` is superseded.
CREATE TABLE covers (
    workspace BYTEA NOT NULL,
    rid BYTEA NOT NULL,
    device BYTEA NOT NULL,
    counter BYTEA NOT NULL,
    by_seq BIGINT NOT NULL,
    domain BYTEA NOT NULL,
    PRIMARY KEY (workspace, rid, device, counter, by_seq)
);

-- The domain each object belongs to.
CREATE TABLE rids (
    workspace BYTEA NOT NULL,
    rid BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    PRIMARY KEY (workspace, rid)
);

-- Each device's chain head per domain, which pruning never deletes.
CREATE TABLE chain_heads (
    workspace BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    device BYTEA NOT NULL,
    counter BYTEA NOT NULL,
    digest BYTEA NOT NULL,
    seq BIGINT NOT NULL,
    policy_revision BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    PRIMARY KEY (workspace, domain, device)
);

-- Epoch bundles, kept whole (§9). `record` is the epoch record alone, as
-- readers get it.
CREATE TABLE epochs (
    workspace BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    epoch BIGINT NOT NULL,
    digest BYTEA NOT NULL,
    previous BYTEA NOT NULL,
    record BYTEA NOT NULL,
    bundle BYTEA NOT NULL,
    PRIMARY KEY (workspace, domain, epoch)
);

CREATE INDEX epochs_by_digest ON epochs (workspace, domain, digest);

-- The key stream (§26): epoch records and envelopes by key sequence.
CREATE TABLE key_entries (
    workspace BYTEA NOT NULL,
    key_seq BIGINT NOT NULL,
    record_type SMALLINT NOT NULL,
    domain BYTEA NOT NULL,
    recipient BYTEA,
    record BYTEA NOT NULL,
    PRIMARY KEY (workspace, key_seq)
);

-- Checkpoint records, which operations name (§17). Ticket 58 fills it.
CREATE TABLE checkpoints (
    workspace BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    covered_seq BIGINT NOT NULL,
    digest BYTEA NOT NULL,
    PRIMARY KEY (workspace, domain, covered_seq, digest)
);

-- The limiter's counters (§26, Rate limits), which every instance shares:
-- a caller's key, when its current window ends in milliseconds since the
-- Unix epoch, and what the key used in that window. The limiter deletes
-- the rows of windows that ended.
CREATE TABLE rate_counters (
    key BYTEA NOT NULL,
    window_ends BIGINT NOT NULL,
    used BIGINT NOT NULL,
    PRIMARY KEY (key, window_ends)
);

CREATE INDEX rate_counters_by_end ON rate_counters (window_ends);
