-- History floors (docs/workspace-format/sync-v2.md §9): per account and
-- domain, the sequence at or below which the server serves the account no
-- entry. An approval sets the new member's floor in the workspace domain
-- to the covered_seq of the checkpoint committed with it.
CREATE TABLE history_floors (
    workspace BYTEA NOT NULL,
    account BYTEA NOT NULL,
    domain BYTEA NOT NULL,
    floor BIGINT NOT NULL,
    PRIMARY KEY (workspace, account, domain)
);
