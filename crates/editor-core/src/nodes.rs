//! AST nodes and their source ranges, offset conversions, and indent
//! math. Indent math works in two units that diverge once tabs are involved:
//! **graphemes** for slicing source, **columns** for nesting depth
//! (CommonMark §2.2: tabs in indent positions act like spaces to
//! the next 4-col stop). Structural prefix helpers live in
//! [`crate::blocks`].

use comrak::nodes::{AstNode, NodeHeading, NodeValue};

use crate::bounds::RangesExt as _;
use crate::buffer::Buffer;
use crate::doc::MdDoc;
use crate::offset_types::{Byte, Grapheme, Graphemes, RangeExt as _, RangeIterExt as _};

impl<'ast> MdDoc {
    /// Returns the range for the node.
    pub fn node_range(&self, node: &'ast AstNode<'ast>) -> (Grapheme, Grapheme) {
        // Check cache first
        if let Some(cached_range) = self.get_cached_node_range(node) {
            return cached_range;
        }

        let node_data = node.data.borrow();
        let mut range = self.sourcepos_to_range(node_data.sourcepos);

        match &node_data.value {
            // hack: comrak's sourcepos's are unstable (and indeed broken) for some
            // nested block situations. clamping paragraph ranges to their parent's
            // prevents the worst of the adverse consequences (e.g. double-rendering
            // source text).
            //
            // see: https://github.com/kivikakk/comrak/issues/567
            NodeValue::Paragraph => {
                let parent = node.parent().unwrap();
                let parent_range = self.node_range(parent);
                range.0 = range.0.max(parent_range.0);
                range.1 = range.1.min(parent_range.1);
            }

            // hack: "A line break (not in a code span or HTML tag) that is preceded
            // by two or more spaces and does not occur at the end of a block is
            // parsed as a hard line break" but we prefer to show the spaces since
            // we render soft breaks as hard breaks (which is up to our discretion).
            // https://github.github.com/gfm/#hard-line-breaks
            NodeValue::LineBreak => {
                range.0 = range.1 - 1; // include only the newline
            }

            // hack: GFM spec says "Blank lines preceding or following an indented
            // code block are not included in it" and I have observed the behavior
            // for following lines to be incorrect in e.g. "    f\n".
            NodeValue::CodeBlock(node_code_block) if !node_code_block.fenced => {
                for line_idx in self.range_lines(range).iter() {
                    let line = self.bounds.source_lines[line_idx];
                    let node_line = self.node_line(node, line);
                    if self.buffer[node_line].chars().any(|c| !c.is_whitespace()) {
                        range.1 = line.end();
                    }
                }
            }

            // hack: thematic breaks are emitted to contain all subsequent lines if
            // they are the last block in the document; we trim them to their first
            // line.
            NodeValue::ThematicBreak => {
                if let Some(line_idx) = self.range_lines(range).iter().next() {
                    let line = self.bounds.source_lines[line_idx];
                    range = range.trim(&line);
                }
            }

            // hack: list items are emitted to contain all lines until the next
            // block which would cause the cursor to be shown indented; we trim
            // trailing blank lines.
            NodeValue::Item(_) | NodeValue::TaskItem(_) => {
                let node_lines = self.range_lines(range);
                let mut last_nonempty_line_idx = node_lines.start();
                for line_idx in node_lines.iter() {
                    let line = self.bounds.source_lines[line_idx];
                    let node_line = self.node_line(node, line);
                    if !node_line.is_empty() {
                        last_nonempty_line_idx = line_idx;
                    }
                }

                let last_nonempty_line = self.bounds.source_lines[last_nonempty_line_idx];
                range.1 = last_nonempty_line.end();
            }

            NodeValue::List(_) => {
                let last_child = node.children().last().unwrap();
                range.1 = self.node_range(last_child).1;
            }

            // hack: comrak misreports the end column of an HtmlBlock that
            // sits inside a `>\t`-prefixed blockquote — it overshoots by
            // the tab's expanded-vs-source width (e.g. for `>\t<div>foo
            // </div>` it reports end col 18 instead of 16). The over-
            // wide end leaks the range onto the next line and causes the
            // following block's first line to be re-rendered as part of
            // the HtmlBlock. Confirmed via repro against comrak 0.50 that
            // this misreport is unique to this combination — every other
            // tab-blockquote shape (paragraph, code span, list-item
            // child) reports source-aligned columns. Clamp to the end of
            // the sourcepos's last line.
            NodeValue::HtmlBlock(_) => {
                let last_line_idx = node_data.sourcepos.end.line.saturating_sub(1);
                if let Some(line) = self.bounds.source_lines.get(last_line_idx) {
                    range.1 = range.1.min(line.end());
                }
            }

            // hack: for a Table inside a list Item, comrak reports
            // sourcepos columns for every row as if that row has the
            // same prefix as the Item's first line (e.g. `- `, 2
            // chars). A continuation line with different leading
            // whitespace (`\t` = 1 char, `    ` = 4 chars) gets the
            // same reported cols, so TableRow/TableCell/Text nodes
            // point at the wrong source bytes. Shift by
            // `(actual leading ws) - (item.padding)` per line to align
            // with actual source. Matches comrak issue #591 (closed
            // for indented-predecessor case, still present for
            // list-item continuation).
            // Any node descended from a Table whose rows hit comrak's
            // canonical-first-line quirk needs the shift, not just
            // Text/TableRow/TableCell — inline wrappers like Emph,
            // Strong, Code, Link have their own sourcepos that
            // otherwise points at pre-shift bytes. The helper's Table
            // ancestor check gates this; inline-vs-block distinction
            // doesn't matter.
            NodeValue::TableRow(_)
            | NodeValue::TableCell
            | NodeValue::Text(_)
            | NodeValue::Emph
            | NodeValue::Strong
            | NodeValue::Strikethrough
            | NodeValue::Code(_)
            | NodeValue::HtmlInline(_)
            | NodeValue::Link(_)
            | NodeValue::Image(_)
            | NodeValue::Highlight
            | NodeValue::Underline
            | NodeValue::Superscript
            | NodeValue::Subscript
            | NodeValue::SpoileredText
            | NodeValue::Math(_)
            | NodeValue::ShortCode(_)
            | NodeValue::WikiLink(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::SoftBreak
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::Subtext => {
                if let Some(shift) = self.table_in_item_continuation_shift(node)
                    && shift != 0
                {
                    // Redo the sourcepos→byte conversion, applying
                    // the shift in BYTE space *before* ceiling to
                    // grapheme boundaries. Taking the ceiled
                    // grapheme back to bytes and shifting there
                    // compounds the ceil with the shift, yielding
                    // a grapheme one or two clusters past where
                    // the actual content ends (observed with
                    // Devanagari in tab-indented table cells).
                    let buf_byte_end = self.buffer.current.len_bytes();
                    let sp = node_data.sourcepos;
                    let line_to_byte = |lc: comrak::nodes::LineColumn,
                                        plus_one_for_exclusive: bool|
                     -> crate::offset_types::Byte {
                        let line_idx = lc.line.saturating_sub(1);
                        let col = if plus_one_for_exclusive {
                            lc.column + 1
                        } else {
                            lc.column
                        };
                        let col_idx = col.saturating_sub(1);
                        let line = self.bounds.source_lines[line_idx];
                        let line_start = self.offset_to_byte(line.start()).0 as isize;
                        let shifted = line_start
                            .saturating_add(col_idx as isize)
                            .saturating_add(shift)
                            .max(0) as usize;
                        crate::offset_types::Byte(shifted.min(buf_byte_end))
                    };
                    let start_byte = line_to_byte(sp.start, false);
                    let end_byte = line_to_byte(sp.end, true);
                    range = (
                        self.buffer.current.segs.byte_to_char_ceil(start_byte),
                        self.buffer.current.segs.byte_to_char_ceil(end_byte),
                    );
                }
            }

            _ => {}
        }

        // Cache the result before returning
        self.set_cached_node_range(node, range);
        range
    }

    /// Byte shift to apply to a `TableRow`/`TableCell`/`Text` node
    /// when comrak's sourcepos columns don't match source — applies
    /// only when a `Table`'s first row sits on a list `Item`'s first
    /// line (e.g. `- | a | b |`, table immediately after the marker).
    /// In that shape comrak reports subsequent rows' cols as if they
    /// share the item-marker prefix, even when continuation lines use
    /// a different indent (tab, 4sp, etc.). Shift by
    /// `actual_leading_ws - item.padding` to align with source. For
    /// tables that *aren't* the item's first content (starting on a
    /// continuation line), comrak already reports source-aligned cols
    /// — skip the shift. Also restricted to top-level items (nested
    /// cases would need ancestor-aware leading-ws accounting).
    pub fn table_in_item_continuation_shift(&self, node: &'ast AstNode<'ast>) -> Option<isize> {
        let mut cur = Some(node);
        let table = loop {
            let n = cur?;
            if matches!(&n.data.borrow().value, NodeValue::Table(_)) {
                break n;
            }
            cur = n.parent();
        };
        let item = table.parent()?;
        let padding = match &item.data.borrow().value {
            NodeValue::Item(nl) => nl.padding,
            NodeValue::TaskItem(_) => match item.parent().map(|p| p.data.borrow().value.clone()) {
                Some(NodeValue::List(nl)) => nl.padding,
                _ => return None,
            },
            _ => return None,
        };
        let list = item.parent()?;
        let list_parent = list.parent()?;
        if !matches!(&list_parent.data.borrow().value, NodeValue::Document) {
            return None;
        }
        // Only when the Table's first row is on the Item's first line
        // (comrak's quirk only fires in that shape).
        let item_first_line = item.data.borrow().sourcepos.start.line;
        if table.data.borrow().sourcepos.start.line != item_first_line {
            return None;
        }
        let node_line = node.data.borrow().sourcepos.start.line;
        if node_line == item_first_line {
            return Some(0);
        }
        let line_idx = node_line.saturating_sub(1);
        let line = self.bounds.source_lines.get(line_idx)?;
        let text = &self.buffer[*line];
        let leading_ws = text.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
        Some(leading_ws as isize - padding as isize)
    }

    /// Returns the lines spanned by the given range.
    pub fn range_lines(&self, range: (Grapheme, Grapheme)) -> (usize, usize) {
        // The first and last "split" positions are just `range.0` and
        // `range.1` — splitting on newlines doesn't change either
        // endpoint. Look those up directly in `source_lines` instead
        // of materializing a Vec of every intermediate line.
        let start_line_idx = self
            .bounds
            .source_lines
            .find_containing(range.0, true, true)
            .start();
        let end_line_idx = self
            .bounds
            .source_lines
            .find_containing(range.1, true, true)
            .end(); // (inclusive, exclusive) preserved
        (start_line_idx, end_line_idx)
    }

    pub fn compute_bounds(&mut self, node: &'ast AstNode<'ast>) {
        let value = &node.data.borrow().value;
        match value {
            NodeValue::FrontMatter(_) => {}
            NodeValue::Raw(_) => unreachable!("can only be created programmatically"),

            // container_block
            NodeValue::Alert(node_alert) => self.compute_bounds_alert(node, node_alert),
            NodeValue::BlockQuote => self.compute_bounds_block_quote(node),
            NodeValue::DescriptionItem(_) => unimplemented!("extension disabled"),
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => self.compute_bounds_document(node),
            NodeValue::FootnoteDefinition(_) => self.compute_bounds_footnote_definition(node),
            NodeValue::Item(_) => self.compute_bounds_item(node),
            NodeValue::List(_) => self.compute_bounds_block_children(node),
            NodeValue::MultilineBlockQuote(_) => unimplemented!("extension disabled"),
            NodeValue::Table(_) => self.compute_bounds_block_children(node),
            NodeValue::TableRow(_) => self.compute_bounds_block_children(node),
            NodeValue::TaskItem(_) => self.compute_bounds_task_item(node),

            // inline
            NodeValue::Code(_) => {}
            NodeValue::Emph => {}
            NodeValue::Escaped => {}
            NodeValue::EscapedTag(_) => {}
            NodeValue::FootnoteReference(_) => {}
            NodeValue::Highlight => {}
            NodeValue::HtmlInline(_) => {}
            NodeValue::Image(_) => {}
            NodeValue::LineBreak => {}
            NodeValue::Link(_) => {}
            NodeValue::Math(_) => {}
            NodeValue::ShortCode(_) => {}
            NodeValue::SoftBreak => {}
            NodeValue::SpoileredText => {}
            NodeValue::Strikethrough => {}
            NodeValue::Strong => {}
            NodeValue::Subscript => {}
            NodeValue::Subtext => {}
            NodeValue::Superscript => {}
            NodeValue::Text(_) => {}
            NodeValue::Underline => {}
            NodeValue::WikiLink(_) => {}

            // leaf_block
            NodeValue::CodeBlock(_) => {}
            NodeValue::DescriptionDetails => unimplemented!("extension disabled"),
            NodeValue::DescriptionTerm => unimplemented!("extension disabled"),
            NodeValue::Heading(NodeHeading { level, setext, .. }) => {
                self.compute_bounds_heading(node, *level, *setext)
            }
            NodeValue::HtmlBlock(_) => {}
            NodeValue::Paragraph => self.compute_bounds_paragraph(node),
            NodeValue::TableCell => self.compute_bounds_table_cell(node),
            NodeValue::ThematicBreak => {}
        }
    }
}

