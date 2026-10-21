//! Find and replace in one note: the matches of a query, which one is
//! current, and the edits that replace them. The find bar in `editor-egui`
//! draws this state and drives it.
//!
//! Adapted from the find logic of Lockbook's editor
//! (`tab/markdown_editor/widget/find.rs`, Unlicense); see
//! `docs/provenance/lockbook-editor.md`. Noura keeps one mode, literal
//! text ignoring case, as the legacy app's find did.

use regex::RegexBuilder;

use crate::doc::MdDoc;
use crate::input::{Event, Region};
use crate::offset_types::{Byte, Grapheme, RangeExt as _};
use crate::unicode_segs::UnicodeSegs;

/// Matches counted before the count shows as "1000+".
pub const MATCH_LIMIT: usize = 1000;

/// Every place `query` appears in `text`, as literal text ignoring case.
/// A match that starts or ends inside a grapheme cluster grows to cover it.
pub fn find_all(text: &str, segs: &UnicodeSegs, query: &str) -> Vec<(Grapheme, Grapheme)> {
    if query.is_empty() {
        return Vec::new();
    }
    let Ok(pattern) = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    else {
        return Vec::new();
    };
    let mut matches: Vec<(Grapheme, Grapheme)> = Vec::new();
    for found in pattern.find_iter(text) {
        let range = (
            segs.byte_to_char_floor(Byte(found.start())),
            segs.byte_to_char_ceil(Byte(found.end())),
        );
        // Two matches inside one cluster grow into the same range.
        if matches
            .last()
            .is_some_and(|last| last.intersects(&range, false))
        {
            continue;
        }
        matches.push(range);
    }
    matches
}

/// Find's state for one note: the query, its matches and the current one.
#[derive(Debug, Default)]
pub struct Find {
    query: String,
    matches: Vec<(Grapheme, Grapheme)>,
    /// The text version and query the matches were found for.
    found_for: Option<(u64, String)>,
    current: Option<usize>,
    /// After a replacement, the next match to make current starts here.
    resume_at: Option<Grapheme>,
}

impl Find {
    /// The text being searched for.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Search for `query` and make the first match at or after the
    /// selection current, wrapping to the top, like a browser's find.
    pub fn set_query(&mut self, doc: &MdDoc, query: &str) {
        self.query = query.to_owned();
        let anchor = doc.selection().0.min(doc.selection().1);
        self.found_for = None;
        self.refresh(doc);
        self.current = self.first_at_or_after(anchor);
    }

    /// Find the matches again if the text or the query changed since.
    pub fn refresh(&mut self, doc: &MdDoc) {
        let key = (doc.text_seq, self.query.clone());
        if self.found_for.as_ref() == Some(&key) {
            return;
        }
        self.matches = find_all(doc.text(), &doc.buffer.current.segs, &self.query);
        self.found_for = Some(key);
        self.current = match self.resume_at.take() {
            Some(at) => self.first_at_or_after(at),
            None => self
                .current
                .filter(|_| !self.matches.is_empty())
                .map(|index| index.min(self.matches.len() - 1)),
        };
    }

    fn first_at_or_after(&self, at: Grapheme) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        Some(
            self.matches
                .iter()
                .position(|found| found.0 >= at)
                .unwrap_or(0),
        )
    }

    /// Every match, in order.
    pub fn matches(&self) -> &[(Grapheme, Grapheme)] {
        &self.matches
    }

    /// The current match.
    pub fn current(&self) -> Option<(Grapheme, Grapheme)> {
        self.current
            .and_then(|index| self.matches.get(index).copied())
    }

    /// Which match is current, counting from 0.
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// Step to the next match, or the previous one if `backwards`,
    /// wrapping around the note. Returns whether there is a match.
    pub fn step(&mut self, backwards: bool) -> bool {
        let count = self.matches.len();
        if count == 0 {
            self.current = None;
            return false;
        }
        self.current = Some(match (self.current, backwards) {
            (Some(index), false) => (index + 1) % count,
            (Some(0), true) | (None, true) => count - 1,
            (Some(index), true) => index - 1,
            (None, false) => 0,
        });
        true
    }

    /// The count the find bar shows: "3/12", "3/1000+" past
    /// [`MATCH_LIMIT`], "No results" for a query with no match, and nothing
    /// for an empty query.
    pub fn count_label(&self) -> String {
        if self.matches.is_empty() {
            return if self.query.is_empty() {
                String::new()
            } else {
                "No results".to_owned()
            };
        }
        let total = if self.matches.len() > MATCH_LIMIT {
            format!("{MATCH_LIMIT}+")
        } else {
            self.matches.len().to_string()
        };
        match self.current {
            Some(index) => format!("{}/{total}", index + 1),
            None => total,
        }
    }

    /// The edit that replaces the current match with `replacement`. The
    /// match after it becomes current once the text changes.
    pub fn replace_current(&mut self, replacement: &str) -> Vec<Event> {
        let Some(range) = self.current() else {
            return Vec::new();
        };
        let added = crate::unicode_segs::calc(replacement)
            .last_cursor_position()
            .0;
        self.resume_at = Some(Grapheme(range.start().0 + added));
        vec![Event::Replace {
            region: Region::from(range),
            text: replacement.to_owned(),
            advance_cursor: false,
        }]
    }

    /// The edits that replace every match with `replacement`. Applied in
    /// one batch, they undo as one step.
    pub fn replace_all(&mut self, replacement: &str) -> Vec<Event> {
        self.resume_at = None;
        self.current = None;
        self.matches
            .iter()
            .rev()
            .map(|range| Event::Replace {
                region: Region::from(*range),
                text: replacement.to_owned(),
                advance_cursor: false,
            })
            .collect()
    }
}
