//! The store's reads and writes on PostgreSQL, each a compile-checked
//! query. They mirror sync-store-sqlite's, with the locks of §15 taken for
//! real.

use std::time::Duration;

use sqlx::{PgConnection, Postgres, Transaction};
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::live::LiveOp;
use sync_protocol::messages::DeviceRemovedDetail;
use sync_protocol::records::{AccountHead, OpClass, Ref, Role};
use sync_protocol::types::{Digest, Id, Key};
use sync_store::{
    ApprovalRequestRow, BlobRow, BlobToCollect, CeremonyKind, CeremonyRow, ChainHeadRow,
    CheckpointProgress, CheckpointRow, DeleteRow, DeviceGrantRow, DomainHistory, DomainRow,
    EpochRow, Error, InvitationRow, KeyRow, LinkRow, LockPlan, OfferRow, PasskeyRow, PendingDomain,
    PendingPasskeyRow, PruneScan, PrunedEntry, RawEntry, ReadTxn, RemovedBy, Result, RewrapRow,
    Session, SessionRow, ShareRow, WorkspaceRow, WriteTxn,
};

/// One transaction: a read-only REPEATABLE READ snapshot for reads, or a
/// READ COMMITTED write transaction.
pub struct PgTxn {
    transaction: Option<Transaction<'static, Postgres>>,
    /// A test seam: how long a write transaction waits right after it
    /// reads a workspace's policy head.
    policy_read_delay: Option<Duration>,
}

impl PgTxn {
    pub(crate) fn new(transaction: Transaction<'static, Postgres>) -> Self {
        Self {
            transaction: Some(transaction),
            policy_read_delay: None,
        }
    }

    pub(crate) fn writer(
        transaction: Transaction<'static, Postgres>,
        policy_read_delay: Option<Duration>,
    ) -> Self {
        Self {
            transaction: Some(transaction),
            policy_read_delay,
        }
    }

    fn c(&mut self) -> &mut PgConnection {
        match &mut self.transaction {
            Some(transaction) => transaction,
            None => unreachable!("a finished transaction takes no query"),
        }
    }

    /// Runs a savepoint statement inside the transaction.
    pub(crate) async fn execute(&mut self, sql: &'static str) -> std::result::Result<(), String> {
        sqlx::raw_sql(sql)
            .execute(self.c())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Commits the transaction.
    pub(crate) async fn commit(&mut self) -> std::result::Result<(), String> {
        match self.transaction.take() {
            Some(transaction) => transaction.commit().await.map_err(|e| e.to_string()),
            None => Err("the transaction already ended".into()),
        }
    }

    /// Rolls the transaction back.
    pub(crate) async fn rollback(&mut self) {
        if let Some(transaction) = self.transaction.take() {
            let _ = transaction.rollback().await;
        }
    }
}

fn id(bytes: Vec<u8>) -> Result<Id> {
    bytes
        .try_into()
        .map(Id)
        .map_err(|_| Error::internal("an ID column isn't 16 bytes"))
}

fn digest(bytes: Vec<u8>) -> Result<Digest> {
    bytes
        .try_into()
        .map(Digest)
        .map_err(|_| Error::internal("a digest column isn't 32 bytes"))
}

/// A chain counter as 8 big-endian bytes, whose byte order is its order.
fn counter_bytes(counter: u64) -> [u8; 8] {
    counter.to_be_bytes()
}

fn counter(bytes: Vec<u8>) -> Result<u64> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| Error::internal("a counter column isn't 8 bytes"))?;
    Ok(u64::from_be_bytes(bytes))
}

/// A server-assigned or server-checked number as a BIGINT.
fn int(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::internal("a number exceeds a BIGINT"))
}

/// A bound for "every sequence".
fn at_most(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn uint(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::internal("a negative number in an unsigned column"))
}

fn byte(value: i16) -> Result<u8> {
    u8::try_from(value).map_err(|_| Error::internal("a type column exceeds a byte"))
}

fn class_code(class: OpClass) -> i16 {
    match class {
        OpClass::State => 1,
        OpClass::Patch => 2,
    }
}

fn class(code: i16) -> Result<OpClass> {
    match code {
        1 => Ok(OpClass::State),
        2 => Ok(OpClass::Patch),
        _ => Err(Error::internal("unknown operation class")),
    }
}

fn db(error: sqlx::Error) -> Error {
    Error::internal(error)
}

fn key(bytes: Vec<u8>) -> Result<Key> {
    bytes
        .try_into()
        .map(Key)
        .map_err(|_| Error::internal("a stored key isn't 32 bytes"))
}

fn removed_by(bytes: Option<Vec<u8>>) -> Result<Option<RemovedBy>> {
    bytes
        .map(|bytes| {
            DeviceRemovedDetail::from_bytes(&bytes)
                .map(|d| RemovedBy {
                    genesis: d.genesis,
                    seqno: d.seqno,
                })
                .map_err(Error::internal)
        })
        .transpose()
}

fn account_head(bytes: Option<Vec<u8>>) -> Result<Option<AccountHead>> {
    bytes
        .map(|bytes| AccountHead::from_bytes(&bytes).map_err(Error::internal))
        .transpose()
}

