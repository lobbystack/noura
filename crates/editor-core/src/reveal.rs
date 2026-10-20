//! Reveal rules: which syntax shows. Syntax shows while the selection, the
//! current find match or a search preview touches its construct, and stays
//! hidden otherwise.

use comrak::nodes::{AstNode, NodeCodeBlock, NodeValue};

use crate::bounds::RangesExt as _;
use crate::doc::MdDoc;
use crate::nodes::NodeValueExt as _;
use crate::offset_types::{Grapheme, RangeExt as _, RangeIterExt as _};

impl<'ast> MdDoc {
    /// Returns true if the node intersects the current selection. Useful for
    /// checking if syntax should be revealed for an inline node. Block nodes
    /// generally need additional consideration for optional indentation etc.
    pub fn node_intersects_selection(&self, node: &'ast AstNode<'ast>) -> bool {
        self.node_range(node)
            .intersects(&self.buffer.current.selection, true)
    }

    pub fn node_contains_selection(&self, node: &'ast AstNode<'ast>) -> bool {
        self.node_range(node)
            .contains_range(&self.buffer.current.selection, true, true)
    }

    /// Returns true if the node's range intersects any reveal range. Drop-in
    /// replacement for `node_intersects_selection` in reveal contexts.
    pub fn node_revealed(&self, node: &'ast AstNode<'ast>) -> bool {
        self.range_revealed(self.node_range(node), true)
    }

    pub fn reveal_ranges(&self) -> impl Iterator<Item = (Grapheme, Grapheme)> + '_ {
        self.reveal_selection
            .into_iter()
            .chain(self.find_current_match)
            .chain(self.preview_match)
    }

    /// Returns true if `range` intersects any reveal range.
    pub fn range_revealed(&self, range: (Grapheme, Grapheme), allow_empty: bool) -> bool {
        self.reveal_ranges()
            .any(|rr| range.intersects(&rr, allow_empty))
    }

    /// True if any reveal range has an endpoint *strictly inside* `range`.
    /// Unlike [`Self::range_revealed`], bordering or wholly enclosing it
    /// (tap-select, select-all) doesn't count — only a cursor/end in the
    /// interior. Exception: `range` is the atom entered via `Event::EnterAtom`,
    /// whose whole-URL selection has no interior endpoint to offer.
    pub fn range_revealed_interior(&self, range: (Grapheme, Grapheme)) -> bool {
        self.entered_atom == Some(range)
            || self.reveal_ranges().any(|rr| {
                range.contains(rr.start(), false, false) || range.contains(rr.end(), false, false)
            })
    }

    /// Returns true if `range` contains any reveal range.
    pub fn range_contains_revealed(
        &self,
        range: (Grapheme, Grapheme),
        allow_empty_range: bool,
        allow_empty_selection: bool,
    ) -> bool {
        self.reveal_ranges()
            .any(|rr| range.contains_range(&rr, allow_empty_range, allow_empty_selection))
    }

    /// Returns true if any reveal range start or end falls within `range`.
    pub fn range_contains_reveal_endpoint(
        &self,
        range: (Grapheme, Grapheme),
        start_inclusive: bool,
        end_inclusive: bool,
    ) -> bool {
        self.reveal_ranges().any(|rr| {
            range.contains(rr.start(), start_inclusive, end_inclusive)
                || range.contains(rr.end(), start_inclusive, end_inclusive)
        })
    }
}

