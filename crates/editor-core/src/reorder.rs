use comrak::nodes::{AstNode, NodeValue};

use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMovePlan {
    pub run_range: (Grapheme, Grapheme),
    pub new_run: String,
}

impl<'ast> MdDoc {
    /// True iff `node` is an `Item`/`TaskItem` with ≥1 sibling item.
    pub fn is_reorderable_item(&self, node: &'ast AstNode<'ast>) -> bool {
        if !matches!(
            node.data.borrow().value,
            NodeValue::Item(_) | NodeValue::TaskItem(_)
        ) {
            return false;
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        parent
            .children()
            .filter(|c| {
                matches!(
                    c.data.borrow().value,
                    NodeValue::Item(_) | NodeValue::TaskItem(_)
                )
            })
            .nth(1)
            .is_some()
    }

    /// Union of the selected sibling items including `node`, or just
    /// `node`'s range when no multi-item selection is active.
    pub fn drag_span(&self, node: &'ast AstNode<'ast>) -> (Grapheme, Grapheme) {
        let own = self.node_range(node);
        if !self.selected_block(node) {
            return own;
        }
        let Some(parent) = node.parent() else {
            return own;
        };
        let mut span = own;
        for c in parent.children() {
            let is_item = matches!(
                c.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            );
            if is_item && self.selected_block(c) {
                let r = self.node_range(c);
                span = (span.0.min(r.0), span.1.max(r.1));
            }
        }
        span
    }

    /// Plan a sibling reorder as a positional permutation: items are
    /// permuted, separator gaps `Gi` stay in their slots. Same-depth
    /// siblings share a line prefix, so item text moves verbatim.
    /// `None` for a no-op or unlocatable item.
    pub fn plan_block_move(
        &self,
        root: &'ast AstNode<'ast>,
        section_range: (Grapheme, Grapheme),
        insert_offset: Grapheme,
    ) -> Option<BlockMovePlan> {
        let dragged = root.descendants().find(|n| {
            matches!(
                n.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            ) && {
                let r = self.node_range(n);
                r.start() == section_range.start() && r.end() <= section_range.end()
            }
        })?;
        let parent = dragged.parent()?;

        let mut units: Vec<(Grapheme, Grapheme)> = parent
            .children()
            .filter(|c| {
                matches!(
                    c.data.borrow().value,
                    NodeValue::Item(_) | NodeValue::TaskItem(_)
                )
            })
            .map(|c| self.node_range(c))
            .collect();
        units.sort_by_key(|s| s.start());
        if units.len() < 2 {
            return None;
        }

        let lo = units
            .iter()
            .position(|u| u.start() >= section_range.start())?;
        let hi = units.iter().rposition(|u| u.end() <= section_range.end())?;
        if hi < lo {
            return None;
        }
        let to = units.iter().filter(|u| u.start() < insert_offset).count();

        let lines = &self.bounds.source_lines;
        let block_range = |u: (Grapheme, Grapheme)| {
            let fi = self.line_idx_for_offset(u.start());
            let li = self.last_line_idx_for_end(u.end());
            (lines[fi].start(), lines[li].end())
        };
        let branges: Vec<(Grapheme, Grapheme)> = units.iter().map(|u| block_range(*u)).collect();
        let blocks: Vec<String> = branges
            .iter()
            .map(|b| self.buffer[*b].to_string())
            .collect();
        let gaps: Vec<String> = (0..branges.len() - 1)
            .map(|i| self.buffer[(branges[i].1, branges[i + 1].0)].to_string())
            .collect();
        let run_range = (branges[0].0, branges[branges.len() - 1].1);

        // Collapse the selected run into a single super-item (carrying
        // its internal gaps) so it permutes as one unit.
        let mut merged = String::new();
        for i in lo..=hi {
            merged.push_str(&blocks[i]);
            if i < hi {
                merged.push_str(&gaps[i]);
            }
        }
        let mut rblocks: Vec<String> = Vec::new();
        rblocks.extend(blocks[..lo].iter().cloned());
        rblocks.push(merged);
        rblocks.extend(blocks[hi + 1..].iter().cloned());
        let mut rgaps: Vec<String> = Vec::new();
        rgaps.extend(gaps[..lo].iter().cloned());
        rgaps.extend(gaps[hi..].iter().cloned());

        // Destination in reduced-list space, then post-removal index.
        let to_reduced = if to <= lo { to } else { to - (hi - lo) };
        let to_adj = if to_reduced > lo {
            to_reduced - 1
        } else {
            to_reduced
        };
        if to_adj == lo {
            return None;
        }

        let moved = rblocks.remove(lo);
        rblocks.insert(to_adj, moved);

        let mut new_run = String::new();
        for i in 0..rblocks.len() {
            new_run.push_str(&rblocks[i]);
            if i < rgaps.len() {
                new_run.push_str(&rgaps[i]);
            }
        }

        Some(BlockMovePlan { run_range, new_run })
    }

    pub fn line_idx_for_offset(&self, offset: Grapheme) -> usize {
        let lines = &self.bounds.source_lines;
        lines
            .iter()
            .position(|l| l.contains(offset, true, true))
            .unwrap_or(lines.len().saturating_sub(1))
    }

    /// Source-line index for an exclusive range end. An end on a line
    /// boundary belongs to the preceding line.
    pub fn last_line_idx_for_end(&self, end: Grapheme) -> usize {
        let lines = &self.bounds.source_lines;
        for i in (0..lines.len()).rev() {
            let l = lines[i];
            if l.end() == end || l.contains(end, true, false) {
                return i;
            }
        }
        self.line_idx_for_offset(end)
    }
}
