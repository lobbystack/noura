use comrak::nodes::AstNode;
use egui::text::LayoutJob;
use egui::{FontFamily, FontId, Pos2, Rect, TextFormat, Ui, Vec2};

use crate::MdRender;
use crate::widget::utils::wrap_layout::Format;

use crate::palette::ThemeExt as _;

impl<'ast> MdRender {
    pub fn text_format_footnote_definition(&self, parent: &AstNode<'_>) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            color: self.ctx.get_lb_theme().neutral_fg_secondary(),
            ..parent_text_format
        }
    }

    pub fn height_footnote_definition(&self, node: &'ast AstNode<'ast>) -> f32 {
        self.height_item(node)
    }

    pub fn show_footnote_definition(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
    ) {
        let annotation_size = Vec2 {
            x: self.layout.indent,
            y: self.layout.row_height,
        };
        let annotation_space = Rect::from_min_size(top_left, annotation_size);

        // when revealed, the raw `[^x]:` marker occupies this column instead
        if !self.reveal_line(node, self.node_first_line(node)) {
            let color = self.ctx.get_lb_theme().neutral_fg_secondary();
            let text = format!("{}.", self.definition_number(node));
            let layout_job = LayoutJob::single_section(
                text,
                TextFormat {
                    font_id: FontId::new(self.layout.row_height, FontFamily::Monospace),
                    color,
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(layout_job));
            ui.painter()
                .galley(annotation_space.left_top(), galley, Default::default());
        }

        // debug
        // ui.painter()
        //     .rect_stroke(annotation_space, 2., egui::Stroke::new(1., self.theme.fg().blue));

        // debug
        // println!("-- line prefix lens --");
        // let range = self.node_range(node);
        // for line in self.range_lines(range) {
        //     let source_line_idx =
        //         crate::bounds::RangesExt::find_containing(
        //             &self.bounds.source_lines,
        //             line.start(),
        //             true,
        //             true,
        //         )
        //         .start();
        //     println!("line {} prefix len: {:?}", source_line_idx, self.line_prefix_len(node, line));
        // }

        top_left.x += annotation_space.width();
        self.show_block_children(ui, node, top_left);
    }
}
