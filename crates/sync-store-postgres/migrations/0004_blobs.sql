-- Blobs (docs/workspace-format/sync-v2.md §21, §29): one row per upload,
-- keyed by workspace and blob ID, naming the domain it was created for, its
-- uploading device, its padded ciphertext length, `ciphertext_digest` and how
-- far the upload reached. No row holds an unpadded size or a time: the blob
-- store's file times date each segment's upload.
CREATE TABLE blobs (
    workspace BYTEA NOT NULL,
    blob BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    device BYTEA NOT NULL,
    length BIGINT NOT NULL,
    digest BYTEA NOT NULL,
    upload_offset BIGINT NOT NULL,
    complete BOOLEAN NOT NULL,
    PRIMARY KEY (workspace, blob)
);

-- Each stored 1 MiB segment's BLAKE3 hash, by index. The hash also names the
-- segment's object in the blob store.
CREATE TABLE blob_segments (
    workspace BYTEA NOT NULL,
    blob BYTEA NOT NULL,
    idx BIGINT NOT NULL,
    hash BYTEA NOT NULL,
    PRIMARY KEY (workspace, blob, idx)
);

-- The operations that list each blob, by their domain and sequence.
CREATE TABLE blob_listings (
    workspace BYTEA NOT NULL,
    blob BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    seq BIGINT NOT NULL,
    PRIMARY KEY (workspace, blob, domain, seq)
);
