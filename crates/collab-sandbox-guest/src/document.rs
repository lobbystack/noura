//! The integration stage (sync-v2.md §22): a text-mode note's one yrs
//! document, whose only root is the text `content`, with its client ID, its
//! undo and its relative positions.
//!
//! Local edits enter as transactions of local origin, remote records as
//! transactions of remote origin, and undo tracks only the local ones, so it
//! never takes back a collaborator's text. Every transaction reports the text
//! change it made as an [`Op`] over UTF-16 units.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use yrs::sync::Clock;
use yrs::types::Delta;
use yrs::types::text::TextEvent;
use yrs::undo::Options as UndoOptions;
use yrs::updates::decoder::Decode as _;
use yrs::updates::encoder::Encode as _;
use yrs::{
    Any, Assoc, ClientID, Doc, GetString, IndexedSequence, Observable, OffsetKind, Options, Origin,
    Out, ReadTxn, StateVector, StickyIndex, Subscription, Text, TextRef, Transact, UndoManager,
    Update,
};

use crate::lib0::{CLIENT_LIMIT, ROOT};
use crate::op::{Component, Op};

/// The origin of transactions that carry your own edits, which undo tracks.
const LOCAL: &str = "local";
/// The origin of transactions that carry an outside edit of the file: this
/// device's change, which undo never takes back.
const OUTSIDE: &str = "outside";
/// The origin of transactions that integrate remote records.
const REMOTE: &str = "remote";

/// The most a note's text may hold, in UTF-8 bytes, before integration stops
/// its lineage.
pub const MAX_TEXT_BYTES: u64 = 8 << 20;
/// The most a note's encoded document may hold before integration stops its
/// lineage.
pub const MAX_DOCUMENT_BYTES: u64 = 32 << 20;
/// How much the encoded document can grow per byte of an integrated update,
/// at most. The document is encoded in full only when the estimate this
/// bound gives passes [`MAX_DOCUMENT_BYTES`].
const GROWTH_PER_BYTE: u64 = 16;

/// Status bits [`Note::integrate`] returns.
pub mod status {
    /// yrs holds structs or deletions whose dependencies haven't arrived.
    pub const PENDING: u32 = 1;
    /// The text passed [`super::MAX_TEXT_BYTES`].
    pub const TEXT_TOO_LARGE: u32 = 2;
    /// The encoded document passed [`super::MAX_DOCUMENT_BYTES`].
    pub const DOCUMENT_TOO_LARGE: u32 = 4;
    /// The record holds an item that sits where its writer couldn't have
    /// put it, so the host goes back to the document before the record.
    pub const MISPLACED: u32 = 8;
    /// The record holds structs at clocks the document already held, which
    /// yrs skips, so the host compares the two copies.
    pub const OVERLAP: u32 = 16;
}

/// Why a call on the document failed without trapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocError {
    /// The bytes aren't a yrs update, or yrs refused them.
    Update,
    /// A local edit reaches past the end of the text.
    Range,
    /// A relative position doesn't decode or doesn't point into `content`.
    Position,
    /// The client ID isn't below 2^53.
    Client,
}

thread_local! {
    /// The time the host passed with the current call, in milliseconds,
    /// which groups local edits into undo steps.
    static NOW: Cell<u64> = const { Cell::new(0) };
}

struct HostClock;

impl Clock for HostClock {
    /// The host's time plus one: yrs's undo manager reads a last change at
    /// time 0 as no change at all, so an edit at the host's time 0 would
    /// never group with the next one.
    fn now(&self) -> u64 {
        NOW.with(Cell::get).saturating_add(1)
    }
}

#[derive(Default)]
struct Collected {
    op: Op,
    local_updates: Vec<Vec<u8>>,
}

/// A note's document.
pub struct Note {
    doc: Doc,
    text: TextRef,
    undo: UndoManager,
    collected: Rc<RefCell<Collected>>,
    _text_events: Subscription,
    _updates: Subscription,
    /// An upper bound on the encoded document's length.
    size_bound: u64,
    /// An upper bound on the text's length in UTF-8 bytes, so a record
    /// costs a copy of the whole text only when the bound passes
    /// [`MAX_TEXT_BYTES`].
    text_bound: u64,
}

