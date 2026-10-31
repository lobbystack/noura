//! What a folder's journal holds (docs/workspace-format/sync-v2.md §15):
//! the accepted policy chain, the epoch records and this device's envelope
//! still wrapped, the domain's cursor with the verifier's state, this
//! device's chain and the signed records it hasn't seen acknowledged, the
//! map from each `rid` to its local file and the states its operations
//! carried, and the review store's index.

use std::collections::{BTreeMap, BTreeSet};

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
    /// Folders whose files wait until you confirm where they go or their
    /// own domain's marker covers them (§9).
    #[serde(default)]
    pub held_roots: Vec<sync_protocol::matching::HeldRoot>,
    /// The domain roots this device recorded (§9).
    #[serde(default)]
    pub roots: Vec<sync_protocol::matching::RecordedRoot>,
    /// The restricted domains the device reads, each with what the fields
    /// above hold for the domain the device works in. While it works in a
    /// restricted domain, that domain's entry holds the workspace domain's
    /// part instead, and [`State::swap`] puts them back.
    #[serde(default)]
    pub restricted: BTreeMap<Id, Frame>,
    /// Notes whose text journal files go once a save of this state reached
    /// disk, because the state no longer names them as text-mode notes.
    #[serde(default)]
    pub ended_text: BTreeSet<Id>,
    /// Versions an earlier journal's review store kept (§15).
    #[serde(default)]
    pub archived: Vec<super::rebuild::Archived>,
    /// Objects the device refuses to write, such as one at a
    /// `.noura-domain` path that no domain names as its marker (§9).
    #[serde(default)]
    pub refused: Vec<Id>,
    /// Whether the device holds its writes after it learned of a removal
    /// the workspace hasn't recorded and rotated past (§9).
    #[serde(default)]
    pub removal_hold: bool,
    /// Epoch records the device accepted that a replacement ended since
    /// (§9), kept to open its own records under them.
    #[serde(default)]
    pub ended_epochs: Vec<Signed<Epoch>>,
    /// This device's envelopes of those epochs, still wrapped, so a restart
    /// reaches their keys while records under them wait to be written
    /// again (§9, §15).
    #[serde(default)]
    pub ended_envelopes: Vec<Signed<Envelope>>,
    /// Those epochs whose records this device still writes again (§9).
    #[serde(default)]
    pub rewrite_epochs: Vec<u64>,
    /// Those epochs whose other devices' live records this device writes
    /// again once `rewrite_due` passes, unless a rewrite covered them
    /// meanwhile (§9).
    #[serde(default)]
    pub rewrite_others: Vec<u64>,
    /// Unix seconds after which the device writes those records again.
    #[serde(default)]
    pub rewrite_due: u64,
    /// Whether the profile sits under an epoch this device accepted and a
    /// replacement skipped, so an owner or admin device signs it again
    /// once `rewrite_due` passes (§9, §25).
    #[serde(default)]
    pub profile_recheck: bool,
    /// The accepted change that took this device's access to the domain,
    /// after which it pulls and pushes nothing there (§15).
    #[serde(default)]
    pub ended: Option<Ended>,
    /// Each object whose reason-2 delete arrived before its move's create,
    /// with the new `rid`: the delete waits for the target domain (§9).
    #[serde(default)]
    pub deferred_moves: BTreeMap<Id, Id>,
    /// Objects whose deferred delete can settle now.
    #[serde(default)]
    pub moved_away: Vec<Id>,
    /// Files a returning device's objects left behind: the manifest lacks
    /// them, and they wait until you confirm their deletion (§18).
    #[serde(default)]
    pub deleted_away: Vec<DeletedAway>,
    /// Unix seconds from which a manager's device finishes the moves of a
    /// folder another device restricted (§9), by domain.
    #[serde(default)]
    pub move_due: BTreeMap<Id, u64>,
    /// The files you moved into a restricted folder, each with its domain,
    /// until you dismiss the warning (§9).
    #[serde(default)]
    pub moved_in: Vec<(Id, String)>,
    /// Whether the device still loads the domain from its latest
    /// checkpoint before it pulls, as a new device does (§18).
    #[serde(default)]
    pub bootstrap: bool,
    /// How far that bootstrap got through the live pages (§18, step 5),
    /// saved with the files of each bucket it installed.
    #[serde(default)]
    pub bootstrap_progress: Option<BootstrapProgress>,
    /// What the device keeps of a server restore (§14).
    #[serde(default)]
    pub restore: super::restore::Restore,
    /// The objects whose changes the server last refused with
    /// `quota_exceeded`, which wait on this device for space (§20).
    #[serde(default)]
    pub waiting_for_space: BTreeSet<Id>,
}

