//! Input method composition. While you compose, the input method's marked
//! text lives here and nowhere else: the buffer, its change events and so
//! the file only see the text the input method commits.

use std::ops::Range;

use crate::doc::MdDoc;

/// Marked text an input method is composing at the caret.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    /// The marked text.
    pub text: String,
    /// The clause the input method is converting, in characters of
    /// `text`, when it names one.
    pub selected: Option<Range<usize>>,
}

impl MdDoc {
    /// The marked text an input method is composing, if any.
    pub fn composition(&self) -> Option<&Composition> {
        self.composition.as_ref()
    }

    /// Ends the composition without inserting its marked text, as a
    /// cancel does. For an input method that drops its composition without
    /// saying so, as when the editor loses the keyboard. The text and its
    /// change events stay as they are.
    pub fn end_composition(&mut self) {
        self.set_composition(String::new(), None);
    }

    /// Shows `text` as marked text at the caret, or ends the composition
    /// when `text` is empty. The text and its change events stay as they
    /// are.
    pub(crate) fn set_composition(&mut self, text: String, selected: Option<Range<usize>>) {
        let composition =
            (!text.is_empty() && !self.readonly).then_some(Composition { text, selected });
        if self.composition != composition {
            self.composition = composition;
            // The marked text lays out inline, like revealed syntax.
            self.bump_reveal_seq();
        }
    }
}