impl Note {
    /// An empty document that inserts under `client`.
    pub fn new(client: u64) -> Result<Self, DocError> {
        if client >= CLIENT_LIMIT {
            return Err(DocError::Client);
        }
        let doc = new_doc(client);
        let text = doc.get_or_insert_text(ROOT);
        let collected = Rc::new(RefCell::new(Collected::default()));

        let sink = collected.clone();
        let text_events = text.observe(move |txn, event: &TextEvent| {
            let mut collected = sink.borrow_mut();
            for delta in event.delta(txn) {
                collected.op.push(component(delta));
            }
        });
        let sink = collected.clone();
        let remote = Origin::from(REMOTE);
        let updates = doc
            .observe_update_v1(move |txn, event| {
                if txn.origin() != Some(&remote) {
                    sink.borrow_mut().local_updates.push(event.update.clone());
                }
            })
            .map_err(|_| DocError::Update)?;

        let mut tracked = HashSet::new();
        tracked.insert(Origin::from(LOCAL));
        let undo = UndoManager::with_scope_and_options(
            &doc,
            &text,
            UndoOptions {
                capture_timeout_millis: 500,
                tracked_origins: tracked,
                capture_transaction: None,
                timestamp: Arc::new(HostClock),
                init_undo_stack: Vec::new(),
                init_redo_stack: Vec::new(),
            },
        );
        Ok(Self {
            doc,
            text,
            undo,
            collected,
            _text_events: text_events,
            _updates: updates,
            size_bound: 0,
            text_bound: 0,
        })
    }

    fn take_op(&self) -> Op {
        std::mem::take(&mut self.collected.borrow_mut().op)
    }

    /// Moves the text's bound past `op`: its inserted bytes, less one byte
    /// for each UTF-16 unit it deletes, which took at least one.
    fn bound_text(&mut self, op: &Op) {
        self.text_bound = self
            .text_bound
            .saturating_add(op.byte_len() as u64)
            .saturating_sub(op.deleted_len());
    }

    /// Merges a record's yrs bytes, which passed the verdict, as a remote
    /// transaction. Returns the text change and the status bits.
    pub fn integrate(&mut self, bytes: &[u8]) -> Result<(Op, u32), DocError> {
        let update = Update::decode_v1(bytes).map_err(|_| DocError::Update)?;
        let items = crate::lib0::read_items(bytes).map_err(|_| DocError::Update)?;
        let before = self.doc.transact().state_vector();
        let overlap = items.clients.iter().any(|(client, items)| {
            items
                .first()
                .is_some_and(|item| item.clock < u64::from(before.get(&ClientID::new(*client))))
        });
        {
            let mut txn = self.doc.transact_mut_with(REMOTE);
            txn.apply_update(update).map_err(|_| DocError::Update)?;
        }
        let misplaced = self.doc.transact().store().misplaced_since(&before);
        self.size_bound = self
            .size_bound
            .saturating_add(GROWTH_PER_BYTE.saturating_mul(bytes.len() as u64));
        let op = self.take_op();
        self.bound_text(&op);
        let mut status = self.status();
        if misplaced {
            status |= status::MISPLACED;
        }
        if overlap {
            status |= status::OVERLAP;
        }
        Ok((op, status))
    }

    /// The clients whose structs wait in yrs for elements that haven't
    /// arrived.
    pub fn pending_clients(&self) -> Vec<u64> {
        let store = self.doc.transact();
        let mut clients: Vec<u64> = store
            .store()
            .pending_clients()
            .into_iter()
            .map(|client| client.get())
            .collect();
        clients.sort_unstable();
        clients
    }

    fn status(&mut self) -> u32 {
        let txn = self.doc.transact();
        let mut status = 0;
        if txn.has_missing_updates() {
            status |= status::PENDING;
        }
        // UTF-8 takes at most three bytes per UTF-16 unit.
        let units = u64::from(self.text.len(&txn));
        self.text_bound = self.text_bound.min(units.saturating_mul(3));
        if self.text_bound > MAX_TEXT_BYTES {
            let bytes = self.text.get_string(&txn).len() as u64;
            self.text_bound = bytes;
            if bytes > MAX_TEXT_BYTES {
                status |= status::TEXT_TOO_LARGE;
            }
        }
        if self.size_bound > MAX_DOCUMENT_BYTES {
            let size = txn.encode_state_as_update_v1(&StateVector::default()).len() as u64;
            self.size_bound = size;
            if size > MAX_DOCUMENT_BYTES {
                status |= status::DOCUMENT_TOO_LARGE;
            }
        }
        status
    }

