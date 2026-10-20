//! Word-level diff between two texts, used to merge outside edits.
//!
//! A byte-level merge of two valid UTF-8 texts can produce invalid UTF-8, and
//! a character-level diff of two words that share letters interleaves them.
//! Diffing words keeps each side's words whole.
//!
//! Adapted from Lockbook's `lb-rs` (`libs/lb/lb-rs/src/model/text/mod.rs`,
//! BSD-3-Clause); see `docs/provenance/lockbook-editor.md`.

use similar::{Algorithm, DiffOp, TextDiff};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::offset_types::{Byte, Grapheme};
use crate::operation_types::Replace;
use crate::unicode_segs::{self, UnicodeSegs};

/// Replacements that turn `from` into `to`, with ranges in `from`'s grapheme
/// offsets, sorted and not overlapping.
pub fn diff(from: &str, to: &str) -> Vec<Replace> {
    let from_side = Side::new(from);
    let to_side = Side::new(to);

    let word_diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_unicode_words(from, to);

    let mut result: Vec<Replace> = Vec::new();
    for op in word_diff.ops() {
        let (old_words, new_words) = match *op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index..old_index + old_len, new_index..new_index),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };

        let range = from_side.graphemes(old_words);
        let new_range = to_side.graphemes(new_words);
        let new_bytes = to_side.segs.range_to_byte(new_range);
        let text = &to[new_bytes.0.0..new_bytes.1.0];

        match result.last_mut() {
            Some(last) if last.range.1 == range.0 => {
                last.range.1 = range.1;
                last.text.push_str(text);
            }
            _ => result.push(Replace {
                range,
                text: text.into(),
            }),
        }
    }
    result
}

/// One side of a diff: its grapheme segmentation and the byte offset where
/// each word starts, followed by the text's length.
struct Side {
    segs: UnicodeSegs,
    word_starts: Vec<Byte>,
}

impl Side {
    fn new(text: &str) -> Self {
        let mut word_starts: Vec<Byte> = text
            .split_word_bound_indices()
            .map(|(offset, _)| Byte(offset))
            .collect();
        word_starts.push(Byte(text.len()));
        Self {
            segs: unicode_segs::calc(text),
            word_starts,
        }
    }

    /// The grapheme range covering a run of words. Word boundaries can fall
    /// inside a grapheme cluster, so both ends snap to the cluster start.
    fn graphemes(&self, words: std::ops::Range<usize>) -> (Grapheme, Grapheme) {
        (
            self.segs.byte_to_char_floor(self.word_starts[words.start]),
            self.segs.byte_to_char_floor(self.word_starts[words.end]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(from: &str, replaces: &[Replace]) -> String {
        let segs = unicode_segs::calc(from);
        let mut result = from.to_string();
        for replace in replaces.iter().rev() {
            let bytes = segs.range_to_byte(replace.range);
            result.replace_range(bytes.0.0..bytes.1.0, &replace.text);
        }
        result
    }

    #[test]
    fn replaces_the_changed_word() {
        let replaces = diff("Hello, world!", "Hello, Rust!");
        assert_eq!(
            replaces,
            vec![Replace {
                range: (Grapheme(7), Grapheme(12)),
                text: "Rust".into()
            }]
        );
    }

    #[test]
    fn replaces_everything_when_nothing_matches() {
        let replaces = diff("Hello", "Goodbye");
        assert_eq!(
            replaces,
            vec![Replace {
                range: (Grapheme(0), Grapheme(5)),
                text: "Goodbye".into()
            }]
        );
    }

    #[test]
    fn applying_the_diff_produces_the_target() {
        let cases = [
            ("", "new text"),
            ("old text", ""),
            ("a b c", "a x c d"),
            ("line one\nline two\n", "line one\ninserted\nline two\n"),
        ];
        for (from, to) in cases {
            assert_eq!(
                apply(from, &diff(from, to)),
                to,
                "diff from {from:?} to {to:?}"
            );
        }
    }

    /// Ported from `lb-rs`'s `diff_fuzz`, which only checked that the diff
    /// doesn't panic. This also applies each diff.
    #[test]
    fn random_texts_diff_and_apply_to_the_target() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        let mut random_text = |next: &mut dyn FnMut(u64) -> u64| -> String {
            let length = next(10);
            (0..length)
                .map(|_| char::from_u32(0x20 + next(0xd7ff - 0x20) as u32).unwrap_or('?'))
                .collect()
        };
        for _ in 0..1000 {
            let from = random_text(&mut next);
            let to = random_text(&mut next);
            assert_eq!(
                apply(&from, &diff(&from, &to)),
                to,
                "diff from {from:?} to {to:?}"
            );
        }
    }

    #[test]
    fn a_word_boundary_inside_a_cluster_still_yields_valid_ranges() {
        // Word segmentation skips the format character and pairs the regional
        // indicators across it, which puts a word boundary inside the flag.
        let from = "\u{1f1f8}";
        let to = "\u{1f1f8}\u{feff}\u{1f1fa}\u{1f1f8}";
        let last = unicode_segs::calc(from).last_cursor_position();
        for replace in diff(from, to) {
            assert!(replace.range.0 <= replace.range.1 && replace.range.1 <= last);
        }
    }
}
