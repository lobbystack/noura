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
//! Outside edits arrive the same way. [`Buffer::reload`] hands the buffer
//! the file's new contents, and the next [`Buffer::update`] diffs the last
//! text loaded or saved against them. While the version that held that text
//! is still in the history, the update queues the diff with that version as
//! the base, so the buffer transforms it through the user's unsaved edits.
//! Undo past that version followed by a new edit drops it, and merging an
//! outside edit into unsaved edits leaves the buffer with no version that
//! held the file's text. The update then merges three texts instead: the
//! last text loaded or saved, the buffer's and the file's. Either way, where
//! an outside edit overlaps a local one, the local edit wins. A replacement
//! keeps out text inserted at either end of its range, so a keystroke typed
//! right after a word the file replaced stays after the new word.
//!
//! # Change events
//!
//! Every replacement that reaches the rope, typed, undone, redone or merged
//! from the file, also lands in a list of [`TextChange`]s: the bytes it
//! replaced and the text it inserted. [`Buffer::take_changes`] drains the
//! list. Replaying the changes on the text the buffer started from gives
//! its text now. IME composition never reaches the buffer, so marked text
//! makes no change until the input method commits it.
//!
//! # Undo
//!
//! Undo and redo step over units. Operations queued together form one unit.
//! A frame that only moves the selection joins the unit before it, and
//! single-keystroke frames typed within half a second of each other join
//! into one unit.
//!
//! Undo takes back only your own edits. An outside edit is a unit of its
//! own that undo never takes back. When one arrived after your last unit,
//! undo first rewrites that stretch of the history so the outside edit
//! comes before your unit, then takes your unit back: the outside edit
//! stays, and redo brings your unit back on top of it.

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
    /// File contents passed to [`Self::reload`], merged by the next update.
    disk: Option<String>,
    /// The changes made to the rope since the last [`Self::take_changes`].
    changes: Vec<TextChange>,
}

