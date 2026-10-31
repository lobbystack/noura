//! Text mode on the desktop client (docs/workspace-format/sync-v2.md §22):
//! a Markdown note whose body lives in a yrs document in the collaboration
//! sandbox, and whose path and properties merge per key beside it.
//!
//! The journal keeps each text-mode note's live records with the sandbox's
//! verdict on each, in ordinary files: a record counts as applied once it
//! reaches them, and the sandbox runs afterward. The note's file follows the
//! document's merged changes through `local-core`'s checked writes, never
//! the editor's. A device opens a note's document on a thread of its own
//! while it edits the note or has records to merge, and hands the editor its
//! side of it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use collab_sandbox::{
    ALLOWANCE_FUEL, Backend, Event, HandleError, JournalRecord, LOAD_BLOCK, LineageStatus, Loader,
    NoteHandle, Opened, Place, REFILL_PERIOD, RemoteRecord, Sandbox, SandboxDocument, SharedNote,
};
use serde::{Deserialize, Serialize};
use sync_protocol::codec::{Bytes, Decode, Encode, Sorted, Text};
use sync_protocol::crypto::hash;
use sync_protocol::ids::{text_lineage_id, yrs_client_id};
use sync_protocol::matching::Placement;
use sync_protocol::records::{
    Content, Cover, MAX_PAYLOAD_LEN, Meta, OpClass, Operation, Payload, PayloadKind, Signed,
};
use sync_protocol::types::{Digest, Id};
use workspace_format::text_note::{NoteParts, compose_note, note_parts};

use super::settle::Body;
use super::state::{Content as Carried, OpKey};
use super::{CheckedRecord, Found, SyncError, WorkspaceSync, core};

/// How often the client sends local edits and presence while a note is
/// open.
pub const TICK: Duration = Duration::from_millis(500);
/// How long a note rests before the device that wrote its latest update
/// snapshots it.
pub const IDLE: Duration = Duration::from_secs(30);
/// How long a note rests before any writer device snapshots it.
pub const IDLE_ANY_WRITER: Duration = Duration::from_secs(300);
/// How old a device's last pull may be for it to write a snapshot on its own
/// judgement (§22), above the time between two rounds on a live channel.
const PULLED_WITHIN: Duration = Duration::from_secs(120);
/// The shortest time between two snapshots of a note from one device, unless
/// the server requires one.
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(10);
/// The live patches a note may hold when it rests.
pub const IDLE_PATCHES: usize = 8;
/// The live patches past which the designated device snapshots a note.
pub const VOLUME_PATCHES: usize = 512;
/// The smallest and largest byte threshold of the volume rule.
const VOLUME_BYTES: (u64, u64) = (1 << 20, 2 << 20);
/// The UTF-8 bytes of text past which your edit takes a note out of text
/// mode.
pub const TEXT_LIMIT: usize = 4 << 20;
/// The encoded document at which a note rebases.
pub const REBASE_AT: usize = 16 << 20;
/// A device's own records carry IDs above every log sequence until the log
/// gives them one.
const OWN_RECORDS: u64 = 1 << 62;
/// The counter of a snapshot the server had no space for. No signed record
/// has it, so the record that takes the snapshot's counter finds its own
/// entry in the journal.
const HELD: u64 = 0;
/// How many review entries the journal keeps for one note. A note whose
/// lineage stopped keeps that entry past the limit.
const MAX_REVIEWS: usize = 64;
/// How often an open note's writer balances reach the journal.
const BALANCES_EVERY: Duration = Duration::from_secs(60);
/// An update that carries no change to the document.
const EMPTY_UPDATE: [u8; 2] = [0, 0];
/// The payload's fields around a text record's bytes and `meta` entries.
const PAYLOAD_OVERHEAD: usize = 128;

/// The one sandbox a process compiles, which takes a moment.
fn sandbox() -> Result<&'static Sandbox, SyncError> {
    static SANDBOX: OnceLock<Result<Sandbox, String>> = OnceLock::new();
    SANDBOX
        .get_or_init(|| Sandbox::new(Backend::Cranelift).map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| SyncError::Invalid(format!("the collaboration sandbox: {error}")))
}

fn handle_error(error: HandleError) -> SyncError {
    SyncError::Invalid(format!("the note's document: {error}"))
}

/// The sandbox's verdict on a record, as the journal keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Judgement {
    /// The record reached the journal and the sandbox hasn't judged it.
    Waiting,
    /// It merged, or this device wrote it.
    Readable,
    /// The verdict stage judged it unreadable.
    Unreadable,
    /// It holds an item that would land elsewhere on a booted device (§22).
    Refused,
    /// A snapshot that made the device rebuild the document. It stays one
    /// of the lineage's live records, so a later rebuild starts from it.
    Disagrees,
    /// An update of a lineage the note left: it went to review as text.
    Ended,
}

/// A `meta` entry of a record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaEntry {
    /// `path`, or `p:` and a property's name.
    pub key: String,
    /// The sequence it names.
    pub seq: u64,
    /// Its value, or `None` for a removal.
    pub value: Option<Vec<u8>>,
}

/// One live record of a text-mode note.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextRecord {
    /// Its position in its signer's chain.
    pub key: OpKey,
    /// Its log sequence, once the device saw it in the log.
    pub seq: Option<u64>,
    /// Its digest.
    pub digest: Digest,
    /// Whether it's a `text_snapshot`.
    pub snapshot: bool,
    /// The lineage it names.
    pub lineage: Id,
    /// The signer's card index.
    pub writer: u32,
    /// The cards the workspace indexed at its `policy_revision`.
    pub card_count: u32,
    /// Its covers.
    pub covers: Vec<Cover>,
    /// What the server counts for it: its ciphertext, blob included.
    pub weight: u64,
    /// The epoch it's sealed under.
    pub epoch: u64,
    /// Its `meta` entries.
    pub meta: Vec<MetaEntry>,
    /// The hash of its payload's bytes, which name its file in the journal.
    pub payload: Digest,
    /// The hash of its blob's file, when its bytes travel in one.
    pub blob: Option<Digest>,
    /// The sandbox's verdict.
    pub judgement: Judgement,
}

impl TextRecord {
    /// The record's name in the sandbox: its log sequence, or a number past
    /// every sequence.
    fn id(&self) -> u64 {
        self.seq.unwrap_or(OWN_RECORDS + self.key.counter)
    }

    fn place(&self) -> Place {
        Place {
            device: self.key.device,
            counter: self.key.counter,
            covers: self.covers.clone(),
        }
    }
}

/// Why a text-mode note waits in the review store (§22).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextReviewKind {
    /// A record judged unreadable.
    Unreadable,
    /// A record whose item would land elsewhere on a booted device.
    Refused,
    /// A snapshot or update that disagrees with what the device holds:
    /// `sync_text_conflict`.
    Disagrees,
    /// Updates that wait on a record that never merged.
    Pending,
    /// The lineage stopped on this device, and the note waits for your
    /// choice.
    Stopped,
    /// An update of a lineage that isn't live any more.
    EndedLineage,
    /// A merged path outside the note's domain root.
    OutsideRoot,
}

/// A text-mode note's entry in the review store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextReview {
    /// The note.
    pub rid: Id,
    /// Why it waits.
    pub kind: TextReviewKind,
    /// The record's signer, when one record is at fault.
    pub signer: Option<Id>,
    /// The record's log sequence, when one record is at fault.
    pub seq: Option<u64>,
}

/// What the note's file held when this device last wrote or read it: the
/// base an outside edit diffs from, so that a change the document merged
/// meanwhile survives the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBase {
    /// The hash of the file's bytes, which name their copy in the journal.
    pub bytes: Digest,
    /// The hash of the canonical form of those bytes, when the file may keep
    /// its own form while the note's merged state matches it.
    pub form: Option<Digest>,
}

/// What the journal keeps of one text-mode note.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextFile {
    /// The lineage the note's document follows.
    pub lineage: Id,
    /// Its live records, and covered ones the sandbox still compares
    /// snapshots against, in the order they reached the journal.
    pub records: Vec<TextRecord>,
    /// Each writer's fuel balance when the note last closed.
    pub balances: Vec<(u32, i64)>,
    /// Unix seconds when those balances last took a refill.
    #[serde(default)]
    pub balances_at: u64,
    /// Whether the lineage stopped on this device.
    pub stopped: bool,
    /// The first record that yrs held pending, while one waits.
    pub pending_since: Option<u64>,
    /// The note's entries in the review store.
    pub reviews: Vec<TextReview>,
    /// What the note's file last held.
    #[serde(default)]
    pub base: Option<FileBase>,
}

impl TextFile {
    fn new(lineage: Id) -> Self {
        Self {
            lineage,
            records: Vec::new(),
            balances: Vec::new(),
            balances_at: 0,
            stopped: false,
            pending_since: None,
            reviews: Vec::new(),
            base: None,
        }
    }

    /// Lists an entry in the review store, once. The list stays bounded,
    /// so a hostile writer can't grow it without limit.
    fn review(&mut self, entry: TextReview) {
        if self.reviews.contains(&entry) {
            return;
        }
        self.reviews.push(entry);
        if self.reviews.len() > MAX_REVIEWS
            && let Some(oldest) = self
                .reviews
                .iter()
                .position(|r| r.kind != TextReviewKind::Stopped)
        {
            self.reviews.remove(oldest);
        }
    }

    /// Whether a readable snapshot of the lineage holds the note's start,
    /// so that its document holds the note.
    fn holds_a_snapshot(&self) -> bool {
        self.records.iter().any(|r| {
            r.snapshot
                && r.lineage == self.lineage
                && matches!(r.judgement, Judgement::Readable | Judgement::Disagrees)
        })
    }
}

/// Why an editor's hold on a note's document ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftWhy {
    /// A file operation ended text mode: the note saves as a plain file.
    Ended,
    /// The lineage stopped on this device, so the note waits for your
    /// choice and its file keeps its last good state.
    Stopped,
    /// The note's document moved on to another lineage, or the editor and
    /// the document went apart: the editor attaches to the note's document
    /// again.
    Moved,
}

/// An editor's document that no longer takes the note's edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftNote {
    /// The note.
    pub rid: Id,
    /// Why.
    pub why: LeftWhy,
    /// The text the document last merged, which the file followed.
    pub body: String,
}

/// One merged `meta` key: its latest write.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MetaWrite {
    value: Option<Vec<u8>>,
    /// The write's effective sequence, or `u64::MAX` for this device's own
    /// write the log hasn't committed yet.
    effective: u64,
    digest: Digest,
}

/// The note's merged `meta`: each key's latest write among the records that
/// passed the verdict, by effective sequence and then record digest (§22).
fn merged_meta(records: &[TextRecord]) -> BTreeMap<String, MetaWrite> {
    let mut merged: BTreeMap<String, MetaWrite> = BTreeMap::new();
    for record in records {
        if record.judgement != Judgement::Readable && record.judgement != Judgement::Disagrees {
            continue;
        }
        for entry in &record.meta {
            let own = record.seq.unwrap_or(u64::MAX);
            let effective = if record.snapshot && entry.seq != 0 && entry.seq <= own {
                entry.seq
            } else {
                own
            };
            let write = MetaWrite {
                value: entry.value.clone(),
                effective,
                digest: record.digest,
            };
            let later = merged.get(&entry.key).is_none_or(|current| {
                (write.effective, write.digest) > (current.effective, current.digest)
            });
            if later {
                merged.insert(entry.key.clone(), write);
            }
        }
    }
    merged
}

/// The path and properties a merged `meta` holds.
fn note_meta(merged: &BTreeMap<String, MetaWrite>) -> (Option<String>, BTreeMap<String, String>) {
    let path = merged
        .get("path")
        .and_then(|write| write.value.as_ref())
        .and_then(|value| String::from_utf8(value.clone()).ok());
    let properties = merged
        .iter()
        .filter_map(|(key, write)| {
            let name = key.strip_prefix("p:")?;
            let value = String::from_utf8(write.value.clone()?).ok()?;
            Some((name.to_owned(), value))
        })
        .collect();
    (path, properties)
}

fn meta_list(entries: &[MetaEntry]) -> Result<sync_protocol::records::MetaList, SyncError> {
    let mut list = Vec::with_capacity(entries.len());
    for entry in entries {
        list.push(Meta {
            key: Text::new(entry.key.clone())
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            seq: entry.seq,
            value: entry.value.clone().map(Bytes),
        });
    }
    Sorted::new(list).map_err(|e| SyncError::Invalid(e.name().to_owned()))
}