/// Consume leading whitespace up to `target_columns` columns,
/// returning graphemes consumed. A tab is atomic: if its column span
/// would exceed `target_columns`, it's refused.
///
/// `target_columns = usize::MAX` claims all leading whitespace as
/// one unit (so a tab straddling a nested-block boundary stays with
/// the outermost block). Editing ops needing per-level granularity
/// should use [`leading_indent_cols`] +
/// `MdDoc::deindent_level_cols`.
pub fn consume_indent_columns(text: &str, target_columns: usize) -> usize {
    let mut cols = 0;
    let mut graphemes = 0;
    for c in text.chars() {
        if cols >= target_columns {
            break;
        }
        match c {
            ' ' => {
                cols += 1;
                graphemes += 1;
            }
            '\t' => {
                let new_cols = (cols / 4 + 1) * 4;
                // Don't consume the tab if its column span straddles
                // the target — the remaining virtual columns belong to
                // whatever strip runs next (per CommonMark §2.2). A
                // tab stripped whole would over-consume and leave less
                // content indent than the spec requires.
                if new_cols > target_columns {
                    break;
                }
                cols = new_cols;
                graphemes += 1;
            }
            _ => break,
        }
    }
    graphemes
}

/// Like [`consume_indent_columns`] but a tab straddling `target_columns`
/// is claimed *whole* rather than refused. Used to split a continuation
/// indent run per nesting level: the indentation must be fully
/// attributed, so a tab that overshoots a level's band goes to the level
/// whose band it starts in (the next level then finds its band already
/// consumed and claims nothing). With spaces it's identical to
/// [`consume_indent_columns`].
pub fn consume_indent_columns_ceil(text: &str, target_columns: usize) -> usize {
    let mut cols = 0;
    let mut graphemes = 0;
    for c in text.chars() {
        if cols >= target_columns {
            break;
        }
        match c {
            ' ' => {
                cols += 1;
                graphemes += 1;
            }
            // Still under target, so claim the whole tab even though it
            // overshoots — it can't be split across the boundary.
            '\t' => {
                cols = (cols / 4 + 1) * 4;
                graphemes += 1;
            }
            _ => break,
        }
    }
    graphemes
}

