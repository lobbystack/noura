//! The sync server's store (docs/workspace-format/sync-v2.md §15): the
//! transaction traits each database implements, the checks and commits
//! every store shares, and the conformance suite both stores pass.
//!
//! A database implements [`Backend`]: read transactions that each see one
//! snapshot, and write transactions that run through its writer, plus the
//! row-level reads and writes of [`ReadTxn`] and [`WriteTxn`]. [`Store`]
//! runs the protocol's checks (§8, §9, §13, §15) inside those transactions,
//! so SQLite and PostgreSQL validate and commit records the same way, and
//! no record is checked against a state older than the one it commits after.
//!
//! Unsigned values that a client chooses, such as chain counters, compare by
//! value in queries, so a backend must store them in a form that keeps
//! unsigned order across the full `u64` range.

#![forbid(unsafe_code)]

mod create;
mod hub;
pub mod limiter;
mod live;
mod policy;
mod pull;
mod push;
mod reads;

#[cfg(feature = "conformance")]
pub mod conformance;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Bytes, Decode, Text};
use sync_protocol::live::{LiveError, LiveOp};
use sync_protocol::messages::{
    EpochsResponse, ErrorBody, HeadResponse, KeysResponse, LimitsResponse, MAX_BODY,
    MAX_WAIT_SECONDS, PolicyResponse, PullRequest, PullResponse, PushResponse, WorkspacesResponse,
    server_error_status,
};
use sync_protocol::records::{
    AccountHead, AccountLink, MAX_COVERS, MAX_RECIPIENTS, OpClass, Ref, Signed,
};
use sync_protocol::types::{Digest, Id};

pub use hub::EventHub;
pub use limiter::{Limit, Limiter, MemoryLimiter, Taken};
pub use policy::PolicyCache;

/// The restore generation a new database starts at (§14). Restores raise it
/// (ticket 60).
pub const INITIAL_RESTORE_GENERATION: u64 = 1;

/// The default `retained_bytes` that `GET /v2/limits` reports (§19).
pub const DEFAULT_RETAINED_BYTES: u64 = 1 << 30;

/// A boxed future that a backend's transaction methods and jobs return.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A refusal that the protocol names (§26): an error code, a message with
/// no database or operating system detail, and the code's `detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The server error code.
    pub code: &'static str,
    /// What went wrong, for people.
    pub message: String,
    /// The code's structured detail.
    pub detail: Option<Vec<u8>>,
}

impl Refusal {
    /// A refusal with no detail.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    /// The HTTP status §26 gives the code.
    pub fn status(&self) -> u16 {
        server_error_status(self.code).unwrap_or(500)
    }

    /// The error body.
    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            code: Text(self.code.to_owned()),
            message: Text(truncate(&self.message, 1_024)),
            retry_after_seconds: None,
            detail: self.detail.clone().map(Bytes),
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Why a store call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The protocol refuses the request.
    #[error("refused with {}: {}", .0.code, .0.message)]
    Refused(Refusal),
    /// The database failed. The text belongs in the server's local log,
    /// never in a response.
    #[error("store failure: {0}")]
    Internal(String),
}

impl Error {
    /// A database failure.
    pub fn internal(error: impl std::fmt::Display) -> Self {
        Error::Internal(error.to_string())
    }
}

/// A store call's result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn refuse(code: &'static str, message: impl Into<String>) -> Error {
    Error::Refused(Refusal::new(code, message))
}

/// A signed-in device: the account and device a session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    /// The account.
    pub account: Id,
    /// The session's device.
    pub device: Id,
}

/// A workspace's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRow {
    /// The owner account at creation.
    pub owner: Id,
    /// `BLAKE3` of the creation request, so a retry with identical bytes
    /// gets the original result.
    pub creation_digest: Digest,
    /// The encoded creation response.
    pub creation_response: Vec<u8>,
    /// The policy head's revision.
    pub policy_revision: u64,
    /// The policy head's digest.
    pub policy_head: Digest,
}

/// A domain's head row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomainRow {
    /// The domain's latest sequence.
    pub head_seq: u64,
    /// The epoch of the latest epoch entry.
    pub epoch: u64,
    /// That epoch record's digest.
    pub epoch_digest: Digest,
    /// That epoch record's `policy_revision`.
    pub epoch_policy_revision: u64,
    /// The highest retention floor the server published (§19).
    pub retention_floor: u64,
}