    /// Applies an edit made on this device as one transaction: of local
    /// origin when `undoable`, or of outside origin for an outside edit of
    /// the file, which undo never takes back.
    pub fn local(&mut self, op: &Op, undoable: bool, now: u64) -> Result<(), DocError> {
        NOW.with(|cell| cell.set(now));
        let len = {
            let txn = self.doc.transact();
            self.text.len(&txn)
        };
        if op.base_len() > u64::from(len) {
            return Err(DocError::Range);
        }
        {
            let origin = if undoable { LOCAL } else { OUTSIDE };
            let mut txn = self.doc.transact_mut_with(origin);
            let mut at: u32 = 0;
            for component in op.components() {
                match component {
                    Component::Retain(n) => at += n,
                    Component::Insert(text) => {
                        self.text.insert(&mut txn, at, text);
                        at += text.encode_utf16().count() as u32;
                    }
                    Component::Delete(n) => self.text.remove_range(&mut txn, at, *n),
                }
            }
        }
        // Your own edits are already in the editor.
        self.take_op();
        self.bound_text(op);
        self.size_bound = self
            .size_bound
            .saturating_add(GROWTH_PER_BYTE.saturating_mul(op.byte_len() as u64));
        Ok(())
    }

    /// Takes back your last undo step. Returns the text change it made.
    pub fn undo(&mut self, now: u64) -> Op {
        NOW.with(|cell| cell.set(now));
        self.undo.undo_blocking();
        let op = self.take_op();
        self.bound_text(&op);
        op
    }

    /// Reapplies your last undone step. Returns the text change it made.
    pub fn redo(&mut self, now: u64) -> Op {
        NOW.with(|cell| cell.set(now));
        self.undo.redo_blocking();
        let op = self.take_op();
        self.bound_text(&op);
        op
    }

    /// Whether undo and redo have a step of yours to take back or
    /// reapply.
    pub fn undo_state(&self) -> (bool, bool) {
        (self.undo.can_undo(), self.undo.can_redo())
    }

    /// Ends the current undo step, so the next local edit starts another.
    pub fn end_undo_step(&mut self) {
        self.undo.reset();
    }

    /// The yrs update holding every local change since the last call, or
    /// `None` when there was none.
    pub fn take_update(&mut self) -> Result<Option<Vec<u8>>, DocError> {
        let updates = std::mem::take(&mut self.collected.borrow_mut().local_updates);
        if updates.is_empty() {
            return Ok(None);
        }
        yrs::merge_updates_v1(&updates)
            .map(Some)
            .map_err(|_| DocError::Update)
    }

    /// The document encoded with garbage collection on, as a snapshot
    /// holds it. It leaves out what yrs holds pending:
    /// `encode_state_as_update_v1` merges that in, and the merge cuts a
    /// pending struct that repeats clocks the document holds, so the part
    /// past them names the element before it, not the origin it waits for,
    /// and shows on a boot. The records it came from stay live.
    pub fn state(&self) -> Vec<u8> {
        self.doc.transact().encode_diff_v1(&StateVector::default())
    }

    /// The text of `content`.
    pub fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    /// The length of `content` in UTF-16 units.
    pub fn len(&self) -> u32 {
        self.text.len(&self.doc.transact())
    }

    /// Whether `content` is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Encodes a relative position for the UTF-16 offset `index`, which
    /// sticks to the character after it when `after` is set.
    pub fn encode_position(&self, index: u32, after: bool) -> Result<Vec<u8>, DocError> {
        let txn = self.doc.transact();
        let assoc = if after { Assoc::After } else { Assoc::Before };
        self.text
            .sticky_index(&txn, index, assoc)
            .map(|position| position.encode_v1())
            .ok_or(DocError::Position)
    }

    /// The UTF-16 offset a relative position points at in `content` now.
    pub fn decode_position(&self, bytes: &[u8]) -> Result<u32, DocError> {
        let position = StickyIndex::decode_v1(bytes).map_err(|_| DocError::Position)?;
        let txn = self.doc.transact();
        let offset = position.get_offset(&txn).ok_or(DocError::Position)?;
        let ours: &yrs::branch::Branch = self.text.as_ref();
        if !std::ptr::eq(offset.branch.as_ref() as *const _, ours as *const _) {
            return Err(DocError::Position);
        }
        Ok(offset.index)
    }
}

