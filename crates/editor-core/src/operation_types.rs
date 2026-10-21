//! The edits a [`Buffer`](crate::buffer::Buffer) applies.
//!
//! Adapted from Lockbook's `lb-rs` (`libs/lb/lb-rs/src/model/text/operation_types.rs`,
//! BSD-3-Clause); see `docs/provenance/lockbook-editor.md`.

use crate::offset_types::Grapheme;

/// One buffer edit. A batch of operations queued together is applied, undone
/// and redone as one unit.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Moves the selection. The first offset is the anchor and the second
    /// the caret.
    Select((Grapheme, Grapheme)),
    /// Replaces a range with new text.
    Replace(Replace),
}

/// Replaces `range` with `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Replace {
    pub range: (Grapheme, Grapheme),
    pub text: String,
}

/// What undoing one applied operation takes: the selection to restore, and
/// the replacement that puts back the text it replaced.
#[derive(Clone, Debug)]
pub struct InverseOperation {
    pub select: (Grapheme, Grapheme),
    pub replace: Option<Replace>,
}
