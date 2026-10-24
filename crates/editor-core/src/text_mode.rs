//! Text mode (sync-v2.md §22): a note whose body lives in a yrs document in
//! the collaboration sandbox, which owns that document, its client IDs and
//! its undo.
//!
//! The editor holds no yrs document. It sends each edit it applies to the
//! document as a [`Utf16Splice`], converted from the buffer's byte offsets,
//! which fall on code-point boundaries, to the UTF-16 offsets of `content`.
//! Marked text an input method is composing never reaches the buffer, so it
//! reaches the document only once committed. The editor applies the changes
//! the document sends back, from collaborators, undo and redo, as edits it
//! didn't make, and never sends them back as its own. Undo and redo go to the
//! document, whose undo tracks only this device's edits, so undo never takes
//! back a collaborator's text.
//!
//! A text-mode note's file follows the document's merged changes, which the
//! sandbox hands to the sync client, not [`MdDoc::take_changes`]: those
//! carry your edits before they merge, while a slow record holds the
//! document and the file at their last merged state.

use ropey::Rope;

use crate::buffer::{self, TextChange};
use crate::doc::MdDoc;

/// A change to a text in UTF-16 units: `removed` units at `start` give way
/// to `inserted`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utf16Splice {
    /// Where the change starts.
    pub start: usize,
    /// How many units it removes.
    pub removed: usize,
    /// The text it inserts.
    pub inserted: String,
}

/// The collaboration sandbox's side of a text-mode note, as the editor sees
/// it.
pub trait CollabDocument: Send {
    /// Edits made on this device, in order, each against the text the one
    /// before it left. `undoable` is false for an outside edit of the file,
    /// which undo never takes back.
    fn edit(&mut self, splices: Vec<Utf16Splice>, undoable: bool);

    /// Asks the document to take back your last undo step. Its change comes
    /// back through [`Self::poll`].
    fn undo(&mut self);

    /// Asks the document to reapply your last undone step.
    fn redo(&mut self);

    /// Whether undo has a step of yours to take back, as the document last
    /// reported it.
    fn can_undo(&self) -> bool;

    /// Whether redo has a step of yours to reapply, as the document last
    /// reported it.
    fn can_redo(&self) -> bool;

    /// The changes the document made since the last call: collaborators'
    /// edits, undo and redo, in order, each against the editor's text once
    /// the ones before it apply.
    fn poll(&mut self) -> Vec<Utf16Splice>;
}

pub(crate) struct TextMode {
    document: Box<dyn CollabDocument>,
    /// The text as the document last heard of it, to convert byte offsets
    /// to UTF-16 offsets.
    mirror: Rope,
}

impl TextMode {
    /// `change` in UTF-16 units of the mirror, which it then applies to.
    fn splice(&mut self, change: &TextChange) -> Utf16Splice {
        let start_char = self.mirror.byte_to_char(change.range.start);
        let end_char = self.mirror.byte_to_char(change.range.end);
        let start = self.mirror.char_to_utf16_cu(start_char);
        let end = self.mirror.char_to_utf16_cu(end_char);
        self.mirror.remove(start_char..end_char);
        self.mirror.insert(start_char, &change.text);
        Utf16Splice {
            start,
            removed: end - start,
            inserted: change.text.clone(),
        }
    }
}

impl MdDoc {
    /// Puts the note in text mode: from now on its edits go to `document`,
    /// and undo and redo too.
    pub fn enter_text_mode(&mut self, document: Box<dyn CollabDocument>) {
        // Changes made before text mode aren't the document's.
        let earlier = self.buffer.take_changes();
        self.changes.extend(earlier);
        self.text_mode = Some(TextMode {
            document,
            mirror: self.buffer.current.rope().clone(),
        });
    }

    /// Takes the note out of text mode, handing back its document.
    pub fn leave_text_mode(&mut self) -> Option<Box<dyn CollabDocument>> {
        self.send_text_mode_edits();
        self.text_mode.take().map(|mode| mode.document)
    }

    /// Whether the note is in text mode.
    pub fn in_text_mode(&self) -> bool {
        self.text_mode.is_some()
    }

    /// Whether undo has something to take back: in text mode, one of your
    /// steps in the document, whose undo the buffer's history doesn't
    /// hold.
    pub fn can_undo(&self) -> bool {
        match &self.text_mode {
            Some(mode) => mode.document.can_undo(),
            None => self.buffer.can_undo(),
        }
    }

    /// Whether redo has something to reapply: in text mode, one of your
    /// undone steps in the document.
    pub fn can_redo(&self) -> bool {
        match &self.text_mode {
            Some(mode) => mode.document.can_redo(),
            None => self.buffer.can_redo(),
        }
    }

    /// Sends the edits the buffer applied since the last call to the
    /// document: yours as undoable, outside edits of the file as not.
    pub(crate) fn send_text_mode_edits(&mut self) {
        let Some(mode) = &mut self.text_mode else {
            return;
        };
        let changes = self.buffer.take_changes();
        let mut batch = Vec::new();
        let mut undoable = true;
        for change in &changes {
            if change.local != undoable && !batch.is_empty() {
                mode.document.edit(std::mem::take(&mut batch), undoable);
            }
            undoable = change.local;
            batch.push(mode.splice(change));
        }
        if !batch.is_empty() {
            mode.document.edit(batch, undoable);
        }
        self.changes.extend(changes);
    }

    /// In text mode, sends undo or redo to the document after the edits
    /// before it. Returns false outside text mode.
    pub(crate) fn text_mode_undo(&mut self, redo: bool) -> bool {
        if self.text_mode.is_none() {
            return false;
        }
        self.send_text_mode_edits();
        if let Some(mode) = &mut self.text_mode {
            if redo {
                mode.document.redo();
            } else {
                mode.document.undo();
            }
        }
        true
    }

    /// Applies the changes the document made since the last call:
    /// collaborators' edits, undo and redo. Call it whenever the document
    /// may have news, such as each frame.
    pub fn sync_text_mode(&mut self) -> buffer::Response {
        self.send_text_mode_edits();
        let Some(mode) = &mut self.text_mode else {
            return buffer::Response::default();
        };
        let splices = mode.document.poll();
        let mut response = buffer::Response {
            seq_before: self.buffer.current.seq,
            seq_after: self.buffer.current.seq,
            ..Default::default()
        };
        for splice in splices {
            let rope = self.buffer.current.rope();
            let units = rope.len_utf16_cu();
            let start = splice.start.min(units);
            let end = (start + splice.removed).min(units);
            let start_char = rope.utf16_cu_to_char(start);
            let end_char = rope.utf16_cu_to_char(end);
            let bytes = rope.char_to_byte(start_char)..rope.char_to_byte(end_char);
            response |= self.buffer.apply_external(bytes, &splice.inserted);
            if let Some(mode) = &mut self.text_mode {
                mode.mirror.remove(start_char..end_char);
                mode.mirror.insert(start_char, &splice.inserted);
            }
        }
        // The document already holds these changes.
        let applied = self.buffer.take_changes();
        self.changes.extend(applied);
        if response.text_updated {
            self.bump_text_seq();
        }
        response
    }
}
