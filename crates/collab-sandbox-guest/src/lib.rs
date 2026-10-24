//! The collaboration sandbox's guest module (sync-v2.md §22): the one
//! WebAssembly module that judges every remote `text_snapshot` and
//! `text_update`, and that holds each open text-mode note's yrs document.
//!
//! It builds from `yrs` 0.26.0 without `small-client`, patched for split
//! surrogate pairs as `docs/provenance/yrs.md` records, for
//! `wasm32-unknown-unknown` with `build.sh`. The host,
//! `crates/collab-sandbox`, runs it in Wasmtime with fuel and memory limits
//! and no imports, and `sync-v2-collaboration.json` pins the BLAKE3 hash of
//! the module it runs.
//!
//! # Calls
//!
//! The host writes inputs into memory from [`alloc`], and the call that
//! takes them frees them. A call that returns bytes leaves them for
//! [`out_ptr`] and [`out_len`] until the next call. Integers wider than 32
//! bits travel as a high and a low half.
//!
//! A panic aborts, which traps, and the host treats the trap as the call's
//! failure.

pub mod document;
pub mod json;
pub mod lib0;
pub mod op;
pub mod verdict;

use std::cell::RefCell;

use document::{DocError, Note};
use op::Op;

/// Why a record is unreadable. The host reads these codes, and
/// `sync-v2-collaboration.json` names each one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Unreadable {
    /// A varint isn't the shortest encoding of its value.
    VarintNotShortest = 1,
    /// A varint's value doesn't fit its field.
    VarintRange = 2,
    /// A client's first clock plus the lengths of its structs passes
    /// 2^32 − 1.
    ClockOverflow = 3,
    /// A delete range's clock plus its length passes 2^32 − 1.
    DeleteRangeOverflow = 4,
    /// The struct section or the delete set names one client ID twice.
    DuplicateClient = 5,
    /// A struct or delete range has length 0.
    ZeroLength = 6,
    /// A string isn't valid UTF-8.
    Utf8 = 7,
    /// The yrs bytes fail to parse.
    YrsParse = 8,
    /// The yrs bytes hold a GC struct.
    GcStruct = 9,
    /// An item's content is neither a string nor a deletion.
    Content = 10,
    /// An item names a parent other than the root `content`.
    Parent = 11,
    /// An item carries a map key.
    MapKey = 12,
    /// An update's `load` lies outside 1 to 2^29 − 1.
    Load = 13,
    /// An update holds a struct under a client ID other than its signer's.
    ForeignClient = 14,
    /// A snapshot holds a struct under a client ID no indexed card has.
    SnapshotClient = 15,
    /// A `meta` key is neither `path` nor `p:` and a valid property name.
    MetaKey = 16,
    /// An update's `meta` entry names a sequence other than 0.
    MetaSeq = 17,
    /// A `p:` value isn't RFC 8785 canonical JSON.
    MetaValue = 18,
    /// `path` lacks a value, breaks the path rules or is missing from a
    /// snapshot.
    MetaPath = 19,
    /// A `p:` key appears on a note whose `object_id` is empty.
    MetaUnmanaged = 20,
    /// `p:id` isn't the JSON string of `object_id`, or is missing from a
    /// managed note's snapshot.
    MetaId = 21,
    /// `p:type` lacks a value, or is missing from a managed note's snapshot.
    MetaType = 22,
    /// The payload doesn't decode, or isn't a text snapshot or update.
    Payload = 23,
    /// The content form and the blob passed with it disagree.
    Blob = 24,
    /// An item names as its origin or right origin an element of its own
    /// client at or after its own clock.
    ForwardOrigin = 25,
}

thread_local! {
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static NOTE: RefCell<Option<Note>> = const { RefCell::new(None) };
}

/// Returns `bytes` from the call, for [`out_ptr`] and [`out_len`].
fn put(bytes: Vec<u8>) {
    OUT.with(|out| *out.borrow_mut() = bytes);
}

