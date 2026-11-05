-- Blobs (docs/workspace-format/sync-v2.md §21, §29): one row per upload,
-- keyed by workspace and blob ID, naming the domain it was created for, its
-- uploading device, its padded ciphertext length, `ciphertext_digest` and how
-- far the upload reached. No row holds an unpadded size or a time: the blob
-- store's file times date each segment's upload.
CREATE TABLE blobs (
    workspace BLOB NOT NULL,
    blob BLOB NOT NULL,
    domain BLOB NOT NULL,
    device BLOB NOT NULL,
    length INTEGER NOT NULL,
    digest BLOB NOT NULL,
    upload_offset INTEGER NOT NULL,
    complete INTEGER NOT NULL,
    PRIMARY KEY (workspace, blob)
) STRICT, WITHOUT ROWID;

-- Each stored 1 MiB segment's BLAKE3 hash, by index. The hash also names the
-- segment's object in the blob store.
CREATE TABLE blob_segments (
    workspace BLOB NOT NULL,
    blob BLOB NOT NULL,
    idx INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY (workspace, blob, idx)
) STRICT, WITHOUT ROWID;

-- The operations that list each blob, by their domain and sequence.
CREATE TABLE blob_listings (
    workspace BLOB NOT NULL,
    blob BLOB NOT NULL,
    domain BLOB NOT NULL,
    seq INTEGER NOT NULL,
    PRIMARY KEY (workspace, blob, domain, seq)
) STRICT, WITHOUT ROWID;
