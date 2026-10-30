//! Request and response bodies, pull pages, log entries, error bodies and
//! realtime frames (§15, §23, §26).
//!
//! Bodies carry no tag: the route fixes their type. Records inside them keep
//! their own tags, and a record whose tag doesn't match the type its
//! position names fails to decode.

use serde::{Deserialize, Serialize};

use crate::codec::{
    Bytes, Decode, DecodeError, DecodeErrorKind, Encode, List, Nested, Reader, SortSpec, Sorted,
    Text, Writer,
};
use crate::records::{
    AccountHead, AccountLink, ByAccount, Card, Checkpoint, CheckpointBundle, Enroll, Envelope,
    Epoch, EpochBundle, InviteContext, Offer, Operation, PolicyChange, Presence, Profile,
    RecoveryProof, Ref, Role, Share, Signed,
};
use crate::types::{Digest, Id, IdOrZero, Key};

/// The media type of every request and response body.
pub const MEDIA_TYPE: &str = "application/vnd.noura.sync.v2";
/// The header that carries the server's restore generation.
pub const RESTORE_GENERATION_HEADER: &str = "Noura-Restore-Generation";
/// The most bytes in a request or response body (§27).
pub const MAX_BODY: usize = 1_048_576;
/// The most bytes in a realtime frame (§27).
pub const MAX_FRAME: usize = 65_536;
/// The most presence channels one subscribe names (§27).
pub const MAX_SUBSCRIBE: usize = 256;
/// The longest a pull waits, in seconds.
pub const MAX_WAIT_SECONDS: u32 = 25;

fn unknown(r: &Reader<'_>, offset: usize) -> DecodeError {
    r.error_at(DecodeErrorKind::UnknownValue, offset)
}

/// A pushed record, `u8 record_type | bytes record` (§15).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "record_type", content = "record", rename_all = "snake_case")]
pub enum RecordRef {
    /// 1: an operation.
    Operation(Signed<Operation>),
    /// 2: a policy change.
    Policy(Signed<PolicyChange>),
    /// 3: an epoch bundle.
    EpochBundle(EpochBundle),
    /// 4: an envelope.
    Envelope(Signed<Envelope>),
    /// 5: a checkpoint bundle.
    CheckpointBundle(CheckpointBundle),
    /// 6: a workspace profile.
    Profile(Signed<Profile>),
}

impl RecordRef {
    /// The type byte.
    pub fn code(&self) -> u8 {
        match self {
            RecordRef::Operation(_) => 1,
            RecordRef::Policy(_) => 2,
            RecordRef::EpochBundle(_) => 3,
            RecordRef::Envelope(_) => 4,
            RecordRef::CheckpointBundle(_) => 5,
            RecordRef::Profile(_) => 6,
        }
    }

    /// The digest a receipt names: the record's, or for an epoch or
    /// checkpoint bundle the digest of the record inside it.
    pub fn receipt_digest(&self) -> Digest {
        match self {
            RecordRef::Operation(r) => r.digest(),
            RecordRef::Policy(r) => r.digest(),
            RecordRef::EpochBundle(b) => b.epoch.digest(),
            RecordRef::Envelope(r) => r.digest(),
            RecordRef::CheckpointBundle(b) => b.checkpoint.digest(),
            RecordRef::Profile(r) => r.digest(),
        }
    }
}

impl Encode for RecordRef {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.code());
        let record = match self {
            RecordRef::Operation(r) => r.to_bytes(),
            RecordRef::Policy(r) => r.to_bytes(),
            RecordRef::EpochBundle(r) => r.to_bytes(),
            RecordRef::Envelope(r) => r.to_bytes(),
            RecordRef::CheckpointBundle(r) => r.to_bytes(),
            RecordRef::Profile(r) => r.to_bytes(),
        };
        w.bytes(&record);
    }
}

impl Decode for RecordRef {
    const MIN_SIZE: usize = 1 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = r.offset();
        Ok(match r.u8()? {
            1 => RecordRef::Operation(r.get::<Nested<_>>()?.0),
            2 => RecordRef::Policy(r.get::<Nested<_>>()?.0),
            3 => RecordRef::EpochBundle(r.get::<Nested<_>>()?.0),
            4 => RecordRef::Envelope(r.get::<Nested<_>>()?.0),
            5 => RecordRef::CheckpointBundle(r.get::<Nested<_>>()?.0),
            6 => RecordRef::Profile(r.get::<Nested<_>>()?.0),
            _ => return Err(unknown(r, offset)),
        })
    }
}

crate::wire_struct! {
    /// A push receipt: one per log entry the push created.
    pub struct Receipt {
        /// The record's digest, or the inner record's for a bundle.
        pub record_digest: Digest,
        /// The domain whose log holds the entry.
        pub domain: Id,
        /// The entry's sequence.
        pub seq: u64,
    }
}

crate::wire_struct! {
    /// A policy entry in a domain's log (§8): where a change takes effect.
    pub struct PolicyEntry {
        /// The change's revision.
        pub revision: u64,
        /// The change's digest.
        pub head: Digest,
    }
}

/// A log entry's record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "record_type", content = "record", rename_all = "snake_case")]
pub enum EntryRecord {
    /// 1: an operation.
    Operation(Signed<Operation>),
    /// 5: a checkpoint bundle.
    Checkpoint(CheckpointBundle),
    /// 8: an epoch record.
    Epoch(Signed<Epoch>),
    /// 9: a policy entry.
    Policy(PolicyEntry),
}