fn entries_of(meta: &sync_protocol::records::MetaList) -> Vec<MetaEntry> {
    meta.iter()
        .map(|entry| MetaEntry {
            key: entry.key.as_str().to_owned(),
            seq: entry.seq,
            value: entry.value.as_ref().map(|value| value.0.clone()),
        })
        .collect()
}

/// What the snapshot rules of §22 look at.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotFacts {
    /// The note's live patches.
    pub patches: usize,
    /// Their ciphertext, blobs included.
    pub bytes: u64,
    /// The encoded size of the note's last snapshot.
    pub snapshot_size: u64,
    /// Whether this device has the lowest device ID among the patch
    /// holders.
    pub designated: bool,
    /// Whether this device wrote the note's latest update.
    pub wrote_latest: bool,
    /// How long the note has had no edit.
    pub idle: Duration,
    /// Whether you closed the note since your last edit.
    pub closed: bool,
    /// How long ago this device last snapshotted the note.
    pub since_snapshot: Option<Duration>,
    /// Whether the server answered `snapshot_required`.
    pub required: bool,
}

/// Whether this device writes a snapshot now (§22).
pub fn snapshot_due(facts: &SnapshotFacts) -> bool {
    if facts.required {
        return true;
    }
    if facts
        .since_snapshot
        .is_some_and(|since| since < SNAPSHOT_EVERY)
    {
        return false;
    }
    let threshold = (facts.snapshot_size / 4).clamp(VOLUME_BYTES.0, VOLUME_BYTES.1);
    let past = facts.patches > VOLUME_PATCHES || facts.bytes > threshold;
    let far_past = facts.patches > 2 * VOLUME_PATCHES || facts.bytes > 2 * threshold;
    let rested = facts.wrote_latest && (facts.idle >= IDLE || facts.closed);
    (past && facts.designated)
        || far_past
        || (facts.patches > IDLE_PATCHES && (rested || facts.idle >= IDLE_ANY_WRITER))
}

/// How a request to edit a note in text mode turned out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entered {
    /// The note is in text mode.
    Yes,
    /// No live `file` operation matches the note's file yet, so its edits
    /// sync as `file` operations until one does.
    NotYet,
    /// The note stays out of text mode: its file isn't a Markdown note whose
    /// frontmatter passes the property rules.
    Never,
}

/// Load numbers this device reserved and hasn't used (§22).
#[derive(Debug, Default)]
struct Loads {
    next: u64,
    reserved: u64,
}

/// A note whose document is open.
struct Session {
    /// The editor's side and the text it opened with, until the editor
    /// takes it.
    handle: Option<(NoteHandle, String)>,
    shared: SharedNote,
    /// The text the document last merged, which the file follows.
    body: String,
    /// The load the document's first client ID names.
    load: u32,
    /// The records handed to the sandbox.
    sent: BTreeSet<u64>,
    /// Where the sandbox's loader reads deferred records.
    records: Arc<Mutex<BTreeMap<u64, Stored>>>,
    /// Whether an editor holds the note.
    editing: bool,
    /// `path` and property writes that wait for the next update.
    meta: Vec<MetaEntry>,
    /// When the journal last took each writer's balance.
    balances_saved: Instant,
}

/// What the sandbox's loader needs to read a deferred record back.
#[derive(Clone)]
struct Stored {
    writer: u32,
    card_count: u32,
    place: Place,
    payload: std::path::PathBuf,
    blob: Option<std::path::PathBuf>,
}

/// When things last happened to a note on this device.
#[derive(Clone, Copy, Debug)]
struct Activity {
    last_edit: Instant,
    last_snapshot: Option<Instant>,
    /// Whether the note closed since its last edit.
    closed: bool,
}

/// What a folder's sync client holds in memory for text mode.
#[derive(Default)]
pub struct TextRuntime {
    loads: Arc<Mutex<Loads>>,
    sessions: BTreeMap<Id, Session>,
    activity: BTreeMap<Id, Activity>,
    /// Records that the next settle judges.
    unsettled: BTreeSet<Id>,
    /// Notes the server required a snapshot of.
    required: BTreeSet<Id>,
    /// Notes with updates of this device that a kill left in no outbox,
    /// which it signs again.
    lost: BTreeSet<Id>,
    /// When a pull last took this device to the log's head in every domain.
    pulled: Option<Instant>,
    /// How old that pull may be for a snapshot rule to run, when a test
    /// sets it.
    pulled_within: Option<Duration>,
    /// Codes reported to the app since it last asked, such as
    /// `sync_text_conflict`.
    reported: Vec<&'static str>,
    /// Documents editors still hold that stopped taking the notes' edits.
    left: Vec<LeftNote>,
    /// The encoded size at which a note rebases, when a test lowers it.
    rebase_at: Option<usize>,
}

impl std::fmt::Debug for TextRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextRuntime")
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

/// `balances` with the refills that came due since `at`, while the note
/// was closed.
fn refilled(balances: &[(u32, i64)], at: u64) -> Vec<(u32, i64)> {
    let periods = super::unix_now().saturating_sub(at) / REFILL_PERIOD.as_secs();
    let added = ALLOWANCE_FUEL.saturating_mul(i64::try_from(periods).unwrap_or(i64::MAX));
    balances
        .iter()
        .map(|(writer, balance)| (*writer, balance.saturating_add(added).min(ALLOWANCE_FUEL)))
        .collect()
}

/// The hash of the canonical form of a note's file bytes.
fn note_form(path: &str, bytes: &[u8]) -> Option<Digest> {
    let (properties, body) = match note_parts(path, bytes) {
        NoteParts::Managed { properties, body } => (properties, body),
        NoteParts::Unmanaged { body } => (BTreeMap::new(), body),
        NoteParts::Outside => return None,
    };
    let composed = compose_note(&properties, &body).ok()?;
    Some(hash(composed.as_bytes()))
}

fn text_name(rid: &Id) -> String {
    format!("text-{}.json", rid.to_hex())
}

/// `payload` as the journal keeps it: the blob's bytes sit beside it, so a
/// note needs no blob key at rest, and none stays in a file of the folder
/// (§15, §21).
fn without_blob_key(payload: &Payload) -> Payload {
    let mut kept = payload.clone();
    if let PayloadKind::TextSnapshot {
        content: Content::Blob(blob),
        ..
    }
    | PayloadKind::TextUpdate {
        content: Content::Blob(blob),
        ..
    } = &mut kept.kind
    {
        blob.blob_key = sync_protocol::types::SecretKey([0; 32]);
    }
    kept
}

fn record_path(rid: &Id, digest: &Digest) -> String {
    format!("text/{}/{}", rid.to_hex(), digest.to_hex())
}

impl WorkspaceSync {
    /// What the journal keeps of the note, when it's in text mode.
    pub fn text_file(&self, rid: &Id) -> Result<Option<TextFile>, SyncError> {
        if self.state.ended_text.contains(rid) {
            return Ok(None);
        }
        Ok(self.journal.read(&text_name(rid))?)
    }

    fn store_text(&mut self, rid: &Id, file: &TextFile) -> Result<(), SyncError> {
        self.state.ended_text.remove(rid);
        Ok(self.journal.write(&text_name(rid), file)?)
    }

    /// Removes the text journal files of the notes that left text mode, now
    /// that a saved state no longer needs them.
    pub(super) fn forget_ended_text(&mut self) {
        for rid in std::mem::take(&mut self.state.ended_text) {
            self.journal.forget(&text_name(&rid));
            self.journal
                .forget_folder(&format!("text/{}", rid.to_hex()));
        }
    }

    /// Brings the text journal files in line with the state file after a
    /// start: they commit separately, so a kill can leave them apart. A
    /// note with no file left is a plain file. A record of this device in
    /// no outbox never went out: a snapshot's document lives on in the
    /// records it holds, and an update goes out again.
    pub(super) fn reconcile_text(&mut self) -> Result<(), SyncError> {
        self.forget_ended_text();
        let mut rids = Vec::new();
        for name in self.journal.names_with("text-") {
            let hex = name
                .strip_prefix("text-")
                .and_then(|name| name.strip_suffix(".json"));
            rids.extend(hex.and_then(Id::from_hex));
        }
        for (rid, object) in &mut self.state.objects {
            if !rids.contains(rid) {
                object.text = None;
            }
        }
        for rid in rids {
            let Some(mut file) = self.text_file(&rid)? else {
                continue;
            };
            let domain = self.state.objects.get(&rid).and_then(|o| o.domain);
            let lost = self.in_domain(domain, |sync| sync.lost_records(&file));
            if lost
                .iter()
                .any(|key| file.records.iter().any(|r| r.key == *key && !r.snapshot))
            {
                self.text.lost.insert(rid);
            }
            let snapshot = |r: &TextRecord| r.snapshot && lost.contains(&r.key);
            if file.records.iter().any(snapshot) {
                let (gone, kept) = std::mem::take(&mut file.records)
                    .into_iter()
                    .partition(snapshot);
                file.records = kept;
                self.release_records(rid, &file, gone);
                self.store_text(&rid, &file)?;
            }
        }
        Ok(())
    }

    /// This device's records in `file` that never left it and that no
    /// outbox of the domain the device works in holds.
    fn lost_records(&self, file: &TextFile) -> BTreeSet<OpKey> {
        let own = self.credentials.keys.device();
        let acked = self.state.own.acked.map_or(0, |(counter, _, _)| counter);
        file.records
            .iter()
            .filter(|r| {
                r.key.device == own
                    && r.seq.is_none()
                    && r.key.counter != HELD
                    && r.key.counter > acked
                    && !self
                        .state
                        .outbox
                        .iter()
                        .any(|o| o.record.body.counter == r.key.counter)
            })
            .map(|r| r.key)
            .collect()
    }

    /// Signs a dropped text update again once the device can write.
    pub(super) fn resend_text_later(&mut self, record: &Signed<Operation>) {
        if self.is_text_update(record) {
            self.text.lost.insert(record.body.rid);
        }
    }

    /// Signs again the updates a kill left out of every outbox. Returns how
    /// many records it signed.
    fn resend_lost_text(&mut self) -> Result<usize, SyncError> {
        if self.state.removal_hold || self.state.ended.is_some() {
            return Ok(0);
        }
        let mut signed = 0;
        for rid in self.text.lost.clone() {
            if !self.can_write_note(&rid) {
                continue;
            }
            let domain = self.state.objects.get(&rid).and_then(|o| o.domain);
            signed += self.in_domain(domain, |sync| sync.resend_note(rid))?;
            self.text.lost.remove(&rid);
        }
        Ok(signed)
    }