#[allow(clippy::too_many_arguments)]
fn request_row(
    commit_digest: Vec<u8>,
    account: Vec<u8>,
    device: Vec<u8>,
    card: Vec<u8>,
    candidate_head: Option<Vec<u8>>,
    invitation: Option<Vec<u8>>,
    invite_proof: Option<Vec<u8>>,
    workspace: Option<Vec<u8>>,
    request: Vec<u8>,
    expires_at: i64,
) -> Result<ApprovalRequestRow> {
    Ok(ApprovalRequestRow {
        commit: digest(commit_digest)?,
        account: id(account)?,
        device: id(device)?,
        card,
        candidate_head: account_head(candidate_head)?,
        invitation: invitation.map(id).transpose()?,
        invite_proof: invite_proof.map(digest).transpose()?,
        workspace: workspace.map(id).transpose()?,
        request,
        expires_at: uint(expires_at)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn invitation_row(
    invitation: Vec<u8>,
    token_hash: Vec<u8>,
    workspace: Vec<u8>,
    role: i16,
    inviter: Vec<u8>,
    accepted_by: Option<Vec<u8>>,
    expires_at: i64,
    invite_context: Vec<u8>,
) -> Result<InvitationRow> {
    Ok(InvitationRow {
        invitation: id(invitation)?,
        token_hash: digest(token_hash)?,
        workspace: id(workspace)?,
        role: Role::from_code(byte(role)?)
            .ok_or_else(|| Error::internal("a stored invitation names an unknown role"))?,
        inviter: id(inviter)?,
        accepted_by: accepted_by.map(id).transpose()?,
        expires_at: uint(expires_at)?,
        invite_context,
    })
}

#[allow(clippy::too_many_arguments)]
fn offer_row(
    offer_digest: Vec<u8>,
    commit_digest: Vec<u8>,
    candidate_card_digest: Vec<u8>,
    approver: Vec<u8>,
    account: Vec<u8>,
    offer: Vec<u8>,
    nc: Option<Vec<u8>>,
    expires_at: i64,
) -> Result<OfferRow> {
    Ok(OfferRow {
        offer_digest: digest(offer_digest)?,
        commit: digest(commit_digest)?,
        candidate_card_digest: digest(candidate_card_digest)?,
        approver: id(approver)?,
        account: id(account)?,
        offer,
        nc: nc.map(key).transpose()?,
        expires_at: uint(expires_at)?,
    })
}

fn grant_row(
    device_code_hash: Vec<u8>,
    user_code_hash: Vec<u8>,
    account: Option<Vec<u8>>,
    denied: bool,
    poll_interval: i64,
    next_poll: i64,
    expires_at: i64,
) -> Result<DeviceGrantRow> {
    Ok(DeviceGrantRow {
        device_code_hash: digest(device_code_hash)?,
        user_code_hash: digest(user_code_hash)?,
        account: account.map(id).transpose()?,
        denied,
        interval: uint(poll_interval)?,
        next_poll: uint(next_poll)?,
        expires_at: uint(expires_at)?,
    })
}

/// The policy lock's key: the first 8 bytes of the workspace ID (§15).
pub(crate) fn policy_key(workspace: &Id) -> i64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&workspace.0[..8]);
    i64::from_be_bytes(bytes)
}

/// The pruning lock's keys: the first 4 bytes of the workspace ID and of
/// the domain ID (§15).
pub(crate) fn pruning_keys(workspace: &Id, domain: &Id) -> (i32, i32) {
    let prefix = |id: &Id| {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(&id.0[..4]);
        i32::from_be_bytes(bytes)
    };
    (prefix(workspace), prefix(domain))
}

#[allow(clippy::too_many_arguments)]
fn checkpoint_row(
    number: i64,
    checkpoint: Vec<u8>,
    seq: i64,
    covered_seq: i64,
    floor: i64,
    record: Vec<u8>,
    heads: Vec<u8>,
    log_weight: i64,
) -> Result<CheckpointRow> {
    Ok(CheckpointRow {
        number: uint(number)?,
        digest: digest(checkpoint)?,
        seq: uint(seq)?,
        covered_seq: uint(covered_seq)?,
        floor: uint(floor)?,
        record,
        heads,
        log_weight: uint(log_weight)?,
    })
}

impl ReadTxn for PgTxn {
    async fn workspace_usage(&mut self, workspace: Id) -> Result<(u64, u64)> {
        let workspace = &workspace.0[..];
        let row = sqlx::query!(
            r#"SELECT CAST(COALESCE(SUM(usage_bytes), 0) AS BIGINT) AS "usage!: i64",
                      CAST(COALESCE(SUM(history_bytes), 0) AS BIGINT) AS "history!: i64"
             FROM domains WHERE workspace = $1 AND deleted = FALSE"#,
            workspace
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok((uint(row.usage)?, uint(row.history)?))
    }

    async fn domain_histories(&mut self, workspace: Id) -> Result<Vec<DomainHistory>> {
        let workspace = &workspace.0[..];
        let rows = sqlx::query!(
            "SELECT domain, history_bytes, retention_floor, checkpoint_seq FROM domains
             WHERE workspace = $1 AND deleted = FALSE
             ORDER BY history_bytes DESC, domain",
            workspace
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(DomainHistory {
                    domain: id(r.domain)?,
                    history_bytes: uint(r.history_bytes)?,
                    retention_floor: uint(r.retention_floor)?,
                    checkpoint_seq: uint(r.checkpoint_seq)?,
                })
            })
            .collect()
    }

    async fn pending_domains(
        &mut self,
        retained_bytes: u64,
        limit: u32,
    ) -> Result<Vec<PendingDomain>> {
        let retained = at_most(retained_bytes);
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT d.workspace, d.domain, d.deleted FROM domains d
             WHERE d.deleted = TRUE OR d.pruned_floor < d.retention_floor
                OR EXISTS (
                  SELECT 1 FROM checkpoints c
                  WHERE c.workspace = d.workspace AND c.domain = d.domain
                    AND c.covered_seq > d.retention_floor AND c.log_weight <= d.log_weight - $1
                )
             ORDER BY random() LIMIT $2"#,
            retained,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(PendingDomain {
                    workspace: id(r.workspace)?,
                    domain: id(r.domain)?,
                    deleted: r.deleted,
                })
            })
            .collect()
    }

    async fn pressured_workspaces(&mut self, limit: u32) -> Result<Vec<Id>> {
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            "SELECT workspace FROM workspaces WHERE quota_pressure = TRUE ORDER BY random() LIMIT $1",
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.workspace)).collect()
    }

    async fn checkpoint_at_weight(
        &mut self,
        workspace: Id,
        domain: Id,
        log_weight: u64,
    ) -> Result<Option<u64>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let log_weight = at_most(log_weight);
        let covered = sqlx::query_scalar!(
            "SELECT covered_seq FROM checkpoints
             WHERE workspace = $1 AND domain = $2 AND log_weight <= $3
             ORDER BY covered_seq DESC LIMIT 1",
            workspace,
            domain,
            log_weight
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        covered.map(uint).transpose()
    }

    async fn checkpoint_above(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
    ) -> Result<Option<u64>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let covered_seq = at_most(covered_seq);
        let covered = sqlx::query_scalar!(
            "SELECT covered_seq FROM checkpoints
             WHERE workspace = $1 AND domain = $2 AND covered_seq > $3
             ORDER BY covered_seq LIMIT 1",
            workspace,
            domain,
            covered_seq
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        covered.map(uint).transpose()
    }

    async fn servable_floor(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
    ) -> Result<Option<u64>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let covered_seq = at_most(covered_seq);
        let floor = sqlx::query_scalar!(
            "SELECT floor FROM checkpoints
             WHERE workspace = $1 AND domain = $2 AND covered_seq >= $3
             ORDER BY covered_seq LIMIT 1",
            workspace,
            domain,
            covered_seq
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        floor.map(uint).transpose()
    }

    async fn prunable_entries(
        &mut self,
        workspace: Id,
        domain: Id,
        scan: &PruneScan,
        limit: u32,
    ) -> Result<Vec<PrunedEntry>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let after = at_most(scan.after);
        let floor = at_most(scan.floor);
        let delete_floor = at_most(scan.delete_floor);
        let keep_epoch = at_most(scan.keep_epoch);
        let keep_policy = at_most(scan.keep_policy);
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT e.seq AS "seq!", e.record_type AS "record_type!",
                      octet_length(e.record)::BIGINT AS "length!: i64", o.rid AS "rid?"
             FROM entries e
             LEFT JOIN operations o
               ON o.workspace = e.workspace AND o.domain = e.domain AND o.seq = e.seq
             WHERE e.workspace = $1 AND e.domain = $2 AND e.seq > $3 AND e.seq <= $4
               AND (
                 (e.record_type = 5 AND e.seq < $4)
                 OR (e.record_type = 8 AND e.seq < $6)
                 OR (e.record_type = 9 AND e.seq < $7)
                 OR (e.record_type = 1 AND (
                   (o.dropped = TRUE AND e.seq <= $5)
                   OR EXISTS (
                     SELECT 1 FROM covers c
                     WHERE c.workspace = o.workspace AND c.rid = o.rid AND c.device = o.device
                       AND c.counter >= o.counter AND c.by_seq <= $4
                   )
                 ))
               )
             ORDER BY e.seq LIMIT $8"#,
            workspace,
            domain,
            after,
            floor,
            delete_floor,
            keep_epoch,
            keep_policy,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(PrunedEntry {
                    seq: uint(r.seq)?,
                    record_type: byte(r.record_type)?,
                    length: uint(r.length)?,
                    rid: r.rid.map(id).transpose()?,
                })
            })
            .collect()
    }

    async fn listed_blobs(&mut self, workspace: Id, domain: Id, seq: u64) -> Result<Vec<Id>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let seq = int(seq)?;
        let rows = sqlx::query_scalar!(
            "SELECT blob FROM blob_listings WHERE workspace = $1 AND domain = $2 AND seq = $3",
            workspace,
            domain,
            seq
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(id).collect()
    }

    async fn entry_length(&mut self, workspace: Id, domain: Id, seq: u64) -> Result<Option<u64>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let seq = int(seq)?;
        let row = sqlx::query!(
            r#"SELECT octet_length(record)::BIGINT AS "length!: i64" FROM entries
             WHERE workspace = $1 AND domain = $2 AND seq = $3"#,
            workspace,
            domain,
            seq
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| uint(r.length)).transpose()
    }

    async fn epoch_bundle_length(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> Result<Option<u64>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch = int(epoch)?;
        let row = sqlx::query!(
            r#"SELECT octet_length(bundle)::BIGINT AS "length!: i64" FROM epochs
             WHERE workspace = $1 AND domain = $2 AND epoch = $3"#,
            workspace,
            domain,
            epoch
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| uint(r.length)).transpose()
    }

    async fn rid_in_use(&mut self, workspace: Id, rid: Id) -> Result<bool> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        let row = sqlx::query!(
            r#"SELECT EXISTS (
                 SELECT 1 FROM operations WHERE workspace = $1 AND rid = $2
             ) AS "exists!: bool""#,
            workspace,
            rid
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok(row.exists)
    }

    async fn collectable_blobs(&mut self, now: u64, limit: u32) -> Result<Vec<BlobToCollect>> {
        let now = at_most(now);
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            "SELECT workspace, blob, domain FROM blobs
             WHERE marked = TRUE OR delete_after <= $1
             ORDER BY marked DESC LIMIT $2",
            now,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(BlobToCollect {
                    workspace: id(r.workspace)?,
                    domain: id(r.domain)?,
                    blob: id(r.blob)?,
                })
            })
            .collect()
    }

    async fn domain_has_blobs(&mut self, workspace: Id, domain: Id) -> Result<bool> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let row = sqlx::query!(
            r#"SELECT EXISTS (
                 SELECT 1 FROM blobs WHERE workspace = $1 AND domain = $2
             ) AS "exists!: bool""#,
            workspace,
            domain
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok(row.exists)
    }

    async fn expired_shares(&mut self, now: u64, limit: u32) -> Result<Vec<(Digest, Id, Id)>> {
        let now = at_most(now);
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            "SELECT token_hash, workspace, domain FROM shares WHERE expires_at <= $1 LIMIT $2",
            now,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| Ok((digest(r.token_hash)?, id(r.workspace)?, id(r.domain)?)))
            .collect()
    }

    async fn position_below(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> Result<Option<(u64, Digest)>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(counter);
        let counter = &counter[..];
        let row = sqlx::query!(
            r#"SELECT seq AS "seq!", digest AS "digest!" FROM operations
             WHERE workspace = $1 AND domain = $2 AND device = $3 AND counter < $4
             ORDER BY counter DESC LIMIT 1"#,
            workspace,
            domain,
            device,
            counter
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| Ok((uint(r.seq)?, digest(r.digest)?)))
            .transpose()
    }

    async fn restore_generation(&mut self) -> Result<u64> {
        let row = sqlx::query!("SELECT restore_generation FROM server WHERE id = 1")
            .fetch_one(self.c())
            .await
            .map_err(db)?;
        uint(row.restore_generation)
    }

    async fn session(&mut self, token_hash: Digest) -> Result<Option<SessionRow>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "SELECT account, device, recovery, removed FROM sessions WHERE token_hash = $1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(SessionRow {
                session: Session {
                    account: id(r.account)?,
                    device: id(r.device)?,
                    recovery: r.recovery.map(id).transpose()?,
                },
                removed: removed_by(r.removed)?,
            })
        })
        .transpose()
    }

    async fn device_grant(&mut self, code_hash: Digest) -> Result<Option<DeviceGrantRow>> {
        let hash = &code_hash.0[..];
        let row = sqlx::query!(
            "SELECT device_code_hash, user_code_hash, account, denied, poll_interval, next_poll,
                    expires_at
             FROM device_grants WHERE device_code_hash = $1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            grant_row(
                r.device_code_hash,
                r.user_code_hash,
                r.account,
                r.denied,
                r.poll_interval,
                r.next_poll,
                r.expires_at,
            )
        })
        .transpose()
    }

    async fn device_grant_by_user_code(
        &mut self,
        user_code_hash: Digest,
    ) -> Result<Option<DeviceGrantRow>> {
        let hash = &user_code_hash.0[..];
        let row = sqlx::query!(
            "SELECT device_code_hash, user_code_hash, account, denied, poll_interval, next_poll,
                    expires_at
             FROM device_grants WHERE user_code_hash = $1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            grant_row(
                r.device_code_hash,
                r.user_code_hash,
                r.account,
                r.denied,
                r.poll_interval,
                r.next_poll,
                r.expires_at,
            )
        })
        .transpose()
    }

    async fn enrolled_cards(&mut self, account: Id) -> Result<Vec<(Id, Vec<u8>)>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            "SELECT device, card FROM cards WHERE account = $1 ORDER BY device",
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| Ok((id(r.device)?, r.card)))
            .collect()
    }

    async fn session_counts(&mut self, account: Id) -> Result<Vec<(Id, u64)>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            r#"SELECT device, COUNT(*) AS "count!" FROM sessions
               WHERE account = $1 AND removed IS NULL GROUP BY device ORDER BY device"#,
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| Ok((id(r.device)?, uint(r.count)?)))
            .collect()
    }

    async fn approval_requests(&mut self, account: Id) -> Result<Vec<ApprovalRequestRow>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            "SELECT commit_digest, account, device, card, candidate_head, invitation,
                    invite_proof, workspace, request, expires_at
             FROM approval_requests WHERE account = $1 ORDER BY commit_digest",
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                request_row(
                    r.commit_digest,
                    r.account,
                    r.device,
                    r.card,
                    r.candidate_head,
                    r.invitation,
                    r.invite_proof,
                    r.workspace,
                    r.request,
                    r.expires_at,
                )
            })
            .collect()
    }

    async fn approval_request(&mut self, commit: Digest) -> Result<Option<ApprovalRequestRow>> {
        let commit = &commit.0[..];
        let row = sqlx::query!(
            "SELECT commit_digest, account, device, card, candidate_head, invitation,
                    invite_proof, workspace, request, expires_at
             FROM approval_requests WHERE commit_digest = $1",
            commit
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            request_row(
                r.commit_digest,
                r.account,
                r.device,
                r.card,
                r.candidate_head,
                r.invitation,
                r.invite_proof,
                r.workspace,
                r.request,
                r.expires_at,
            )
        })
        .transpose()
    }

    async fn approval_offer(&mut self, offer_digest: Digest) -> Result<Option<OfferRow>> {
        let offer_digest = &offer_digest.0[..];
        let row = sqlx::query!(
            "SELECT offer_digest, commit_digest, candidate_card_digest, approver, account, offer,
                    nc, expires_at
             FROM approval_offers WHERE offer_digest = $1",
            offer_digest
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            offer_row(
                r.offer_digest,
                r.commit_digest,
                r.candidate_card_digest,
                r.approver,
                r.account,
                r.offer,
                r.nc,
                r.expires_at,
            )
        })
        .transpose()
    }

    async fn approval_offers_for(&mut self, card_digest: Digest) -> Result<Vec<OfferRow>> {
        let card_digest = &card_digest.0[..];
        let rows = sqlx::query!(
            "SELECT offer_digest, commit_digest, candidate_card_digest, approver, account, offer,
                    nc, expires_at
             FROM approval_offers WHERE candidate_card_digest = $1 ORDER BY offer_digest",
            card_digest
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                offer_row(
                    r.offer_digest,
                    r.commit_digest,
                    r.candidate_card_digest,
                    r.approver,
                    r.account,
                    r.offer,
                    r.nc,
                    r.expires_at,
                )
            })
            .collect()
    }

    async fn rewrap_requests(&mut self, workspace: Id) -> Result<Vec<RewrapRow>> {
        let workspace = &workspace.0[..];
        let rows = sqlx::query!(
            "SELECT domain, epoch, device, key_seq FROM rewrap_requests
             WHERE workspace = $1 ORDER BY domain, epoch, device",
            workspace
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(RewrapRow {
                    domain: id(r.domain)?,
                    epoch: uint(r.epoch)?,
                    device: id(r.device)?,
                    key_seq: uint(r.key_seq)?,
                })
            })
            .collect()
    }

    async fn rewrap_answers(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
        device: Id,
    ) -> Result<Vec<(Id, u64)>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch = at_most(epoch);
        let device = &device.0[..];
        let rows = sqlx::query!(
            "SELECT wrapper, key_seq FROM rewrap_answers
             WHERE workspace = $1 AND domain = $2 AND epoch = $3 AND device = $4
             ORDER BY key_seq",
            workspace,
            domain,
            epoch,
            device
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| Ok((id(r.wrapper)?, uint(r.key_seq)?)))
            .collect()
    }

    async fn writers_between(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        before: u64,
    ) -> Result<Vec<Id>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let after = at_most(after);
        let before = at_most(before);
        let rows = sqlx::query!(
            "SELECT DISTINCT device FROM operations
             WHERE workspace = $1 AND domain = $2 AND seq > $3 AND seq < $4 ORDER BY device",
            workspace,
            domain,
            after,
            before
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.device)).collect()
    }

    async fn profiles(&mut self, workspace: Id) -> Result<Vec<Vec<u8>>> {
        let workspace = &workspace.0[..];
        let rows = sqlx::query!(
            "SELECT record FROM profiles WHERE workspace = $1 ORDER BY profile_revision",
            workspace
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(|r| r.record).collect())
    }

    async fn current_genesis(&mut self, account: Id) -> Result<Option<Digest>> {
        let account = &account.0[..];
        let row = sqlx::query!("SELECT genesis FROM accounts WHERE account = $1", account)
            .fetch_optional(self.c())
            .await
            .map_err(db)?;
        row.map(|r| digest(r.genesis)).transpose()
    }

    async fn account_links(&mut self, account: Id) -> Result<Vec<LinkRow>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            "SELECT genesis, seqno, link FROM account_links WHERE account = $1 ORDER BY genesis, seqno",
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(LinkRow {
                    genesis: digest(r.genesis)?,
                    seqno: uint(r.seqno)?,
                    link: r.link,
                })
            })
            .collect()
    }

    async fn current_head(&mut self, account: Id) -> Result<Option<AccountHead>> {
        let account_id = account;
        let account = &account.0[..];
        let row = sqlx::query!(
            r#"SELECT a.genesis AS "genesis!", l.seqno AS "seqno!", l.digest AS "digest!"
             FROM accounts a
             JOIN account_links l ON l.account = a.account AND l.genesis = a.genesis
             WHERE a.account = $1 ORDER BY l.seqno DESC LIMIT 1"#,
            account
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(AccountHead {
                account: account_id,
                genesis: digest(r.genesis)?,
                seqno: uint(r.seqno)?,
                head: digest(r.digest)?,
            })
        })
        .transpose()
    }

    async fn blob_rows(
        &mut self,
        after: Option<(Id, Id)>,
        limit: u32,
    ) -> Result<Vec<(Id, BlobRow)>> {
        let (workspace, blob) = after.unwrap_or((Id([0; 16]), Id([0; 16])));
        let started = after.is_some();
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            r#"SELECT workspace AS "workspace!", blob AS "blob!", domain, device, length, digest,
                      upload_offset, complete, delete_after, marked, live_listings
               FROM blobs
               WHERE NOT $1 OR (workspace, blob) > ($2, $3)
               ORDER BY workspace, blob LIMIT $4"#,
            started,
            workspace,
            blob,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    id(r.workspace)?,
                    BlobRow {
                        blob: id(r.blob)?,
                        domain: id(r.domain)?,
                        device: id(r.device)?,
                        length: uint(r.length)?,
                        digest: digest(r.digest)?,
                        offset: uint(r.upload_offset)?,
                        complete: r.complete,
                        delete_after: r.delete_after.map(uint).transpose()?,
                        marked: r.marked,
                        live_listings: uint(r.live_listings)?,
                    },
                ))
            })
            .collect()
    }

    async fn missing_blobs(&mut self, workspace: Id) -> Result<Vec<Id>> {
        let workspace = &workspace.0[..];
        let rows = sqlx::query!(
            r#"SELECT blob AS "blob!" FROM blobs AS b
               WHERE b.workspace = $1 AND NOT b.complete AND NOT b.marked AND EXISTS (
                 SELECT 1 FROM blob_listings AS l
                 WHERE l.workspace = b.workspace AND l.blob = b.blob
               )
               ORDER BY blob"#,
            workspace
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.blob)).collect()
    }

    async fn reset_code(&mut self, account: Id) -> Result<Option<(Digest, u64)>> {
        let account = &account.0[..];
        let row = sqlx::query!(
            "SELECT code_hash, expires_at FROM reset_codes WHERE account = $1",
            account
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| Ok((digest(r.code_hash)?, uint(r.expires_at)?)))
            .transpose()
    }

    async fn live_instances(&mut self, now: u64) -> Result<Vec<Id>> {
        let now = at_most(now);
        let rows = sqlx::query!(
            r#"SELECT instance AS "instance!" FROM instances WHERE lease_until > $1"#,
            now
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.instance)).collect()
    }

    async fn workspace(&mut self, workspace: Id) -> Result<Option<WorkspaceRow>> {
        let workspace = &workspace.0[..];
        let row = sqlx::query!(
            "SELECT owner, creation_digest, creation_response, policy_revision, policy_head,
                    quota_pressure, quota_crossings
             FROM workspaces WHERE workspace = $1 AND closed = 0",
            workspace
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        if let Some(delay) = self.policy_read_delay {
            tokio::time::sleep(delay).await;
        }
        row.map(|r| {
            Ok(WorkspaceRow {
                owner: id(r.owner)?,
                creation_digest: digest(r.creation_digest)?,
                creation_response: r.creation_response,
                policy_revision: uint(r.policy_revision)?,
                policy_head: digest(r.policy_head)?,
                quota_pressure: r.quota_pressure,
                quota_crossings: uint(r.quota_crossings)?,
            })
        })
        .transpose()
    }

    async fn owned_workspaces(&mut self, account: Id) -> Result<u64> {
        let account = &account.0[..];
        let row = sqlx::query!(
            r#"SELECT COUNT(*) AS "count!" FROM workspaces WHERE owner = $1 AND closed = 0"#,
            account
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        uint(row.count)
    }

    async fn memberships(&mut self, account: Id) -> Result<Vec<(Id, u8)>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            "SELECT workspace, role FROM members WHERE account = $1 ORDER BY workspace",
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| Ok((id(r.workspace)?, byte(r.role)?)))
            .collect()
    }

    async fn policy_changes(&mut self, workspace: Id, after: u64) -> Result<Vec<Vec<u8>>> {
        let workspace = &workspace.0[..];
        let after = at_most(after);
        let rows = sqlx::query!(
            "SELECT record FROM policy_changes WHERE workspace = $1 AND revision > $2 ORDER BY revision",
            workspace,
            after
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(|r| r.record).collect())
    }

    async fn latest_profile(&mut self, workspace: Id) -> Result<Option<Vec<u8>>> {
        let workspace = &workspace.0[..];
        let row = sqlx::query!(
            "SELECT record FROM profiles WHERE workspace = $1 ORDER BY profile_revision DESC LIMIT 1",
            workspace
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(row.map(|r| r.record))
    }

    async fn history_floor(&mut self, workspace: Id, account: Id, domain: Id) -> Result<u64> {
        let workspace = &workspace.0[..];
        let account = &account.0[..];
        let domain = &domain.0[..];
        let floor = sqlx::query_scalar!(
            "SELECT floor FROM history_floors
             WHERE workspace = $1 AND account = $2 AND domain = $3",
            workspace,
            account,
            domain
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        floor.map_or(Ok(0), uint)
    }

    async fn domain(&mut self, workspace: Id, domain: Id) -> Result<Option<DomainRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let row = sqlx::query!(
            r#"SELECT d.head_seq, d.epoch, d.epoch_digest, d.epoch_policy_revision,
                    d.retention_floor, d.checkpoint_seq, d.checkpoint_due,
                    d.entries_since_checkpoint, d.weight_since_checkpoint, d.deleted,
                    d.log_weight, d.usage_bytes, d.history_bytes, d.pruned_floor,
                    d.crossing_seen, w.quota_crossings AS "quota_crossings?"
             FROM domains d LEFT JOIN workspaces w ON w.workspace = d.workspace
             WHERE d.workspace = $1 AND d.domain = $2"#,
            workspace,
            domain
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(DomainRow {
                head_seq: uint(r.head_seq)?,
                epoch: uint(r.epoch)?,
                epoch_digest: digest(r.epoch_digest)?,
                epoch_policy_revision: uint(r.epoch_policy_revision)?,
                retention_floor: uint(r.retention_floor)?,
                checkpoint_seq: uint(r.checkpoint_seq)?,
                checkpoint_due: uint(r.checkpoint_due)?,
                entries_since_checkpoint: uint(r.entries_since_checkpoint)?,
                weight_since_checkpoint: uint(r.weight_since_checkpoint)?,
                deleted: r.deleted,
                log_weight: uint(r.log_weight)?,
                usage_bytes: uint(r.usage_bytes)?,
                history_bytes: uint(r.history_bytes)?,
                pruned_floor: uint(r.pruned_floor)?,
                crossing_seen: uint(r.crossing_seen)?,
                quota_crossings: r.quota_crossings.map_or(Ok(0), uint)?,
            })
        })
        .transpose()
    }

    async fn entries_after(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        limit: u32,
    ) -> Result<Vec<RawEntry>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let after = at_most(after);
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            "SELECT seq, record_type, record FROM entries
             WHERE workspace = $1 AND domain = $2 AND seq > $3 ORDER BY seq LIMIT $4",
            workspace,
            domain,
            after,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(RawEntry {
                    seq: uint(r.seq)?,
                    record_type: byte(r.record_type)?,
                    record: r.record,
                })
            })
            .collect()
    }

    async fn position(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> Result<Option<(u64, Digest)>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(counter);
        let counter = &counter[..];
        let row = sqlx::query!(
            "SELECT seq, digest FROM operations
             WHERE workspace = $1 AND domain = $2 AND device = $3 AND counter = $4",
            workspace,
            domain,
            device,
            counter
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| Ok((uint(r.seq)?, digest(r.digest)?)))
            .transpose()
    }

    async fn position_at_or_above(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> Result<Option<RawEntry>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(counter);
        let counter = &counter[..];
        let row = sqlx::query!(
            r#"SELECT e.seq AS "seq!", e.record_type AS "record_type!", e.record AS "record!"
             FROM operations o
             JOIN entries e ON e.workspace = o.workspace AND e.domain = o.domain AND e.seq = o.seq
             WHERE o.workspace = $1 AND o.domain = $2 AND o.device = $3 AND o.counter >= $4
             ORDER BY o.counter LIMIT 1"#,
            workspace,
            domain,
            device,
            counter
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(RawEntry {
                seq: uint(r.seq)?,
                record_type: byte(r.record_type)?,
                record: r.record,
            })
        })
        .transpose()
    }

    async fn chain_head(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
    ) -> Result<Option<ChainHeadRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let device = &device.0[..];
        let row = sqlx::query!(
            "SELECT counter, digest, seq, policy_revision, generation FROM chain_heads
             WHERE workspace = $1 AND domain = $2 AND device = $3",
            workspace,
            domain,
            device
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(ChainHeadRow {
                counter: counter(r.counter)?,
                digest: digest(r.digest)?,
                seq: uint(r.seq)?,
                policy_revision: uint(r.policy_revision)?,
                generation: uint(r.generation)?,
            })
        })
        .transpose()
    }

    async fn rid_domain(&mut self, workspace: Id, rid: Id) -> Result<Option<Id>> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        let row = sqlx::query!(
            "SELECT domain FROM rids WHERE workspace = $1 AND rid = $2",
            workspace,
            rid
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| id(r.domain)).transpose()
    }

    async fn op_exists(
        &mut self,
        workspace: Id,
        rid: Id,
        device: Id,
        counter: u64,
    ) -> Result<bool> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(counter);
        let counter = &counter[..];
        let row = sqlx::query!(
            r#"SELECT EXISTS (
                 SELECT 1 FROM operations
                 WHERE workspace = $1 AND rid = $2 AND device = $3 AND counter = $4
             ) AS "exists!""#,
            workspace,
            rid,
            device,
            counter
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok(row.exists)
    }

    async fn covered(
        &mut self,
        workspace: Id,
        rid: Id,
        device: Id,
        counter: u64,
        at: u64,
    ) -> Result<bool> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(counter);
        let counter = &counter[..];
        let at = at_most(at);
        let row = sqlx::query!(
            r#"SELECT EXISTS (
                 SELECT 1 FROM covers
                 WHERE workspace = $1 AND rid = $2 AND device = $3 AND counter >= $4 AND by_seq <= $5
             ) AS "covered!""#,
            workspace,
            rid,
            device,
            counter,
            at
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok(row.covered)
    }

    async fn live_ops(
        &mut self,
        workspace: Id,
        rid_id: Id,
        at: u64,
    ) -> Result<Vec<(LiveOp, OpClass)>> {
        let workspace = &workspace.0[..];
        let rid = &rid_id.0[..];
        let at = at_most(at);
        let rows = sqlx::query!(
            r#"SELECT o.device AS "device!", o.counter AS "counter!", o.seq AS "seq!",
                      o.digest AS "digest!", o.is_delete AS "is_delete!",
                      o.class AS "class!", o.weight AS "weight!"
             FROM operations o
             WHERE o.workspace = $1 AND o.rid = $2 AND o.seq <= $3 AND NOT o.dropped
               AND NOT EXISTS (
                 SELECT 1 FROM covers c
                 WHERE c.workspace = o.workspace AND c.rid = o.rid AND c.device = o.device
                   AND c.counter >= o.counter AND c.by_seq <= $3
               )
             ORDER BY o.seq"#,
            workspace,
            rid,
            at
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    LiveOp {
                        rid: rid_id,
                        device: id(r.device)?,
                        counter: counter(r.counter)?,
                        seq: uint(r.seq)?,
                        digest: digest(r.digest)?,
                        delete: r.is_delete,
                        covers: Vec::new(),
                        weight: uint(r.weight)?,
                    },
                    class(r.class)?,
                ))
            })
            .collect()
    }

    async fn deletes_between(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        through: u64,
    ) -> Result<Vec<DeleteRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let after = at_most(after);
        let through = at_most(through);
        let rows = sqlx::query!(
            "SELECT rid, device, counter, seq, digest FROM operations
             WHERE workspace = $1 AND domain = $2 AND is_delete AND NOT dropped
               AND seq > $3 AND seq <= $4
             ORDER BY seq",
            workspace,
            domain,
            after,
            through
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(DeleteRow {
                    rid: id(r.rid)?,
                    op: Ref {
                        device: id(r.device)?,
                        counter: counter(r.counter)?,
                        seq: uint(r.seq)?,
                        op_digest: digest(r.digest)?,
                    },
                })
            })
            .collect()
    }

    async fn checkpoint_exists(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
        checkpoint: Digest,
    ) -> Result<bool> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let covered_seq = at_most(covered_seq);
        let checkpoint = &checkpoint.0[..];
        let row = sqlx::query!(
            r#"SELECT EXISTS (
                 SELECT 1 FROM checkpoints
                 WHERE workspace = $1 AND domain = $2 AND covered_seq = $3 AND digest = $4
             ) AS "exists!""#,
            workspace,
            domain,
            covered_seq,
            checkpoint
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok(row.exists)
    }

    async fn latest_checkpoint(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> Result<Option<CheckpointRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let row = sqlx::query!(
            "SELECT number, digest, seq, covered_seq, floor, record, heads, log_weight
             FROM checkpoints
             WHERE workspace = $1 AND domain = $2 ORDER BY number DESC LIMIT 1",
            workspace,
            domain
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            checkpoint_row(
                r.number,
                r.digest,
                r.seq,
                r.covered_seq,
                r.floor,
                r.record,
                r.heads,
                r.log_weight,
            )
        })
        .transpose()
    }

    async fn checkpoint_by_number(
        &mut self,
        workspace: Id,
        domain: Id,
        number: u64,
    ) -> Result<Option<CheckpointRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let number = at_most(number);
        let row = sqlx::query!(
            "SELECT number, digest, seq, covered_seq, floor, record, heads, log_weight
             FROM checkpoints
             WHERE workspace = $1 AND domain = $2 AND number = $3",
            workspace,
            domain,
            number
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            checkpoint_row(
                r.number,
                r.digest,
                r.seq,
                r.covered_seq,
                r.floor,
                r.record,
                r.heads,
                r.log_weight,
            )
        })
        .transpose()
    }

    async fn checkpoint_by_digest(
        &mut self,
        workspace: Id,
        domain: Id,
        checkpoint: Digest,
    ) -> Result<Option<CheckpointRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let checkpoint = &checkpoint.0[..];
        let row = sqlx::query!(
            "SELECT number, digest, seq, covered_seq, floor, record, heads, log_weight
             FROM checkpoints
             WHERE workspace = $1 AND domain = $2 AND digest = $3",
            workspace,
            domain,
            checkpoint
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            checkpoint_row(
                r.number,
                r.digest,
                r.seq,
                r.covered_seq,
                r.floor,
                r.record,
                r.heads,
                r.log_weight,
            )
        })
        .transpose()
    }

    async fn checkpoint_at(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
    ) -> Result<Option<CheckpointRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let covered_seq = at_most(covered_seq);
        let row = sqlx::query!(
            "SELECT number, digest, seq, covered_seq, floor, record, heads, log_weight
             FROM checkpoints
             WHERE workspace = $1 AND domain = $2 AND covered_seq = $3",
            workspace,
            domain,
            covered_seq
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            checkpoint_row(
                r.number,
                r.digest,
                r.seq,
                r.covered_seq,
                r.floor,
                r.record,
                r.heads,
                r.log_weight,
            )
        })
        .transpose()
    }

    async fn checkpoint_buckets(
        &mut self,
        workspace: Id,
        domain: Id,
        number: u64,
    ) -> Result<Option<Vec<u8>>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let number = at_most(number);
        let row = sqlx::query!(
            "SELECT buckets FROM checkpoints WHERE workspace = $1 AND domain = $2 AND number = $3",
            workspace,
            domain,
            number
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(row.and_then(|r| r.buckets))
    }

    async fn latest_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        record_type: u8,
        at: u64,
    ) -> Result<Option<RawEntry>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let record_type = i16::from(record_type);
        let at = at_most(at);
        let row = sqlx::query!(
            "SELECT seq, record_type, record FROM entries
             WHERE workspace = $1 AND domain = $2 AND record_type = $3 AND seq <= $4
             ORDER BY seq DESC LIMIT 1",
            workspace,
            domain,
            record_type,
            at
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(RawEntry {
                seq: uint(r.seq)?,
                record_type: byte(r.record_type)?,
                record: r.record,
            })
        })
        .transpose()
    }

    async fn ops_between(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        through: u64,
    ) -> Result<Vec<(Id, Ref)>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let after = at_most(after);
        let through = at_most(through);
        let rows = sqlx::query!(
            "SELECT rid, device, counter, seq, digest FROM operations
             WHERE workspace = $1 AND domain = $2 AND seq > $3 AND seq <= $4
             ORDER BY seq",
            workspace,
            domain,
            after,
            through
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    id(r.rid)?,
                    Ref {
                        device: id(r.device)?,
                        counter: counter(r.counter)?,
                        seq: uint(r.seq)?,
                        op_digest: digest(r.digest)?,
                    },
                ))
            })
            .collect()
    }

    async fn domain_rids(
        &mut self,
        workspace: Id,
        domain: Id,
        from: Id,
        through: Id,
        limit: u32,
    ) -> Result<Vec<Id>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let from = &from.0[..];
        let through = &through.0[..];
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            "SELECT rid FROM rids
             WHERE workspace = $1 AND rid >= $2 AND rid <= $3 AND domain = $4
             ORDER BY rid LIMIT $5",
            workspace,
            from,
            through,
            domain,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.rid)).collect()
    }

    async fn chain_head_devices(&mut self, workspace: Id, domain: Id) -> Result<Vec<Id>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let rows = sqlx::query!(
            "SELECT device FROM chain_heads WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.device)).collect()
    }

    async fn epoch(&mut self, workspace: Id, domain: Id, epoch: u64) -> Result<Option<EpochRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch = at_most(epoch);
        let row = sqlx::query!(
            "SELECT epoch, seq, digest, previous, record FROM epochs
             WHERE workspace = $1 AND domain = $2 AND epoch = $3",
            workspace,
            domain,
            epoch
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(EpochRow {
                epoch: uint(r.epoch)?,
                seq: uint(r.seq)?,
                digest: digest(r.digest)?,
                previous: digest(r.previous)?,
                record: r.record,
            })
        })
        .transpose()
    }

    async fn epoch_by_digest(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch_digest: Digest,
    ) -> Result<Option<EpochRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch_digest = &epoch_digest.0[..];
        let row = sqlx::query!(
            "SELECT epoch, seq, digest, previous, record FROM epochs
             WHERE workspace = $1 AND domain = $2 AND digest = $3",
            workspace,
            domain,
            epoch_digest
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(EpochRow {
                epoch: uint(r.epoch)?,
                seq: uint(r.seq)?,
                digest: digest(r.digest)?,
                previous: digest(r.previous)?,
                record: r.record,
            })
        })
        .transpose()
    }

    async fn key_entries(&mut self, workspace: Id, after: u64, limit: u32) -> Result<Vec<KeyRow>> {
        let workspace = &workspace.0[..];
        let after = at_most(after);
        let limit = i64::from(limit);
        let rows = sqlx::query!(
            "SELECT key_seq, record_type, domain, recipient, record FROM key_entries
             WHERE workspace = $1 AND key_seq > $2 ORDER BY key_seq LIMIT $3",
            workspace,
            after,
            limit
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(KeyRow {
                    key_seq: uint(r.key_seq)?,
                    record_type: byte(r.record_type)?,
                    domain: id(r.domain)?,
                    recipient: r.recipient.map(id).transpose()?,
                    record: r.record,
                })
            })
            .collect()
    }

    async fn key_head(&mut self, workspace: Id) -> Result<u64> {
        let workspace = &workspace.0[..];
        let row = sqlx::query!(
            "SELECT key_seq FROM key_sequences WHERE workspace = $1",
            workspace
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map_or(Ok(0), |r| uint(r.key_seq))
    }

    async fn enrolled_card(&mut self, device: Id) -> Result<Option<(Id, Vec<u8>)>> {
        let device = &device.0[..];
        let row = sqlx::query!("SELECT account, card FROM cards WHERE device = $1", device)
            .fetch_optional(self.c())
            .await
            .map_err(db)?;
        row.map(|r| Ok((id(r.account)?, r.card))).transpose()
    }

    async fn workspaces_recording(&mut self, account: Id) -> Result<Vec<Id>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            "SELECT workspace FROM recorded_accounts WHERE account = $1 ORDER BY workspace",
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| id(r.workspace)).collect()
    }

    async fn account_email(&mut self, account: Id) -> Result<Option<String>> {
        let account = &account.0[..];
        let row = sqlx::query!(
            "SELECT email FROM account_emails WHERE account = $1",
            account
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(row.map(|r| r.email))
    }

    async fn invitation_requests(&mut self, account: Id) -> Result<Vec<(Id, u64)>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            r#"SELECT workspace AS "workspace!", expires_at FROM approval_requests
               WHERE account = $1 AND invitation IS NOT NULL AND workspace IS NOT NULL
               ORDER BY commit_digest"#,
            account
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| Ok((id(r.workspace)?, uint(r.expires_at)?)))
            .collect()
    }

    async fn passkeys(&mut self, account: Id) -> Result<Vec<PasskeyRow>> {
        let bytes = &account.0[..];
        let rows = sqlx::query!(
            "SELECT credential_id, passkey FROM passkeys WHERE account = $1 ORDER BY credential_id",
            bytes
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| PasskeyRow {
                credential_id: r.credential_id,
                account,
                passkey: r.passkey,
            })
            .collect())
    }

    async fn passkey(&mut self, credential_id: &[u8]) -> Result<Option<PasskeyRow>> {
        let row = sqlx::query!(
            "SELECT credential_id, account, passkey FROM passkeys WHERE credential_id = $1",
            credential_id
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(PasskeyRow {
                credential_id: r.credential_id,
                account: id(r.account)?,
                passkey: r.passkey,
            })
        })
        .transpose()
    }

    async fn web_session(&mut self, token_hash: Digest) -> Result<Option<Id>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "SELECT account FROM web_sessions WHERE token_hash = $1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| id(r.account)).transpose()
    }

    async fn blob(&mut self, workspace: Id, blob: Id) -> Result<Option<BlobRow>> {
        let workspace = &workspace.0[..];
        let blob_id = &blob.0[..];
        let row = sqlx::query!(
            "SELECT domain, device, length, digest, upload_offset, complete, delete_after, marked,
                    live_listings
             FROM blobs WHERE workspace = $1 AND blob = $2",
            workspace,
            blob_id
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(BlobRow {
                blob,
                domain: id(r.domain)?,
                device: id(r.device)?,
                length: uint(r.length)?,
                digest: digest(r.digest)?,
                offset: uint(r.upload_offset)?,
                complete: r.complete,
                delete_after: r.delete_after.map(uint).transpose()?,
                marked: r.marked,
                live_listings: uint(r.live_listings)?,
            })
        })
        .transpose()
    }

    async fn blob_segments(&mut self, workspace: Id, blob: Id) -> Result<Vec<Digest>> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let rows = sqlx::query!(
            "SELECT hash FROM blob_segments WHERE workspace = $1 AND blob = $2 ORDER BY idx",
            workspace,
            blob
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(|r| digest(r.hash)).collect()
    }

    async fn blob_listed(&mut self, workspace: Id, blob: Id) -> Result<bool> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let row = sqlx::query!(
            "SELECT seq FROM blob_listings WHERE workspace = $1 AND blob = $2 LIMIT 1",
            workspace,
            blob
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(row.is_some())
    }

    async fn blob_listing_seqs(&mut self, workspace: Id, blob: Id) -> Result<Vec<u64>> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let rows = sqlx::query_scalar!(
            "SELECT seq FROM blob_listings WHERE workspace = $1 AND blob = $2",
            workspace,
            blob
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter().map(uint).collect()
    }

    async fn invitation(&mut self, invitation: Id) -> Result<Option<InvitationRow>> {
        let invitation = &invitation.0[..];
        let row = sqlx::query!(
            "SELECT invitation, token_hash, workspace, role, inviter, accepted_by, expires_at, invite_context FROM invitations WHERE invitation = $1",
            invitation
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            invitation_row(
                r.invitation,
                r.token_hash,
                r.workspace,
                r.role,
                r.inviter,
                r.accepted_by,
                r.expires_at,
                r.invite_context,
            )
        })
        .transpose()
    }

    async fn invitation_ended(&mut self, invitation: Id) -> Result<bool> {
        let invitation = &invitation.0[..];
        let row = sqlx::query!(
            "SELECT expires_at FROM ended_invitations WHERE invitation = $1",
            invitation
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(row.is_some())
    }

    async fn invitation_by_token(&mut self, token_hash: Digest) -> Result<Option<InvitationRow>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "SELECT invitation, token_hash, workspace, role, inviter, accepted_by, expires_at, invite_context FROM invitations WHERE token_hash = $1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            invitation_row(
                r.invitation,
                r.token_hash,
                r.workspace,
                r.role,
                r.inviter,
                r.accepted_by,
                r.expires_at,
                r.invite_context,
            )
        })
        .transpose()
    }

    async fn invitations(&mut self, workspace: Id, now: u64) -> Result<Vec<InvitationRow>> {
        let workspace = &workspace.0[..];
        let now = at_most(now);
        let rows = sqlx::query!(
            "SELECT invitation, token_hash, workspace, role, inviter, accepted_by, expires_at, invite_context FROM invitations WHERE workspace = $1 AND expires_at > $2 ORDER BY invitation",
            workspace,
            now
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                invitation_row(
                    r.invitation,
                    r.token_hash,
                    r.workspace,
                    r.role,
                    r.inviter,
                    r.accepted_by,
                    r.expires_at,
                    r.invite_context,
                )
            })
            .collect()
    }

    async fn workspace_approval_requests(
        &mut self,
        workspace: Id,
    ) -> Result<Vec<ApprovalRequestRow>> {
        let workspace = &workspace.0[..];
        let rows = sqlx::query!(
            "SELECT commit_digest, account, device, card, candidate_head, invitation,
                    invite_proof, workspace, request, expires_at
             FROM approval_requests WHERE workspace = $1 ORDER BY commit_digest",
            workspace
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                request_row(
                    r.commit_digest,
                    r.account,
                    r.device,
                    r.card,
                    r.candidate_head,
                    r.invitation,
                    r.invite_proof,
                    r.workspace,
                    r.request,
                    r.expires_at,
                )
            })
            .collect()
    }

    async fn share(&mut self, token_hash: Digest) -> Result<Option<ShareRow>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "SELECT workspace, domain, device, expires_at, share FROM shares WHERE token_hash = $1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(ShareRow {
                token_hash,
                workspace: id(r.workspace)?,
                domain: id(r.domain)?,
                device: id(r.device)?,
                expires_at: uint(r.expires_at)?,
                share: r.share,
            })
        })
        .transpose()
    }

    async fn shares(&mut self, workspace: Id, now: u64) -> Result<Vec<ShareRow>> {
        let workspace_bytes = &workspace.0[..];
        let now = at_most(now);
        let rows = sqlx::query!(
            "SELECT token_hash, domain, device, expires_at FROM shares WHERE workspace = $1 AND expires_at > $2 ORDER BY token_hash",
            workspace_bytes,
            now
        )
        .fetch_all(self.c())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(ShareRow {
                    token_hash: digest(r.token_hash)?,
                    workspace,
                    domain: id(r.domain)?,
                    device: id(r.device)?,
                    expires_at: uint(r.expires_at)?,
                    share: Vec::new(),
                })
            })
            .collect()
    }
}