impl EntryRecord {
    /// The type byte.
    pub fn code(&self) -> u8 {
        match self {
            EntryRecord::Operation(_) => 1,
            EntryRecord::Checkpoint(_) => 5,
            EntryRecord::Epoch(_) => 8,
            EntryRecord::Policy(_) => 9,
        }
    }
}

/// `entry`: a log entry with its sequence, `u64 seq | u8 record_type |
/// bytes record`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Entry {
    /// The sequence.
    pub seq: u64,
    /// The record.
    #[serde(flatten)]
    pub record: EntryRecord,
}

impl Encode for Entry {
    fn encode(&self, w: &mut Writer) {
        w.u64(self.seq);
        w.u8(self.record.code());
        let record = match &self.record {
            EntryRecord::Operation(r) => r.to_bytes(),
            EntryRecord::Checkpoint(r) => r.to_bytes(),
            EntryRecord::Epoch(r) => r.to_bytes(),
            EntryRecord::Policy(r) => r.to_bytes(),
        };
        w.bytes(&record);
    }
}

impl Decode for Entry {
    const MIN_SIZE: usize = 8 + 1 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let seq = r.u64()?;
        let offset = r.offset();
        let record = match r.u8()? {
            1 => EntryRecord::Operation(r.get::<Nested<_>>()?.0),
            5 => EntryRecord::Checkpoint(r.get::<Nested<_>>()?.0),
            8 => EntryRecord::Epoch(r.get::<Nested<_>>()?.0),
            9 => EntryRecord::Policy(r.get::<Nested<_>>()?.0),
            _ => return Err(unknown(r, offset)),
        };
        Ok(Self { seq, record })
    }
}

/// Sorts entries by sequence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BySeq;

impl SortSpec<Entry> for BySeq {
    fn key(item: &Entry) -> Vec<u8> {
        item.seq.to_be_bytes().to_vec()
    }
}

/// Entries sorted by sequence.
pub type Entries = Sorted<Entry, BySeq>;

crate::wire_struct! {
    /// `pull_cursor`: a domain and the last sequence the client applied.
    pub struct PullCursor {
        /// The domain.
        pub domain: Id,
        /// The cursor.
        pub after: u64,
    }
}

crate::byte_enum! {
    /// A pull page's status (§15).
    pub enum PageStatus {
        /// 1: ok.
        Ok = 1,
        /// 2: compacted, the cursor lies below the retention floor.
        Compacted = 2,
        /// 3: before the account's history floor.
        BeforeHistoryFloor = 3,
        /// 4: forbidden, the device can't read the domain.
        Forbidden = 4,
        /// 5: not found, the domain was deleted.
        NotFound = 5,
    }
}

crate::wire_struct! {
    /// A pull page for one domain (§15).
    pub struct Page {
        /// The domain.
        pub domain: Id,
        /// The status.
        pub status: PageStatus,
        /// The requested cursor.
        pub after: u64,
        /// The page holds every record in (after, complete_through].
        pub complete_through: u64,
        /// The domain's latest sequence.
        pub head: u64,
        /// The retention floor.
        pub retention_floor: u64,
        /// `covered_seq` of the latest checkpoint, 0 before the first.
        pub checkpoint_seq: u64,
        /// The server's restore generation.
        pub restore_generation: u64,
        /// The server's policy head revision.
        pub policy_revision: u64,
        /// The next checkpoint's due point, or 0.
        pub checkpoint_due: u64,
        /// The entries, sorted by sequence.
        pub entries: Entries,
    }
}

/// A pull page that breaks the shape rules of §15.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PageShapeError {
    /// A page whose status isn't ok holds entries, moves its cursor or
    /// reports a head, a floor or a due point it must leave at 0.
    #[error("a page whose status isn't ok must be empty")]
    NotEmpty,
    /// An entry lies outside (after, complete_through].
    #[error("an entry lies outside the page's range")]
    OutOfRange,
    /// `complete_through` lies below `after` or above `head`.
    #[error("complete_through outside after..=head")]
    Range,
}

impl Page {
    /// Checks the rules of §15 that a page's fields alone show. A client
    /// compares `head` with its cursor first: a server that went back
    /// answers a cursor above its head with `complete_through` at the
    /// cursor, which this check reports only as [`PageShapeError::Range`].
    pub fn check_shape(&self) -> Result<(), PageShapeError> {
        if self.status != PageStatus::Ok {
            let keeps_floor = self.status == PageStatus::Compacted;
            if !self.entries.is_empty()
                || self.complete_through != self.after
                || self.head != 0
                || self.checkpoint_due != 0
                || (!keeps_floor && (self.retention_floor != 0 || self.checkpoint_seq != 0))
            {
                return Err(PageShapeError::NotEmpty);
            }
            return Ok(());
        }
        if self.complete_through < self.after || self.complete_through > self.head {
            return Err(PageShapeError::Range);
        }
        if self
            .entries
            .iter()
            .any(|e| e.seq <= self.after || e.seq > self.complete_through)
        {
            return Err(PageShapeError::OutOfRange);
        }
        Ok(())
    }
}

crate::wire_struct! {
    /// `live_cursor`: the last entry a `live` or level-3 `manifest` page
    /// returned.
    pub struct LiveCursor {
        /// The object.
        pub rid: Id,
        /// The device.
        pub device: Id,
        /// The counter.
        pub counter: u64,
    }
}

impl LiveCursor {
    /// The text form a query carries: base64url of the encoding (§5).
    pub fn to_query(&self) -> String {
        crate::types::to_base64url(&self.to_bytes())
    }