/// A device's chain head in one domain, with the fields the next record is
/// checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainHeadRow {
    /// The head's counter.
    pub counter: u64,
    /// The head's digest.
    pub digest: Digest,
    /// The head's sequence.
    pub seq: u64,
    /// The head record's `policy_revision`.
    pub policy_revision: u64,
    /// The head record's `generation`.
    pub generation: u64,
}

/// A log entry as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEntry {
    /// Its sequence.
    pub seq: u64,
    /// The record type of §15: 1 operation, 5 checkpoint bundle, 8 epoch
    /// record, 9 policy entry.
    pub record_type: u8,
    /// The record's bytes.
    pub record: Vec<u8>,
}

/// A key stream row (§26, `keys`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRow {
    /// The key sequence.
    pub key_seq: u64,
    /// 3 epoch record, 4 envelope, 5 re-wrap request.
    pub record_type: u8,
    /// The domain it belongs to.
    pub domain: Id,
    /// An envelope's recipient.
    pub recipient: Option<Id>,
    /// The record's bytes; an epoch record without its recipients.
    pub record: Vec<u8>,
}

/// An epoch record as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochRow {
    /// The epoch number.
    pub epoch: u64,
    /// The record's digest.
    pub digest: Digest,
    /// The digest its `previous` names.
    pub previous: Digest,
    /// The signed epoch record, without its recipients.
    pub record: Vec<u8>,
}

/// An account chain link as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRow {
    /// The digest of the chain's genesis link.
    pub genesis: Digest,
    /// The link's seqno.
    pub seqno: u64,
    /// The signed link.
    pub link: Vec<u8>,
}

/// A lone delete a rising checkpoint floor may drop (§17).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteRow {
    /// The object.
    pub rid: Id,
    /// The operation's ref.
    pub op: Ref,
}

/// The locks a push takes, in the order of §15: the workspace's policy
/// lock, shared without a policy change and exclusive with one, the key
/// sequence row, the domains' head rows in domain-ID order and the blobs'
/// rows in blob-ID order. SQLite's single writer stands in for all of them;
/// PostgreSQL takes the policy lock as a transaction-level advisory lock in
/// the one-key form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockPlan {
    /// The workspace.
    pub workspace: Option<Id>,
    /// Whether the push changes the policy.
    pub policy_exclusive: bool,
    /// Whether the push carries an epoch bundle or envelope.
    pub key_sequence: bool,
    /// The domains it touches, sorted.
    pub domains: Vec<Id>,
    /// The blobs it lists, sorted.
    pub blobs: Vec<Id>,
}

/// The pushes a backend may commit together: those into one domain of one
/// workspace that change no policy (§15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WriteGroup {
    /// The workspace.
    pub workspace: Id,
    /// The one domain every push of the group writes.
    pub domain: Id,
}

