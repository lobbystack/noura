//! What a folder's journal holds (docs/workspace-format/sync-v2.md §15):
//! the accepted policy chain, the epoch records and this device's envelope
//! still wrapped, the domain's cursor with the verifier's state, this
//! device's chain and the signed records it hasn't seen acknowledged, the
//! map from each `rid` to its local file and the states its operations
//! carried, and the review store's index.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sync_protocol::chain::ChainVerifier;
use sync_protocol::keys::LogJudge;
use sync_protocol::live::LiveSet;
use sync_protocol::records::{Cover, Envelope, Epoch, Operation, PolicyChange, Signed};
use sync_protocol::types::{Digest, Id};

/// The journal format this client writes.
pub const VERSION: u32 = 1;

/// One folder's sync state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    /// The journal format.
    pub version: u32,
    /// The replica ID the registry gave this folder.
    pub replica: Id,
    /// The sync server's address.
    pub server: String,
    /// The account.
    pub account: Id,
    /// The remote workspace.
    pub workspace: Id,
    /// Its workspace domain.
    pub domain: Id,
    /// The last restore generation the server reported.
    pub generation: u64,
    /// The accepted policy chain.
    pub policy: Vec<Signed<PolicyChange>>,
    /// The accepted epoch records of the workspace domain, oldest first.
    pub epochs: Vec<Signed<Epoch>>,
    /// This device's envelope of the latest epoch, still wrapped.
    pub envelope: Option<Signed<Envelope>>,
    /// The workspace's name, from its profile.
    pub name: String,
    /// The latest profile the device accepted (§25).
    pub profile: Option<ProfileMark>,
    /// The domain's log as the device followed it.
    pub log: Log,
    /// This device's chain in the domain.
    pub own: Own,
    /// Every object the device knows, by `rid`.
    pub objects: BTreeMap<Id, Object>,
    /// Signed records not yet acknowledged, in chain order, persisted before
    /// any is sent (§14).
    pub outbox: Vec<Outgoing>,
    /// Records that passed every check but didn't open, as unreadable
    /// versions from their signers (§13).
    pub unreadable: Vec<Unreadable>,
    /// Local versions held for review after a lost journal (§15).
    pub held: BTreeMap<Id, Held>,
    /// Unix seconds of the last round that finished without an error.
    pub last_success: Option<u64>,
    /// Folders whose files wait until you confirm where they go, each at a
    /// local `.noura-domain` file found without a journal (§9).
    #[serde(default)]
    pub held_roots: Vec<String>,
    /// Versions an earlier journal's review store kept (§15).
    #[serde(default)]
    pub archived: Vec<super::rebuild::Archived>,
    /// Objects the device refuses to write, such as one at a
    /// `.noura-domain` path that no domain names as its marker (§9).
    #[serde(default)]
    pub refused: Vec<Id>,
}

/// The profile a device accepted last (§25).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMark {
    /// Its `profile_revision`.
    pub revision: u64,
    /// Its digest.
    pub digest: Digest,
    /// Its `policy_revision`, which a later profile never goes below.
    pub policy_revision: u64,
    /// The device that signed it.
    pub signer: Id,
}

/// The domain's log as the device followed it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Log {
    /// The cursor: every record through it is applied.
    pub cursor: u64,
    /// The highest `head` the server reported in this generation.
    pub head: u64,
    /// The highest `checkpoint_seq` the server reported.
    pub checkpoint_seq: u64,
    /// The highest retention floor the server reported.
    pub retention_floor: u64,
    /// Each record's position against the policy and epoch entries.
    pub judge: LogJudge,
    /// The chain verifier.
    pub verifier: ChainVerifier,
    /// Every `rid`'s live set.
    pub live: LiveSet,
    /// Each device's last `policy_revision` and `generation`, which never
    /// decrease along its chain.
    pub last: BTreeMap<Id, (u64, u64)>,
    /// The policy and epoch entries the device requires in the log, each at
    /// or below the `head` of the first page it requested after it accepted
    /// the change or learned the epoch (§8, §9).
    #[serde(default)]
    pub wanted: Vec<Wanted>,
    /// The highest epoch whose entry the log carried.
    #[serde(default)]
    pub epoch_entries: u64,
}