/// Merges updates into one, as `merge_updates` in lib.rs passes them.
///
/// yrs merges many updates at once in time quadratic in their number, so a
/// device that typed ten thousand keystrokes between two takes would run out
/// of fuel merging them. Merging them two at a time, in rounds, gives the
/// same update in time proportional to their size times the number of
/// rounds.
pub fn merge(updates: &[&[u8]]) -> Result<Vec<u8>, DocError> {
    if updates.len() <= 2 {
        return yrs::merge_updates_v1(updates).map_err(|_| DocError::Update);
    }
    let mut round: Vec<Vec<u8>> = updates
        .chunks(2)
        .map(|pair| yrs::merge_updates_v1(pair).map_err(|_| DocError::Update))
        .collect::<Result<_, _>>()?;
    while round.len() > 1 {
        round = round
            .chunks(2)
            .map(|pair| {
                let pair: Vec<&[u8]> = pair.iter().map(Vec::as_slice).collect();
                yrs::merge_updates_v1(&pair).map_err(|_| DocError::Update)
            })
            .collect::<Result<_, _>>()?;
    }
    Ok(round.pop().unwrap_or_default())
}

fn new_doc(client: u64) -> Doc {
    let mut options = Options::with_client_id(ClientID::new(client));
    options.offset_kind = OffsetKind::Utf16;
    options.skip_gc = false;
    Doc::with_options(options)
}