/// Grapheme range of the leading ` ` / `\t` run. Lexical, not
/// structural — see [`consume_indent_columns`] for why ownership
/// is ambiguous.
pub fn leading_indent_range(buffer: &Buffer, line: (Grapheme, Grapheme)) -> (Grapheme, Grapheme) {
    let text = &buffer[line];
    let mut graphemes = 0usize;
    for c in text.chars() {
        if c == ' ' || c == '\t' {
            graphemes += 1;
        } else {
            break;
        }
    }
    (line.start(), line.start() + Graphemes(graphemes))
}

/// Column count of leading whitespace, expanding tabs to 4-col
/// stops.
pub fn leading_indent_cols(text: &str) -> usize {
    let mut cols = 0;
    for c in text.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols = (cols / 4 + 1) * 4,
            _ => break,
        }
    }
    cols
}

/// Width, in columns, of `text`'s leading indentation, measured until it
/// reaches `target` columns. A space is one column; a tab advances to the
/// next 4-column stop (CommonMark §2.2), so the width can exceed the
/// number of characters.
///
/// A tab is counted whole even when it crosses `target`, so the result can
/// overshoot: one leading tab measured against `target` 3 returns 4, not
/// 3. (This is what "ceil" names — the partial tab rounds up to its stop.)
pub fn leading_indent_cols_ceil(text: &str, target: usize) -> usize {
    let mut cols = 0;
    for c in text.chars() {
        if cols >= target {
            break; // measured enough indentation
        }
        match c {
            ' ' => cols += 1,
            // jump to the next tab stop: the next multiple of 4 above `cols`
            '\t' => cols = (cols / 4 + 1) * 4,
            _ => break, // indentation ends at the first non-whitespace
        }
    }
    cols
}

