use comrak::nodes::{AstNode, ListType, NodeList, NodeValue};
use egui::{Pos2, Rect, Ui, Vec2};

use crate::MdRender;
use crate::TextBufferArea;
use crate::widget::utils::wrap_layout::{BufferExt as _, FontFamily};

use crate::palette::ThemeExt as _;

// https://github.github.com/gfm/#list-items
impl<'ast> MdRender {
    pub fn height_item(&self, node: &'ast AstNode<'ast>) -> f32 {
        let any_children = node.children().next().is_some();
        if any_children {
            self.block_children_height(node)
        } else {
            let line = self.node_first_line(node);
            let line_content = self.line_content(node, line);
            let width = self.width(node) - self.layout.indent;
            self.compute_section_layout_new(
                line_content,
                width,
                self.layout.row_height,
                self.text_format_syntax(),
            )
            .height
        }
    }

    pub fn show_item(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>, top_left: Pos2) {
        let first_line = self.node_first_line(node);
        let row_height = self.node_line_row_height(node, first_line);
        // `Some` when an image inflates the first content row — marker
        // centers on the inflated row instead of the text row's top.
        let inflated = self.first_content_row_height_inflated(node);
        let annotation_row_height = inflated.unwrap_or(row_height);

        let parent = node.parent().unwrap();
        let NodeValue::List(node_list) = parent.data.borrow().value else {
            unreachable!("items always have list parents")
        };
        let NodeList {
            list_type, start, ..
        } = node_list;

        let annotation_size = Vec2 {
            x: self.layout.indent,
            y: annotation_row_height,
        };
        let annotation_space = Rect::from_min_size(top_left, annotation_size);

        // Desktop drag handle (bullet/number is non-interactive). On touch
        // the body must own the press instead, so a pan scrolls and a hold
        // arms a reorder (see `MdEdit::detect_touch_reorder`).
        let drag_id = ui.scope_id().with(("item_drag", self.node_range(node)));
        let sense = if self.touch_mode {
            egui::Sense::hover()
        } else {
            egui::Sense::drag()
        };
        let drag_resp = ui.interact(annotation_space, drag_id, sense);
        self.handle_item_drag_resp(ui, node, &drag_resp);

        let annotation_color = self.ctx.get_lb_theme().neutral_fg_secondary();

        // when revealed, the raw marker occupies this column instead
        if !self.reveal_line(node, first_line) {
            match list_type {
                ListType::Bullet => {
                    ui.painter().circle_filled(
                        annotation_space.center(),
                        self.layout.bullet_radius * row_height / self.layout.row_height,
                        annotation_color,
                    );
                }
                ListType::Ordered => {
                    let mut sibling_index = 0usize;
                    let mut prev = node.previous_sibling();
                    while let Some(p) = prev {
                        sibling_index += 1;
                        prev = p.previous_sibling();
                    }
                    let number = start + sibling_index;

                    // Trailing space included, as in source, so this matches
                    // the marker the reveal path draws from the source bytes.
                    let text = format!("{number}. ");
                    let ppi = self.ctx.pixels_per_point();
                    let [r, g, b, a] = annotation_color.to_array();
                    let color = glyphon::Color::rgba(r, g, b, a);
                    let mut format = self.text_format_document();
                    format.family = FontFamily::Mono;
                    format.color = annotation_color;
                    let afs = self.layout.annotation_font_size;
                    let buffer = self.upsert_glyphon_buffer(&text, afs, afs, f32::MAX, &format);
                    let size = buffer.read().unwrap().shaped_size(ppi);

                    // Right-align to the content edge; baseline-shift y
                    // so toggling reveal doesn't jump the number. On an
                    // image-inflated row, baseline-alignment would park
                    // it at the bottom corner — center vertically.
                    let x = annotation_space.right() - size.x;
                    let y = if inflated.is_some() {
                        annotation_space.top() + (annotation_row_height - afs) / 2.0
                    } else {
                        annotation_space.top() + (row_height - afs) * 0.8
                    };
                    let rect = Rect::from_min_size(Pos2::new(x, y), size);
                    self.text_areas.push(TextBufferArea::new(
                        buffer,
                        rect,
                        color,
                        ui.ctx(),
                        ui.clip_rect(),
                    ));
                }
            }
        }

        let any_children = node.children().next().is_some();
        let hovered = if any_children {
            self.show_block_children(ui, node, top_left + self.layout.indent * Vec2::X);

            // todo: proper hit-testing (this ignores anything covering the space)
            let children_height = self.block_children_height(node);
            let children_space =
                Rect::from_min_size(top_left, Vec2::new(self.width(node), children_height));
            children_space.contains(ui.input(|i| i.pointer.latest_pos().unwrap_or_default()))
        } else {
            let line = self.node_first_line(node);
            let line_content = self.line_content(node, line);
            let width = self.width(node) - self.layout.indent;
            let result = self.compute_section_layout_new(
                line_content,
                width,
                self.layout.row_height,
                self.text_format_syntax(),
            );
            self.show_wrap_layout(ui, top_left + self.layout.indent * Vec2::X, &result);
            self.show_block_line_prefixes(
                ui,
                node,
                line,
                top_left + self.layout.indent * Vec2::X,
                row_height,
            );
            let item_rect =
                Rect::from_min_size(top_left, Vec2::new(self.width(node), result.height));
            item_rect.contains(ui.input(|i| i.pointer.latest_pos().unwrap_or_default()))
        };

        // fold button
        // todo: proper hit-testing (this ignores anything covering the space)
        let pointer = ui.input(|i| i.pointer.latest_pos().unwrap_or_default());

        let (fold_button_size, fold_button_icon_size, fold_button_space) =
            Self::fold_button_size_icon_size_space(top_left, row_height, self.layout.indent);
        let show_fold_button = self.interactive
            && !self.painting_drag_float // chrome shouldn't travel with the drag card
            && !self.reveal_line(node, first_line) // the revealed marker occupies the gutter
            && (self.touch_mode
                || hovered
                || fold_button_space.contains(pointer)
                || annotation_space.contains(pointer)
                || self.fold(node).is_some()
                || self.selected_fold_item(node));
        if !show_fold_button {
            return;
        }

        self.show_fold_button(
            ui,
            node,
            (fold_button_size, fold_button_icon_size, fold_button_space),
            self.item_fold_reveal(node),
        );
    }
}
