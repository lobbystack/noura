use comrak::nodes::{AstNode, NodeValue};
use editor_core::offset_types::{Grapheme, RangeExt as _, RangeIterExt as _};
use egui::{Pos2, Rect, Stroke, Ui, Vec2};

use crate::MdRender;
use crate::TouchTarget;
use crate::input::Event;
use crate::style::{FG_HOVER, FG_PRESS, HOVER_ANIM_SECS, Radius, ThemeExt as _, sense_click};
use crate::widget::inline::Response;
use crate::widget::utils::wrap_layout::Layout;

impl<'ast> MdRender {
    pub fn heading_row_height(&self, level: u8) -> f32 {
        self.layout.row_height
            * match level {
                6 => 1.2,
                5 => 1.4,
                4 => 1.6,
                3 => 1.8,
                2 => 2.0,
                _ => 2.4,
            }
    }

    pub fn height_heading(&self, node: &'ast AstNode<'ast>, level: u8, setext: bool) -> f32 {
        let text_height = if setext {
            self.height_setext_heading(node)
        } else {
            self.height_atx_heading(node)
        };
        text_height
            + if level == 1 {
                self.layout.block_spacing
            } else {
                0.
            }
    }

    /// Build `Layout` for a setext heading's content line (non-underline
    /// row of a setext heading). When syntax is revealed, leading
    /// indentation and the source-line prefix come through as syntax-
    /// formatted text; otherwise just the inline children.
    fn layout_setext_heading_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_line: (Grapheme, Grapheme),
        reveal: bool,
    ) -> Layout {
        let mut layout = Layout::new(node_line);
        let Some((indentation, prefix, _, postfix_whitespace, _)) =
            self.split_range(node, node_line)
        else {
            unreachable!("setext headings never have empty lines");
        };
        if reveal {
            if !indentation.is_empty() {
                layout.push_source(
                    indentation,
                    &self.buffer[indentation],
                    self.text_format_syntax(),
                );
            }
            if !prefix.is_empty() {
                layout.push_source(prefix, &self.buffer[prefix], self.text_format_syntax());
            }
        }
        self.layout_inline_children(&mut layout, node, node_line);
        if reveal && !postfix_whitespace.is_empty() {
            layout.push_source(
                postfix_whitespace,
                &self.buffer[postfix_whitespace],
                self.text_format(node),
            );
        }
        layout
    }

    // https://github.github.com/gfm/#setext-headings
    fn height_setext_heading(&self, node: &'ast AstNode<'ast>) -> f32 {
        let width = self.width(node);
        let row_height = self.row_height(node);
        let reveal = self.reveal_setext_syntax(node);
        let mut result = 0.;

        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            if line_idx < last_line_idx {
                result += self.height_setext_heading_line(node, node_line, reveal);
                result += self.layout.row_spacing;
            } else if reveal {
                // setext heading underline as its own wrap unit
                let underline = self.compute_section_layout_new(
                    node_line,
                    width,
                    row_height,
                    self.text_format_syntax(),
                );
                result += underline.height;
                result += self.layout.row_spacing;
            }
        }

        result - self.layout.row_spacing
    }

    pub fn height_setext_heading_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_line: (Grapheme, Grapheme),
        reveal: bool,
    ) -> f32 {
        let width = self.width(node);
        let row_height = self.row_height(node);
        let layout = self.layout_setext_heading_line(node, node_line, reveal);
        self.compute_layout_from(layout, width, row_height).height
    }

    /// Build `Layout` for an ATX heading line (`# heading`).
    /// Indentation + prefix `#`s + inline children + postfix
    /// whitespace, with syntax visibility driven by reveal. Empty
    /// heading falls back to rendering the whole line as syntax
    /// (Obsidian-inspired).
    fn layout_atx_heading_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_line: (Grapheme, Grapheme),
        reveal: bool,
    ) -> Layout {
        let mut layout = Layout::new(node_line);
        if let Some((indentation, prefix_range, _, postfix_range, _)) =
            self.split_range(node, node_line)
        {
            if reveal {
                if !indentation.is_empty() {
                    layout.push_source(
                        indentation,
                        &self.buffer[indentation],
                        self.text_format_syntax(),
                    );
                }
                if !prefix_range.is_empty() {
                    layout.push_source(
                        prefix_range,
                        &self.buffer[prefix_range],
                        self.text_format_syntax(),
                    );
                }
            }
            if self.infix_range(node).is_some() {
                self.layout_inline_children(&mut layout, node, node_line);
            }
            if reveal && !postfix_range.is_empty() {
                layout.push_source(
                    postfix_range,
                    &self.buffer[postfix_range],
                    self.text_format(node),
                );
            }
        } else {
            layout.push_source(
                node_line,
                &self.buffer[node_line],
                self.text_format_syntax(),
            );
        }
        layout
    }

    // https://github.github.com/gfm/#atx-headings
    fn height_atx_heading(&self, node: &'ast AstNode<'ast>) -> f32 {
        let width = self.width(node);
        let row_height = self.row_height(node);
        let line = self.node_first_line(node); // more like node_ONLY_line amirite?
        let node_line = self.node_line(node, line);
        let reveal = self.range_revealed(line, true);
        let layout = self.layout_atx_heading_line(node, node_line, reveal);
        self.compute_layout_from(layout, width, row_height).height
    }

    pub fn show_heading(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        level: u8,
        setext: bool,
    ) {
        if setext {
            self.show_setext_heading(ui, node, top_left, level);
        } else {
            self.show_atx_heading(ui, node, top_left, level);
        }

        let pointer = ui.input(|i| i.pointer.latest_pos().unwrap_or_default());
        let hovered = {
            let siblings_height = self.height(node) + self.heading_contained_siblings_height(node);
            let siblings_space =
                Rect::from_min_size(top_left, Vec2::new(self.width(node), siblings_height));

            siblings_space.contains(pointer)
        };

        // fold button
        // todo: proper hit-testing (this ignores anything covering the space)
        let first_line = self.node_first_line(node);
        let row_height = self.node_line_row_height(node, first_line);

        let (fold_button_size, fold_button_icon_size, fold_button_space) =
            Self::fold_button_size_icon_size_space(top_left, row_height, self.layout.indent);
        let show_fold_button = self.interactive
            && (self.touch_mode
                || hovered
                || fold_button_space.contains(pointer)
                || self.fold(node).is_some()
                || self.selected_block(node));
        if show_fold_button {
            self.show_fold_button(
                ui,
                node,
                (fold_button_size, fold_button_icon_size, fold_button_space),
                self.heading_fold_reveal(node),
            );
        }
    }

    // https://github.github.com/gfm/#setext-headings
    fn show_setext_heading(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
        level: u8,
    ) -> Response {
        let resp = Default::default();
        let width = self.width(node);
        let row_height = self.row_height(node);
        let reveal = self.reveal_setext_syntax(node);

        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            if line_idx < last_line_idx {
                let layout = self.layout_setext_heading_line(node, node_line, reveal);
                let result = self.compute_layout_from(layout, width, row_height);
                let h = result.height;
                self.show_wrap_layout(ui, top_left, &result);
                self.show_block_line_prefixes(ui, node, line, top_left, row_height);
                top_left.y += h;
                top_left.y += self.layout.row_spacing;
            } else if reveal {
                let underline = self.compute_section_layout_new(
                    node_line,
                    width,
                    row_height,
                    self.text_format_syntax(),
                );
                let h = underline.height;
                self.show_wrap_layout(ui, top_left, &underline);
                top_left.y += h;
                top_left.y += self.layout.row_spacing;
            }
        }

        top_left.y -= self.layout.row_spacing;
        if level == 1 {
            self.show_heading_rule(ui, top_left, width);
        }
        resp
    }

    // https://github.github.com/gfm/#atx-headings
    fn show_atx_heading(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
        level: u8,
    ) -> Response {
        let resp = Default::default();
        let width = self.width(node);
        let row_height = self.row_height(node);
        let line = self.node_first_line(node); // more like node_ONLY_line amirite?
        let node_line = self.node_line(node, line);
        let reveal = self.range_revealed(line, true);

        let layout = self.layout_atx_heading_line(node, node_line, reveal);
        let result = self.compute_layout_from(layout, width, row_height);
        let height = result.height;
        self.show_wrap_layout(ui, top_left, &result);
        self.show_block_line_prefixes(ui, node, line, top_left, row_height);

        top_left.y += height;
        if level == 1 {
            self.show_heading_rule(ui, top_left, width);
        }
        resp
    }

    fn show_heading_rule(&mut self, ui: &mut Ui, top_left: Pos2, width: f32) {
        let line_break_rect =
            Rect::from_min_size(top_left, Vec2::new(width, self.layout.block_spacing));

        ui.painter().hline(
            line_break_rect.x_range(),
            line_break_rect.center().y,
            Stroke {
                width: 1.0,
                color: self.ctx.get_lb_theme().neutral(),
            },
        );
    }

    fn heading_contained_siblings_height(&self, node: &'ast AstNode<'ast>) -> f32 {
        let NodeValue::Heading(heading) = &node.data.borrow().value else {
            panic!("heading_contained_siblings_height() invoked for non-heading")
        };
        let level = heading.level;
        let mut height_sum = 0.0;
        let mut sibling = node.next_sibling();
        while let Some(s) = sibling {
            if let NodeValue::Heading(sib_h) = &s.data.borrow().value
                && sib_h.level <= level
            {
                break;
            }
            height_sum += self.block_pre_spacing_height(s);
            height_sum += self.height(s);
            height_sum += self.block_post_spacing_height(s);
            sibling = s.next_sibling();
        }
        height_sum
    }

    pub fn fold_button_size_icon_size_space(
        top_left: Pos2,
        row_height: f32,
        indent: f32,
    ) -> (f32, f32, Rect) {
        let fold_button_size = indent * 0.8;
        let fold_button_icon_size = fold_button_size * 0.6;
        let fold_button_space = Rect::from_min_size(
            top_left + Vec2::new(-indent, (row_height - fold_button_size) / 2.),
            Vec2::splat(fold_button_size),
        );
        (fold_button_size, fold_button_icon_size, fold_button_space)
    }

    pub fn show_fold_button(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        size_icon_size_space: (f32, f32, Rect),
        fold_reveal: bool,
    ) {
        let (_size, icon_size, space) = size_icon_size_space;
        let folded = self.fold(node).is_some();
        let toggle = Event::ToggleFold {
            node: Some(self.node_range(node)),
        };
        // A finger's target: the gutter around the button, the row's height.
        let target = Rect::from_min_max(
            space.min - Vec2::new(6.0, 4.0),
            space.max + Vec2::new(6.0, 4.0),
        );
        self.touch_targets
            .push((target, TouchTarget::Tap(toggle.clone())));

        if !folded && self.foldable(node).is_none() {
            return;
        }

        // Interact on the gutter rect — do not allocate in the parent ui
        // (off-screen neighbor rows would displace the mobile toolbar, #4892).
        let range = self.node_range(node);
        let id = ui
            .scope_id()
            .with(("md_fold_btn", range.start().0, range.end().0));
        let resp = ui.interact(space, id, sense_click());
        if resp.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }

        let t = self.ctx.get_lb_theme();
        let over = resp.hovered() || ui.ctx().rect_contains_pointer(ui.layer_id(), space);
        let hover = if over {
            ui.ctx()
                .animate_bool_with_time(resp.id.with("icon_hov"), true, HOVER_ANIM_SECS)
        } else {
            let _ = ui
                .ctx()
                .animate_bool_with_time(resp.id.with("icon_hov"), false, 0.0);
            0.0
        };
        if hover > 0.0 {
            let amt = if folded && !fold_reveal {
                FG_PRESS * hover
            } else {
                FG_HOVER * hover
            };
            ui.painter().rect_filled(
                space,
                Radius::Sm.corner(),
                t.wash_toward_neutral_fg(t.neutral_bg(), amt),
            );
        }

        let color = if folded && !fold_reveal {
            t.accent()
        } else {
            t.neutral_fg_secondary()
                .lerp_to_gamma(t.neutral_fg(), hover)
        };
        // A caret pointing right when folded and down when open.
        let c = space.center();
        let r = icon_size * 0.3;
        let points = if folded {
            vec![
                Pos2::new(c.x - r * 0.5, c.y - r),
                Pos2::new(c.x + r * 0.7, c.y),
                Pos2::new(c.x - r * 0.5, c.y + r),
            ]
        } else {
            vec![
                Pos2::new(c.x - r, c.y - r * 0.5),
                Pos2::new(c.x + r, c.y - r * 0.5),
                Pos2::new(c.x, c.y + r * 0.7),
            ]
        };
        ui.painter()
            .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));

        if resp.clicked() {
            self.render_events.push(toggle);
        }
    }
}
