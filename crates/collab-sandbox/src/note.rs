//! The integration stage (sync-v2.md §22): a text-mode note's one yrs
//! document, the records it merged, and each writer's fuel allowance.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::Instant;

use sync_protocol::codec::Decode;
use sync_protocol::records::{Content, Cover, Payload, PayloadKind};
use sync_protocol::types::Id;

use crate::allowance::Allowance;
use crate::guest::{Failure, Guest};
use crate::op::{Mismatch, Op};
use crate::verdict::{Record, Verdict};
use crate::{Sandbox, Unreadable};

/// The most fuel one record's integration may use. A record that needs
/// more stops its lineage on this device.
pub const INTEGRATION_FUEL_LIMIT: u64 = 10_000_000_000;
/// The most memory a note's instance may hold. A note rebases at 16 MiB of
/// encoded document and stops at 32 MiB, and yrs holds a document in a few
/// times its encoded size.
pub const INTEGRATION_MEMORY_BYTES: usize = 1 << 30;
/// The fuel a call made for this device may use: an edit, undo, redo, or a
/// relative position.
const LOCAL_FUEL: u64 = 10_000_000_000;

/// How many bytes of deferred records stay in memory when the journal can
/// hand back the rest.
pub const DEFERRED_MEMORY_BYTES: usize = 8 << 20;

const PENDING: u32 = 1;
const TEXT_TOO_LARGE: u32 = 2;
const DOCUMENT_TOO_LARGE: u32 = 4;
const MISPLACED: u32 = 8;
const OVERLAP: u32 = 16;
/// The bits of a client ID below its card index (§22).
const LOAD_BITS: u32 = 29;

/// Where a record sits in its signer's chain, which tells what a snapshot
/// covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The signing device.
    pub device: Id,
    /// Its counter.
    pub counter: u64,
    /// The record's covers, for a snapshot.
    pub covers: Vec<Cover>,
}

impl Default for Place {
    fn default() -> Self {
        Self {
            device: Id([0; 16]),
            counter: 0,
            covers: Vec::new(),
        }
    }
}

impl Place {
    /// Whether a snapshot at this place covers the record at `other`.
    fn covers(&self, other: &Place) -> bool {
        self.covers
            .iter()
            .any(|cover| cover.device == other.device && other.counter <= cover.counter)
    }
}

/// A record the journal holds and the sandbox judged readable, as a note
/// opens from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalRecord {
    /// The caller's name for the record: its log sequence, or a number past
    /// every sequence for this device's records not yet committed.
    pub id: u64,
    /// Whether it's a snapshot.
    pub snapshot: bool,
    /// Where it sits in its signer's chain.
    pub place: Place,
    /// Its yrs bytes.
    pub bytes: Vec<u8>,
}

/// What a note opens from: its records in canonical order, each writer's
/// balance when the note last closed, and where deferred records come back
/// from.
#[derive(Default)]
pub struct Opened {
    /// The readable records of the lineage, in canonical order.
    pub records: Vec<JournalRecord>,
    /// Each writer's balance, so a writer in debt stays in debt.
    pub balances: Vec<(u32, i64)>,
    /// Hands back a deferred record the sandbox doesn't keep in memory.
    pub loader: Option<Loader>,
}

/// Reads a deferred record back from the journal by its ID.
pub type Loader = Box<dyn FnMut(u64) -> Option<RemoteRecord> + Send>;

/// A remote `text_snapshot` or `text_update` that passed the client checks
/// of §13 and reached the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRecord {
    /// The caller's name for the record, such as its log sequence; events
    /// about the record carry it.
    pub id: u64,
    /// The signer's card index in the workspace.
    pub writer: u32,
    /// How many cards the workspace indexed at the record's
    /// `policy_revision`.
    pub card_count: u32,
    /// The decrypted, unpadded payload.
    pub payload: Vec<u8>,
    /// The blob's file, when the payload's content is a blob.
    pub blob: Option<Vec<u8>>,
    /// Where it sits in its signer's chain.
    pub place: Place,
}

impl RemoteRecord {
    /// How many bytes the record holds in memory.
    fn size(&self) -> usize {
        self.payload.len() + self.blob.as_ref().map_or(0, Vec::len)
    }

    /// Whether it's a snapshot.
    fn is_snapshot(&self) -> bool {
        Payload::from_bytes(&self.payload)
            .is_ok_and(|payload| matches!(payload.kind, PayloadKind::TextSnapshot { .. }))
    }

    fn as_record(&self) -> Record<'_> {
        Record {
            payload: &self.payload,
            blob: self.blob.as_deref(),
            card_index: self.writer,
            card_count: self.card_count,
        }
    }

    /// The yrs bytes the payload holds inline or in its blob.
    fn yrs_bytes(&self) -> Option<Cow<'_, [u8]>> {
        let payload = Payload::from_bytes(&self.payload).ok()?;
        let content = match payload.kind {
            PayloadKind::TextSnapshot { content, .. } | PayloadKind::TextUpdate { content, .. } => {
                content
            }
            _ => return None,
        };
        match content {
            Content::Inline { bytes } => Some(Cow::Owned(bytes.0)),
            Content::Blob(_) => self.blob.as_deref().map(Cow::Borrowed),
        }
    }
}

