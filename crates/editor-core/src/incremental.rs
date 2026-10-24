//! Bounds after an edit, rebuilt only for the lines the edit reparsed.
//!
//! [`crate::Ast::update`] reparses the run of top-level blocks an edit
//! reaches. Every bound outside that run's lines stays as it was, moved by
//! the number of graphemes the edit added; the bounds inside it come from
//! the reparsed blocks, through the same code a full rebuild runs.

use comrak::nodes::{AstNode, NodeValue};

use crate::ast::Reparsed;
use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _};

impl MdDoc {
    /// Brings bounds built for the text before an edit up to date with
    /// `root`, the parse [`crate::Ast::update`] left after reparsing
    /// `reparsed`. The note must hold no fold tags.
    pub(crate) fn update_bounds<'a>(&mut self, root: &'a AstNode<'a>, reparsed: &Reparsed) {
        let old_total = self.bounds.source_lines.last().map_or(0, |line| line.1.0);
        let new_total = self.last_cursor_position().0;
        let delta = new_total as isize - old_total as isize;
        let line_delta = reparsed.new_lines.end as isize - reparsed.old_lines.end as isize;
        let line_start = |lines: &[(Grapheme, Grapheme)], line: usize, total: usize| {
            lines.get(line).map_or(total, |range| range.0.0)
        };

        // Source lines: those of the reparsed lines.
        let run_at_end = reparsed.old_lines.end >= self.bounds.source_lines.len();
        let run_start = Grapheme(line_start(
            &self.bounds.source_lines,
            reparsed.old_lines.start,
            old_total,
        ));
        let run_end = if run_at_end {
            Grapheme(new_total)
        } else {
            let old_end = line_start(&self.bounds.source_lines, reparsed.old_lines.end, old_total);
            shift(Grapheme(old_end), delta)
        };
        let mut lines = self.range_split_newlines((run_start, run_end));
        if !run_at_end {
            // The split ends with the empty line that starts at the next
            // block, which isn't the run's.
            lines.pop();
        }
        splice(
            &mut self.bounds.source_lines,
            reparsed.old_lines.clone(),
            lines,
            delta,
        );

        // The other bounds come from the reparsed blocks with the blocks on
        // either side, whose nested blocks can reach into the blank lines
        // around them, and the blank lines between them all.
        let previous = reparsed
            .blocks
            .start
            .checked_sub(1)
            .and_then(|index| root.children().nth(index));
        let reparsed_blocks: Vec<&'a AstNode<'a>> = root
            .children()
            .skip(reparsed.blocks.start)
            .take(reparsed.blocks.len())
            .collect();
        let next = root.children().nth(reparsed.blocks.end);
        let blocks: Vec<&'a AstNode<'a>> = previous
            .into_iter()
            .chain(reparsed_blocks.iter().copied())
            .chain(next)
            .collect();
        let first_line = previous.map_or(reparsed.old_lines.start, |previous| {
            previous
                .data
                .borrow()
                .sourcepos
                .start
                .line
                .saturating_sub(1)
        });
        // Up to the end of the next block as the editor counts its lines,
        // or of the text when no block follows it, which ends with the next
        // block's spacing.
        let new_end_line = next
            .filter(|next| next.next_sibling().is_some())
            .map(|next| self.node_last_line_idx(next) + 1);
        let lo = Grapheme(line_start(&self.bounds.source_lines, first_line, new_total));
        let (old_hi, new_hi, new_lines) = match new_end_line {
            Some(end) => {
                let new_hi = Grapheme(line_start(&self.bounds.source_lines, end, new_total));
                (shift(new_hi, -delta), new_hi, first_line..end)
            }
            None => (
                Grapheme(usize::MAX),
                Grapheme(new_total),
                first_line..self.bounds.source_lines.len(),
            ),
        };
        debug_assert!(new_end_line.is_none_or(|end| end as isize - line_delta >= 0));

        // Words, with the gutter columns of those lines.
        let mut words = self.words_in((lo, new_hi));
        self.syntax_words_in(&blocks, new_lines, &mut words);
        splice_by_start(&mut self.bounds.words, (lo, old_hi), words, delta);

        // Inline paragraphs.
        let kept = std::mem::take(&mut self.bounds.inline_paragraphs);
        if root.children().next().is_none() {
            self.compute_bounds(root);
        } else {
            for block in &blocks {
                if !previous.is_some_and(|previous| previous.same_node(block)) {
                    self.compute_bounds_block_pre_spacing(block);
                }
                self.compute_bounds(block);
                if block.next_sibling().is_none() {
                    self.compute_bounds_block_post_spacing(block);
                }
            }
        }
        let mut paragraphs = std::mem::replace(&mut self.bounds.inline_paragraphs, kept);
        paragraphs.sort();
        splice_by_start(
            &mut self.bounds.inline_paragraphs,
            (lo, old_hi),
            paragraphs,
            delta,
        );

        // Image atoms.
        let mut images = Vec::new();
        if !self.disable_images {
            for block in &blocks {
                for node in block.descendants() {
                    if let NodeValue::Image(_) = node.data.borrow().value {
                        images.push(self.node_range(node));
                    }
                }
            }
        }
        images.sort_unstable();
        splice_by_start(&mut self.bounds.images, (lo, old_hi), images, delta);

        // Obsidian-style spans, and the frontmatter, which only the start
        // of the text decides.
        let spans = self.inline_spans_in(blocks.iter().copied());
        let all = &mut self.bounds.spans;
        let first = all.partition_point(|span| span.range.start() < lo);
        let after = all
            .partition_point(|span| span.range.start() < old_hi)
            .max(first);
        for span in &mut all[after..] {
            span.shift(delta);
        }
        all.splice(first..after, spans);
        self.bounds.frontmatter = self.frontmatter_range();

        self.bounds.text_seq = self.text_seq;
        self.bounds_seq = self.next_seq();
    }
}

fn shift(offset: Grapheme, delta: isize) -> Grapheme {
    Grapheme((offset.0 as isize + delta) as usize)
}

/// Replaces `ranges[old]` with `new`, and moves the ranges after it by
/// `delta`.
fn splice(
    ranges: &mut Vec<(Grapheme, Grapheme)>,
    old: std::ops::Range<usize>,
    new: Vec<(Grapheme, Grapheme)>,
    delta: isize,
) {
    let old = old.start.min(ranges.len())..old.end.min(ranges.len());
    for range in &mut ranges[old.end..] {
        *range = (shift(range.0, delta), shift(range.1, delta));
    }
    ranges.splice(old, new);
}

/// Replaces the sorted ranges that start in `region`, in the text before
/// an edit, with `new`, and moves the ranges after it by `delta`.
fn splice_by_start(
    ranges: &mut Vec<(Grapheme, Grapheme)>,
    region: (Grapheme, Grapheme),
    new: Vec<(Grapheme, Grapheme)>,
    delta: isize,
) {
    let first = ranges.partition_point(|range| range.start() < region.0);
    let after = ranges.partition_point(|range| range.start() < region.1);
    splice(ranges, first..after.max(first), new, delta);
}
