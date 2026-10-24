//! The store's reads and writes on SQLite, each a compile-checked query.

use sqlx::{Sqlite, SqliteConnection, Transaction};
use sync_protocol::live::LiveOp;
use sync_protocol::records::{OpClass, Ref};
use sync_protocol::types::{Digest, Id};
use sync_store::{
    ChainHeadRow, DeleteRow, DomainRow, EpochRow, Error, KeyRow, LinkRow, LockPlan, RawEntry,
    ReadTxn, Result, Session, WorkspaceRow, WriteTxn,
};

/// A read transaction from the pool, or the writer's connection inside its
/// write transaction.
pub struct SqliteTxn {
    inner: Inner,
}

enum Inner {
    Read(Option<Transaction<'static, Sqlite>>),
    Writer(SqliteConnection),
}

impl SqliteTxn {
    pub(crate) fn read(transaction: Transaction<'static, Sqlite>) -> Self {
        Self {
            inner: Inner::Read(Some(transaction)),
        }
    }

    pub(crate) fn writer(connection: SqliteConnection) -> Self {
        Self {
            inner: Inner::Writer(connection),
        }
    }

    fn c(&mut self) -> &mut SqliteConnection {
        match &mut self.inner {
            Inner::Read(Some(transaction)) => transaction,
            Inner::Writer(connection) => connection,
            Inner::Read(None) => unreachable!("a finished read transaction takes no query"),
        }
    }

    /// Ends a read transaction.
    pub(crate) async fn finish_read(&mut self) -> Result<()> {
        if let Inner::Read(transaction) = &mut self.inner
            && let Some(transaction) = transaction.take()
        {
            transaction.commit().await.map_err(Error::internal)?;
        }
        Ok(())
    }