    /// Parses the text form.
    pub fn from_query(text: &str) -> Result<Self, crate::types::TextFormError> {
        let bytes: [u8; 40] = crate::types::from_base64url(text)?;
        Self::from_bytes(&bytes).map_err(|_| crate::types::TextFormError::ZeroId)
    }
}

crate::wire_struct! {
    /// `leaf_ref`: a live operation's ref under its object.
    pub struct LeafRef {
        /// The object.
        pub rid: Id,
        /// The operation's ref.
        pub op: Ref,
    }
}

/// Sorts leaf refs by `rid`, device and counter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByRidDeviceCounter;

impl SortSpec<LeafRef> for ByRidDeviceCounter {
    fn key(item: &LeafRef) -> Vec<u8> {
        let mut key = item.rid.0.to_vec();
        key.extend_from_slice(&item.op.device.0);
        key.extend_from_slice(&item.op.counter.to_be_bytes());
        key
    }
}

crate::wire_struct! {
    /// `chain_cursor`: an account chain to read, after a seqno.
    pub struct ChainCursor {
        /// The account.
        pub account: Id,
        /// The chain's genesis, absent for the account's current chain.
        pub genesis: Option<Digest>,
        /// The last seqno received.
        pub after: u64,
    }
}

crate::byte_enum! {
    /// Whether a chain is the account's current one.
    pub enum ChainStatus {
        /// 1: current.
        Current = 1,
        /// 2: retired by a reset.
        Retired = 2,
    }
}

/// An account chain link inside a body, as `bytes`.
pub type LinkBytes = Nested<Signed<AccountLink>>;

crate::wire_struct! {
    /// `chain_page`: links of one chain after a cursor.
    pub struct ChainPage {
        /// The account.
        pub account: Id,
        /// The chain's genesis digest.
        pub genesis: Digest,
        /// Current or retired.
        pub status: ChainStatus,
        /// The chain's head seqno.
        pub head_seqno: u64,
        /// Links after the cursor, in seqno order.
        pub links: List<LinkBytes>,
    }
}

crate::wire_struct! {
    /// `pending`: an approval request awaiting an approver.
    pub struct Pending {
        /// The candidate's card.
        pub card: Nested<Card>,
        /// The request's commitment.
        pub commit: Digest,
        /// The candidate's account head, for an invitation or reset.
        pub account: Option<AccountHead>,
        /// The invitation, for an invited account.
        pub invitation: Option<Id>,
        /// The invitation proof.
        pub invite_proof: Option<Digest>,
        /// The workspace a reset asks to be accepted in.
        pub workspace: Option<Id>,
    }
}

crate::wire_struct! {
    /// `workspace_info`: a workspace the account belongs to.
    pub struct WorkspaceInfo {
        /// The workspace.
        pub workspace: Id,
        /// The account's role.
        pub role: Role,
        /// Live usage.
        pub used_bytes: u64,
        /// History.
        pub history_bytes: u64,
        /// The quota, absent when unlimited.
        pub quota_bytes: Option<u64>,
        /// The latest profile record.
        pub profile: Option<Nested<Signed<Profile>>>,
    }
}

crate::byte_enum! {
    /// Whether an enrolled card is in the account chain.
    pub enum DeviceState {
        /// 1: enrolled and listed by no chain.
        Enrolled = 1,
        /// 2: active in the account's chain.
        Active = 2,
    }
}

crate::wire_struct! {
    /// `device_info`: an enrolled card and its sessions.
    pub struct DeviceInfo {
        /// The card.
        pub card: Nested<Card>,
        /// Its session count.
        pub sessions: u32,
        /// Its state.
        pub state: DeviceState,
    }
}

crate::wire_struct! {
    /// A re-wrap request in the key stream: a device asks for a new
    /// envelope of a domain's epoch.
    pub struct RewrapRequest {
        /// The domain.
        pub domain: Id,
        /// The requesting device.
        pub device: Id,
        /// The epoch.
        pub epoch: u64,
    }
}

/// A key stream record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "record_type", content = "record", rename_all = "snake_case")]
pub enum KeyRecord {
    /// 3: an epoch record.
    Epoch(Signed<Epoch>),
    /// 4: an envelope.
    Envelope(Signed<Envelope>),
    /// 5: a re-wrap request.
    Rewrap(RewrapRequest),
}

/// `key_entry`: a key stream record with its key sequence.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyEntry {
    /// The key sequence.
    pub key_seq: u64,
    /// The record.
    #[serde(flatten)]
    pub record: KeyRecord,
}

impl Encode for KeyEntry {
    fn encode(&self, w: &mut Writer) {
        w.u64(self.key_seq);
        let (code, record) = match &self.record {
            KeyRecord::Epoch(r) => (3, r.to_bytes()),
            KeyRecord::Envelope(r) => (4, r.to_bytes()),
            KeyRecord::Rewrap(r) => (5, r.to_bytes()),
        };
        w.u8(code);
        w.bytes(&record);
    }
}

impl Decode for KeyEntry {
    const MIN_SIZE: usize = 8 + 1 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let key_seq = r.u64()?;
        let offset = r.offset();
        let record = match r.u8()? {
            3 => KeyRecord::Epoch(r.get::<Nested<_>>()?.0),
            4 => KeyRecord::Envelope(r.get::<Nested<_>>()?.0),
            5 => KeyRecord::Rewrap(r.get::<Nested<_>>()?.0),
            _ => return Err(unknown(r, offset)),
        };
        Ok(Self { key_seq, record })
    }
}