/// One change to the buffer's text: the bytes in `range` of the text
/// before it gave way to `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    /// Byte offsets into the text as it was just before this change.
    pub range: std::ops::Range<usize>,
    pub text: String,
    /// Whether the change came from the editor: typing, a command, undo or
    /// redo. A merged outside edit of the file isn't local.
    pub local: bool,
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

        // The flat view and the grapheme index change only around the
        // replacement: clusters never cross a line ending.
        let before = self.segs.last_cursor_position();
        self.flat.replace_range(start.0..end.0, &replace.text);
        self.segs
            .replace_lines(&self.flat, start.0..end.0, replace.text.len());
        let added = Graphemes(
            (self.segs.last_cursor_position().0 + replace.range.len().0).saturating_sub(before.0),
        );

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
    /// False for an outside edit of the file.
    local: bool,
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
    /// range. They were queued together, and are all local or all outside
    /// edits.
    fn frame_at(&self, index: usize) -> (usize, usize) {
        let base = self.received[index].base;
        let local = self.received[index].local;
        let same = |received: &Received| received.base == base && received.local == local;
        let mut start = index;
        while start > 0 && same(&self.received[start - 1]) {
            start -= 1;
        }
        let mut end = index + 1;
        while end < self.received.len() && same(&self.received[end]) {
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
        // Another app's edits never share a unit with yours.
        if self.received[index].local != self.received[index - 1].local {
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
                if self.received[previous.0].local
                    && self.frame_is_keystroke(frame)
                    && self.frame_is_keystroke(previous) =>
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

/// The last text loaded from or saved to the file: the base for merging
/// outside edits.
#[derive(Default)]
struct External {
    text: String,
    /// The version that held `text`. `None` once undo went back past that
    /// version and a new edit dropped it, or when no version held `text`,
    /// as after an outside edit merged into unsaved edits.
    seq: Option<usize>,
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
            .extend(joined.into_iter().map(|op| Received {
                op,
                base,
                at,
                local: true,
            }));
    }

    /// The changes made to the text since the last call, in order.
    pub fn take_changes(&mut self) -> Vec<TextChange> {
        std::mem::take(&mut self.changes)
    }

    /// Loads new file contents. The next [`Self::update`] merges the edits
    /// made outside the editor since the last load or save with the edits
    /// made in it.
    pub fn reload(&mut self, text: String) {
        self.disk = Some(text);
    }

    /// Records that the file now holds `text`, which the buffer held at
    /// version `seq`. Later reloads merge against it. Call it before undo
    /// and a new edit can drop version `seq`.
    pub fn saved(&mut self, seq: usize, text: String) {
        self.external = External {
            text,
            seq: Some(seq),
        };
    }

    /// Applies the queued operations, then merges the file contents passed
    /// to [`Self::reload`].
    pub fn update(&mut self) -> Response {
        let mut response = self.apply_queued();
        if let Some(text) = self.disk.take()
            && text != self.external.text
        {
            self.queue_outside_edits(&text);
            if let Some(merged) = self.apply_queued() {
                match &mut response {
                    Some(response) => *response |= merged,
                    None => response = Some(merged),
                }
            }
            // An unsaved edit that survived the merge leaves the buffer with
            // no version that holds the file's text.
            let seq = (self.current.text() == text).then_some(self.current.seq);
            self.external = External { text, seq };
        }
        response.unwrap_or_default()
    }

    /// Queues the operations that turn the buffer's text into its merge with
    /// `text`, the file's new contents.
    fn queue_outside_edits(&mut self, text: &str) {
        let (base, ops) = match self.external.seq {
            // A version on the way to this one held the last text loaded or
            // saved: diff the file against that text, and let the update
            // transform the result through every edit made since.
            Some(seq) if seq <= self.current.seq => (seq, diff(&self.external.text, text)),
            // Undo took that version back, or no version held that text.
            _ => {
                let merged = merge(&self.external.text, self.current.text(), text);
                (self.current.seq, diff(self.current.text(), &merged))
            }
        };
        self.queue_replacements(ops, base, Instant::now(), false);
    }

    /// Queues replacements made against version `base`.
    fn queue_replacements(&mut self, ops: Vec<Replace>, base: usize, at: Instant, local: bool) {
        self.history
            .received
            .extend(ops.into_iter().map(|replace| Received {
                op: Operation::Replace(replace),
                base,
                at,
                local,
            }));
    }

    /// Applies the operations waiting in the queue. Returns `None` if none
    /// were waiting.
    fn apply_queued(&mut self) -> Option<Response> {
        if self.history.received.len() == self.history.applied.len() {
            return None;
        }

        // New edits drop what undo had taken back, which includes the
        // version that held the file's text if undo went back past it.
        let redo = self.current.seq..self.history.applied.len();
        if !redo.is_empty() && self.external.seq.is_some_and(|seq| seq > self.current.seq) {
            self.external.seq = None;
        }
        self.history.received.drain(redo.clone());
        self.history.applied.drain(redo);

        let mut response = Response {
            seq_before: self.current.seq,
            ..Default::default()
        };
        while self.history.applied.len() < self.history.received.len() {
            let index = self.history.applied.len();
            let received = &self.history.received[index];
            let local = received.local;
            let op = self.transform(received.op.clone(), received.base);

            let (inverse, start) = self.current.inverse_before(&op);
            let (op_response, added) = self.apply(&op, local);
            response |= op_response;

            let mut inverse = inverse;
            if let (Some(replace), Some(start)) = (&mut inverse.replace, start) {
                replace.range = (start, start + added);
            }
            self.history.applied.push(Applied { op, inverse, added });
            self.current.seq += 1;
        }
        response.seq_after = self.current.seq;
        Some(response)
    }

    fn apply(&mut self, op: &Operation, local: bool) -> (Response, Graphemes) {
        match op {
            Operation::Select(range) => (self.current.select(*range), Graphemes(0)),
            Operation::Replace(replace) => self.replace(replace, local),
        }
    }

    /// Applies a replacement to the text and records its change event.
    fn replace(&mut self, replace: &Replace, local: bool) -> (Response, Graphemes) {
        let bytes = self.current.segs.range_to_byte(replace.range);
        let range = bytes.start().0..bytes.end().0;
        let changed = !range.is_empty() || !replace.text.is_empty();
        let result = self.current.replace(replace);
        if changed {
            self.changes.push(TextChange {
                range,
                text: replace.text.clone(),
                local,
            });
        }
        result
    }

    /// Rewrites an operation made against version `base` to apply to the
    /// current version.
    fn transform(&self, mut op: Operation, base: usize) -> Operation {
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
                Operation::Select(range) => {
                    adjust_subsequent_range(earlier.range, applied.added, true, range);
                }
                // A replacement keeps out text inserted at its end, which
                // stays after it, as text inserted at its start stays
                // before it. An insertion lands after the earlier one.
                Operation::Replace(Replace { range, .. }) => {
                    let (start, end) = (range.start(), range.end());
                    let start =
                        adjust_subsequent_position(earlier.range, applied.added, true, start);
                    let end = adjust_subsequent_position(
                        earlier.range,
                        applied.added,
                        range.is_empty(),
                        end,
                    )
                    .max(start);
                    *range = if range.0 <= range.1 {
                        (start, end)
                    } else {
                        (end, start)
                    };
                }
            }
        }

        // An outside edit's offsets come from the text recorded as saved at
        // its base. If a caller recorded the wrong version, they can run
        // past the text: keep them inside it so the merge degrades instead
        // of panicking.
        let last = self.current.segs.last_cursor_position();
        match &mut op {
            Operation::Select(range) | Operation::Replace(Replace { range, .. }) => {
                range.0 = range.0.min(last);
                range.1 = range.1.min(last);
            }
        }
        op
    }

    /// Whether undo has one of your edits to take back. Another app's
    /// edits aren't yours to undo.
    pub fn can_undo(&self) -> bool {
        let done = &self.history.received[..self.current.seq];
        done.iter()
            .any(|received| received.local && matches!(received.op, Operation::Replace(_)))
            || (!done.is_empty() && done.iter().all(|received| received.local))
    }

    pub fn can_redo(&self) -> bool {
        self.current.seq < self.history.applied.len()
    }

    /// Takes back the last undo unit of your own edits. Another app's
    /// edits made since stay.
    pub fn undo(&mut self) -> Response {
        let mut response = Response {
            seq_before: self.current.seq,
            ..Default::default()
        };
        if !self.can_undo() {
            response.seq_after = self.current.seq;
            return response;
        }
        self.move_outside_edits_back();
        while self.current.seq > 0 && self.history.received[self.current.seq - 1].local {
            self.current.seq -= 1;
            let inverse = self.history.applied[self.current.seq].inverse.clone();
            if let Some(replace) = &inverse.replace {
                response |= self.replace(replace, true).0;
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
            let (op_response, added) = self.apply(&op, true);
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

    /// Before undo: when another app's edits arrived after your last edit
    /// still in effect, rewrite the history from the start of that edit's
    /// unit so their edits come first. The rewritten stretch holds the
    /// other app's edits, applied to the text before your unit, then your
    /// unit as one replacement, padded with selections so every version
    /// after it keeps its number. The text and selection on screen stay
    /// as they are, and the undo that follows takes back your unit alone.
    ///
    /// Where the other app edited text your unit wrote, its edit stays.
    fn move_outside_edits_back(&mut self) {
        let seq = self.current.seq;
        let received = &self.history.received;
        let Some(last) = (0..seq).rev().find(|&index| {
            received[index].local && matches!(received[index].op, Operation::Replace(_))
        }) else {
            return;
        };
        if received[last + 1..seq]
            .iter()
            .all(|received| received.local)
        {
            return;
        }
        let mut start = last;
        while !self.history.is_unit_boundary(start) {
            start -= 1;
        }

        // The text before the other app's edits, then before your unit.
        let mut scratch = Snapshot::new(self.current.text());
        for applied in self.history.applied[last + 1..seq].iter().rev() {
            if let Some(replace) = &applied.inverse.replace {
                scratch.replace(replace);
            }
        }
        let before_outside = scratch.text().to_owned();
        for applied in self.history.applied[start..=last].iter().rev() {
            if let Some(replace) = &applied.inverse.replace {
                scratch.replace(replace);
            }
        }
        let undone = take_back(&before_outside, self.current.text(), scratch.text());

        let before_unit = self.history.applied[start].inverse.select;
        let at = self.history.received[start].at;
        scratch.selection = before_unit;
        let mut rewritten: Vec<(Received, Applied)> = Vec::with_capacity(seq - start);
        if let Some(outside) = span(&scratch, &undone) {
            let applied = apply_to(&mut scratch, Operation::Replace(outside));
            let received = Received {
                op: applied.op.clone(),
                base: start,
                at,
                local: false,
            };
            rewritten.push((received, applied));
        }
        let base = start + rewritten.len();
        if let Some(yours) = span(&scratch, self.current.text()) {
            let applied = apply_to(&mut scratch, Operation::Replace(yours));
            let received = Received {
                op: applied.op.clone(),
                base,
                at,
                local: true,
            };
            rewritten.push((received, applied));
        }
        while start + rewritten.len() < seq {
            let applied = apply_to(&mut scratch, Operation::Select(self.current.selection));
            let received = Received {
                op: applied.op.clone(),
                base,
                at,
                local: true,
            };
            rewritten.push((received, applied));
        }
        debug_assert_eq!(scratch.text(), self.current.text());

        let (received, applied): (Vec<_>, Vec<_>) = rewritten.into_iter().unzip();
        self.history.received.splice(start..seq, received);
        self.history.applied.splice(start..seq, applied);
        // A version inside the stretch no longer holds what it did.
        if self
            .external
            .seq
            .is_some_and(|saved| saved > start && saved < seq)
        {
            self.external.seq = None;
        }
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
                seq: Some(0),
            },
            disk: None,
            changes: Vec::new(),
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

/// `outside`, another app's edit of `base`, with the edit that turns
/// `base` into `undone` applied too, as undo applies it. Where the other
/// app edited text the undo would take out or put back, its edit stays,
/// and the undo takes out only the rest.
fn take_back(base: &str, outside: &str, undone: &str) -> String {
    let theirs = diff(base, outside);
    let mut undo = Vec::new();
    for replace in diff(base, undone) {
        let mut cursor = replace.range.start();
        let mut split = false;
        for edit in theirs
            .iter()
            .filter(|edit| edit.range.intersects(&replace.range, false))
        {
            split = true;
            if edit.range.start() > cursor {
                undo.push(Replace {
                    range: (cursor, edit.range.start()),
                    text: String::new(),
                });
            }
            cursor = cursor.max(edit.range.end());
        }
        if !split {
            undo.push(replace);
        } else if cursor < replace.range.end() {
            undo.push(Replace {
                range: (cursor, replace.range.end()),
                text: String::new(),
            });
        }
    }
    let mut buffer = Buffer::from(base);
    let at = Instant::now();
    buffer.queue_replacements(theirs, 0, at, false);
    buffer.apply_queued();
    buffer.queue_replacements(undo, 0, at, true);
    buffer.apply_queued();
    buffer.current.rope().to_string()
}

/// One replacement that turns the text of `snapshot` into `to`: the
/// clusters between their common start and their common end. `None` when
/// the texts are the same.
fn span(snapshot: &Snapshot, to: &str) -> Option<Replace> {
    let from = snapshot.text();
    if from == to {
        return None;
    }
    let prefix: usize = from
        .chars()
        .zip(to.chars())
        .take_while(|(left, right)| left == right)
        .map(|(left, _)| left.len_utf8())
        .sum();
    let suffix: usize = from[prefix..]
        .chars()
        .rev()
        .zip(to[prefix..].chars().rev())
        .take_while(|(left, right)| left == right)
        .map(|(left, _)| left.len_utf8())
        .sum();
    // Widen to whole clusters; the texts match outside the change, so the
    // replacement widens with it.
    let start = snapshot.segs.byte_to_char_floor(Byte(prefix));
    let end = snapshot.segs.byte_to_char_ceil(Byte(from.len() - suffix));
    let (start_byte, end_byte) = snapshot.segs.range_to_byte((start, end));
    Some(Replace {
        range: (start, end),
        text: to[start_byte.0..to.len() - (from.len() - end_byte.0)].to_owned(),
    })
}

/// Applies `op` to `snapshot` and returns it as the history keeps it, with
/// what undoing it takes.
fn apply_to(snapshot: &mut Snapshot, op: Operation) -> Applied {
    let (mut inverse, start) = snapshot.inverse_before(&op);
    let added = match &op {
        Operation::Select(range) => {
            snapshot.select(*range);
            Graphemes(0)
        }
        Operation::Replace(replace) => snapshot.replace(replace).1,
    };
    if let (Some(replace), Some(start)) = (&mut inverse.replace, start) {
        replace.range = (start, start + added);
    }
    Applied { op, inverse, added }
}

/// Merges two edits of `base`, one that produced `local` and one that
/// produced `outside`. Where they overlap, `local`'s edit wins.
fn merge(base: &str, local: &str, outside: &str) -> String {
    let mut buffer = Buffer::from(base);
    let at = Instant::now();
    buffer.queue_replacements(diff(base, local), 0, at, true);
    buffer.apply_queued();
    buffer.queue_replacements(diff(base, outside), 0, at, false);
    buffer.apply_queued();
    buffer.current.rope().to_string()
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