    /// Runs a transaction-control statement on the writer.
    pub(crate) async fn execute(&mut self, sql: &'static str) -> std::result::Result<(), String> {
        sqlx::raw_sql(sql)
            .execute(self.c())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Runs a WAL checkpoint in `PASSIVE` or `TRUNCATE` mode.
    pub(crate) async fn checkpoint(&mut self, mode: &str) -> std::result::Result<(), String> {
        let sql = if mode == "TRUNCATE" {
            "PRAGMA wal_checkpoint(TRUNCATE)"
        } else {
            "PRAGMA wal_checkpoint(PASSIVE)"
        };
        self.execute(sql).await
    }

    pub(crate) fn into_writer(self) -> Option<SqliteConnection> {
        match self.inner {
            Inner::Writer(connection) => Some(connection),
            Inner::Read(_) => None,
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

/// A server-assigned or server-checked number as an INTEGER.
fn int(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::internal("a number exceeds an INTEGER"))
}

/// A bound for "every sequence".
fn at_most(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn uint(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::internal("a negative number in an unsigned column"))
}

fn byte(value: i64) -> Result<u8> {
    u8::try_from(value).map_err(|_| Error::internal("a type column exceeds a byte"))
}

fn class_code(class: OpClass) -> i64 {
    match class {
        OpClass::State => 1,
        OpClass::Patch => 2,
    }
}

fn class(code: i64) -> Result<OpClass> {
    match code {
        1 => Ok(OpClass::State),
        2 => Ok(OpClass::Patch),
        _ => Err(Error::internal("unknown operation class")),
    }
}

fn db(error: sqlx::Error) -> Error {
    Error::internal(error)
}

impl ReadTxn for SqliteTxn {
    async fn restore_generation(&mut self) -> Result<u64> {
        let row = sqlx::query!("SELECT restore_generation FROM server WHERE id = 1")
            .fetch_one(self.c())
            .await
            .map_err(db)?;
        uint(row.restore_generation)
    }

    async fn session(&mut self, token_hash: Digest) -> Result<Option<Session>> {
        let hash = &token_hash.0[..];
        let row = sqlx::query!(
            "SELECT account, device FROM sessions WHERE token_hash = ?1",
            hash
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(Session {
                account: id(r.account)?,
                device: id(r.device)?,
            })
        })
        .transpose()
    }

    async fn current_genesis(&mut self, account: Id) -> Result<Option<Digest>> {
        let account = &account.0[..];
        let row = sqlx::query!("SELECT genesis FROM accounts WHERE account = ?1", account)
            .fetch_optional(self.c())
            .await
            .map_err(db)?;
        row.map(|r| digest(r.genesis)).transpose()
    }

    async fn account_links(&mut self, account: Id) -> Result<Vec<LinkRow>> {
        let account = &account.0[..];
        let rows = sqlx::query!(
            "SELECT genesis, seqno, link FROM account_links WHERE account = ?1 ORDER BY genesis, seqno",
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

    async fn workspace(&mut self, workspace: Id) -> Result<Option<WorkspaceRow>> {
        let workspace = &workspace.0[..];
        let row = sqlx::query!(
            "SELECT owner, creation_digest, creation_response, policy_revision, policy_head
             FROM workspaces WHERE workspace = ?1",
            workspace
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(WorkspaceRow {
                owner: id(r.owner)?,
                creation_digest: digest(r.creation_digest)?,
                creation_response: r.creation_response,
                policy_revision: uint(r.policy_revision)?,
                policy_head: digest(r.policy_head)?,
            })
        })
        .transpose()
    }

    async fn owned_workspaces(&mut self, account: Id) -> Result<u64> {
        let account = &account.0[..];
        let row = sqlx::query!(
            r#"SELECT COUNT(*) AS "count!: i64" FROM workspaces WHERE owner = ?1"#,
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
            "SELECT workspace, role FROM members WHERE account = ?1 ORDER BY workspace",
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
            "SELECT record FROM policy_changes WHERE workspace = ?1 AND revision > ?2 ORDER BY revision",
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
            "SELECT record FROM profiles WHERE workspace = ?1 ORDER BY profile_revision DESC LIMIT 1",
            workspace
        )
        .fetch_optional(self.c())
        .await
        .map_err(db)?;
        Ok(row.map(|r| r.record))
    }

    async fn domain(&mut self, workspace: Id, domain: Id) -> Result<Option<DomainRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let row = sqlx::query!(
            "SELECT head_seq, epoch, epoch_digest, epoch_policy_revision, retention_floor
             FROM domains WHERE workspace = ?1 AND domain = ?2",
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
             WHERE workspace = ?1 AND domain = ?2 AND seq > ?3 ORDER BY seq LIMIT ?4",
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
            r#"SELECT seq AS "seq!", digest AS "digest!" FROM operations
             WHERE workspace = ?1 AND domain = ?2 AND device = ?3 AND counter = ?4"#,
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
             WHERE o.workspace = ?1 AND o.domain = ?2 AND o.device = ?3 AND o.counter >= ?4
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
             WHERE workspace = ?1 AND domain = ?2 AND device = ?3",
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
            "SELECT domain FROM rids WHERE workspace = ?1 AND rid = ?2",
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
                 WHERE workspace = ?1 AND rid = ?2 AND device = ?3 AND counter = ?4
             ) AS "exists!: bool""#,
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
                 WHERE workspace = ?1 AND rid = ?2 AND device = ?3 AND counter >= ?4 AND by_seq <= ?5
             ) AS "covered!: bool""#,
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
                      o.digest AS "digest!", o.is_delete AS "is_delete!: bool",
                      o.class AS "class!", o.weight AS "weight!"
             FROM operations o
             WHERE o.workspace = ?1 AND o.rid = ?2 AND o.seq <= ?3 AND o.dropped = 0
               AND NOT EXISTS (
                 SELECT 1 FROM covers c
                 WHERE c.workspace = o.workspace AND c.rid = o.rid AND c.device = o.device
                   AND c.counter >= o.counter AND c.by_seq <= ?3
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
             WHERE workspace = ?1 AND domain = ?2 AND is_delete = 1 AND dropped = 0
               AND seq > ?3 AND seq <= ?4
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
                 WHERE workspace = ?1 AND domain = ?2 AND covered_seq = ?3 AND digest = ?4
             ) AS "exists!: bool""#,
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

    async fn epoch(&mut self, workspace: Id, domain: Id, epoch: u64) -> Result<Option<EpochRow>> {
        let workspace = &workspace.0[..];
        let domain = &domain.0[..];
        let epoch = at_most(epoch);
        let row = sqlx::query!(
            "SELECT epoch, digest, previous, record FROM epochs
             WHERE workspace = ?1 AND domain = ?2 AND epoch = ?3",
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
            "SELECT epoch, digest, previous, record FROM epochs
             WHERE workspace = ?1 AND domain = ?2 AND digest = ?3",
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
             WHERE workspace = ?1 AND key_seq > ?2 ORDER BY key_seq LIMIT ?3",
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
            r#"SELECT COALESCE(MAX(key_seq), 0) AS "head!: i64" FROM key_entries WHERE workspace = ?1"#,
            workspace
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        uint(row.head)
    }
}

impl WriteTxn for SqliteTxn {
    async fn lock(&mut self, _plan: &LockPlan) -> Result<()> {
        // The one writer holds the database for the whole transaction, which
        // stands in for every lock of §15.
        Ok(())
    }

    async fn lock_account(&mut self, _account: Id) -> Result<()> {
        // The one writer stands in for the account's chain row.
        Ok(())
    }

    async fn lock_pruning(&mut self, _workspace: Id, _domain: Id) -> Result<()> {
        // The one writer stands in for the pruning lock.
        Ok(())
    }

    async fn insert_session(&mut self, token_hash: Digest, session: Session) -> Result<()> {
        let hash = &token_hash.0[..];
        let account = &session.account.0[..];
        let device = &session.device.0[..];
        sqlx::query!(
            "INSERT INTO sessions (token_hash, account, device) VALUES (?1, ?2, ?3)",
            hash,
            account,
            device
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
            "INSERT INTO account_links (account, genesis, seqno, digest, link) VALUES (?1, ?2, ?3, ?4, ?5)",
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

    async fn set_current_genesis(&mut self, account: Id, genesis: Digest) -> Result<()> {
        let account = &account.0[..];
        let genesis = &genesis.0[..];
        sqlx::query!(
            "INSERT INTO accounts (account, genesis) VALUES (?1, ?2)
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
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
        let role = i64::from(role);
        sqlx::query!(
            "INSERT INTO members (workspace, account, role) VALUES (?1, ?2, ?3)
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
            "INSERT INTO policy_changes (workspace, revision, digest, record) VALUES (?1, ?2, ?3, ?4)",
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
            "UPDATE workspaces SET policy_revision = ?2, policy_head = ?3 WHERE workspace = ?1",
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
             VALUES (?1, ?2, 0, 0, ?3, 0)",
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
        let row = sqlx::query!(
            "UPDATE domains SET head_seq = head_seq + 1 WHERE workspace = ?1 AND domain = ?2
             RETURNING head_seq",
            workspace,
            domain
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        let seq = row.head_seq;
        let record_type = i64::from(record_type);
        sqlx::query!(
            "INSERT INTO entries (workspace, domain, seq, record_type, record) VALUES (?1, ?2, ?3, ?4, ?5)",
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
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
            "INSERT INTO rids (workspace, rid, domain) VALUES (?1, ?2, ?3)",
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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
        let epoch_digest = &row.digest.0[..];
        let previous = &row.previous.0[..];
        let record = &row.record[..];
        let policy_revision = int(policy_revision)?;
        sqlx::query!(
            "INSERT INTO epochs (workspace, domain, epoch, digest, previous, record, bundle)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            workspace,
            domain,
            epoch,
            epoch_digest,
            previous,
            record,
            bundle
        )
        .execute(self.c())
        .await
        .map_err(db)?;
        sqlx::query!(
            "UPDATE domains SET epoch = ?3, epoch_digest = ?4, epoch_policy_revision = ?5
             WHERE workspace = ?1 AND domain = ?2",
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
        recipient: Option<Id>,
        record: &[u8],
    ) -> Result<u64> {
        let workspace = &workspace.0[..];
        let record_type = i64::from(record_type);
        let domain = &domain.0[..];
        let recipient = recipient.map(|r| r.0.to_vec());
        let row = sqlx::query!(
            "INSERT INTO key_entries (workspace, key_seq, record_type, domain, recipient, record)
             VALUES (?1, (SELECT COALESCE(MAX(key_seq), 0) + 1 FROM key_entries WHERE workspace = ?1),
                     ?2, ?3, ?4, ?5)
             RETURNING key_seq",
            workspace,
            record_type,
            domain,
            recipient,
            record
        )
        .fetch_one(self.c())
        .await
        .map_err(db)?;
        uint(row.key_seq)
    }

    async fn insert_profile(&mut self, workspace: Id, revision: u64, record: &[u8]) -> Result<()> {
        let workspace = &workspace.0[..];
        let revision = int(revision)?;
        sqlx::query!(
            "INSERT INTO profiles (workspace, profile_revision, record) VALUES (?1, ?2, ?3)",
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
            "UPDATE operations SET dropped = 1
             WHERE workspace = ?1 AND rid = ?2 AND device = ?3 AND counter = ?4",
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
}