impl<'ast> MdDoc {
    /// Whether `node` contributes an indent-width gutter column —
    /// these are the containers a drag selects an indentation unit of.
    /// `List`/`Table`/`TableRow`/`Document` nest without shifting, so
    /// they own no column.
    pub fn is_gutter_level(&self, node: &'ast AstNode<'ast>) -> bool {
        matches!(
            node.data.borrow().value,
            NodeValue::Item(_)
                | NodeValue::TaskItem(_)
                | NodeValue::BlockQuote
                | NodeValue::Alert(_)
                | NodeValue::FootnoteDefinition(_)
        )
    }

    /// A gutter level whose marker lives only on its first line; later
    /// lines carry pure indentation. Such a level must reveal per-line
    /// (see [`Self::reveal_line`]) so cursoring into a continuation line's
    /// indentation doesn't also reveal the first-line marker. Block quotes
    /// and alerts mark every line, so they reveal node-wide.
    pub fn marks_first_line_only(&self, node: &'ast AstNode<'ast>) -> bool {
        matches!(
            node.data.borrow().value,
            NodeValue::Item(_) | NodeValue::TaskItem(_) | NodeValue::FootnoteDefinition(_)
        )
    }

    /// Whether the gutter column `level` owns on `line` should render its
    /// raw source. First-line-marker levels scope to the line; block
    /// quotes/alerts reveal node-wide.
    pub fn reveal_gutter_column(
        &self,
        level: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> bool {
        if self.marks_first_line_only(level) {
            self.reveal_line(level, line)
        } else {
            self.reveal(level)
        }
    }

    /// The nearest gutter-contributing ancestor of `node` (excluding
    /// `node` itself), i.e. the level whose column sits one to the left.
    pub fn gutter_parent(&self, node: &'ast AstNode<'ast>) -> Option<&'ast AstNode<'ast>> {
        node.ancestors().skip(1).find(|n| self.is_gutter_level(n))
    }

    /// The innermost gutter level on `line` (the deepest column), or
    /// `None` if the line has no gutter. This is the level whose
    /// `line_own_prefix` absorbs any indentation past the levels' own
    /// columns (a straddling tab's residual, over-indentation) so the
    /// columns tile the prefix.
    pub fn deepest_gutter_level(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<&'ast AstNode<'ast>> {
        let root = node.ancestors().last()?;
        let deepest = self.deepest_container_block_at_offset(root, line.end());
        deepest.ancestors().find(|n| self.is_gutter_level(n))
    }

    /// Fills `map[line_idx]` with the deepest gutter container covering
    /// each source line, via a single pre-order DFS: a deeper container is
    /// visited after its ancestor, so it overwrites the ancestor on any
    /// line they share (compact nesting).
    pub fn assign_deepest_gutter(
        &self,
        node: &'ast AstNode<'ast>,
        map: &mut [Option<&'ast AstNode<'ast>>],
    ) {
        for child in node.children() {
            if !child.is_container_block() {
                continue;
            }
            if self.is_gutter_level(child) {
                for i in self.node_lines(child).iter() {
                    if let Some(slot) = map.get_mut(i) {
                        *slot = Some(child);
                    }
                }
            }
            self.assign_deepest_gutter(child, map);
        }
    }

    /// returns true if the syntax for a container block should be revealed
    ///
    /// Reveal `node` when a reveal-range endpoint lands strictly inside a
    /// gutter column `node` *owns the source for*. Columns render as
    /// atomic fragments, so a drag snaps to their edges and never reaches
    /// an interior — only keyboard navigation or edits do, so a
    /// click-drag never reveals syntax.
    ///
    /// An endpoint can only reveal the column it sits in, so the scan is
    /// over the (typically 0–2) reveal endpoints, one line each.
    pub fn reveal(&self, node: &'ast AstNode<'ast>) -> bool {
        for reveal_range in self.reveal_ranges() {
            for endpoint in [reveal_range.start(), reveal_range.end()] {
                let (line_idx, _) = self
                    .bounds
                    .source_lines
                    .find_containing(endpoint, true, true);
                let Some(&line) = self.bounds.source_lines.get(line_idx) else {
                    continue;
                };
                if !self.node_contains_line(node, line) {
                    continue;
                }
                if self.reveal_line(node, line) {
                    return true;
                }
            }
        }
        false
    }

    /// Like [`Self::reveal`] but scoped to a single `line`: true when a reveal
    /// endpoint lands strictly inside the gutter column `node` owns on
    /// `line` — marker *or* leading indentation.
    ///
    /// An indentation-only own-prefix (a nested continuation line) renders
    /// collapsed to a single indent-wide column, so its interior cursor
    /// positions can't be distinguished until it reveals into per-grapheme
    /// source. Strict interiority keeps level-boundary edges (and drag,
    /// which snaps to edges) non-revealing.
    ///
    /// Line scoping matters for first-line-marker containers (items, task
    /// items, footnotes): the cursor inside a continuation line's
    /// indentation must reveal that indentation without also revealing the
    /// node's marker, which lives on a different line.
    pub fn reveal_line(&self, node: &'ast AstNode<'ast>, line: (Grapheme, Grapheme)) -> bool {
        let own = self.line_own_prefix(node, line);
        if own.is_empty() {
            return false;
        }
        self.reveal_ranges().any(|rr| {
            [rr.start(), rr.end()]
                .into_iter()
                .any(|ep| own.contains(ep, false, false))
        })
    }
}

impl<'ast> MdDoc {
    /// Reveal the whole table as source only when the cursor is on the
    /// delimiter row — it has no AST node, so per-row reveal can't
    /// surface it. Cursor inside a cell (boundaries included) and
    /// cursor in pipe gutters are handled by `reveal_table_row`.
    pub fn reveal_table(&self, node: &'ast AstNode<'ast>) -> bool {
        let delimiter_row_line_idx = self.node_first_line_idx(node) + 1;
        let Some(&delimiter_line) = self.bounds.source_lines.get(delimiter_row_line_idx) else {
            return false;
        };
        let node_line = self.node_line(node, delimiter_line);
        self.range_revealed(node_line, true)
    }
}

