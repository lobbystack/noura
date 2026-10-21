//! Block structure: each block's source lines, the prefixes container
//! blocks own on each line (list markers, quote markers, indentation), the
//! prefixes Return and Tab insert, folding, and the bounds that blocks
//! contribute.

use std::ops::Range;

use comrak::nodes::{AstNode, NodeAlert, NodeCodeBlock, NodeList, NodeValue};

use crate::bounds::RangesExt as _;
use crate::doc::MdDoc;
use crate::fold::FOLD_TAG;
use crate::input::Event;
use crate::nodes::{
    NodeValueExt as _, consume_indent_columns, consume_indent_columns_ceil,
    leading_indent_cols_ceil,
};
use crate::offset_types::{
    Grapheme, Graphemes, IntoRangeExt as _, RangeExt as _, RangeIterExt as _,
};

// Fast integer mapping for NodeValue variants - no hashing needed
fn node_value_to_discriminant_id(value: &NodeValue) -> u8 {
    match value {
        NodeValue::FrontMatter(_) => 1,
        NodeValue::Raw(_) => 2,
        NodeValue::Alert(_) => 3,
        NodeValue::BlockQuote => 4,
        NodeValue::DescriptionItem(_) => 5,
        NodeValue::DescriptionList => 6,
        NodeValue::Document => 7,
        NodeValue::FootnoteDefinition(_) => 8,
        NodeValue::Item(_) => 9,
        NodeValue::List(_) => 10,
        NodeValue::MultilineBlockQuote(_) => 11,
        NodeValue::Table(_) => 12,
        NodeValue::TableRow(_) => 13,
        NodeValue::TaskItem(_) => 14,
        NodeValue::Image(_) => 15,
        NodeValue::Code(_) => 16,
        NodeValue::Emph => 17,
        NodeValue::Escaped => 18,
        NodeValue::EscapedTag(_) => 19,
        NodeValue::FootnoteReference(_) => 20,
        NodeValue::HtmlInline(_) => 21,
        NodeValue::LineBreak => 22,
        NodeValue::Link(_) => 23,
        NodeValue::Math(_) => 24,
        NodeValue::SoftBreak => 25,
        NodeValue::SpoileredText => 26,
        NodeValue::Strikethrough => 27,
        NodeValue::Strong => 28,
        NodeValue::Subscript => 29,
        NodeValue::Superscript => 30,
        NodeValue::Text(_) => 31,
        NodeValue::Underline => 32,
        NodeValue::WikiLink(_) => 33,
        NodeValue::CodeBlock(_) => 34,
        NodeValue::DescriptionDetails => 35,
        NodeValue::DescriptionTerm => 36,
        NodeValue::Heading(_) => 37,
        NodeValue::HtmlBlock(_) => 38,
        NodeValue::Paragraph => 39,
        NodeValue::TableCell => 40,
        NodeValue::ThematicBreak => 41,
        NodeValue::Highlight => 42,
        NodeValue::ShortCode(_) => 43,
        NodeValue::Subtext => 44,
    }
}