/// One key domain's part of the journal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    /// The domain.
    pub domain: Id,
    /// Its accepted epoch records, oldest first.
    pub epochs: Vec<Signed<Epoch>>,
    /// This device's envelope of the latest epoch, still wrapped.
    pub envelope: Option<Signed<Envelope>>,
    /// Its log as the device followed it.
    pub log: Log,
    /// This device's chain in it.
    pub own: Own,
    /// Signed records not yet acknowledged, in chain order.
    pub outbox: Vec<Outgoing>,
    /// Epoch records a replacement ended since.
    pub ended_epochs: Vec<Signed<Epoch>>,
    /// This device's envelopes of those epochs, still wrapped.
    #[serde(default)]
    pub ended_envelopes: Vec<Signed<Envelope>>,
    /// Those epochs whose records this device still writes again.
    pub rewrite_epochs: Vec<u64>,
    /// Those epochs whose other devices' records it writes again later.
    pub rewrite_others: Vec<u64>,
    /// Unix seconds after which it writes those.
    pub rewrite_due: u64,
    /// Whether the device still loads it from its latest checkpoint.
    pub bootstrap: bool,
    /// How far that bootstrap got.
    pub bootstrap_progress: Option<BootstrapProgress>,
}

impl Frame {
    /// A domain the device hasn't read yet, whose chain starts at `next`.
    pub fn new(domain: Id, next: u64) -> Self {
        Self {
            domain,
            epochs: Vec::new(),
            envelope: None,
            log: Log::new(domain),
            own: Own {
                next,
                head: None,
                acked: None,
            },
            outbox: Vec::new(),
            ended_epochs: Vec::new(),
            ended_envelopes: Vec::new(),
            rewrite_epochs: Vec::new(),
            rewrite_others: Vec::new(),
            rewrite_due: 0,
            bootstrap: true,
            bootstrap_progress: None,
        }
    }
}

impl State {
    /// Exchanges the domain the fields hold with `domain`'s entry. Doing
    /// it twice leaves everything as it was.
    pub fn swap(&mut self, domain: &Id) {
        let Some(frame) = self.restricted.get_mut(domain) else {
            return;
        };
        std::mem::swap(&mut self.domain, &mut frame.domain);
        std::mem::swap(&mut self.epochs, &mut frame.epochs);
        std::mem::swap(&mut self.envelope, &mut frame.envelope);
        std::mem::swap(&mut self.log, &mut frame.log);
        std::mem::swap(&mut self.own, &mut frame.own);
        std::mem::swap(&mut self.outbox, &mut frame.outbox);
        std::mem::swap(&mut self.ended_epochs, &mut frame.ended_epochs);
        std::mem::swap(&mut self.ended_envelopes, &mut frame.ended_envelopes);
        std::mem::swap(&mut self.rewrite_epochs, &mut frame.rewrite_epochs);
        std::mem::swap(&mut self.rewrite_others, &mut frame.rewrite_others);
        std::mem::swap(&mut self.rewrite_due, &mut frame.rewrite_due);
        std::mem::swap(&mut self.bootstrap, &mut frame.bootstrap);
        std::mem::swap(&mut self.bootstrap_progress, &mut frame.bootstrap_progress);
    }
}

