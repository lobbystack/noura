//! The editor's text buffer: a ropey rope with grapheme offsets, a selection,
//! and an operation history for undo, redo and merging outside edits.
//!
//! Adapted from Lockbook's `lb-rs` (`libs/lb/lb-rs/src/model/text/buffer.rs`,
//! BSD-3-Clause); see `docs/provenance/lockbook-editor.md`.
//!
//! # The rope and its flat view
//!
//! The rope holds the text: every edit, undo, redo and merged outside edit
//! writes it, and owned reads such as the text to save come from it. comrak
//! parses one `&str` and the layout shapes `&str` runs, so the buffer also
//! keeps the rope's text as one string, rebuilt from the rope after each
//! change. [`Snapshot::text`] and the `Index` impls read that view.
//!
//! # Operations and their base
//!
//! Every operation names the buffer version it was made against, its base.
//! Operations queued together share a base: the version on screen when the
//! user acted. Before an operation applies, the buffer transforms it through
//! every operation applied since its base, so an insertion typed at the end
//! of a line lands after text inserted earlier in the same frame.
//!
//! Outside edits arrive the same way. [`Buffer::reload`] diffs the last text
//! loaded or saved against the new file contents and queues the result with
//! that text's version as the base, so the buffer transforms it through the
//! user's unsaved edits. Where an outside edit overlaps a local one, the
//! local edit wins.
//!
//! # Undo
//!
//! Undo and redo step over units. Operations queued together form one unit.
//! A frame that only moves the selection joins the unit before it, and
//! single-keystroke frames typed within half a second of each other join
//! into one unit.

use std::ops::Index;
use std::time::{Duration, Instant};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::diff::diff;
use crate::offset_types::{Byte, Grapheme, Graphemes, RangeExt as _};
use crate::operation_types::{InverseOperation, Operation, Replace};
use crate::unicode_segs::{self, UnicodeSegs};

/// Keystrokes closer together than this undo as one unit.
const TYPING_UNIT_GAP: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Buffer {
    /// The text on screen, its selection and its version.
    pub current: Snapshot,
    history: History,
    external: External,
}

/// A version of the buffer's text with its selection.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// The text. Edits write here and nowhere else.
    rope: Rope,
    /// The rope's text as one string, rebuilt from the rope after each
    /// change, for the parser and for `&str` slices.
    flat: String,
    pub segs: UnicodeSegs,
    /// `(anchor, caret)`.
    pub selection: (Grapheme, Grapheme),
    /// How many operations have been applied to reach this version.
    pub seq: usize,
}

impl Snapshot {
    fn new(text: &str) -> Self {
        let mut snapshot = Self {
            rope: Rope::from_str(text),
            ..Default::default()
        };
        snapshot.flatten();
        snapshot
    }

    /// The whole text, as one string.
    pub fn text(&self) -> &str {
        &self.flat
    }

    /// The whole text as a rope.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// The text's length in bytes.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// The whole text followed by `suffix`, copied from the rope.
    pub fn text_with(&self, suffix: &str) -> String {
        let mut text = String::with_capacity(self.rope.len_bytes() + suffix.len());
        for chunk in self.rope.chunks() {
            text.push_str(chunk);
        }
        text.push_str(suffix);
        text
    }

    /// The text in `range`, copied from the rope.
    pub fn slice_to_string(&self, range: (Grapheme, Grapheme)) -> String {
        let bytes = self.segs.range_to_byte(range);
        self.rope
            .byte_slice(bytes.start().0..bytes.end().0)
            .to_string()
    }

    /// Rebuilds the flat view and the grapheme index from the rope.
    fn flatten(&mut self) {
        self.flat.clear();
        self.flat.reserve(self.rope.len_bytes());
        for chunk in self.rope.chunks() {
            self.flat.push_str(chunk);
        }
        self.segs = unicode_segs::calc(&self.flat);
    }

    fn select(&mut self, range: (Grapheme, Grapheme)) -> Response {
        self.selection = range;
        Response {
            selection_user_moved: true,
            ..Default::default()
        }
    }

    /// Applies a replacement and returns how many graphemes it added once
    /// spliced in, which can differ from the replacement's own count where
    /// its ends fuse with neighboring characters.
    fn replace(&mut self, replace: &Replace) -> (Response, Graphemes) {
        // A range taken from a selection made leftward runs backward.
        let bytes = self.segs.range_to_byte(replace.range);
        let (start, end) = (bytes.start(), bytes.end());

        let start_char = self.rope.byte_to_char(start.0);
        let end_char = self.rope.byte_to_char(end.0);
        self.rope.remove(start_char..end_char);
        self.rope.insert(start_char, &replace.text);

        let before = std::mem::take(&mut self.segs);
        self.flatten();
        let added = before.measure_replace(&self.segs, replace.range);

        adjust_subsequent_range(replace.range, added, false, &mut self.selection);
        (
            Response {
                text_updated: true,
                ..Default::default()
            },
            added,
        )
    }