impl<'ast> MdDoc {
    /// Returns true if the given block node is selected for the purposes of
    /// rich editing. All selected nodes are siblings at any given time.
    ///
    /// Containers normally yield to a more specific descendant (the cursor
    /// inside `* a`'s paragraph picks the paragraph, not the item).
    /// Containers that *fully contain* the selection are picked only when
    /// no proper descendant block intersects — the empty-container case
    /// (`* ` with no body), where there's nothing deeper to target.
    pub fn selected_block(&self, node: &'ast AstNode<'ast>) -> bool {
        // the document is never selected
        let Some(parent) = node.parent() else {
            return false;
        };
        if !self.node_intersects_selection(node) || !self.node_contains_selection(parent) {
            return false;
        }
        if node.is_leaf_block() {
            return true;
        }
        if !node.is_container_block() {
            return false;
        }
        // multi-block selection that extends past this container, OR
        // empty container (deepest, no descendant blocks to target)
        !self.node_contains_selection(node)
            || !node.descendants().skip(1).any(|d| {
                (d.is_leaf_block() || d.is_container_block()) && self.node_intersects_selection(d)
            })
    }

    /// Returns the portion of the line that's within the node, excluding line
    /// prefixes due to parent nodes. For container blocks, this is equivalent
    /// to [`Self::line_own_prefix`] + [`Self::line_content`]. For leaf blocks, which have
    /// no prefix, this is equivalent to [`Self::line_content`].
    pub fn node_line(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> (Grapheme, Grapheme) {
        let Some(parent) = node.parent() else {
            return line;
        }; // document has no prefix
        let (parent_prefix_len, _) = self.line_prefix_len(parent, line);

        (line.start() + parent_prefix_len, line.end())
    }

    /// Returns the (inclusive, exclusive) range of lines that this node is sourced from.
    pub fn node_lines(&self, node: &'ast AstNode<'ast>) -> (usize, usize) {
        self.range_lines(self.node_range(node))
    }

    /// Returns the line index of the first line of a node
    pub fn node_first_line_idx(&self, node: &'ast AstNode<'ast>) -> usize {
        self.bounds
            .source_lines
            .find_containing(self.node_range(node).start(), true, true)
            .start()
    }

    /// Returns the line index of the last line of a node
    pub fn node_last_line_idx(&self, node: &'ast AstNode<'ast>) -> usize {
        self.bounds
            .source_lines
            .find_containing(self.node_range(node).end(), true, true)
            .start()
    }

    /// Returns the first line, the whole first line, and nothing but the first
    /// line of the given node.
    pub fn node_first_line(&self, node: &'ast AstNode<'ast>) -> (Grapheme, Grapheme) {
        self.bounds.source_lines[self.node_first_line_idx(node)]
    }

    /// Returns the last line, the whole last line, and nothing but the last line
    /// of the given node.
    pub fn node_last_line(&self, node: &'ast AstNode<'ast>) -> (Grapheme, Grapheme) {
        self.bounds.source_lines[self.node_last_line_idx(node)]
    }

    /// Returns the node that should have a fold node appended to make this node folded, if there is one
    pub fn foldable(&self, node: &'ast AstNode<'ast>) -> Option<&'ast AstNode<'ast>> {
        // must not be the first block in a list or in a block quote, since then
        // we have no space to render the fold button (matches Bear)
        if let Some(parent) = node.parent() {
            if matches!(parent.data.borrow().value, NodeValue::List(_)) {
                if let Some(grandparent) = parent.parent() {
                    if matches!(grandparent.data.borrow().value, NodeValue::Item(_))
                        && self.node_first_line_idx(node) == self.node_first_line_idx(grandparent)
                    {
                        return None;
                    }
                    if matches!(grandparent.data.borrow().value, NodeValue::BlockQuote) {
                        return None;
                    }
                }
            } else {
                if matches!(parent.data.borrow().value, NodeValue::Item(_))
                    && self.node_first_line_idx(node) == self.node_first_line_idx(parent)
                {
                    return None;
                }
                if matches!(parent.data.borrow().value, NodeValue::BlockQuote) {
                    return None;
                }
            }
        }

        // list items
        if matches!(
            node.data.borrow().value,
            NodeValue::Item(_) | NodeValue::TaskItem(_)
        ) {
            // must have paragraph to add fold html tag + something to fold
            if node.children().count() < 2 {
                return None;
            }

            if let Some(first_child_block) = node.first_child()
                && first_child_block.data.borrow().value == NodeValue::Paragraph
            {
                return Some(first_child_block);
            }
        }

        // headings
        if let NodeValue::Heading(heading) = node.data.borrow().value {
            // must have something to fold
            {
                let next_sibling = node.next_sibling()?;
                if let NodeValue::Heading(next_heading) = next_sibling.data.borrow().value
                    && next_heading.level <= heading.level
                {
                    return None;
                }
            }

            return Some(node);
        }

        None
    }

    /// Returns the fold node - the node that is causing this node to be folded - if there is one
    pub fn fold(&self, node: &'ast AstNode<'ast>) -> Option<&'ast AstNode<'ast>> {
        if let Some(foldable) = self.foldable(node) {
            for inline in foldable.children() {
                if let NodeValue::HtmlInline(html) = &inline.data.borrow().value
                    && html == FOLD_TAG
                {
                    return Some(inline);
                }
            }
        }
        None
    }

    #[allow(clippy::collapsible_else_if)]
    pub fn apply_fold(
        &mut self,
        node: &'ast AstNode<'ast>,
        contents: (Grapheme, Grapheme),
        unapply: bool,
    ) {
        if unapply {
            if let Some(fold) = self.fold(node) {
                self.deferred_events.push(Event::Replace {
                    region: self.node_range(fold).into(),
                    text: "".into(),
                    advance_cursor: false,
                });
            }
        } else {
            if let Some(foldable) = self.foldable(node) {
                self.deferred_events.push(Event::Replace {
                    region: self.node_range(foldable).end().into_range().into(),
                    text: FOLD_TAG.into(),
                    advance_cursor: false,
                });

                // when folding a section that intersects the cursor, adjust the selection
                // this ensures the folded section appears folded / avoids immediate selection reveal
                let selection = self.buffer.current.selection;

                if contents.intersects(&selection, true)
                    && !selection.contains_range(&contents, true, true)
                {
                    self.deferred_events.push(Event::Select {
                        region: (
                            selection.start().min(contents.start()),
                            selection.end().min(contents.start()),
                        )
                            .into(),
                    });
                }
            }
        }
    }

    pub fn hidden_by_fold(&self, node: &'ast AstNode<'ast>) -> bool {
        self.get_cached_hidden_by_fold(node)
            .expect("hidden_by_fold queried for a node not in the current AST")
    }

    /// One DFS over the tree. Carries `item_fold_active` down through
    /// the recursion (any folded-unrevealed Item/TaskItem ancestor
    /// turns it on, hiding all but the first child). Per parent,
    /// maintains a stack of currently-open headings as we walk
    /// children left-to-right; each entry is `(level, folded_unrevealed)`.
    pub fn populate_hidden_by_fold(&self, root: &'ast AstNode<'ast>) {
        let deps = self.reveal_seq;
        if self.cache.hidden_by_fold_deps.get() == Some(deps) {
            return;
        }
        self.cache.hidden_by_fold_deps.set(Some(deps));

        // The root is never hidden; cache it directly so a query for
        // `hidden_by_fold(root)` doesn't fall back to the unwrap
        // default.
        self.set_cached_hidden_by_fold(root, false);
        self.populate_hidden_by_fold_subtree(root, false);
    }

    pub fn populate_hidden_by_fold_subtree(
        &self,
        parent: &'ast AstNode<'ast>,
        item_fold_active: bool,
    ) {
        // Per-parent state. Stack invariant: levels strictly
        // increasing from bottom to top.
        let mut heading_stack: Vec<(u8, bool)> = Vec::new();
        let mut sibling_index: usize = 0;
        let mut child = parent.first_child();
        while let Some(c) = child {
            // Pull only the bits we need out of the borrow — copying
            // the heading level and the variant kind avoids cloning
            // the whole `NodeValue` (which carries owned `String`s
            // for inline payloads).
            let (heading_level, is_item_or_task) = {
                let value = &c.data.borrow().value;
                (
                    if let NodeValue::Heading(h) = value {
                        Some(h.level)
                    } else {
                        None
                    },
                    matches!(value, NodeValue::Item(_) | NodeValue::TaskItem(_)),
                )
            };

            // Headings end prior headings at >= their level — pop
            // before the visibility check so the popped headings
            // don't count as containers of this heading.
            if let Some(level) = heading_level {
                while heading_stack.last().is_some_and(|&(lvl, _)| lvl >= level) {
                    heading_stack.pop();
                }
            }

            let hidden_by_item = sibling_index > 0 && item_fold_active;
            // Any folded heading currently on the stack hides us.
            let hidden_by_heading = heading_stack.iter().any(|&(_, folded)| folded);
            self.set_cached_hidden_by_fold(c, hidden_by_item || hidden_by_heading);

            // Push self onto the heading stack so subsequent siblings
            // know we exist as a section container. Even unfolded
            // headings get pushed — they "block" prior less-significant
            // folded headings from contributing past us.
            if let Some(level) = heading_level {
                let folded = self.fold(c).is_some() && !self.heading_fold_reveal(c);
                heading_stack.push((level, folded));
            }

            let child_item_fold_active = item_fold_active
                || (is_item_or_task && self.fold(c).is_some() && !self.item_fold_reveal(c));
            self.populate_hidden_by_fold_subtree(c, child_item_fold_active);

            child = c.next_sibling();
            sibling_index += 1;
        }
    }

    pub fn get_cached_line_prefix_len(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<(Graphemes, bool)> {
        let node_key_hash = Self::pack_node_key(node);
        self.cache
            .line_prefix_len
            .borrow()
            .get(&(node_key_hash, line))
            .copied()
    }

    pub fn set_cached_line_prefix_len(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
        value: (Graphemes, bool),
    ) {
        let node_key_hash = Self::pack_node_key(node);
        self.cache
            .line_prefix_len
            .borrow_mut()
            .insert((node_key_hash, line), value);
    }

    #[inline]
    pub fn get_cached_node_range(&self, node: &'ast AstNode<'ast>) -> Option<(Grapheme, Grapheme)> {
        let key_hash = Self::pack_node_key(node);
        self.cache.node_range.borrow().get(&key_hash).copied()
    }

    #[inline]
    pub fn set_cached_node_range(&self, node: &'ast AstNode<'ast>, range: (Grapheme, Grapheme)) {
        let key_hash = Self::pack_node_key(node);
        self.cache.node_range.borrow_mut().insert(key_hash, range);
    }

    pub fn get_cached_hidden_by_fold(&self, node: &'ast AstNode<'ast>) -> Option<bool> {
        let key = Self::pack_node_key(node);
        self.cache.hidden_by_fold.borrow().get(&key).copied()
    }

    pub fn set_cached_hidden_by_fold(&self, node: &'ast AstNode<'ast>, hidden: bool) {
        let key = Self::pack_node_key(node);
        self.cache.hidden_by_fold.borrow_mut().insert(key, hidden);
    }

    /// Pack node info into u64 using bit manipulation - ultra fast cache key
    pub fn pack_node_key(node: &AstNode) -> u64 {
        let borrowed = node.data.borrow();
        let sp = borrowed.sourcepos;
        let (start_line, start_column, end_line, end_column, discriminant) = (
            sp.start.line as u64,
            sp.start.column as u64,
            sp.end.line as u64,
            sp.end.column as u64,
            node_value_to_discriminant_id(&borrowed.value) as u64,
        );
        ((start_line & 0x3FFF) << 50)
            | ((start_column & 0x3FFF) << 36)
            | ((end_line & 0x3FFF) << 22)
            | ((end_column & 0x3FFF) << 8)
            | (discriminant & 0xFF)
    }
}

impl<'ast> MdDoc {
    /// Converts an optional source line index range to a doc char offset range.
    pub fn spacing_range(&self, line_range: &Option<Range<usize>>) -> (Grapheme, Grapheme) {
        match line_range {
            Some(r) if !r.is_empty() => {
                let start = self.bounds.source_lines[r.start].start();
                let end = self.bounds.source_lines[r.end - 1].end();
                (start, end)
            }
            _ => (Grapheme(usize::MAX), Grapheme(0)),
        }
    }

    /// Returns the source line index range for the pre-spacing of `node`, i.e.
    /// the empty lines between the previous sibling (or parent start) and this
    /// node. Returns `None` when there is no pre-spacing (document root, table
    /// rows, folded nodes).
    pub fn pre_spacing_lines(&self, node: &'ast AstNode<'ast>) -> Option<Range<usize>> {
        let parent = node.parent()?;
        if matches!(node.data.borrow().value, NodeValue::TableRow(_)) {
            return None;
        }
        if self.hidden_by_fold(node) {
            return None;
        }

        let first = match node.previous_sibling() {
            None => {
                let mut first = self.node_first_line_idx(parent);
                if matches!(&parent.data.borrow().value, NodeValue::Alert(_)) {
                    first += 1;
                }
                first
            }
            Some(prev) => self.node_last_line_idx(prev) + 1,
        };

        Some(first..self.node_first_line_idx(node))
    }

    /// Returns the source line index range for the post-spacing of `node`, i.e.
    /// the empty lines between this node and the parent's end. Only the last
    /// sibling has post-spacing; returns `None` otherwise.
    pub fn post_spacing_lines(&self, node: &'ast AstNode<'ast>) -> Option<Range<usize>> {
        let parent = node.parent()?;
        if matches!(node.data.borrow().value, NodeValue::TableRow(_)) {
            return None;
        }

        if node.next_sibling().is_some() {
            return None;
        }

        let first = self.node_last_line_idx(node) + 1;
        let last = self.node_last_line_idx(parent);
        Some(first..(last + 1))
    }

    pub fn compute_bounds_block_pre_spacing(&mut self, node: &'ast AstNode<'ast>) {
        let Some(line_range) = self.pre_spacing_lines(node) else {
            return;
        };

        for line_idx in line_range {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);
            self.bounds.inline_paragraphs.push(node_line);
        }
    }

    pub fn compute_bounds_block_post_spacing(&mut self, node: &'ast AstNode<'ast>) {
        let Some(line_range) = self.post_spacing_lines(node) else {
            return;
        };

        for line_idx in line_range {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);
            self.bounds.inline_paragraphs.push(node_line);
        }
    }
}