#[allow(dead_code)]
pub trait NodeValueExt {
    fn is_leaf_block(&self) -> bool;
    fn is_container_block(&self) -> bool;
    fn is_inline(&self) -> bool;
}

impl NodeValueExt for NodeValue {
    fn is_leaf_block(&self) -> bool {
        match self {
            NodeValue::FrontMatter(_) | NodeValue::Raw(_) => false,

            // container_block
            NodeValue::Alert(_)
            | NodeValue::BlockQuote
            | NodeValue::DescriptionItem(_)
            | NodeValue::DescriptionList
            | NodeValue::Document
            | NodeValue::FootnoteDefinition(_)
            | NodeValue::Item(_)
            | NodeValue::List(_)
            | NodeValue::MultilineBlockQuote(_)
            | NodeValue::Table(_)
            | NodeValue::TableRow(_)
            | NodeValue::TaskItem(_) => false,

            // inline
            NodeValue::Code(_)
            | NodeValue::Emph
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::Highlight
            | NodeValue::HtmlInline(_)
            | NodeValue::Image(_)
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
            | NodeValue::WikiLink(_) => false,

            // leaf_block
            NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => true,
        }
    }

    fn is_container_block(&self) -> bool {
        match self {
            NodeValue::FrontMatter(_) | NodeValue::Raw(_) => false,

            // container_block
            NodeValue::Alert(_)
            | NodeValue::BlockQuote
            | NodeValue::DescriptionItem(_)
            | NodeValue::DescriptionList
            | NodeValue::Document
            | NodeValue::FootnoteDefinition(_)
            | NodeValue::Item(_)
            | NodeValue::List(_)
            | NodeValue::MultilineBlockQuote(_)
            | NodeValue::Table(_)
            | NodeValue::TableRow(_)
            | NodeValue::TaskItem(_) => true,

            // inline
            NodeValue::Code(_)
            | NodeValue::Emph
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::Highlight
            | NodeValue::HtmlInline(_)
            | NodeValue::Image(_)
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
            | NodeValue::WikiLink(_) => false,

            // leaf_block
            NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => false,
        }
    }