/// Why a record stopped its lineage on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StopReason {
    /// Its integration passed 10,000,000,000 fuel units.
    Fuel,
    /// Its integration hit the instance's memory cap.
    Memory,
    /// The guest trapped while integrating it.
    Trap,
    /// It would take the note's text past 8 MiB.
    TextTooLarge,
    /// It would take the encoded document past 32 MiB.
    DocumentTooLarge,
}

impl StopReason {
    fn from_failure(failure: &Failure) -> Self {
        match failure {
            Failure::Fuel => Self::Fuel,
            Failure::Memory => Self::Memory,
            Failure::Trap(_) => Self::Trap,
        }
    }
}

/// Something the app records or shows about a note's records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The verdict on a record, which the journal keeps.
    Verdict {
        /// The record.
        id: u64,
        /// Its verdict.
        verdict: Verdict,
    },
    /// The record waits: its writer's allowance is spent, or an earlier
    /// record of the writer waits.
    Deferred {
        /// The record.
        id: u64,
    },
    /// The record merged into the note.
    Merged {
        /// The record.
        id: u64,
        /// The fuel its integration used.
        fuel: u64,
    },
    /// The record stopped the lineage on this device: the note keeps its
    /// last good state and waits for your choice.
    Stopped {
        /// The record.
        id: u64,
        /// Why.
        reason: StopReason,
    },
    /// The record passed the verdict after the lineage stopped, so it
    /// doesn't merge.
    Skipped {
        /// The record.
        id: u64,
    },
    /// The record holds an item that sits where its writer couldn't have
    /// put it (§22), so it doesn't merge and goes to review.
    Refused {
        /// The record.
        id: u64,
    },
    /// The record disagrees with what this device holds of the lineage. A
    /// snapshot made the device rebuild the document in canonical order.
    /// Either way it goes to review with its signer.
    Disagrees {
        /// The record.
        id: u64,
        /// The signer's card index.
        writer: u32,
    },
    /// yrs started or stopped holding updates whose dependencies haven't
    /// arrived, such as an update that depends on an unreadable record.
    Pending(bool),
    /// The instance was discarded and rebuilt from the journal.
    Rebuilt,
}

/// What a call did: events for the app, and the text changes the note's
/// document went through, in order, each against the text the one before it
/// left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Events about records.
    pub events: Vec<Event>,
    /// Text changes.
    pub changes: Vec<Op>,
}

impl Effects {
    fn extend(&mut self, other: Effects) {
        self.events.extend(other.events);
        self.changes.extend(other.changes);
    }
}

/// Why a call on a note's document failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DocumentError {
    /// The guest failed; the note's instance was rebuilt from the journal.
    #[error("the sandbox failed: {0}")]
    Sandbox(String),
    /// An edit doesn't fit the note's text.
    #[error("the edit doesn't fit the text")]
    Mismatch,
    /// A relative position doesn't decode or points outside `content`.
    #[error("the position doesn't point into the note")]
    Position,
    /// The device has no load number left to rebuild the document with.
    #[error("no load number is left")]
    NoLoad,
}

impl From<Mismatch> for DocumentError {
    fn from(_: Mismatch) -> Self {
        Self::Mismatch
    }
}

fn sandbox_error(failure: Failure) -> DocumentError {
    DocumentError::Sandbox(match failure {
        Failure::Fuel => "out of fuel".into(),
        Failure::Memory => "out of memory".into(),
        Failure::Trap(trap) => trap,
    })
}

/// Hands out the client ID for each load of a note's document, from the
/// device's load reservation: see
/// [`LoadReservation::client_ids`](crate::LoadReservation::client_ids).
pub type ClientIds = Box<dyn FnMut() -> Option<u64> + Send>;

/// The low 29 bits of a client ID hold the load number (§22).
const LOAD_MASK: u64 = (1 << 29) - 1;

/// The changes this device made under one load, for a `text_update` that
/// names that load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalUpdate {
    /// The load the changes went in under: the `load` field of the
    /// `text_update`.
    pub load: u32,
    /// The yrs update holding them.
    pub bytes: Vec<u8>,
}

/// Where a note's lineage stands on this device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineageStatus {
    /// Records merge as they arrive.
    #[default]
    Merging,
    /// yrs holds updates whose dependencies haven't arrived, such as an
    /// update that depends on an unreadable record: the device lists the
    /// lineage for review as text until another record supplies them.
    Pending,
    /// A record stopped the lineage on this device. The note keeps its last
    /// good state and waits for your choice; keeping that state writes a
    /// `file` operation that covers every live operation of the note, which
    /// ends the lineage, as a rebase from any device does.
    Stopped(StopReason),
}

