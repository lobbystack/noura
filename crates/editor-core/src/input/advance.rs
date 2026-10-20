//! Cursor movement by characters, words, lines, paragraphs and the
//! document, and by visual rows through the layout.

use crate::bounds::{BoundExt as _, RangesExt as _};
use crate::doc::MdDoc;
use crate::input::{Advance, Increment, TextLayout};
use crate::offset_types::{Grapheme, RangeExt as _};

impl MdDoc {
    pub fn advance(
        &mut self,
        offset: Grapheme,
        advance: Advance,
        backwards: bool,
        layout: &dyn TextLayout,
    ) -> Grapheme {
        let maybe_x_target_value = std::mem::take(&mut self.cursor.x_target);
        let result = match advance {
            Advance::To(bound) => offset.advance_to_bound(bound, backwards, &self.bounds),
            Advance::Next(bound) => offset.advance_to_next_bound(bound, backwards, &self.bounds),
            Advance::By(Increment::Char) => {
                let mut result = offset;
                if backwards {
                    if result.0 > 0 {
                        result -= 1;
                    }
                } else {
                    result += 1;
                    result = result.min(self.buffer.current.segs.last_cursor_position());
                }
                result
            }
            Advance::By(Increment::Lines(n)) if layout.rows_are_source_lines() => {
                self.advance_source_lines(offset, n, backwards, maybe_x_target_value)
            }
            Advance::By(Increment::Lines(n)) => {
                let mut result = offset;
                for _ in 0..n {
                    let Some(result_x) = layout.offset_x(result) else {
                        break;
                    };
                    let x_target = maybe_x_target_value.unwrap_or(result_x);
                    result = layout.offset_by_row(result, x_target, backwards);
                    if result != 0 && result != self.buffer.current.segs.last_cursor_position() {
                        self.cursor.x_target = Some(x_target);
                    }
                }
                result
            }
        };
        self.snap_offset_out_of_folds(result, backwards)
    }

    /// `n` source lines from `offset`, holding its column in graphemes, or
    /// the column a run of vertical moves started from. Past the first or
    /// last line, the move lands on the document's edge.
    fn advance_source_lines(
        &mut self,
        offset: Grapheme,
        n: usize,
        backwards: bool,
        column: Option<f32>,
    ) -> Grapheme {
        let lines = &self.bounds.source_lines;
        let last = self.buffer.current.segs.last_cursor_position();
        if lines.is_empty() {
            return offset;
        }
        let (mut line, _) = lines.find_containing(offset, true, true);
        line = line.min(lines.len() - 1);
        let column = column.map_or((offset - lines[line].start()).0, |x| x as usize);

        for _ in 0..n {
            if backwards && line == 0 {
                return Grapheme(0);
            }
            if !backwards && line + 1 == lines.len() {
                return last;
            }
            line = if backwards { line - 1 } else { line + 1 };
        }

        let (start, end) = (lines[line].start(), lines[line].end());
        let result = (start + column).min(end);
        if result != 0 && result != last {
            self.cursor.x_target = Some(column as f32);
        }
        result
    }
}