    fn is_inline(&self) -> bool {
        match self {
            NodeValue::FrontMatter(_) | NodeValue::Raw(_) => false,

            // container_block
            NodeValue::Alert(_)
            | NodeValue::BlockQuote
            | NodeValue::DescriptionItem(_)
            | NodeValue::DescriptionList
            | NodeValue::Document
            | NodeValue::FootnoteDefinition(_)
            | NodeValue::Item(_)
            | NodeValue::List(_)
            | NodeValue::MultilineBlockQuote(_)
            | NodeValue::Table(_)
            | NodeValue::TableRow(_)
            | NodeValue::TaskItem(_) => false,

            // inline
            NodeValue::Code(_)
            | NodeValue::Emph
            | NodeValue::Escaped
            | NodeValue::EscapedTag(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::Highlight
            | NodeValue::HtmlInline(_)
            | NodeValue::Image(_)
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
            | NodeValue::WikiLink(_) => true,

            // leaf_block
            NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => false,
        }
    }
}

impl<'ast> NodeValueExt for AstNode<'ast> {
    fn is_leaf_block(&self) -> bool {
        self.data.borrow().value.is_leaf_block()
    }

    fn is_container_block(&self) -> bool {
        self.data.borrow().value.is_container_block()
    }

    fn is_inline(&self) -> bool {
        self.data.borrow().value.is_inline()
    }
}

impl<'ast> MdDoc {
    // wrappers because I'm tired of writing ".buffer.current.segs" all the time
    pub fn offset_to_byte(&self, i: Grapheme) -> Byte {
        self.buffer.current.segs.offset_to_byte(i)
    }

    pub fn range_to_byte(&self, i: (Grapheme, Grapheme)) -> (Byte, Byte) {
        self.buffer.current.segs.range_to_byte(i)
    }

    pub fn offset_to_char(&self, i: Byte) -> Grapheme {
        self.buffer.current.segs.offset_to_char(i)
    }

