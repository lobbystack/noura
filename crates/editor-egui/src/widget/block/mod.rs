use std::cell::RefCell;
use std::sync::{Arc, Mutex, RwLock};

use crate::widget::utils::wrap_layout::Format;

use comrak::nodes::{AstNode, NodeHeading, NodeValue};
use egui::{Pos2, Ui};

use crate::MdRender;

pub(crate) mod container;
pub(crate) mod drag;
pub(crate) mod leaf;
pub(crate) mod spacing;

impl<'ast> MdRender {
    pub fn width(&self, node: &'ast AstNode<'ast>) -> f32 {
        let parent = || node.parent().unwrap();
        let parent_width = || self.width(parent());
        // A container shifts its content by one indent unit iff it paints
        // a gutter column.
        let parent_indent = || {
            if self.is_gutter_level(parent()) {
                self.layout.indent
            } else {
                0.0
            }
        };
        // Clamp at 0: deeply nested containers at narrow doc widths
        // can drive `parent_width - parent_indent` negative; `show_block`
        // / `height` bail separately when width is genuinely too small.
        let indented_width = || (parent_width() - parent_indent()).max(0.0);

        let value = &node.data.borrow().value;
        let sp = &node.data.borrow().sourcepos;
        match value {
            NodeValue::FrontMatter(_) => self.width,
            NodeValue::Raw(_) => unreachable!("can only be created programmatically"),

            // container_block
            NodeValue::Alert(_) => indented_width(),
            NodeValue::BlockQuote => indented_width(),
            NodeValue::DescriptionItem(_) => unimplemented!("extension disabled"),
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => self.width,
            NodeValue::FootnoteDefinition(_) => indented_width(),
            NodeValue::Item(_) => indented_width(),
            NodeValue::List(_) => indented_width(), // indentation handled by items
            NodeValue::MultilineBlockQuote(_) => unimplemented!("extension disabled"),
            NodeValue::Table(_) => indented_width(),
            NodeValue::TableRow(_) => indented_width(),
            NodeValue::TaskItem(_) => indented_width(),

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
            | NodeValue::WikiLink(_) => unimplemented!("not a block: {} {:?}", sp, value),

            // leaf_block
            NodeValue::CodeBlock(_) => indented_width(),
            NodeValue::DescriptionDetails => unimplemented!("extension disabled"),
            NodeValue::DescriptionTerm => unimplemented!("extension disabled"),
            NodeValue::Heading(_) => indented_width(),
            NodeValue::HtmlBlock(_) => indented_width(),
            NodeValue::Paragraph => indented_width(),
            NodeValue::TableCell => self.width_table_cell(node),
            NodeValue::ThematicBreak => indented_width(),
        }
    }

    pub fn height(&self, node: &'ast AstNode<'ast>) -> f32 {
        if let Some(cached) = self.get_cached_node_height(node) {
            return cached;
        }

        // Block too narrow to fit anything meaningful: render nothing.
        // `show_block` short-circuits on the same condition, so this
        // matches what gets painted.
        if self.width(node) < self.layout.row_height {
            self.set_cached_node_height(node, 0.0);
            return 0.0;
        }

        // hide folded nodes only if they are not revealed
        if self.hidden_by_fold(node) {
            return 0.;
        }

        let value = &node.data.borrow().value;
        let sp = &node.data.borrow().sourcepos;
        let height = match value {
            NodeValue::FrontMatter(_) => self.height_front_matter(node),
            NodeValue::Raw(_) => unreachable!("can only be created programmatically"),

            // container_block
            NodeValue::Alert(node_alert) => self.height_alert(node, node_alert),
            NodeValue::BlockQuote => self.height_block_quote(node),
            NodeValue::DescriptionItem(_) => unimplemented!("extension disabled"),
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => self.height_document(node),
            NodeValue::FootnoteDefinition(_) => self.height_footnote_definition(node),
            NodeValue::Item(_) => self.height_item(node),
            NodeValue::List(_) => self.block_children_height(node),
            NodeValue::MultilineBlockQuote(_) => unimplemented!("extension disabled"),
            NodeValue::Table(_) => self.height_table(node),
            NodeValue::TableRow(_) => self.height_table_row(node),
            NodeValue::TaskItem(_) => self.height_task_item(node),

            // inline
            NodeValue::Subtext
            | NodeValue::Code(_)
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
            | NodeValue::Superscript
            | NodeValue::Text(_)
            | NodeValue::Underline
            | NodeValue::WikiLink(_) => unimplemented!("not a block: {} {:?}", sp, value),

            // leaf_block
            NodeValue::CodeBlock(node_code_block) => self.height_code_block(node, node_code_block),
            NodeValue::DescriptionDetails => unimplemented!("extension disabled"),
            NodeValue::DescriptionTerm => unimplemented!("extension disabled"),
            NodeValue::Heading(NodeHeading { level, setext, .. }) => {
                self.height_heading(node, *level, *setext)
            }
            NodeValue::HtmlBlock(_) => self.height_html_block(node),
            NodeValue::Paragraph => self.height_paragraph(node),
            NodeValue::TableCell => self.height_table_cell(node),
            NodeValue::ThematicBreak => self.height_thematic_break(),
        };

        self.set_cached_node_height(node, height);

        height
    }

    /// Cheap height estimate keyed on char count × presumed char
    /// width, no cosmic-text shaping. Drifts from precise layout —
    /// safe for off-screen sizing (scrollbar) only; visible content
    /// must use [`Self::height`].
    pub fn height_approx(&self, node: &'ast AstNode<'ast>) -> f32 {
        if let Some(cached) = self.get_cached_node_height_approx(node) {
            return cached;
        }
        if self.hidden_by_fold(node) {
            self.set_cached_node_height_approx(node, 0.);
            return 0.;
        }
        if self.width(node) < self.layout.row_height {
            self.set_cached_node_height_approx(node, 0.);
            return 0.;
        }
        let value = &node.data.borrow().value;
        let height = match value {
            NodeValue::Document
            | NodeValue::List(_)
            | NodeValue::BlockQuote
            | NodeValue::Item(_)
            | NodeValue::TaskItem(_)
            | NodeValue::Alert(_)
            | NodeValue::Table(_)
            | NodeValue::TableRow(_)
            | NodeValue::FootnoteDefinition(_) => {
                let mut total = 0.;
                for child in node.children() {
                    total += self.block_pre_spacing_height_approx(child);
                    total += self.height_approx(child);
                    total += self.block_post_spacing_height_approx(child);
                }
                total
            }
            NodeValue::Paragraph | NodeValue::Heading(_) | NodeValue::TableCell => {
                let row_height = self.row_height(node);
                let width = self.width(node).max(row_height);
                let mut chars = 0usize;
                // Approx doesn't shape inline content, so track image
                // height separately and `max` it in at the end.
                let mut image_height = 0.0f32;
                for d in node.descendants() {
                    match &d.data.borrow().value {
                        NodeValue::Text(t) => chars += t.chars().count(),
                        NodeValue::Code(c) => chars += c.literal.chars().count(),
                        NodeValue::HtmlInline(s) => chars += s.chars().count(),
                        NodeValue::Math(m) => chars += m.literal.chars().count(),
                        NodeValue::SoftBreak | NodeValue::LineBreak => chars += 1,
                        NodeValue::Image(link) => {
                            let (dims, _) = self.embed_texture_size(&link.url);
                            image_height = image_height
                                .max(self.image_size(dims, width, self.image_dims(d)).y);
                        }
                        _ => {}
                    }
                }
                let char_width = row_height * 0.5;
                let chars_per_row = (width / char_width).floor().max(1.0) as usize;
                let rows = ((chars as f32) / chars_per_row as f32).ceil().max(1.0);
                let text_height =
                    rows * row_height + (rows - 1.0).max(0.0) * self.layout.row_spacing;
                text_height.max(image_height)
            }
            NodeValue::CodeBlock(_) | NodeValue::HtmlBlock(_) => {
                let row_height = self.row_height(node);
                let n = (self.node_last_line_idx(node) - self.node_first_line_idx(node) + 1) as f32;
                n * row_height + (n - 1.0).max(0.0) * self.layout.row_spacing
            }
            NodeValue::ThematicBreak => self.height_thematic_break(),
            // Front matter lays out like a code block, whose fences show
            // with the caret in it: measured, not estimated or kept.
            NodeValue::FrontMatter(_) => return self.height_front_matter(node),
            _ => 0.,
        };
        self.set_cached_node_height_approx(node, height);
        height
    }

    /// Approx-y of the top of the last top-level block — i.e., the
    /// scroll offset at which the last block's top sits at viewport
    /// top. Function of doc + width only.
    pub fn approx_y_top_last_block(&self, root: &'ast AstNode<'ast>) -> f32 {
        let last = match root.last_child() {
            Some(l) => l,
            None => return 0.,
        };
        let mut y = 0.;
        let mut child = root.first_child();
        while let Some(c) = child {
            if std::ptr::eq(c, last) {
                break;
            }
            y += self.block_pre_spacing_height_approx(c);
            y += self.height_approx(c);
            y += self.block_post_spacing_height_approx(c);
            child = c.next_sibling();
        }
        y += self.block_pre_spacing_height_approx(last);
        y
    }

    /// Approx scroll extent — used to seed the affine widget's
    /// `max_offset` when reading persisted state.
    pub fn scroll_extent(&self, root: &'ast AstNode<'ast>, viewport_height: f32) -> f32 {
        self.approx_y_top_last_block(root) + viewport_height
    }

    pub(crate) fn show_block(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>, top_left: Pos2) {
        let ui = &mut self.node_ui(ui, node);

        // Block too narrow to fit anything meaningful: skip. `height`
        // returns 0 for the same condition so the layout stays aligned.
        if self.width(node) < self.layout.row_height {
            return;
        }

        // hide folded nodes only if they are not revealed
        if self.hidden_by_fold(node) {
            return;
        }

        let value = &node.data.borrow().value;
        let sp = &node.data.borrow().sourcepos;
        match value {
            NodeValue::FrontMatter(_) => self.show_front_matter(ui, node, top_left),
            NodeValue::Raw(_) => unreachable!("can only be created programmatically"),

            // container_block
            NodeValue::Alert(node_alert) => self.show_alert(ui, node, top_left, node_alert),
            NodeValue::BlockQuote => self.show_block_quote(ui, node, top_left),
            NodeValue::DescriptionItem(_) => unimplemented!("extension disabled"),
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => self.show_document(ui, node, top_left),
            NodeValue::FootnoteDefinition(_) => self.show_footnote_definition(ui, node, top_left),
            NodeValue::Item(_) => self.show_item(ui, node, top_left),
            NodeValue::List(_) => self.show_block_children(ui, node, top_left),
            NodeValue::MultilineBlockQuote(_) => unimplemented!("extension disabled"),
            NodeValue::Table(_) => self.show_table(ui, node, top_left),
            NodeValue::TableRow(is_header_row) => {
                self.show_table_row(ui, node, top_left, *is_header_row)
            }
            NodeValue::TaskItem(node_task_item) => {
                self.show_task_item(ui, node, top_left, node_task_item)
            }

            // inline
            NodeValue::Subtext
            | NodeValue::Code(_)
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
            | NodeValue::Superscript
            | NodeValue::Text(_)
            | NodeValue::Underline
            | NodeValue::WikiLink(_) => unimplemented!("not a block: {} {:?}", sp, value),

            // leaf_block
            NodeValue::CodeBlock(node_code_block) => {
                self.show_code_block(ui, node, top_left, node_code_block)
            }
            NodeValue::DescriptionDetails => unimplemented!("extension disabled"),
            NodeValue::DescriptionTerm => unimplemented!("extension disabled"),
            NodeValue::Heading(NodeHeading { level, setext, .. }) => {
                self.show_heading(ui, node, top_left, *level, *setext)
            }
            NodeValue::HtmlBlock(_) => self.show_html_block(ui, node, top_left),
            NodeValue::Paragraph => self.show_paragraph(ui, node, top_left),
            NodeValue::TableCell => self.show_table_cell(ui, node, top_left),
            NodeValue::ThematicBreak => self.show_thematic_break(ui, node, top_left),
        }
    }
}