/// Takes ownership of an input the host wrote at `ptr`.
fn take(ptr: u32, len: u32) -> Box<[u8]> {
    // SAFETY: the host passes back a pointer and length that `alloc` handed
    // out, once.
    unsafe {
        Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            ptr as usize as *mut u8,
            len as usize,
        ))
    }
}

fn wide(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn with_note<R>(work: impl FnOnce(&mut Note) -> R) -> Option<R> {
    NOTE.with(|note| note.borrow_mut().as_mut().map(work))
}

fn doc_code(error: DocError) -> u32 {
    match error {
        DocError::Update => 1,
        DocError::Range => 2,
        DocError::Position => 3,
        DocError::Client => 4,
    }
}

/// Code for a document call made before `doc_open`.
const NO_DOCUMENT: u32 = 5;

/// Allocates `len` bytes for the host to write an input into.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: u32) -> u32 {
    let buffer = vec![0u8; len as usize].into_boxed_slice();
    Box::into_raw(buffer) as *mut u8 as usize as u32
}

/// Where the last call's output starts.
#[unsafe(no_mangle)]
pub extern "C" fn out_ptr() -> u32 {
    OUT.with(|out| out.borrow().as_ptr() as usize as u32)
}

/// How long the last call's output is.
#[unsafe(no_mangle)]
pub extern "C" fn out_len() -> u32 {
    OUT.with(|out| out.borrow().len() as u32)
}

/// Judges a record: 0 when it's readable, or an [`Unreadable`] code.
#[unsafe(no_mangle)]
pub extern "C" fn verdict(
    payload_ptr: u32,
    payload_len: u32,
    blob_ptr: u32,
    blob_len: u32,
    has_blob: u32,
    card_index: u32,
    card_count: u32,
) -> u32 {
    let payload = take(payload_ptr, payload_len);
    let blob = (has_blob != 0).then(|| take(blob_ptr, blob_len));
    let record = verdict::Record {
        payload: &payload,
        blob: blob.as_deref(),
        card_index,
        card_count,
    };
    match verdict::judge(&record) {
        Ok(()) => 0,
        Err(reason) => reason as u32,
    }
}

/// Opens this instance's document, which inserts under the client ID
/// `high:low`.
#[unsafe(no_mangle)]
pub extern "C" fn doc_open(client_high: u32, client_low: u32) -> u32 {
    match Note::new(wide(client_high, client_low)) {
        Ok(note) => {
            NOTE.with(|slot| *slot.borrow_mut() = Some(note));
            0
        }
        Err(error) => doc_code(error),
    }
}

/// Merges a record's yrs bytes. On success the output holds a status word
/// (little-endian `u32`), then the text change.
#[unsafe(no_mangle)]
pub extern "C" fn doc_integrate(ptr: u32, len: u32) -> u32 {
    let bytes = take(ptr, len);
    match with_note(|note| note.integrate(&bytes)) {
        None => NO_DOCUMENT,
        Some(Err(error)) => doc_code(error),
        Some(Ok((op, status))) => {
            let mut out = status.to_le_bytes().to_vec();
            out.extend_from_slice(&op.encode());
            put(out);
            0
        }
    }
}

/// Applies an edit made on this device, an encoded [`Op`], at `now`
/// milliseconds. Undo takes it back only when `undoable` is set.
#[unsafe(no_mangle)]
pub extern "C" fn doc_local(ptr: u32, len: u32, undoable: u32, now_high: u32, now_low: u32) -> u32 {
    let bytes = take(ptr, len);
    let Some(op) = Op::decode(&bytes) else {
        return doc_code(DocError::Range);
    };
    match with_note(|note| note.local(&op, undoable != 0, wide(now_high, now_low))) {
        None => NO_DOCUMENT,
        Some(Err(error)) => doc_code(error),
        Some(Ok(())) => 0,
    }
}

/// Takes back your last undo step; the output holds the text change.
#[unsafe(no_mangle)]
pub extern "C" fn doc_undo(now_high: u32, now_low: u32) -> u32 {
    match with_note(|note| note.undo(wide(now_high, now_low))) {
        None => NO_DOCUMENT,
        Some(op) => {
            put(op.encode());
            0
        }
    }
}