/// A bootstrap's progress through the `live` pages of one checkpoint (§18,
/// step 5). The buckets below `next_bucket` are installed: their hashes
/// passed and their files are written, and the domain's live set holds
/// their operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BootstrapProgress {
    /// The checkpoint the pages belong to.
    pub checkpoint: Digest,
    /// The cursor after the last page.
    pub after: Option<sync_protocol::messages::LiveCursor>,
    /// The first bucket not installed yet.
    pub next_bucket: u32,
    /// The operations fetched for buckets not complete yet, with their
    /// sequences.
    pub pending: Vec<(Signed<Operation>, u64)>,
    /// The leaves of the installed buckets.
    pub live_objects: u64,
    /// Each device's highest `policy_revision` and `generation` among the
    /// installed operations.
    pub last: BTreeMap<Id, (u64, u64)>,
}

/// A file whose object was deleted on another device while this one was
/// away (§18). It stays until you confirm, and a capture leaves it alone
/// while its bytes stay these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedAway {
    /// Where it sits.
    pub path: String,
    /// The revision of its bytes.
    pub revision: Digest,
}

/// The change that took this device's access to the domain (§15).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    /// The change's revision.
    pub revision: u64,
    /// The device that signed it.
    pub by_device: Id,
    /// That device's account, when the policy before the change lists it.
    pub by_account: Option<Id>,
    /// Whether the change removed this device's account (`remove_member`).
    pub account_removed: bool,
    /// Whether the change deleted the domain.
    pub domain_deleted: bool,
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
    /// The epoch whose key encrypts it.
    #[serde(default)]
    pub epoch: u64,
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
    /// The rotator of each epoch whose entry the log carried after a
    /// replacement ended it, for judging records under it (§9).
    #[serde(default)]
    pub skipped_rotators: BTreeMap<u64, Id>,
    /// The checkpoints the log carried, and what the next one's checks
    /// need (§17).
    #[serde(default)]
    pub checkpoints: crate::sync::checkpoints::Checkpoints,
    /// The devices whose chain forked, which hold the cursor until a
    /// checkpoint settles the fork (§14), each with the revision of the
    /// latest policy entry before its forked record.
    #[serde(default)]
    pub forked: BTreeMap<Id, u64>,
    /// The sequence of each `create_domain`'s entry the device saw, by
    /// revision: a workspace-domain object below it awaits its move (§9).
    #[serde(default)]
    pub domain_entries: BTreeMap<u64, u64>,
    /// The final head of each device a change took write access from
    /// (§14), which no rewind or generation change alters.
    #[serde(default)]
    pub final_heads: Vec<super::restore::FinalHead>,
    /// Whether a rewind ever took the server's positions here. From then on
    /// a checkpoint's heads answer to the final heads as well (§14). Before
    /// that the client's own heads are the ones it verified, and comparing
    /// with them reports a checkpoint that lists another head (§17).
    #[serde(default)]
    pub rewound: bool,
}

impl Log {
    /// The start of `domain`'s log.
    pub fn new(domain: Id) -> Self {
        Self {
            cursor: 0,
            head: 0,
            checkpoint_seq: 0,
            retention_floor: 0,
            judge: LogJudge::new(domain),
            verifier: ChainVerifier::new(),
            live: LiveSet::new(),
            last: BTreeMap::new(),
            wanted: Vec::new(),
            epoch_entries: 0,
            skipped_rotators: BTreeMap::new(),
            checkpoints: Default::default(),
            forked: BTreeMap::new(),
            domain_entries: BTreeMap::new(),
            final_heads: Vec::new(),
            rewound: false,
        }
    }

    /// Notes each device whose write access to `domain` the change at
    /// `revision`, whose entry sits at `seq`, ended: its chain head there
    /// now is its final head (§14). A later change that gives the access
    /// back closes the head's window, and a generation change alters
    /// neither.
    pub fn note_write_access(
        &mut self,
        domain: &Id,
        revision: u64,
        seq: u64,
        prev: &sync_protocol::policy::Snapshot,
        next: &sync_protocol::policy::Snapshot,
    ) -> Result<(), super::SyncError> {
        for kept in self.final_heads.iter_mut() {
            if kept.until.is_none()
                && kept.revision < revision
                && next.can_write(&kept.device, domain)
            {
                kept.until = Some(seq);
            }
        }
        for device in prev.devices.keys() {
            if prev.can_write(device, domain) && !next.can_write(device, domain) {
                let head = self.verifier.head(device);
                let kept = self
                    .final_heads
                    .iter_mut()
                    .find(|k| k.device == *device && k.revision == revision);
                match kept {
                    // A rewind reached the change's entry again: the head
                    // there must be the one the device kept.
                    Some(kept) => {
                        if (kept.counter, kept.digest) != (head.counter, head.digest) {
                            return Err(super::SyncError::Check("sync_restore_lost"));
                        }
                        kept.entry = seq;
                    }
                    None => self.final_heads.push(super::restore::FinalHead {
                        device: *device,
                        revision,
                        entry: seq,
                        counter: head.counter,
                        digest: head.digest,
                        until: None,
                        records: Vec::new(),
                    }),
                }
            }
        }
        Ok(())
    }

