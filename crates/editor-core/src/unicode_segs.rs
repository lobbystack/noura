//! Conversions between byte offsets and grapheme offsets.

use unicode_segmentation::UnicodeSegmentation as _;

use crate::offset_types::{Byte, Grapheme, Graphemes, RangeExt as _};

/// The byte offset where each grapheme cluster of a text starts, followed by
/// the text's length. Index `i` holds the byte offset of [`Grapheme`]`(i)`,
/// so a text of `n` clusters has `n + 1` entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnicodeSegs {
    pub grapheme_indexes: Vec<Byte>,
}

impl Default for UnicodeSegs {
    fn default() -> Self {
        Self {
            grapheme_indexes: vec![Byte(0)],
        }
    }
}

/// Segments `text` into grapheme clusters.
pub fn calc(text: &str) -> UnicodeSegs {
    let mut grapheme_indexes: Vec<Byte> = text
        .grapheme_indices(true)
        .map(|(offset, _)| Byte(offset))
        .collect();
    grapheme_indexes.push(Byte(text.len()));
    UnicodeSegs { grapheme_indexes }
}

impl UnicodeSegs {
    /// Byte offset of a grapheme offset.
    pub fn offset_to_byte(&self, offset: Grapheme) -> Byte {
        self.grapheme_indexes[offset.0]
    }

    pub fn range_to_byte(&self, range: (Grapheme, Grapheme)) -> (Byte, Byte) {
        (self.offset_to_byte(range.0), self.offset_to_byte(range.1))
    }

    /// Grapheme offset of a byte offset that starts a cluster.
    ///
    /// # Panics
    ///
    /// Panics when `byte` falls inside a cluster. Use
    /// [`Self::byte_to_char_floor`] or [`Self::byte_to_char_ceil`] for a byte
    /// from a source that doesn't know about clusters.
    pub fn offset_to_char(&self, byte: Byte) -> Grapheme {
        match self.grapheme_indexes.binary_search(&byte) {
            Ok(index) => Grapheme(index),
            Err(_) => panic!("byte offset {byte:?} is not on a grapheme boundary"),
        }
    }

    pub fn range_to_char(&self, range: (Byte, Byte)) -> (Grapheme, Grapheme) {
        (self.offset_to_char(range.0), self.offset_to_char(range.1))
    }

    /// The cluster containing `byte`, or the one it starts.
    pub fn byte_to_char_floor(&self, byte: Byte) -> Grapheme {
        match self.grapheme_indexes.binary_search(&byte) {
            Ok(index) => Grapheme(index),
            Err(index) => Grapheme(index.saturating_sub(1)),
        }
    }

    /// The first cluster boundary at or after `byte`.
    pub fn byte_to_char_ceil(&self, byte: Byte) -> Grapheme {
        match self.grapheme_indexes.binary_search(&byte) {
            Ok(index) => Grapheme(index),
            Err(index) => Grapheme(index.min(self.grapheme_indexes.len() - 1)),
        }
    }

    /// The cursor position at the end of the text.
    pub fn last_cursor_position(&self) -> Grapheme {
        Grapheme(self.grapheme_indexes.len() - 1)
    }

    /// Same as [`Self::last_cursor_position`].
    pub fn last_grapheme(&self) -> Grapheme {
        self.last_cursor_position()
    }

    /// Number of clusters a replacement added to the text, counting clusters
    /// that fused with their neighbors at the seams. `self` describes the text
    /// before the replacement and `after` the text after it.
    pub fn measure_replace(
        &self,
        after: &UnicodeSegs,
        replaced: (Grapheme, Grapheme),
    ) -> Graphemes {
        let before = self.last_cursor_position().0;
        let after = after.last_cursor_position().0;
        Graphemes((after + replaced.len().0).saturating_sub(before))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_has_one_cursor_position() {
        let segs = calc("");
        assert_eq!(segs.last_cursor_position(), Grapheme(0));
        assert_eq!(segs.offset_to_byte(Grapheme(0)), Byte(0));
    }

    #[test]
    fn clusters_map_to_their_starting_bytes() {
        // "é" as e + combining acute is one cluster of three bytes.
        let segs = calc("ae\u{301}b");
        assert_eq!(segs.last_cursor_position(), Grapheme(3));
        assert_eq!(segs.offset_to_byte(Grapheme(2)), Byte(4));
        assert_eq!(segs.offset_to_char(Byte(4)), Grapheme(2));
        assert_eq!(segs.byte_to_char_floor(Byte(3)), Grapheme(1));
        assert_eq!(segs.byte_to_char_ceil(Byte(3)), Grapheme(2));
    }

    #[test]
    fn measure_replace_counts_fused_clusters() {
        // Inserting a combining mark after "a" adds bytes but no cluster.
        let before = calc("ab");
        let after = calc("a\u{301}b");
        assert_eq!(
            before.measure_replace(&after, (Grapheme(1), Grapheme(1))),
            Graphemes(0)
        );
    }
}