/// height inputs: `[width_seq, embeds_seq, reveal_seq]`. Text-
/// change invalidation is handled wholesale by [`LayoutCache::ensure_text_consistent`]
/// before any read, so text_seq isn't part of the per-entry stamp.
pub type HeightDeps = [u64; 3];

#[derive(Default)]
pub struct LayoutCache {
    pub height: RefCell<editor_core::doc::NodeMap<editor_core::doc::NodeKey, (f32, HeightDeps)>>,
    pub height_approx:
        RefCell<editor_core::doc::NodeMap<editor_core::doc::NodeKey, (f32, HeightDeps)>>,

    /// The document's `bounds_seq` this cache was last consistent with,
    /// which moves when the text changes or a new parse of it arrives.
    /// Drives wholesale invalidation in [`Self::ensure_text_consistent`] —
    /// text changes can affect any entry (fold tag insertions move distant
    /// sibling heights, AST re-parse shifts sourcepos keys), so per-entry
    /// stamping wouldn't help. Text-derived values live in `editor-core`'s
    /// `TextCache`.
    pub text_seq: std::cell::Cell<u64>,
}

impl LayoutCache {
    /// Full clear for width/resize changes where everything must be recomputed.
    pub fn clear(&self) {
        self.height.borrow_mut().clear();
        self.height_approx.borrow_mut().clear();
    }

