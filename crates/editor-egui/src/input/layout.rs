//! The layout side of edit commands: `editor-core` asks where a point lands
//! and which offset sits a visual row away, and the fragments painted last
//! frame answer.

use editor_core::input::{Pos, TextLayout};
use editor_core::offset_types::{Grapheme, IntoRangeExt as _, RangeExt as _};
use egui::{Pos2, Rangef};

use comrak::nodes::AstNode;
use editor_core::buffer;
use editor_core::operation_types::Operation;

use crate::MdEdit;
use crate::input::{Event, Location, Region};
use crate::widget::utils::wrap_layout::{FragmentContent, Fragments};

/// The fragments painted last frame, as `editor-core`'s [`TextLayout`].
pub struct FragmentLayout<'a> {
    pub fragments: Fragments<'a>,
    /// The cursor position at the end of the text.
    pub last: Grapheme,
}

impl TextLayout for FragmentLayout<'_> {
    fn pos_to_range(&self, pos: Pos) -> (Grapheme, Grapheme) {
        FragmentLayout::pos_to_range(self, Pos2::new(pos.x, pos.y))
    }

    fn pos_to_offset(&self, pos: Pos) -> Grapheme {
        self.pos_to_char_offset(Pos2::new(pos.x, pos.y))
    }

    fn offset_x(&self, offset: Grapheme) -> Option<f32> {
        self.x(offset)
    }

    fn offset_by_row(&self, offset: Grapheme, x_target: f32, backwards: bool) -> Grapheme {
        self.advance_by_line(offset, x_target, backwards)
    }
}

impl MdEdit {
    /// Translates an event into buffer operations through `editor-core`,
    /// answering its layout questions from the last frame's fragments, and
    /// hands any copied text to the system clipboard.
    pub fn calc_operations<'ast>(
        &mut self,
        ctx: &egui::Context,
        root: &'ast AstNode<'ast>,
        event: Event,
        operations: &mut Vec<Operation>,
    ) -> buffer::Response {
        let renderer = &mut self.renderer;
        let layout = FragmentLayout {
            fragments: Fragments(&renderer.fragments),
            last: renderer.doc.buffer.current.segs.last_cursor_position(),
        };
        let response = renderer
            .doc
            .calc_operations(root, event, operations, &layout);
        if let Some(text) = renderer.doc.copied.take() {
            ctx.copy_text(text);
        }
        response
    }

    /// The range a region names, resolved against the last frame's layout.
    pub fn region_to_range(&mut self, region: Region) -> (Grapheme, Grapheme) {
        let renderer = &mut self.renderer;
        let layout = FragmentLayout {
            fragments: Fragments(&renderer.fragments),
            last: renderer.doc.buffer.current.segs.last_cursor_position(),
        };
        renderer.doc.region_to_range(region, &layout)
    }

    /// The cursor position a location names, resolved against the last
    /// frame's layout.
    pub fn location_to_char_offset(&self, location: Location) -> Grapheme {
        self.renderer
            .doc
            .location_to_char_offset(location, &self.renderer.text_layout())
    }

    /// `n` visual lines from `offset`, holding its column. Pure: a platform
    /// text system keeps the column by always moving from the same start.
    pub fn lines_from(&self, offset: Grapheme, n: usize, backwards: bool) -> Grapheme {
        let Some(x_target) = self.renderer.text_layout().x(offset) else {
            return offset;
        };
        let result = (0..n).fold(offset, |result, _| {
            self.renderer
                .text_layout()
                .advance_by_line(result, x_target, backwards)
        });
        self.renderer.snap_offset_out_of_folds(result, backwards)
    }
}