    fn resend_note(&mut self, rid: Id) -> Result<usize, SyncError> {
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(0);
        };
        let lost = self.lost_records(&file);
        let (mut resent, kept): (Vec<TextRecord>, Vec<TextRecord>) =
            std::mem::take(&mut file.records)
                .into_iter()
                .partition(|r| lost.contains(&r.key));
        file.records = kept;
        resent.sort_by_key(|r| r.key.counter);
        let signed = resent.len();
        for record in resent {
            let bytes = self
                .journal
                .kept(&record_path(&rid, &record.payload), &record.payload)?;
            let payload = Payload::from_bytes(&bytes)
                .map_err(|_| SyncError::Invalid("a kept text record didn't read".into()))?;
            let PayloadKind::TextUpdate { load, content, .. } = payload.kind else {
                continue;
            };
            let yrs = match (content, record.blob) {
                (Content::Inline { bytes }, _) => bytes.0,
                (_, Some(blob)) => self.journal.kept(&record_path(&rid, &blob), &blob)?,
                (_, None) => continue,
            };
            self.sign_update(rid, &mut file, load, yrs, &record.meta)?;
        }
        self.store_text(&rid, &file)?;
        Ok(signed)
    }

    /// The note at `path`, by its object.
    pub fn text_rid(&self, path: &str) -> Option<Id> {
        self.rid_at(path)
    }

    /// Whether the object is in text mode on this device.
    pub fn in_text_mode(&self, rid: &Id) -> bool {
        self.state
            .objects
            .get(rid)
            .is_some_and(|object| object.text.is_some())
    }

    /// The body the note's document merged, for a note whose document is
    /// open.
    pub fn text_body(&self, rid: &Id) -> Option<&str> {
        self.text.sessions.get(rid).map(|s| s.body.as_str())
    }

    /// Where the note's lineage stands, for a note whose document is open.
    pub fn text_status(&self, rid: &Id) -> Option<LineageStatus> {
        self.text.sessions.get(rid).map(|s| s.shared.status())
    }

    /// Every text-mode note's entries in the review store.
    pub fn text_reviews(&self) -> Vec<TextReview> {
        self.state
            .objects
            .iter()
            .filter(|(_, object)| object.text.is_some())
            .filter_map(|(rid, _)| self.text_file(rid).ok().flatten())
            .flat_map(|file| file.reviews)
            .collect()
    }

    /// The codes reported since the last call, such as
    /// `sync_text_conflict`.
    pub fn take_text_reports(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.text.reported)
    }

    /// The documents editors hold that stopped taking the notes' edits
    /// since the last call.
    pub fn take_left_notes(&mut self) -> Vec<LeftNote> {
        std::mem::take(&mut self.text.left)
    }

    /// Tells the note's editor, if one holds its document, that the
    /// document takes no more of its edits.
    fn editor_left(&mut self, rid: Id, why: LeftWhy) {
        if let Some(session) = self.text.sessions.get(&rid)
            && session.editing
        {
            self.text.left.push(LeftNote {
                rid,
                why,
                body: session.body.clone(),
            });
        }
    }

    /// Lowers the encoded size at which a note rebases, because no test
    /// builds a real 16 MiB document.
    #[cfg(feature = "test-hooks")]
    pub fn set_rebase_at(&mut self, bytes: usize) {
        self.text.rebase_at = Some(bytes);
    }

    /// Keeps `bytes`, what the note's file holds now, as the base the next
    /// outside edit diffs from. `form` says the file may keep its form
    /// while the merged state matches it.
    fn set_base(
        &mut self,
        rid: Id,
        file: &mut TextFile,
        bytes: &[u8],
        form: Option<Digest>,
    ) -> Result<(), SyncError> {
        let digest = hash(bytes);
        let old = file.base.map(|base| base.bytes);
        file.base = Some(FileBase {
            bytes: digest,
            form,
        });
        if old == Some(digest) {
            return Ok(());
        }
        self.journal.keep(&record_path(&rid, &digest), bytes)?;
        if let Some(old) = old
            && !file
                .records
                .iter()
                .any(|r| r.payload == old || r.blob == Some(old))
        {
            self.journal.forget(&record_path(&rid, &old));
        }
        Ok(())
    }

    /// What the note's file last held, as a note's parts.
    fn base_parts(
        &self,
        rid: &Id,
        file: &TextFile,
        path: &str,
    ) -> Option<(BTreeMap<String, String>, String)> {
        let base = file.base?;
        let bytes = self
            .journal
            .kept(&record_path(rid, &base.bytes), &base.bytes)
            .ok()?;
        match note_parts(path, &bytes) {
            NoteParts::Managed { properties, body } => Some((properties, body)),
            NoteParts::Unmanaged { body } => Some((BTreeMap::new(), body)),
            NoteParts::Outside => None,
        }
    }

    /// Reserves load numbers when few are left, so the note threads, which
    /// can't reach the credential store, never run out while they rebuild.
    fn reserve_loads(&mut self) -> Result<(), SyncError> {
        let mut loads = self.text.loads.lock().unwrap_or_else(|p| p.into_inner());
        if loads.next == 0 {
            loads.next = self.mark.reserved_load + 1;
            loads.reserved = self.mark.reserved_load;
        }
        if loads.next + u64::from(LOAD_BLOCK) / 4 > loads.reserved {
            loads.reserved += u64::from(LOAD_BLOCK);
            self.mark.reserved_load = loads.reserved;
            drop(loads);
            self.mark.write(
                &*self.secrets,
                &self.credentials.keys.device(),
                &self.state.workspace,
            )?;
        }
        Ok(())
    }

    /// Hands out client IDs from the reserved load numbers.
    fn client_ids(&mut self) -> Result<(collab_sandbox::ClientIds, u32), SyncError> {
        self.reserve_loads()?;
        let card = self.card_index()?;
        let loads = self.text.loads.clone();
        let first = {
            let mut loads = loads.lock().unwrap_or_else(|p| p.into_inner());
            let load = loads.next;
            loads.next += 1;
            load as u32
        };
        let mut pending = Some(first);
        let client_ids: collab_sandbox::ClientIds = Box::new(move || {
            let load = match pending.take() {
                Some(load) => load,
                None => {
                    let mut loads = loads.lock().unwrap_or_else(|p| p.into_inner());
                    if loads.next > loads.reserved {
                        return None;
                    }
                    let load = loads.next;
                    loads.next += 1;
                    load as u32
                }
            };
            yrs_client_id(card, load)
        });
        Ok((client_ids, first))
    }

    fn card_index(&self) -> Result<u32, SyncError> {
        self.card_of(&self.credentials.keys.device(), None)
            .map(|(index, _)| index)
    }

    /// `device`'s card index, and how many cards the workspace indexed, at
    /// `revision`, or at the head.
    fn card_of(&self, device: &Id, revision: Option<u64>) -> Result<(u32, u32), SyncError> {
        let remote = self
            .remote
            .as_ref()
            .ok_or(SyncError::Check("sync_workspace_unverified"))?;
        let snapshot = match revision {
            Some(revision) => remote
                .policy
                .at(revision)
                .ok_or(SyncError::Check("sync_writer_not_authorized"))?,
            None => remote.policy.head_snapshot(),
        };
        let index = snapshot
            .card_indices
            .get(device)
            .copied()
            .ok_or(SyncError::Check("sync_writer_not_authorized"))?;
        Ok((index, snapshot.card_indices.len() as u32))
    }

    fn can_write_note(&self, rid: &Id) -> bool {
        let device = self.credentials.keys.device();
        let domain = self
            .state
            .objects
            .get(rid)
            .and_then(|object| object.domain)
            .unwrap_or(self.remote.as_ref().map_or(self.state.domain, |r| r.domain));
        self.remote
            .as_ref()
            .is_some_and(|r| r.policy.head_snapshot().can_write(&device, &domain))
            && self.stopped.is_none()
    }

    /// The readable records of the note's lineage, in canonical order: each
    /// live snapshot by log sequence, then each update, this device's own
    /// records not yet in the log last.
    fn canonical(&self, rid: &Id, file: &TextFile) -> Result<Vec<JournalRecord>, SyncError> {
        let mut records: Vec<&TextRecord> = file
            .records
            .iter()
            .filter(|r| {
                r.lineage == file.lineage
                    && matches!(r.judgement, Judgement::Readable | Judgement::Disagrees)
            })
            .collect();
        records.sort_by_key(|r| (!r.snapshot, r.id()));
        let mut out = Vec::with_capacity(records.len());
        for record in records {
            let Some(bytes) = self.record_yrs(rid, record)? else {
                continue;
            };
            out.push(JournalRecord {
                id: record.id(),
                snapshot: record.snapshot,
                place: record.place(),
                bytes,
            });
        }
        Ok(out)
    }

    /// The yrs bytes a record holds, from the journal.
    fn record_yrs(&self, rid: &Id, record: &TextRecord) -> Result<Option<Vec<u8>>, SyncError> {
        use sync_protocol::codec::Decode;
        let payload = self
            .journal
            .kept(&record_path(rid, &record.payload), &record.payload)?;
        let Ok(payload) = Payload::from_bytes(&payload) else {
            return Ok(None);
        };
        let content = match payload.kind {
            PayloadKind::TextSnapshot { content, .. } | PayloadKind::TextUpdate { content, .. } => {
                content
            }
            _ => return Ok(None),
        };
        match (content, &record.blob) {
            (Content::Inline { bytes }, _) => Ok(Some(bytes.0)),
            (Content::Blob(_), Some(blob)) => {
                Ok(Some(self.journal.kept(&record_path(rid, blob), blob)?))
            }
            (Content::Blob(_), None) => Ok(None),
        }
    }

    /// Opens the note's document, loading it from the journal.
    fn open_session(&mut self, rid: &Id) -> Result<(), SyncError> {
        if self.text.sessions.contains_key(rid) {
            return Ok(());
        }
        let file = self
            .text_file(rid)?
            .ok_or_else(|| SyncError::Invalid("the note isn't in text mode".into()))?;
        let records = self.canonical(rid, &file)?;
        let (client_ids, load) = self.client_ids()?;
        let stored: Arc<Mutex<BTreeMap<u64, Stored>>> = Arc::default();
        let source = stored.clone();
        let loader: Loader = Box::new(move |id| {
            let stored = source
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&id)
                .cloned()?;
            Some(RemoteRecord {
                id,
                writer: stored.writer,
                card_count: stored.card_count,
                payload: std::fs::read(&stored.payload).ok()?,
                blob: match &stored.blob {
                    Some(path) => Some(std::fs::read(path).ok()?),
                    None => None,
                },
                place: stored.place,
            })
        });
        let opening = NoteHandle::start_lineage(
            sandbox()?,
            Opened {
                records,
                balances: refilled(&file.balances, file.balances_at),
                loader: Some(loader),
            },
            client_ids,
        );
        let (handle, body) = opening.wait().map_err(handle_error)?;
        let session = Session {
            shared: handle.shared(),
            handle: Some((handle, body.clone())),
            body,
            load,
            sent: BTreeSet::new(),
            records: stored,
            editing: false,
            meta: Vec::new(),
            balances_saved: Instant::now(),
        };
        self.text.sessions.insert(*rid, session);
        Ok(())
    }

    /// Closes the note's document, keeping each writer's balance, unless an
    /// editor holds it.
    fn close_session(&mut self, rid: &Id) -> Result<(), SyncError> {
        let Some(session) = self.text.sessions.get(rid) else {
            return Ok(());
        };
        if session.editing {
            return Ok(());
        }
        if let Ok(balances) = session.shared.balances()
            && let Some(mut file) = self.text_file(rid)?
        {
            file.balances = balances;
            file.balances_at = super::unix_now();
            self.store_text(rid, &file)?;
        }
        self.text.sessions.remove(rid);
        Ok(())
    }

    /// The note's document for the editor. The note enters text mode now,
    /// when a live `file` operation matches its file, and its edits go to
    /// the document from here on. Call [`Self::close_text`] when the editor
    /// lets go of it.
    pub fn open_text(
        &mut self,
        path: &str,
    ) -> Result<(Entered, Option<SandboxDocument>), SyncError> {
        let (entered, document) = self.open_text_at(path)?;
        Ok((entered, document.map(|(document, _)| document)))
    }

    /// [`Self::open_text`], with the text the editor's side of the document
    /// starts from: the changes it holds apply to that text, whatever the
    /// document merged since.
    pub(crate) fn open_text_at(
        &mut self,
        path: &str,
    ) -> Result<(Entered, Option<(SandboxDocument, String)>), SyncError> {
        let Some(rid) = self.rid_at(path) else {
            return Ok((Entered::NotYet, None));
        };
        if !self.in_text_mode(&rid) {
            let entered = self.enter_text(rid)?;
            if entered != Entered::Yes {
                return Ok((entered, None));
            }
        } else if self.local_edit(&rid) {
            // The file holds an edit sync hasn't captured yet.
            return Ok((Entered::NotYet, None));
        }
        self.text_settle(rid)?;
        if !self.in_text_mode(&rid) {
            return Ok((Entered::NotYet, None));
        }
        if self.text_file(&rid)?.is_some_and(|file| file.stopped) {
            // The note waits for your choice.
            return Ok((Entered::Never, None));
        }
        self.open_session(&rid)?;
        let session = self
            .text
            .sessions
            .get_mut(&rid)
            .expect("the session just opened");
        session.editing = true;
        let handle = session.handle.take();
        self.touch(&rid);
        Ok((
            Entered::Yes,
            handle.map(|(mut handle, text)| {
                handle.outlive();
                (SandboxDocument::new(handle), text)
            }),
        ))
    }

    /// The editor let go of the note at `path`: see
    /// [`Self::close_text_note`].
    pub fn close_text(&mut self, path: &str) -> Result<(), SyncError> {
        match self.rid_at(path) {
            Some(rid) => self.close_text_note(rid),
            None => Ok(()),
        }
    }

    /// The editor let go of the note: the idle rule runs now (§22).
    pub fn close_text_note(&mut self, rid: Id) -> Result<(), SyncError> {
        self.close_presence(&rid);
        if let Some(session) = self.text.sessions.get_mut(&rid) {
            session.editing = false;
        }
        // Sending your last edits counts as an edit, so the close comes
        // after them.
        self.send_text(rid)?;
        self.text
            .activity
            .entry(rid)
            .and_modify(|a| a.closed = true);
        self.text_upkeep_note(rid, Instant::now())?;
        self.close_session(&rid)
    }

    fn touch(&mut self, rid: &Id) {
        let now = Instant::now();
        self.text
            .activity
            .entry(*rid)
            .and_modify(|a| {
                a.last_edit = now;
                a.closed = false;
            })
            .or_insert(Activity {
                last_edit: now,
                last_snapshot: None,
                closed: false,
            });
    }

    /// Puts the note in text mode (§22, Entering text mode): a
    /// `text_snapshot` derived from the base operation alone.
    fn enter_text(&mut self, rid: Id) -> Result<Entered, SyncError> {
        let Some(object) = self.state.objects.get(&rid) else {
            return Ok(Entered::NotYet);
        };
        let domain = object.domain;
        let (Some(base), Some(local)) = (object.base, object.local.clone()) else {
            return Ok(Entered::NotYet);
        };
        if !local.path.ends_with(".md") {
            return Ok(Entered::Never);
        }
        let Some(info) = object.states.get(&base).cloned() else {
            return Ok(Entered::NotYet);
        };
        let (Some(seq), Some(content)) = (info.seq, info.content.clone()) else {
            return Ok(Entered::NotYet);
        };
        if content.delete || content.revision != local.revision {
            return Ok(Entered::NotYet);
        }
        if self.live_states(&rid) != [base] || self.local_edit(&rid) {
            return Ok(Entered::NotYet);
        }
        if !self.can_write_note(&rid) {
            return Ok(Entered::NotYet);
        }
        let Some(bytes) = self.engine.read_synced(&local.path).map_err(core)? else {
            return Ok(Entered::NotYet);
        };
        let (properties, body) = match note_parts(&local.path, &bytes) {
            NoteParts::Managed { properties, body } => (properties, body),
            NoteParts::Unmanaged { body } => (BTreeMap::new(), body),
            NoteParts::Outside => return Ok(Entered::Never),
        };
        if body.len() > TEXT_LIMIT {
            return Ok(Entered::Never);
        }
        let document = sandbox()?
            .first_document(&body)
            .map_err(|error| SyncError::Invalid(error.to_string()))?;
        let lineage = text_lineage_id(&rid, &info.digest, &document);
        let mut meta = vec![MetaEntry {
            key: "path".into(),
            seq,
            value: Some(content.path.clone().into_bytes()),
        }];
        for (name, value) in &properties {
            meta.push(MetaEntry {
                key: format!("p:{name}"),
                seq,
                value: Some(value.clone().into_bytes()),
            });
        }
        meta.sort_by(|a, b| a.key.cmp(&b.key));
        let composed = compose_note(&properties, &body)
            .map_err(|_| SyncError::Invalid("the note's frontmatter".into()))?;
        let revision = hash(composed.as_bytes());
        let mut file = TextFile::new(lineage);
        self.set_base(rid, &mut file, &bytes, None)?;
        self.in_domain(domain, |sync| {
            let view: BTreeSet<OpKey> = sync.live_ops(&rid).into_iter().collect();
            let covers = sync.covers_of(&rid, &view);
            sync.sign_snapshot(rid, &mut file, covers, revision, &meta, document)
        })?;
        self.state
            .objects
            .get_mut(&rid)
            .expect("signed for it")
            .text = Some(lineage);
        self.store_text(&rid, &file)?;
        self.touch(&rid);
        self.save()?;
        Ok(Entered::Yes)
    }

    /// Every live operation on `rid`, patches included.
    pub(super) fn live_ops(&self, rid: &Id) -> Vec<OpKey> {
        let domain = self.state.objects.get(rid).and_then(|o| o.domain);
        self.log_of(domain)
            .live
            .live(rid)
            .into_iter()
            .map(|(op, _)| OpKey {
                device: op.device,
                counter: op.counter,
            })
            .collect()
    }

    /// The content of a record's payload: inline, or in a blob when the
    /// payload would pass the inline limit.
    fn text_content(
        &mut self,
        rid: Id,
        bytes: &[u8],
        meta_len: usize,
    ) -> Result<(Content, Option<super::state::BlobMeta>), SyncError> {
        if bytes.len() + meta_len + PAYLOAD_OVERHEAD <= MAX_PAYLOAD_LEN {
            return Ok((
                Content::Inline {
                    bytes: Bytes(bytes.to_vec()),
                },
                None,
            ));
        }
        let found = Found {
            file: local_core::SyncedFile {
                relative_path: String::new(),
                size: bytes.len() as u64,
                modified_ns: 0,
                file_id: None,
            },
            revision: hash(bytes),
            object_id: String::new(),
            bytes: Some(bytes.to_vec()),
        };
        let (blob, upload) = self.blob_for(rid, &found)?;
        Ok((Content::Blob(blob), upload))
    }

    fn meta_len(meta: &[MetaEntry]) -> usize {
        meta.iter()
            .map(|e| e.key.len() + 16 + e.value.as_ref().map_or(0, Vec::len))
            .sum()
    }

    /// Keeps a record this device wrote in the note's file, so a restart
    /// rebuilds the document from it.
    #[allow(clippy::too_many_arguments)]
    fn keep_own(
        &mut self,
        rid: Id,
        file: &mut TextFile,
        signed: &Signed<Operation>,
        payload: &Payload,
        snapshot: bool,
        lineage: Id,
        meta: &[MetaEntry],
        blob: Option<&[u8]>,
    ) -> Result<(), SyncError> {
        let bytes = without_blob_key(payload).to_bytes();
        let digest = hash(&bytes);
        self.journal.keep(&record_path(&rid, &digest), &bytes)?;
        let blob = match blob {
            Some(blob) => {
                let digest = hash(blob);
                self.journal.keep(&record_path(&rid, &digest), blob)?;
                Some(digest)
            }
            None => None,
        };
        let (writer, card_count) = self.card_of(&signed.body.device, None)?;
        file.records.push(TextRecord {
            key: OpKey {
                device: signed.body.device,
                counter: signed.body.counter,
            },
            seq: None,
            digest: signed.digest(),
            snapshot,
            lineage,
            writer,
            card_count,
            covers: signed.body.supersedes.to_vec(),
            weight: LiveWeight::of(signed),
            epoch: signed.body.epoch,
            meta: meta.to_vec(),
            payload: digest,
            blob,
            judgement: Judgement::Readable,
        });
        Ok(())
    }

    /// Signs a `text_snapshot` holding `document` in the note's lineage.
    fn sign_snapshot(
        &mut self,
        rid: Id,
        file: &mut TextFile,
        covers: Vec<Cover>,
        revision: Digest,
        meta: &[MetaEntry],
        document: Vec<u8>,
    ) -> Result<(), SyncError> {
        let object_id = self.state.objects[&rid].object_id.clone();
        let path = meta
            .iter()
            .find(|e| e.key == "path")
            .and_then(|e| e.value.clone())
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_default();
        let (content, upload) = self.text_content(rid, &document, Self::meta_len(meta))?;
        let in_blob = matches!(content, Content::Blob(_));
        let payload = Payload {
            rid,
            kind: PayloadKind::TextSnapshot {
                object_id: Text::new(object_id.clone())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                revision,
                text_lineage: file.lineage,
                meta: meta_list(meta)?,
                content,
                origin: None,
            },
        };
        let carried = Carried {
            path,
            revision,
            delete: false,
            object_id,
            kept: None,
            blob: None,
            text: Some(file.lineage),
        };
        let key = self.sign(rid, false, covers, &payload, carried)?;
        if let Some(outgoing) = self.state.outbox.last_mut() {
            outgoing.upload = upload;
        }
        let signed = self
            .state
            .outbox
            .last()
            .map(|o| o.record.clone())
            .expect("just signed");
        let lineage = file.lineage;
        self.keep_own(
            rid,
            file,
            &signed,
            &payload,
            true,
            lineage,
            meta,
            in_blob.then_some(document.as_slice()),
        )?;
        self.release_held(rid, file);
        self.state.objects.get_mut(&rid).expect("signed").base = Some(key);
        Ok(())
    }

    /// Signs a `text_update` carrying `yrs` and `meta` under `load`.
    fn sign_update(
        &mut self,
        rid: Id,
        file: &mut TextFile,
        load: u32,
        yrs: Vec<u8>,
        meta: &[MetaEntry],
    ) -> Result<(), SyncError> {
        let object_id = self.state.objects[&rid].object_id.clone();
        let (content, upload) = self.text_content(rid, &yrs, Self::meta_len(meta))?;
        let in_blob = matches!(content, Content::Blob(_));
        let payload = Payload {
            rid,
            kind: PayloadKind::TextUpdate {
                object_id: Text::new(object_id.clone())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                text_lineage: file.lineage,
                load,
                meta: meta_list(meta)?,
                content,
            },
        };
        let carried = Carried {
            path: String::new(),
            revision: Digest::ZERO,
            delete: false,
            object_id,
            kept: None,
            blob: None,
            text: Some(file.lineage),
        };
        let key = self.sign(rid, false, Vec::new(), &payload, carried)?;
        if let Some(outgoing) = self.state.outbox.last_mut() {
            outgoing.upload = upload;
        }
        // A patch is no state of the object.
        self.state
            .objects
            .get_mut(&rid)
            .expect("signed")
            .states
            .remove(&key);
        let signed = self
            .state
            .outbox
            .last()
            .map(|o| o.record.clone())
            .expect("just signed");
        let lineage = file.lineage;
        self.keep_own(
            rid,
            file,
            &signed,
            &payload,
            false,
            lineage,
            meta,
            in_blob.then_some(yrs.as_slice()),
        )
    }

    /// Signs what the note's document holds that hasn't gone out: at most
    /// one update per load, with the waiting `path` and property writes.
    pub(super) fn send_text(&mut self, rid: Id) -> Result<usize, SyncError> {
        let domain = self.state.objects.get(&rid).and_then(|o| o.domain);
        self.in_domain(domain, |sync| sync.send_text_here(rid))
    }

    fn send_text_here(&mut self, rid: Id) -> Result<usize, SyncError> {
        if !self.text.sessions.contains_key(&rid) {
            return Ok(0);
        }
        if self.state.removal_hold || self.state.ended.is_some() {
            return Ok(0);
        }
        // The document keeps what it can't send yet, and writes it once it
        // can.
        if !self.can_write_note(&rid) {
            return Ok(0);
        }
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(0);
        };
        if file.stopped {
            return Ok(0);
        }
        let Some(session) = self.text.sessions.get(&rid) else {
            return Ok(0);
        };
        let updates = session.shared.take_updates().map_err(handle_error)?;
        let load = session.load;
        let meta = session.meta.clone();
        if updates.is_empty() && meta.is_empty() {
            return Ok(0);
        }
        let mut signed = 0;
        let mut meta = Some(meta);
        for update in updates {
            let entries = meta.take().unwrap_or_default();
            self.sign_update(rid, &mut file, update.load, update.bytes, &entries)?;
            signed += 1;
        }
        if let Some(entries) = meta.take()
            && !entries.is_empty()
        {
            self.sign_update(rid, &mut file, load, EMPTY_UPDATE.to_vec(), &entries)?;
            signed += 1;
        }
        if let Some(session) = self.text.sessions.get_mut(&rid) {
            session.meta.clear();
        }
        self.store_text(&rid, &file)?;
        self.touch(&rid);
        Ok(signed)
    }

    /// Takes in a `text_snapshot` or `text_update` that passed the checks of
    /// §13: from here on it counts as applied. The sandbox judges it later.
    pub(super) fn take_text(
        &mut self,
        record: &CheckedRecord,
        blob: Option<Vec<u8>>,
    ) -> Result<(), SyncError> {
        let op = &record.op;
        let rid = op.body.rid;
        let key = OpKey {
            device: op.body.device,
            counter: op.body.counter,
        };
        let payload = record.payload.as_ref().expect("a text record opened");
        let (snapshot, lineage, meta, object_id, revision) = match &payload.kind {
            PayloadKind::TextSnapshot {
                text_lineage,
                meta,
                object_id,
                revision,
                ..
            } => (true, *text_lineage, meta, object_id, Some(*revision)),
            PayloadKind::TextUpdate {
                text_lineage,
                meta,
                object_id,
                ..
            } => (false, *text_lineage, meta, object_id, None),
            _ => return Ok(()),
        };
        let mut file = self
            .text_file(&rid)?
            .unwrap_or_else(|| TextFile::new(lineage));
        if let Some(own) = file.records.iter_mut().find(|r| r.key == key) {
            own.seq = Some(record.seq);
            own.digest = op.digest();
            own.epoch = op.body.epoch;
        } else {
            let bytes = without_blob_key(payload).to_bytes();
            let digest = hash(&bytes);
            self.journal.keep(&record_path(&rid, &digest), &bytes)?;
            let blob = match blob {
                Some(blob) => {
                    let digest = hash(&blob);
                    self.journal.keep(&record_path(&rid, &digest), &blob)?;
                    Some(digest)
                }
                None => None,
            };
            let (writer, card_count) =
                self.card_of(&op.body.device, Some(op.body.policy_revision))?;
            file.records.push(TextRecord {
                key,
                seq: Some(record.seq),
                digest: op.digest(),
                snapshot,
                lineage,
                writer,
                card_count,
                covers: op.body.supersedes.to_vec(),
                weight: LiveWeight::of(op),
                epoch: op.body.epoch,
                meta: entries_of(meta),
                payload: digest,
                blob,
                judgement: Judgement::Waiting,
            });
        }
        self.store_text(&rid, &file)?;
        let domain = self.active;
        let object = self.state.objects.entry(rid).or_default();
        object.domain = domain;
        if object.object_id.is_empty() {
            object.object_id = object_id.0.clone();
        }
        if op.body.device == self.credentials.keys.device() {
            object.own_highest = object.own_highest.max(op.body.counter);
        }
        if record.class == OpClass::State {
            let path = meta
                .iter()
                .find(|e| e.key.as_str() == "path")
                .and_then(|e| e.value.as_ref())
                .and_then(|v| String::from_utf8(v.0.clone()).ok())
                .unwrap_or_default();
            let entry = object.states.entry(key).or_insert(super::StateInfo {
                digest: op.digest(),
                seq: None,
                covers: op.body.supersedes.to_vec(),
                content: None,
                epoch: op.body.epoch,
                policy_revision: op.body.policy_revision,
            });
            entry.seq = Some(record.seq);
            entry.digest = op.digest();
            entry.epoch = op.body.epoch;
            entry.content = Some(Carried {
                path,
                revision: revision.unwrap_or(Digest::ZERO),
                delete: false,
                object_id: object_id.0.clone(),
                kept: None,
                blob: None,
                text: Some(lineage),
            });
        }
        self.text.unsettled.insert(rid);
        Ok(())
    }

    /// A state operation reached a text-mode note: the next settle looks
    /// at its lineage again.
    pub(super) fn note_text_change(&mut self, rid: Id) {
        if self.in_text_mode(&rid) {
            self.text.unsettled.insert(rid);
        }
    }

    /// Runs the sandbox on what the journal took in since the last call.
    pub(super) fn settle_text(&mut self) -> Result<(), SyncError> {
        for rid in std::mem::take(&mut self.text.unsettled) {
            self.text_settle(rid)?;
        }
        Ok(())
    }

    /// The lineage the live set leaves the note on: the lineage of its live
    /// snapshots, when they all name one.
    fn live_lineage(&self, rid: &Id) -> Option<Id> {
        let object = self.state.objects.get(rid)?;
        let mut lineages = self.live_states(rid).into_iter().filter_map(|key| {
            let content = object.states.get(&key)?.content.as_ref()?;
            Some(content.text)
        });
        let first = lineages.next()??;
        lineages.all(|l| l == Some(first)).then_some(first)
    }

    /// Brings the note in line with its live records: follows the live
    /// lineage, hands the sandbox what it hasn't judged, and writes the
    /// note's file from the merged state.
    pub(super) fn text_settle(&mut self, rid: Id) -> Result<(), SyncError> {
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(());
        };
        // This device's own snapshot on its way holds the lineage up.
        let own_snapshot = file.records.iter().any(|r| {
            r.snapshot && r.seq.is_none() && r.key.counter != HELD && r.lineage == file.lineage
        });
        let live = own_snapshot
            .then_some(file.lineage)
            .or_else(|| self.live_lineage(&rid));
        let Some(lineage) = live else {
            // A `file` operation covers the lineage, or a conflict holds it.
            if !self.live_states(&rid).is_empty()
                && !self
                    .state
                    .objects
                    .get(&rid)
                    .is_some_and(|o| self.has_live_snapshot(&rid, o))
            {
                self.end_text(rid)?;
            }
            return Ok(());
        };
        if file.lineage != lineage {
            // A rebase: the note starts again from the new lineage.
            file.lineage = lineage;
            file.stopped = false;
            file.pending_since = None;
            self.store_text(&rid, &file)?;
            self.editor_left(rid, LeftWhy::Moved);
            self.text.sessions.remove(&rid);
        }
        self.state.objects.entry(rid).or_default().text = Some(lineage);
        // Updates of a lineage the note left go to review as text, whenever
        // they arrive. The ones the rebase covered aren't live, so they
        // merged into it.
        // This device's own rebase covers its records once the log has it.
        if !own_snapshot {
            let live: BTreeSet<OpKey> = self.live_ops(&rid).into_iter().collect();
            let mut late = Vec::new();
            for record in &mut file.records {
                if !record.snapshot
                    && record.lineage != lineage
                    && matches!(record.judgement, Judgement::Waiting | Judgement::Readable)
                    && live.contains(&record.key)
                {
                    record.judgement = Judgement::Ended;
                    late.push(TextReview {
                        rid,
                        kind: TextReviewKind::EndedLineage,
                        signer: Some(record.key.device),
                        seq: record.seq,
                    });
                }
            }
            for review in late {
                file.review(review);
            }
        }
        if file.stopped {
            return Ok(());
        }
        self.open_session(&rid)?;
        let session = self.text.sessions.get_mut(&rid).expect("just opened");
        for record in &file.records {
            if record.judgement != Judgement::Waiting || record.lineage != lineage {
                continue;
            }
            let id = record.id();
            if !session.sent.insert(id) {
                continue;
            }
            let payload = self
                .journal
                .kept(&record_path(&rid, &record.payload), &record.payload)?;
            let blob = match &record.blob {
                Some(blob) => Some(self.journal.kept(&record_path(&rid, blob), blob)?),
                None => None,
            };
            session
                .records
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(
                    id,
                    Stored {
                        writer: record.writer,
                        card_count: record.card_count,
                        place: record.place(),
                        payload: self.journal.dir().join(record_path(&rid, &record.payload)),
                        blob: record
                            .blob
                            .map(|blob| self.journal.dir().join(record_path(&rid, &blob))),
                    },
                );
            session
                .shared
                .receive(RemoteRecord {
                    id,
                    writer: record.writer,
                    card_count: record.card_count,
                    payload,
                    blob,
                    place: record.place(),
                })
                .map_err(handle_error)?;
        }
        session.shared.settle().map_err(handle_error)?;
        self.touch(&rid);
        self.absorb(rid, &mut file)?;
        self.store_text(&rid, &file)?;
        self.write_note(rid, &mut file)?;
        self.prune(rid, &mut file)?;
        self.store_text(&rid, &file)?;
        let busy = self
            .text
            .sessions
            .get(&rid)
            .is_some_and(|s| s.editing || !s.meta.is_empty());
        // Records that wait on a writer's allowance run on its refill.
        let deferred = file
            .records
            .iter()
            .any(|r| r.judgement == Judgement::Waiting && r.lineage == lineage);
        if !busy && !deferred {
            self.close_session(&rid)?;
        }
        Ok(())
    }

    fn has_live_snapshot(&self, rid: &Id, object: &super::Object) -> bool {
        self.live_states(rid).iter().any(|key| {
            object
                .states
                .get(key)
                .and_then(|s| s.content.as_ref())
                .is_some_and(|c| c.text.is_some())
        })
    }

    /// Takes what the sandbox reported and the changes the document merged.
    fn absorb(&mut self, rid: Id, file: &mut TextFile) -> Result<(), SyncError> {
        let Some(session) = self.text.sessions.get_mut(&rid) else {
            return Ok(());
        };
        let device_of = |file: &TextFile, id: u64| {
            file.records
                .iter()
                .find(|r| r.id() == id)
                .map(|r| (r.key.device, r.seq))
        };
        let mut reports = Vec::new();
        let stopped_before = file.stopped;
        let events = session.shared.take_events();
        let status = session.shared.status();
        for event in events {
            let review = |kind, id: u64, file: &mut TextFile| {
                let (signer, seq) = device_of(file, id).map_or((None, None), |(d, s)| (Some(d), s));
                file.review(TextReview {
                    rid,
                    kind,
                    signer,
                    seq,
                });
            };
            let set = |id: u64, judgement: Judgement, file: &mut TextFile| {
                if let Some(record) = file.records.iter_mut().find(|r| r.id() == id) {
                    record.judgement = judgement;
                }
            };
            match event {
                Event::Verdict { id, verdict } if verdict.outcome.is_err() => {
                    set(id, Judgement::Unreadable, file);
                    review(TextReviewKind::Unreadable, id, file);
                }
                Event::Merged { id, .. } => {
                    set(id, Judgement::Readable, file);
                    if status == LineageStatus::Pending && file.pending_since.is_none() {
                        file.pending_since = Some(id);
                    }
                }
                Event::Refused { id } => {
                    set(id, Judgement::Refused, file);
                    review(TextReviewKind::Refused, id, file);
                }
                Event::Disagrees { id, .. } => {
                    let snapshot = file.records.iter().any(|r| r.id() == id && r.snapshot);
                    set(
                        id,
                        if snapshot {
                            Judgement::Disagrees
                        } else {
                            Judgement::Readable
                        },
                        file,
                    );
                    review(TextReviewKind::Disagrees, id, file);
                    reports.push("sync_text_conflict");
                }
                Event::Stopped { id, .. } => {
                    file.stopped = true;
                    review(TextReviewKind::Stopped, id, file);
                }
                Event::Pending(true) => file.review(TextReview {
                    rid,
                    kind: TextReviewKind::Pending,
                    signer: None,
                    seq: None,
                }),
                Event::Pending(false) => {
                    file.pending_since = None;
                    file.reviews.retain(|r| r.kind != TextReviewKind::Pending);
                }
                _ => {}
            }
        }
        let stopped_now = file.stopped && !stopped_before;
        let shared = session.shared.clone();
        for op in shared.take_merged_changes() {
            session.body = match op.apply(&session.body) {
                Ok(body) => body,
                Err(_) => shared.merged_text().map_err(handle_error)?,
            };
        }
        self.text.reported.extend(reports);
        if stopped_now {
            self.editor_left(rid, LeftWhy::Stopped);
        }
        Ok(())
    }

    /// Writes the note's file from its merged state, through the kernel's
    /// durable write path. A file with an edit sync hasn't captured keeps
    /// it until the next capture turns it into an update.
    fn write_note(&mut self, rid: Id, file: &mut TextFile) -> Result<(), SyncError> {
        let Some(session) = self.text.sessions.get(&rid) else {
            return Ok(());
        };
        // A document with no snapshot yet holds nothing of the note: one
        // that waits on its writer's allowance, or a lineage's start that
        // was refused.
        if file.stopped || !file.holds_a_snapshot() || self.local_edit(&rid) {
            return Ok(());
        }
        let merged = merged_meta(&file.records);
        let (path, properties) = note_meta(&merged);
        let Some(path) = path else {
            return Ok(());
        };
        let Ok(composed) = compose_note(&properties, &session.body) else {
            return Ok(());
        };
        let bytes = composed.into_bytes();
        let revision = hash(&bytes);
        let synced = match self.placement_with_exception(&rid, &path) {
            Placement::At(path) | Placement::Shared(path) => path,
            // A path that left the object's root: the file stays, and the
            // change goes to review (§9).
            Placement::Outside(_) => {
                file.review(TextReview {
                    rid,
                    kind: TextReviewKind::OutsideRoot,
                    signer: None,
                    seq: None,
                });
                return Ok(());
            }
        };
        let object = &self.state.objects[&rid];
        let local = object.local.as_ref();
        let in_place = local.is_some_and(|l| l.path == synced);
        if in_place && local.is_some_and(|l| l.revision == revision) {
            return self.set_base(rid, file, &bytes, Some(revision));
        }
        // A note whose file holds its merged state in its own form keeps
        // those bytes until another change rewrites them.
        if in_place && file.base.is_some_and(|base| base.form == Some(revision)) {
            return Ok(());
        }
        let Some(base) = self.latest_snapshot(&rid) else {
            return Ok(());
        };
        let (target, displaced) = self.destination(&rid, &base, &synced);
        for other in displaced {
            if let Err(error) = self.move_aside(&other) {
                tracing::warn!(%error, "a file that yields its path couldn't move aside");
                return Ok(());
            }
        }
        match self.place(&rid, &target, &Body::Bytes(bytes.clone()), revision) {
            Ok(()) => {
                self.state.objects.get_mut(&rid).expect("present").base = Some(base);
                self.set_base(rid, file, &bytes, Some(revision))
            }
            Err(SyncError::Invalid(message)) if message.starts_with("revision_conflict") => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The live snapshot at the highest log sequence.
    fn latest_snapshot(&self, rid: &Id) -> Option<OpKey> {
        let object = self.state.objects.get(rid)?;
        self.live_states(rid)
            .into_iter()
            .filter(|key| {
                object
                    .states
                    .get(key)
                    .and_then(|s| s.content.as_ref())
                    .is_some_and(|c| c.text.is_some())
            })
            .max_by_key(|key| object.states[key].seq.unwrap_or(u64::MAX))
            .or(object.base)
    }

    /// Forgets the records no live operation holds, once the sandbox judged
    /// every snapshot that covers one (§22).
    fn prune(&mut self, rid: Id, file: &mut TextFile) -> Result<(), SyncError> {
        if file
            .records
            .iter()
            .any(|r| r.snapshot && r.judgement == Judgement::Waiting)
        {
            return Ok(());
        }
        let live: BTreeSet<OpKey> = self.live_ops(&rid).into_iter().collect();
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut file.records)
            .into_iter()
            .partition(|r| r.seq.is_none() || live.contains(&r.key));
        file.records = kept;
        self.release_records(rid, file, gone);
        Ok(())
    }

    /// Drops from the journal what only `gone` records used.
    fn release_records(&self, rid: Id, file: &TextFile, gone: Vec<TextRecord>) {
        for record in gone {
            let used = |digest: &Digest| {
                file.records
                    .iter()
                    .any(|r| r.payload == *digest || r.blob == Some(*digest))
            };
            if !used(&record.payload) {
                self.journal.forget(&record_path(&rid, &record.payload));
            }
            if let Some(blob) = record.blob.filter(|blob| !used(blob)) {
                self.journal.forget(&record_path(&rid, &blob));
            }
        }
    }

    /// Ends text mode on this device: the note's file is what it holds now,
    /// and a `file` operation covers the lineage.
    pub(super) fn end_text(&mut self, rid: Id) -> Result<(), SyncError> {
        self.editor_left(rid, LeftWhy::Ended);
        self.text.sessions.remove(&rid);
        self.text.activity.remove(&rid);
        self.state.ended_text.insert(rid);
        if let Some(object) = self.state.objects.get_mut(&rid) {
            object.text = None;
        }
        Ok(())
    }

    /// Signs a `file` operation of the note's file that covers every live
    /// operation, which ends the lineage on every device (§22).
    fn end_with_file(&mut self, rid: Id, found: &Found) -> Result<(), SyncError> {
        let domain = self.object_domain(&rid);
        self.in_domain(domain, |sync| {
            let view: BTreeSet<OpKey> = sync.live_ops(&rid).into_iter().collect();
            let covers = sync.covers_of(&rid, &view);
            sync.sign_file(rid, found, covers)
        })?;
        self.end_text(rid)
    }

    /// Keeps a stopped note's last good state: a `file` operation that
    /// covers every live operation of the note ends its lineage (§22).
    pub fn keep_last_good(&mut self, rid: Id) -> Result<(), SyncError> {
        let path = self
            .state
            .objects
            .get(&rid)
            .and_then(|o| o.local.as_ref())
            .map(|l| l.path.clone())
            .ok_or_else(|| SyncError::Invalid("the note has no file".into()))?;
        let found = self.found_on_disk(&path)?;
        self.end_with_file(rid, &found)?;
        self.save()
    }

    /// Turns a local change to a text-mode note's file into what §22 asks:
    /// a `text_update` computed from the difference, nothing for a change
    /// that only reformats the frontmatter, or a `file` operation that ends
    /// text mode. Returns how many records it signed.
    ///
    /// The difference runs from what the file last held to what it holds
    /// now, and then applies to the document as an edit made from that base,
    /// so a change the document merged meanwhile stays. `yielded` says the
    /// note moved to a name that yields its path to another object, which
    /// its file already has.
    pub(super) fn capture_text(
        &mut self,
        rid: Id,
        found: &Found,
        yielded: bool,
    ) -> Result<usize, SyncError> {
        // The file keeps a change the device can't send: taken into the
        // document it would leave with the document (§14).
        if !self.can_write_note(&rid) {
            return Ok(0);
        }
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(0);
        };
        if file.stopped {
            return Ok(0);
        }
        let bytes = match &found.bytes {
            Some(bytes) => bytes.clone(),
            None => {
                self.end_with_file(rid, found)?;
                return Ok(1);
            }
        };
        let path = found.file.relative_path.as_str();
        let merged = merged_meta(&file.records);
        let (merged_path, merged_properties) = note_meta(&merged);
        let managed = !merged_properties.is_empty();
        let (properties, body) = match note_parts(path, &bytes) {
            NoteParts::Managed { properties, body } if managed => (properties, body),
            NoteParts::Unmanaged { body } if !managed => (BTreeMap::new(), body),
            _ => {
                self.end_with_file(rid, found)?;
                return Ok(1);
            }
        };
        if body.len() > TEXT_LIMIT {
            self.end_with_file(rid, found)?;
            return Ok(1);
        }
        let old_path = self.state.objects[&rid]
            .local
            .as_ref()
            .map(|l| l.path.clone());
        self.open_session(&rid)?;
        self.absorb(rid, &mut file)?;
        self.store_text(&rid, &file)?;
        let current = self.text.sessions[&rid].body.clone();
        let (base_properties, base_body) = self
            .base_parts(&rid, &file, old_path.as_deref().unwrap_or(path))
            .unwrap_or_else(|| (merged_properties.clone(), current.clone()));
        let body_changed = body != base_body && body != current;
        let mut meta = Vec::new();
        // The file moved here, or took a name that yields its path. A note
        // that only hasn't reached a merged path yet writes nothing.
        if (yielded || old_path.as_deref() != Some(path))
            && merged_path.as_deref() != Some(path)
            && let synced = sync_protocol::matching::synced_path(path, &self.state.roots)
            && merged_path.as_deref() != Some(synced.as_str())
        {
            meta.push(MetaEntry {
                key: "path".into(),
                seq: 0,
                value: Some(synced.into_bytes()),
            });
        }
        for (name, value) in &properties {
            if base_properties.get(name) != Some(value)
                && merged_properties.get(name) != Some(value)
            {
                meta.push(MetaEntry {
                    key: format!("p:{name}"),
                    seq: 0,
                    value: Some(value.clone().into_bytes()),
                });
            }
        }
        for name in base_properties.keys() {
            if !properties.contains_key(name) && merged_properties.contains_key(name) {
                meta.push(MetaEntry {
                    key: format!("p:{name}"),
                    seq: 0,
                    value: None,
                });
            }
        }
        meta.sort_by(|a, b| a.key.cmp(&b.key));
        let session = self.text.sessions.get_mut(&rid).expect("just opened");
        if body_changed {
            session
                .shared
                .merge_text(base_body, body, 0)
                .map_err(handle_error)?;
        }
        session.meta.extend(meta);
        let changed = body_changed || !session.meta.is_empty();
        let signed = self.send_text(rid)?;
        if let Some(object) = self.state.objects.get_mut(&rid) {
            object.local = Some(found.local());
        }
        let mut file = self
            .text_file(&rid)?
            .ok_or_else(|| SyncError::Invalid("the note left text mode".into()))?;
        self.absorb(rid, &mut file)?;
        // Only the frontmatter's form changed: your bytes stay until another
        // change rewrites them.
        let form = (!changed).then(|| note_form(path, &bytes)).flatten();
        self.set_base(rid, &mut file, &bytes, form)?;
        if changed {
            // A change the document merged while the file held this edit
            // reaches the file now.
            self.write_note(rid, &mut file)?;
        }
        self.store_text(&rid, &file)?;
        // What the server had no space for goes again as a snapshot (§20).
        let waiting = self.state.waiting_for_space.contains(&rid) && self.can_write_note(&rid);
        if waiting {
            self.write_snapshot(rid, false)?;
        }
        if !changed && !waiting {
            return Ok(0);
        }
        self.touch(&rid);
        let editing = self
            .text
            .sessions
            .get(&rid)
            .is_some_and(|session| session.editing);
        if !editing {
            self.close_session(&rid)?;
        }
        Ok(signed + usize::from(waiting))
    }

    /// Notes how far the pull that just ended took this device.
    pub(super) fn text_pulled(&mut self) {
        let caught_up = self.state.log.cursor >= self.state.log.head
            && self
                .state
                .restricted
                .values()
                .all(|frame| frame.log.cursor >= frame.log.head);
        self.text.pulled = caught_up.then(Instant::now);
    }

    /// Sets how old the last pull may be for a snapshot rule to run.
    pub fn set_pulled_within(&mut self, within: Duration) {
        self.text.pulled_within = Some(within);
    }

    /// One tick of text mode: takes the documents' merged changes to the
    /// files, signs the edits that wait, and runs the snapshot rules.
    pub fn text_tick(&mut self, now: Instant) -> Result<usize, SyncError> {
        let mut signed = self.resend_lost_text()?;
        self.settle_text()?;
        let rids: Vec<Id> = self.text.sessions.keys().copied().collect();
        for rid in rids {
            let sent = self.send_text(rid)?;
            signed += sent;
            let Some(mut file) = self.text_file(&rid)? else {
                self.text.sessions.remove(&rid);
                continue;
            };
            let Some(session) = self.text.sessions.get(&rid) else {
                continue;
            };
            let closed = !session.shared.is_alive();
            // An edit that failed sits in the editor and not in the
            // document, so the editor comes back from the file.
            if session.editing && !session.shared.take_errors().is_empty() {
                self.absorb(rid, &mut file)?;
                self.write_note(rid, &mut file)?;
                self.store_text(&rid, &file)?;
                self.editor_left(rid, LeftWhy::Moved);
                self.text.sessions.remove(&rid);
                continue;
            }
            if session.balances_saved.elapsed() >= BALANCES_EVERY
                && let Ok(balances) = session.shared.balances()
            {
                file.balances = balances;
                file.balances_at = super::unix_now();
                if let Some(session) = self.text.sessions.get_mut(&rid) {
                    session.balances_saved = Instant::now();
                }
            }
            self.absorb(rid, &mut file)?;
            // Only your own edit ends text mode at the limit, so a
            // collaborator's flood can't end the lineage.
            if sent > 0
                && let Some(session) = self.text.sessions.get(&rid)
                && session.body.len() > TEXT_LIMIT
                && let Some(path) = self.state.objects[&rid]
                    .local
                    .as_ref()
                    .map(|l| l.path.clone())
            {
                self.write_note(rid, &mut file)?;
                let found = self.found_on_disk(&path)?;
                self.end_with_file(rid, &found)?;
                signed += 1;
                continue;
            }
            self.write_note(rid, &mut file)?;
            self.store_text(&rid, &file)?;
            if closed {
                self.text.sessions.remove(&rid);
            }
        }
        let notes: Vec<Id> = self
            .state
            .objects
            .iter()
            .filter(|(_, object)| object.text.is_some())
            .map(|(rid, _)| *rid)
            .collect();
        for rid in notes {
            signed += usize::from(self.text_upkeep_note(rid, now)?);
        }
        Ok(signed)
    }

    /// The notes whose documents an editor holds.
    pub(super) fn text_open_notes(&self) -> Vec<Id> {
        self.text
            .sessions
            .iter()
            .filter(|(_, session)| session.editing)
            .map(|(rid, _)| *rid)
            .collect()
    }

    /// The sync client's side of the note's open document.
    pub(super) fn text_shared(&self, rid: &Id) -> Option<SharedNote> {
        self.text.sessions.get(rid).map(|s| s.shared.clone())
    }

    /// One 500 ms tick while a note is open: sends the edits that wait.
    pub async fn text_step(
        &mut self,
        service: &mut crate::sync::SyncService,
    ) -> Result<usize, SyncError> {
        let signed = self.text_tick(Instant::now())?;
        if signed > 0 {
            self.save()?;
            self.push_everything(service).await?;
        }
        Ok(signed)
    }

    /// Signs an update holding what `yrs` builds for this device's client
    /// ID, whatever it holds, as a hostile writer would.
    #[cfg(feature = "test-hooks")]
    pub fn sign_hostile_update(
        &mut self,
        path: &str,
        yrs: impl FnOnce(u64) -> Vec<u8>,
    ) -> Result<(), SyncError> {
        self.sign_hostile_update_at(path, 536_870_911, yrs)
    }

    /// [`Self::sign_hostile_update`] under the load number `load`.
    #[cfg(feature = "test-hooks")]
    pub fn sign_hostile_update_at(
        &mut self,
        path: &str,
        load: u32,
        yrs: impl FnOnce(u64) -> Vec<u8>,
    ) -> Result<(), SyncError> {
        let rid = self
            .rid_at(path)
            .ok_or_else(|| SyncError::Invalid("no such note".into()))?;
        let client = yrs_client_id(self.card_index()?, load)
            .ok_or_else(|| SyncError::Invalid("no client ID".into()))?;
        let mut file = self
            .text_file(&rid)?
            .ok_or_else(|| SyncError::Invalid("not in text mode".into()))?;
        self.sign_update(rid, &mut file, load, yrs(client), &[])?;
        self.store_text(&rid, &file)
    }

    /// Signs an update that writes `path` and nothing else, whatever the
    /// note's root says.
    #[cfg(feature = "test-hooks")]
    pub fn sign_text_path(&mut self, path: &str, new_path: &str) -> Result<(), SyncError> {
        let rid = self
            .rid_at(path)
            .ok_or_else(|| SyncError::Invalid("no such note".into()))?;
        let mut file = self
            .text_file(&rid)?
            .ok_or_else(|| SyncError::Invalid("not in text mode".into()))?;
        let meta = [MetaEntry {
            key: "path".into(),
            seq: 0,
            value: Some(new_path.as_bytes().to_vec()),
        }];
        let domain = self.object_domain(&rid);
        self.in_domain(domain, |sync| {
            sync.sign_update(rid, &mut file, 1, EMPTY_UPDATE.to_vec(), &meta)?;
            sync.store_text(&rid, &file)
        })
    }

    /// Signs a snapshot of the note holding the first document of `text`,
    /// which covers every live record, whatever they hold, as a hostile
    /// writer would.
    #[cfg(feature = "test-hooks")]
    pub fn sign_hostile_snapshot(&mut self, path: &str, text: &str) -> Result<(), SyncError> {
        let rid = self
            .rid_at(path)
            .ok_or_else(|| SyncError::Invalid("no such note".into()))?;
        let mut file = self
            .text_file(&rid)?
            .ok_or_else(|| SyncError::Invalid("not in text mode".into()))?;
        let document = sandbox()?
            .first_document(text)
            .map_err(|error| SyncError::Invalid(error.to_string()))?;
        let merged = merged_meta(&file.records);
        let meta: Vec<MetaEntry> = merged
            .iter()
            .map(|(key, write)| MetaEntry {
                key: key.clone(),
                seq: if write.effective == u64::MAX {
                    0
                } else {
                    write.effective
                },
                value: write.value.clone(),
            })
            .collect();
        let view: BTreeSet<OpKey> = self.live_ops(&rid).into_iter().collect();
        let covers = self.covers_of(&rid, &view);
        self.sign_snapshot(
            rid,
            &mut file,
            covers,
            hash(text.as_bytes()),
            &meta,
            document,
        )?;
        self.store_text(&rid, &file)
    }

    /// Notes whose lineage stopped on this device, which wait for your
    /// choice.
    pub fn stopped_notes(&self) -> Vec<(Id, String)> {
        self.state
            .objects
            .iter()
            .filter(|(_, object)| object.text.is_some())
            .filter(|(rid, _)| {
                self.text_file(rid)
                    .ok()
                    .flatten()
                    .is_some_and(|file| file.stopped)
            })
            .map(|(rid, object)| {
                let path = object.local.as_ref().map(|l| l.path.clone());
                (*rid, path.unwrap_or_default())
            })
            .collect()
    }

    /// Runs the snapshot writer on the note now.
    #[cfg(feature = "test-hooks")]
    pub fn write_snapshot_for_testing(&mut self, rid: Id) -> Result<(), SyncError> {
        self.write_snapshot(rid, false)
    }

    /// Signs what the note's document holds, as a tick does.
    #[cfg(feature = "test-hooks")]
    pub fn send_text_for_testing(&mut self, rid: Id) -> Result<usize, SyncError> {
        self.send_text(rid)
    }

    /// Notes whose merged path left their domain root, which wait for your
    /// choice.
    pub fn outside_notes(&self) -> Vec<(Id, String)> {
        self.text_reviews()
            .into_iter()
            .filter(|review| review.kind == TextReviewKind::OutsideRoot)
            .map(|review| {
                let path = self.local_path(&review.rid).unwrap_or_default();
                (review.rid, path)
            })
            .collect()
    }

    /// Lists the notes with records that wait for the sandbox, which a
    /// restart left unjudged.
    pub(super) fn find_unsettled(&mut self) -> Result<(), SyncError> {
        let notes: Vec<Id> = self
            .state
            .objects
            .iter()
            .filter(|(_, object)| object.text.is_some())
            .map(|(rid, _)| *rid)
            .collect();
        for rid in notes {
            let waits = self.text_file(&rid)?.is_some_and(|file| {
                file.records
                    .iter()
                    .any(|r| r.judgement == Judgement::Waiting)
            });
            if waits {
                self.text.unsettled.insert(rid);
            }
        }
        Ok(())
    }

    /// Whether the client ticks: a note's document is open.
    pub fn text_active(&self) -> bool {
        !self.text.sessions.is_empty()
    }

    /// Runs the snapshot rules on the note (§22). Returns whether it wrote
    /// a snapshot.
    fn text_upkeep_note(&mut self, rid: Id, now: Instant) -> Result<bool, SyncError> {
        let Some(file) = self.text_file(&rid)? else {
            return Ok(false);
        };
        if file.stopped || !self.can_write_note(&rid) {
            return Ok(false);
        }
        let required = self.text.required.contains(&rid);
        // A device back from away catches up first, or it would repeat what
        // the others did meanwhile (§22).
        let within = self.text.pulled_within.unwrap_or(PULLED_WITHIN);
        let current = self.text.pulled.is_some_and(|at| at.elapsed() < within);
        if !required && !current {
            return Ok(false);
        }
        let own = self.credentials.keys.device();
        let activity = *self.text.activity.entry(rid).or_insert(Activity {
            last_edit: now,
            last_snapshot: None,
            closed: false,
        });
        let live: BTreeSet<OpKey> = self.live_ops(&rid).into_iter().collect();
        let patches: Vec<&TextRecord> = file
            .records
            .iter()
            .filter(|r| !r.snapshot && live.contains(&r.key))
            .collect();
        let facts = SnapshotFacts {
            patches: patches.len(),
            bytes: patches.iter().map(|r| r.weight).sum(),
            snapshot_size: file
                .records
                .iter()
                .filter(|r| r.snapshot)
                .map(|r| r.weight)
                .max()
                .unwrap_or(0),
            designated: patches.iter().map(|r| r.key.device).min() == Some(own),
            wrote_latest: patches
                .iter()
                .max_by_key(|r| r.id())
                .is_some_and(|r| r.key.device == own),
            idle: now.saturating_duration_since(activity.last_edit),
            closed: activity.closed,
            since_snapshot: activity
                .last_snapshot
                .map(|at| now.saturating_duration_since(at)),
            required,
        };
        if !snapshot_due(&facts) {
            return Ok(false);
        }
        self.write_snapshot(rid, false)?;
        self.text.required.remove(&rid);
        if let Some(activity) = self.text.activity.get_mut(&rid) {
            activity.last_snapshot = Some(now);
        }
        Ok(true)
    }

    /// Writes a `text_snapshot` of the note's document, or a rebase once
    /// the document reaches 16 MiB (§22). A `rewrite` writes again records
    /// under an epoch a replacement skipped.
    pub(super) fn write_snapshot(&mut self, rid: Id, rewrite: bool) -> Result<(), SyncError> {
        let domain = self.object_domain(&rid);
        self.send_text(rid)?;
        let keep_open = self.text.sessions.contains_key(&rid);
        self.open_session(&rid)?;
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(());
        };
        self.absorb(rid, &mut file)?;
        let session = &self.text.sessions[&rid];
        let document = session.shared.state().map_err(handle_error)?;
        let body = session.body.clone();
        let merged = merged_meta(&file.records);
        let (_, properties) = note_meta(&merged);
        let composed = compose_note(&properties, &body)
            .map_err(|_| SyncError::Invalid("the note's frontmatter".into()))?;
        let revision = hash(composed.as_bytes());
        let meta: Vec<MetaEntry> = merged
            .iter()
            .map(|(key, write)| MetaEntry {
                key: key.clone(),
                seq: if write.effective == u64::MAX {
                    0
                } else {
                    write.effective
                },
                value: write.value.clone(),
            })
            .collect();
        let live: BTreeSet<OpKey> = self.live_ops(&rid).into_iter().collect();
        let designated = file
            .records
            .iter()
            .filter(|r| !r.snapshot && live.contains(&r.key))
            .map(|r| r.key.device)
            .min()
            == Some(self.credentials.keys.device());
        let rebase_at = self.text.rebase_at.unwrap_or(REBASE_AT);
        let rebase = document.len() >= rebase_at && designated && !rewrite;
        let covers = self.snapshot_covers(&rid, &file);
        let before = self.state.outbox.len();
        if rebase {
            let new_document = sandbox()?
                .first_document(&body)
                .map_err(|error| SyncError::Invalid(error.to_string()))?;
            let base = self
                .latest_snapshot(&rid)
                .and_then(|key| self.state.objects[&rid].states.get(&key).map(|s| s.digest))
                .unwrap_or(Digest::ZERO);
            file.lineage = text_lineage_id(&rid, &base, &new_document);
            self.in_domain(domain, |sync| {
                sync.sign_snapshot(rid, &mut file, covers, revision, &meta, new_document)
            })?;
            self.state.objects.entry(rid).or_default().text = Some(file.lineage);
            self.store_text(&rid, &file)?;
            self.editor_left(rid, LeftWhy::Moved);
            self.text.sessions.remove(&rid);
        } else {
            self.in_domain(domain, |sync| {
                sync.sign_snapshot(rid, &mut file, covers, revision, &meta, document)
            })?;
            self.store_text(&rid, &file)?;
        }
        if rewrite {
            for outgoing in &mut self.state.outbox[before..] {
                outgoing.rewrite = true;
            }
        }
        if !keep_open {
            self.close_session(&rid)?;
        }
        Ok(())
    }

    /// Forgets the log positions of `lost` records, so a snapshot's `meta`
    /// names 0 for their writes (§14, §22).
    pub(super) fn forget_text_positions(
        &mut self,
        rid: &Id,
        lost: &[OpKey],
    ) -> Result<(), SyncError> {
        let Some(mut file) = self.text_file(rid)? else {
            return Ok(());
        };
        for record in &mut file.records {
            if lost.contains(&record.key) {
                record.seq = None;
            }
        }
        self.store_text(rid, &file)
    }

    /// The text-mode note whose record lists `blob`, when this device holds
    /// the note's document.
    pub(super) fn text_note_of_blob(&self, blob: &Id) -> Result<Option<Id>, SyncError> {
        let notes: Vec<Id> = self
            .state
            .objects
            .iter()
            .filter(|(_, object)| object.text.is_some())
            .map(|(rid, _)| *rid)
            .collect();
        for rid in notes {
            let Some(file) = self.text_file(&rid)? else {
                continue;
            };
            for record in file.records.iter().filter(|r| r.blob.is_some()) {
                let Ok(bytes) = self
                    .journal
                    .kept(&record_path(&rid, &record.payload), &record.payload)
                else {
                    continue;
                };
                let Ok(payload) = <Payload as sync_protocol::codec::Decode>::from_bytes(&bytes)
                else {
                    continue;
                };
                let (PayloadKind::TextSnapshot { content, .. }
                | PayloadKind::TextUpdate { content, .. }) = payload.kind
                else {
                    continue;
                };
                if matches!(content, Content::Blob(listed) if listed.blob == *blob) {
                    return Ok(Some(rid));
                }
            }
        }
        Ok(None)
    }

    /// The blob keys in the note's kept records, for a test that looks for
    /// them at rest.
    #[cfg(feature = "test-hooks")]
    pub fn kept_text_keys_for_testing(&self, rid: &Id) -> Vec<sync_protocol::types::SecretKey> {
        let Ok(Some(file)) = self.text_file(rid) else {
            return Vec::new();
        };
        file.records
            .iter()
            .filter_map(|record| {
                let path = record_path(rid, &record.payload);
                let bytes = self.journal.kept(&path, &record.payload).ok()?;
                let payload = Payload::from_bytes(&bytes).ok()?;
                match payload.kind {
                    PayloadKind::TextSnapshot {
                        content: Content::Blob(blob),
                        ..
                    }
                    | PayloadKind::TextUpdate {
                        content: Content::Blob(blob),
                        ..
                    } => Some(blob.blob_key),
                    _ => None,
                }
            })
            .collect()
    }

    /// Whether the journal holds this record of the note already.
    pub(super) fn text_holds(&self, rid: &Id, key: &OpKey) -> bool {
        self.text_file(rid)
            .ok()
            .flatten()
            .is_some_and(|file| file.records.iter().any(|r| r.key == *key))
    }

    /// A snapshot's covers (§22): for each device, the highest live
    /// operation below its lowest live patch the document hasn't
    /// integrated, deferred or pending, so those patches stay live.
    fn snapshot_covers(&self, rid: &Id, file: &TextFile) -> Vec<Cover> {
        let own = self.credentials.keys.device();
        let waiting = |record: &TextRecord| {
            record.judgement == Judgement::Waiting
                || file
                    .pending_since
                    .is_some_and(|since| !record.snapshot && record.id() >= since)
        };
        let mut lowest: BTreeMap<Id, u64> = BTreeMap::new();
        for record in file.records.iter().filter(|r| waiting(r)) {
            let entry = lowest.entry(record.key.device).or_insert(u64::MAX);
            *entry = (*entry).min(record.key.counter);
        }
        let view: BTreeSet<OpKey> = self
            .live_ops(rid)
            .into_iter()
            .filter(|key| {
                key.device == own
                    || key.counter < lowest.get(&key.device).copied().unwrap_or(u64::MAX)
            })
            .collect();
        self.covers_of(rid, &view)
    }

    /// Drops a record the server refused from the note's file. The
    /// document still holds its change.
    pub(super) fn forget_text_record(
        &mut self,
        record: &Signed<Operation>,
    ) -> Result<(), SyncError> {
        let rid = record.body.rid;
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(());
        };
        file.records
            .retain(|r| r.key.device != record.body.device || r.key.counter != record.body.counter);
        self.store_text(&rid, &file)
    }

    /// Sets aside a snapshot the server had no space for (§20). It may
    /// carry a patch that left the journal, so a closed note still rebuilds
    /// from it, until the next snapshot replaces it.
    pub(super) fn hold_refused_snapshot(
        &mut self,
        record: &Signed<Operation>,
    ) -> Result<(), SyncError> {
        let rid = record.body.rid;
        let Some(mut file) = self.text_file(&rid)? else {
            return Ok(());
        };
        let refused = |r: &TextRecord| {
            r.snapshot && r.key.device == record.body.device && r.key.counter == record.body.counter
        };
        if !file.records.iter().any(refused) {
            return Ok(());
        }
        self.release_held(rid, &mut file);
        for held in file.records.iter_mut().filter(|r| refused(r)) {
            held.key.counter = HELD;
        }
        self.store_text(&rid, &file)
    }

    /// Drops a held snapshot, once a newer snapshot holds what it held.
    fn release_held(&self, rid: Id, file: &mut TextFile) {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut file.records)
            .into_iter()
            .partition(|r| r.key.counter == HELD);
        file.records = kept;
        self.release_records(rid, file, gone);
    }

    /// Moves a re-sealed record's journal entry to its new counter, where
    /// its echo finds it.
    pub(super) fn rekey_text(&mut self, rid: &Id, old: OpKey, new: OpKey) -> Result<(), SyncError> {
        if old == new {
            return Ok(());
        }
        let Some(mut file) = self.text_file(rid)? else {
            return Ok(());
        };
        let Some(record) = file.records.iter_mut().find(|r| r.key == old) else {
            return Ok(());
        };
        record.key = new;
        self.store_text(rid, &file)
    }

    /// The server asked for a snapshot of the note before it takes another
    /// patch (§14).
    pub(super) fn snapshot_required(&mut self, rid: Id) -> Result<(), SyncError> {
        self.text.required.insert(rid);
        self.write_snapshot(rid, false)?;
        self.text.required.remove(&rid);
        Ok(())
    }

    /// Whether `rid` is in text mode and its rejected record was a text
    /// update.
    pub(super) fn is_text_update(&self, record: &Signed<Operation>) -> bool {
        record.body.supersedes.is_empty() && self.in_text_mode(&record.body.rid)
    }

    /// Writes again, as snapshots of their lineage, the text-mode notes
    /// that hold this device's own records under `epochs`, which a
    /// replacement skipped (§9): each snapshot covers those records.
    pub(super) fn rewrite_text(&mut self, epochs: &BTreeSet<u64>) -> Result<usize, SyncError> {
        let own = self.credentials.keys.device();
        let rids: Vec<Id> = self
            .state
            .objects
            .iter()
            .filter(|(_, object)| object.text.is_some() && object.domain == self.active)
            .map(|(rid, _)| *rid)
            .collect();
        let mut written = 0;
        for rid in rids {
            let Some(file) = self.text_file(&rid)? else {
                continue;
            };
            let holds = file
                .records
                .iter()
                .any(|r| r.key.device == own && epochs.contains(&r.epoch));
            if holds {
                self.write_snapshot(rid, true)?;
                written += 1;
            }
        }
        Ok(written)
    }

    /// The bytes a text-mode state gives, for a review of a conflict.
    pub(super) fn text_body_of(&mut self, rid: &Id) -> Result<Option<Vec<u8>>, SyncError> {
        let Some(file) = self.text_file(rid)? else {
            return Ok(None);
        };
        let keep = self.text.sessions.contains_key(rid);
        self.open_session(rid)?;
        let merged = merged_meta(&file.records);
        let (_, properties) = note_meta(&merged);
        let body = self.text.sessions[rid].body.clone();
        if !keep {
            self.close_session(rid)?;
        }
        Ok(compose_note(&properties, &body)
            .ok()
            .map(String::into_bytes))
    }
}

