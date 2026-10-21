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

/// A piece of marked text the last frame drew: where, and where each of
/// its characters starts.
struct MarkedPiece {
    rect: Rect,
    /// The x where each character starts, in order.
    starts: Vec<f32>,
}

impl MdRender {
    /// The pieces of marked text the last frame drew, in order.
    fn marked_pieces(&self) -> Vec<MarkedPiece> {
        let caret = self.buffer.current.selection.1;
        let ppi = self.ctx.pixels_per_point();
        self.fragments
            .iter()
            .filter(|fragment| fragment.source_range == (caret, caret) && fragment.atomic)
            .filter_map(|fragment| match &fragment.content {
                FragmentContent::Glyphs { buffer, .. } => {
                    let buffer = buffer.read().ok()?;
                    // Each character's advance, from the glyphs the
                    // shaper placed: a glyph covering several characters
                    // counts toward its first.
                    let lines: Vec<Vec<usize>> = buffer
                        .lines
                        .iter()
                        .map(|line| line.text().char_indices().map(|(at, _)| at).collect())
                        .collect();
                    let firsts: Vec<usize> = lines
                        .iter()
                        .scan(0, |first, chars| {
                            let at = *first;
                            *first += chars.len();
                            Some(at)
                        })
                        .collect();
                    let mut advances = vec![0.0; lines.iter().map(Vec::len).sum()];
                    for run in buffer.layout_runs() {
                        let (Some(chars), Some(first)) =
                            (lines.get(run.line_i), firsts.get(run.line_i))
                        else {
                            continue;
                        };
                        for glyph in run.glyphs {
                            let index = chars.partition_point(|&at| at <= glyph.start);
                            if let Some(index) = index.checked_sub(1) {
                                advances[first + index] += glyph.w / ppi;
                            }
                        }
                    }
                    if advances.is_empty() {
                        return None;
                    }
                    let mut x = fragment.rect.left() + fragment.content_inset.left;
                    let starts = advances
                        .iter()
                        .map(|advance| {
                            let start = x;
                            x += advance;
                            start
                        })
                        .collect();
                    Some(MarkedPiece {
                        rect: fragment.rect,
                        starts,
                    })
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
        let total: usize = pieces.iter().map(|piece| piece.starts.len()).sum();
        let last = pieces.last()?;
        // The x and row of the boundary before character `index`.
        let at = |index: usize| -> (f32, Rangef) {
            let mut seen = 0;
            for piece in &pieces {
                if let Some(x) = piece.starts.get(index - seen) {
                    return (*x, piece.rect.y_range());
                }
                seen += piece.starts.len();
            }
            (last.rect.right(), last.rect.y_range())
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
