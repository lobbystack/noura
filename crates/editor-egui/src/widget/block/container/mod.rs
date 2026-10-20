//! Container blocks (lists, items, blockquotes, alerts) and per-
//! line prefix math. Prefixes are graphemes (`line_own_prefix`,
//! `extension_own_prefix`); nesting depth is columns
//! (`deindent_level_cols` + helpers in [`super::super::utils`]).

use comrak::nodes::{AstNode, NodeValue};
use editor_core::offset_types::{Grapheme, RangeExt as _};
use egui::{Pos2, Rect, Ui};

use crate::MdRender;
use crate::widget::utils::wrap_layout::{Fragment, FragmentContent, FragmentInset};

pub(crate) mod alert;
pub(crate) mod block_quote;
pub(crate) mod document;
pub(crate) mod footnote_definition;
pub(crate) mod item;
pub(crate) mod list;
pub(crate) mod table;
pub(crate) mod table_row;
pub(crate) mod task_item;

impl<'ast> MdRender {
    // the height of a block that contains blocks is the sum of the heights of the blocks it contains
    pub fn block_children_height(&self, node: &'ast AstNode<'ast>) -> f32 {
        let mut height_sum = 0.0;
        for child in node.children() {
            height_sum += self.block_pre_spacing_height(child);
            height_sum += self.height(child);
            height_sum += self.block_post_spacing_height(child);
        }
        height_sum
    }

    /// Returns the document offset ranges for which fragments must
    /// exist, covering the selection ± 1 source line so arrow-key
    /// navigation across the viewport edge has a fragment to land
    /// on. Also includes the current find match.
    pub fn fragment_required_ranges(
        &self,
        in_progress_selection: Option<(Grapheme, Grapheme)>,
        find_match: Option<(Grapheme, Grapheme)>,
    ) -> Vec<(Grapheme, Grapheme)> {
        if self.bounds.source_lines.is_empty() {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        let selection = in_progress_selection.unwrap_or(self.buffer.current.selection);
        ranges.push(self.source_line_range(selection));
        if let Some(match_range) = find_match {
            ranges.push(self.source_line_range(match_range));
        }
        if let Some(preview_range) = self.preview_match {
            ranges.push(self.source_line_range(preview_range));
        }
        // Keep the dragged item rendered even after auto-scroll parks its
        // source slot off-screen — the floating card and drop-gap both
        // read its `block_box`.
        if let Some(drag) = self.in_progress_block_drag {
            ranges.push(self.source_line_range(drag.section_range));
        }
        ranges
    }

    // blocks are stacked vertically; only visible blocks and blocks
    // whose node range intersects `fragment_required_ranges` are shown
    pub fn show_block_children(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
    ) {
        let required_ranges =
            self.fragment_required_ranges(self.in_progress_selection, self.find_current_match);
        let viewport = ui.clip_rect();
        let buffer = viewport.height();

        let intersects_any_required = |range: &(Grapheme, Grapheme)| -> bool {
            required_ranges.iter().any(|rr| range.intersects(rr, true))
        };
        let past_all_required =
            |offset: Grapheme| -> bool { required_ranges.iter().all(|rr| offset > rr.end()) };

        for child in node.children() {
            let child_range = self.node_range(child);
            let pre_lines = self.pre_spacing_lines(child);
            let post_lines = self.post_spacing_lines(child);

            // add pre-spacing
            let pre_spacing = self.block_pre_spacing_height(child);
            let pre_spacing_below_viewport = viewport.max.y < top_left.y;
            let pre_spacing_above_viewport = viewport.min.y > top_left.y + pre_spacing;
            let pre_spacing_visible = !pre_spacing_above_viewport && !pre_spacing_below_viewport;
            let pre_spacing_needed = intersects_any_required(&self.spacing_range(&pre_lines));
            if pre_spacing_visible || pre_spacing_needed {
                self.show_block_pre_spacing(ui, child, top_left);
            }
            top_left.y += pre_spacing;

            // add block
            let child_height = self.height(child);

            if self.debug {
                self.show_debug_block_highlight(
                    ui,
                    child,
                    top_left,
                    self.width(child),
                    child_height,
                );
            }

            let block_below_viewport = viewport.max.y < top_left.y;
            let block_above_viewport = viewport.min.y > top_left.y + child_height;
            let block_visible = !block_above_viewport && !block_below_viewport;
            let block_needed = intersects_any_required(&child_range);
            if block_visible || block_needed {
                self.show_block(ui, child, top_left);

                // Index list items for drag-to-reorder; the parent
                // `List`'s start identifies the sibling group.
                if matches!(
                    child.data.borrow().value,
                    NodeValue::Item(_) | NodeValue::TaskItem(_)
                ) {
                    let block_rect = Rect::from_min_size(
                        top_left,
                        egui::Vec2::new(self.width(child), child_height),
                    );
                    let parent_start = self.node_range(node).start();
                    self.push_block_box(child, block_rect, parent_start);
                }
            } else {
                let in_buffer = top_left.y + child_height > viewport.min.y - buffer
                    && top_left.y < viewport.max.y + buffer;
                if in_buffer {
                    // walks all descendants, not just those within the buffer —
                    // a tall container may warm images beyond the zone
                    self.warm_images(child);
                }
            }
            top_left.y += child_height;

            // add post-spacing
            let post_spacing = self.block_post_spacing_height(child);
            let post_spacing_below_viewport = viewport.max.y < top_left.y;
            let post_spacing_above_viewport = viewport.min.y > top_left.y + post_spacing;
            let post_spacing_visible = !post_spacing_above_viewport && !post_spacing_below_viewport;
            let post_spacing_needed = intersects_any_required(&self.spacing_range(&post_lines));
            if post_spacing_visible || post_spacing_needed {
                self.show_block_post_spacing(ui, child, top_left);
            }
            top_left.y += post_spacing;

            // safe to stop: everything remaining is below the buffer zone and
            // past all ranges that need galleys
            let past_buffer = top_left.y > viewport.max.y + buffer;
            if past_buffer && past_all_required(child_range.start()) {
                break;
            }
        }
    }

    /// Registers an atomic, selectable `Fragment` for each gutter
    /// column (marker / per-level indentation) on `line`, laid out in
    /// the indent-width columns left of `content_top_left` (the content
    /// origin, after every prefix shift).
    ///
    /// Each column's `line_own_prefix` becomes one `Spacer`: a drag
    /// lands only at its edges (so a click-drag never reveals syntax —
    /// see `reveal`) and a selection highlights the whole column. The
    /// columns are non-atomic, so a single click places the cursor at the
    /// nearer edge rather than selecting the column; drag or double-click
    /// selects it. Marker glyphs are still painted by the per-container
    /// `show_*`; these fragments carry none and exist only for
    /// hit-testing and selection highlighting.
    ///
    /// Called from the block-line renderers, including the pre/post
    /// spacing renderers whose blank lines can still carry a container's
    /// indentation. A revealed level renders its own column as ordinary
    /// source syntax in place of the `Spacer` and its decoration.
    ///
    /// Resolves the line's columns from its `deepest_gutter_level`, not
    /// `node`'s ancestors: a spacing line sits in a shallower node than
    /// the content it spaces, so walking up from `node` would miss the
    /// deeper columns. For content lines `node` is already on the line, so
    /// the two agree.
    pub fn show_block_line_prefixes(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
        content_top_left: Pos2,
        row_height: f32,
    ) {
        let Some(deepest) = self.deepest_gutter_level(node, line) else {
            return;
        };
        let indent = self.layout.indent;
        let mut levels: Vec<_> = deepest
            .ancestors()
            .filter(|n| self.is_gutter_level(n))
            .collect();
        levels.reverse();
        let base_x = content_top_left.x - indent * levels.len() as f32;

        let line_inflated = self.row_height_inflated_at_line(node, line);
        let line_row_height = line_inflated.unwrap_or(row_height);

        for (k, level) in levels.iter().enumerate() {
            let range = self.line_own_prefix(level, line);
            if range.is_empty() {
                continue;
            }
            let left = base_x + indent * k as f32;

            // Revealed: the cursor is in this level's syntax, so render its
            // prefix bytes as monospace source, sized and baseline-aligned to
            // match the gutter annotation (e.g. an ordered marker). Right-
            // aligned to the column's content edge so the marker sits about
            // where its decoration was; a marker wider than the column
            // overflows left (into the gutter) rather than over the content.
            if self.reveal_gutter_column(level, line) {
                let afs = self.layout.annotation_font_size;
                let result = self.compute_section_layout_new(
                    range,
                    self.width(level),
                    afs,
                    self.text_format_syntax(),
                );
                let baseline_shift = if line_inflated.is_some() {
                    (line_row_height - afs) / 2.0
                } else {
                    (row_height - afs) * 0.8
                };
                // `result.width` is the available width, not the laid-out
                // marker width; measure the rightmost glyph edge instead.
                let marker_width = result
                    .rows
                    .iter()
                    .flat_map(|row| &row.fragments)
                    .fold(0.0_f32, |w, frag| w.max(frag.rect.right()));
                let marker_left = left + indent - marker_width;
                let top = Pos2::new(marker_left, content_top_left.y + baseline_shift);
                let first = self.fragments.len();
                self.show_wrap_layout(ui, top, &result);
                // Cursor reads `rect`: give the small marker the row's full
                // height so an offset inside it shares the row's center y
                // (vertical nav round-trips).
                for frag in &mut self.fragments[first..] {
                    frag.rect.min.y = content_top_left.y;
                    frag.rect.max.y = content_top_left.y + line_row_height;
                }
                continue;
            }

            let rect = Rect::from_min_max(
                Pos2::new(left, content_top_left.y),
                Pos2::new(left + indent, content_top_left.y + line_row_height),
            );
            self.fragments.push(Fragment {
                rect,
                content_inset: FragmentInset::default(),
                source_range: range,
                style_stack: Vec::new(),
                content: FragmentContent::Spacer,
                // Non-atomic: a click places the cursor at the nearer
                // edge. A drag still selects the whole column (an empty-
                // glyph `Spacer` midpoint-snaps to an edge either way, so
                // a drag never lands interior — see `reveal`); a double-
                // click selects it via `bounds.words`.
                atomic: false,
                interaction: None,
                far: false,
            });
        }
    }

    /// Returns the row height of a line in the given node, even if that node is
    /// a container block.
    pub fn node_line_row_height(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> f32 {
        // leaf blocks and inlines
        if node.data.borrow().value.contains_inlines() || !node.data.borrow().value.block() {
            return self.row_height(node);
        }

        // container blocks only
        for child in node.children() {
            if self.node_contains_line(child, line) {
                return self.node_line_row_height(child, line);
            }
        }

        self.row_height(node)
    }

    /// Visual height of `node`'s first content row when an inline image
    /// inflates it; `None` otherwise. Mirrors the wrap-layout row
    /// metric (`max(default_ascent, max_image_height) + default_descent`)
    /// so sibling markers can center on the actual row.
    pub fn first_content_row_height_inflated(&self, node: &'ast AstNode<'ast>) -> Option<f32> {
        let mut leaf = node;
        while !leaf.data.borrow().value.contains_inlines() {
            leaf = leaf.children().next()?;
        }
        self.row_height_inflated_at_line(leaf, self.node_first_line(leaf))
    }

    /// Visual height of `node`'s row at `line` when an inline image inflates
    /// it; `None` otherwise.
    pub fn row_height_inflated_at_line(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<f32> {
        let mut leaf = node;
        while !leaf.data.borrow().value.contains_inlines() {
            leaf = leaf.children().next()?;
        }
        let leaf_node_line = self.node_line(leaf, line);
        if leaf_node_line.is_empty() || self.disable_images {
            return None;
        }
        let max_image_height = leaf
            .descendants()
            .filter(|d| matches!(d.data.borrow().value, NodeValue::Image(_)))
            .filter(|d| leaf_node_line.contains_range(&self.node_range(d), true, true))
            // Same collapse predicate as `layout_image`, else a selected image
            // stays tall but the row doesn't inflate, shifting the marker.
            .filter(|d| !self.range_revealed_interior(self.node_range(d)))
            .filter_map(|d| self.image_logical_size(d).map(|s| s.y))
            .fold(0.0_f32, f32::max);
        let leaf_row_height = self.row_height(leaf);
        let leaf_ascent = leaf_row_height * 0.8;
        let leaf_descent = leaf_row_height * 0.2;
        (max_image_height > leaf_ascent).then_some(max_image_height + leaf_descent)
    }
}