impl FragmentLayout<'_> {
    fn advance_by_line(&self, offset: Grapheme, x_target: f32, backwards: bool) -> Grapheme {
        // Mirror `fragment_at_offset`'s last-match semantics so navigation
        // agrees with cursor rendering at wrap boundaries.
        let Some(cur_idx) = self
            .fragments
            .0
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, f)| {
                let (s, e) = f.source_range;
                (s <= offset && offset <= e).then_some(i)
            })
        else {
            return offset;
        };
        // A far row has no laid-out neighbors to move to.
        if self.fragments.0[cur_idx].far {
            return offset;
        }
        let cur_top = self.fragments.0[cur_idx].rect.top();

        // Walk one visual row at a time in the travel direction, nearest
        // first, taking the first row with a candidate that actually
        // *renders* past the current row. Climbing past a row with no such
        // candidate steps over wrap seams whose only offsets render back
        // onto the current row.
        let mut row_cutoff = if backwards {
            cur_top
        } else {
            self.fragments.0[cur_idx].rect.bottom()
        };
        loop {
            let (best, row_edge) =
                self.best_offset_on_row(cur_idx, row_cutoff, cur_top, x_target, backwards);
            if let Some(best) = best {
                return best;
            }
            match row_edge {
                Some(edge) => row_cutoff = edge, // nothing rendered past us; keep climbing
                None => break,                   // no further row in this direction
            }
        }

        // No reachable row remains: snap to the document edge.
        if backwards { Grapheme(0) } else { self.last }
    }

    /// Picks the destination on the visual row adjacent to `row_cutoff` (just
    /// above it when `backwards`, just below otherwise): the candidate closest
    /// to `x_target` that *renders* past `cur_top` in the travel direction.
    /// Also returns the row's far edge (top going up, bottom going down) so the
    /// caller can climb past a row with no such candidate.
    ///
    /// Candidates are each fragment's closest point to `x_target`, plus the
    /// grapheme just inside the row's far edge (`row_end - 1` up, `row_start +
    /// 1` down) for nonempty rows — all ordered only by distance to `x_target`.
    /// The inner grapheme matters at a soft-wrap seam: the row's end offset is
    /// closest to a right-leaning `x_target` but renders on the row below (so it
    /// fails the test), leaving the inner grapheme as the nearest candidate that
    /// renders on the row itself. Down's `row_start + 1` never wins — last-match
    /// resolves a seam to the lower row, already correct for down — but is kept
    /// for symmetry.
    fn best_offset_on_row(
        &self,
        cur_idx: usize,
        row_cutoff: f32,
        cur_top: f32,
        x_target: f32,
        backwards: bool,
    ) -> (Option<Grapheme>, Option<f32>) {
        let fragments = self.fragments.0;

        // Gather the adjacent row's fragments: each contributes its closest
        // point to `x_target`, while we track the row's source extent and
        // the fragments holding its far edges for the inner-grapheme step.
        let mut row_edge: Option<f32> = None;
        let mut candidates: Vec<(Grapheme, f32, bool)> = Vec::new(); // (offset, gen_x, empty_frag)
        let (mut lo, mut hi) = (Grapheme(usize::MAX), Grapheme(0));
        let (mut lo_idx, mut hi_idx) = (None, None);

        let mut idx = cur_idx;
        loop {
            if backwards {
                if idx == 0 {
                    break;
                }
                idx -= 1;
            } else {
                idx += 1;
                if idx >= fragments.len() {
                    break;
                }
            }
            let frag = &fragments[idx];
            if frag.far {
                continue;
            }

            // Adjacent in the travel direction, and not yet onto the row
            // past the first one we land on.
            let adjacent = if backwards {
                frag.rect.bottom() < row_cutoff
            } else {
                frag.rect.top() > row_cutoff
            };
            let past_row = row_edge.is_some_and(|edge| {
                if backwards {
                    frag.rect.bottom() < edge
                } else {
                    frag.rect.top() > edge
                }
            });

            if past_row {
                break;
            } else if adjacent {
                row_edge = Some(if backwards {
                    frag.rect.top()
                } else {
                    frag.rect.bottom()
                });

                let (s, e) = (frag.source_range.start(), frag.source_range.end());
                if s < lo {
                    (lo, lo_idx) = (s, Some(idx));
                }
                if e > hi {
                    (hi, hi_idx) = (e, Some(idx));
                }

                let off = self.fragments.fragment_offset(frag, x_target);
                candidates.push((off, self.fragments.fragment_x(frag, off), s == e));
            }
        }

        // The grapheme just inside the row's far edge — lets an up-press
        // land on a soft-wrapped row rather than on its seam (see above).
        if lo < hi {
            if backwards {
                if let Some(i) = hi_idx {
                    let off = Grapheme(hi.0 - 1);
                    candidates.push((off, self.fragments.fragment_x(&fragments[i], off), false));
                }
            } else if let Some(i) = lo_idx {
                let off = Grapheme(lo.0 + 1);
                candidates.push((off, self.fragments.fragment_x(&fragments[i], off), false));
            }
        }

        // Closest candidate to `x_target` that renders past the current row
        // in the travel direction. Ties prefer empty fragments, placed
        // deliberately to steer this navigation.
        let mut best: Option<Grapheme> = None;
        let mut best_distance = f32::INFINITY;
        for (off, gen_x, empty) in candidates {
            let renders_past = self.fragments.fragment_at_offset(off).is_some_and(|f| {
                if backwards {
                    f.rect.top() < cur_top
                } else {
                    f.rect.top() > cur_top
                }
            });
            if !renders_past {
                continue;
            }
            let distance = (gen_x - x_target).abs();
            if distance < best_distance || (distance == best_distance && empty) {
                (best, best_distance) = (Some(off), distance);
            }
        }

        (best, row_edge)
    }

    /// returns the x coordinate of the absolute position of `self` in `fragment`
    fn x(&self, offset: Grapheme) -> Option<f32> {
        let frag = self.fragments.fragment_at_offset(offset)?;
        Some(self.fragments.fragment_x(frag, offset))
    }
}

