use comrak::nodes::{AstNode, NodeTaskItem};
use egui::{CursorIcon, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::TouchTarget;
use crate::palette::ThemeExt;
use crate::{Event, MdRender};

impl<'ast> MdRender {
    pub fn height_task_item(&self, node: &'ast AstNode<'ast>) -> f32 {
        self.height_item(node)
    }

    pub fn show_task_item(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        node_task_item: &NodeTaskItem,
    ) {
        let theme = self.ctx.get_lb_theme();
        let maybe_check = node_task_item.symbol;
        let checked = maybe_check.is_some();

        let first_line = self.node_first_line(node);
        let row_height = self.node_line_row_height(node, first_line);
        // Inflated when an image sits on the first row, so the
        // checkbox centers on the visual row.
        let annotation_row_height = self
            .first_content_row_height_inflated(node)
            .unwrap_or(row_height);

        let annotation_size = Vec2 {
            x: self.layout.indent,
            y: annotation_row_height,
        };
        let annotation_space = Rect::from_min_size(top_left, annotation_size);
        // when revealed, the raw `- [ ]` marker occupies this column instead
        if !self.reveal_line(node, first_line) {
            let mut checkbox_space = Rect::from_center_size(
                annotation_space.center(),
                Vec2::splat(annotation_space.size().min_elem()),
            );
            let border_radius = 2.0;

            let checkbox_id = ui.scope_id().with(self.node_range(node));

            // allocate a slightly larger clickable space that makes the checkbox
            // easier to tap without overflowing the annotation space horizontally
            // or overlapping with another row vertically.
            let extra_width = (annotation_space.width() - checkbox_space.width()) / 2.;
            let extra_height = self.layout.row_spacing / 2.;
            let clickable_space = checkbox_space.expand(extra_width.min(extra_height));

            // On touch the checkbox still toggles (click) but doesn't drag —
            // reorder is the body's long-press (see `detect_touch_reorder`).
            let sense = if self.readonly {
                Sense::hover()
            } else if self.touch_mode {
                Sense::click()
            } else {
                Sense::click_and_drag()
            };
            let checkbox_response =
            // ui.scope_id().with() instead of Id::unique() so two views of the same document
            // get distinct checkbox IDs
            ui.interact(clickable_space, checkbox_id, sense);
            let toggle = Event::ToggleTask {
                node: self.node_range(node),
            };
            if checkbox_response.clicked() {
                self.render_events.push(toggle.clone());
            }
            if checkbox_response.hovered() && !self.readonly {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                checkbox_space = checkbox_space.expand(0.5);
            }
            if !self.readonly {
                // A finger's target: the gutter beside the box and the row's
                // height, stopping short of the item's text.
                let target = Rect::from_min_max(
                    clickable_space.min - Vec2::new(11.0, 4.0),
                    clickable_space.max + Vec2::new(4.0, 4.0),
                );
                self.touch_targets.push((target, TouchTarget::Tap(toggle)));
            }
            // Click toggles; drag starts a reorder (`click_and_drag`
            // above distinguishes the two).
            self.handle_item_drag_resp(ui, node, &checkbox_response);

            let how_on = ui.ctx().animate_value_with_time(
                checkbox_id.with("animation"),
                if checked { 1.0 } else { 0.0 },
                0.2,
            );

            // draw rect background for checkbox
            ui.painter().rect(
                checkbox_space.shrink(2. * how_on), // anticipation for the main animation
                border_radius,
                theme
                    .bg()
                    .get_color(theme.prefs().primary)
                    .gamma_multiply(0.1),
                Stroke::new(0.5_f32, theme.fg().get_color(theme.prefs().primary)),
                StrokeKind::Inside,
            );

            // animate filled in rect for checked state
            ui.painter().rect_filled(
                checkbox_space.expand2(checkbox_space.size() * how_on - checkbox_space.size()),
                border_radius,
                theme.fg().get_color(theme.prefs().primary),
            );

            // draw check
            let check_space = checkbox_space.shrink(4.5);
            ui.painter().add(Shape::line(
                vec![
                    egui::pos2(check_space.left(), check_space.center().y),
                    egui::pos2(
                        check_space.center().x - check_space.width() / 7.0,
                        check_space.bottom(),
                    ),
                    egui::pos2(check_space.right(), check_space.top()),
                ],
                Stroke::new(
                    2_f32,
                    theme
                        .fg()
                        .get_color(crate::palette::Palette::White)
                        .linear_multiply(how_on),
                ),
            ));
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
            let width = self.width(node);
            let result = self.compute_section_layout_new(
                line_content,
                width,
                self.layout.row_height,
                self.text_format_document(),
            );
            self.show_wrap_layout(ui, top_left + self.layout.indent * Vec2::X, &result);
            self.show_block_line_prefixes(
                ui,
                node,
                line,
                top_left + self.layout.indent * Vec2::X,
                row_height,
            );
            false
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