    /// The selection to restore and the text an operation would replace,
    /// read before it applies.
    fn inverse_before(&self, op: &Operation) -> (InverseOperation, Option<Grapheme>) {
        let mut inverse = InverseOperation {
            select: self.selection,
            replace: None,
        };
        let mut start = None;
        if let Operation::Replace(Replace { range, .. }) = op {
            inverse.replace = Some(Replace {
                range: *range,
                text: self.slice_to_string(*range),
            });
            start = Some(range.start());
        }
        (inverse, start)
    }
}

/// One operation the buffer received, with its base and arrival time.
#[derive(Clone, Debug)]
struct Received {
    op: Operation,
    base: usize,
    at: Instant,
}

/// An operation as it applied: transformed to the version it applied to,
/// what undoing it takes, and how many graphemes it added.
#[derive(Clone, Debug)]
struct Applied {
    op: Operation,
    inverse: InverseOperation,
    added: Graphemes,
}

/// Every operation received, in order. The first `applied.len()` operations
/// have been transformed; the first `current.seq` of those are in effect and
/// the rest can be redone. Operations past `applied.len()` wait in the queue.
#[derive(Default)]
struct History {
    received: Vec<Received>,
    applied: Vec<Applied>,
}

impl History {
    /// The operations sharing the base of operation `index`, as an index
    /// range. They were queued together.
    fn frame_at(&self, index: usize) -> (usize, usize) {
        let base = self.received[index].base;
        let mut start = index;
        while start > 0 && self.received[start - 1].base == base {
            start -= 1;
        }
        let mut end = index + 1;
        while end < self.received.len() && self.received[end].base == base {
            end += 1;
        }
        (start, end)
    }

    fn frame_replaces_text(&self, (start, end): (usize, usize)) -> bool {
        self.received[start..end]
            .iter()
            .any(|received| matches!(received.op, Operation::Replace(_)))
    }

    /// A single keystroke: one replacement of at most one grapheme, plus at
    /// most one selection.
    fn frame_is_keystroke(&self, (start, end): (usize, usize)) -> bool {
        let mut replaces = 0;
        let mut selects = 0;
        let mut graphemes = 0;
        for received in &self.received[start..end] {
            match &received.op {
                Operation::Replace(Replace { text, .. }) => {
                    replaces += 1;
                    graphemes += text.graphemes(true).count();
                }
                Operation::Select(_) => selects += 1,
            }
        }
        replaces == 1 && graphemes <= 1 && selects <= 1
    }

    /// Whether undo and redo stop between operation `index - 1` and
    /// operation `index`.
    fn is_unit_boundary(&self, index: usize) -> bool {
        if index == 0 || index >= self.received.len() {
            return true;
        }
        if self.received[index].base == self.received[index - 1].base {
            return false;
        }

        let frame = self.frame_at(index);
        if !self.frame_replaces_text(frame) {
            // A selection-only frame joins the unit before it.
            return false;
        }

        // Find the previous frame that replaced text, passing over
        // selection-only frames.
        let mut cursor = frame.0;
        let previous = loop {
            if cursor == 0 {
                break None;
            }
            let candidate = self.frame_at(cursor - 1);
            if self.frame_replaces_text(candidate) {
                break Some(candidate);
            }
            cursor = candidate.0;
        };

        match previous {
            Some(previous)
                if self.frame_is_keystroke(frame) && self.frame_is_keystroke(previous) =>
            {
                let gap = self.received[frame.0]
                    .at
                    .saturating_duration_since(self.received[previous.1 - 1].at);
                gap > TYPING_UNIT_GAP
            }
            _ => true,
        }
    }
}

/// The last text loaded from or saved to the file, and its version: the
/// base for merging outside edits.
#[derive(Default)]
struct External {
    text: String,
    seq: usize,
}

/// What an update, undo or redo changed.
#[derive(Debug, Default)]
pub struct Response {
    pub text_updated: bool,
    /// The selection moved because of a selection operation, undo or redo,
    /// and not only because an edit shifted it.
    pub selection_user_moved: bool,
    /// Versions before and after the call.
    pub seq_before: usize,
    pub seq_after: usize,
}