crate::byte_enum! {
    /// An invitation's status.
    pub enum InvitationStatus {
        /// 1: pending.
        Pending = 1,
        /// 2: accepted.
        Accepted = 2,
    }
}

crate::wire_struct! {
    /// `invitation_info`: an open invitation.
    pub struct InvitationInfo {
        /// The invitation.
        pub invitation: Id,
        /// The role it grants.
        pub role: Role,
        /// Pending or accepted.
        pub status: InvitationStatus,
        /// The accepting account, once known.
        pub accepted_by: Option<Id>,
        /// Unix seconds.
        pub expires_at: u64,
        /// The encrypted context.
        pub invite_context: Nested<InviteContext>,
    }
}

crate::wire_struct! {
    /// `share_info`: an active share link.
    pub struct ShareInfo {
        /// The token hash.
        pub token_hash: Digest,
        /// The domain.
        pub domain: Id,
        /// Unix seconds.
        pub expires_at: u64,
    }
}

crate::wire_struct! {
    /// `error`: every error response's body.
    pub struct ErrorBody {
        /// The error code.
        pub code: Text,
        /// A message with no database or operating system detail.
        pub message: Text,
        /// When to retry, for `rate_limited`.
        pub retry_after_seconds: Option<u64>,
        /// The code's structured detail.
        pub detail: Option<Bytes>,
    }
}

crate::wire_struct! {
    /// The detail of `chain_conflict`: the server's head.
    pub struct ChainConflictDetail {
        /// The domain.
        pub domain: Id,
        /// The head's counter.
        pub counter: u64,
        /// The head's digest.
        pub record_digest: Digest,
    }
}

crate::wire_struct! {
    /// The detail of `cursor_compacted` from `live` or `manifest`.
    pub struct CursorCompactedDetail {
        /// The retention floor.
        pub retention_floor: u64,
        /// The latest checkpoint's `covered_seq`.
        pub checkpoint_covered_seq: u64,
    }
}

crate::wire_struct! {
    /// The detail of a failed push: the failing record and its error.
    pub struct PushFailureDetail {
        /// The failing record's index in the push.
        pub record_index: u32,
        /// Its error code.
        pub code: Text,
        /// Its own detail, such as a `chain_conflict` head.
        pub record_detail: Option<Bytes>,
    }
}

crate::wire_struct! {
    /// The detail of `device_removed`: the link that removed the card or
    /// the genesis that retired its chain.
    pub struct DeviceRemovedDetail {
        /// The chain's genesis digest.
        pub genesis: Digest,
        /// The link's seqno.
        pub seqno: u64,
    }
}

/// The server error codes of §26 with their HTTP statuses.
pub const SERVER_ERRORS: &[(&str, u16)] = &[
    ("bad_request", 400),
    ("too_large", 413),
    ("unauthorized", 401),
    ("forbidden", 403),
    ("not_found", 404),
    ("rate_limited", 429),
    ("version_unsupported", 400),
    ("signature_invalid", 422),
    ("padding_invalid", 422),
    ("chain_conflict", 409),
    ("supersedes_unknown", 409),
    ("epoch_stale", 409),
    ("epoch_conflict", 409),
    ("rotation_pending", 409),
    ("policy_stale", 409),
    ("policy_conflict", 409),
    ("policy_regressed", 422),
    ("recipients_invalid", 422),
    ("manifest_mismatch", 422),
    ("cursor_compacted", 410),
    ("pruned", 410),
    ("blob_incomplete", 409),
    ("blob_digest_mismatch", 422),
    ("blob_domain_mismatch", 422),
    ("blob_offset_conflict", 409),
    ("quota_exceeded", 507),
    ("id_conflict", 409),
    ("rid_domain_mismatch", 409),
    ("checkpoint_not_due", 409),
    ("checkpoint_stale", 409),
    ("checkpoint_unknown", 409),
    ("rotation_not_due", 409),
    ("recipient_limit", 409),
    ("account_conflict", 409),
    ("device_removed", 401),
    ("reset_code_invalid", 400),
    ("profile_conflict", 409),
    ("workspace_limit", 409),
    ("snapshot_required", 409),
    ("conflict_limit", 409),
    ("supersedes_incomplete", 409),
    ("domain_limit", 409),
    ("before_history_floor", 403),
    ("challenge_invalid", 400),
    ("origin_mismatch", 400),
    ("authorization_pending", 400),
    ("slow_down", 400),
    ("expired_token", 400),
    ("access_denied", 400),
    ("offer_expired", 410),
    ("invitation_limit", 409),
    ("invitation_invalid", 404),
    ("share_limit", 409),
    ("internal", 500),
];

/// The HTTP status of a server error code.
pub fn server_error_status(code: &str) -> Option<u16> {
    SERVER_ERRORS
        .iter()
        .find(|(name, _)| *name == code)
        .map(|(_, status)| *status)
}

/// The client error codes of §26.
pub const CLIENT_ERRORS: &[&str] = &[
    "sync_chain_gap",
    "sync_chain_reordered",
    "sync_chain_replayed",
    "sync_chain_forked",
    "sync_policy_forked",
    "sync_policy_rollback",
    "sync_policy_unauthorized",
    "sync_policy_regressed",
    "sync_writer_not_authorized",
    "sync_epoch_stale",
    "sync_key_mismatch",
    "sync_record_unreadable",
    "sync_manifest_mismatch",
    "sync_manifest_rollback",
    "sync_journal_rollback",
    "sync_approval_mismatch",
    "sync_recovery_kit_invalid",
    "sync_workspace_unverified",
    "sync_profile_rollback",
    "sync_reseal_limit",
    "sync_server_rollback",
    "sync_account_forked",
    "sync_account_unauthorized",
    "sync_restore_lost",
    "sync_text_conflict",
];