    pub fn range_to_char(&self, i: (Byte, Byte)) -> (Grapheme, Grapheme) {
        self.buffer.current.segs.range_to_char(i)
    }

    /// Byte→char conversion for byte offsets that may not land on a grapheme
    /// boundary. Snaps each endpoint up to the next boundary.
    ///
    /// `Grapheme` indexes graphemes; the strict `range_to_char` panics on
    /// any byte that isn't a grapheme start (no corresponding char). That's the
    /// right behavior when bytes come from the buffer's own segs or round-trip
    /// through a `Grapheme` — a panic there is a real bug, not data we
    /// should paper over.
    ///
    /// Use this version *only* when the byte source promises codepoint
    /// boundaries but not grapheme boundaries — i.e. cosmic-text glyph byte
    /// positions and comrak sourcepos. Both index by codepoint and will hand
    /// us positions inside a cluster whenever an extending codepoint
    /// (Devanagari vowel sign, ZWJ, variation selector, virama) sits at a
    /// rendering or parsing seam. The strict converter has no recourse for
    /// these; rounding up is the only well-defined answer that keeps the
    /// editor usable on Indic / emoji text.
    ///
    /// Snapping consistently to ceil gives every cluster exactly one home: it
    /// belongs to whichever row/section's *end* boundary crosses it, and the
    /// next row/section's *start* lands at or past the cluster's far edge — no
    /// double-counting, no gaps.
    pub fn range_to_char_ceil(&self, i: (Byte, Byte)) -> (Grapheme, Grapheme) {
        let segs = &self.buffer.current.segs;
        (segs.byte_to_char_ceil(i.0), segs.byte_to_char_ceil(i.1))
    }

    pub fn last_cursor_position(&self) -> Grapheme {
        self.buffer.current.segs.last_cursor_position()
    }

    /// Returns a Vec of ranges that represent the given range split on newlines
    /// (based on source text). Behavior inspired by [`str::split`].
    pub fn range_split_newlines(&self, range: (Grapheme, Grapheme)) -> Vec<(Grapheme, Grapheme)> {
        let text = &self.buffer[range];
        let byte_range = self.range_to_byte(range);
        let base_offset = byte_range.0;

        // Special case for empty input
        if text.is_empty() {
            return vec![range];
        }

        let mut result = Vec::new();
        let bytes = text.as_bytes();
        let mut pos = 0;

        while pos < bytes.len() {
            let mut start_pos = pos;

            // Find the end of this line. CommonMark line endings: \n, \r\n, or
            // bare \r (CR not followed by LF). Must match comrak's line
            // counting — if we missed a separator it splits on, our
            // `source_lines` wouldn't align with comrak's sourcepos line
            // numbers.
            while pos < bytes.len() {
                if bytes[pos] == b'\n' {
                    let line_range = (base_offset + start_pos, base_offset + pos);
                    result.push(self.range_to_char(line_range));
                    pos += 1;
                    start_pos = pos;
                    break;
                } else if bytes[pos] == b'\r' {
                    let line_range = (base_offset + start_pos, base_offset + pos);
                    result.push(self.range_to_char(line_range));
                    pos += if pos + 1 < bytes.len() && bytes[pos + 1] == b'\n' {
                        2
                    } else {
                        1
                    };
                    start_pos = pos;
                    break;
                }

                pos += 1;
            }

            // If we reached the end without finding a line ending
            if pos == bytes.len() {
                let line_range = (base_offset + start_pos, base_offset + pos);
                result.push(self.range_to_char(line_range));
            }
        }

        // For empty text or text with only newlines
        if result.is_empty() {
            result.push(range);
        }

        result
    }

    pub fn selection_offset(&self) -> Option<Grapheme> {
        if self.buffer.current.selection.is_empty() {
            Some(self.buffer.current.selection.0)
        } else {
            None
        }
    }

    /// Returns the deepest container block node containing the offset.
    pub fn deepest_container_block_at_offset(
        &self,
        node: &'ast AstNode<'ast>,
        offset: Grapheme,
    ) -> &'ast AstNode<'ast> {
        for child in node.children() {
            if !child.data.borrow().value.is_container_block() {
                continue;
            }
            for line_idx in self.node_lines(child).iter() {
                let line = self.bounds.source_lines[line_idx];
                let node_line = self.node_line(child, line);
                if node_line.contains(offset, false, true) {
                    return self.deepest_container_block_at_offset(child, offset);
                }
            }
        }
        node
    }