/// A text-mode note's document in the sandbox.
pub struct Note {
    sandbox: Sandbox,
    /// The sandbox that judges records: the same one, except in a test that
    /// stands in a module whose verdict traps.
    verdicts: Sandbox,
    guest: Guest,
    /// The text's length in UTF-16 units.
    len: u32,
    allowances: HashMap<u32, Allowance>,
    /// Each writer's balance from the journal, until the writer first
    /// shows up.
    opening_balances: HashMap<u32, i64>,
    /// Records waiting on their writer's allowance or on an earlier update
    /// of the writer that yrs holds, in arrival order.
    deferred: VecDeque<Waiting>,
    /// The bytes of deferred records in memory.
    deferred_bytes: usize,
    loader: Option<Loader>,
    /// The yrs bytes of every record and local change merged, in order, to
    /// rebuild the document from.
    journal: Vec<JournalRecord>,
    /// The next number for a local change in the journal.
    next_local: u64,
    /// The writers whose updates wait in yrs.
    pending_writers: BTreeSet<u32>,
    /// Local changes not yet taken for a `text_update`, with the client ID
    /// each went in under.
    unsent: Vec<(u64, Vec<u8>)>,
    stop: Option<StopReason>,
    pending: bool,
    /// The client ID the document inserts under.
    client: u64,
    client_ids: ClientIds,
    /// Whether the next integration traps, as tests ask; always false
    /// outside them.
    trap_next: bool,
}

/// A record that waits, in memory or in the journal.
struct Waiting {
    id: u64,
    writer: u32,
    snapshot: bool,
    size: usize,
    /// The record, or `None` while only the journal holds it.
    record: Option<RemoteRecord>,
}

impl Note {
    /// Loads a note's document from the records the journal holds for its
    /// lineage, in canonical order, skipping the ones judged unreadable.
    /// The document inserts under the client ID `client_ids` hands out, and
    /// takes another for each rebuild.
    pub fn open(
        sandbox: &Sandbox,
        journal: Vec<Vec<u8>>,
        client_ids: ClientIds,
    ) -> Result<Self, DocumentError> {
        let records = journal
            .into_iter()
            .enumerate()
            .map(|(id, bytes)| JournalRecord {
                id: id as u64,
                snapshot: id == 0,
                place: Place::default(),
                bytes,
            })
            .collect();
        Self::open_lineage(
            sandbox,
            Opened {
                records,
                ..Opened::default()
            },
            client_ids,
        )
    }

    /// [`Self::open`] from the journal's records with their places, each
    /// writer's balance, and where deferred records come back from.
    pub fn open_lineage(
        sandbox: &Sandbox,
        opened: Opened,
        mut client_ids: ClientIds,
    ) -> Result<Self, DocumentError> {
        let client = client_ids().ok_or(DocumentError::NoLoad)?;
        let journal = opened.records;
        let (guest, len, pending, _) = load(sandbox, client, &journal)?;
        let mut note = Self {
            sandbox: sandbox.clone(),
            verdicts: sandbox.clone(),
            guest,
            len,
            allowances: HashMap::new(),
            opening_balances: opened.balances.into_iter().collect(),
            deferred: VecDeque::new(),
            deferred_bytes: 0,
            loader: opened.loader,
            next_local: journal.iter().map(|r| r.id + 1).max().unwrap_or(0),
            pending_writers: BTreeSet::new(),
            journal,
            unsent: Vec::new(),
            stop: None,
            pending,
            client,
            client_ids,
            trap_next: false,
        };
        note.note_pending();
        Ok(note)
    }

    /// Asks the document which writers' updates wait in yrs.
    fn note_pending(&mut self) {
        self.pending_writers.clear();
        if !self.pending {
            return;
        }
        self.guest.begin(LOCAL_FUEL);
        let clients = match self.guest.call::<(), u32>("doc_pending_clients", ()) {
            Ok(0) => self.guest.output().unwrap_or_default(),
            _ => Vec::new(),
        };
        self.pending_writers = clients
            .chunks_exact(8)
            .map(|word| {
                let client = u64::from_le_bytes(word.try_into().expect("eight bytes"));
                (client >> LOAD_BITS) as u32
            })
            .collect();
    }

    /// The text's length in UTF-16 units.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the note holds no text.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether a record stopped the lineage on this device.
    pub fn is_stopped(&self) -> bool {
        self.stop.is_some()
    }

    /// Where the note's lineage stands on this device.
    pub fn status(&self) -> LineageStatus {
        match (self.stop, self.pending) {
            (Some(reason), _) => LineageStatus::Stopped(reason),
            (None, true) => LineageStatus::Pending,
            (None, false) => LineageStatus::Merging,
        }
    }

    /// The client ID the document inserts under now.
    pub fn client_id(&self) -> u64 {
        self.client
    }

    /// The load the document inserts under now, which a `text_update` of
    /// its changes names.
    pub fn load(&self) -> u32 {
        (self.client & LOAD_MASK) as u32
    }

    /// Whether yrs holds updates whose dependencies haven't arrived.
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// A writer's allowance balance, refilled to `now`.
    pub fn balance(&mut self, writer: u32, now: Instant) -> i64 {
        self.allowance(writer, now).balance()
    }