impl FragmentLayout<'_> {
    // todo: find a better home
    pub fn pos_to_range(&self, pos: Pos2) -> (Grapheme, Grapheme) {
        let Some(frag_idx) = self.fragments.closest_fragment_at_pos(pos) else {
            return Grapheme(0).into_range();
        };
        let frag = &self.fragments.0[frag_idx];

        // Past the last fragment's bottom: jump to doc end.
        let is_last = frag_idx + 1 == self.fragments.0.len();
        if is_last && pos.y > frag.rect.max.y {
            return self.last.into_range();
        }

        if frag.source_range.is_empty() {
            // Anchor / empty-range fragment: every position maps to
            // its source point.
            frag.source_range.start().into_range()
        } else if frag.atomic && !matches!(frag.content, FragmentContent::Embed { .. }) {
            // Atomic fragment (marker, indentation): a click selects the whole
            // range. Images are excluded — clicking *near* one (resolved as the
            // nearest fragment) places a cursor at its edge; selecting the image
            // is reserved for a click on it via `handle_image_interactions`.
            frag.source_range
        } else {
            self.fragments.fragment_offset(frag, pos.x).into_range()
        }
    }

    /// Resolves a pointer position to a single cursor offset. Unlike
    /// [`Self::pos_to_range`] (which returns an atomic fragment's *whole*
    /// range so a click selects it), this edge-snaps an atomic fragment
    /// to the nearer edge by `pos.x` via `fragment_offset`. That lets
    /// a drag's anchor and moving end land on opposite edges of a marker
    /// / indentation column and select it, rather than both collapsing
    /// to its start.
    pub fn pos_to_char_offset(&self, pos: Pos2) -> Grapheme {
        let Some(frag_idx) = self.fragments.closest_fragment_at_pos(pos) else {
            return Grapheme(0);
        };
        let frag = &self.fragments.0[frag_idx];

        // Past the last fragment's bottom: jump to doc end.
        let is_last = frag_idx + 1 == self.fragments.0.len();
        if is_last && pos.y > frag.rect.max.y {
            return self.last;
        }

        // The hit-test inverse: edge-snaps atomic fragments by the rect
        // midpoint, walks clusters otherwise, and collapses an empty
        // range to its start.
        self.fragments.fragment_offset(frag, pos.x)
    }
}

pub fn distance(coord: f32, range: Rangef) -> f32 {
    if range.contains(coord) {
        0.
    } else {
        (coord - range.min).abs().min((coord - range.max).abs())
    }
}
