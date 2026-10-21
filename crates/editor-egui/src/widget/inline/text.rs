use comrak::nodes::AstNode;
use editor_core::callouts::CalloutFamily;
use editor_core::offset_types::{Grapheme, RangeExt};
use editor_core::spans::{InlineSpan, SpanKind};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{EmbedSpec, Format, Layout};

impl<'ast> MdRender {
    pub fn layout_text(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let node_range = self.node_range(node).trim(&range);
        if node_range.is_empty() {
            return;
        }
        let text_format = self.text_format(node);

        // Split the text where Obsidian-style syntax starts and ends.
        let spans: Vec<&InlineSpan> = self.spans_in(node_range).collect();
        let mut cuts = vec![node_range.start(), node_range.end()];
        for span in &spans {
            cuts.push(span.range.start());
            cuts.push(span.range.end());
            for syntax in &span.syntax {
                cuts.push(syntax.start());
                cuts.push(syntax.end());
            }
        }
        if let Some(search_range) = self.search_range {
            cuts.push(search_range.start());
            cuts.push(search_range.end());
        }
        cuts.retain(|cut| node_range.contains(*cut, true, true));
        cuts.sort();
        cuts.dedup();

        for piece in cuts.windows(2).map(|pair| (pair[0], pair[1])) {
            // Syntax hidden away from the caret draws as its label, or as
            // nothing, once, from where it starts.
            if let Some((span, syntax)) = spans.iter().find_map(|span| {
                span.syntax
                    .iter()
                    .find(|syntax| syntax.contains_range(&piece, true, true))
                    .filter(|_| !self.span_revealed(span))
                    .map(|syntax| (*span, *syntax))
            }) {
                if piece.start() == syntax.start().max(range.start()) {
                    let hidden = syntax.trim(&range);
                    let label = match &span.kind {
                        SpanKind::CalloutMarker { label, family } => {
                            layout.push_override(
                                hidden,
                                label,
                                self.text_format_callout_label(&text_format, *family),
                            );
                            true
                        }
                        // An embed that isn't drawn as a block draws as a
                        // link named by its alias or its target.
                        SpanKind::Embed { target, alias, .. } => {
                            let state = self.link_state_for_wikilink(target);
                            let format = self.text_format_link(node.parent().unwrap(), state);
                            layout.push_override(
                                hidden,
                                alias.as_deref().unwrap_or(target),
                                format,
                            );
                            true
                        }
                        // An image embed draws as the image, or as a link
                        // named by its target until its size is known.
                        SpanKind::ImageEmbed { target, width } => {
                            match self.image_embed_size(node, target, *width) {
                                Some(size) if hidden == syntax => {
                                    let (_, kind) = self.embed_texture_size(target);
                                    layout.push_embed(EmbedSpec {
                                        advance: size.x,
                                        ascent: size.y,
                                        descent: 0.0,
                                        source_range: syntax,
                                        url: target.clone(),
                                        kind,
                                    });
                                }
                                _ => {
                                    let state = self.link_state_for_wikilink(target);
                                    let format =
                                        self.text_format_link(node.parent().unwrap(), state);
                                    layout.push_override(hidden, target, format);
                                }
                            }
                            true
                        }
                        _ => false,
                    };
                    if !label {
                        layout.push_override(hidden, "", self.text_format_syntax());
                    }
                }
                continue;
            }

            let mut format = text_format.clone();
            for span in spans
                .iter()
                .filter(|span| span.range.contains_range(&piece, true, true))
            {
                match &span.kind {
                    SpanKind::Tag => format.color = self.ctx.get_lb_theme().accent(),
                    SpanKind::BlockId => {
                        format.color = self.ctx.get_lb_theme().neutral_fg_secondary()
                    }
                    SpanKind::Comment => {
                        format.color = self.ctx.get_lb_theme().neutral_fg_secondary()
                    }
                    SpanKind::Underline => {
                        let tags = span
                            .syntax
                            .iter()
                            .any(|tag| tag.contains_range(&piece, true, true));
                        if tags {
                            format = self.text_format_syntax();
                        } else {
                            format.underline = true;
                        }
                    }
                    SpanKind::CalloutTitle { family } if !self.span_revealed(span) => {
                        format.bold = true;
                        format.color = self.callout_color(*family);
                    }
                    SpanKind::CalloutTitle { .. }
                    | SpanKind::CalloutMarker { .. }
                    | SpanKind::Embed { .. }
                    | SpanKind::ImageEmbed { .. } => {}
                }
            }
            if let Some(search_range) = self.search_range
                && search_range.contains_range(&piece, true, true)
                && !search_range.is_empty()
            {
                let theme = self.ctx.get_lb_theme();
                format.color = theme.fg().get_color(theme.prefs().primary);
            }
            layout.push_source(piece, &self.buffer[piece], format);
        }
    }

    /// The size an `![[image.png|width]]` embed in `node`'s block draws
    /// at, or `None` before its size is known.
    fn image_embed_size(
        &self,
        node: &'ast AstNode<'ast>,
        target: &str,
        width: Option<u32>,
    ) -> Option<egui::Vec2> {
        use crate::widget::utils::NodeValueExt as _;

        let block = node
            .ancestors()
            .skip(1)
            .find(|ancestor| ancestor.data.borrow().value.is_leaf_block())?;
        let (texture_size, _) = self.embed_texture_size(target);
        let requested = editor_core::images::ImageDims {
            width: width.map(|width| width as f32),
            height: None,
        };
        let size = self.image_size(texture_size, self.width(block), requested);
        (size.x > 0.0 && size.y > 0.0 && !self.disable_images).then_some(size)
    }

    /// The color a callout of `family` draws its bar, label and title in.
    pub fn callout_color(&self, family: CalloutFamily) -> egui::Color32 {
        let fg = self.ctx.get_lb_theme().fg();
        match family {
            CalloutFamily::Note => fg.blue,
            CalloutFamily::Tip => fg.cyan,
            CalloutFamily::Success => fg.green,
            CalloutFamily::Question | CalloutFamily::Warning => fg.yellow,
            CalloutFamily::Danger => fg.red,
            CalloutFamily::Example => fg.magenta,
            CalloutFamily::Quote => self.ctx.get_lb_theme().neutral_fg_secondary(),
        }
    }

    fn text_format_callout_label(&self, base: &Format, family: CalloutFamily) -> Format {
        Format {
            bold: true,
            color: self.callout_color(family),
            ..base.clone()
        }
    }
}