    /// Sets a writer's balance, as the fixtures' base states do.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_balance(&mut self, writer: u32, balance: i64, now: Instant) {
        self.allowances
            .insert(writer, Allowance::with_balance(balance, now));
    }

    fn allowance(&mut self, writer: u32, now: Instant) -> &mut Allowance {
        let opening = self.opening_balances.remove(&writer);
        let allowance = self.allowances.entry(writer).or_insert_with(|| match opening {
            Some(balance) => Allowance::with_balance(balance, now),
            None => Allowance::new(now),
        });
        allowance.refill(now);
        allowance
    }

    /// Each writer's balance as of `now`, which the journal keeps, so
    /// reopening the note doesn't clear a writer's debt.
    pub fn balances(&mut self, now: Instant) -> Vec<(u32, i64)> {
        let writers: Vec<u32> = self
            .allowances
            .keys()
            .chain(self.opening_balances.keys())
            .copied()
            .collect();
        let mut balances: Vec<(u32, i64)> = writers
            .into_iter()
            .map(|writer| (writer, self.balance(writer, now)))
            .collect();
        balances.sort_unstable();
        balances
    }

    /// Takes in a remote record: judges and merges it, or defers it while
    /// its writer's allowance is spent.
    pub fn receive(&mut self, record: RemoteRecord, now: Instant) -> Effects {
        let mut effects = Effects::default();
        let snapshot = record.is_snapshot();
        let waiting = self
            .deferred
            .iter()
            .any(|deferred| deferred.writer == record.writer);
        if waiting
            || !self.allowance(record.writer, now).in_credit()
            || (!snapshot && self.pending_writers.contains(&record.writer))
        {
            effects.events.push(Event::Deferred { id: record.id });
            self.defer(record, snapshot);
        } else {
            let released = !self.pending_writers.is_empty();
            effects.extend(self.process(&record, now));
            if released {
                effects.extend(self.wake(now));
            }
        }
        effects
    }

    /// Keeps a record for later: in memory while the waiting records fit,
    /// and otherwise only in the journal, when the journal can hand it back.
    fn defer(&mut self, record: RemoteRecord, snapshot: bool) {
        let size = record.size();
        let fits = self.deferred_bytes + size <= DEFERRED_MEMORY_BYTES;
        let keep = fits || self.loader.is_none();
        if keep {
            self.deferred_bytes += size;
        }
        self.deferred.push_back(Waiting {
            id: record.id,
            writer: record.writer,
            snapshot,
            size,
            record: keep.then_some(record),
        });
    }

    /// Takes the waiting record at `index`, from memory or the journal.
    fn take_waiting(&mut self, index: usize) -> Option<RemoteRecord> {
        let waiting = self
            .deferred
            .remove(index)
            .expect("the index came from the queue");
        match waiting.record {
            Some(record) => {
                self.deferred_bytes -= waiting.size;
                Some(record)
            }
            None => self.loader.as_mut().and_then(|load| load(waiting.id)),
        }
    }

    /// How many bytes of waiting records sit in memory.
    pub fn deferred_bytes(&self) -> usize {
        self.deferred_bytes
    }

    /// Runs the deferred records whose writers' allowances refilled by
    /// `now`.
    pub fn wake(&mut self, now: Instant) -> Effects {
        let mut effects = Effects::default();
        loop {
            // Each writer's first waiting record, in arrival order.
            let mut firsts: Vec<(usize, u32, bool)> = Vec::new();
            for (index, record) in self.deferred.iter().enumerate() {
                if !firsts.iter().any(|(_, writer, _)| *writer == record.writer) {
                    firsts.push((index, record.writer, record.snapshot));
                }
            }
            let ready = firsts.into_iter().find(|(_, writer, snapshot)| {
                self.allowance(*writer, now).in_credit()
                    && (*snapshot || !self.pending_writers.contains(writer))
            });
            let Some((index, _, _)) = ready else { break };
            let Some(record) = self.take_waiting(index) else {
                continue;
            };
            effects.extend(self.process(&record, now));
        }
        effects
    }

    /// When a deferred record's writer next gets a refill, if any record
    /// waits.
    pub fn next_wake(&self) -> Option<Instant> {
        self.deferred
            .iter()
            .filter(|record| !self.pending_writers.contains(&record.writer))
            .filter_map(|record| self.allowances.get(&record.writer))
            .map(Allowance::next_refill)
            .min()
    }

    /// How many records wait on their writers' allowances.
    pub fn deferred(&self) -> usize {
        self.deferred.len()
    }

    fn process(&mut self, record: &RemoteRecord, now: Instant) -> Effects {
        let mut effects = Effects::default();
        let verdict = self.verdicts.verdict(&record.as_record());
        self.allowance(record.writer, now).charge(verdict.fuel);
        effects.events.push(Event::Verdict {
            id: record.id,
            verdict,
        });
        if verdict.outcome.is_err() {
            return effects;
        }
        let Some(bytes) = record.yrs_bytes().filter(|_| self.stop.is_none()) else {
            effects.events.push(Event::Skipped { id: record.id });
            return effects;
        };
        let entry = JournalRecord {
            id: record.id,
            snapshot: record.is_snapshot(),
            place: record.place.clone(),
            bytes: bytes.to_vec(),
        };
        if entry.snapshot {
            let (disagrees, fuel) = self.compare(&entry);
            self.allowance(record.writer, now).charge(fuel);
            if disagrees {
                self.rebuild_with(record.writer, entry, now, &mut effects);
                return effects;
            }
        }
        let trap = std::mem::take(&mut self.trap_next);
        let stop = match integrate(&mut self.guest, &bytes, self.len, trap) {
            Ok((op, status, fuel)) => {
                self.allowance(record.writer, now).charge(fuel);
                if status & MISPLACED != 0 {
                    // The instance holds the record: go back without it.
                    effects.events.push(Event::Refused { id: record.id });
                    match self.rebuild() {
                        Ok(fuel) => {
                            self.allowance(record.writer, now).charge(fuel);
                            effects.events.push(Event::Rebuilt);
                        }
                        Err(_) => self.stop = Some(StopReason::Trap),
                    }
                    return effects;
                }
                if status & TEXT_TOO_LARGE != 0 {
                    Some(StopReason::TextTooLarge)
                } else if status & DOCUMENT_TOO_LARGE != 0 {
                    Some(StopReason::DocumentTooLarge)
                } else {
                    if status & OVERLAP != 0 && !entry.snapshot {
                        let (disagrees, fuel) = self.compare(&entry);
                        self.allowance(record.writer, now).charge(fuel);
                        if disagrees {
                            effects.events.push(Event::Disagrees {
                                id: record.id,
                                writer: record.writer,
                            });
                        }
                    }
                    self.journal.push(entry);
                    self.len = op.target_len();
                    effects.changes.push(op);
                    effects.events.push(Event::Merged {
                        id: record.id,
                        fuel,
                    });
                    let pending = status & PENDING != 0;
                    if pending != self.pending {
                        self.pending = pending;
                        effects.events.push(Event::Pending(pending));
                    }
                    self.note_pending();
                    None
                }
            }
            Err((failure, fuel)) => {
                self.allowance(record.writer, now).charge(fuel);
                Some(StopReason::from_failure(&failure))
            }
        };
        if let Some(reason) = stop {
            self.stop = Some(reason);
            effects.events.push(Event::Stopped {
                id: record.id,
                reason,
            });
            // The instance holds the record that stopped the lineage, or
            // trapped: go back to the last good state.
            if self.rebuild().is_ok() {
                effects.events.push(Event::Rebuilt);
            }
        }
        effects
    }

    /// Compares a record with what the device holds of its lineage: the
    /// records in the journal and those that wait. Returns whether they
    /// disagree and the fuel the comparison used. A comparison that fails
    /// counts as a disagreement.
    fn compare(&mut self, entry: &JournalRecord) -> (bool, u64) {
        let mut input = Vec::new();
        let push = |input: &mut Vec<u8>, bytes: &[u8]| {
            input.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            input.extend_from_slice(bytes);
        };
        push(&mut input, &entry.bytes);
        let covers = |place: &Place| entry.snapshot && entry.place.covers(place);
        for held in &self.journal {
            input.push(u8::from(covers(&held.place)));
            push(&mut input, &held.bytes);
        }
        let waiting: Vec<u64> = self.deferred.iter().map(|w| w.id).collect();
        for id in waiting {
            let index = self.deferred.iter().position(|w| w.id == id);
            let Some(index) = index else { continue };
            let record = match &self.deferred[index].record {
                Some(record) => Some(record.clone()),
                None => self.loader.as_mut().and_then(|load| load(id)),
            };
            let Some(record) = record else { continue };
            if let Some(bytes) = record.yrs_bytes() {
                input.push(u8::from(covers(&record.place)));
                push(&mut input, &bytes);
            }
        }
        let run = || -> Result<(bool, u64), Failure> {
            let mut guest =
                Guest::new(&self.sandbox, INTEGRATION_MEMORY_BYTES, INTEGRATION_FUEL_LIMIT)?;
            let ptr = guest.write(&input)?;
            let code: u32 = guest.call("compare_records", (ptr, input.len() as u32))?;
            Ok((code != 0, guest.used()))
        };
        run().unwrap_or((true, 0))
    }

    /// Takes in a snapshot that disagrees with what the device holds:
    /// discards the document and rebuilds it from the lineage's live
    /// records in canonical order, charging the work to the signer.
    fn rebuild_with(
        &mut self,
        writer: u32,
        entry: JournalRecord,
        now: Instant,
        effects: &mut Effects,
    ) {
        let id = entry.id;
        let Ok(before) = self.text() else {
            self.stop = Some(StopReason::Trap);
            return;
        };
        self.journal.push(entry);
        let live: Vec<Place> = self
            .journal
            .iter()
            .filter(|record| record.snapshot)
            .map(|record| record.place.clone())
            .collect();
        self.journal.retain(|record| {
            record.place == Place::default()
                || !live.iter().any(|snapshot| snapshot.covers(&record.place))
        });
        self.journal.sort_by_key(|record| (!record.snapshot, record.id));
        match self.rebuild() {
            Ok(fuel) => self.allowance(writer, now).charge(fuel),
            Err(_) => {
                self.stop = Some(StopReason::Trap);
                return;
            }
        }
        let after = self.text().unwrap_or_default();
        effects.changes.push(crate::op::difference(&before, &after));
        effects.events.push(Event::Disagrees { id, writer });
        effects.events.push(Event::Rebuilt);
    }

    /// Discards the instance and loads the journal into a new one, under the
    /// next client ID. Returns the fuel it used.
    fn rebuild(&mut self) -> Result<u64, DocumentError> {
        let client = (self.client_ids)().ok_or(DocumentError::NoLoad)?;
        let (guest, len, pending, fuel) = load(&self.sandbox, client, &self.journal)?;
        self.guest = guest;
        self.len = len;
        self.pending = pending;
        self.client = client;
        self.note_pending();
        Ok(fuel)
    }

    /// After a failed call: rebuild, and stop the lineage if even that
    /// fails.
    fn recover(&mut self, failure: Failure) -> DocumentError {
        if self.rebuild().is_err() {
            self.stop.get_or_insert(StopReason::from_failure(&failure));
        }
        sandbox_error(failure)
    }

    /// Keeps the local change the last call made, for the journal and the
    /// next `text_update`.
    fn keep_local_change(&mut self) -> Result<(), Failure> {
        self.guest.call::<(), u32>("doc_take_update", ())?;
        let update = self.guest.output()?;
        if !update.is_empty() {
            self.journal.push(JournalRecord {
                id: self.next_local,
                snapshot: false,
                place: Place::default(),
                bytes: update.clone(),
            });
            self.next_local += 1;
            self.unsent.push((self.client, update));
        }
        Ok(())
    }

    /// Applies an edit made on this device, against the note's text, as a
    /// transaction of local origin; undo takes it back only when
    /// `undoable`. An outside edit of the file isn't. `now` is the time in
    /// milliseconds, which groups edits into undo steps.
    pub fn local(&mut self, op: &Op, undoable: bool, now: u64) -> Result<(), DocumentError> {
        if op.base_len() != self.len {
            return Err(DocumentError::Mismatch);
        }
        if op.is_identity() {
            return Ok(());
        }
        let result = (|| {
            self.guest.begin(LOCAL_FUEL);
            let bytes = op.encode();
            let ptr = self.guest.write(&bytes)?;
            let code: u32 = self.guest.call(
                "doc_local",
                (
                    ptr,
                    bytes.len() as u32,
                    u32::from(undoable),
                    (now >> 32) as u32,
                    now as u32,
                ),
            )?;
            if code != 0 {
                return Err(Failure::Trap(format!("doc_local returned {code}")));
            }
            self.keep_local_change()
        })();
        match result {
            Ok(()) => {
                self.len = op.target_len();
                Ok(())
            }
            Err(failure) => {
                // The edit is in the editor but not in the document.
                self.stop = Some(StopReason::from_failure(&failure));
                Err(self.recover(failure))
            }
        }
    }

    fn undo_or_redo(&mut self, export: &str, now: u64) -> Result<Op, DocumentError> {
        let result = (|| {
            self.guest.begin(LOCAL_FUEL);
            let code: u32 = self.guest.call(export, ((now >> 32) as u32, now as u32))?;
            if code != 0 {
                return Err(Failure::Trap(format!("{export} returned {code}")));
            }
            let op = Op::decode(&self.guest.output()?, self.len)
                .map_err(|_| Failure::Trap(format!("{export} returned a bad change")))?;
            self.keep_local_change()?;
            Ok(op)
        })();
        match result {
            Ok(op) => {
                self.len = op.target_len();
                Ok(op)
            }
            Err(failure) => Err(self.recover(failure)),
        }
    }

    /// Takes back your last undo step. Returns the text change it made.
    pub fn undo(&mut self, now: u64) -> Result<Op, DocumentError> {
        self.undo_or_redo("doc_undo", now)
    }

    /// Reapplies your last undone step. Returns the text change it made.
    pub fn redo(&mut self, now: u64) -> Result<Op, DocumentError> {
        self.undo_or_redo("doc_redo", now)
    }

    /// Whether undo has a step of yours to take back, and redo one to
    /// reapply.
    pub fn undo_state(&mut self) -> Result<(bool, bool), DocumentError> {
        self.guest.begin(LOCAL_FUEL);
        match self.guest.call::<(), u32>("doc_undo_state", ()) {
            Ok(bits) if bits <= 3 => Ok((bits & 1 != 0, bits & 2 != 0)),
            Ok(bits) => Err(DocumentError::Sandbox(format!(
                "doc_undo_state returned {bits}"
            ))),
            Err(failure) => Err(self.recover(failure)),
        }
    }

    /// Ends the current undo step, so your next edit starts another.
    pub fn end_undo_step(&mut self) -> Result<(), DocumentError> {
        self.guest.begin(LOCAL_FUEL);
        let result = self.guest.call::<(), u32>("doc_end_undo_step", ());
        result.map(|_| ()).map_err(|failure| self.recover(failure))
    }

    /// Your changes since the last call, one update per load they went in
    /// under, for the `text_update`s that carry them; empty when there were
    /// none, or once the lineage stopped.
    ///
    /// When merging them fails, they stay for the next call.
    pub fn take_updates(&mut self) -> Result<Vec<LocalUpdate>, DocumentError> {
        let unsent = std::mem::take(&mut self.unsent);
        if self.stop.is_some() {
            return Ok(Vec::new());
        }
        let mut updates = Vec::new();
        let mut rest = unsent.as_slice();
        while let Some((client, _)) = rest.first() {
            let run = rest.iter().take_while(|(other, _)| other == client).count();
            let (group, tail) = rest.split_at(run);
            rest = tail;
            let bytes = match group {
                [(_, update)] => update.clone(),
                _ => match self.merge(group.iter().map(|(_, update)| update.as_slice())) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        // Keep them ahead of any change made after this call.
                        let later = std::mem::replace(&mut self.unsent, unsent);
                        self.unsent.extend(later);
                        return Err(error);
                    }
                },
            };
            updates.push(LocalUpdate {
                load: (client & LOAD_MASK) as u32,
                bytes,
            });
        }
        Ok(updates)
    }

    fn merge<'a>(
        &mut self,
        updates: impl Iterator<Item = &'a [u8]>,
    ) -> Result<Vec<u8>, DocumentError> {
        let mut input = Vec::new();
        for update in updates {
            input.extend_from_slice(&(update.len() as u32).to_le_bytes());
            input.extend_from_slice(update);
        }
        let result = (|| {
            self.guest.begin(LOCAL_FUEL);
            let ptr = self.guest.write(&input)?;
            let code: u32 = self
                .guest
                .call("merge_updates", (ptr, input.len() as u32))?;
            if code != 0 {
                return Err(Failure::Trap(format!("merge_updates returned {code}")));
            }
            self.guest.output()
        })();
        result.map_err(|failure| self.recover(failure))
    }

    fn read(&mut self, export: &str) -> Result<Vec<u8>, DocumentError> {
        let result = (|| {
            self.guest.begin(LOCAL_FUEL);
            let code: u32 = self.guest.call(export, ())?;
            if code != 0 {
                return Err(Failure::Trap(format!("{export} returned {code}")));
            }
            self.guest.output()
        })();
        result.map_err(|failure| self.recover(failure))
    }

    /// The document encoded with garbage collection on, as a snapshot holds
    /// it.
    pub fn state(&mut self) -> Result<Vec<u8>, DocumentError> {
        self.read("doc_state")
    }

    /// The note's text.
    pub fn text(&mut self) -> Result<String, DocumentError> {
        let bytes = self.read("doc_text")?;
        String::from_utf8(bytes).map_err(|_| DocumentError::Sandbox("text isn't UTF-8".into()))
    }

    /// The relative position of the UTF-16 offset `index`, for presence,
    /// sticking to the character after it when `after` is set.
    pub fn encode_position(&mut self, index: u32, after: bool) -> Result<Vec<u8>, DocumentError> {
        self.guest.begin(LOCAL_FUEL);
        let result = self
            .guest
            .call::<(u32, u32), u32>("doc_encode_position", (index, u32::from(after)));
        match result {
            Ok(0) => self.guest.output().map_err(|failure| self.recover(failure)),
            Ok(_) => Err(DocumentError::Position),
            Err(failure) => Err(self.recover(failure)),
        }
    }

    /// The UTF-16 offset a collaborator's relative position points at now.
    pub fn decode_position(&mut self, bytes: &[u8]) -> Result<u32, DocumentError> {
        let result = (|| {
            self.guest.begin(LOCAL_FUEL);
            let ptr = self.guest.write(bytes)?;
            let code: u32 = self
                .guest
                .call("doc_decode_position", (ptr, bytes.len() as u32))?;
            if code != 0 {
                return Ok(None);
            }
            let out = self.guest.output()?;
            Ok(out
                .get(..4)
                .map(|word| u32::from_le_bytes(word.try_into().expect("four bytes"))))
        })();
        match result {
            Ok(Some(index)) => Ok(index),
            Ok(None) => Err(DocumentError::Position),
            Err(failure) => {
                self.recover(failure);
                Err(DocumentError::Position)
            }
        }
    }

    /// Makes the instance trap while it integrates the next record that
    /// passes the verdict, as the fixtures' trap cases ask: a bug in the
    /// guest, such as a panic in its yrs, would trap the same way.
    #[cfg(any(test, feature = "test-support"))]
    pub fn trap_next_integration(&mut self) {
        self.trap_next = true;
    }

    /// Judges the records after this call with `sandbox`'s module, as the
    /// fixtures' verdict trap cases ask.
    #[cfg(test)]
    pub(crate) fn judge_with(&mut self, sandbox: Sandbox) {
        self.verdicts = sandbox;
    }

    /// The memory the note's instance holds, in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.guest.memory_bytes()
    }
}