    /// Wipe and re-stamp if `current` differs from the cache's recorded
    /// text_seq. Called once per [`MdRender::reparse`] so reads in the
    /// rest of the frame can rely on the cache reflecting current text.
    pub fn ensure_text_consistent(&self, current: u64) {
        if self.text_seq.get() != current {
            self.clear();
            self.text_seq.set(current);
        }
    }
}

impl MdRender {
    /// Look up or shape a glyphon buffer for the given text and formatting.
    /// Delegates to the shared GlyphonCache so the same shaped buffer is reused
    /// across frames (and across widgets).
    /// Default wrap mode: word boundaries preferred, glyph boundaries as a
    /// fallback for over-wide single tokens. This is what `split_rows` reaches
    /// for first when discovering row breaks.
    pub fn upsert_glyphon_buffer(
        &self,
        text: &str,
        font_size: f32,
        line_height: f32,
        width: f32,
        format: &Format,
    ) -> Arc<RwLock<glyphon::Buffer>> {
        self.upsert_glyphon_buffer_inner(
            text,
            font_size,
            line_height,
            width,
            format,
            glyphon::Wrap::WordOrGlyph,
        )
    }

    /// Glyph-level wrap. Used as a fallback by `split_rows` when
    /// `WordOrGlyph` produces a layout run wider than the wrap width — a
    /// known cosmic-text quirk on some bold mixed-script content. Glyph
    /// mode breaks at any glyph boundary, which means mid-word splits, but
    /// that's better than overflowing a cell.
    pub fn upsert_glyphon_buffer_glyph(
        &self,
        text: &str,
        font_size: f32,
        line_height: f32,
        width: f32,
        format: &Format,
    ) -> Arc<RwLock<glyphon::Buffer>> {
        self.upsert_glyphon_buffer_inner(
            text,
            font_size,
            line_height,
            width,
            format,
            glyphon::Wrap::Glyph,
        )
    }