    /// Returns the leaf block node containing the offset.
    pub fn leaf_block_at_offset(
        &self,
        node: &'ast AstNode<'ast>,
        offset: Grapheme,
    ) -> &'ast AstNode<'ast> {
        for child in node.children() {
            for line_idx in self.node_lines(child).iter() {
                let line = self.bounds.source_lines[line_idx];
                let node_line = self.node_line(child, line);
                if node_line.contains(offset, false, true) {
                    if child.data.borrow().value.is_leaf_block() {
                        return child;
                    } else {
                        return self.leaf_block_at_offset(child, offset);
                    }
                }
            }
        }
        node
    }

    pub fn line_at_offset(&self, offset: Grapheme) -> (Grapheme, Grapheme) {
        let (line_idx, _) = self.bounds.source_lines.find_containing(offset, true, true);
        self.bounds.source_lines[line_idx]
    }
}

/// Whether `node` is a strikethrough written with single tildes, as in
/// `H~2~O`. comrak reads those as strikethrough, but today's editor and
/// Obsidian draw strikethrough only between `~~`, so the editor draws them
/// as written.
pub fn is_single_tilde_strikethrough(node: &AstNode<'_>) -> bool {
    if !matches!(node.data.borrow().value, NodeValue::Strikethrough) {
        return false;
    }
    let start = node.data.borrow().sourcepos.start;
    match node.first_child() {
        Some(child) => {
            let child_start = child.data.borrow().sourcepos.start;
            child_start.line == start.line && child_start.column == start.column + 1
        }
        None => false,
    }
}

impl<'ast> MdDoc {
    /// Returns the range between the start of the node and the start of its
    /// first child, if there is one.
    pub fn prefix_range(&self, node: &'ast AstNode<'ast>) -> Option<(Grapheme, Grapheme)> {
        let range = self.node_range(node);
        let first_child = node.children().next()?;
        let first_child_range = self.node_range(first_child);
        Some((range.start(), first_child_range.start()))
    }

    /// Returns the range of the leading syntax characters that define this node
    pub fn head_range(&self, node: &'ast AstNode<'ast>) -> Option<(Grapheme, Grapheme)> {
        if matches!(node.data.borrow().value, NodeValue::Code(_)) {
            let range = self.node_range(node);
            Some((range.start(), range.start() + 1)) // code has no children; contains text directly
        } else {
            self.prefix_range(node)
        }
    }

    /// Returns the range between the end of the node's last child if there is
    /// one, and the end of the node.
    pub fn postfix_range(&self, node: &'ast AstNode<'ast>) -> Option<(Grapheme, Grapheme)> {
        let range = self.node_range(node);
        let last_child = node.children().last()?;
        let last_child_range = self.node_range(last_child);
        Some((last_child_range.end(), range.end()))
    }

    /// Returns the range of the trailing syntax characters that define this node
    pub fn tail_range(&self, node: &'ast AstNode<'ast>) -> Option<(Grapheme, Grapheme)> {
        if matches!(node.data.borrow().value, NodeValue::Code(_)) {
            let range = self.node_range(node);
            Some((range.end() - 1, range.end())) // code has no children; contains text directly
        } else {
            self.postfix_range(node)
        }
    }

    /// Returns the range between the start of the node's first child and the
    /// end of it's last child, if there are any children. For many nodes, this
    /// is the content in the node.
    pub fn infix_range(&self, node: &'ast AstNode<'ast>) -> Option<(Grapheme, Grapheme)> {
        let first_child = node.children().next()?;
        let first_child_range = self.node_range(first_child);
        let last_child = node.children().last()?;
        let last_child_range = self.node_range(last_child);
        Some((first_child_range.start(), last_child_range.end()))
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn range_lines_char_no_newline() {
        let text = "*";
        let md = crate::MdDoc::new(text);
        let lines = md.range_split_newlines((0.into(), text.len().into()));
        assert_eq!(lines, vec![(0.into(), 1.into())]);
    }

    #[test]
    fn range_lines_char_newline() {
        let text = "*\n";
        let md = crate::MdDoc::new(text);
        let lines = md.range_split_newlines((0.into(), text.len().into()));
        assert_eq!(lines, vec![(0.into(), 1.into()), (2.into(), 2.into())]);
    }
}