/// A lineage's first document (§22): one transaction by client ID 0 that
/// inserts `text` into `content`, encoded with garbage collection on.
pub fn first_document(text: &str) -> Vec<u8> {
    let doc = new_doc(0);
    let content = doc.get_or_insert_text(ROOT);
    {
        let mut txn = doc.transact_mut();
        content.insert(&mut txn, 0, text);
    }
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

fn component(delta: &Delta) -> Component {
    match delta {
        Delta::Retain(n, _) => Component::Retain(*n),
        Delta::Deleted(n) => Component::Delete(*n),
        Delta::Inserted(Out::Any(Any::String(text)), _) => Component::Insert(text.to_string()),
        // The verdict lets only strings into `content`.
        Delta::Inserted(other, _) => panic!("content holds a non-string value: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect()
    }

    /// `op` applied to `text`, after checking that every change it makes
    /// starts and ends on a code point of `text`: a run of deletes and
    /// inserts between two retains is one change, as the host's splices
    /// are.
    fn apply_on_code_points(text: &str, op: &Op) -> String {
        let units: Vec<u16> = text.encode_utf16().collect();
        let boundary =
            |at: usize| at == 0 || at == units.len() || !(0xDC00..0xE000).contains(&units[at]);
        let mut out: Vec<u16> = Vec::new();
        let mut at = 0usize;
        let mut change: Option<usize> = None;
        for component in op.components() {
            match component {
                Component::Retain(n) => {
                    if let Some(start) = change.take() {
                        assert!(boundary(start) && boundary(at), "{op:?} on {text:?}");
                    }
                    out.extend_from_slice(&units[at..at + *n as usize]);
                    at += *n as usize;
                }
                Component::Insert(inserted) => {
                    change.get_or_insert(at);
                    out.extend(inserted.encode_utf16());
                }
                Component::Delete(n) => {
                    change.get_or_insert(at);
                    at += *n as usize;
                }
            }
        }
        if let Some(start) = change {
            assert!(boundary(start) && boundary(at), "{op:?} on {text:?}");
        }
        out.extend_from_slice(&units[at..]);
        String::from_utf16(&out).unwrap()
    }

    /// The surrogate cases of `sync-v2-collaboration.json`: card 1's first
    /// load edits "a😀b" inside the emoji's pair.
    const SURROGATES: [(&str, &str, &str); 5] = [
        (
            "insert_after_the_high_half",
            "0101818080800200840001015800",
            "a\u{FFFD}\u{FFFD}bX",
        ),
        (
            "insert_before_the_low_half",
            "0101818080800200440002015800",
            "a\u{FFFD}X\u{FFFD}b",
        ),
        (
            "insert_between_the_halves",
            "0101818080800200c400010002015800",
            "a\u{FFFD}X\u{FFFD}b",
        ),
        ("delete_from_the_low_half", "000100010202", "a\u{FFFD}"),
        ("delete_to_the_high_half", "000100010101", "a\u{FFFD}b"),
    ];

    /// The text's bound never falls below its UTF-8 length, through
    /// records, your edits, undo and redo, so a text past 8 MiB of UTF-8
    /// always stops, while a long ASCII text never costs a copy of itself.
    #[test]
    fn the_text_bound_holds_the_utf8_length() {
        let mut note = Note::new(3 << 29 | 1).unwrap();
        note.integrate(&first_document("aé中😀")).unwrap();
        let bounded = |note: &Note| note.text_bound >= note.text().len() as u64;
        assert!(bounded(&note));
        let mut edits = Vec::new();
        for (at, removed, inserted) in [(0, 0, "😀😀"), (2, 3, "中"), (0, 6, ""), (1, 0, "é")]
        {
            let mut op = Op::default();
            op.push(Component::Retain(at));
            op.push(Component::Delete(removed));
            op.push(Component::Insert(inserted.into()));
            edits.push(op);
        }
        for (step, op) in edits.iter().enumerate() {
            note.local(op, true, step as u64 * 1_000).unwrap();
            assert!(bounded(&note), "after edit {step}: {:?}", note.text());
        }
        for step in 0..4 {
            note.undo(10_000 + step * 1_000);
            assert!(bounded(&note), "after undo {step}");
        }
        for step in 0..4 {
            note.redo(20_000 + step * 1_000);
            assert!(bounded(&note), "after redo {step}");
        }

        let units = (MAX_TEXT_BYTES / 3 + 1_000) as usize;
        let mut ascii = Note::new(3 << 29 | 1).unwrap();
        let (_, status) = ascii
            .integrate(&first_document(&"a".repeat(units)))
            .unwrap();
        assert_eq!(status & status::TEXT_TOO_LARGE, 0);
        assert_eq!(ascii.text_bound, units as u64, "no copy of the text");
        let mut wide = Note::new(3 << 29 | 1).unwrap();
        let (_, status) = wide
            .integrate(&first_document(&"中".repeat(units)))
            .unwrap();
        assert_ne!(status & status::TEXT_TOO_LARGE, 0);
    }

    /// Merging in rounds of two gives the update one merge of them all
    /// gives, for typing, deletions and undo.
    #[test]
    fn a_merge_in_rounds_is_one_merge() {
        let mut note = Note::new(3 << 29 | 1).unwrap();
        note.integrate(&first_document("x")).unwrap();
        let mut updates = Vec::new();
        for i in 0..301u32 {
            let mut op = Op::default();
            if i % 7 == 6 {
                op.push(Component::Retain(i / 2));
                op.push(Component::Delete(2));
            } else {
                op.push(Component::Retain(note.len() / 2));
                op.push(Component::Insert(["a", "é", "😀"][i as usize % 3].into()));
            }
            note.local(&op, true, u64::from(i) * 1_000).unwrap();
            if i % 50 == 49 {
                note.undo(u64::from(i) * 1_000 + 1);
            }
            updates.extend(note.take_update().unwrap());
        }
        let refs: Vec<&[u8]> = updates.iter().map(Vec::as_slice).collect();
        let merged = merge(&refs).unwrap();
        assert_eq!(merged, yrs::merge_updates_v1(&refs).unwrap());
        let mut reader = Note::new(4 << 29 | 1).unwrap();
        reader.integrate(&first_document("x")).unwrap();
        reader.integrate(&merged).unwrap();
        assert_eq!(reader.text(), note.text());
    }

    #[test]
    fn a_state_holds_nothing_that_waits() {
        // Client 0's clock 0 again and one clock more, after an element of
        // a client nobody holds.
        let waits = unhex("0101000084070002616200");
        let mut note = Note::new(3 << 29 | 1).unwrap();
        note.integrate(&first_document("x")).unwrap();
        let (_, status) = note.integrate(&waits).unwrap();
        assert_ne!(status & status::PENDING, 0);
        assert_eq!(note.text(), "x");

        let mut booted = Note::new(4 << 29 | 1).unwrap();
        let (_, status) = booted.integrate(&note.state()).unwrap();
        assert_eq!(status & status::PENDING, 0);
        assert_eq!(booted.text(), "x");
    }

    #[test]
    fn a_split_surrogate_pair_reaches_the_change_as_u_fffd() {
        let base = unhex("01010000040107636f6e74656e740661f09f98806200");
        for (name, update, expected) in SURROGATES {
            let mut note = Note::new(3 << 29 | 1).unwrap();
            let (op, _) = note.integrate(&base).unwrap();
            assert_eq!(apply_on_code_points("", &op), "a😀b");
            let (op, _) = note.integrate(&unhex(update)).unwrap();
            assert_eq!(note.text(), expected, "{name}");
            assert_eq!(apply_on_code_points("a😀b", &op), expected, "{name}");

            // The next change names no unit the split replaced.
            let mut typed = Op::default();
            typed.push(Component::Retain(note.len()));
            typed.push(Component::Insert("Z".into()));
            note.local(&typed, true, 0).unwrap();
            let theirs = format!("{expected}Z");
            assert_eq!(note.text(), theirs, "{name}");
            let mut undo = note.undo(1_000);
            assert_eq!(apply_on_code_points(&theirs, &undo), expected, "{name}");
            undo = note.redo(2_000);
            assert_eq!(apply_on_code_points(expected, &undo), theirs, "{name}");
        }
    }
}