crate::wire_struct! {
    /// `GET /v2/limits`.
    pub struct LimitsResponse {
        /// The protocol version, 2.
        pub protocol: u16,
        /// The restore generation.
        pub restore_generation: u64,
        /// 1,048,576.
        pub max_body: u32,
        /// 262,144.
        pub max_inline: u32,
        /// 2,147,483,648.
        pub max_blob: u64,
        /// 256.
        pub max_covers: u32,
        /// 512.
        pub max_recipients: u32,
        /// 2.
        pub presence_per_second: u32,
        /// The server's `retained_bytes`.
        pub retained_bytes: u64,
    }
}

crate::wire_struct! {
    /// `POST /v2/device/authorize` response.
    pub struct DeviceAuthorizeResponse {
        /// 32 random bytes in base64url.
        pub device_code: Text,
        /// 8 characters from `BCDFGHJKLMNPQRSTVWXZ`.
        pub user_code: Text,
        /// Where to enter the code.
        pub verification_uri: Text,
        /// Seconds until the codes expire.
        pub expires_in: u32,
        /// Seconds between polls.
        pub interval: u32,
    }
}

crate::wire_struct! {
    /// `POST /v2/device/token` request.
    pub struct DeviceTokenRequest {
        /// The device code.
        pub device_code: Text,
    }
}

crate::wire_struct! {
    /// `POST /v2/device/token` response.
    pub struct DeviceTokenResponse {
        /// The session token's 43 characters.
        pub session_token: Text,
        /// The account the person who approved the code signed in to, which
        /// the device's card names.
        pub account: Id,
    }
}

crate::wire_struct! {
    /// `POST /v2/devices/challenge` response.
    pub struct ChallengeResponse {
        /// The challenge.
        pub challenge: Key,
        /// Seconds until it expires.
        pub expires_in: u32,
    }
}

crate::wire_struct! {
    /// `POST /v2/devices` request.
    pub struct EnrollRequest {
        /// The signed enrollment proof.
        pub enroll: Nested<Signed<Enroll>>,
    }
}

crate::wire_struct! {
    /// `GET /v2/devices` response.
    pub struct DevicesResponse {
        /// The account's enrolled cards that no link removed.
        pub devices: List<DeviceInfo>,
    }
}

crate::wire_struct! {
    /// `POST /v2/accounts/chain` request: links to append, in order.
    pub struct ChainAppendRequest {
        /// The links.
        pub links: List<LinkBytes>,
    }
}

crate::wire_struct! {
    /// `POST /v2/accounts/chain` response.
    pub struct ChainAppendResponse {
        /// The seqno of the chain's new head.
        pub seqno: u64,
    }
}

crate::wire_struct! {
    /// `POST /v2/accounts/chains` request.
    pub struct ChainsRequest {
        /// One cursor per chain.
        pub cursors: List<ChainCursor>,
    }
}

crate::wire_struct! {
    /// `POST /v2/accounts/chains` response.
    pub struct ChainsResponse {
        /// One page per cursor, in request order.
        pub pages: List<ChainPage>,
    }
}

crate::wire_struct! {
    /// `POST /v2/account/reset` request.
    pub struct ResetRequest {
        /// The emailed reset code.
        pub code: Text,
        /// The new chain's genesis.
        pub genesis: LinkBytes,
    }
}

crate::wire_struct! {
    /// `POST /v2/approval-requests` request.
    pub struct ApprovalRequest {
        /// The candidate's commitment.
        pub commit: Digest,
        /// The candidate's account head, for an invitation or reset.
        pub account: Option<AccountHead>,
        /// The invitation, for an invited account.
        pub invitation: Option<Id>,
        /// The invitation proof.
        pub invite_proof: Option<Digest>,
        /// The workspace a reset asks to be accepted in.
        pub workspace: Option<Id>,
    }
}

/// What an approval request asks for (§10, §26).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestTarget {
    /// The account's own new device: no invitation, no workspace and no
    /// account head. Only these reach `GET /v2/approval-requests`.
    OwnDevice,
    /// An invited account's device, with the invitation, its proof and the
    /// candidate's account head.
    Invitation,
    /// A reset account's device asking a workspace to accept it, with the
    /// candidate's account head.
    Reset,
}

/// Why the server refuses an approval request's shape with `bad_request`
/// (§26).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestShapeError {
    /// It names both an invitation and a workspace.
    InvitationAndWorkspace,
    /// It carries an invitation proof without its invitation, or an
    /// invitation without its proof.
    ProofWithoutInvitation,
    /// It names neither but carries an account head.
    OwnDeviceWithHead,
    /// It names an invitation or a workspace without the candidate's
    /// account head.
    MissingHead,
}

impl RequestShapeError {
    /// What the refusal says.
    pub fn message(&self) -> &'static str {
        match self {
            RequestShapeError::InvitationAndWorkspace => {
                "a request names at most one of an invitation and a workspace"
            }
            RequestShapeError::ProofWithoutInvitation => {
                "an invitation proof comes with its invitation"
            }
            RequestShapeError::OwnDeviceWithHead => {
                "a request of the account's own device carries no account head"
            }
            RequestShapeError::MissingHead => {
                "an invitation or reset request carries the candidate's account head"
            }
        }
    }
}

