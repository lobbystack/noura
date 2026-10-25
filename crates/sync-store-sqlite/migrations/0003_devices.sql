-- Devices (docs/workspace-format/sync-v2.md §7, §9, §10, §29): the device
-- authorization grant, sessions that a sign-in starts before their device
-- enrolls and that a removing link ends, approval requests, offers and
-- reveals, and re-wrap requests. Every grant, request and offer carries the
-- expiry §29 allows, and the server deletes each when it's used or 10
-- minutes after it issued it.

-- A session that a device grant started holds no card until its device
-- enrolls; its `device` is then 16 zero bytes, the reserved ID (§5). A link
-- that removes the session's card marks the session removed with the
-- `device_removed` detail, the removing link's genesis and seqno, and the
-- session keeps only the right to read its account's chains (§7).
ALTER TABLE sessions ADD COLUMN removed BLOB;

CREATE INDEX sessions_by_account ON sessions (account, device);

-- Device authorization grants (RFC 8628, §7), by the BLAKE3 hash of the
-- device code and of the user code. `account` is set once a signed-in
-- person approves the code on the device page; `denied` once they refuse
-- it. `poll_interval` is the polling interval in seconds, raised by 5 after each
-- `slow_down`, and `next_poll` the earliest Unix second the next poll may
-- arrive.
CREATE TABLE device_grants (
    device_code_hash BLOB PRIMARY KEY,
    user_code_hash BLOB NOT NULL UNIQUE,
    account BLOB,
    denied INTEGER NOT NULL DEFAULT 0,
    poll_interval INTEGER NOT NULL,
    next_poll INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- Approval requests (§10) until they expire, now with the whole request:
-- the candidate's card, its account head for an invitation or a reset, the
-- invitation and its proof, the workspace a reset names, and the encoded
-- request, so a retry with identical bytes gets the original result.
DROP TABLE approval_requests;

CREATE TABLE approval_requests (
    commit_digest BLOB PRIMARY KEY,
    account BLOB NOT NULL,
    device BLOB NOT NULL,
    card BLOB NOT NULL,
    candidate_head BLOB,
    invitation BLOB,
    invite_proof BLOB,
    workspace BLOB,
    request BLOB NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX approval_requests_by_account ON approval_requests (account, commit_digest);

-- Approval offers (§10) until they expire, by the offer's digest, with the
-- commitment and card digest they answer, the approving device and its
-- account, and the candidate's nonce once it reveals it.
CREATE TABLE approval_offers (
    offer_digest BLOB PRIMARY KEY,
    commit_digest BLOB NOT NULL,
    candidate_card_digest BLOB NOT NULL,
    approver BLOB NOT NULL,
    account BLOB NOT NULL,
    offer BLOB NOT NULL,
    nc BLOB,
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX approval_offers_by_card ON approval_offers (candidate_card_digest, offer_digest);

-- The epoch each envelope and re-wrap request belongs to, so the server
-- deletes an epoch's envelopes and closes its requests once the next epoch
-- of the domain commits (§9). Epoch records carry their own number.
ALTER TABLE key_entries ADD COLUMN epoch INTEGER;

-- Each workspace's latest key sequence, which never goes back when the
-- server deletes an old epoch's envelopes or a closed re-wrap request, so
-- no key stream row ever takes a sequence a reader already passed (§15).
CREATE TABLE key_sequences (
    workspace BLOB PRIMARY KEY,
    key_seq INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

INSERT INTO key_sequences (workspace, key_seq)
SELECT workspace, MAX(key_seq) FROM key_entries GROUP BY workspace;

CREATE INDEX key_entries_by_domain ON key_entries (workspace, domain, epoch);

-- Open re-wrap requests (§9), one per device and epoch of a domain, with
-- the key sequence of the request's key stream row, and the devices that
-- answered each with a lone envelope.
CREATE TABLE rewrap_requests (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    device BLOB NOT NULL,
    key_seq INTEGER NOT NULL,
    PRIMARY KEY (workspace, domain, epoch, device)
) STRICT, WITHOUT ROWID;

CREATE TABLE rewrap_answers (
    workspace BLOB NOT NULL,
    domain BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    device BLOB NOT NULL,
    wrapper BLOB NOT NULL,
    key_seq INTEGER NOT NULL,
    PRIMARY KEY (workspace, domain, epoch, device, wrapper)
) STRICT, WITHOUT ROWID;

-- The sequence of each epoch's log entry, so the server finds the records
-- written under an epoch that a replacement would skip (§9).
ALTER TABLE epochs ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;