impl<'ast> MdDoc {
    /// Reveal the table row's source when the cursor sits between
    /// cells or after the last cell. Reads `reveal_selection` rather
    /// than `buffer.current.selection` so unfocused frames see stable
    /// layout.
    pub fn reveal_table_row(&self, node: &'ast AstNode<'ast>) -> bool {
        let row_range = self.node_range(node);
        let line = self.node_first_line(node);
        let node_line = self.node_line(node, line);

        // check for cursor between cells
        let mut range_start = row_range.start();
        for cell in node.children() {
            let cell_range = if let Some((_, _, cell_children_range, _, _)) =
                self.split_range(cell, node_line)
            {
                cell_children_range
            } else {
                self.node_range(cell)
            };

            let between_range = (range_start, cell_range.start());

            if self.range_contains_revealed(between_range, false, false) {
                return true;
            }

            range_start = cell_range.end();
        }

        // check for cursor after last cell
        if let Some(cell) = node.children().last() {
            let cell_range = if let Some((_, _, cell_children_range, _, _)) =
                self.split_range(cell, node_line)
            {
                cell_children_range
            } else {
                self.node_range(cell)
            };

            let between_range = (cell_range.end(), row_range.end());
            if self.range_contains_revealed(between_range, false, false) {
                return true;
            }
        }

        false
    }
}

impl<'ast> MdDoc {
    pub fn reveal_fenced_code_block(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
    ) -> bool {
        let first_line_idx = self.node_first_line_idx(node);
        let last_line_idx = self.node_last_line_idx(node);

        if first_line_idx == last_line_idx {
            return true; // only opening fence: always reveal
        }
        if first_line_idx + 1 == last_line_idx
            && self.is_closing_fence(
                node,
                node_code_block,
                self.bounds.source_lines[last_line_idx],
            )
        {
            return true; // only opening + closing fence: always reveal
        }

        // Reveal when a reveal range touches any of the block's source
        // lines, *including* a container prefix (`> `): a cursor on the
        // prefix of an otherwise-collapsed fence line would otherwise have
        // no fragment, since the fence line isn't rendered.
        self.node_lines(node).iter().any(|line_idx| {
            let line = self.bounds.source_lines[line_idx];
            self.range_revealed(line, true)
        })
    }

    pub fn reveal_indented_code_block(&self, node: &'ast AstNode<'ast>, synthetic: bool) -> bool {
        let mut reveal = false;
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            // reveal if selection is inside the indentation, but not at the
            // indentation's end / content's start
            let indentation = (
                node_line.start(),
                node_line
                    .end()
                    .min(node_line.start() + if synthetic { 0 } else { 4 }),
            );
            if self.range_revealed(indentation, false)
                || self
                    .reveal_ranges()
                    .any(|rr| rr.contains(indentation.start(), true, true))
            {
                reveal = true;
                break;
            }
        }
        reveal
    }
}

impl<'ast> MdDoc {
    pub fn reveal_setext_syntax(&self, node: &'ast AstNode<'ast>) -> bool {
        // reveal syntax even if the cursor is in the indentation before the node
        for line in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line];

            if self.range_revealed(line, true) {
                return true;
            }
        }
        false
    }

    /// Returns true if the heading contents should be revealed whether the heading is folded or not
    pub fn heading_fold_reveal(&self, node: &'ast AstNode<'ast>) -> bool {
        self.range_contains_fold_revealed(self.heading_contents(node), false, true)
    }
}

impl<'ast> MdDoc {
    /// Returns true if the item contents should be revealed whether the item is folded or not
    pub fn item_fold_reveal(&self, node: &'ast AstNode<'ast>) -> bool {
        self.range_contains_fold_revealed(self.item_contents(node), false, true)
    }
}
