-- Retention, quota and the blob collector (docs/workspace-format/sync-v2.md
-- §19, §20). Each domain's head row carries its usage, history weight and
-- the log's running weight. A checkpoint keeps the running weight at its
-- entry, so the retention floor comes from one indexed read and no table
-- maps sequences to dates.
ALTER TABLE domains ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE domains ADD COLUMN log_weight INTEGER NOT NULL DEFAULT 0;
ALTER TABLE domains ADD COLUMN usage_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE domains ADD COLUMN history_bytes INTEGER NOT NULL DEFAULT 0;
-- The floor pruning finished, and the workspace's last quota crossing this
-- domain handled.
ALTER TABLE domains ADD COLUMN pruned_floor INTEGER NOT NULL DEFAULT 0;
ALTER TABLE domains ADD COLUMN crossing_seen INTEGER NOT NULL DEFAULT 0;

ALTER TABLE checkpoints ADD COLUMN log_weight INTEGER NOT NULL DEFAULT 0;

-- Whether usage and history together stand above the quota, and how many
-- times they crossed it.
ALTER TABLE workspaces ADD COLUMN quota_pressure INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workspaces ADD COLUMN quota_crossings INTEGER NOT NULL DEFAULT 0;

-- A deletion time only while a blob is incomplete or unlisted (§29), the
-- collector's mark, and how many operations live at the head list it.
ALTER TABLE blobs ADD COLUMN delete_after INTEGER;
ALTER TABLE blobs ADD COLUMN marked INTEGER NOT NULL DEFAULT 0;
ALTER TABLE blobs ADD COLUMN live_listings INTEGER NOT NULL DEFAULT 0;

CREATE INDEX blobs_collectable ON blobs (delete_after) WHERE delete_after IS NOT NULL;
CREATE INDEX blobs_marked ON blobs (workspace, blob) WHERE marked = 1;
CREATE INDEX blobs_by_domain ON blobs (workspace, domain);
CREATE INDEX blob_listings_by_seq ON blob_listings (workspace, domain, seq);
CREATE INDEX covers_by_domain ON covers (workspace, domain, by_seq);
CREATE INDEX shares_expiring ON shares (expires_at);