fn kind_code(kind: CeremonyKind) -> i16 {
    i16::from(kind.code())
}

fn ceremony_kind(code: i16) -> Result<CeremonyKind> {
    match code {
        1 => Ok(CeremonyKind::SignUp),
        2 => Ok(CeremonyKind::AddPasskey),
        3 => Ok(CeremonyKind::SignIn),
        _ => Err(Error::internal("unknown ceremony kind")),
    }
}

impl WriteTxn for PgTxn {
    async fn add_usage(
        &mut self,
        workspace: Id,
        domain: Id,
        usage: i64,
        history: i64,
    ) -> Result<()> {
        if usage == 0 && history == 0 {
            return Ok(());
        }
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        sqlx::query!(
            "UPDATE domains
             SET usage_bytes = GREATEST(usage_bytes + $3, 0), history_bytes = GREATEST(history_bytes + $4, 0)
             WHERE workspace = $1 AND domain = $2",
            workspace,
            domain,
            usage,
            history
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_retention_floor(&mut self, workspace: Id, domain: Id, floor: u64) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let floor = int(floor)?;
        sqlx::query!(
            "UPDATE domains SET retention_floor = $3 WHERE workspace = $1 AND domain = $2",
            workspace,
            domain,
            floor
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_pruned_floor(&mut self, workspace: Id, domain: Id, floor: u64) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let floor = int(floor)?;
        sqlx::query!(
            "UPDATE domains SET pruned_floor = $3 WHERE workspace = $1 AND domain = $2",
            workspace,
            domain,
            floor
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn mark_domain_deleted(&mut self, workspace: Id, domain: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        sqlx::query!(
            "UPDATE domains SET deleted = TRUE WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_log_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        seq: u64,
        record_type: u8,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let seq = int(seq)?;
        if record_type == 1 {
            sqlx::query!(
                "DELETE FROM covers WHERE workspace = $1 AND domain = $2 AND by_seq = $3",
                workspace,
                domain,
                seq
            )
            .execute(self.c())
            .await
            .map_err(db)?;
            sqlx::query!(
                "DELETE FROM operations WHERE workspace = $1 AND domain = $2 AND seq = $3",
                workspace,
                domain,
                seq
            )
            .execute(self.c())
            .await
            .map_err(db)?;
        }
        sqlx::query!(
            "DELETE FROM entries WHERE workspace = $1 AND domain = $2 AND seq = $3",
            workspace,
            domain,
            seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_rid(&mut self, workspace: Id, rid: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        sqlx::query!(
            "DELETE FROM rids WHERE workspace = $1 AND rid = $2",
            workspace,
            rid
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_blob_listing(
        &mut self,
        workspace: Id,
        blob: Id,
        domain: Id,
        seq: u64,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let domain = &domain.0[..];
        let seq = int(seq)?;
        sqlx::query!(
            "DELETE FROM blob_listings
             WHERE workspace = $1 AND blob = $2 AND domain = $3 AND seq = $4",
            workspace,
            blob,
            domain,
            seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_blob_delete_after(
        &mut self,
        workspace: Id,
        blob: Id,
        delete_after: Option<u64>,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let delete_after = delete_after.map(at_most);
        sqlx::query!(
            "UPDATE blobs SET delete_after = $3 WHERE workspace = $1 AND blob = $2",
            workspace,
            blob,
            delete_after
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn add_blob_live(&mut self, workspace: Id, blob: Id, delta: i64) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        sqlx::query!(
            "UPDATE blobs SET live_listings = live_listings + $3
             WHERE workspace = $1 AND blob = $2",
            workspace,
            blob,
            delta
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn mark_blob(&mut self, workspace: Id, blob: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        sqlx::query!(
            "UPDATE blobs SET marked = TRUE WHERE workspace = $1 AND blob = $2",
            workspace,
            blob
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn clear_checkpoint_data(
        &mut self,
        workspace: Id,
        domain: Id,
        floor: u64,
        empty_heads: &[u8],
        limit: u32,
    ) -> Result<u64> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let floor = at_most(floor);
        let limit = i64::from(limit);
        let cleared = sqlx::query!(
            "UPDATE checkpoints SET buckets = NULL, heads = $4
             WHERE workspace = $1 AND domain = $2 AND number IN (
               SELECT number FROM checkpoints
               WHERE workspace = $1 AND domain = $2 AND covered_seq < $3
                 AND buckets IS NOT NULL
               ORDER BY number LIMIT $5
             )",
            workspace,
            domain,
            floor,
            empty_heads,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(cleared.rows_affected())
    }

    async fn set_quota_pressure(&mut self, workspace: Id, above: bool) -> Result<bool> {
        let workspace = &workspace.0[..];
        let done = if above {
            sqlx::query!(
                "UPDATE workspaces
                 SET quota_pressure = TRUE, quota_crossings = quota_crossings + 1
                 WHERE workspace = $1 AND quota_pressure = FALSE",
                workspace
            )
            .execute(self.c())
            .await
        } else {
            sqlx::query!(
                "UPDATE workspaces SET quota_pressure = FALSE
                 WHERE workspace = $1 AND quota_pressure = TRUE",
                workspace
            )
            .execute(self.c())
            .await
        }
        .map_err(db)?;
        Ok(done.rows_affected() > 0)
    }

    async fn reclaim_rows(&mut self, workspace: Id, domain: Id, limit: u32) -> Result<u64> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let limit = i64::from(limit);
        let mut deleted = 0;
        deleted += sqlx::query!(
            "DELETE FROM entries WHERE workspace = $1 AND domain = $2 AND seq IN (
               SELECT seq FROM entries WHERE workspace = $1 AND domain = $2 ORDER BY seq LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM operations WHERE workspace = $1 AND domain = $2 AND seq IN (
               SELECT seq FROM operations WHERE workspace = $1 AND domain = $2
               ORDER BY seq LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM covers WHERE workspace = $1 AND domain = $2 AND by_seq IN (
               SELECT by_seq FROM covers WHERE workspace = $1 AND domain = $2
               ORDER BY by_seq LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM rids WHERE workspace = $1 AND rid IN (
               SELECT rid FROM rids WHERE workspace = $1 AND domain = $2 LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM checkpoints WHERE workspace = $1 AND domain = $2 AND number IN (
               SELECT number FROM checkpoints WHERE workspace = $1 AND domain = $2
               ORDER BY number LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM epochs WHERE workspace = $1 AND domain = $2 AND epoch IN (
               SELECT epoch FROM epochs WHERE workspace = $1 AND domain = $2
               ORDER BY epoch LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM key_entries WHERE workspace = $1 AND key_seq IN (
               SELECT key_seq FROM key_entries WHERE workspace = $1 AND domain = $2 LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM blob_listings WHERE workspace = $1 AND domain = $2 AND seq IN (
               SELECT seq FROM blob_listings WHERE workspace = $1 AND domain = $2
               ORDER BY seq LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM shares WHERE token_hash IN (
               SELECT token_hash FROM shares WHERE workspace = $1 AND domain = $2 LIMIT $3
             )",
            workspace,
            domain,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        // A deleted domain's small tables hold one row per device or
        // account of its readers, at most a few thousand.
        deleted += sqlx::query!(
            "DELETE FROM chain_heads WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM history_floors WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM rewrap_answers WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        deleted += sqlx::query!(
            "DELETE FROM rewrap_requests WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected();
        Ok(deleted)
    }

    async fn expire_domain_blobs(
        &mut self,
        workspace: Id,
        domain: Id,
        now: u64,
        limit: u32,
    ) -> Result<u64> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let now = at_most(now);
        let limit = i64::from(limit);
        let changed = sqlx::query!(
            "UPDATE blobs SET delete_after = $3
             WHERE workspace = $1 AND blob IN (
               SELECT blob FROM blobs
               WHERE workspace = $1 AND domain = $2 AND marked = FALSE
                 AND (delete_after IS NULL OR delete_after > $3)
               LIMIT $4
             )",
            workspace,
            domain,
            now,
            limit
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(changed.rows_affected())
    }

    async fn delete_domain_row(&mut self, workspace: Id, domain: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        sqlx::query!(
            "DELETE FROM domains WHERE workspace = $1 AND domain = $2",
            workspace,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn lock(&mut self, plan: &LockPlan) -> Result<()> {
        // The policy lock: a transaction-level advisory lock in the one-key
        // form, so concurrent pushes lock no row and write none.
        if let Some(workspace) = &plan.workspace {
            let key = policy_key(workspace);
            if plan.policy_exclusive {
                sqlx::query!(
                    r#"SELECT 1 AS "locked!" FROM pg_advisory_xact_lock($1)"#,
                    key
                )
                .fetch_one(self.c())
                .await
                .map_err(db)?;
            } else {
                sqlx::query!(
                    r#"SELECT 1 AS "locked!" FROM pg_advisory_xact_lock_shared($1)"#,
                    key
                )
                .fetch_one(self.c())
                .await
                .map_err(db)?;
            }
            let workspace = &workspace.0[..];
            // The key-sequence row, which a creation makes with its first
            // key stream row.
            if plan.key_sequence {
                sqlx::query!(
                    "SELECT key_seq FROM key_sequences WHERE workspace = $1 FOR UPDATE",
                    workspace
                )
                .fetch_optional(self.c())
                .await
                .map_err(db)?;
            }
            // The domains' head rows, one at a time in domain-ID order.
            let mut domains = plan.domains.clone();
            domains.sort();
            domains.dedup();
            for domain in &domains {
                let domain = &domain.0[..];
                sqlx::query!(
                    "SELECT head_seq FROM domains WHERE workspace = $1 AND domain = $2 FOR UPDATE",
                    workspace,
                    domain
                )
                .fetch_optional(self.c())
                .await
                .map_err(db)?;
            }
            // The blobs' rows, in blob-ID order after the head rows. A blob
            // a creation inserts has no row yet; its primary key then
            // serializes two creations under one ID.
            let mut blobs = plan.blobs.clone();
            blobs.sort();
            blobs.dedup();
            for blob in &blobs {
                let blob = &blob.0[..];
                sqlx::query!(
                    "SELECT complete FROM blobs WHERE workspace = $1 AND blob = $2 FOR UPDATE",
                    workspace,
                    blob
                )
                .fetch_optional(self.c())
                .await
                .map_err(db)?;
            }
        }
        Ok(())
    }

    async fn lock_account(&mut self, account: Id) -> Result<()> {
        // A new account has no row yet; its genesis insert takes the
        // primary key, so a second genesis for it waits and then fails.
        let account = &account.0[..];
        sqlx::query!(
            "SELECT genesis FROM accounts WHERE account = $1 FOR UPDATE",
            account
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn lock_pruning(&mut self, workspace: Id, domain: Id) -> Result<()> {
        let (workspace, domain) = pruning_keys(&workspace, &domain);
        sqlx::query!(
            r#"SELECT 1 AS "locked!" FROM pg_advisory_xact_lock($1::int4, $2::int4)"#,
            workspace,
            domain
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_session(&mut self, token_hash: Digest, session: Session) -> Result<()> {
        let hash = &token_hash.0[..];
        let account = &session.account.0[..];
        let device = &session.device.0[..];
        let recovery = session.recovery.as_ref().map(|r| r.0.to_vec());
        sqlx::query!(
            "INSERT INTO sessions (token_hash, account, device, recovery) VALUES ($1, $2, $3, $4)",
            hash,
            account,
            device,
            recovery
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_account_link(
        &mut self,
        account: Id,
        link: &LinkRow,
        link_digest: Digest,
    ) -> Result<()> {
        let account = &account.0[..];
        let genesis = &link.genesis.0[..];
        let seqno = int(link.seqno)?;
        let link_digest = &link_digest.0[..];
        let bytes = &link.link[..];
        sqlx::query!(
            "INSERT INTO account_links (account, genesis, seqno, digest, link) VALUES ($1, $2, $3, $4, $5)",
            account,
            genesis,
            seqno,
            link_digest,
            bytes
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn start_chain(&mut self, account: Id, genesis: Digest) -> Result<bool> {
        // A racing genesis's uncommitted row holds the key: this insert
        // waits for it, then inserts nothing once it commits.
        let account = &account.0[..];
        let genesis = &genesis.0[..];
        let done = sqlx::query!(
            "INSERT INTO accounts (account, genesis) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            account,
            genesis
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(done.rows_affected() == 1)
    }

    async fn set_current_genesis(&mut self, account: Id, genesis: Digest) -> Result<()> {
        let account = &account.0[..];
        let genesis = &genesis.0[..];
        sqlx::query!(
            "INSERT INTO accounts (account, genesis) VALUES ($1, $2)
             ON CONFLICT (account) DO UPDATE SET genesis = excluded.genesis",
            account,
            genesis
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_workspace(&mut self, workspace: Id, row: &WorkspaceRow) -> Result<()> {
        let workspace = &workspace.0[..];
        let owner = &row.owner.0[..];
        let creation_digest = &row.creation_digest.0[..];
        let creation_response = &row.creation_response[..];
        let policy_revision = int(row.policy_revision)?;
        let policy_head = &row.policy_head.0[..];
        sqlx::query!(
            "INSERT INTO workspaces
               (workspace, owner, creation_digest, creation_response, policy_revision, policy_head)
             VALUES ($1, $2, $3, $4, $5, $6)",
            workspace,
            owner,
            creation_digest,
            creation_response,
            policy_revision,
            policy_head
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn upsert_member(&mut self, workspace: Id, account: Id, role: u8) -> Result<()> {
        let workspace = &workspace.0[..];
        let account = &account.0[..];
        let role = i16::from(role);
        sqlx::query!(
            "INSERT INTO members (workspace, account, role) VALUES ($1, $2, $3)
             ON CONFLICT (workspace, account) DO UPDATE SET role = excluded.role",
            workspace,
            account,
            role
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_history_floor(
        &mut self,
        workspace: Id,
        account: Id,
        domain: Id,
        floor: u64,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let account = &account.0[..];
        let domain = &domain.0[..];
        let floor = int(floor)?;
        sqlx::query!(
            "INSERT INTO history_floors (workspace, account, domain, floor)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (workspace, account, domain) DO UPDATE SET floor = excluded.floor",
            workspace,
            account,
            domain,
            floor
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_member(&mut self, workspace: Id, account: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let account = &account.0[..];
        sqlx::query!(
            "DELETE FROM members WHERE workspace = $1 AND account = $2",
            workspace,
            account
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_policy_change(
        &mut self,
        workspace: Id,
        revision: u64,
        change_digest: Digest,
        record: &[u8],
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let revision = int(revision)?;
        let change_digest = &change_digest.0[..];
        sqlx::query!(
            "INSERT INTO policy_changes (workspace, revision, digest, record) VALUES ($1, $2, $3, $4)",
            workspace,
            revision,
            change_digest,
            record
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_policy_head(&mut self, workspace: Id, revision: u64, head: Digest) -> Result<()> {
        let workspace = &workspace.0[..];
        let revision = int(revision)?;
        let head = &head.0[..];
        sqlx::query!(
            "UPDATE workspaces SET policy_revision = $2, policy_head = $3 WHERE workspace = $1",
            workspace,
            revision,
            head
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_domain(&mut self, workspace: Id, domain: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let zero = &Digest::ZERO.0[..];
        sqlx::query!(
            "INSERT INTO domains (workspace, domain, head_seq, epoch, epoch_digest, epoch_policy_revision)
             VALUES ($1, $2, 0, 0, $3, 0)",
            workspace,
            domain,
            zero
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn append_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        record_type: u8,
        record: &[u8],
    ) -> Result<u64> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        // The update takes the head row, so the sequence is assigned under
        // it (§15).
        let row = sqlx::query!(
            "UPDATE domains SET head_seq = head_seq + 1 WHERE workspace = $1 AND domain = $2
             RETURNING head_seq",
            workspace,
            domain
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        let seq = row.head_seq;
        let record_type = i16::from(record_type);
        sqlx::query!(
            "INSERT INTO entries (workspace, domain, seq, record_type, record) VALUES ($1, $2, $3, $4, $5)",
            workspace,
            domain,
            seq,
            record_type,
            record
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        uint(seq)
    }

    async fn insert_checkpoint(
        &mut self,
        workspace: Id,
        domain: Id,
        row: &CheckpointRow,
        buckets: &[u8],
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let number = int(row.number)?;
        let checkpoint = &row.digest.0[..];
        let seq = int(row.seq)?;
        let covered_seq = int(row.covered_seq)?;
        let floor = int(row.floor)?;
        let record = &row.record[..];
        let heads = &row.heads[..];
        let log_weight = at_most(row.log_weight);
        sqlx::query!(
            "INSERT INTO checkpoints
               (workspace, domain, number, digest, seq, covered_seq, floor, record, heads, buckets,
                log_weight)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            workspace,
            domain,
            number,
            checkpoint,
            seq,
            covered_seq,
            floor,
            record,
            heads,
            buckets,
            log_weight
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "UPDATE domains SET checkpoint_seq = $3 WHERE workspace = $1 AND domain = $2",
            workspace,
            domain,
            covered_seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_checkpoint_progress(
        &mut self,
        workspace: Id,
        domain: Id,
        progress: &CheckpointProgress,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let entries = at_most(progress.entries);
        let weight = at_most(progress.weight);
        let due = int(progress.due)?;
        let log_weight = at_most(progress.log_weight);
        let crossing_seen = at_most(progress.crossing_seen);
        sqlx::query!(
            "UPDATE domains
             SET entries_since_checkpoint = $3, weight_since_checkpoint = $4, checkpoint_due = $5,
                 log_weight = $6, crossing_seen = $7
             WHERE workspace = $1 AND domain = $2",
            workspace,
            domain,
            entries,
            weight,
            due,
            log_weight,
            crossing_seen
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_operation(
        &mut self,
        workspace: Id,
        domain: Id,
        op: &LiveOp,
        op_class: OpClass,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let seq = int(op.seq)?;
        let device = &op.device.0[..];
        let counter = counter_bytes(op.counter);
        let counter = &counter[..];
        let rid = &op.rid.0[..];
        let op_digest = &op.digest.0[..];
        let class = class_code(op_class);
        let weight = int(op.weight)?;
        sqlx::query!(
            "INSERT INTO operations
               (workspace, domain, seq, device, counter, rid, digest, is_delete, class, weight)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            workspace,
            domain,
            seq,
            device,
            counter,
            rid,
            op_digest,
            op.delete,
            class,
            weight
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        for cover in &op.covers {
            let cover_device = &cover.device.0[..];
            let cover_counter = counter_bytes(cover.counter);
            let cover_counter = &cover_counter[..];
            sqlx::query!(
                "INSERT INTO covers (workspace, rid, device, counter, by_seq, domain)
                 VALUES ($1, $2, $3, $4, $5, $6)",
                workspace,
                rid,
                cover_device,
                cover_counter,
                seq,
                domain
            )
            .execute(self.c())
            .await
            .map_err(db)?;
        }
        Ok(())
    }

    async fn insert_rid(&mut self, workspace: Id, rid: Id, domain: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        let domain = &domain.0[..];
        sqlx::query!(
            "INSERT INTO rids (workspace, rid, domain) VALUES ($1, $2, $3)",
            workspace,
            rid,
            domain
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_chain_head(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        head: &ChainHeadRow,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(head.counter);
        let counter = &counter[..];
        let head_digest = &head.digest.0[..];
        let seq = int(head.seq)?;
        let policy_revision = int(head.policy_revision)?;
        let generation = int(head.generation)?;
        sqlx::query!(
            "INSERT INTO chain_heads
               (workspace, domain, device, counter, digest, seq, policy_revision, generation)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (workspace, domain, device) DO UPDATE SET
               counter = excluded.counter, digest = excluded.digest, seq = excluded.seq,
               policy_revision = excluded.policy_revision, generation = excluded.generation",
            workspace,
            domain,
            device,
            counter,
            head_digest,
            seq,
            policy_revision,
            generation
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_epoch(
        &mut self,
        workspace: Id,
        domain: Id,
        row: &EpochRow,
        policy_revision: u64,
        bundle: &[u8],
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch = int(row.epoch)?;
        let seq = int(row.seq)?;
        let epoch_digest = &row.digest.0[..];
        let previous = &row.previous.0[..];
        let record = &row.record[..];
        let policy_revision = int(policy_revision)?;
        sqlx::query!(
            "INSERT INTO epochs (workspace, domain, epoch, seq, digest, previous, record, bundle)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            workspace,
            domain,
            epoch,
            seq,
            epoch_digest,
            previous,
            record,
            bundle
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "UPDATE domains SET epoch = $3, epoch_digest = $4, epoch_policy_revision = $5
             WHERE workspace = $1 AND domain = $2",
            workspace,
            domain,
            epoch,
            epoch_digest,
            policy_revision
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn append_key_entry(
        &mut self,
        workspace: Id,
        record_type: u8,
        domain: Id,
        epoch: u64,
        recipient: Option<Id>,
        record: &[u8],
    ) -> Result<u64> {
        let workspace = &workspace.0[..];
        let record_type = i16::from(record_type);
        let domain = &domain.0[..];
        let epoch = int(epoch)?;
        let recipient = recipient.map(|r| r.0.to_vec());
        // The key sequence is assigned under the key-sequence row, which the
        // first key stream row of a workspace creates.
        let row = sqlx::query!(
            "INSERT INTO key_sequences (workspace, key_seq) VALUES ($1, 1)
             ON CONFLICT (workspace) DO UPDATE SET key_seq = key_sequences.key_seq + 1
             RETURNING key_seq",
            workspace
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        let key_seq = row.key_seq;
        sqlx::query!(
            "INSERT INTO key_entries
                 (workspace, key_seq, record_type, domain, epoch, recipient, record)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            workspace,
            key_seq,
            record_type,
            domain,
            epoch,
            recipient,
            record
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        uint(key_seq)
    }

    async fn drop_old_epochs(&mut self, workspace: Id, domain: Id, epoch: u64) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch = int(epoch)?;
        sqlx::query!(
            "DELETE FROM key_entries
             WHERE workspace = $1 AND domain = $2 AND record_type IN (4, 5) AND epoch < $3",
            workspace,
            domain,
            epoch
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "DELETE FROM rewrap_requests WHERE workspace = $1 AND domain = $2 AND epoch < $3",
            workspace,
            domain,
            epoch
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "DELETE FROM rewrap_answers WHERE workspace = $1 AND domain = $2 AND epoch < $3",
            workspace,
            domain,
            epoch
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_rewrap_request(&mut self, workspace: Id, row: &RewrapRow) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &row.domain.0[..];
        let epoch = int(row.epoch)?;
        let device = &row.device.0[..];
        let key_seq = int(row.key_seq)?;
        sqlx::query!(
            "INSERT INTO rewrap_requests (workspace, domain, epoch, device, key_seq)
             VALUES ($1, $2, $3, $4, $5)",
            workspace,
            domain,
            epoch,
            device,
            key_seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_rewrap_answer(
        &mut self,
        workspace: Id,
        request: &RewrapRow,
        wrapper: Id,
        key_seq: u64,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &request.domain.0[..];
        let epoch = int(request.epoch)?;
        let device = &request.device.0[..];
        let wrapper = &wrapper.0[..];
        let key_seq = int(key_seq)?;
        sqlx::query!(
            "INSERT INTO rewrap_answers (workspace, domain, epoch, device, wrapper, key_seq)
             VALUES ($1, $2, $3, $4, $5, $6)",
            workspace,
            domain,
            epoch,
            device,
            wrapper,
            key_seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn close_rewrap(&mut self, workspace: Id, request: &RewrapRow) -> Result<()> {
        let workspace = &workspace.0[..];
        let domain = &request.domain.0[..];
        let epoch = int(request.epoch)?;
        let device = &request.device.0[..];
        let key_seq = int(request.key_seq)?;
        sqlx::query!(
            "DELETE FROM rewrap_requests
             WHERE workspace = $1 AND domain = $2 AND epoch = $3 AND device = $4",
            workspace,
            domain,
            epoch,
            device
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "DELETE FROM rewrap_answers
             WHERE workspace = $1 AND domain = $2 AND epoch = $3 AND device = $4",
            workspace,
            domain,
            epoch,
            device
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "DELETE FROM key_entries WHERE workspace = $1 AND key_seq = $2 AND record_type = 5",
            workspace,
            key_seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_device_grant(&mut self, row: &DeviceGrantRow) -> Result<()> {
        let code = &row.device_code_hash.0[..];
        let user = &row.user_code_hash.0[..];
        let account = row.account.as_ref().map(|a| a.0.to_vec());
        let interval = int(row.interval)?;
        let next_poll = int(row.next_poll)?;
        let expires_at = int(row.expires_at)?;
        sqlx::query!(
            "INSERT INTO device_grants
                 (device_code_hash, user_code_hash, account, denied, poll_interval, next_poll,
                  expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            code,
            user,
            account,
            row.denied,
            interval,
            next_poll,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn update_device_grant(&mut self, row: &DeviceGrantRow) -> Result<()> {
        let code = &row.device_code_hash.0[..];
        let account = row.account.as_ref().map(|a| a.0.to_vec());
        let interval = int(row.interval)?;
        let next_poll = int(row.next_poll)?;
        sqlx::query!(
            "UPDATE device_grants SET account = $2, denied = $3, poll_interval = $4, next_poll = $5
             WHERE device_code_hash = $1",
            code,
            account,
            row.denied,
            interval,
            next_poll
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_device_grant(&mut self, code_hash: Digest) -> Result<()> {
        let code = &code_hash.0[..];
        sqlx::query!(
            "DELETE FROM device_grants WHERE device_code_hash = $1",
            code
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn bind_session(&mut self, token_hash: Digest, device: Id) -> Result<()> {
        let hash = &token_hash.0[..];
        let device = &device.0[..];
        sqlx::query!(
            "UPDATE sessions SET device = $2 WHERE token_hash = $1",
            hash,
            device
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn remove_sessions(
        &mut self,
        account: Id,
        devices: &[Id],
        link: RemovedBy,
    ) -> Result<()> {
        let account = &account.0[..];
        let detail = DeviceRemovedDetail {
            genesis: link.genesis,
            seqno: link.seqno,
        }
        .to_bytes();
        for device in devices {
            let device = &device.0[..];
            sqlx::query!(
                "UPDATE sessions SET removed = $3
                 WHERE account = $1 AND device = $2 AND removed IS NULL",
                account,
                device,
                detail
            )
            .execute(self.c())
            .await
            .map_err(db)?;
        }
        Ok(())
    }

    async fn delete_device_sessions(&mut self, account: Id, device: Id) -> Result<u64> {
        let account = &account.0[..];
        let device = &device.0[..];
        Ok(sqlx::query!(
            "DELETE FROM sessions WHERE account = $1 AND device = $2",
            account,
            device
        )
        .execute(self.c())
        .await
        .map_err(db)?
        .rows_affected())
    }

    async fn insert_approval_offer(&mut self, row: &OfferRow) -> Result<()> {
        let offer_digest = &row.offer_digest.0[..];
        let commit = &row.commit.0[..];
        let card = &row.candidate_card_digest.0[..];
        let approver = &row.approver.0[..];
        let account = &row.account.0[..];
        let offer = &row.offer[..];
        let nc = row.nc.as_ref().map(|k| k.0.to_vec());
        let expires_at = int(row.expires_at)?;
        sqlx::query!(
            "INSERT INTO approval_offers
                 (offer_digest, commit_digest, candidate_card_digest, approver, account, offer, nc,
                  expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            offer_digest,
            commit,
            card,
            approver,
            account,
            offer,
            nc,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_reveal(&mut self, offer_digest: Digest, nc: Key) -> Result<()> {
        let offer_digest = &offer_digest.0[..];
        let nc = &nc.0[..];
        sqlx::query!(
            "UPDATE approval_offers SET nc = $2 WHERE offer_digest = $1",
            offer_digest,
            nc
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_profile(&mut self, workspace: Id, revision: u64, record: &[u8]) -> Result<()> {
        let workspace = &workspace.0[..];
        let revision = int(revision)?;
        sqlx::query!(
            "INSERT INTO profiles (workspace, profile_revision, record) VALUES ($1, $2, $3)",
            workspace,
            revision,
            record
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn drop_delete(
        &mut self,
        workspace: Id,
        rid: Id,
        device: Id,
        counter: u64,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let rid = &rid.0[..];
        let device = &device.0[..];
        let counter = counter_bytes(counter);
        let counter = &counter[..];
        sqlx::query!(
            "UPDATE operations SET dropped = TRUE
             WHERE workspace = $1 AND rid = $2 AND device = $3 AND counter = $4",
            workspace,
            rid,
            device,
            counter
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_session_recovery(&mut self, token_hash: Digest, recovery: Id) -> Result<()> {
        let hash = &token_hash.0[..];
        let recovery = &recovery.0[..];
        sqlx::query!(
            "UPDATE sessions SET recovery = $2 WHERE token_hash = $1",
            hash,
            recovery
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_enrolled_card(&mut self, device: Id, account: Id, card: &[u8]) -> Result<bool> {
        let device = &device.0[..];
        let account = &account.0[..];
        let done = sqlx::query!(
            "INSERT INTO cards (device, account, card) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            device,
            account,
            card
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(done.rows_affected() == 1)
    }

    async fn record_accounts(&mut self, workspace: Id, accounts: &[Id]) -> Result<()> {
        let workspace = &workspace.0[..];
        for account in accounts {
            let account = &account.0[..];
            sqlx::query!(
                "INSERT INTO recorded_accounts (workspace, account) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
                workspace,
                account
            )
            .execute(self.c())
            .await
            .map_err(db)?;
        }
        Ok(())
    }

    async fn insert_challenge(
        &mut self,
        challenge_hash: Digest,
        account: Id,
        expires_at: u64,
    ) -> Result<()> {
        let hash = &challenge_hash.0[..];
        let account = &account.0[..];
        let expires_at = int(expires_at)?;
        sqlx::query!(
            "INSERT INTO challenges (challenge_hash, account, expires_at) VALUES ($1, $2, $3)",
            hash,
            account,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn take_challenge(&mut self, challenge_hash: Digest, account: Id) -> Result<Option<u64>> {
        let hash = &challenge_hash.0[..];
        let account = &account.0[..];
        let row = sqlx::query!(
            "DELETE FROM challenges WHERE challenge_hash = $1 AND account = $2
             RETURNING expires_at",
            hash,
            account
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| uint(r.expires_at)).transpose()
    }

    async fn insert_approval_request(&mut self, row: &ApprovalRequestRow) -> Result<()> {
        let commit = &row.commit.0[..];
        let account = &row.account.0[..];
        let device = &row.device.0[..];
        let card = &row.card[..];
        let candidate_head = row.candidate_head.as_ref().map(|h| h.to_bytes());
        let invitation = row.invitation.as_ref().map(|i| i.0.to_vec());
        let invite_proof = row.invite_proof.as_ref().map(|p| p.0.to_vec());
        let workspace = row.workspace.as_ref().map(|w| w.0.to_vec());
        let request = &row.request[..];
        let expires_at = int(row.expires_at)?;
        sqlx::query!(
            "INSERT INTO approval_requests
                 (commit_digest, account, device, card, candidate_head, invitation, invite_proof,
                  workspace, request, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            commit,
            account,
            device,
            card,
            candidate_head,
            invitation,
            invite_proof,
            workspace,
            request,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_account(&mut self, account: Id, email: &str) -> Result<()> {
        let account = &account.0[..];
        sqlx::query!(
            "INSERT INTO account_emails (account, email) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
            account,
            email
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_passkey(&mut self, row: &PasskeyRow) -> Result<bool> {
        let credential_id = &row.credential_id[..];
        let account = &row.account.0[..];
        let passkey = &row.passkey[..];
        let done = sqlx::query!(
            "INSERT INTO passkeys (credential_id, account, passkey) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
            credential_id,
            account,
            passkey
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(done.rows_affected() == 1)
    }

    async fn update_passkey(&mut self, credential_id: &[u8], passkey: &[u8]) -> Result<()> {
        sqlx::query!(
            "UPDATE passkeys SET passkey = $2 WHERE credential_id = $1",
            credential_id,
            passkey
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_web_session(&mut self, token_hash: Digest, account: Id) -> Result<()> {
        let hash = &token_hash.0[..];
        let account = &account.0[..];
        sqlx::query!(
            "INSERT INTO web_sessions (token_hash, account) VALUES ($1, $2)",
            hash,
            account
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_web_session(&mut self, token_hash: Digest) -> Result<()> {
        let hash = &token_hash.0[..];
        sqlx::query!("DELETE FROM web_sessions WHERE token_hash = $1", hash)
            .execute(self.c())
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn insert_magic_link(
        &mut self,
        token_hash: Digest,
        email: &str,
        expires_at: u64,
    ) -> Result<()> {
        let hash = &token_hash.0[..];
        let expires_at = int(expires_at)?;
        sqlx::query!(
            "INSERT INTO magic_links (token_hash, email, expires_at) VALUES ($1, $2, $3)",
            hash,
            email,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn take_magic_link(&mut self, token_hash: Digest) -> Result<Option<(String, u64)>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "DELETE FROM magic_links WHERE token_hash = $1 RETURNING email, expires_at",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| Ok((r.email, uint(r.expires_at)?))).transpose()
    }

    async fn insert_ceremony(&mut self, row: &CeremonyRow) -> Result<()> {
        let ceremony = &row.id.0[..];
        let kind = kind_code(row.kind);
        let account = row.account.as_ref().map(|a| a.0.to_vec());
        let email = row.email.as_deref();
        let state = &row.state[..];
        let expires_at = int(row.expires_at)?;
        sqlx::query!(
            "INSERT INTO ceremonies (id, kind, account, email, state, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
            ceremony,
            kind,
            account,
            email,
            state,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn take_ceremony(&mut self, ceremony: Id) -> Result<Option<CeremonyRow>> {
        let bytes = &ceremony.0[..];
        let row = sqlx::query!(
            "DELETE FROM ceremonies WHERE id = $1 RETURNING kind, account, email, state, expires_at",
            bytes
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(CeremonyRow {
                id: ceremony,
                kind: ceremony_kind(r.kind)?,
                account: r.account.map(id).transpose()?,
                email: r.email,
                state: r.state,
                expires_at: uint(r.expires_at)?,
            })
        })
        .transpose()
    }

    async fn insert_pending_passkey(
        &mut self,
        token_hash: Digest,
        row: &PendingPasskeyRow,
    ) -> Result<()> {
        let hash = &token_hash.0[..];
        let credential_id = &row.credential_id[..];
        let passkey = &row.passkey[..];
        let expires_at = int(row.expires_at)?;
        sqlx::query!(
            "INSERT INTO pending_passkeys (token_hash, email, credential_id, passkey, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
            hash,
            row.email,
            credential_id,
            passkey,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn take_pending_passkey(
        &mut self,
        token_hash: Digest,
    ) -> Result<Option<PendingPasskeyRow>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "DELETE FROM pending_passkeys WHERE token_hash = $1
             RETURNING email, credential_id, passkey, expires_at",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(PendingPasskeyRow {
                email: r.email,
                credential_id: r.credential_id,
                passkey: r.passkey,
                expires_at: uint(r.expires_at)?,
            })
        })
        .transpose()
    }

    async fn delete_expired(&mut self, now: u64) -> Result<u64> {
        let now = at_most(now);
        let mut deleted = 0;
        deleted += sqlx::query!("DELETE FROM reset_codes WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM magic_links WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM pending_passkeys WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM ceremonies WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM challenges WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM approval_requests WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM approval_offers WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM device_grants WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM invitations WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        deleted += sqlx::query!("DELETE FROM ended_invitations WHERE expires_at <= $1", now)
            .execute(self.c())
            .await
            .map_err(db)?
            .rows_affected();
        Ok(deleted)
    }

    async fn insert_blob(&mut self, workspace: Id, row: &BlobRow) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &row.blob.0[..];
        let domain = &row.domain.0[..];
        let device = &row.device.0[..];
        let length = int(row.length)?;
        let digest = &row.digest.0[..];
        let offset = int(row.offset)?;
        let complete = row.complete;
        let delete_after = row.delete_after.map(at_most);
        sqlx::query!(
            "INSERT INTO blobs (workspace, blob, domain, device, length, digest, upload_offset, complete, delete_after) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            workspace,
            blob,
            domain,
            device,
            length,
            digest,
            offset,
            complete,
            delete_after
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn append_blob_segment(
        &mut self,
        workspace: Id,
        blob: Id,
        index: u64,
        hash: Digest,
        offset: u64,
        complete: bool,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let index = int(index)?;
        let hash = &hash.0[..];
        let offset = int(offset)?;
        sqlx::query!(
            "INSERT INTO blob_segments (workspace, blob, idx, hash) VALUES ($1, $2, $3, $4)",
            workspace,
            blob,
            index,
            hash
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "UPDATE blobs SET upload_offset = $3, complete = $4 WHERE workspace = $1 AND blob = $2",
            workspace,
            blob,
            offset,
            complete
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn reset_blob(
        &mut self,
        workspace: Id,
        blob: Id,
        delete_after: Option<u64>,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let delete_after = delete_after.map(at_most);
        sqlx::query!(
            "DELETE FROM blob_segments WHERE workspace = $1 AND blob = $2",
            workspace,
            blob
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "UPDATE blobs SET upload_offset = 0, complete = FALSE, delete_after = $3
             WHERE workspace = $1 AND blob = $2",
            workspace,
            blob,
            delete_after
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn put_reset_code(&mut self, account: Id, hash: Digest, expires_at: u64) -> Result<()> {
        let account = &account.0[..];
        let hash = &hash.0[..];
        let expires_at = at_most(expires_at);
        sqlx::query!(
            "INSERT INTO reset_codes (account, code_hash, expires_at) VALUES ($1, $2, $3)
             ON CONFLICT (account) DO UPDATE
             SET code_hash = excluded.code_hash, expires_at = excluded.expires_at",
            account,
            hash,
            expires_at
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn take_reset_code(&mut self, account: Id) -> Result<()> {
        let account = &account.0[..];
        sqlx::query!("DELETE FROM reset_codes WHERE account = $1", account)
            .execute(self.c())
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn close_workspace(&mut self, workspace: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        sqlx::query!(
            "UPDATE workspaces SET closed = 1 WHERE workspace = $1",
            workspace
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!("DELETE FROM members WHERE workspace = $1", workspace)
            .execute(self.c())
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn hold_lease(&mut self, instance: Id, until: u64) -> Result<()> {
        let instance = &instance.0[..];
        let until = at_most(until);
        sqlx::query!(
            "INSERT INTO instances (instance, lease_until) VALUES ($1, $2)
             ON CONFLICT (instance) DO UPDATE SET lease_until = excluded.lease_until",
            instance,
            until
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn drop_lease(&mut self, instance: Id) -> Result<()> {
        let instance = &instance.0[..];
        sqlx::query!("DELETE FROM instances WHERE instance = $1", instance)
            .execute(self.c())
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn delete_blob(&mut self, workspace: Id, blob: Id) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        sqlx::query!(
            "DELETE FROM blob_segments WHERE workspace = $1 AND blob = $2",
            workspace,
            blob
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "DELETE FROM blobs WHERE workspace = $1 AND blob = $2",
            workspace,
            blob
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_blob_listing(
        &mut self,
        workspace: Id,
        blob: Id,
        domain: Id,
        seq: u64,
    ) -> Result<()> {
        let workspace = &workspace.0[..];
        let blob = &blob.0[..];
        let domain = &domain.0[..];
        let seq = int(seq)?;
        sqlx::query!(
            "INSERT INTO blob_listings (workspace, blob, domain, seq) VALUES ($1, $2, $3, $4)",
            workspace,
            blob,
            domain,
            seq
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_invitation(&mut self, row: &InvitationRow) -> Result<()> {
        let invitation = &row.invitation.0[..];
        let hash = &row.token_hash.0[..];
        let workspace = &row.workspace.0[..];
        let role = i16::from(row.role.code());
        let inviter = &row.inviter.0[..];
        let accepted_by = row.accepted_by.as_ref().map(|a| &a.0[..]);
        let expires_at = int(row.expires_at)?;
        let context = &row.invite_context[..];
        sqlx::query!(
            "INSERT INTO invitations (invitation, token_hash, workspace, role, inviter, accepted_by, expires_at, invite_context) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            invitation,
            hash,
            workspace,
            role,
            inviter,
            accepted_by,
            expires_at,
            context
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn accept_invitation(&mut self, invitation: Id, account: Id) -> Result<()> {
        let invitation = &invitation.0[..];
        let account = &account.0[..];
        sqlx::query!(
            "UPDATE invitations SET accepted_by = $1 WHERE invitation = $2",
            account,
            invitation
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_invitation(&mut self, invitation: Id) -> Result<()> {
        let invitation = &invitation.0[..];
        sqlx::query!(
            "INSERT INTO ended_invitations (invitation, expires_at) SELECT invitation, expires_at FROM invitations WHERE invitation = $1 ON CONFLICT DO NOTHING",
            invitation
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!("DELETE FROM invitations WHERE invitation = $1", invitation)
            .execute(self.c())
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn insert_share(&mut self, row: &ShareRow) -> Result<()> {
        let hash = &row.token_hash.0[..];
        let workspace = &row.workspace.0[..];
        let domain = &row.domain.0[..];
        let device = &row.device.0[..];
        let expires_at = int(row.expires_at)?;
        let share = &row.share[..];
        sqlx::query!(
            "INSERT INTO shares (token_hash, workspace, domain, device, expires_at, share) VALUES ($1, $2, $3, $4, $5, $6)",
            hash,
            workspace,
            domain,
            device,
            expires_at,
            share
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn set_share(&mut self, token_hash: Digest, share: &[u8]) -> Result<()> {
        let hash = &token_hash.0[..];
        sqlx::query!(
            "UPDATE shares SET share = $1 WHERE token_hash = $2",
            share,
            hash
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn delete_share(&mut self, token_hash: Digest) -> Result<()> {
        let hash = &token_hash.0[..];
        sqlx::query!("DELETE FROM shares WHERE token_hash = $1", hash)
            .execute(self.c())
            .await
            .map_err(db)?;
        Ok(())
    }
}
