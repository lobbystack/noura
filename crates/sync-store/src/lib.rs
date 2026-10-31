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

mod accounts;
mod blobs;
mod chains;
mod checkpoints;
mod create;
pub mod devices;
mod hub;
mod invitations;
pub mod limiter;
mod live;
mod policy;
mod pull;
mod push;
mod reads;
mod realtime;
mod reset;
pub mod restore;
mod retention;
mod shares;

#[cfg(feature = "conformance")]
pub mod conformance;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Bytes, Decode, Text};
use sync_protocol::live::{LiveError, LiveOp};
use sync_protocol::messages::{
    ChainAppendResponse, ChainsRequest, ChainsResponse, ChallengeResponse, EpochsResponse,
    ErrorBody, HeadResponse, KeysResponse, LimitsResponse, MAX_BODY, MAX_WAIT_SECONDS,
    PolicyResponse, PullRequest, PullResponse, PushResponse, WorkspacesResponse,
    server_error_status,
};
use sync_protocol::records::{
    AccountHead, AccountLink, Card, MAX_COVERS, MAX_RECIPIENTS, OpClass, RecoveryProof, Ref, Signed,
};

pub use accounts::{SIGN_IN_TOKEN_LEN, SignInToken};
pub use blobs::{BlobCreation, BlobUpload, Recorded};
pub use chains::SIGN_IN_TTL;
pub use invitations::{INVITATION_TTL, InvitationCreation, MAX_INVITATIONS};
pub use shares::{MAX_SHARE_DAYS, MAX_SHARES, ShareCreation};
use sync_protocol::types::{Digest, Id, SecretKey};

pub use hub::{EventHub, Relayed};
pub use limiter::{Limit, Limiter, MemoryLimiter, Taken};
pub use policy::PolicyCache;
pub use realtime::Watch;
pub use retention::{BlobToCollect, Maintenance, Retention, UPLOAD_TTL};

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
    /// When to retry, for `rate_limited`.
    pub retry_after_seconds: Option<u64>,
}

impl Refusal {
    /// A refusal with no detail.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            retry_after_seconds: None,
        }
    }

    /// This refusal, telling the client to retry after `seconds`.
    pub fn retry_after(mut self, seconds: u64) -> Self {
        self.retry_after_seconds = Some(seconds);
        self
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
            retry_after_seconds: self.retry_after_seconds,
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

/// `device_removed`, whose detail names the link that removed the
/// session's card (§7, §26).
pub(crate) fn device_removed(link: RemovedBy) -> Error {
    use sync_protocol::codec::Encode as _;
    Error::Refused(Refusal {
        code: "device_removed",
        message: "a link of the account chain removed this device".into(),
        retry_after_seconds: None,
        detail: Some(
            sync_protocol::messages::DeviceRemovedDetail {
                genesis: link.genesis,
                seqno: link.seqno,
            }
            .to_bytes(),
        ),
    })
}

/// A signed-in device: the account and device a session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    /// The account.
    pub account: Id,
    /// The session's device.
    pub device: Id,
    /// The recovery identity this session proved it holds (§12), if any.
    pub recovery: Option<Id>,
}

impl Session {
    /// A device's session that proved no recovery identity.
    pub fn new(account: Id, device: Id) -> Self {
        Self {
            account,
            device,
            recovery: None,
        }
    }
}

/// A passkey ceremony's kind (§26 account pages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeremonyKind {
    /// Registering a passkey to sign up with an email.
    SignUp,
    /// Registering another passkey for a signed-in account.
    AddPasskey,
    /// Signing in with a passkey.
    SignIn,
}

impl CeremonyKind {
    /// The kind's stored code.
    pub fn code(self) -> u8 {
        match self {
            CeremonyKind::SignUp => 1,
            CeremonyKind::AddPasskey => 2,
            CeremonyKind::SignIn => 3,
        }
    }
}

/// A passkey ceremony's state between its two requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CeremonyRow {
    /// The ceremony's random ID.
    pub id: Id,
    /// Its kind.
    pub kind: CeremonyKind,
    /// The account it serves, once known.
    pub account: Option<Id>,
    /// The email a sign-up names.
    pub email: Option<String>,
    /// The webauthn-rs state, serialized.
    pub state: Vec<u8>,
    /// When it expires, in Unix seconds.
    pub expires_at: u64,
}

/// A passkey registered at sign-up, waiting for its email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPasskeyRow {
    /// The email the sign-up names.
    pub email: String,
    /// The credential ID.
    pub credential_id: Vec<u8>,
    /// The passkey, serialized.
    pub passkey: Vec<u8>,
    /// When it expires, in Unix seconds.
    pub expires_at: u64,
}

/// A stored passkey.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasskeyRow {
    /// The credential ID.
    pub credential_id: Vec<u8>,
    /// The account it signs in to.
    pub account: Id,
    /// The passkey, serialized.
    pub passkey: Vec<u8>,
}

/// A stored approval request (§10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequestRow {
    /// The request's commitment, which identifies it.
    pub commit: Digest,
    /// The account that posted it.
    pub account: Id,
    /// The card that posted it.
    pub device: Id,
    /// That card's encoding.
    pub card: Vec<u8>,
    /// The candidate's account head, for an invitation or a reset.
    pub candidate_head: Option<AccountHead>,
    /// The invitation it names, if any.
    pub invitation: Option<Id>,
    /// The invitation proof.
    pub invite_proof: Option<Digest>,
    /// The workspace that invitation invites to, or that a reset asks to be
    /// accepted in.
    pub workspace: Option<Id>,
    /// The encoded request, so a retry with identical bytes gets the
    /// original result.
    pub request: Vec<u8>,
    /// When it expires, in Unix seconds.
    pub expires_at: u64,
}

/// The link that removed a session's card: the `device_removed` detail
/// (§26).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemovedBy {
    /// The chain's genesis digest.
    pub genesis: Digest,
    /// The removing link's seqno.
    pub seqno: u64,
}

/// A stored session: its account and device, and the link that removed its
/// card, if one did (§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRow {
    /// The session.
    pub session: Session,
    /// The link that removed the session's card.
    pub removed: Option<RemovedBy>,
}

/// A device authorization grant (RFC 8628, §7) until it's used or expires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceGrantRow {
    /// `BLAKE3` of the device code's text.
    pub device_code_hash: Digest,
    /// `BLAKE3` of the user code's 8 characters.
    pub user_code_hash: Digest,
    /// The account that approved it on the device page.
    pub account: Option<Id>,
    /// Whether the person refused it.
    pub denied: bool,
    /// The polling interval in seconds.
    pub interval: u64,
    /// The earliest Unix second the next poll may arrive.
    pub next_poll: u64,
    /// When it expires, in Unix seconds.
    pub expires_at: u64,
}

/// A stored approval offer (§10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferRow {
    /// The offer's digest.
    pub offer_digest: Digest,
    /// The commitment it answers.
    pub commit: Digest,
    /// `BLAKE3` of the candidate's card.
    pub candidate_card_digest: Digest,
    /// The approving device.
    pub approver: Id,
    /// The approving device's account.
    pub account: Id,
    /// The signed offer.
    pub offer: Vec<u8>,
    /// The candidate's nonce, once it revealed it.
    pub nc: Option<sync_protocol::types::Key>,
    /// When it expires, in Unix seconds.
    pub expires_at: u64,
}

