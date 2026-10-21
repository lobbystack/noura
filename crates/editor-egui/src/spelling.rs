//! Spell check. The app hands the editor a [`SpellChecker`], such as the
//! platform's; the editor asks it about the prose it lays out and draws a
//! wavy line under each word it flags.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use editor_core::offset_types::Grapheme;

use crate::MdRender;

/// Finds misspelled words.
pub trait SpellChecker: Send + Sync {
    /// The byte ranges of the misspelled words in `text`, in order.
    fn misspelled(&self, text: &str) -> Vec<Range<usize>>;
}

/// The spell checker an editor uses, and what it said about each text it
/// was asked about.
#[derive(Default)]
pub(crate) struct Spelling {
    checker: Option<Arc<dyn SpellChecker>>,
    answers: RefCell<HashMap<String, Arc<[Range<usize>]>>>,
}

/// How many texts' answers an editor keeps before it starts over.
const MAX_ANSWERS: usize = 4096;

impl Spelling {
    pub(crate) fn set_checker(&mut self, checker: Option<Arc<dyn SpellChecker>>) {
        self.checker = checker;
        self.answers.get_mut().clear();
    }

    fn misspelled(&self, text: &str) -> Option<Arc<[Range<usize>]>> {
        let checker = self.checker.as_ref()?;
        if let Some(answer) = self.answers.borrow().get(text) {
            return Some(Arc::clone(answer));
        }
        let mut found = checker.misspelled(text);
        found.retain(|range| {
            range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
        });
        let answer: Arc<[Range<usize>]> = found.into();
        let mut answers = self.answers.borrow_mut();
        if answers.len() >= MAX_ANSWERS {
            answers.clear();
        }
        answers.insert(text.to_owned(), Arc::clone(&answer));
        Some(answer)
    }
}

impl MdRender {
    /// The misspelled words in the source text `range`, as grapheme
    /// ranges, leaving out the word the caret is typing.
    pub(crate) fn misspelled_in(&self, range: (Grapheme, Grapheme)) -> Vec<(Grapheme, Grapheme)> {
        let text = &self.buffer[range];
        let Some(found) = self.spelling.misspelled(text) else {
            return Vec::new();
        };
        let segs = &self.buffer.current.segs;
        let base = segs.offset_to_byte(range.0);
        let caret = self.reveal_selection.map(|selection| selection.1);
        found
            .iter()
            .map(|bytes| {
                (
                    segs.byte_to_char_floor(base + bytes.start),
                    segs.byte_to_char_ceil(base + bytes.end),
                )
            })
            .filter(|(start, end)| start < end)
            .filter(|(start, end)| !caret.is_some_and(|caret| *start <= caret && caret <= *end))
            .collect()
    }
}