impl ApprovalRequest {
    /// What the request asks for, or why its shape gets `bad_request`: it
    /// names at most one of an invitation and a workspace, an invitation
    /// comes with its proof, and only a request that names one of them
    /// carries the candidate's account head (§10, §26).
    pub fn target(&self) -> Result<RequestTarget, RequestShapeError> {
        if self.invitation.is_some() && self.workspace.is_some() {
            return Err(RequestShapeError::InvitationAndWorkspace);
        }
        if self.invite_proof.is_some() != self.invitation.is_some() {
            return Err(RequestShapeError::ProofWithoutInvitation);
        }
        match (self.invitation, self.workspace, &self.account) {
            (None, None, None) => Ok(RequestTarget::OwnDevice),
            (None, None, Some(_)) => Err(RequestShapeError::OwnDeviceWithHead),
            (_, _, None) => Err(RequestShapeError::MissingHead),
            (Some(_), _, Some(_)) => Ok(RequestTarget::Invitation),
            (None, Some(_), Some(_)) => Ok(RequestTarget::Reset),
        }
    }
}

crate::wire_struct! {
    /// `GET /v2/approval-requests` and `GET .../pending-devices` response.
    pub struct PendingResponse {
        /// The requests.
        pub pending: List<Pending>,
    }
}

crate::wire_struct! {
    /// `POST /v2/approvals` request.
    pub struct OfferRequest {
        /// The signed offer.
        pub offer: Nested<Signed<Offer>>,
    }
}

crate::wire_struct! {
    /// `GET /v2/approvals` response.
    pub struct OffersResponse {
        /// Offers addressed to the calling device's card.
        pub offers: List<Nested<Signed<Offer>>>,
    }
}

crate::wire_struct! {
    /// `POST` request and `GET` response of `/v2/approvals/{offer}/reveal`.
    pub struct RevealBody {
        /// The candidate's nonce.
        pub nc: Key,
    }
}

crate::wire_struct! {
    /// `POST /v2/recovery` request.
    pub struct RecoveryRequest {
        /// The signed recovery proof.
        pub recovery_proof: Nested<Signed<RecoveryProof>>,
    }
}

crate::wire_struct! {
    /// `GET /v2/workspaces` response.
    pub struct WorkspacesResponse {
        /// The workspaces.
        pub workspaces: List<WorkspaceInfo>,
    }
}

crate::wire_struct! {
    /// `POST /v2/workspaces/{w}/ops` request, and `POST /v2/workspaces`:
    /// records in order.
    pub struct PushRequest {
        /// The records.
        pub records: List<RecordRef>,
    }
}

crate::wire_struct! {
    /// The response of a push or a workspace creation.
    pub struct PushResponse {
        /// The server's policy head revision.
        pub policy_revision: u64,
        /// One receipt per log entry, in request order.
        pub receipts: List<Receipt>,
    }
}

crate::wire_struct! {
    /// `GET .../policy` response.
    pub struct PolicyResponse {
        /// The head revision.
        pub head_revision: u64,
        /// Changes after `after`, in revision order.
        pub changes: List<Nested<Signed<PolicyChange>>>,
        /// The current head of each member account's chain.
        pub account_heads: Sorted<AccountHead, ByAccount>,
    }
}

crate::wire_struct! {
    /// `GET .../keys` and `GET .../keys/current` response.
    pub struct KeysResponse {
        /// The key sequence to continue from.
        pub head_key_seq: u64,
        /// Key stream records in key-sequence order.
        pub entries: List<KeyEntry>,
    }
}

crate::wire_struct! {
    /// `GET .../epochs/{epoch}` response.
    pub struct EpochsResponse {
        /// Epoch records, newest first.
        pub epochs: List<Nested<Signed<Epoch>>>,
    }
}

crate::wire_struct! {
    /// `POST .../domains/{d}/rewrap` request.
    pub struct RewrapBody {
        /// The epoch to re-wrap.
        pub epoch: u64,
    }
}

crate::wire_struct! {
    /// `POST .../pull` request.
    pub struct PullRequest {
        /// One cursor per domain.
        pub cursors: List<PullCursor>,
        /// How long to wait, at most 25 seconds.
        pub wait_seconds: u32,
    }
}

crate::wire_struct! {
    /// `POST .../pull` response.
    pub struct PullResponse {
        /// One page per domain.
        pub pages: List<Page>,
    }
}

crate::wire_struct! {
    /// `GET .../heads/{device}` response.
    pub struct HeadResponse {
        /// The head's counter.
        pub counter: u64,
        /// The head's digest.
        pub record_digest: Digest,
        /// The head's sequence.
        pub seq: u64,
    }
}

crate::wire_struct! {
    /// `GET .../checkpoint` response.
    pub struct CheckpointResponse {
        /// The checkpoint entry's sequence.
        pub seq: u64,
        /// The bundle.
        pub bundle: CheckpointBundle,
    }
}

crate::wire_struct! {
    /// `GET .../checkpoint-chain` response.
    pub struct CheckpointChainResponse {
        /// Checkpoint records on the walk, newest first.
        pub checkpoints: List<Nested<Signed<Checkpoint>>>,
    }
}

crate::wire_struct! {
    /// `GET .../live` response.
    pub struct LiveResponse {
        /// The checkpoint's `covered_seq`.
        pub at: u64,
        /// The cursor for the next page, absent at the end.
        pub next: Option<LiveCursor>,
        /// Live operations ordered by `rid`, device and counter.
        pub entries: List<Entry>,
    }
}