    /// Like [`Self::upsert_glyphon_buffer`] but with `Wrap::None`. Use for
    /// already-split text when you want a stable single-row shape; the
    /// wrapped variant's break-point decisions vary subtly with input
    /// context (a known cosmic-text quirk), so a piece chosen by one call
    /// may re-wrap differently on a second call at the same width.
    pub fn upsert_glyphon_buffer_unwrapped(
        &self,
        text: &str,
        font_size: f32,
        line_height: f32,
        width: f32,
        format: &Format,
    ) -> Arc<RwLock<glyphon::Buffer>> {
        self.upsert_glyphon_buffer_inner(
            text,
            font_size,
            line_height,
            width,
            format,
            glyphon::Wrap::None,
        )
    }

    fn upsert_glyphon_buffer_inner(
        &self,
        text: &str,
        font_size: f32,
        line_height: f32,
        width: f32,
        format: &Format,
        wrap_mode: glyphon::Wrap,
    ) -> Arc<RwLock<glyphon::Buffer>> {
        let font_system = self
            .ctx
            .data(|d| d.get_temp::<Arc<Mutex<glyphon::FontSystem>>>(egui::Id::NULL))
            .unwrap();
        let glyphon_cache = self
            .ctx
            .data(|d| {
                d.get_temp::<Arc<Mutex<crate::widgets::glyphon_cache::GlyphonCache>>>(
                    egui::Id::NULL,
                )
            })
            .unwrap();

        let ppi = self.ctx.pixels_per_point();
        let font_size = font_size * ppi;
        let line_height = line_height * ppi;
        let width = width * ppi;

        use crate::widgets::glyphon_cache::*;
        // Fold wrap mode into the cache key via low-bit nudges to width.
        // Three modes need three distinct keys — a sub-ULP perturbation of
        // an already-quantized pixel value, harmless to layout but enough
        // to separate cache entries.
        let width_bits = match wrap_mode {
            glyphon::Wrap::WordOrGlyph => width.to_bits(),
            glyphon::Wrap::None => width.to_bits() ^ 1,
            glyphon::Wrap::Glyph => width.to_bits() ^ 2,
            _ => width.to_bits() ^ 3,
        };
        let key = GlyphonCacheKey::single(
            text,
            format.family.glyphon_cache_family(),
            format.bold,
            format.italic,
            Some(format.color.to_array()),
            font_size.to_bits(),
            line_height.to_bits(),
            width_bits,
        );

        let fs = Arc::clone(&font_system);
        let mut cache = glyphon_cache.lock().unwrap();
        cache.get_or_shape(key, move || {
            let attrs = glyphon::Attrs::new()
                .family(format.family.glyphon_family())
                .weight(if format.bold {
                    glyphon::Weight::BOLD
                } else {
                    glyphon::Weight::NORMAL
                })
                .style(if format.italic {
                    glyphon::Style::Italic
                } else {
                    glyphon::Style::Normal
                });
            let metrics = glyphon::Metrics::new(font_size, line_height);
            let mut guard = fs.lock().unwrap();
            let mut b = glyphon::Buffer::new(&mut guard, metrics);
            b.set_size(Some(width), None);
            // Default cosmic-text tab stop is 8 char-widths; 4 reads
            // closer to a code-editor tab.
            b.set_tab_width(4);
            b.set_text(text, &attrs, glyphon::Shaping::Advanced, None);
            b.set_wrap(wrap_mode);
            b.shape_until_scroll(&mut guard, false);
            b
        })
    }
}