impl<'ast> MdDoc {
    /// Returns 5 ranges representing the pre-node range, pre-first-child section,
    /// inter-children section, post-last-child section, and post-node range.
    #[allow(clippy::type_complexity)]
    pub fn split_range(
        &self,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) -> Option<(
        (Grapheme, Grapheme),
        (Grapheme, Grapheme),
        (Grapheme, Grapheme),
        (Grapheme, Grapheme),
        (Grapheme, Grapheme),
    )> {
        let node_range = self.node_range(node);

        let first_child_start = node
            .descendants()
            .skip(1)
            .filter(|descendant| self.node_range(descendant).intersects(&range, false))
            .map(|descendant| self.node_range(descendant).start())
            .min()?;
        let last_child_end = node
            .descendants()
            .skip(1)
            .filter(|descendant| self.node_range(descendant).intersects(&range, false))
            .map(|descendant| self.node_range(descendant).end())
            .max()?;

        let pre_node = (range.start(), node_range.start());
        let pre_first_child = (node_range.start(), first_child_start);
        let inter_children = (first_child_start, last_child_end);
        let post_last_child = (last_child_end, node_range.end());
        let post_node = (node_range.end(), range.end());

        Some((
            pre_node.trim(&range),
            pre_first_child.trim(&range),
            inter_children.trim(&range),
            post_last_child.trim(&range),
            post_node.trim(&range),
        ))
    }

    /// Returns true if the node's lines intersect the selection. Differs from
    /// node_lines_intersect_selection in cases where the selection intersects
    /// optional indentation, trailing whitespace, or the portion of a node's
    /// lines that are due to container blocks.
    pub fn node_lines_intersect_selection(&self, node: &'ast AstNode<'ast>) -> bool {
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);
            if self.range_revealed(node_line, true) {
                return true;
            }
        }
        false
    }
}

impl<'ast> MdDoc {
    /// Compute the `(chunk_start, end)` slice of `node_line` that's the
    /// actual content of a code-block line (fence indent stripped for
    /// fenced; 4-col indent stripped for indented, combined with parent
    /// item padding if any).
    pub fn code_line_range(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        node_line: (Grapheme, Grapheme),
        synthetic: bool,
    ) -> (Grapheme, Grapheme) {
        let NodeCodeBlock {
            fenced,
            fence_offset,
            ..
        } = node_code_block;
        if *fenced {
            // "If the leading code fence is indented N spaces, then up to N spaces
            // of indentation are removed from each line of the content..."
            // https://github.github.com/gfm/#fenced-code-blocks
            let text = &self.buffer[node_line];
            let indentation = text
                .chars()
                .take_while(|&c| c == ' ')
                .count()
                .min(*fence_offset);
            (node_line.start() + indentation, node_line.end())
        } else {
            // "An indented code block ... minus four spaces of indentation."
            // https://github.github.com/gfm/#indented-code-blocks
            //
            // Strip column-aware. If inside a list item, combine with
            // the item's padding so a tab straddling the boundary
            // doesn't get attributed entirely to one side.
            let target_cols = if synthetic {
                0
            } else {
                let parent_item_padding = node
                    .ancestors()
                    .find_map(|a| match &a.data.borrow().value {
                        NodeValue::Item(nl) => Some(nl.padding),
                        _ => None,
                    })
                    .unwrap_or(0);
                parent_item_padding + 4
            };
            let text = &self.buffer[node_line];
            let strip_graphemes = consume_indent_columns(text, target_cols);
            let chunk_start = (node_line.start() + strip_graphemes).min(node_line.end());
            (chunk_start, node_line.end())
        }
    }

    pub fn is_closing_fence(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
    ) -> bool {
        let NodeCodeBlock {
            fence_char,
            fence_length,
            ..
        } = node_code_block;
        let fence_char = *fence_char as char;

        let node_line = self.node_line(node, line);

        let text = &self.buffer[node_line];
        let mut chars = text.chars().peekable();

        // Skip up to 3 leading spaces
        for _ in 0..3 {
            if chars.peek() == Some(&' ') {
                chars.next();
            } else {
                break;
            }
        }

        // Skip exactly fence_length fence_char's
        for _ in 0..*fence_length {
            if chars.peek() == Some(&fence_char) {
                chars.next();
            } else {
                return false;
            }
        }

        // Skip any number of additional fence_char's
        loop {
            if chars.peek() == Some(&fence_char) {
                chars.next();
            } else {
                break;
            }
        }

        // Any additional characters may be spaces only
        chars.all(|c| c == ' ')
    }
}

impl MdDoc {
    pub fn frontmatter_as_code_block() -> NodeCodeBlock {
        NodeCodeBlock {
            fenced: true,
            fence_char: b'-',
            fence_length: 3,
            fence_offset: 0,
            info: "yaml".to_string(),
            literal: String::new(),
            closed: true,
        }
    }
}