    /// Refuses a record that disagrees with a final head during the pull
    /// (§14): at or below its counter, one other than those the device
    /// fetched by position, and above it, one before the change's entry.
    pub fn check_final_record(
        &self,
        device: &Id,
        counter: u64,
        digest: &Digest,
        seq: u64,
    ) -> Result<(), super::SyncError> {
        for kept in self.final_heads.iter().filter(|k| k.device == *device) {
            if kept.until.is_some_and(|until| until <= seq) {
                continue;
            }
            let refused = if counter <= kept.counter {
                !kept.records.is_empty() && !kept.records.contains(&(counter, *digest))
            } else {
                seq < kept.entry
            };
            if refused {
                return Err(super::SyncError::Check("sync_restore_lost"));
            }
        }
        Ok(())
    }

    /// Refuses a checkpoint that covers a change's entry and lists another
    /// head than the final head kept for it (§14).
    pub fn check_final_heads(
        &self,
        covered: u64,
        heads: &[sync_protocol::records::Ref],
        may_leave: impl Fn(&Id) -> bool,
    ) -> Result<(), super::SyncError> {
        for kept in &self.final_heads {
            if kept.entry > covered || kept.until.is_some_and(|until| until <= covered) {
                continue;
            }
            let agrees = match heads.iter().find(|h| h.device == kept.device) {
                Some(head) => (head.counter, head.op_digest) == (kept.counter, kept.digest),
                None => kept.counter == 0 || may_leave(&kept.device),
            };
            if !agrees {
                return Err(super::SyncError::Check("sync_restore_lost"));
            }
        }
        Ok(())
    }
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
    /// Its restricted domain, or `None` in the workspace domain.
    #[serde(default)]
    pub domain: Option<Id>,
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
    /// Whether the server refused this device's delete of the object, so
    /// the next capture signs it again and nothing writes the file back
    /// until then (§14).
    #[serde(default)]
    pub delete_again: bool,
    /// The text lineage the note's document follows, in text mode (§22).
    #[serde(default)]
    pub text: Option<Id>,
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
    /// The epoch whose key encrypts it.
    #[serde(default)]
    pub epoch: u64,
    /// The policy revision it was signed at, which places it against a
    /// `create_domain` whose log entry the device never pulled (§9).
    #[serde(default)]
    pub policy_revision: u64,
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
    /// The blob that carries the bytes, for a file above the inline limit
    /// (§21). A move inside the domain reuses it.
    #[serde(default)]
    pub blob: Option<sync_protocol::records::BlobRef>,
    /// The text lineage, for a `text_snapshot` (§22).
    #[serde(default)]
    pub text: Option<Id>,
}

/// A signed record waiting for its acknowledgement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outgoing {
    /// The record, sent byte for byte on every retry.
    pub record: Signed<Operation>,
    /// The re-seals at this position that no newer epoch or policy change
    /// forced (§14).
    pub reseals: u32,
    /// Whether it writes again a record under an epoch a replacement
    /// skipped, which keeps its cover or writes nothing (§9).
    #[serde(default)]
    pub rewrite: bool,
    /// The blob the record brought. Its ciphertext stays in the journal at
    /// [`upload_path`] until the server acknowledges the record, since the
    /// collector may delete a completed upload no push listed yet (§14).
    #[serde(default)]
    pub upload: Option<sync_protocol::records::BlobRef>,
    /// Whether that blob's upload completed.
    #[serde(default)]
    pub uploaded: bool,
    /// The move between domains this record is half of, which goes in one
    /// push with its other half (§9).
    #[serde(default)]
    pub pair: Option<MovePair>,
}