impl<'ast> MdRender {
    fn height_deps(&self) -> HeightDeps {
        [self.width_seq, self.embeds.seq(), self.reveal_seq]
    }

    /// An estimate counts characters rather than laying text out, so the
    /// syntax the caret reveals doesn't change it; only a fold the reveal
    /// state opens does. Without fold tags, moving the caret keeps every
    /// estimate.
    fn approx_deps(&self) -> HeightDeps {
        let reveal = if self.bounds.has_fold_tags {
            self.reveal_seq
        } else {
            0
        };
        [self.width_seq, self.embeds.seq(), reveal]
    }

    pub fn get_cached_node_height(&self, node: &'ast AstNode<'ast>) -> Option<f32> {
        let key = editor_core::MdDoc::pack_node_key(node);
        let deps = self.height_deps();
        let cache = self.layout_cache.height.borrow();
        let (v, stamps) = cache.get(&key)?;
        (*stamps == deps).then_some(*v)
    }

    pub fn set_cached_node_height(&self, node: &'ast AstNode<'ast>, height: f32) {
        let key = editor_core::MdDoc::pack_node_key(node);
        let deps = self.height_deps();
        self.layout_cache
            .height
            .borrow_mut()
            .insert(key, (height, deps));
    }

    pub fn get_cached_node_height_approx(&self, node: &'ast AstNode<'ast>) -> Option<f32> {
        let key = editor_core::MdDoc::pack_node_key(node);
        let deps = self.approx_deps();
        let cache = self.layout_cache.height_approx.borrow();
        let (v, stamps) = cache.get(&key)?;
        (*stamps == deps).then_some(*v)
    }

    pub fn set_cached_node_height_approx(&self, node: &'ast AstNode<'ast>, height: f32) {
        let key = editor_core::MdDoc::pack_node_key(node);
        let deps = self.approx_deps();
        self.layout_cache
            .height_approx
            .borrow_mut()
            .insert(key, (height, deps));
    }
}