impl<'ast> MdDoc {
    pub fn compute_bounds_heading(&mut self, node: &'ast AstNode<'ast>, _level: u8, setext: bool) {
        if setext {
            self.compute_bounds_setext_heading(node)
        } else {
            self.compute_bounds_atx_heading(node)
        }
    }

    pub fn compute_bounds_setext_heading(&mut self, node: &'ast AstNode<'ast>) {
        let reveal = self.reveal_setext_syntax(node);
        let last_line_idx = self.node_last_line_idx(node);

        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];

            if line_idx < last_line_idx {
                // non-underline content
                self.compute_bounds_setext_heading_line(node, line, reveal);
            }
        }
    }

    pub fn compute_bounds_setext_heading_line(
        &mut self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
        reveal: bool,
    ) {
        let node_line = self.node_line(node, line);

        if reveal {
            let Some((_, _, children, postfix_whitespace, _)) = self.split_range(node, node_line)
            else {
                self.bounds.inline_paragraphs.push(node_line);
                return;
            };

            if !postfix_whitespace.is_empty() {
                self.bounds
                    .inline_paragraphs
                    .push((children.start(), postfix_whitespace.end()));
            } else {
                self.bounds.inline_paragraphs.push(children);
            }
        } else {
            let Some((_indentation, _prefix, children, _postfix_whitespace, _)) =
                self.split_range(node, node_line)
            else {
                self.bounds.inline_paragraphs.push(node_line);
                return;
            };

            self.bounds.inline_paragraphs.push(children);
        }
    }

    pub fn compute_bounds_atx_heading(&mut self, node: &'ast AstNode<'ast>) {
        let line = self.node_first_line(node);
        let node_line = self.node_line(node, line);

        if let Some((_, _, _infix_range, postfix_range, _)) = self.split_range(node, node_line) {
            if let Some(infix_range) = self.infix_range(node) {
                if !postfix_range.is_empty() {
                    self.bounds
                        .inline_paragraphs
                        .push((infix_range.start(), postfix_range.end()));
                } else {
                    self.bounds.inline_paragraphs.push(infix_range);
                }
            } else if !postfix_range.is_empty() {
                self.bounds.inline_paragraphs.push(postfix_range);
            }
        } else {
            // heading is empty - show the syntax regardless if cursored (Obsidian-inspired)
            self.bounds.inline_paragraphs.push(node_line);
        }
    }

    /// Contents end at the last contained sibling's last line: blank
    /// lines past that — before the next section or at doc end — render
    /// as visible spacing rows.
    pub fn heading_contents(&self, node: &'ast AstNode<'ast>) -> (Grapheme, Grapheme) {
        let NodeValue::Heading(heading) = &node.data.borrow().value else {
            panic!("heading_contents() invoked for non-heading")
        };

        let mut contents = self.node_range(node).end().into_range();
        let mut sibling = node.next_sibling();
        while let Some(s) = sibling {
            if let NodeValue::Heading(sib_h) = &s.data.borrow().value
                && sib_h.level <= heading.level
            {
                break;
            }
            contents.1 = self.node_last_line(s).end();
            sibling = s.next_sibling();
        }
        contents
    }
}

impl<'ast> MdDoc {
    pub fn compute_bounds_paragraph(&mut self, node: &'ast AstNode<'ast>) {
        for line in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line];
            let node_line = self.node_line(node, line);

            self.bounds.inline_paragraphs.push(node_line);
        }
    }
}

impl<'ast> MdDoc {
    pub fn compute_bounds_table_cell(&mut self, node: &'ast AstNode<'ast>) {
        let node_line = self.node_range(node); // table cells are always single-line

        self.bounds.inline_paragraphs.push(node_line);
    }
}

impl<'ast> MdDoc {
    pub fn own_prefix_len_alert(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<Graphemes> {
        self.own_prefix_len_block_quote(node, line)
    }

    pub fn compute_bounds_alert(&mut self, node: &'ast AstNode<'ast>, _node_alert: &NodeAlert) {
        // Handle children or remaining lines
        let first_line = self.node_first_line(node);
        let any_children = node.children().next().is_some();
        if any_children {
            self.compute_bounds_block_children(node);
        } else {
            for line in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line];
                let line_content = self.line_content(node, line);

                if line != first_line {
                    self.bounds.inline_paragraphs.push(line_content);
                }
            }
        }
    }

    pub fn alert_type_title_ranges(
        &self,
        node: &'ast AstNode<'ast>,
    ) -> ((Grapheme, Grapheme), (Grapheme, Grapheme)) {
        let line = self.node_first_line(node);
        let line_content = self.line_content(node, line);
        let line_content_text = &self.buffer[line_content].to_uppercase();

        let type_len = if line_content_text.starts_with("[!NOTE]") {
            "[!NOTE]".len()
        } else if line_content_text.starts_with("[!TIP]") {
            "[!TIP]".len()
        } else if line_content_text.starts_with("[!IMPORTANT]") {
            "[!IMPORTANT]".len()
        } else if line_content_text.starts_with("[!WARNING]") {
            "[!WARNING]".len()
        } else if line_content_text.starts_with("[!CAUTION]") {
            "[!CAUTION]".len()
        } else {
            // comrak found an alert whose first line the editor reads
            // elsewhere, as after frontmatter that holds a lone carriage
            // return: the type is empty and the whole line is the title.
            0
        };
        let _type = (line_content.start(), line_content.start() + type_len);

        // todo: empty title
        let untrimmed_title = (line_content.start() + type_len, line_content.end());
        let untrimmed_title_text = &self.buffer[untrimmed_title];
        let title = if untrimmed_title_text.trim().is_empty() {
            untrimmed_title
        } else {
            let title_leading_whitespace_len = untrimmed_title_text.chars().count()
                - untrimmed_title_text.trim_start().chars().count();
            let title_trailing_whitespace_len = untrimmed_title_text.chars().count()
                - untrimmed_title_text.trim_end().chars().count();
            (
                untrimmed_title.start() + title_leading_whitespace_len,
                untrimmed_title.end() - title_trailing_whitespace_len,
            )
        };

        (_type, title)
    }
}

impl<'ast> MdDoc {
    pub fn compute_bounds_block_quote(&mut self, node: &'ast AstNode<'ast>) {
        // Handle children or remaining lines
        let any_children = node.children().next().is_some();
        if any_children {
            self.compute_bounds_block_children(node);
        } else {
            for line_idx in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line_idx];
                let line_content = self.line_content(node, line);
                self.bounds.inline_paragraphs.push(line_content);
            }
        }
    }

    // This routine is standard-/reference-complexity, as the prefix len is
    // line-by-line (unlike list items) and block quotes contain blocks
    // including other block quotes. Most of the fundamental behavior with line
    // prefix lengths can be observed with block quotes alone.
    //
    // This implementation does benefit from the simplicity of the node - there
    // are only 8 cases.
    pub fn own_prefix_len_block_quote(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<Graphemes> {
        let node_line = self.node_line(node, line);
        let mut result = 0.into();

        // "A block quote marker consists of 0-3 spaces of initial indent, plus
        // (a) the character > together with a following space, or (b) a single
        // character > not followed by a space."
        //
        // "If a string of lines Ls constitute a sequence of blocks Bs, then the
        // result of prepending a block quote marker to the beginning of each
        // line in Ls is a block quote containing Bs."
        let text = &self.buffer[node_line];

        // Up to 3 columns of leading whitespace before `>`. Ceil so a tab
        // a parent level left straddling the boundary is claimed here
        // rather than leaking into content (comrak already decided this is
        // a blockquote; we just attribute the bytes). Leading whitespace is
        // ' ' or '\t' only (both ASCII), so grapheme count equals byte count.
        let lead = consume_indent_columns_ceil(text, 3);
        let after_lead = &text[lead..];
        if !after_lead.starts_with('>') {
            // "If a string of lines Ls constitute a block quote with contents Bs,
            // then the result of deleting the initial block quote marker from one
            // or more lines in which the next non-whitespace character after the
            // block quote marker is paragraph continuation text is a block quote
            // with Bs as its content."
            return None;
        }
        result += lead;
        result += 1; // the `>`
        // Optional single column of marker space — a literal space or a tab
        // (the tab grapheme is consumed even though it represents more than
        // 1 column, per CommonMark §2.2).
        let after_marker = &after_lead[1..];
        if after_marker.starts_with(' ') || after_marker.starts_with('\t') {
            result += 1;
        }

        Some(result)
    }
}