crate::wire_struct! {
    /// `GET .../manifest`, levels 0 to 2: a node's 16 children.
    pub struct ManifestNodeResponse {
        /// The child hashes.
        pub children: List<Digest, 16>,
    }
}

crate::wire_struct! {
    /// `GET .../manifest`, level 3: a bucket's leaf refs.
    pub struct ManifestLeavesResponse {
        /// Leaf refs sorted by `rid`, device and counter.
        pub leaves: Sorted<LeafRef, ByRidDeviceCounter>,
        /// The cursor for the next page, absent at the end.
        pub next: Option<LiveCursor>,
    }
}

crate::wire_struct! {
    /// `GET .../manifest?buckets=all`: all 4,096 bucket hashes.
    pub struct ManifestBucketsResponse {
        /// The bucket hashes.
        pub buckets: List<Digest, 4096>,
    }
}

crate::wire_struct! {
    /// `GET .../missing-blobs` response.
    pub struct MissingBlobsResponse {
        /// Blobs a restore left missing.
        pub blobs: List<Id>,
    }
}

crate::wire_struct! {
    /// `POST .../invitations` request.
    pub struct CreateInvitationRequest {
        /// The invitation ID.
        pub invitation: Id,
        /// The token's hash.
        pub token_hash: Digest,
        /// The role.
        pub role: Role,
        /// The encrypted context.
        pub invite_context: Nested<InviteContext>,
    }
}

crate::wire_struct! {
    /// The response of creating an invitation or a share link.
    pub struct ExpiresAtResponse {
        /// Unix seconds.
        pub expires_at: u64,
    }
}

crate::wire_struct! {
    /// `GET /v2/invitations/{token}` response.
    pub struct InvitationResponse {
        /// The workspace.
        pub workspace: Id,
        /// The invitation.
        pub invitation: Id,
        /// The role.
        pub role: Role,
        /// Pending or accepted.
        pub status: InvitationStatus,
        /// The encrypted context.
        pub invite_context: Nested<InviteContext>,
    }
}

crate::wire_struct! {
    /// `GET .../invitations` response.
    pub struct InvitationsResponse {
        /// Open invitations.
        pub invitations: List<InvitationInfo>,
    }
}

crate::wire_struct! {
    /// `POST .../shares` request.
    pub struct CreateShareRequest {
        /// The token's hash.
        pub token_hash: Digest,
        /// The domain.
        pub domain: Id,
        /// 1 to 30 days.
        pub expires_in_days: u8,
        /// The signed snapshot.
        pub share: Nested<Signed<Share>>,
    }
}

crate::wire_struct! {
    /// `GET .../shares` response.
    pub struct SharesResponse {
        /// Active links.
        pub shares: List<ShareInfo>,
    }
}

crate::wire_struct! {
    /// `PUT .../shares/{token_hash}` request and `GET /v2/shares/{token}`
    /// response.
    pub struct ShareBody {
        /// The signed snapshot.
        pub share: Nested<Signed<Share>>,
    }
}

/// A realtime frame, `u8 frame_type | bytes body` (§23).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "frame_type", rename_all = "snake_case")]
pub enum Frame {
    /// 1: a domain has new entries.
    Notify {
        /// The domain.
        domain: Id,
        /// Its new head.
        head: u64,
        /// The checkpoint due point, or 0.
        checkpoint_due: u64,
    },
    /// 2: a presence record.
    Presence {
        /// The signed record.
        record: Signed<Presence>,
    },
    /// 3: a presence session ended.
    PresenceGone {
        /// The device.
        device: Id,
        /// The session.
        session: Id,
    },
    /// 4: the policy head moved.
    PolicyChanged {
        /// The new revision.
        revision: u64,
    },
    /// 5: the key stream moved.
    KeysChanged,
    /// 6: committed log entries that fit one frame.
    Records {
        /// The domain.
        domain: Id,
        /// The sequence of the domain's entry just before the first, or 0.
        previous_seq: u64,
        /// The checkpoint due point, or 0.
        checkpoint_due: u64,
        /// The entries, in sequence order.
        entries: Entries,
    },
    /// 7: the presence channels of the notes this device has open.
    Subscribe {
        /// The channels.
        channels: List<Id, MAX_SUBSCRIBE>,
    },
    /// 8: the server's restore generation.
    Restored {
        /// The generation.
        restore_generation: u64,
    },
    /// 9: the server refused a client frame.
    Error {
        /// The refused frame's type.
        rejected_type: u8,
        /// The record's domain, or zero for a subscribe.
        domain: IdOrZero,
        /// The record's session, or zero for a subscribe.
        session: IdOrZero,
        /// The record's sequence, or 0 for a subscribe.
        sequence: u64,
        /// The error code.
        code: Text,
        /// The error's detail.
        detail: Option<Bytes>,
    },
    /// 10: an account that shares this workspace changed its chain.
    AccountChanged {
        /// The account.
        account: Id,
        /// The chain's genesis.
        genesis: Digest,
        /// Its new head seqno.
        seqno: u64,
    },
}

impl Frame {
    /// The frame type byte.
    pub fn code(&self) -> u8 {
        match self {
            Frame::Notify { .. } => 1,
            Frame::Presence { .. } => 2,
            Frame::PresenceGone { .. } => 3,
            Frame::PolicyChanged { .. } => 4,
            Frame::KeysChanged => 5,
            Frame::Records { .. } => 6,
            Frame::Subscribe { .. } => 7,
            Frame::Restored { .. } => 8,
            Frame::Error { .. } => 9,
            Frame::AccountChanged { .. } => 10,
        }
    }