impl std::ops::BitOrAssign for Response {
    fn bitor_assign(&mut self, other: Response) {
        self.text_updated |= other.text_updated;
        self.selection_user_moved |= other.selection_user_moved;
        if self.seq_before == self.seq_after {
            self.seq_before = other.seq_before;
        }
        self.seq_after = other.seq_after;
    }
}

impl Buffer {
    /// Queues operations made against the current version. They apply, undo
    /// and redo together.
    pub fn queue(&mut self, ops: Vec<Operation>) {
        self.queue_at(ops, Instant::now());
    }

    /// Like [`Self::queue`], with the time the operations happened, which
    /// decides whether keystrokes join one undo unit.
    pub fn queue_at(&mut self, mut ops: Vec<Operation>, at: Instant) {
        let base = self.current.seq;

        // Sort by start, then join replacements that touch end to start.
        ops.sort_by_key(|op| match op {
            Operation::Select(range) | Operation::Replace(Replace { range, .. }) => range.start(),
        });
        let mut joined: Vec<Operation> = Vec::with_capacity(ops.len());
        for op in ops {
            if let (Operation::Replace(next), Some(Operation::Replace(last))) =
                (&op, joined.last_mut())
                && last.range.end() == next.range.start()
            {
                last.range.1 = next.range.1;
                last.text.push_str(&next.text);
                continue;
            }
            joined.push(op);
        }

        self.history
            .received
            .extend(joined.into_iter().map(|op| Received { op, base, at }));
    }

    /// Loads new file contents, merging the edits made outside the editor
    /// since the last load or save with the edits made inside it. Call
    /// [`Self::update`] to apply the merge.
    pub fn reload(&mut self, text: String) {
        let base = self.external.seq;
        let at = Instant::now();
        let ops = diff(&self.external.text, &text);
        self.history
            .received
            .extend(ops.into_iter().map(|replace| Received {
                op: Operation::Replace(replace),
                base,
                at,
            }));

        self.external.text = text;
        self.external.seq = self.history.received.len();
    }

    /// Records that the file now holds `text`, which the buffer held at
    /// version `seq`. Later reloads merge against it.
    pub fn saved(&mut self, seq: usize, text: String) {
        self.external.text = text;
        self.external.seq = seq;
    }

    /// Applies the queued operations.
    pub fn update(&mut self) -> Response {
        if self.history.received.len() == self.history.applied.len() {
            return Response::default();
        }

        // New edits drop what undo had taken back.
        let redo = self.current.seq..self.history.applied.len();
        self.history.received.drain(redo.clone());
        self.history.applied.drain(redo);

        let mut response = Response {
            seq_before: self.current.seq,
            ..Default::default()
        };
        while self.history.applied.len() < self.history.received.len() {
            let index = self.history.applied.len();
            let received = &self.history.received[index];
            let op = self.transform(received.op.clone(), received.base);

            let (inverse, start) = self.current.inverse_before(&op);
            let (op_response, added) = self.apply(&op);
            response |= op_response;

            let mut inverse = inverse;
            if let (Some(replace), Some(start)) = (&mut inverse.replace, start) {
                replace.range = (start, start + added);
            }
            self.history.applied.push(Applied { op, inverse, added });
            self.current.seq += 1;
        }
        response.seq_after = self.current.seq;
        response
    }

    fn apply(&mut self, op: &Operation) -> (Response, Graphemes) {
        match op {
            Operation::Select(range) => (self.current.select(*range), Graphemes(0)),
            Operation::Replace(replace) => self.current.replace(replace),
        }
    }

    /// Rewrites an operation made against version `base` to apply to the
    /// current version.
    fn transform(&self, mut op: Operation, base: usize) -> Operation {
        // An outside edit's base can name a version that undo took back.
        let base = base.min(self.history.applied.len());
        for applied in &self.history.applied[base..] {
            let Operation::Replace(earlier) = &applied.op else {
                continue;
            };

            if let Operation::Replace(Replace { range, text }) = &mut op
                && earlier.range.intersects(range, false)
            {
                // Overlapping edits: the earlier one wins.
                text.clear();
                range.1 = range.0;
            }

            match &mut op {
                Operation::Select(range) | Operation::Replace(Replace { range, .. }) => {
                    adjust_subsequent_range(earlier.range, applied.added, true, range);
                }
            }
        }

        // An outside edit is made against the last text loaded or saved,
        // which the buffer never held if a local edit won an overlap with
        // the outside edit before it. Its offsets can then run past the
        // text: keep them inside it so the merge degrades instead of
        // panicking.
        let last = self.current.segs.last_cursor_position();
        match &mut op {
            Operation::Select(range) | Operation::Replace(Replace { range, .. }) => {
                range.0 = range.0.min(last);
                range.1 = range.1.min(last);
            }
        }
        op
    }