/// A new instance holding a document that inserts under `client`, with the
/// journal merged in: the instance, the text's length, whether yrs holds
/// pending updates and the fuel it used.
fn load(
    sandbox: &Sandbox,
    client: u64,
    journal: &[JournalRecord],
) -> Result<(Guest, u32, bool, u64), DocumentError> {
    let mut guest =
        Guest::new(sandbox, INTEGRATION_MEMORY_BYTES, LOCAL_FUEL).map_err(sandbox_error)?;
    guest.begin(LOCAL_FUEL);
    let code: u32 = guest
        .call("doc_open", ((client >> 32) as u32, client as u32))
        .map_err(sandbox_error)?;
    if code != 0 {
        return Err(DocumentError::Sandbox(format!("doc_open returned {code}")));
    }
    let mut len = 0;
    let mut pending = false;
    let mut used = 0;
    for record in journal {
        let (op, status, fuel) = integrate(&mut guest, &record.bytes, len, false)
            .map_err(|(failure, _)| sandbox_error(failure))?;
        len = op.target_len();
        pending = status & PENDING != 0;
        used += fuel;
    }
    Ok((guest, len, pending, used))
}

/// Merges yrs bytes into the guest's document, returning the text change,
/// the status bits and the fuel used, or the failure and the fuel used.
/// With `trap` set, the call hands the guest a pointer past its memory, so
/// the guest traps reading it, as a bug in the guest would make it.
fn integrate(
    guest: &mut Guest,
    bytes: &[u8],
    len: u32,
    trap: bool,
) -> Result<(Op, u32, u64), (Failure, u64)> {
    guest.begin(INTEGRATION_FUEL_LIMIT);
    let result = (|| {
        let mut ptr = guest.write(bytes)?;
        if trap {
            ptr = u32::MAX - bytes.len() as u32;
        }
        let code: u32 = guest.call("doc_integrate", (ptr, bytes.len() as u32))?;
        if code != 0 {
            return Err(Failure::Trap(format!("doc_integrate returned {code}")));
        }
        let out = guest.output()?;
        let (word, op) = out
            .split_first_chunk::<4>()
            .ok_or_else(|| Failure::Trap("doc_integrate returned no status".into()))?;
        let op = Op::decode(op, len)
            .map_err(|_| Failure::Trap("doc_integrate returned a bad change".into()))?;
        Ok((op, u32::from_le_bytes(*word)))
    })();
    let fuel = guest.used();
    result
        .map(|(op, status)| (op, status, fuel))
        .map_err(|failure| (failure, fuel))
}