    fn body(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Frame::Notify {
                domain,
                head,
                checkpoint_due,
            } => {
                w.put(domain);
                w.put(head);
                w.put(checkpoint_due);
            }
            Frame::Presence { record } => w.put(record),
            Frame::PresenceGone { device, session } => {
                w.put(device);
                w.put(session);
            }
            Frame::PolicyChanged { revision } => w.put(revision),
            Frame::KeysChanged => {}
            Frame::Records {
                domain,
                previous_seq,
                checkpoint_due,
                entries,
            } => {
                w.put(domain);
                w.put(previous_seq);
                w.put(checkpoint_due);
                w.put(entries);
            }
            Frame::Subscribe { channels } => w.put(channels),
            Frame::Restored { restore_generation } => w.put(restore_generation),
            Frame::Error {
                rejected_type,
                domain,
                session,
                sequence,
                code,
                detail,
            } => {
                w.put(rejected_type);
                w.put(domain);
                w.put(session);
                w.put(sequence);
                w.put(code);
                w.put(detail);
            }
            Frame::AccountChanged {
                account,
                genesis,
                seqno,
            } => {
                w.put(account);
                w.put(genesis);
                w.put(seqno);
            }
        }
        w.into_bytes()
    }

    fn decode_body(code: u8, r: &mut Reader<'_>) -> Result<Option<Frame>, DecodeError> {
        Ok(Some(match code {
            1 => Frame::Notify {
                domain: r.get()?,
                head: r.get()?,
                checkpoint_due: r.get()?,
            },
            2 => Frame::Presence { record: r.get()? },
            3 => Frame::PresenceGone {
                device: r.get()?,
                session: r.get()?,
            },
            4 => Frame::PolicyChanged { revision: r.get()? },
            5 => Frame::KeysChanged,
            6 => Frame::Records {
                domain: r.get()?,
                previous_seq: r.get()?,
                checkpoint_due: r.get()?,
                entries: r.get()?,
            },
            7 => Frame::Subscribe { channels: r.get()? },
            8 => Frame::Restored {
                restore_generation: r.get()?,
            },
            9 => Frame::Error {
                rejected_type: r.get()?,
                domain: r.get()?,
                session: r.get()?,
                sequence: r.get()?,
                code: r.get()?,
                detail: r.get()?,
            },
            10 => Frame::AccountChanged {
                account: r.get()?,
                genesis: r.get()?,
                seqno: r.get()?,
            },
            _ => return Ok(None),
        }))
    }
}

impl Encode for Frame {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.code());
        w.bytes(&self.body());
    }
}

impl Decode for Frame {
    const MIN_SIZE: usize = 1 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let start = r.offset();
        let code = r.u8()?;
        let body = r.bytes(MAX_FRAME)?;
        let base = r.offset() - body.len();
        let mut inner = Reader::new(body);
        let frame = Frame::decode_body(code, &mut inner)
            .and_then(|frame| {
                inner.finish()?;
                Ok(frame)
            })
            .map_err(|mut e| {
                e.offset += base;
                e
            })?;
        frame.ok_or_else(|| unknown(r, start))
    }
}

/// What a client does with a records frame (§23), given its cursor for
/// the frame's domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameAction {
    /// The cursor is at or above `previous_seq` and below the first entry:
    /// verify and apply the entries.
    Apply,
    /// The cursor already covers the frame's last entry.
    Ignore,
    /// A gap, or a cursor inside the frame: pull instead.
    Pull,
}

/// Decides what a client with `cursor` does with a records frame whose
/// first and last entries sit at `first` and `last`.
pub fn records_frame_action(cursor: u64, previous_seq: u64, first: u64, last: u64) -> FrameAction {
    if cursor >= last {
        FrameAction::Ignore
    } else if cursor >= previous_seq && cursor < first {
        FrameAction::Apply
    } else {
        FrameAction::Pull
    }
}

/// Every message body type, by name, for fixtures and fuzzing.
pub const BODY_NAMES: &[&str] = &[
    "limits_response",
    "device_authorize_response",
    "device_token_request",
    "device_token_response",
    "challenge_response",
    "enroll_request",
    "devices_response",
    "chain_append_request",
    "chain_append_response",
    "chains_request",
    "chains_response",
    "reset_request",
    "approval_request",
    "pending_response",
    "offer_request",
    "offers_response",
    "reveal_body",
    "recovery_request",
    "workspaces_response",
    "push_request",
    "push_response",
    "policy_response",
    "keys_response",
    "epochs_response",
    "rewrap_body",
    "pull_request",
    "pull_response",
    "entry",
    "head_response",
    "checkpoint_response",
    "checkpoint_chain_response",
    "live_response",
    "manifest_node_response",
    "manifest_leaves_response",
    "manifest_buckets_response",
    "missing_blobs_response",
    "create_invitation_request",
    "expires_at_response",
    "invitation_response",
    "invitations_response",
    "create_share_request",
    "shares_response",
    "share_body",
    "error_body",
    "chain_conflict_detail",
    "cursor_compacted_detail",
    "push_failure_detail",
    "device_removed_detail",
    "frame",
];

/// The entries of a records frame, sorted by sequence, must start after
/// `previous_seq`; a frame whose first entry doesn't is malformed.
pub fn records_frame_is_well_formed(previous_seq: u64, entries: &[Entry]) -> bool {
    entries
        .first()
        .is_some_and(|first| first.seq > previous_seq)
}
