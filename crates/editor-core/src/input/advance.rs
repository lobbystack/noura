//! Cursor movement by characters, words, lines, paragraphs and the
//! document, and by visual rows through the layout.

use crate::bounds::BoundExt as _;
use crate::doc::MdDoc;
use crate::input::{Advance, Increment, TextLayout};
use crate::offset_types::Grapheme;

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
}