/// A move between domains: the object's `rid` in the source domain and
/// the new one in the target (§9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MovePair {
    /// The source `rid`, whose reason-2 delete is one half.
    pub source: Id,
    /// The new `rid`, whose state operation is the other.
    pub target: Id,
}

/// Where the journal keeps a blob's ciphertext until a push lists it.
pub fn upload_path(blob: &sync_protocol::types::Id) -> String {
    format!("uploads/{}", sync_protocol::types::to_hex(&blob.0))
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

#[cfg(test)]
mod final_head_tests {
    use super::*;
    use crate::sync::workspace::restore::FinalHead;
    use sync_protocol::records::Ref;

    fn kept(until: Option<u64>) -> FinalHead {
        FinalHead {
            device: Id([1; 16]),
            revision: 2,
            entry: 10,
            counter: 3,
            digest: Digest([7; 32]),
            until,
            records: Vec::new(),
        }
    }

    fn head(counter: u64, digest: u8) -> Ref {
        Ref {
            device: Id([1; 16]),
            counter,
            seq: 9,
            op_digest: Digest([digest; 32]),
        }
    }

    #[test]
    fn a_checkpoint_past_the_entry_lists_exactly_the_final_head() {
        let mut log = Log::new(Id([2; 16]));
        log.final_heads.push(kept(None));
        let stays = |_: &Id| false;
        assert!(log.check_final_heads(12, &[head(3, 7)], stays).is_ok());
        assert!(
            log.check_final_heads(12, &[], |_| true).is_ok(),
            "a head leaves under §17's rule"
        );
        let dropped = log.check_final_heads(12, &[], stays);
        assert_eq!(
            dropped
                .err()
                .and_then(|e| e.code().map(str::to_owned))
                .as_deref(),
            Some("sync_restore_lost"),
            "a demoted device stays listed"
        );
        assert_eq!(
            log.check_final_heads(12, &[head(4, 8)], stays)
                .err()
                .and_then(|e| e.code().map(str::to_owned))
                .as_deref(),
            Some("sync_restore_lost")
        );
        assert!(
            log.check_final_heads(9, &[head(4, 8)], stays).is_ok(),
            "a checkpoint below the entry doesn't cover it"
        );
    }

    #[test]
    fn a_change_that_gives_access_back_ends_the_window() {
        let mut log = Log::new(Id([2; 16]));
        log.final_heads.push(kept(Some(11)));
        assert!(log.check_final_heads(12, &[head(4, 8)], |_| false).is_ok());
    }

    #[test]
    fn the_pull_takes_no_other_record_at_or_below_a_final_head() {
        let mut log = Log::new(Id([2; 16]));
        log.final_heads.push(FinalHead {
            records: vec![(2, Digest([5; 32])), (3, Digest([7; 32]))],
            ..kept(None)
        });
        let device = Id([1; 16]);
        assert!(
            log.check_final_record(&device, 3, &Digest([7; 32]), 8)
                .is_ok()
        );
        assert!(
            log.check_final_record(&device, 3, &Digest([8; 32]), 8)
                .is_err(),
            "a record signed again"
        );
        assert!(
            log.check_final_record(&device, 4, &Digest([8; 32]), 9)
                .is_err(),
            "above the final head, before the entry"
        );
        assert!(
            log.check_final_record(&device, 4, &Digest([8; 32]), 11)
                .is_ok()
        );
        assert!(
            log.check_final_record(&Id([3; 16]), 4, &Digest([8; 32]), 9)
                .is_ok()
        );
    }

    #[test]
    fn a_zero_final_head_needs_no_listed_head() {
        let mut log = Log::new(Id([2; 16]));
        log.final_heads.push(FinalHead {
            counter: 0,
            digest: Digest::ZERO,
            ..kept(None)
        });
        assert!(log.check_final_heads(12, &[], |_| false).is_ok());
    }
}