/// The reads a store transaction offers. Every read in one transaction sees
/// one snapshot.
pub trait ReadTxn: Send {
    /// The server's restore generation.
    fn restore_generation(&mut self) -> impl Future<Output = Result<u64>> + Send;
    /// The session whose token hashes to `token_hash`.
    fn session(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<Session>>> + Send;
    /// The genesis digest of the account's current chain.
    fn current_genesis(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Option<Digest>>> + Send;
    /// Every link of every chain of the account, by genesis then seqno.
    fn account_links(&mut self, account: Id) -> impl Future<Output = Result<Vec<LinkRow>>> + Send;
    /// The head of the account's current chain: its genesis digest and its
    /// last link's seqno and digest.
    fn current_head(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Option<AccountHead>>> + Send;
    /// A workspace's row.
    fn workspace(
        &mut self,
        workspace: Id,
    ) -> impl Future<Output = Result<Option<WorkspaceRow>>> + Send;
    /// How many workspaces the account owns.
    fn owned_workspaces(&mut self, account: Id) -> impl Future<Output = Result<u64>> + Send;
    /// The workspaces the account belongs to, with its role byte.
    fn memberships(&mut self, account: Id) -> impl Future<Output = Result<Vec<(Id, u8)>>> + Send;
    /// The policy changes after `after`, by revision.
    fn policy_changes(
        &mut self,
        workspace: Id,
        after: u64,
    ) -> impl Future<Output = Result<Vec<Vec<u8>>>> + Send;
    /// The latest profile record.
    fn latest_profile(
        &mut self,
        workspace: Id,
    ) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send;
    /// A domain's head row.
    fn domain(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<Option<DomainRow>>> + Send;
    /// At most `limit` log entries after `after`, by sequence.
    fn entries_after(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RawEntry>>> + Send;
    /// The sequence and digest of the device's operation at exactly
    /// `counter`.
    fn position(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> impl Future<Output = Result<Option<(u64, Digest)>>> + Send;
    /// The device's operation at `counter`, or its next one above it.
    fn position_at_or_above(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> impl Future<Output = Result<Option<RawEntry>>> + Send;
    /// The device's chain head in a domain.
    fn chain_head(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
    ) -> impl Future<Output = Result<Option<ChainHeadRow>>> + Send;
    /// The domain an object belongs to.
    fn rid_domain(
        &mut self,
        workspace: Id,
        rid: Id,
    ) -> impl Future<Output = Result<Option<Id>>> + Send;
    /// Whether the store holds an operation on `rid` with exactly this
    /// device and counter.
    fn op_exists(
        &mut self,
        workspace: Id,
        rid: Id,
        device: Id,
        counter: u64,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Whether an operation on `rid` at or below sequence `at` covers the
    /// device's operation at `counter`.
    fn covered(
        &mut self,
        workspace: Id,
        rid: Id,
        device: Id,
        counter: u64,
        at: u64,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// The operations live on `rid` at sequence `at`, with the classes the
    /// server gave them, in log order. Their covers may be left out.
    fn live_ops(
        &mut self,
        workspace: Id,
        rid: Id,
        at: u64,
    ) -> impl Future<Output = Result<Vec<(LiveOp, OpClass)>>> + Send;
    /// The deletes of a domain with a sequence in (`after`, `through`] that
    /// no floor dropped yet.
    fn deletes_between(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        through: u64,
    ) -> impl Future<Output = Result<Vec<DeleteRow>>> + Send;
    /// Whether the domain holds a checkpoint with this `covered_seq` and
    /// digest.
    fn checkpoint_exists(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
        digest: Digest,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// A domain's epoch record by number.
    fn epoch(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> impl Future<Output = Result<Option<EpochRow>>> + Send;
    /// A domain's epoch record by digest.
    fn epoch_by_digest(
        &mut self,
        workspace: Id,
        domain: Id,
        digest: Digest,
    ) -> impl Future<Output = Result<Option<EpochRow>>> + Send;
    /// At most `limit` key stream rows after `after`, by key sequence.
    fn key_entries(
        &mut self,
        workspace: Id,
        after: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<KeyRow>>> + Send;
    /// The workspace's latest key sequence.
    fn key_head(&mut self, workspace: Id) -> impl Future<Output = Result<u64>> + Send;
}

/// The writes a store's write transaction offers, besides its reads.
pub trait WriteTxn: ReadTxn {
    /// Takes the locks of §15 in their order, before any read the push's
    /// checks depend on.
    fn lock(&mut self, plan: &LockPlan) -> impl Future<Output = Result<()>> + Send;
    /// Locks the account's chain row before an append reads the chain. An
    /// append takes no workspace lock, so it never waits in a cycle with a
    /// push (§15).
    fn lock_account(&mut self, account: Id) -> impl Future<Output = Result<()>> + Send;
    /// Takes a domain's pruning lock (§15), which one instance at a time
    /// holds while it prunes the domain. In PostgreSQL it is an advisory
    /// lock in the two-key form, keyed by the first 4 bytes of the
    /// workspace ID and of the domain ID, so it never meets a policy lock,
    /// and two domains whose prefixes collide only delay each other.
    fn lock_pruning(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a session.
    fn insert_session(
        &mut self,
        token_hash: Digest,
        session: Session,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores an account chain link.
    fn insert_account_link(
        &mut self,
        account: Id,
        link: &LinkRow,
        digest: Digest,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Makes `genesis` the account's current chain.
    fn set_current_genesis(
        &mut self,
        account: Id,
        genesis: Digest,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a new workspace.
    fn insert_workspace(
        &mut self,
        workspace: Id,
        row: &WorkspaceRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Records a member account and its role byte.
    fn upsert_member(
        &mut self,
        workspace: Id,
        account: Id,
        role: u8,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a policy change. The workspace row names the head.
    fn insert_policy_change(
        &mut self,
        workspace: Id,
        revision: u64,
        digest: Digest,
        record: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Makes the change at `revision` the workspace's policy head.
    fn set_policy_head(
        &mut self,
        workspace: Id,
        revision: u64,
        digest: Digest,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Creates a domain's head row, empty and at no epoch.
    fn insert_domain(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Appends a log entry at the domain's next sequence, under its head
    /// row, and returns that sequence.
    fn append_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        record_type: u8,
        record: &[u8],
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Records an operation's live-set fields, its covers included.
    fn insert_operation(
        &mut self,
        workspace: Id,
        domain: Id,
        op: &LiveOp,
        class: OpClass,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Records that `rid` belongs to `domain`.
    fn insert_rid(
        &mut self,
        workspace: Id,
        rid: Id,
        domain: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Sets a device's chain head in a domain.
    fn set_chain_head(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        head: &ChainHeadRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores an epoch bundle and makes its record the domain's latest
    /// epoch entry.
    fn insert_epoch(
        &mut self,
        workspace: Id,
        domain: Id,
        row: &EpochRow,
        policy_revision: u64,
        bundle: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Appends a key stream row at the workspace's next key sequence, under
    /// its key sequence row, and returns that sequence.
    fn append_key_entry(
        &mut self,
        workspace: Id,
        record_type: u8,
        domain: Id,
        recipient: Option<Id>,
        record: &[u8],
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Stores a profile record.
    fn insert_profile(
        &mut self,
        workspace: Id,
        revision: u64,
        record: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Drops a lone delete from the live set for good (§17).
    fn drop_delete(
        &mut self,
        workspace: Id,
        rid: Id,
        device: Id,
        counter: u64,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// A job that runs in one read transaction.
pub type ReadJob<T, R> = Box<dyn for<'t> FnOnce(&'t mut T) -> BoxFut<'t, Result<R>> + Send>;

/// A job that runs in a write transaction. Its writes commit only when it
/// returns `Ok` and the transaction commits; its result reaches the caller
/// only after the commit is durable.
pub type WriteJob<T, R> = Box<dyn for<'t> FnOnce(&'t mut T) -> BoxFut<'t, Result<R>> + Send>;

/// A database a [`Store`] runs on.
pub trait Backend: Send + Sync + 'static {
    /// A read transaction.
    type Read: ReadTxn;
    /// A write transaction.
    type Write: WriteTxn;

    /// Runs `job` in one read transaction, which holds one snapshot and
    /// ends when the job does.
    fn read<R: Send + 'static>(&self, job: ReadJob<Self::Read, R>) -> BoxFut<'_, Result<R>>;

    /// Runs `job` in a write transaction and returns once its writes are
    /// durable, or once it has rolled them back.
    fn write<R: Send + 'static>(&self, job: WriteJob<Self::Write, R>) -> BoxFut<'_, Result<R>>;

    /// Runs a push's `job` like [`Backend::write`]. A backend may commit
    /// queued jobs of one `group` in one transaction with a savepoint each,
    /// so one job's failure rolls back only that job (§15).
    fn write_grouped<R: Send + 'static>(
        &self,
        group: WriteGroup,
        job: WriteJob<Self::Write, R>,
    ) -> BoxFut<'_, Result<R>> {
        let _ = group;
        self.write(job)
    }

    /// Starts feeding `hub` the commits that other instances on the same
    /// database announce. A database that one instance serves needs
    /// nothing.
    fn attach(&self, hub: EventHub) -> BoxFut<'_, Result<()>> {
        let _ = hub;
        Box::pin(async { Ok(()) })
    }

    /// Tells the other instances on the same database that `workspace`'s
    /// domains reached these heads. The store calls it only after the push
    /// commits, and it returns at once: the announcement leaves from its own
    /// statement outside the push's transaction, and a lost one costs at
    /// most a pull's wait.
    fn announce(&self, workspace: Id, heads: Vec<(Id, u64)>) {
        let _ = (workspace, heads);
    }

    /// Whether write transactions run side by side, so the locks of §15
    /// decide which one waits. SQLite's one writer runs them one at a time.
    fn parallel_writes(&self) -> bool {
        false
    }

    /// The limiter for this database's mode: counters in memory for one
    /// instance, in the database for several.
    fn limiter(&self) -> Arc<dyn Limiter>;

    /// Finishes the queued writes and closes every connection.
    fn close(&self) -> BoxFut<'_, ()>;
}

/// The protocol's state on a database: workspace creation, push and pull,
/// and the reads of §26, each checked as the spec sets.
pub struct Store<B: Backend> {
    backend: B,
    policies: PolicyCache,
    chains: policy::ChainCache,
    hub: EventHub,
    generation: u64,
    retained_bytes: u64,
}

impl<B: Backend> Store<B> {
    /// A store on `backend`, which must hold a migrated database. The
    /// backend starts feeding the store's event hub the commits of other
    /// instances.
    pub async fn open(backend: B) -> Result<Self> {
        let generation = backend
            .read(Box::new(|txn: &mut B::Read| {
                Box::pin(async move { txn.restore_generation().await })
            }))
            .await?;
        let hub = EventHub::default();
        backend.attach(hub.clone()).await?;
        Ok(Self {
            backend,
            policies: PolicyCache::default(),
            chains: policy::ChainCache::default(),
            hub,
            generation,
            retained_bytes: DEFAULT_RETAINED_BYTES,
        })
    }

    /// The backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The event hub that wakes waiting pulls.
    pub fn hub(&self) -> &EventHub {
        &self.hub
    }

    /// The restore generation every response carries.
    pub fn restore_generation(&self) -> u64 {
        self.generation
    }

    /// `GET /v2/limits`.
    pub fn limits(&self) -> LimitsResponse {
        LimitsResponse {
            protocol: sync_protocol::PROTOCOL_VERSION,
            restore_generation: self.generation,
            max_body: MAX_BODY as u32,
            max_inline: sync_protocol::records::MAX_PAYLOAD_LEN as u32,
            max_blob: 2_147_483_648,
            max_covers: MAX_COVERS as u32,
            max_recipients: MAX_RECIPIENTS as u32,
            presence_per_second: 2,
            retained_bytes: self.retained_bytes,
        }
    }

    /// The session a bearer token names, or `unauthorized`.
    pub async fn session(&self, token: &str) -> Result<Session> {
        let bytes: [u8; 32] = sync_protocol::types::from_base64url(token)
            .map_err(|_| refuse("unauthorized", "no valid session"))?;
        let hash = sync_protocol::crypto::hash(&bytes);
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.session(hash).await })
            }))
            .await?
            .ok_or_else(|| refuse("unauthorized", "no valid session"))
    }

    /// Stores a session for `token`, the 32 random bytes a sign-in returns,
    /// as their BLAKE3 hash. Until ticket 46 adds sign-in, tests and tools
    /// create sessions this way.
    pub async fn create_session(&self, token: [u8; 32], session: Session) -> Result<()> {
        let hash = sync_protocol::crypto::hash(&token);
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_session(hash, session).await })
            }))
            .await
    }

    /// Appends verified links to an account's chain, starting a chain with
    /// a genesis when the account has none. Until ticket 46 adds the chain
    /// routes, tests write account chains this way.
    pub async fn append_account_links(&self, links: Vec<Signed<AccountLink>>) -> Result<u64> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { reads::append_account_links(txn, &links).await })
            }))
            .await
    }

    /// `POST /v2/workspaces`.
    pub async fn create_workspace(&self, session: Session, body: Vec<u8>) -> Result<PushResponse> {
        let checked = create::decode(&body)?;
        let chain = self.session_chain(session).await?;
        create::check_signatures(&checked, session, &chain)?;
        let cache = self.policies.clone();
        let digest = sync_protocol::crypto::hash(&body);
        let response = self
            .backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { create::commit(txn, &cache, session, checked, digest).await })
            }))
            .await?;
        let response = PushResponse::from_bytes(&response).map_err(Error::internal)?;
        Ok(response)
    }

    /// `GET /v2/workspaces`.
    pub async fn workspaces(&self, session: Session) -> Result<WorkspacesResponse> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { reads::workspaces(txn, session).await })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/policy?after={revision}`.
    pub async fn policy(
        &self,
        session: Session,
        workspace: Id,
        after: u64,
    ) -> Result<PolicyResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { reads::policy(txn, &cache, session, workspace, after).await })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/keys?after={key_seq}&recipient={device}`.
    pub async fn keys(
        &self,
        session: Session,
        workspace: Id,
        after: u64,
        recipient: Id,
    ) -> Result<KeysResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    reads::keys(txn, &cache, session, workspace, after, recipient).await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/epochs/{epoch}`.
    pub async fn epochs(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> Result<EpochsResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    reads::epochs(txn, &cache, session, workspace, domain, epoch).await
                })
            }))
            .await
    }

    /// `POST /v2/workspaces/{w}/ops`.
    pub async fn push(
        &self,
        session: Session,
        workspace: Id,
        body: Vec<u8>,
    ) -> Result<PushResponse> {
        let ops = push::decode(&body, workspace, session)?;
        let chain = self.session_chain(session).await?;
        push::check_signatures(&ops, session, &chain)?;
        let cache = self.policies.clone();
        let generation = self.generation;
        let group = push::group(workspace, &ops);
        let job: WriteJob<B::Write, PushResponse> = Box::new(move |txn: &mut B::Write| {
            Box::pin(
                async move { push::commit(txn, &cache, generation, session, workspace, ops).await },
            )
        });
        let response = match group {
            Some(group) => self.backend.write_grouped(group, job).await?,
            None => self.backend.write(job).await?,
        };
        if !response.receipts.is_empty() {
            self.hub.notify(workspace);
            let mut heads: BTreeMap<Id, u64> = BTreeMap::new();
            for receipt in response.receipts.iter() {
                let head = heads.entry(receipt.domain).or_default();
                *head = (*head).max(receipt.seq);
            }
            self.backend
                .announce(workspace, heads.into_iter().collect());
        }
        Ok(response)
    }

    /// `POST /v2/workspaces/{w}/pull`: one page per listed domain, waiting
    /// up to the request's wait while no domain has records after its
    /// cursor. Each attempt reads one snapshot, and no transaction stays
    /// open while the pull waits.
    pub async fn pull(
        &self,
        session: Session,
        workspace: Id,
        request: PullRequest,
    ) -> Result<PullResponse> {
        // One cursor per domain a workspace can hold keeps the pages within
        // 1 MiB.
        if request.cursors.len() > sync_protocol::policy::MAX_DOMAINS {
            return Err(refuse("too_large", "a pull lists at most 256 domains"));
        }
        let wait = Duration::from_secs(u64::from(request.wait_seconds.min(MAX_WAIT_SECONDS)));
        let deadline = tokio::time::Instant::now() + wait;
        let mut changes = self.hub.subscribe(workspace);
        let cursors = request.cursors.0;
        loop {
            let cache = self.policies.clone();
            let generation = self.generation;
            let cursors = cursors.clone();
            let pages = self
                .backend
                .read(Box::new(move |txn: &mut B::Read| {
                    Box::pin(async move {
                        pull::snapshot(txn, &cache, generation, session, workspace, &cursors).await
                    })
                }))
                .await?;
            let has_records = pages.iter().any(|page| !page.entries.is_empty());
            if has_records || tokio::time::Instant::now() >= deadline {
                return Ok(PullResponse {
                    pages: sync_protocol::codec::List(pages),
                });
            }
            tokio::select! {
                _ = changes.changed() => {}
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}`.
    pub async fn chain_record(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> Result<sync_protocol::messages::Entry> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    reads::chain_record(txn, &cache, session, workspace, domain, device, counter)
                        .await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/heads/{device}`.
    pub async fn chain_head(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        device: Id,
    ) -> Result<HeadResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    reads::chain_head(txn, &cache, session, workspace, domain, device).await
                })
            }))
            .await
    }

    /// Records an operation's live-set fields in `domain` without checking
    /// them, classing it as the server does, as `LiveSet::apply` does. The
    /// push path checks first; the conformance suite replays fixtures with
    /// this.
    pub async fn record_live(&self, workspace: Id, domain: Id, op: LiveOp) -> Result<OpClass> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { live::record(txn, workspace, domain, &op).await })
            }))
            .await
    }

    /// The live-set checks of §13 for `op`, as the push path runs them.
    pub async fn check_live(
        &self,
        workspace: Id,
        op: LiveOp,
    ) -> Result<std::result::Result<OpClass, LiveError>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { live::check(txn, workspace, &op).await })
            }))
            .await
    }

    /// The operations live on `rid` at sequence `at`.
    pub async fn live_at(&self, workspace: Id, rid: Id, at: u64) -> Result<Vec<(LiveOp, OpClass)>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.live_ops(workspace, rid, at).await })
            }))
            .await
    }

    /// Applies a checkpoint's rising floor (§17) and returns the lone
    /// deletes it drops for good.
    pub async fn apply_floor(
        &self,
        workspace: Id,
        domain: Id,
        previous_floor: u64,
        floor: u64,
        covered_seq: u64,
    ) -> Result<Vec<DeleteRow>> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    live::apply_floor(txn, workspace, domain, previous_floor, floor, covered_seq)
                        .await
                })
            }))
            .await
    }

    /// The session account's current chain, which lists the session's card.
    async fn session_chain(&self, session: Session) -> Result<Arc<AccountChain>> {
        let chains = self.chains.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { reads::session_chain(txn, &chains, session).await })
            }))
            .await
    }
}