/// An open re-wrap request (§9): a device asks for a new envelope of one
/// epoch of a domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RewrapRow {
    /// The domain.
    pub domain: Id,
    /// The epoch.
    pub epoch: u64,
    /// The requesting device.
    pub device: Id,
    /// The key sequence of the request's key stream row.
    pub key_seq: u64,
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
    /// Whether usage and history together stand above the quota (§20).
    pub quota_pressure: bool,
    /// How many times they crossed it.
    pub quota_crossings: u64,
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
    /// The latest checkpoint's `covered_seq`, 0 before the first.
    pub checkpoint_seq: u64,
    /// The due point the server publishes (§17), or 0.
    pub checkpoint_due: u64,
    /// Entries after the latest checkpoint's entry.
    pub entries_since_checkpoint: u64,
    /// Their weight (§19).
    pub weight_since_checkpoint: u64,
    /// Whether `delete_domain` deleted the domain, or a closed workspace
    /// took it along; the server reclaims it in batches (§19).
    pub deleted: bool,
    /// The running weight of every entry the domain's log ever took.
    pub log_weight: u64,
    /// The domain's share of the workspace's usage (§20).
    pub usage_bytes: u64,
    /// Its history weight (§20).
    pub history_bytes: u64,
    /// The floor through which pruning finished.
    pub pruned_floor: u64,
    /// The workspace's last quota crossing this domain handled.
    pub crossing_seen: u64,
    /// The workspace's count of quota crossings.
    pub quota_crossings: u64,
}

/// What each log entry changes on a domain's head row (§17, §19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointProgress {
    /// Entries after the latest checkpoint's entry.
    pub entries: u64,
    /// Their weight.
    pub weight: u64,
    /// The due point, or 0.
    pub due: u64,
    /// The running weight of the log.
    pub log_weight: u64,
    /// The last quota crossing the domain handled.
    pub crossing_seen: u64,
}

/// A domain's numbers that the pressure rule of §20 reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomainHistory {
    /// The domain.
    pub domain: Id,
    /// Its history weight.
    pub history_bytes: u64,
    /// Its retention floor.
    pub retention_floor: u64,
    /// Its latest checkpoint's `covered_seq`.
    pub checkpoint_seq: u64,
}

/// A domain with pruning or reclaiming to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingDomain {
    /// The workspace.
    pub workspace: Id,
    /// The domain.
    pub domain: Id,
    /// Whether it waits to be reclaimed.
    pub deleted: bool,
}

/// What one pruning scan may delete (§19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneScan {
    /// The sequence the scan continues after.
    pub after: u64,
    /// The retention floor.
    pub floor: u64,
    /// The highest sequence of a dropped lone delete that may go: the floor
    /// or the lowest `floor` field among servable checkpoints.
    pub delete_floor: u64,
    /// The newest epoch entry at or below the floor, which stays.
    pub keep_epoch: u64,
    /// The newest policy entry at or below the floor, which stays.
    pub keep_policy: u64,
}

/// A log entry pruning deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrunedEntry {
    /// Its sequence.
    pub seq: u64,
    /// Its record type.
    pub record_type: u8,
    /// Its record's length.
    pub length: u64,
    /// Its `rid`, for an operation.
    pub rid: Option<Id>,
}

/// A checkpoint as stored (§17, §19): its record, which the server keeps
/// while the domain exists, and the heads it lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointRow {
    /// Its `number`.
    pub number: u64,
    /// The signed record's digest.
    pub digest: Digest,
    /// The sequence of its log entry.
    pub seq: u64,
    /// Its `covered_seq`.
    pub covered_seq: u64,
    /// Its `floor`.
    pub floor: u64,
    /// The signed checkpoint record.
    pub record: Vec<u8>,
    /// Its heads, as an encoded `list<ref>`; none once the floor passes it.
    pub heads: Vec<u8>,
    /// The log's running weight through its entry.
    pub log_weight: u64,
}

/// When a checkpoint falls due by volume (§17): at the entry that brings
/// a domain's log to `entries` entries or `weight` bytes of weight since
/// its latest checkpoint entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointInterval {
    /// Entries since the latest checkpoint entry.
    pub entries: u64,
    /// Weight since the latest checkpoint entry.
    pub weight: u64,
}

impl Default for CheckpointInterval {
    fn default() -> Self {
        Self {
            entries: 10_000,
            weight: 64 << 20,
        }
    }
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
    /// The sequence of its entry in the domain's log.
    pub seq: u64,
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

/// A blob's row (§21, §29): what its creation named and how far its upload
/// reached. It holds no unpadded size, and a time only while the upload is
/// incomplete or unlisted; the blob store's file times date the upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobRow {
    /// The blob ID, unique within its workspace.
    pub blob: Id,
    /// The domain it was created for; only that domain's operations list it.
    pub domain: Id,
    /// The device that created the upload.
    pub device: Id,
    /// The padded ciphertext length, `Upload-Length`.
    pub length: u64,
    /// `ciphertext_digest`, as creation named it.
    pub digest: Digest,
    /// The bytes stored so far, `Upload-Offset`.
    pub offset: u64,
    /// Whether the upload completed and its digest matched.
    pub complete: bool,
    /// When the collector may take it, while no retained operation lists it.
    pub delete_after: Option<u64>,
    /// Whether the collector marked it: no push lists it from then on.
    pub marked: bool,
    /// How many operations live at the domain's head list it.
    pub live_listings: u64,
}

/// A share link's row (§24): what the server stores of it, which is the
/// token's hash, the workspace, the domain, the creating device, the expiry
/// and the signed padded snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareRow {
    /// `BLAKE3(tag(noura.sync.share.token) || token)`.
    pub token_hash: Digest,
    /// The workspace.
    pub workspace: Id,
    /// The domain the creating device named.
    pub domain: Id,
    /// The device that created the link, the only one that updates it.
    pub device: Id,
    /// Unix seconds after which the link no longer opens.
    pub expires_at: u64,
    /// The signed `share` record's bytes.
    pub share: Vec<u8>,
}