    pub fn can_undo(&self) -> bool {
        self.current.seq > 0
    }

    pub fn can_redo(&self) -> bool {
        self.current.seq < self.history.applied.len()
    }

    /// Takes back the last undo unit.
    pub fn undo(&mut self) -> Response {
        let mut response = Response {
            seq_before: self.current.seq,
            ..Default::default()
        };
        while self.can_undo() {
            self.current.seq -= 1;
            let inverse = self.history.applied[self.current.seq].inverse.clone();
            if let Some(replace) = &inverse.replace {
                response |= self.current.replace(replace).0;
            }
            response |= self.current.select(inverse.select);

            if self.history.is_unit_boundary(self.current.seq) {
                break;
            }
        }
        response.seq_after = self.current.seq;
        response
    }

    /// Reapplies the next undone unit.
    pub fn redo(&mut self) -> Response {
        let mut response = Response {
            seq_before: self.current.seq,
            ..Default::default()
        };
        while self.can_redo() {
            let index = self.current.seq;
            let op = self.history.applied[index].op.clone();
            let (op_response, added) = self.apply(&op);
            response |= op_response;
            self.history.applied[index].added = added;
            self.current.seq += 1;

            if self.history.is_unit_boundary(self.current.seq) {
                break;
            }
        }
        response.seq_after = self.current.seq;
        response
    }

    /// Moves `range`, which referred to version `since_seq`, through the
    /// replacements applied since. Returns false if a replacement touched
    /// the range's text, which leaves the range meaningless.
    pub fn transform_range(&self, since_seq: usize, range: &mut (Grapheme, Grapheme)) -> bool {
        let applied = &self.history.applied[since_seq.min(self.current.seq)..self.current.seq];
        for applied in applied {
            let Operation::Replace(replace) = &applied.op else {
                continue;
            };
            if range.intersects(&replace.range, true)
                && !(range.is_empty() && replace.range.is_empty())
            {
                return false;
            }
            adjust_subsequent_range(replace.range, applied.added, false, range);
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.current.len_bytes() == 0
    }

    pub fn selection_text(&self) -> String {
        self.current.slice_to_string(self.current.selection)
    }
}

impl From<&str> for Buffer {
    fn from(text: &str) -> Self {
        Self {
            current: Snapshot::new(text),
            history: Default::default(),
            external: External {
                text: text.into(),
                seq: 0,
            },
        }
    }
}

/// Moves a range to account for a replacement of `replaced` by text that
/// added `added` graphemes. Positions before the replacement stay. Positions
/// after it shift by the change in length. Positions at or inside it move to
/// the end of the new text if `prefer_advance`, or to its start otherwise.
pub fn adjust_subsequent_range(
    replaced: (Grapheme, Grapheme),
    added: Graphemes,
    prefer_advance: bool,
    range: &mut (Grapheme, Grapheme),
) {
    for position in [&mut range.0, &mut range.1] {
        *position = adjust_subsequent_position(replaced, added, prefer_advance, *position);
    }
}

fn adjust_subsequent_position(
    replaced: (Grapheme, Grapheme),
    added: Graphemes,
    prefer_advance: bool,
    position: Grapheme,
) -> Grapheme {
    let (start, end) = (replaced.start(), replaced.end());
    if position < start {
        position
    } else if position > end {
        position + added - (end - start)
    } else if prefer_advance {
        start + added
    } else {
        start
    }
}

impl Index<(Byte, Byte)> for Snapshot {
    type Output = str;

    fn index(&self, range: (Byte, Byte)) -> &str {
        &self.flat[range.start().0..range.end().0]
    }
}

impl Index<(Grapheme, Grapheme)> for Snapshot {
    type Output = str;

    fn index(&self, range: (Grapheme, Grapheme)) -> &str {
        let range = self.segs.range_to_byte(range);
        &self[range]
    }
}

impl Index<(Byte, Byte)> for Buffer {
    type Output = str;

    fn index(&self, range: (Byte, Byte)) -> &str {
        &self.current[range]
    }
}

impl Index<(Grapheme, Grapheme)> for Buffer {
    type Output = str;

    fn index(&self, range: (Grapheme, Grapheme)) -> &str {
        &self.current[range]
    }
}