impl<'ast> MdDoc {
    pub fn compute_bounds_document(&mut self, node: &'ast AstNode<'ast>) {
        let any_children = node.children().next().is_some();
        if any_children {
            self.compute_bounds_block_children(node);
        } else {
            for line_idx in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line_idx];
                self.bounds.inline_paragraphs.push(line);
            }
        }
    }
}

impl<'ast> MdDoc {
    // A clean spec is not available because these are a GFM extension, so
    // assumptions are made with experimental verification:
    // * 0-3 spaces indentation
    // * space between the syntax and first child are part of this
    // node (e.g. they don't affect indentation requirements for nested list
    // items)
    pub fn own_prefix_len_footnote_definition(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<Graphemes> {
        // todo: change paramater type of `line` to `usize` (index instead of
        // value) here and elsewhere
        // todo: unsure of lazy continuation line behavior in footnote
        // definitions
        Some(if line == self.node_first_line(node) {
            self.prefix_range(node).unwrap().len()
        } else {
            Graphemes(2).min(line.len())
        })
    }

    pub fn compute_bounds_footnote_definition(&mut self, node: &'ast AstNode<'ast>) {
        self.compute_bounds_block_children(node);
    }

    /// Footnote definitions are usually rendered in the order in which they're
    /// referenced, rather than the order in which they're written ('source
    /// order'). From where I'm sitting, I just don't see how that works in an
    /// interactive editor, even though I'm quite committed to rendering to
    /// spec, so we render them in source order.
    ///
    /// One aspect of footnote rendering we can support, though, is rendering
    /// footnote references and the labels for definitions as a number
    /// representing the order that they're referenced. So, the first
    /// _reference_ will always be rendered with a superscript '1', the next
    /// with a '2', etc, regardless the order the _definitions_ are written.
    /// This creates a unique editing experience because the rendered reference
    /// number is not from the source text; it changes to the reference text
    /// only when you select it.
    ///
    /// In the AST, it turns out, the nodes are not presented in source order
    /// when it comes to footnote definitions. Instead, they're presented in
    /// reference order. At first, I
    /// [thought](https://github.com/kivikakk/comrak/issues/554) this a bug, but
    /// it's intended behavior. This is why `Self::sorted_siblings` exists.
    /// Anyway, we leverage this behavior to determine the number we should
    /// render for the definition, since the node itself does not contain it.
    pub fn definition_number(&self, node: &'ast AstNode<'ast>) -> usize {
        let mut result = 0;
        let document = node.ancestors().last().expect("There is always a document");
        for descendant in document.descendants() {
            if matches!(
                descendant.data.borrow().value,
                NodeValue::FootnoteDefinition(_)
            ) {
                result += 1;
            }
            if descendant.data.borrow().sourcepos == node.data.borrow().sourcepos {
                return result;
            }
        }
        unreachable!("All nodes are somewhere in the document");
    }
}

impl<'ast> MdDoc {
    pub fn own_prefix_len_item(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
        node_list: &NodeList,
    ) -> Option<Graphemes> {
        let node_line = self.node_line(node, line);
        let mut result: Graphemes = 0.into();

        // "If a sequence of lines Ls constitutes a list item according to rule
        // #1, #2, or #3, then the result of indenting each line of Ls by 1-3
        // spaces (the same for each line) also constitutes a list item with the
        // same contents and attributes."
        let first_node_line = self.node_line(node, self.node_first_line(node));
        let first_text = &self.buffer[first_node_line];

        // 1-3 cols of relative indent before the marker; ceil claims a
        // straddling tab as indent, not content. `indentation` (graphemes)
        // advances the first-line prefix; `indent_cols` sizes continuations.
        let indentation = consume_indent_columns_ceil(first_text, 3);
        let indent_cols = leading_indent_cols_ceil(first_text, 3);
        let NodeList {
            padding: marker_width_including_spaces,
            ..
        } = *node_list;
        if line == self.node_first_line(node) {
            result += indentation;

            // "If a sequence of lines Ls constitute a sequence of blocks Bs starting
            // with a non-whitespace character, and M is a list marker of width W
            // followed by 1 ≤ N ≤ 4 spaces, then the result of prepending M and the
            // following spaces to the first line of Ls, and indenting subsequent lines
            // of Ls by W + N spaces, is a list item with Bs as its contents."
            //
            // "If a sequence of lines Ls starting with a single blank line
            // constitute a (possibly empty) sequence of blocks Bs, not separated
            // from each other by more than one blank line, and M is a list marker
            // of width W, then the result of prepending M to the first line of Ls,
            // and indenting subsequent lines of Ls by W + 1 spaces, is a list item
            // with Bs as its contents."
            result += marker_width_including_spaces;
        } else {
            // "If a string of lines Ls constitute a list item with contents Bs, then
            // the result of deleting some or all of the indentation from one or
            // more lines in which the next non-whitespace character after the
            // indentation is paragraph continuation text is a list item with the
            // same contents and attributes."
            //
            // "If a line is empty, then it need not be indented."
            //
            // Two regimes: an indented-code-block child line defers
            // stripping to `code_block.rs` (combined item.padding+4
            // column-aware strip — needed so a tab straddling the
            // item/code boundary doesn't render differently from
            // 4 spaces). All other continuation lines strip leading
            // ws here. Child detection uses sourcepos (cheap) instead
            // of `node_range` (recursive).
            let line_1_based = self
                .bounds
                .source_lines
                .find_containing(line.start(), true, false)
                .start()
                + 1;
            let line_has_code_block_child = node.children().any(|c| match &c.data.borrow().value {
                NodeValue::CodeBlock(b) if !b.fenced => {
                    let sp = c.data.borrow().sourcepos;
                    sp.start.line <= line_1_based && line_1_based <= sp.end.line
                }
                _ => false,
            });
            let text = &self.buffer[node_line];
            if line_has_code_block_child {
                // Indented code block line — leave stripping to
                // `code_block.rs`.
            } else {
                // Per-level continuation indent. `has_deeper` tests raw
                // sourcepos, not `node_range`/`node_line`, which recurse
                // back into the `line_prefix_len` being computed here.
                let has_deeper = node.descendants().skip(1).any(|d| {
                    self.is_gutter_level(d) && {
                        let sp = d.data.borrow().sourcepos;
                        sp.start.line <= line_1_based && line_1_based <= sp.end.line
                    }
                });
                if has_deeper {
                    // Claim this item's content column (relative indent +
                    // marker) so the deeper level starts where content does;
                    // marker width alone under-claims when source indents
                    // wider than the marker. Ceil keeps a straddling tab.
                    result += consume_indent_columns_ceil(
                        text,
                        indent_cols + marker_width_including_spaces,
                    );
                } else {
                    // Deepest gutter item — take the rest so content
                    // isn't over-indented.
                    result += consume_indent_columns(text, usize::MAX);
                }
            }
        }

        // marker_width_including_spaces reports the width _with_ spaces even
        // when they're not present
        Some(result.min(node_line.len()))
    }

    pub fn compute_bounds_item(&mut self, node: &'ast AstNode<'ast>) {
        let any_children = node.children().next().is_some();
        if any_children {
            self.compute_bounds_block_children(node);
        } else {
            let line = self.node_first_line(node);
            let line_content = self.line_content(node, line);

            self.bounds.inline_paragraphs.push(line_content);
        }
    }

    pub fn item_contents(&self, node: &'ast AstNode<'ast>) -> (Grapheme, Grapheme) {
        // contents start at the end of the first child, which acts as a sort of section title
        // if no children, start at end of node first line
        let mut contents: (Grapheme, Grapheme) = if let Some(first_child) = node.children().next() {
            self.node_range(first_child).end().into_range()
        } else {
            self.node_first_line(node).end().into_range()
        };

        // Contents end at the last child's last line — the end of the
        // hidden subtree. Blank lines past that render as visible
        // spacing rows.
        if let Some(last_child) = node.children().last() {
            contents.1 = contents.1.max(self.node_last_line(last_child).end());
        }

        contents
    }

    /// Returns true if the item is selected for folding; specialized adaptation of self.selected_block()
    pub fn selected_fold_item(&self, node: &'ast AstNode<'ast>) -> bool {
        // any items selected -> those items selected for fold
        let root = node.ancestors().last().unwrap();
        for descendent in root.descendants() {
            if self.selected_block(descendent)
                && matches!(
                    descendent.data().value,
                    NodeValue::Item(_) | NodeValue::TaskItem(_)
                )
            {
                return self.selected_block(node);
            }
        }

        // else -> parent item of any selected items selected for fold
        node.first_child()
            .map(|c| self.selected_block(c))
            .unwrap_or_default()
    }
}

