//! Where an input method's marked text drew: the layout lays it out inline
//! at the caret (see `Layout::insert_marked`), and the caret and the
//! platform's candidate window follow it.

use egui::{Pos2, Rangef, Rect};

use crate::MdRender;
use crate::widget::utils::wrap_layout::FragmentContent;

/// The marked text as the last frame drew it.
pub(crate) struct MarkedGeometry {
    /// Where the caret sits: after the clause the input method converts,
    /// or after the whole marked text.
    pub caret: [Pos2; 2],
    /// Where the candidate window goes: under the start of that clause.
    pub candidates: Rect,
}

impl MdRender {
    /// The pieces of marked text the last frame drew, in order, each with
    /// how many characters it holds.
    fn marked_pieces(&self) -> Vec<(Rect, usize)> {
        let caret = self.buffer.current.selection.1;
        self.fragments
            .iter()
            .filter(|fragment| fragment.source_range == (caret, caret) && fragment.atomic)
            .filter_map(|fragment| match &fragment.content {
                FragmentContent::Glyphs { buffer, .. } => {
                    let buffer = buffer.read().ok()?;
                    let chars: usize = buffer
                        .lines
                        .iter()
                        .map(|line| line.text().chars().count())
                        .sum();
                    (chars > 0).then_some((fragment.rect, chars))
                }
                _ => None,
            })
            .collect()
    }

    /// Where the marked text drew, when an input method is composing and
    /// the last frame laid it out.
    pub(crate) fn marked_geometry(&self) -> Option<MarkedGeometry> {
        let composition = self.doc.composition()?;
        let pieces = self.marked_pieces();
        let total: usize = pieces.iter().map(|(_, chars)| chars).sum();
        if pieces.is_empty() {
            return None;
        }
        // The x and row of the boundary before character `index`.
        let at = |index: usize| -> (f32, Rangef) {
            let mut seen = 0;
            for (rect, chars) in &pieces {
                if index < seen + chars {
                    let into = (index - seen) as f32 / *chars as f32;
                    return (rect.left() + into * rect.width(), rect.y_range());
                }
                seen += chars;
            }
            let (last, _) = pieces.last().expect("pieces aren't empty");
            (last.right(), last.y_range())
        };
        let selected = composition.selected.clone().unwrap_or(total..total);
        let start = if selected.is_empty() {
            0
        } else {
            selected.start
        };
        let (caret_x, caret_row) = at(selected.end.min(total));
        let (start_x, start_row) = at(start.min(total));
        let pad = self.layout.row_spacing / 2.0;
        let caret_row = caret_row.expand(pad);
        Some(MarkedGeometry {
            caret: [
                Pos2::new(caret_x, caret_row.min),
                Pos2::new(caret_x, caret_row.max),
            ],
            candidates: Rect::from_x_y_ranges(start_x..=start_x, start_row.expand(pad)),
        })
    }
}