/// Reapplies your last undone step; the output holds the text change.
#[unsafe(no_mangle)]
pub extern "C" fn doc_redo(now_high: u32, now_low: u32) -> u32 {
    match with_note(|note| note.redo(wide(now_high, now_low))) {
        None => NO_DOCUMENT,
        Some(op) => {
            put(op.encode());
            0
        }
    }
}

/// Ends the current undo step.
#[unsafe(no_mangle)]
pub extern "C" fn doc_end_undo_step() -> u32 {
    with_note(Note::end_undo_step).map_or(NO_DOCUMENT, |()| 0)
}

/// Puts the update holding your changes since the last call in the output,
/// empty when there were none.
#[unsafe(no_mangle)]
pub extern "C" fn doc_take_update() -> u32 {
    match with_note(Note::take_update) {
        None => NO_DOCUMENT,
        Some(Err(error)) => doc_code(error),
        Some(Ok(update)) => {
            put(update.unwrap_or_default());
            0
        }
    }
}

/// The length of `content` in UTF-16 units, or `u32::MAX` before
/// `doc_open`.
#[unsafe(no_mangle)]
pub extern "C" fn doc_len() -> u32 {
    with_note(|note| note.len()).unwrap_or(u32::MAX)
}

/// Merges updates into one and puts it in the output. The input holds each
/// update as a little-endian `u32` length and that many bytes.
#[unsafe(no_mangle)]
pub extern "C" fn merge_updates(ptr: u32, len: u32) -> u32 {
    let bytes = take(ptr, len);
    let mut updates = Vec::new();
    let mut rest: &[u8] = &bytes;
    while let Some((word, tail)) = rest.split_first_chunk::<4>() {
        let Some((update, tail)) = tail.split_at_checked(u32::from_le_bytes(*word) as usize) else {
            return doc_code(DocError::Update);
        };
        updates.push(update);
        rest = tail;
    }
    if !rest.is_empty() {
        return doc_code(DocError::Update);
    }
    match document::merge(&updates) {
        Ok(update) => {
            put(update);
            0
        }
        Err(error) => doc_code(error),
    }
}

/// Puts the encoded document in the output.
#[unsafe(no_mangle)]
pub extern "C" fn doc_state() -> u32 {
    match with_note(|note| note.state()) {
        None => NO_DOCUMENT,
        Some(state) => {
            put(state);
            0
        }
    }
}

/// Puts the text of `content` in the output.
#[unsafe(no_mangle)]
pub extern "C" fn doc_text() -> u32 {
    match with_note(|note| note.text()) {
        None => NO_DOCUMENT,
        Some(text) => {
            put(text.into_bytes());
            0
        }
    }
}

/// Puts the relative position of the UTF-16 offset `index` in the output.
#[unsafe(no_mangle)]
pub extern "C" fn doc_encode_position(index: u32, after: u32) -> u32 {
    match with_note(|note| note.encode_position(index, after != 0)) {
        None => NO_DOCUMENT,
        Some(Err(error)) => doc_code(error),
        Some(Ok(bytes)) => {
            put(bytes);
            0
        }
    }
}

/// Puts the UTF-16 offset a relative position points at in the output, as a
/// little-endian `u32`.
#[unsafe(no_mangle)]
pub extern "C" fn doc_decode_position(ptr: u32, len: u32) -> u32 {
    let bytes = take(ptr, len);
    match with_note(|note| note.decode_position(&bytes)) {
        None => NO_DOCUMENT,
        Some(Err(error)) => doc_code(error),
        Some(Ok(index)) => {
            put(index.to_le_bytes().to_vec());
            0
        }
    }
}

/// Puts a lineage's first document holding the UTF-8 text at `ptr` in the
/// output.
#[unsafe(no_mangle)]
pub extern "C" fn first_document(ptr: u32, len: u32) -> u32 {
    let bytes = take(ptr, len);
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return doc_code(DocError::Update);
    };
    put(document::first_document(text));
    0
}