impl Sandbox {
    /// A lineage's first document (§22): one transaction by client ID 0
    /// that inserts `text` into `content`, encoded with garbage collection
    /// on.
    pub fn first_document(&self, text: &str) -> Result<Vec<u8>, DocumentError> {
        let mut guest =
            Guest::new(self, INTEGRATION_MEMORY_BYTES, LOCAL_FUEL).map_err(sandbox_error)?;
        guest.begin(LOCAL_FUEL);
        let ptr = guest.write(text.as_bytes()).map_err(sandbox_error)?;
        let code: u32 = guest
            .call("first_document", (ptr, text.len() as u32))
            .map_err(sandbox_error)?;
        if code != 0 {
            return Err(DocumentError::Sandbox(format!(
                "first_document returned {code}"
            )));
        }
        guest.output().map_err(sandbox_error)
    }
}

impl Unreadable {
    /// Whether the record's bytes, not the limits, made it unreadable.
    pub fn is_structural(self) -> bool {
        !matches!(self, Self::Fuel | Self::Memory | Self::Trap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;
    use serde_json::Value;
    use sync_protocol::types::from_hex;

    fn hex(value: &Value) -> Vec<u8> {
        from_hex(value.as_str().expect("a hex string")).expect("hexadecimal")
    }

    /// The fixture's verdict trap cases: a stand-in module, whose `verdict`
    /// export traps, judges some records, as a guest that panics would.
    /// Each such record is unreadable, and the next valid record that the
    /// pinned module judges still merges.
    #[test]
    fn a_trap_in_the_verdict_makes_one_record_unreadable_and_the_next_applies() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/workspace-format/fixtures/sync-v2-collaboration.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let cases: Vec<&Value> = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["kind"] == "verdict_trap")
            .collect();
        assert_eq!(cases.len(), 1);
        for backend in [
            Backend::Cranelift,
            #[cfg(feature = "pulley")]
            Backend::Pulley,
        ] {
            let pinned = Sandbox::new(backend).unwrap();
            for case in &cases {
                let name = &case["name"];
                let stand_in =
                    Sandbox::with_module(backend, case["module_wat"].as_str().unwrap().as_bytes())
                        .unwrap();
                let mut load = 0;
                let mut note = Note::open(
                    &pinned,
                    vec![hex(&case["base"])],
                    Box::new(move || {
                        load += 1;
                        Some((3 << 29) + load)
                    }),
                )
                .unwrap();
                for step in case["steps"].as_array().unwrap() {
                    note.verdicts = if step["judge"] == "stand_in" {
                        stand_in.clone()
                    } else {
                        pinned.clone()
                    };
                    let receive = &step["receive"];
                    let id = receive["id"].as_u64().unwrap();
                    let effects = note.receive(
                        RemoteRecord {
                            id,
                            writer: receive["card_index"].as_u64().unwrap() as u32,
                            card_count: receive["card_count"].as_u64().unwrap() as u32,
                            payload: hex(&receive["payload"]),
                            blob: None,
                            place: Place::default(),
                        },
                        Instant::now(),
                    );
                    let outcome = effects
                        .events
                        .iter()
                        .find_map(|event| match event {
                            Event::Verdict { verdict, .. } => Some(verdict.outcome),
                            _ => None,
                        })
                        .expect("a verdict");
                    let merged = effects
                        .events
                        .iter()
                        .any(|event| matches!(event, Event::Merged { .. }));
                    let got = match outcome {
                        Err(reason) => reason.code(),
                        Ok(()) if merged => "merged",
                        Ok(()) => "other",
                    };
                    assert_eq!(got, step["expect"], "{name} record {id}");
                }
                assert!(!note.is_stopped(), "{name}");
                assert_eq!(note.text().unwrap(), case["text"], "{name}");
            }
        }
    }
}