impl<'ast> MdDoc {
    pub fn source_line_range(&self, range: (Grapheme, Grapheme)) -> (Grapheme, Grapheme) {
        let first_line = self
            .bounds
            .source_lines
            .find_containing(range.start(), true, true)
            .0
            .saturating_sub(1);
        let last_line = self
            .bounds
            .source_lines
            .find_containing(range.end(), true, true)
            .1
            .min(self.bounds.source_lines.len() - 1);
        let start = self.bounds.source_lines[first_line].start();
        let end = self.bounds.source_lines[last_line].end();
        (start, end)
    }

    /// How many leading characters on the given line belong to the given node
    /// and its ancestors? For example, in "`> * p`", the line prefix len of the
    /// block quote is 2 and of the list item is 4. This function only processes
    /// container blocks, so the paragraph is not supported.
    ///
    /// This fn's second return value indicates whether the given line is a lazy
    /// continuation line of the given node. For example, in:
    /// ```md
    /// > block quote line one
    /// block quote line two
    /// ```
    /// the second line is a lazy continuation line of the block quote.
    // "It is tempting to think of this in terms of columns: the continuation
    // blocks must be indented at least to the column of the first
    // non-whitespace character after the list marker. However, that is not
    // quite right. The spaces after the list marker determine how much relative
    // indentation is needed. Which column this indentation reaches will depend
    // on how the list item is embedded in other constructions, as shown by this
    // example:
    //
    //    > > 1.  one
    // >>
    // >>     two
    //
    // Here two occurs in the same column as the list marker 1., but is actually
    // contained in the list item, because there is sufficient indentation after
    // the last containing blockquote marker."
    //
    // https://github.github.com/gfm/#list-items
    pub fn line_prefix_len(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> (Graphemes, bool) {
        if let Some(cached) = self.get_cached_line_prefix_len(node, line) {
            return cached;
        }

        let Some(parent) = node.parent() else {
            return (0.into(), false); // document has no prefix
        };
        let (parent_prefix_len, parent_lazy) = self.line_prefix_len(parent, line);
        if parent_lazy {
            // lazy continuation status is inherited
            return (parent_prefix_len, parent_lazy);
        }

        let value = &node.data.borrow().value;
        let sp = &node.data.borrow().sourcepos;
        let maybe_own_prefix_len = match value {
            NodeValue::FrontMatter(_) => unimplemented!("not a block: {} {:?}", sp, value),
            NodeValue::Raw(_) => unimplemented!("can only be created programmatically"),

            // container_block
            NodeValue::Alert(_) => self.own_prefix_len_alert(node, line),
            NodeValue::BlockQuote => self.own_prefix_len_block_quote(node, line),
            NodeValue::DescriptionItem(_) => unimplemented!("extension disabled"),
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => Some(0.into()),
            NodeValue::FootnoteDefinition(_) => self.own_prefix_len_footnote_definition(node, line),
            NodeValue::Item(node_list) => self.own_prefix_len_item(node, line, node_list),
            NodeValue::List(_) => Some(0.into()),
            NodeValue::MultilineBlockQuote(_) => unimplemented!("extension disabled"),
            NodeValue::Table(_) => Some(0.into()),
            NodeValue::TableRow(_) => Some(0.into()),
            NodeValue::TaskItem(_) => self.own_prefix_len_task_item(node, line),

            // inline
            NodeValue::Image(_)
            | NodeValue::Code(_)
            | NodeValue::Emph
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::Highlight
            | NodeValue::HtmlInline(_)
            | NodeValue::LineBreak
            | NodeValue::Link(_)
            | NodeValue::Math(_)
            | NodeValue::ShortCode(_)
            | NodeValue::SoftBreak
            | NodeValue::SpoileredText
            | NodeValue::Strikethrough
            | NodeValue::Strong
            | NodeValue::Subscript
            | NodeValue::Subtext
            | NodeValue::Superscript
            | NodeValue::Text(_)
            | NodeValue::Underline
            | NodeValue::WikiLink(_) => unreachable!("not a container block: {} {:?}", sp, value),

            // leaf_block
            NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => {
                unimplemented!("not a container block: {} {:?}", sp, value)
            }
        };
        let result = if let Some(own_prefix_len) = maybe_own_prefix_len {
            (parent_prefix_len + own_prefix_len, false)
        } else {
            (parent_prefix_len, true)
        };

        self.set_cached_line_prefix_len(node, line, result);
        result
    }

    /// Returns the range representing the portion of the line that belongs to
    /// the given node's ancestors; comes before [`Self::line_own_prefix`] which
    /// comes before [`Self::line_content`].
    ///
    /// In the following example:
    ///
    /// ```md
    /// > * quoted list item
    /// ```
    ///
    /// * For the list item:
    ///   * the line ancestors prefix is "`> `"
    ///   * the line own prefix is "`* `"
    ///   * the line content is "`quoted list item`"
    /// * For the block quote:
    ///   * the line ancestors prefix is ""
    ///   * the line own prefix is "`> `"
    ///   * the line content is "`* quoted list item`"
    pub fn line_ancestors_prefix(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> (Grapheme, Grapheme) {
        let Some(parent) = node.parent() else {
            return line.start().into_range();
        }; // document has no ancestors
        let (parent_prefix_len, _) = self.line_prefix_len(parent, line);
        (line.start(), line.start() + parent_prefix_len)
    }

    /// Returns the range representing the portion of the line that constitutes
    /// the given node's prefix; comes after [`Self::line_ancestors_prefix`] and
    /// before [`Self::line_content`].
    ///
    /// See [`Self::line_ancestors_prefix`] for an example.
    pub fn line_own_prefix(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> (Grapheme, Grapheme) {
        let Some(parent) = node.parent() else {
            return line.start().into_range();
        }; // document has no prefix
        let (parent_prefix_len, _) = self.line_prefix_len(parent, line);
        let (prefix_len, _) = self.line_prefix_len(node, line);

        (line.start() + parent_prefix_len, line.start() + prefix_len)
    }

    /// Returns the range representing the portion of the line after the given
    /// node's prefix; comes after [`Self::line_ancestors_prefix`] and
    /// [`Self::line_own_prefix`].
    ///
    /// See [`Self::line_ancestors_prefix`] for an example.
    pub fn line_content(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> (Grapheme, Grapheme) {
        let (prefix_len, _) = self.line_prefix_len(node, line);
        (line.start() + prefix_len, line.end())
    }

    /// Returns the range representing the portion of the line before the
    /// `node_content`. Equivalent to [`Self::line_ancestors_prefix`] +
    /// [`Self::line_own_prefix`]. Always has length == [`Self::line_prefix_len`].
    pub fn line_prefix(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> (Grapheme, Grapheme) {
        let (prefix_len, _) = self.line_prefix_len(node, line);
        (line.start(), line.start() + prefix_len)
    }

    /// Returns the string that could be prepended to a line to make that line
    /// an extension of the given node e.g. "`> `" for block quotes or "` `" for
    /// list items. Returns `None` if the node is not supported e.g. tables.
    pub fn extension_own_prefix(&self, node: &'ast AstNode<'ast>) -> Option<String> {
        let line = self.node_first_line(node);
        let own_prefix = self.line_own_prefix(node, line);

        Some(match &node.data.borrow().value {
            NodeValue::FrontMatter(_) | NodeValue::Raw(_) => {
                unreachable!("not a container block")
            }

            // container_block
            NodeValue::Alert(_) => self.buffer[own_prefix].into(),
            NodeValue::BlockQuote => self.buffer[own_prefix].into(),
            NodeValue::DescriptionItem(_) => {
                unimplemented!("extension disabled")
            }
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => {
                return None;
            }
            NodeValue::FootnoteDefinition(_) => "  ".into(),

            NodeValue::Item(_) => " ".repeat(own_prefix.len().0),
            NodeValue::List(_) => {
                return None;
            }
            NodeValue::MultilineBlockQuote(_) => {
                unimplemented!("extension disabled")
            }
            NodeValue::Table(_) => {
                return None;
            }
            NodeValue::TableRow(_) => {
                return None;
            }
            NodeValue::TaskItem(_) => {
                if line == self.node_first_line(node) {
                    // ' [ ]' is not part of the list marker for indentation purposes
                    " ".repeat(own_prefix.len().0.saturating_sub(4))
                } else {
                    " ".repeat(own_prefix.len().0)
                }
            }

            // inline
            NodeValue::Image(_)
            | NodeValue::Code(_)
            | NodeValue::Emph
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::Highlight
            | NodeValue::HtmlInline(_)
            | NodeValue::LineBreak
            | NodeValue::Link(_)
            | NodeValue::Math(_)
            | NodeValue::ShortCode(_)
            | NodeValue::SoftBreak
            | NodeValue::SpoileredText
            | NodeValue::Strikethrough
            | NodeValue::Strong
            | NodeValue::Subscript
            | NodeValue::Subtext
            | NodeValue::Superscript
            | NodeValue::Text(_)
            | NodeValue::Underline
            | NodeValue::WikiLink(_) => unreachable!("not a container block"),

            // leaf_block
            NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => unreachable!("not a container block"),
        })
    }

    /// Columns one nesting level of `node` contributes — what
    /// shift-tab strips. `Item` uses its own `padding` (not the
    /// parent list's: comrak merges sibling-compatible lists across
    /// blank lines and the merged list keeps the first item's
    /// padding). `BlockQuote`/`Alert` return `None` — they nest via
    /// `>` markers, deindented through `line_own_prefix` instead.
    pub fn deindent_level_cols(&self, node: &'ast AstNode<'ast>) -> Option<usize> {
        match &node.data.borrow().value {
            NodeValue::Item(item) => Some(item.padding),
            NodeValue::TaskItem(_) => match &node.parent()?.data.borrow().value {
                NodeValue::List(list) => Some(list.padding),
                _ => None,
            },
            NodeValue::FootnoteDefinition(_) => Some(2),
            _ => None,
        }
    }

    /// Returns the prefix required to extend the given node and its ancestors
    /// to a new line i.e. the concatenation of [`Self::extension_own_prefix`] of all
    /// container block ancestors.
    pub fn extension_prefix(&self, node: &'ast AstNode<'ast>) -> Option<String> {
        let mut result = if let Some(parent) = node.parent() {
            self.extension_prefix(parent)?
        } else {
            Default::default()
        };

        if let Some(own_prefix) = self.extension_own_prefix(node) {
            result += &own_prefix;
        }

        Some(result)
    }

    /// Returns the prefix required to insert a new node of the same type after
    /// the given prior node, extending all ancestors.
    pub fn insertion_prefix(&self, prior_node: &'ast AstNode<'ast>) -> Option<String> {
        let line = self.node_first_line(prior_node);

        let is_list_item = matches!(
            prior_node.data.borrow().value,
            NodeValue::Item(_) | NodeValue::TaskItem(_)
        );
        let mut result = if is_list_item {
            // Preserve the prior item's actual source indentation rather than
            // reconstructing it from ancestor marker widths, which drops any
            // indentation deeper than the minimum (e.g. tab / 4-space nesting).
            self.buffer[self.line_ancestors_prefix(prior_node, line)].to_string()
        } else if let Some(parent) = prior_node.parent() {
            self.extension_prefix(parent)?
        } else {
            Default::default()
        };

        let own_prefix = self.line_own_prefix(prior_node, line);

        match &prior_node.data.borrow().value {
            NodeValue::FrontMatter(_) | NodeValue::Raw(_) => {
                unreachable!("not a container block")
            }

            // container_block
            NodeValue::Alert(_) => result += &self.buffer[own_prefix],
            NodeValue::BlockQuote => result += &self.buffer[own_prefix],
            NodeValue::DescriptionItem(_) => {
                unimplemented!("extension disabled")
            }
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => {
                return None;
            }
            NodeValue::FootnoteDefinition(_) => result += &self.buffer[own_prefix], // unsure about this one

            NodeValue::Item(_) => result += &self.buffer[own_prefix],
            NodeValue::List(_) => {
                return None;
            }
            NodeValue::MultilineBlockQuote(_) => {
                unimplemented!("extension disabled")
            }
            NodeValue::Table(_) => {
                return None;
            }
            NodeValue::TableRow(_) => {
                return None;
            }
            NodeValue::TaskItem(node_task_item) => {
                let check = &node_task_item.symbol;
                if let Some(check) = check {
                    result += &self.buffer[own_prefix].replace(*check, " ")
                } else {
                    result += &self.buffer[own_prefix]
                }
            }

            // inline
            NodeValue::Image(_)
            | NodeValue::Code(_)
            | NodeValue::Emph
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::Highlight
            | NodeValue::HtmlInline(_)
            | NodeValue::LineBreak
            | NodeValue::Link(_)
            | NodeValue::Math(_)
            | NodeValue::ShortCode(_)
            | NodeValue::SoftBreak
            | NodeValue::SpoileredText
            | NodeValue::Strikethrough
            | NodeValue::Strong
            | NodeValue::Subscript
            | NodeValue::Subtext
            | NodeValue::Superscript
            | NodeValue::Text(_)
            | NodeValue::Underline
            | NodeValue::WikiLink(_) => unreachable!("not a container block"),

            // leaf_block
            NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => unreachable!("not a container block"),
        };

        Some(result)
    }

    /// Makes each gutter column (marker / per-level indentation) a word
    /// for word navigation and double-click. Replaces the default
    /// unicode-segmented words inside each line's prefix region — which
    /// would split a marker like `* ` into `*` + ` ` and drop
    /// indentation — with the per-level [`Self::line_own_prefix`] columns. Call after
    /// `calc_words`, with the parsed `root`.
    pub fn calc_syntax_words(&mut self, root: &'ast AstNode<'ast>) {
        let n = self.bounds.source_lines.len();
        if n == 0 {
            return;
        }
        // One DFS resolves every line's deepest gutter container; a
        // per-line `deepest_container_block_at_offset` lookup would be
        // quadratic, and this runs every reparse.
        let mut deepest_by_line: Vec<Option<&'ast AstNode<'ast>>> = vec![None; n];
        self.assign_deepest_gutter(root, &mut deepest_by_line);

        let mut prefix_regions: Vec<(Grapheme, Grapheme)> = Vec::new();
        let mut columns: Vec<(Grapheme, Grapheme)> = Vec::new();
        for (line_idx, &deepest) in deepest_by_line.iter().enumerate() {
            let Some(deepest) = deepest else {
                continue;
            };
            let line = self.bounds.source_lines[line_idx];
            let (prefix_len, _) = self.line_prefix_len(deepest, line);
            if prefix_len.0 == 0 {
                continue;
            }
            prefix_regions.push((line.start(), line.start() + prefix_len));
            let mut level = Some(deepest);
            while let Some(node) = level {
                let range = self.line_own_prefix(node, line);
                if !range.is_empty() {
                    columns.push(range);
                }
                level = self.gutter_parent(node);
            }
        }
        if columns.is_empty() {
            return;
        }
        // Drop unicode words that fell in a prefix region; the columns
        // replace them. Both sets are non-overlapping and the regions are
        // disjoint from content, so the merged list stays sorted and
        // non-overlapping as `range_bound` requires.
        self.bounds
            .words
            .retain(|w| !prefix_regions.iter().any(|p| w.intersects(p, false)));
        self.bounds.words.extend(columns);
        self.bounds.words.sort();
    }

    /// Returns whether the given line is one of the source lines of the given node
    pub fn node_contains_line(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> bool {
        let first_line = self.node_first_line(node);
        let last_line = self.node_last_line(node);

        (first_line.start(), last_line.end()).contains_range(&line, true, true)
    }

    // compute bounds for blocks stacked vertically
    pub fn compute_bounds_block_children(&mut self, node: &'ast AstNode<'ast>) {
        for child in node.children() {
            // add pre-spacing bounds
            self.compute_bounds_block_pre_spacing(child);

            // add block bounds
            self.compute_bounds(child);

            // add post-spacing bounds
            self.compute_bounds_block_post_spacing(child);
        }
    }
}

impl<'ast> MdDoc {
    pub fn own_prefix_len_task_item(
        &self,
        node: &'ast AstNode<'ast>,
        line: (Grapheme, Grapheme),
    ) -> Option<Graphemes> {
        let node_line = self.node_line(node, line);
        let mut result = 0.into();

        // "If a sequence of lines Ls constitutes a list item according to rule
        // #1, #2, or #3, then the result of indenting each line of Ls by 1-3
        // spaces (the same for each line) also constitutes a list item with the
        // same contents and attributes."
        let indentation = {
            let first_line = self.node_first_line(node);
            let node_line = self.node_line(node, first_line);

            // 1-3 columns of relative indent before the marker; ceil so a
            // straddling tab is claimed rather than leaking into content.
            let text = &self.buffer[(node_line.start(), node_line.end())];
            consume_indent_columns_ceil(text, 3)
        };
        let marker_width_including_spaces: usize = {
            // task items don't have a NodeList so we have to do this ourselves
            let first_line = self.node_first_line(node);

            // "   - [ ]   item"
            // indentation + marker + spaces + content
            let node_line = self.node_line(node, first_line);

            // spaces + content
            let marker_width = 5;
            let text = &self.buffer[(
                node_line.start() + indentation + marker_width,
                node_line.end(),
            )];
            marker_width
                + if text.starts_with("   ") {
                    "   ".len()
                } else if text.starts_with("  ") {
                    "  ".len()
                } else if text.starts_with(" ") {
                    " ".len()
                } else {
                    0
                }
        };
        if line == self.node_first_line(node) {
            result += indentation;

            // "If a sequence of lines Ls constitute a sequence of blocks Bs starting
            // with a non-whitespace character, and M is a list marker of width W
            // followed by 1 ≤ N ≤ 4 spaces, then the result of prepending M and the
            // following spaces to the first line of Ls, and indenting subsequent lines
            // of Ls by W + N spaces, is a list item with Bs as its contents."
            //
            // "If a sequence of lines Ls starting with a single blank line
            // constitute a (possibly empty) sequence of blocks Bs, not separated
            // from each other by more than one blank line, and M is a list marker
            // of width W, then the result of prepending M to the first line of Ls,
            // and indenting subsequent lines of Ls by W + 1 spaces, is a list item
            // with Bs as its contents."
            result += marker_width_including_spaces;
        } else {
            // "If a string of lines Ls constitute a list item with contents Bs, then
            // the result of deleting some or all of the indentation from one or
            // more lines in which the next non-whitespace character after the
            // indentation is paragraph continuation text is a list item with the
            // same contents and attributes."
            //
            // "If a line is empty, then it need not be indented."
            //
            // The threshold here is the *list's* content column (= the
            // bullet marker `- ` width, typically 2), not the visual
            // `- [ ] ` width. Comrak parses `- [ ] foo` as a `- ` bullet
            // item whose content begins with the task syntax `[ ] `, so an
            // indented code block inside the item begins at content_col + 4
            // (= 6). Stripping the full visual `- [ ] ` here would consume
            // the very indent the code block is defined by — and the
            // code block's own 4-space strip would then leave it empty.
            let parent_padding = match &node.parent().unwrap().data.borrow().value {
                NodeValue::List(node_list) => node_list.padding,
                _ => marker_width_including_spaces, // fallback, shouldn't happen
            };
            let text = &self.buffer[node_line];
            // Ceil so a straddling tab is claimed here rather than leaking
            // into a nested block's prefix (per-level tab attribution).
            result += consume_indent_columns_ceil(text, indentation + parent_padding);
        }

        Some(result)
    }

    pub fn compute_bounds_task_item(&mut self, node: &'ast AstNode<'ast>) {
        let any_children = node.children().next().is_some();
        if any_children {
            self.compute_bounds_block_children(node);
        } else {
            let line = self.node_first_line(node);
            let line_content = self.line_content(node, line);
            self.bounds.inline_paragraphs.push(line_content);
        }
    }

    pub fn check_offset(&self, node: &'ast AstNode<'ast>) -> Grapheme {
        let line = self.node_first_line(node);
        let node_line = self.node_line(node, line);

        let text = &self.buffer[(node_line.start(), node_line.end())];
        let indentation = if text.starts_with("   ") {
            "   ".len()
        } else if text.starts_with("  ") {
            "  ".len()
        } else if text.starts_with(" ") {
            " ".len()
        } else {
            0
        };

        node_line.start() + indentation + 3
    }
}
