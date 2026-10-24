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
        })
    }

    fn take_op(&self) -> Op {
        std::mem::take(&mut self.collected.borrow_mut().op)
    }

    /// Merges a record's yrs bytes, which passed the verdict, as a remote
    /// transaction. Returns the text change and the status bits.
    pub fn integrate(&mut self, bytes: &[u8]) -> Result<(Op, u32), DocError> {
        let update = Update::decode_v1(bytes).map_err(|_| DocError::Update)?;
        {
            let mut txn = self.doc.transact_mut_with(REMOTE);
            txn.apply_update(update).map_err(|_| DocError::Update)?;
        }
        self.size_bound = self
            .size_bound
            .saturating_add(GROWTH_PER_BYTE.saturating_mul(bytes.len() as u64));
        let op = self.take_op();
        Ok((op, self.status()))
    }

    fn status(&mut self) -> u32 {
        let txn = self.doc.transact();
        let mut status = 0;
        if txn.has_missing_updates() {
            status |= status::PENDING;
        }
        // UTF-8 takes at most three bytes per UTF-16 unit.
        let units = u64::from(self.text.len(&txn));
        if units.saturating_mul(3) > MAX_TEXT_BYTES
            && self.text.get_string(&txn).len() as u64 > MAX_TEXT_BYTES
        {
            status |= status::TEXT_TOO_LARGE;
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
        self.size_bound = self
            .size_bound
            .saturating_add(GROWTH_PER_BYTE.saturating_mul(op.byte_len() as u64));
        Ok(())
    }

    /// Takes back your last undo step. Returns the text change it made.
    pub fn undo(&mut self, now: u64) -> Op {
        NOW.with(|cell| cell.set(now));
        self.undo.undo_blocking();
        self.take_op()
    }

    /// Reapplies your last undone step. Returns the text change it made.
    pub fn redo(&mut self, now: u64) -> Op {
        NOW.with(|cell| cell.set(now));
        self.undo.redo_blocking();
        self.take_op()
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
    /// holds it.
    pub fn state(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
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
pub fn merge(updates: &[&[u8]]) -> Result<Vec<u8>, DocError> {
    yrs::merge_updates_v1(updates).map_err(|_| DocError::Update)
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
