-- The sync server's tables for protocol v2 (docs/workspace-format/sync-v2.md).
-- IDs, digests and records are BLOBs. A chain counter is chosen by a client
-- and compared in queries, so it is stored as 8 big-endian bytes, whose byte
-- order is its unsigned order. Server-assigned numbers (sequences, key
-- sequences, revisions) stay INTEGERs. No table holds a per-record time.

-- The server's own state (§14): its restore generation and the key check of
-- its secret, which lives in a file beside the database.
CREATE TABLE server (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    restore_generation INTEGER NOT NULL,
    secret_check BLOB NOT NULL
) STRICT;

-- Account chains (§7). `accounts` names each account's current chain.
CREATE TABLE accounts (
    account BLOB PRIMARY KEY,
    genesis BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE account_links (
    account BLOB NOT NULL,
    genesis BLOB NOT NULL,
    seqno INTEGER NOT NULL,
    digest BLOB NOT NULL,
    link BLOB NOT NULL,
    PRIMARY KEY (account, genesis, seqno)
) STRICT, WITHOUT ROWID;

-- Sessions hold only the token's BLAKE3 hash, and no expiry (§7).
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,
    account BLOB NOT NULL,
    device BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE workspaces (
    workspace BLOB PRIMARY KEY,
    owner BLOB NOT NULL,
    creation_digest BLOB NOT NULL,
    creation_response BLOB NOT NULL,
    policy_revision INTEGER NOT NULL,
    policy_head BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX workspaces_by_owner ON workspaces (owner);

CREATE TABLE members (
    workspace BLOB NOT NULL,
    account BLOB NOT NULL,
    role INTEGER NOT NULL,
    PRIMARY KEY (workspace, account)
) STRICT, WITHOUT ROWID;

CREATE INDEX members_by_account ON members (account, workspace);

CREATE TABLE policy_changes (
    workspace BLOB NOT NULL,
    revision INTEGER NOT NULL,
    digest BLOB NOT NULL,
    record BLOB NOT NULL,
    PRIMARY KEY (workspace, revision)
) STRICT, WITHOUT ROWID;

CREATE TABLE profiles (
    workspace BLOB NOT NULL,
    profile_revision INTEGER NOT NULL,
    record BLOB NOT NULL,
    PRIMARY KEY (workspace, profile_revision)
) STRICT, WITHOUT ROWID;

-- Each domain's head row (§15): its latest sequence and its latest epoch
-- entry. Domains are keyed by workspace and ID.
CREATE TABLE domains (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    head_seq INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    epoch_digest BLOB NOT NULL,
    epoch_policy_revision INTEGER NOT NULL,
    -- The highest retention floor the server published (§19); ticket 59
    -- raises it.
    retention_floor INTEGER NOT NULL DEFAULT 0,
    -- The latest checkpoint's covered_seq, the due point the server
    -- publishes, and the entries and weight since the latest checkpoint
    -- entry (§17).
    checkpoint_seq INTEGER NOT NULL DEFAULT 0,
    checkpoint_due INTEGER NOT NULL DEFAULT 0,
    entries_since_checkpoint INTEGER NOT NULL DEFAULT 0,
    weight_since_checkpoint INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace, domain)
) STRICT, WITHOUT ROWID;

-- Each domain's log: operations, checkpoint bundles, epoch records and
-- policy entries, by sequence.
CREATE TABLE entries (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    seq INTEGER NOT NULL,
    record_type INTEGER NOT NULL,
    record BLOB NOT NULL,
    PRIMARY KEY (workspace, domain, seq)
) STRICT, WITHOUT ROWID;

-- A checkpoint names the latest policy and epoch entries at or below its
-- covered_seq (§17).
CREATE INDEX entries_by_type ON entries (workspace, domain, record_type, seq);

-- The cleartext fields of each operation that chains and live sets need
-- (§13, §14). The record itself is the entry at the same sequence.
CREATE TABLE operations (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    seq INTEGER NOT NULL,
    device BLOB NOT NULL,
    counter BLOB NOT NULL,
    rid BLOB NOT NULL,
    digest BLOB NOT NULL,
    is_delete INTEGER NOT NULL,
    class INTEGER NOT NULL,
    weight INTEGER NOT NULL,
    dropped INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace, domain, seq)
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX operations_by_position ON operations (workspace, domain, device, counter);
CREATE INDEX operations_by_rid ON operations (workspace, rid, device, counter);
CREATE INDEX operations_deletes ON operations (workspace, domain, is_delete, seq);

-- One row per cover an operation makes: every operation on `rid` from
-- `device` with a counter at or below `counter` is superseded.
CREATE TABLE covers (
    workspace BLOB NOT NULL,
    rid BLOB NOT NULL,
    device BLOB NOT NULL,
    counter BLOB NOT NULL,
    by_seq INTEGER NOT NULL,
    domain BLOB NOT NULL,
    PRIMARY KEY (workspace, rid, device, counter, by_seq)
) STRICT, WITHOUT ROWID;

-- The domain each object belongs to.
CREATE TABLE rids (
    workspace BLOB NOT NULL,
    rid BLOB NOT NULL,
    domain BLOB NOT NULL,
    PRIMARY KEY (workspace, rid)
) STRICT, WITHOUT ROWID;

-- Each device's chain head per domain, which pruning never deletes.
CREATE TABLE chain_heads (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    device BLOB NOT NULL,
    counter BLOB NOT NULL,
    digest BLOB NOT NULL,
    seq INTEGER NOT NULL,
    policy_revision INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (workspace, domain, device)
) STRICT, WITHOUT ROWID;

-- Epoch bundles, kept whole (§9). `record` is the epoch record alone, as
-- readers get it.
CREATE TABLE epochs (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    digest BLOB NOT NULL,
    previous BLOB NOT NULL,
    record BLOB NOT NULL,
    bundle BLOB NOT NULL,
    PRIMARY KEY (workspace, domain, epoch)
) STRICT, WITHOUT ROWID;

CREATE INDEX epochs_by_digest ON epochs (workspace, domain, digest);

-- The key stream (§26): epoch records and envelopes by key sequence.
CREATE TABLE key_entries (
    workspace BLOB NOT NULL,
    key_seq INTEGER NOT NULL,
    record_type INTEGER NOT NULL,
    domain BLOB NOT NULL,
    recipient BLOB,
    record BLOB NOT NULL,
    PRIMARY KEY (workspace, key_seq)
) STRICT, WITHOUT ROWID;

-- Checkpoints (§17, §19): each record, kept while the domain exists,
-- with its heads and, while it stays servable, its 4,096 bucket hashes.
CREATE TABLE checkpoints (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    number INTEGER NOT NULL,
    digest BLOB NOT NULL,
    seq INTEGER NOT NULL,
    covered_seq INTEGER NOT NULL,
    floor INTEGER NOT NULL,
    record BLOB NOT NULL,
    heads BLOB NOT NULL,
    buckets BLOB,
    PRIMARY KEY (workspace, domain, number)
) STRICT;

CREATE UNIQUE INDEX checkpoints_by_digest ON checkpoints (workspace, domain, digest);
CREATE UNIQUE INDEX checkpoints_by_covered_seq ON checkpoints (workspace, domain, covered_seq);