/// A record's weight as the server counts it.
struct LiveWeight;

impl LiveWeight {
    fn of(op: &Signed<Operation>) -> u64 {
        sync_protocol::live::LiveOp::from_operation(op, 0).weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        seq: u64,
        snapshot: bool,
        digest: u8,
        entries: &[(&str, u64, Option<&str>)],
    ) -> TextRecord {
        TextRecord {
            key: OpKey {
                device: Id([1; 16]),
                counter: seq,
            },
            seq: Some(seq),
            digest: Digest([digest; 32]),
            snapshot,
            lineage: Id([2; 16]),
            writer: 1,
            card_count: 1,
            covers: Vec::new(),
            weight: 0,
            epoch: 1,
            meta: entries
                .iter()
                .map(|(key, seq, value)| MetaEntry {
                    key: (*key).to_owned(),
                    seq: *seq,
                    value: value.map(|v| v.as_bytes().to_vec()),
                })
                .collect(),
            payload: Digest::ZERO,
            blob: None,
            judgement: Judgement::Readable,
        }
    }

    fn value(merged: &BTreeMap<String, MetaWrite>, key: &str) -> Option<String> {
        merged[key]
            .value
            .clone()
            .map(|v| String::from_utf8(v).unwrap())
    }

    #[test]
    fn meta_merges_as_the_fixture_says() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/workspace-format/fixtures/sync-v2-collaboration.json"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        let cases: Vec<_> = cases.iter().filter(|c| c["kind"] == "meta_merge").collect();
        assert!(cases.len() >= 7);
        for case in cases {
            let records: Vec<TextRecord> = case["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| {
                    let entries: Vec<(String, u64, Option<String>)> =
                        serde_json::from_value(value["entries"].clone()).unwrap();
                    let entries: Vec<(&str, u64, Option<&str>)> = entries
                        .iter()
                        .map(|(key, seq, value)| (key.as_str(), *seq, value.as_deref()))
                        .collect();
                    let digest = u8::from_str_radix(value["digest"].as_str().unwrap(), 16).unwrap();
                    record(
                        value["seq"].as_u64().unwrap(),
                        value["snapshot"].as_bool().unwrap(),
                        digest,
                        &entries,
                    )
                })
                .collect();
            let merged = merged_meta(&records);
            for (key, expected) in case["expect"].as_object().unwrap() {
                assert_eq!(
                    value(&merged, key).as_deref(),
                    expected.as_str(),
                    "{}",
                    case["name"]
                );
            }
        }
    }

    #[test]
    fn the_device_that_wrote_last_snapshots_a_resting_note() {
        let facts = SnapshotFacts {
            patches: 9,
            wrote_latest: true,
            idle: IDLE,
            ..SnapshotFacts::default()
        };
        assert!(snapshot_due(&facts));
        assert!(!snapshot_due(&SnapshotFacts {
            patches: 8,
            ..facts
        }));
        assert!(!snapshot_due(&SnapshotFacts {
            idle: IDLE - Duration::from_secs(1),
            ..facts
        }));
        assert!(snapshot_due(&SnapshotFacts {
            idle: Duration::ZERO,
            closed: true,
            ..facts
        }));
        assert!(!snapshot_due(&SnapshotFacts {
            wrote_latest: false,
            ..facts
        }));
        assert!(snapshot_due(&SnapshotFacts {
            wrote_latest: false,
            idle: IDLE_ANY_WRITER,
            ..facts
        }));
    }

    #[test]
    fn the_designated_device_snapshots_past_the_volume_and_others_past_twice_it() {
        let facts = SnapshotFacts {
            patches: 513,
            ..SnapshotFacts::default()
        };
        assert!(!snapshot_due(&facts));
        assert!(snapshot_due(&SnapshotFacts {
            designated: true,
            ..facts
        }));
        assert!(snapshot_due(&SnapshotFacts {
            patches: 1_025,
            ..facts
        }));
        // The byte threshold grows with the snapshot, up to 2 MiB.
        let bytes = SnapshotFacts {
            bytes: 1_500_000,
            designated: true,
            ..SnapshotFacts::default()
        };
        assert!(snapshot_due(&bytes));
        assert!(!snapshot_due(&SnapshotFacts {
            snapshot_size: 8 << 20,
            ..bytes
        }));
        assert!(snapshot_due(&SnapshotFacts {
            bytes: (4 << 20) + 1,
            snapshot_size: 32 << 20,
            designated: false,
            ..bytes
        }));
    }

    #[test]
    fn a_device_snapshots_a_note_once_in_ten_seconds_unless_the_server_asks() {
        let facts = SnapshotFacts {
            patches: 2_000,
            designated: true,
            since_snapshot: Some(Duration::from_secs(9)),
            ..SnapshotFacts::default()
        };
        assert!(!snapshot_due(&facts));
        assert!(snapshot_due(&SnapshotFacts {
            required: true,
            ..facts
        }));
    }

    #[test]
    fn concurrent_edits_to_different_keys_both_survive() {
        let merged = merged_meta(&[
            record(
                1,
                true,
                1,
                &[("path", 0, Some("a.md")), ("p:a", 0, Some("1"))],
            ),
            record(2, false, 2, &[("p:a", 0, Some("2"))]),
            record(3, false, 3, &[("p:b", 0, Some("3"))]),
        ]);
        assert_eq!(value(&merged, "p:a").as_deref(), Some("2"));
        assert_eq!(value(&merged, "p:b").as_deref(), Some("3"));
    }

    #[test]
    fn the_later_committed_write_wins_a_key_and_a_digest_breaks_a_tie() {
        let merged = merged_meta(&[
            record(2, false, 9, &[("p:a", 0, Some("early"))]),
            record(3, false, 1, &[("p:a", 0, Some("late"))]),
        ]);
        assert_eq!(value(&merged, "p:a").as_deref(), Some("late"));
        // Two snapshots name one sequence for different values.
        let merged = merged_meta(&[
            record(5, true, 1, &[("p:a", 3, Some("low"))]),
            record(6, true, 2, &[("p:a", 3, Some("high"))]),
        ]);
        assert_eq!(value(&merged, "p:a").as_deref(), Some("high"));
    }

    #[test]
    fn a_snapshot_keeps_a_covered_writes_place_against_a_concurrent_one() {
        // Its entry names sequence 3, so the update at 4 it never saw wins.
        let merged = merged_meta(&[
            record(4, false, 1, &[("p:a", 0, Some("concurrent"))]),
            record(5, true, 2, &[("p:a", 3, Some("covered"))]),
        ]);
        assert_eq!(value(&merged, "p:a").as_deref(), Some("concurrent"));
    }

    #[test]
    fn a_snapshot_that_names_0_or_a_later_sequence_writes_at_its_own() {
        for named in [0, 9] {
            let merged = merged_meta(&[
                record(4, false, 1, &[("p:a", 0, Some("update"))]),
                record(5, true, 2, &[("p:a", named, Some("snapshot"))]),
            ]);
            assert_eq!(value(&merged, "p:a").as_deref(), Some("snapshot"));
        }
    }

    #[test]
    fn a_write_with_no_value_removes_the_property() {
        let merged = merged_meta(&[
            record(1, true, 1, &[("p:a", 0, Some("1"))]),
            record(2, false, 2, &[("p:a", 0, None)]),
        ]);
        let (_, properties) = note_meta(&merged);
        assert!(properties.is_empty());
    }

    #[test]
    fn an_unreadable_records_entries_never_reach_the_note() {
        let mut bad = record(2, false, 2, &[("p:a", 0, Some("bad"))]);
        bad.judgement = Judgement::Unreadable;
        let merged = merged_meta(&[record(1, true, 1, &[("p:a", 0, Some("good"))]), bad]);
        assert_eq!(value(&merged, "p:a").as_deref(), Some("good"));
    }
}