/// A log entry the device requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wanted {
    /// A policy entry, or an epoch entry.
    pub epoch: bool,
    /// Its revision or epoch.
    pub number: u64,
    /// The head of the first page requested afterwards, or `u64::MAX`
    /// before that page.
    pub by: u64,
}

/// This device's chain in the domain.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Own {
    /// The counter its next new record takes.
    pub next: u64,
    /// The last record it signed, which the next one names as `previous`.
    pub head: Option<(u64, Digest)>,
    /// The last record the server acknowledged: counter, digest, sequence.
    pub acked: Option<(u64, Digest, u64)>,
}

/// An operation's position: its device and counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpKey {
    /// The signing device.
    pub device: Id,
    /// Its counter.
    pub counter: u64,
}

impl Serialize for OpKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{}:{}", self.device.to_hex(), self.counter))
    }
}

impl<'de> Deserialize<'de> for OpKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let (device, counter) = text
            .split_once(':')
            .ok_or_else(|| serde::de::Error::custom("an operation key"))?;
        Ok(Self {
            device: Id::from_hex(device).ok_or_else(|| serde::de::Error::custom("a device"))?,
            counter: counter
                .parse()
                .map_err(|_| serde::de::Error::custom("a counter"))?,
        })
    }
}

/// One object.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Object {
    /// The managed file's frontmatter ID, or empty for an ordinary file.
    pub object_id: String,
    /// Its file on this device, until it's deleted or before it's written.
    pub local: Option<Local>,
    /// The state operation the local file holds, which an edit covers.
    pub base: Option<OpKey>,
    /// The states the object's state operations carried, by position.
    pub states: BTreeMap<OpKey, StateInfo>,
    /// The highest counter this device signed on the object.
    pub own_highest: u64,
    /// Whether this device's next state operation covers every live
    /// branch, as one the server refused with `conflict_limit` does.
    #[serde(default)]
    pub cover_all: bool,
}

/// A file on this device, as sync last left or saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Local {
    /// Where it sits.
    pub path: String,
    /// The revision of its bytes.
    pub revision: Digest,
    /// Its size, modification time and file ID when sync saw it, so an
    /// unchanged file isn't read again.
    pub size: u64,
    pub modified_ns: i64,
    pub file_id: Option<u64>,
}

/// One state operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateInfo {
    /// The operation's digest.
    pub digest: Digest,
    /// Its sequence, once the device saw it in the log.
    pub seq: Option<u64>,
    /// Its covers.
    pub covers: Vec<Cover>,
    /// What it carries, or `None` when it didn't open.
    pub content: Option<Content>,
}

/// What a state operation carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// The path, empty for a delete.
    pub path: String,
    /// The revision of the file's bytes, or of the deleted bytes.
    pub revision: Digest,
    /// Whether it deletes the object.
    pub delete: bool,
    /// The frontmatter ID it names.
    pub object_id: String,
    /// The review store's copy of the bytes, when the device keeps one.
    pub kept: Option<String>,
}

/// A signed record waiting for its acknowledgement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outgoing {
    /// The record, sent byte for byte on every retry.
    pub record: Signed<Operation>,
    /// The re-seals at this position that no newer epoch or policy change
    /// forced (§14).
    pub reseals: u32,
}

/// A record that passed every check but didn't open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unreadable {
    /// The object.
    pub rid: Id,
    /// The signer.
    pub device: Id,
    /// Its counter.
    pub counter: u64,
    /// Its sequence.
    pub seq: u64,
}

/// A local version held for review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// Where the file sits.
    pub path: String,
    /// Its revision.
    pub revision: Digest,
}