/// An open invitation's row (§11): the token's hash, the workspace, the
/// role, the inviting account, the accepting account once known, the expiry
/// and the encrypted context. Its status is pending until an account
/// accepts it, then accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitationRow {
    /// The invitation ID the inviting device chose.
    pub invitation: Id,
    /// `BLAKE3(tag(noura.sync.invite.token) || token)`.
    pub token_hash: Digest,
    /// The workspace.
    pub workspace: Id,
    /// The role it grants.
    pub role: sync_protocol::records::Role,
    /// The inviting account.
    pub inviter: Id,
    /// The account that accepted it.
    pub accepted_by: Option<Id>,
    /// Unix seconds after which the link no longer opens.
    pub expires_at: u64,
    /// The encoded `invite_context`.
    pub invite_context: Vec<u8>,
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
/// workspace that take only the shared policy lock and that domain's head
/// row (§15).
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
    /// The sum of the workspace's domains' usage and history (§15, §20),
    /// leaving out domains that wait to be reclaimed.
    fn workspace_usage(&mut self, workspace: Id)
    -> impl Future<Output = Result<(u64, u64)>> + Send;
    /// The history of the workspace's domains, most first.
    fn domain_histories(
        &mut self,
        workspace: Id,
    ) -> impl Future<Output = Result<Vec<DomainHistory>>> + Send;
    /// Domains with pruning or reclaiming to do, under `retained_bytes`.
    fn pending_domains(
        &mut self,
        retained_bytes: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<PendingDomain>>> + Send;
    /// Workspaces whose usage and history stand above the quota.
    fn pressured_workspaces(&mut self, limit: u32) -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// The `covered_seq` of the newest checkpoint whose entry ends at or
    /// below `log_weight` of running weight.
    fn checkpoint_at_weight(
        &mut self,
        workspace: Id,
        domain: Id,
        log_weight: u64,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// The lowest `covered_seq` of a checkpoint above `covered_seq`.
    fn checkpoint_above(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// The `floor` field of the oldest checkpoint at or above `covered_seq`.
    fn servable_floor(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// Up to `limit` log entries pruning may delete, in ascending order.
    fn prunable_entries(
        &mut self,
        workspace: Id,
        domain: Id,
        scan: &PruneScan,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<PrunedEntry>>> + Send;
    /// The blobs the operation at `seq` lists.
    fn listed_blobs(
        &mut self,
        workspace: Id,
        domain: Id,
        seq: u64,
    ) -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// The length of the record at `seq`.
    fn entry_length(
        &mut self,
        workspace: Id,
        domain: Id,
        seq: u64,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// The length of an epoch's stored bundle.
    fn epoch_bundle_length(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// Whether any operation names the `rid`.
    fn rid_in_use(&mut self, workspace: Id, rid: Id) -> impl Future<Output = Result<bool>> + Send;
    /// The blobs due for the collector at `now`: marked rows first, then
    /// those whose time has come.
    fn collectable_blobs(
        &mut self,
        now: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<retention::BlobToCollect>>> + Send;
    /// Whether the domain still holds blob rows.
    fn domain_has_blobs(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Share links expired at `now`, as token hash, workspace and domain.
    fn expired_shares(
        &mut self,
        now: u64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(Digest, Id, Id)>>> + Send;
    /// The server's restore generation.
    fn restore_generation(&mut self) -> impl Future<Output = Result<u64>> + Send;
    /// The session whose token hashes to `token_hash`.
    fn session(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<SessionRow>>> + Send;
    /// The device grant whose device code hashes to `code_hash`.
    fn device_grant(
        &mut self,
        code_hash: Digest,
    ) -> impl Future<Output = Result<Option<DeviceGrantRow>>> + Send;
    /// The device grant whose user code hashes to `user_code_hash`.
    fn device_grant_by_user_code(
        &mut self,
        user_code_hash: Digest,
    ) -> impl Future<Output = Result<Option<DeviceGrantRow>>> + Send;
    /// The account's enrolled cards, by device ID.
    fn enrolled_cards(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Vec<(Id, Vec<u8>)>>> + Send;
    /// How many sessions that no link removed each device of the account
    /// holds, by device ID; a session whose device hasn't enrolled counts
    /// under the zero ID.
    fn session_counts(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Vec<(Id, u64)>>> + Send;
    /// The account's stored approval requests, by commitment.
    fn approval_requests(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Vec<ApprovalRequestRow>>> + Send;
    /// The approval request with this commitment.
    fn approval_request(
        &mut self,
        commit: Digest,
    ) -> impl Future<Output = Result<Option<ApprovalRequestRow>>> + Send;
    /// The approval offer with this digest.
    fn approval_offer(
        &mut self,
        offer_digest: Digest,
    ) -> impl Future<Output = Result<Option<OfferRow>>> + Send;
    /// The approval offers that name this candidate card digest, by digest.
    fn approval_offers_for(
        &mut self,
        card_digest: Digest,
    ) -> impl Future<Output = Result<Vec<OfferRow>>> + Send;
    /// The open re-wrap requests of a workspace, by domain, epoch and
    /// device.
    fn rewrap_requests(
        &mut self,
        workspace: Id,
    ) -> impl Future<Output = Result<Vec<RewrapRow>>> + Send;
    /// The devices that answered one re-wrap request, with the key sequence
    /// of each answer.
    fn rewrap_answers(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
        device: Id,
    ) -> impl Future<Output = Result<Vec<(Id, u64)>>> + Send;
    /// The devices that signed operations with a sequence in (`after`,
    /// `before`) of a domain's log.
    fn writers_between(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        before: u64,
    ) -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// Every profile record of a workspace, by profile revision.
    fn profiles(&mut self, workspace: Id) -> impl Future<Output = Result<Vec<Vec<u8>>>> + Send;
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
    /// A page of every blob row, in workspace and blob order, after
    /// `after`, for the check a restore runs against the blob store (§19).
    fn blob_rows(
        &mut self,
        after: Option<(Id, Id)>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(Id, BlobRow)>>> + Send;
    /// The blobs a retained operation lists that have no complete upload,
    /// which a restore left missing (§19).
    fn missing_blobs(&mut self, workspace: Id) -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// The account's reset code's hash and when it expires.
    fn reset_code(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Option<(Digest, u64)>>> + Send;
    /// The instances whose leases last past `now`.
    fn live_instances(&mut self, now: u64) -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// A workspace's row, unless its owner closed it.
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
    /// Whether the workspace's row stands closed. [`Self::workspace`] hides
    /// such a row.
    fn workspace_closed(&mut self, workspace: Id) -> impl Future<Output = Result<bool>> + Send;
    /// Closed workspaces with no domain left whose policy rows still stand.
    fn drained_closed_workspaces(
        &mut self,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Id>>> + Send;
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
    /// An account's history floor in a domain (§9), 0 when it has none.
    fn history_floor(
        &mut self,
        workspace: Id,
        account: Id,
        domain: Id,
    ) -> impl Future<Output = Result<u64>> + Send;
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
    /// The device's highest record below `counter`, as sequence and digest.
    fn position_below(
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
    /// The domain's latest checkpoint.
    fn latest_checkpoint(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<Option<CheckpointRow>>> + Send;
    /// The domain's checkpoint numbered `number`.
    fn checkpoint_by_number(
        &mut self,
        workspace: Id,
        domain: Id,
        number: u64,
    ) -> impl Future<Output = Result<Option<CheckpointRow>>> + Send;
    /// The domain's checkpoint with this digest.
    fn checkpoint_by_digest(
        &mut self,
        workspace: Id,
        domain: Id,
        digest: Digest,
    ) -> impl Future<Output = Result<Option<CheckpointRow>>> + Send;
    /// The domain's checkpoint with this `covered_seq`.
    fn checkpoint_at(
        &mut self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
    ) -> impl Future<Output = Result<Option<CheckpointRow>>> + Send;
    /// The 4,096 bucket hashes stored for checkpoint `number`, concatenated.
    fn checkpoint_buckets(
        &mut self,
        workspace: Id,
        domain: Id,
        number: u64,
    ) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send;
    /// The latest log entry of `record_type` at or below sequence `at`.
    fn latest_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        record_type: u8,
        at: u64,
    ) -> impl Future<Output = Result<Option<RawEntry>>> + Send;
    /// The operations of a domain with a sequence in (`after`, `through`],
    /// each with its `rid`, by sequence.
    fn ops_between(
        &mut self,
        workspace: Id,
        domain: Id,
        after: u64,
        through: u64,
    ) -> impl Future<Output = Result<Vec<(Id, Ref)>>> + Send;
    /// At most `limit` `rid`s of a domain in [`from`, `through`], sorted.
    fn domain_rids(
        &mut self,
        workspace: Id,
        domain: Id,
        from: Id,
        through: Id,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// The devices with a chain head in a domain.
    fn chain_head_devices(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<Vec<Id>>> + Send;
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
    /// The account and card bytes of an enrolled card.
    fn enrolled_card(
        &mut self,
        device: Id,
    ) -> impl Future<Output = Result<Option<(Id, Vec<u8>)>>> + Send;
    /// The workspaces whose policy records a head of the account.
    fn workspaces_recording(&mut self, account: Id)
    -> impl Future<Output = Result<Vec<Id>>> + Send;
    /// The email a signed-up account names.
    fn account_email(&mut self, account: Id)
    -> impl Future<Output = Result<Option<String>>> + Send;
    /// The workspaces that the account's stored approval requests name
    /// through an invitation, each with the request's expiry.
    fn invitation_requests(
        &mut self,
        account: Id,
    ) -> impl Future<Output = Result<Vec<(Id, u64)>>> + Send;
    /// The account's passkeys, by credential ID.
    fn passkeys(&mut self, account: Id) -> impl Future<Output = Result<Vec<PasskeyRow>>> + Send;
    /// The passkey with this credential ID.
    fn passkey(
        &mut self,
        credential_id: &[u8],
    ) -> impl Future<Output = Result<Option<PasskeyRow>>> + Send;
    /// The account of the web session whose token hashes to `token_hash`.
    fn web_session(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<Id>>> + Send;
    /// The blob's row.
    fn blob(
        &mut self,
        workspace: Id,
        blob: Id,
    ) -> impl Future<Output = Result<Option<BlobRow>>> + Send;
    /// The hashes of the blob's stored segments, in offset order.
    fn blob_segments(
        &mut self,
        workspace: Id,
        blob: Id,
    ) -> impl Future<Output = Result<Vec<Digest>>> + Send;
    /// Whether an operation lists the blob.
    fn blob_listed(&mut self, workspace: Id, blob: Id)
    -> impl Future<Output = Result<bool>> + Send;
    /// The sequences of the operations that list the blob.
    fn blob_listing_seqs(
        &mut self,
        workspace: Id,
        blob: Id,
    ) -> impl Future<Output = Result<Vec<u64>>> + Send;
    /// The invitation with this ID, expired or not.
    fn invitation(
        &mut self,
        invitation: Id,
    ) -> impl Future<Output = Result<Option<InvitationRow>>> + Send;
    /// Whether an invitation with this ID was approved or revoked before
    /// the expiry it had.
    fn invitation_ended(&mut self, invitation: Id) -> impl Future<Output = Result<bool>> + Send;
    /// The invitation whose token hashes to `token_hash`, expired or not.
    fn invitation_by_token(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<InvitationRow>>> + Send;
    /// The workspace's invitations that expire after `now`, by invitation
    /// ID.
    fn invitations(
        &mut self,
        workspace: Id,
        now: u64,
    ) -> impl Future<Output = Result<Vec<InvitationRow>>> + Send;
    /// The stored approval requests that name `workspace`, through an
    /// invitation or a reset, by commitment.
    fn workspace_approval_requests(
        &mut self,
        workspace: Id,
    ) -> impl Future<Output = Result<Vec<ApprovalRequestRow>>> + Send;
    /// The share link whose token hashes to `token_hash`, expired or not.
    fn share(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<ShareRow>>> + Send;
    /// The workspace's share links that expire after `now`, by token hash,
    /// with empty snapshots.
    fn shares(
        &mut self,
        workspace: Id,
        now: u64,
    ) -> impl Future<Output = Result<Vec<ShareRow>>> + Send;
}

/// The writes a store's write transaction offers, besides its reads.
pub trait WriteTxn: ReadTxn {
    /// Adds to the domain's usage and history, either of which may fall.
    fn add_usage(
        &mut self,
        workspace: Id,
        domain: Id,
        usage: i64,
        history: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Persists the highest retention floor published (§19).
    fn set_retention_floor(
        &mut self,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Records the floor through which pruning finished.
    fn set_pruned_floor(
        &mut self,
        workspace: Id,
        domain: Id,
        floor: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Marks the domain deleted, in the transaction of its `delete_domain`.
    fn mark_domain_deleted(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a log entry, with the operation rows of an operation.
    fn delete_log_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        seq: u64,
        record_type: u8,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a `rid` no operation names any more.
    fn delete_rid(&mut self, workspace: Id, rid: Id) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a blob listing of a pruned operation.
    fn delete_blob_listing(
        &mut self,
        workspace: Id,
        blob: Id,
        domain: Id,
        seq: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Sets or clears when the collector may take the blob.
    fn set_blob_delete_after(
        &mut self,
        workspace: Id,
        blob: Id,
        delete_after: Option<u64>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Adds to the count of operations live at the head that list the blob.
    fn add_blob_live(
        &mut self,
        workspace: Id,
        blob: Id,
        delta: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Marks the blob deleted, the collector's first step.
    fn mark_blob(&mut self, workspace: Id, blob: Id) -> impl Future<Output = Result<()>> + Send;
    /// Drops the heads and bucket hashes of up to `limit` checkpoints below
    /// `floor`, and returns how many it changed. The records stay.
    fn clear_checkpoint_data(
        &mut self,
        workspace: Id,
        domain: Id,
        floor: u64,
        empty_heads: &[u8],
        limit: u32,
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Records that usage and history stand above or within the quota.
    /// Going above counts a crossing. Returns whether the state changed, so
    /// two racing pushes count one crossing.
    fn set_quota_pressure(
        &mut self,
        workspace: Id,
        above: bool,
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Deletes up to `limit` rows from each table of a deleted domain's log,
    /// checkpoints, keys and links, and returns how many it deleted. Blob
    /// rows stay for the collector.
    fn reclaim_rows(
        &mut self,
        workspace: Id,
        domain: Id,
        limit: u32,
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Sets up to `limit` of the domain's waiting blobs to expire at `now`,
    /// and returns how many it changed.
    fn expire_domain_blobs(
        &mut self,
        workspace: Id,
        domain: Id,
        now: u64,
        limit: u32,
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Deletes a reclaimed domain's head row.
    fn delete_domain_row(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> impl Future<Output = Result<()>> + Send;
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
    /// Makes `genesis` the current chain of an account that has none.
    /// Returns false, and changes nothing, when the account has a chain,
    /// such as one a racing genesis named first.
    fn start_chain(
        &mut self,
        account: Id,
        genesis: Digest,
    ) -> impl Future<Output = Result<bool>> + Send;
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
    /// Sets an account's history floor in a domain (§9).
    fn set_history_floor(
        &mut self,
        workspace: Id,
        account: Id,
        domain: Id,
        floor: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Forgets a member account that `remove_member` took out.
    fn delete_member(
        &mut self,
        workspace: Id,
        account: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a policy change. The workspace row names the head. The store
    /// calls it only through `policy::store_change`, which also records the
    /// change's accounts with [`WriteTxn::record_accounts`], or the chain
    /// read route stops serving their chains (§7).
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
    /// Stores an accepted checkpoint with its bucket hashes, concatenated,
    /// and makes it the domain's latest.
    fn insert_checkpoint(
        &mut self,
        workspace: Id,
        domain: Id,
        row: &CheckpointRow,
        buckets: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Sets the domain's entries and weight since its latest checkpoint
    /// entry, its due point, its log's running weight and the last quota
    /// crossing it handled.
    fn set_checkpoint_progress(
        &mut self,
        workspace: Id,
        domain: Id,
        progress: &CheckpointProgress,
    ) -> impl Future<Output = Result<()>> + Send;
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
    /// Appends a key stream row of one epoch of a domain at the workspace's
    /// next key sequence, under its key sequence row, and returns that
    /// sequence.
    fn append_key_entry(
        &mut self,
        workspace: Id,
        record_type: u8,
        domain: Id,
        epoch: u64,
        recipient: Option<Id>,
        record: &[u8],
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Deletes a domain's envelopes and re-wrap requests of epochs below
    /// `epoch`, with their key stream rows, once epoch `epoch` committed
    /// (§9).
    fn drop_old_epochs(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a re-wrap request.
    fn insert_rewrap_request(
        &mut self,
        workspace: Id,
        row: &RewrapRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Records a lone envelope that answered a re-wrap request.
    fn insert_rewrap_answer(
        &mut self,
        workspace: Id,
        request: &RewrapRow,
        wrapper: Id,
        key_seq: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Closes a re-wrap request: deletes it, its answers and its key stream
    /// row.
    fn close_rewrap(
        &mut self,
        workspace: Id,
        request: &RewrapRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a device grant.
    fn insert_device_grant(
        &mut self,
        row: &DeviceGrantRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Replaces a device grant's state.
    fn locked_device_grant(
        &mut self,
        code_hash: Digest,
    ) -> impl Future<Output = Result<Option<DeviceGrantRow>>> + Send;
    /// [`ReadTxn::device_grant_by_user_code`], and it holds the row against
    /// other writers until the transaction ends.
    fn locked_device_grant_by_user_code(
        &mut self,
        user_code_hash: Digest,
    ) -> impl Future<Output = Result<Option<DeviceGrantRow>>> + Send;
    /// Stores a grant's new state.
    fn update_device_grant(
        &mut self,
        row: &DeviceGrantRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a device grant.
    fn delete_device_grant(&mut self, code_hash: Digest)
    -> impl Future<Output = Result<()>> + Send;
    /// Binds a session to the card its device enrolled.
    fn bind_session(
        &mut self,
        token_hash: Digest,
        device: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Marks every session of the account's `devices` removed by `link`.
    fn remove_sessions(
        &mut self,
        account: Id,
        devices: &[Id],
        link: RemovedBy,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Marks every session that proved one of `cards` removed by `link`,
    /// except those of `keep`, which only lose the proof: a reset retires
    /// the identities those sessions read as (§7).
    fn retire_proven_sessions(
        &mut self,
        account: Id,
        cards: &[Id],
        keep: Id,
        link: RemovedBy,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes every session of one device of the account and returns how
    /// many it deleted.
    fn delete_device_sessions(
        &mut self,
        account: Id,
        device: Id,
    ) -> impl Future<Output = Result<u64>> + Send;
    /// Stores an approval offer.
    fn insert_approval_offer(&mut self, row: &OfferRow) -> impl Future<Output = Result<()>> + Send;
    /// Records the nonce the candidate revealed for an offer.
    fn set_reveal(
        &mut self,
        offer_digest: Digest,
        nc: sync_protocol::types::Key,
    ) -> impl Future<Output = Result<()>> + Send;
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
    /// Records the recovery identity the session proved it holds.
    fn set_session_recovery(
        &mut self,
        token_hash: Digest,
        recovery: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores an enrolled card; returns false when the device ID is
    /// taken, such as by an enrollment that committed meanwhile.
    fn insert_enrolled_card(
        &mut self,
        device: Id,
        account: Id,
        card: &[u8],
    ) -> impl Future<Output = Result<bool>> + Send;
    /// Records that the workspace's policy records heads of these accounts.
    fn record_accounts(
        &mut self,
        workspace: Id,
        accounts: &[Id],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores an enrollment challenge by its BLAKE3 hash.
    fn insert_challenge(
        &mut self,
        challenge_hash: Digest,
        account: Id,
        expires_at: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes the account's challenge with this hash and returns its
    /// expiry, if it was stored.
    fn take_challenge(
        &mut self,
        challenge_hash: Digest,
        account: Id,
    ) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// Stores an approval request.
    fn insert_approval_request(
        &mut self,
        row: &ApprovalRequestRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a signed-up account, unless it exists.
    fn insert_account(
        &mut self,
        account: Id,
        email: &str,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a passkey; returns false when its credential ID is taken.
    fn insert_passkey(&mut self, row: &PasskeyRow) -> impl Future<Output = Result<bool>> + Send;
    /// Replaces a passkey's stored form, such as after its counter moved.
    fn update_passkey(
        &mut self,
        credential_id: &[u8],
        passkey: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a web session.
    fn insert_web_session(
        &mut self,
        token_hash: Digest,
        account: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a web session.
    fn delete_web_session(&mut self, token_hash: Digest)
    -> impl Future<Output = Result<()>> + Send;
    /// Stores a magic link.
    fn insert_magic_link(
        &mut self,
        token_hash: Digest,
        email: &str,
        expires_at: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a magic link and returns its email and expiry, if stored.
    fn take_magic_link(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<(String, u64)>>> + Send;
    /// Stores a ceremony's state.
    fn insert_ceremony(&mut self, row: &CeremonyRow) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a ceremony and returns it, if stored.
    fn take_ceremony(&mut self, id: Id)
    -> impl Future<Output = Result<Option<CeremonyRow>>> + Send;
    /// Stores a passkey that waits for its email.
    fn insert_pending_passkey(
        &mut self,
        token_hash: Digest,
        row: &PendingPasskeyRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a waiting passkey and returns it, if stored.
    fn take_pending_passkey(
        &mut self,
        token_hash: Digest,
    ) -> impl Future<Output = Result<Option<PendingPasskeyRow>>> + Send;
    /// Deletes every magic link, waiting passkey, ceremony, challenge,
    /// device grant, approval request, approval offer, invitation and share
    /// link that expires at or before `now`, and returns how many it
    /// deleted.
    fn delete_expired(&mut self, now: u64) -> impl Future<Output = Result<u64>> + Send;
    /// Stores a new blob row at offset 0.
    fn insert_blob(
        &mut self,
        workspace: Id,
        row: &BlobRow,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Records the segment at `index` with its hash, and moves the blob's
    /// offset to `offset`, marking it complete when `complete`.
    fn append_blob_segment(
        &mut self,
        workspace: Id,
        blob: Id,
        index: u64,
        hash: Digest,
        offset: u64,
        complete: bool,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Returns a blob whose objects a restore lost to an upload at offset
    /// 0 that the collector leaves alone until `delete_after`.
    fn reset_blob(
        &mut self,
        workspace: Id,
        blob: Id,
        delete_after: Option<u64>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores the account's reset code, replacing any other.
    fn put_reset_code(
        &mut self,
        account: Id,
        hash: Digest,
        expires_at: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes the account's reset code.
    fn take_reset_code(&mut self, account: Id) -> impl Future<Output = Result<()>> + Send;
    /// Marks a workspace closed and drops its members and invitations.
    fn close_workspace(&mut self, workspace: Id) -> impl Future<Output = Result<()>> + Send;
    /// Deletes what a closed workspace with no domain left still names: its
    /// policy chain, profiles, key stream, recorded accounts and approval
    /// requests. Its closed row stays, so nothing takes the ID again.
    fn delete_closed_workspace_rows(
        &mut self,
        workspace: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Starts or extends an instance's lease.
    fn hold_lease(&mut self, instance: Id, until: u64) -> impl Future<Output = Result<()>> + Send;
    /// Ends an instance's lease.
    fn drop_lease(&mut self, instance: Id) -> impl Future<Output = Result<()>> + Send;
    /// Deletes the blob's row and its segment rows.
    fn delete_blob(&mut self, workspace: Id, blob: Id) -> impl Future<Output = Result<()>> + Send;
    /// Records that the operation at `seq` in `domain` lists the blob.
    fn insert_blob_listing(
        &mut self,
        workspace: Id,
        blob: Id,
        domain: Id,
        seq: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stores a new invitation.
    fn insert_invitation(&mut self, row: &InvitationRow)
    -> impl Future<Output = Result<()>> + Send;
    /// Binds an invitation to the account that accepted it.
    fn accept_invitation(
        &mut self,
        invitation: Id,
        account: Id,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes an invitation, and remembers its ID until the expiry it had,
    /// so a late retry of its creation can't open its link again.
    fn delete_invitation(&mut self, invitation: Id) -> impl Future<Output = Result<()>> + Send;
    /// Stores a new share link.
    fn insert_share(&mut self, row: &ShareRow) -> impl Future<Output = Result<()>> + Send;
    /// Replaces a share link's snapshot.
    fn set_share(
        &mut self,
        token_hash: Digest,
        share: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;
    /// Deletes a share link and its snapshot.
    fn delete_share(&mut self, token_hash: Digest) -> impl Future<Output = Result<()>> + Send;
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
    /// domains reached these heads, or, with no heads, that the workspace
    /// changed without moving one, as an envelope or a re-wrap request
    /// does. The store calls it only after the change commits, and it
    /// returns at once: the announcement leaves from its own statement
    /// outside the change's transaction, and a lost one costs at most a
    /// pull's wait.
    fn announce(&self, workspace: Id, heads: Vec<(Id, u64)>) {
        let _ = (workspace, heads);
    }

    /// Hands a presence frame to the other instances on the same database
    /// (§23). It returns at once, and a frame that goes astray costs at
    /// most one cursor update. A database that one instance serves needs
    /// nothing.
    fn relay_presence(&self, workspace: Id, relayed: &Relayed) {
        let _ = (workspace, relayed);
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

/// The store's clock, in Unix seconds. Tests move it forward to pass the
/// 10 minutes after which sign-in rows expire.
#[derive(Debug, Default)]
pub struct Clock {
    offset: AtomicU64,
}

impl Clock {
    /// The current time in Unix seconds, plus any advance.
    pub fn now(&self) -> u64 {
        let system = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        system.saturating_add(self.offset.load(Ordering::Relaxed))
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        self.offset.fetch_add(by.as_secs(), Ordering::Relaxed);
    }
}

/// The protocol's state on a database: workspace creation, push and pull,
/// and the reads of §26, each checked as the spec sets.
pub struct Store<B: Backend> {
    backend: B,
    policies: PolicyCache,
    chains: policy::ChainCache,
    hub: EventHub,
    generation: u64,
    retention: Retention,
    interval: CheckpointInterval,
    secret: Option<SecretKey>,
    clock: Clock,
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
            retention: Retention::default(),
            interval: CheckpointInterval::default(),
            secret: None,
            clock: Clock::default(),
        })
    }

    /// Sets when checkpoints fall due by volume (§17); tests shorten it.
    pub fn with_checkpoint_interval(mut self, interval: CheckpointInterval) -> Self {
        self.interval = interval;
        self
    }

    /// Sets `retained_bytes` and `workspace_quota_bytes` (§19, §20).
    pub fn with_retention(mut self, retention: Retention) -> Self {
        self.retention = retention;
        self
    }

    /// Sets the server secret that account IDs derive from (§14).
    pub fn with_secret(mut self, secret: SecretKey) -> Self {
        self.secret = Some(secret);
        self
    }

    /// The store's clock.
    pub fn clock(&self) -> &Clock {
        &self.clock
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
            retained_bytes: self.retention.retained_bytes,
        }
    }

    /// The session a bearer token names, or `unauthorized`, or
    /// `device_removed` for a session whose card a link removed.
    pub async fn session(&self, token: &str) -> Result<Session> {
        Ok(self.session_and_hash(token).await?.0)
    }

    /// The session a bearer token names and the token's hash, or
    /// `unauthorized`, or `device_removed` for a session whose card a link
    /// removed (§7).
    pub async fn session_and_hash(&self, token: &str) -> Result<(Session, Digest)> {
        let (row, hash) = self.session_row(token).await?;
        if let Some(link) = row.removed {
            return Err(device_removed(link));
        }
        Ok((row.session, hash))
    }

    /// The session a bearer token names, also when a link removed its card:
    /// such a session keeps one right, reading its account's chains (§7).
    pub async fn session_reading_chains(&self, token: &str) -> Result<Session> {
        Ok(self.session_row(token).await?.0.session)
    }

    /// The stored session a bearer token names and the token's hash, or
    /// `unauthorized`.
    pub async fn session_row(&self, token: &str) -> Result<(SessionRow, Digest)> {
        let bytes: [u8; 32] = sync_protocol::types::from_base64url(token)
            .map_err(|_| refuse("unauthorized", "no valid session"))?;
        let hash = sync_protocol::crypto::hash(&bytes);
        let row = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.session(hash).await })
            }))
            .await?
            .ok_or_else(|| refuse("unauthorized", "no valid session"))?;
        Ok((row, hash))
    }

    /// `POST /v2/accounts/chain`: appends links to the session account's
    /// chain, in order and all or none (§7).
    pub async fn append_chain(
        &self,
        session: Session,
        links: Vec<Signed<AccountLink>>,
    ) -> Result<ChainAppendResponse> {
        let seqno = self
            .backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { chains::append(txn, session, &links).await })
            }))
            .await?;
        self.wake_recording(session.account).await?;
        Ok(ChainAppendResponse { seqno })
    }

    /// `POST /v2/accounts/chains` for a stored session: a session whose card
    /// a link removed reads only its own account's chains, and a cursor for
    /// any other account answers `device_removed` (§7).
    pub async fn read_chains_as(
        &self,
        row: SessionRow,
        request: ChainsRequest,
    ) -> Result<ChainsResponse> {
        if let Some(link) = row.removed
            && request
                .cursors
                .iter()
                .any(|cursor| cursor.account != row.session.account)
        {
            return Err(device_removed(link));
        }
        self.read_chains(row.session, request).await
    }

    /// `POST /v2/accounts/chains`: the links of the chains the session may
    /// read, after each cursor (§7).
    pub async fn read_chains(
        &self,
        session: Session,
        request: ChainsRequest,
    ) -> Result<ChainsResponse> {
        let cache = self.policies.clone();
        let verified = self.chains.clone();
        let now = self.clock.now();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    chains::read(txn, &cache, &verified, session, &request.cursors.0, now).await
                })
            }))
            .await
    }

    /// Stores an approval request that `session` posted with the
    /// commitment `commit`, naming `invitation` to `workspace` when it names
    /// one, for 10 minutes (§10). While it waits, the members of that
    /// workspace read the account's current chain (§7). Tickets 47 and 50
    /// build `POST /v2/approval-requests`, which checks a request before it
    /// stores it; until then tests store requests this way.
    pub async fn store_approval_request(
        &self,
        session: Session,
        commit: Digest,
        invitation: Option<(Id, Id)>,
    ) -> Result<()> {
        let now = self.clock.now();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    chains::store_approval_request(txn, session, commit, invitation, now).await
                })
            }))
            .await
    }

    /// Stores a card that a session of its account enrolled, as ticket 47's
    /// enrollment route will once it verifies the proof; until then tests
    /// enroll cards this way. A device ID held with other bytes, or that a
    /// link removed, gets `id_conflict`.
    pub async fn enroll_card(&self, card: Card) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { chains::enroll(txn, &card).await })
            }))
            .await
    }

    /// `POST /v2/devices/challenge`.
    pub async fn challenge(&self, session: Session) -> Result<ChallengeResponse> {
        let now = self.clock.now();
        let challenge = self
            .backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { chains::challenge(txn, session, now).await })
            }))
            .await?;
        Ok(ChallengeResponse {
            challenge,
            expires_in: chains::SIGN_IN_TTL as u32,
        })
    }

    /// `POST /v2/recovery`: records the recovery identity that the proof
    /// shows the session holds (§12). `origin` is the server's public
    /// origin, which the proof must name.
    pub async fn prove_recovery(
        &self,
        session: Session,
        token_hash: Digest,
        origin: &str,
        proof: Signed<RecoveryProof>,
    ) -> Result<()> {
        let now = self.clock.now();
        let origin = origin.to_owned();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    chains::prove_recovery(txn, session, token_hash, &origin, &proof, now).await
                })
            }))
            .await
    }

    /// The account ID an email signs up under (§14), or `internal` when
    /// the store has no server secret.
    pub fn account_id(&self, email: &str) -> Result<Id> {
        let secret = self
            .secret
            .as_ref()
            .ok_or_else(|| Error::internal("the store has no server secret"))?;
        Ok(sync_protocol::ids::account_id(secret, email))
    }

    /// The user handle of `account`'s passkeys, derived from the server
    /// secret (§14), or an internal error when the store has no secret.
    pub fn passkey_user_handle(&self, account: Id) -> Result<[u8; 16]> {
        let secret = self
            .secret
            .as_ref()
            .ok_or_else(|| Error::internal("the store has no server secret"))?;
        Ok(sync_protocol::ids::passkey_user_handle(secret, &account))
    }

    /// The confirmation the device page's decision form carries for the
    /// code `account` entered, valid until `expires_at` (§7).
    pub fn device_confirmation(
        &self,
        account: Id,
        user_code: &str,
        expires_at: u64,
    ) -> Result<Digest> {
        let secret = self
            .secret
            .as_ref()
            .ok_or_else(|| Error::internal("the store has no server secret"))?;
        Ok(sync_protocol::ids::device_confirmation(
            secret, &account, user_code, expires_at,
        ))
    }

    /// Whether `confirmation` is the device page's confirmation for the
    /// code `account` entered, and it hasn't expired (§7).
    pub fn device_confirmed(
        &self,
        account: Id,
        user_code: &str,
        expires_at: u64,
        confirmation: &[u8; 32],
    ) -> Result<bool> {
        let secret = self
            .secret
            .as_ref()
            .ok_or_else(|| Error::internal("the store has no server secret"))?;
        Ok(expires_at > self.clock.now()
            && sync_protocol::ids::device_confirmation_matches(
                secret,
                &account,
                user_code,
                expires_at,
                confirmation,
            ))
    }

    /// Stores a session for `token`, the 32 random bytes a sign-in returns,
    /// as their BLAKE3 hash. Until ticket 47 adds the device grant that signs
    /// a device in, tests and tools create device sessions this way.
    pub async fn create_session(&self, token: [u8; 32], session: Session) -> Result<()> {
        let hash = sync_protocol::crypto::hash(&token);
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.insert_session(hash, session).await })
            }))
            .await
    }

    /// Appends verified links to an account's chain, starting a chain with
    /// a genesis when the account has none, checking the links but not who
    /// sends them: tests set up stores this way. `POST /v2/accounts/chain`
    /// runs [`Store::append_chain`], which checks the sender too.
    pub async fn append_account_links(&self, links: Vec<Signed<AccountLink>>) -> Result<u64> {
        let account = links.first().map(|link| link.body.account);
        let seqno = self
            .backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { reads::append_account_links(txn, &links).await })
            }))
            .await?;
        if let Some(account) = account {
            self.wake_recording(account).await?;
        }
        Ok(seqno)
    }

    /// `POST /v2/workspaces`.
    pub async fn create_workspace(&self, session: Session, body: Vec<u8>) -> Result<PushResponse> {
        let checked = create::decode(&body)?;
        let chain = self.session_chain(session).await?;
        create::check_signatures(&checked, session, &chain)?;
        let cache = self.policies.clone();
        let interval = self.interval;
        let digest = sync_protocol::crypto::hash(&body);
        let response = self
            .backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    create::commit(txn, &cache, interval, session, checked, digest).await
                })
            }))
            .await?;
        let response = PushResponse::from_bytes(&response).map_err(Error::internal)?;
        Ok(response)
    }

    /// `GET /v2/workspaces`.
    pub async fn workspaces(&self, session: Session) -> Result<WorkspacesResponse> {
        let quota = self.retention.quota_bytes;
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { reads::workspaces(txn, session, quota).await })
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
        let (response, acknowledged) = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let response =
                        reads::keys(txn, &cache, session, workspace, after, recipient).await?;
                    let acknowledged =
                        reads::acknowledged_rewraps(txn, workspace, recipient, after).await?;
                    Ok((response, acknowledged))
                })
            }))
            .await?;
        // A fetch that passed an answer to the device's re-wrap request
        // acknowledges it, and the server closes the request (§9).
        if !acknowledged.is_empty() {
            self.backend
                .write(Box::new(move |txn: &mut B::Write| {
                    Box::pin(async move {
                        txn.lock(&LockPlan {
                            workspace: Some(workspace),
                            policy_exclusive: false,
                            key_sequence: true,
                            domains: Vec::new(),
                            blobs: Vec::new(),
                        })
                        .await?;
                        for request in
                            reads::acknowledged_rewraps(txn, workspace, recipient, after).await?
                        {
                            txn.close_rewrap(workspace, &request).await?;
                        }
                        Ok(())
                    })
                }))
                .await?;
        }
        Ok(response)
    }

    /// `GET /v2/workspaces/{w}/keys/current?recipient={device}`.
    pub async fn current_keys(
        &self,
        session: Session,
        workspace: Id,
        recipient: Id,
    ) -> Result<KeysResponse> {
        let cache = self.policies.clone();
        let (response, acknowledged) = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let response =
                        reads::current_keys(txn, &cache, session, workspace, recipient).await?;
                    // The fetch hands the device every answer to its
                    // requests, which acknowledges them (§9).
                    let acknowledged = reads::acknowledged_rewraps(
                        txn,
                        workspace,
                        recipient,
                        response.head_key_seq,
                    )
                    .await?;
                    Ok((response, acknowledged))
                })
            }))
            .await?;
        if !acknowledged.is_empty() {
            self.backend
                .write(Box::new(move |txn: &mut B::Write| {
                    Box::pin(async move {
                        txn.lock(&LockPlan {
                            workspace: Some(workspace),
                            policy_exclusive: false,
                            key_sequence: true,
                            domains: Vec::new(),
                            blobs: Vec::new(),
                        })
                        .await?;
                        for request in acknowledged {
                            txn.close_rewrap(workspace, &request).await?;
                        }
                        Ok(())
                    })
                }))
                .await?;
        }
        Ok(response)
    }

    /// `POST /v2/workspaces/{w}/domains/{d}/rewrap`.
    pub async fn rewrap(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> Result<()> {
        let cache = self.policies.clone();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    reads::rewrap(txn, &cache, session, workspace, domain, epoch).await
                })
            }))
            .await?;
        // The request joined the key stream, which the readers' realtime
        // connections announce (§9, §23).
        self.wake(workspace);
        Ok(())
    }

    /// Wakes `workspace`'s pulls and realtime connections on this instance
    /// and the others, after a change committed.
    fn wake(&self, workspace: Id) {
        self.hub.notify(workspace);
        self.backend.announce(workspace, Vec::new());
    }

    /// Wakes every workspace whose policy records a head of `account`, so
    /// their realtime connections announce its new chain head (§23).
    async fn wake_recording(&self, account: Id) -> Result<()> {
        let workspaces = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.workspaces_recording(account).await })
            }))
            .await?;
        for workspace in workspaces {
            self.wake(workspace);
        }
        Ok(())
    }

    /// What a realtime connection of `session` to `workspace` starts from:
    /// the heads, policy revision, key stream head and member chains it
    /// would tell of, read in one snapshot. A device that isn't active in
    /// the workspace gets `forbidden` (§23).
    pub async fn realtime_watch(&self, session: Session, workspace: Id) -> Result<Watch> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let mut watch = Watch::default();
                    realtime::read(txn, &cache, session, workspace, &mut watch, true).await?;
                    Ok(watch)
                })
            }))
            .await
    }

    /// Checks a presence record from `session` (§23): its signer is the
    /// session's device and active in `workspace`, reads the domain, and
    /// wrote under the domain's current epoch with no rotation pending. The
    /// server stores nothing of it.
    pub async fn check_presence(
        &self,
        session: Session,
        workspace: Id,
        record: sync_protocol::records::Signed<sync_protocol::records::Presence>,
    ) -> Result<()> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    realtime::check_presence(txn, &cache, session, workspace, &record).await
                })
            }))
            .await
    }

    /// Sends a checked presence frame to the connections on this instance
    /// and to the other instances.
    pub fn relay_presence(&self, workspace: Id, relayed: Relayed) {
        self.backend.relay_presence(workspace, &relayed);
        self.hub.relay(workspace, relayed);
    }

    /// The frames that tell a realtime connection of `session` what
    /// committed in `workspace` since `watch`, from one snapshot: a records
    /// frame for each domain whose new entries fit one frame, a notify for
    /// each other domain that moved, and the policy, key and account chain
    /// hints. Brings `watch` up to date. A device no longer active in the
    /// workspace gets `forbidden` (§23).
    pub async fn realtime_frames(
        &self,
        session: Session,
        workspace: Id,
        watch: &mut Watch,
    ) -> Result<Vec<sync_protocol::messages::Frame>> {
        let cache = self.policies.clone();
        let mut moved = watch.clone();
        let (frames, moved) = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let frames =
                        realtime::read(txn, &cache, session, workspace, &mut moved, false).await?;
                    Ok((frames, moved))
                })
            }))
            .await?;
        *watch = moved;
        Ok(frames)
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
        let mut ops = push::decode(&body, workspace, session)?;
        let chain = self.session_chain(session).await?;
        push::check_signatures(&ops, session, &chain)?;
        let cache = self.policies.clone();
        let generation = self.generation;
        let interval = self.interval;
        let quota = self.retention.quota_bytes;
        let now = self.clock().now();
        let response = if let [push::Pushed::Checkpoint(..)] = ops.as_slice() {
            let Some(push::Pushed::Checkpoint(_, bundle)) = ops.pop() else {
                unreachable!("one checkpoint");
            };
            self.push_checkpoint(session, workspace, bundle).await?
        } else {
            // An approval's fresh checkpoint, last in its push: the checks
            // and the recomputation over committed records run first, in a
            // read transaction, as for a checkpoint alone (§17).
            let plan = match ops.last() {
                Some(push::Pushed::Checkpoint(index, bundle)) => {
                    let index = *index;
                    let bundle = Arc::new(bundle.clone());
                    let read_cache = self.policies.clone();
                    let plan = self
                        .backend
                        .read(Box::new(move |txn: &mut B::Read| {
                            Box::pin(async move {
                                checkpoints::plan(txn, &read_cache, session, workspace, &bundle)
                                    .await
                            })
                        }))
                        .await
                        .map_err(|e| match e {
                            Error::Refused(refusal) => push::failure(index, refusal),
                            internal => internal,
                        })?;
                    Some(plan)
                }
                _ => None,
            };
            let group = push::group(workspace, &ops);
            let job: WriteJob<B::Write, PushResponse> = Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    push::commit(
                        txn, &cache, interval, quota, generation, session, workspace, ops, plan,
                        now,
                    )
                    .await
                })
            });
            match group {
                Some(group) => self.backend.write_grouped(group, job).await?,
                None => self.backend.write(job).await?,
            }
        };
        // Every committed push wakes the workspace's pulls and realtime
        // connections, also one that only added envelopes to the key
        // stream (§23).
        self.hub.notify(workspace);
        let mut heads: BTreeMap<Id, u64> = BTreeMap::new();
        for receipt in response.receipts.iter() {
            let head = heads.entry(receipt.domain).or_default();
            *head = (*head).max(receipt.seq);
        }
        self.backend
            .announce(workspace, heads.into_iter().collect());
        Ok(response)
    }

    /// A push of one checkpoint bundle (§17): the checks and the
    /// recomputation in a read transaction, then the commit under the
    /// domain's head row.
    async fn push_checkpoint(
        &self,
        session: Session,
        workspace: Id,
        bundle: sync_protocol::records::CheckpointBundle,
    ) -> Result<PushResponse> {
        let in_record = |e: Error| match e {
            Error::Refused(refusal) => push::failure(0, refusal),
            internal => internal,
        };
        let bundle = Arc::new(bundle);
        let cache = self.policies.clone();
        let read_bundle = bundle.clone();
        let plan = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::plan(txn, &cache, session, workspace, &read_bundle).await
                })
            }))
            .await
            .map_err(in_record)?;
        let cache = self.policies.clone();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    checkpoints::commit(txn, &cache, session, workspace, &bundle, plan).await
                })
            }))
            .await
            .map_err(in_record)
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/checkpoint`.
    pub async fn checkpoint(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
    ) -> Result<sync_protocol::messages::CheckpointResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::latest(txn, &cache, session, workspace, domain).await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/checkpoints/{digest}`.
    pub async fn checkpoint_by_digest(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        digest: Digest,
    ) -> Result<sync_protocol::messages::Entry> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::by_digest(txn, &cache, session, workspace, domain, digest).await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/checkpoint-chain`.
    pub async fn checkpoint_chain(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        from: Digest,
        to: Digest,
    ) -> Result<sync_protocol::messages::CheckpointChainResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::chain(txn, &cache, session, workspace, domain, from, to).await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/live`.
    pub async fn live(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        at: u64,
        rid: Option<Id>,
        after: Option<sync_protocol::messages::LiveCursor>,
    ) -> Result<sync_protocol::messages::LiveResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::live(txn, &cache, session, workspace, domain, at, rid, after).await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/manifest?at&buckets=all`.
    pub async fn manifest_buckets(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        at: u64,
    ) -> Result<sync_protocol::messages::ManifestBucketsResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::manifest_buckets(txn, &cache, session, workspace, domain, at).await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/manifest`, levels 0 to 2.
    #[allow(clippy::too_many_arguments)]
    pub async fn manifest_node(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        at: u64,
        level: u8,
        index: u16,
    ) -> Result<sync_protocol::messages::ManifestNodeResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::manifest_node(
                        txn, &cache, session, workspace, domain, at, level, index,
                    )
                    .await
                })
            }))
            .await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/manifest`, level 3.
    #[allow(clippy::too_many_arguments)]
    pub async fn manifest_leaves(
        &self,
        session: Session,
        workspace: Id,
        domain: Id,
        at: u64,
        index: u16,
        after: Option<sync_protocol::messages::LiveCursor>,
    ) -> Result<sync_protocol::messages::ManifestLeavesResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    checkpoints::manifest_leaves(
                        txn, &cache, session, workspace, domain, at, index, after,
                    )
                    .await
                })
            }))
            .await
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
