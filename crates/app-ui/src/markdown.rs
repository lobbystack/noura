//! Markdown that plugins hand Noura, drawn with egui widgets in the theme's
//! type scale. It parses with comrak, the editor's parser.
//!
//! Images show as their description and never load, so a plugin can't
//! reach a network host or a local file it didn't declare through an image
//! address. HTML shows as its text. Links open in the browser through
//! `app-core`, which opens only web addresses.

use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue},
};
use egui::{Frame, Margin, RichText, Stroke, Ui};

use crate::theme::{BODY_SIZE, Palette, TITLE_SIZE, radius};

/// The deepest that quotes and lists nest before their content shows as
/// plain paragraphs, which keeps drawing from recursing without bound.
const MAX_NESTING: usize = 16;

/// A block of a Markdown document.
enum Block {
    Heading(u8, Vec<Run>),
    Paragraph(Vec<Run>),
    Code(String),
    Quote(Vec<Block>),
    List {
        /// The first item's number, for a numbered list.
        start: Option<usize>,
        items: Vec<Vec<Block>>,
    },
    Rule,
}

/// A run of text with one style.
#[derive(Default, Clone)]
struct Run {
    text: String,
    strong: bool,
    emphasis: bool,
    code: bool,
    strike: bool,
    /// Where a link goes.
    link: Option<String>,
}

/// Draw `markdown`. Returns the address of a link that was clicked.
pub(crate) fn show(ui: &mut Ui, markdown: &str) -> Option<String> {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, markdown, &options());
    let blocks = blocks(root, 0);
    let mut clicked = None;
    draw_blocks(ui, &blocks, &mut clicked);
    clicked
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options
}

fn blocks<'a>(node: &'a AstNode<'a>, depth: usize) -> Vec<Block> {
    node.children()
        .flat_map(|child| block(child, depth))
        .collect()
}

fn block<'a>(node: &'a AstNode<'a>, depth: usize) -> Vec<Block> {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Heading(heading) => vec![Block::Heading(heading.level, runs(node))],
        NodeValue::Paragraph => vec![Block::Paragraph(runs(node))],
        NodeValue::CodeBlock(code) => vec![Block::Code(code.literal.trim_end().to_owned())],
        NodeValue::HtmlBlock(html) => vec![Block::Code(html.literal.trim_end().to_owned())],
        NodeValue::ThematicBreak => vec![Block::Rule],
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) | NodeValue::Alert(_)
            if depth < MAX_NESTING =>
        {
            vec![Block::Quote(blocks(node, depth + 1))]
        }
        NodeValue::List(list) if depth < MAX_NESTING => vec![Block::List {
            start: (list.list_type == ListType::Ordered).then_some(list.start),
            items: node
                .children()
                .map(|item| blocks(item, depth + 1))
                .collect(),
        }],
        _ if node.first_child().is_some() && depth < MAX_NESTING => blocks(node, depth + 1),
        _ => {
            let runs = runs(node);
            if runs.iter().all(|run| run.text.trim().is_empty()) {
                Vec::new()
            } else {
                vec![Block::Paragraph(runs)]
            }
        }
    }
}

/// The styled runs of the inline content under `node`.
fn runs<'a>(node: &'a AstNode<'a>) -> Vec<Run> {
    let mut runs = Vec::new();
    for child in node.children() {
        inline(child, &Run::default(), &mut runs);
    }
    runs
}

fn inline<'a>(node: &'a AstNode<'a>, style: &Run, runs: &mut Vec<Run>) {
    let value = node.data.borrow().value.clone();
    let mut push = |text: &str, style: &Run| {
        runs.push(Run {
            text: text.to_owned(),
            ..style.clone()
        });
    };
    match value {
        NodeValue::Text(text) => push(&text, style),
        NodeValue::Code(code) => push(
            &code.literal,
            &Run {
                code: true,
                ..style.clone()
            },
        ),
        NodeValue::HtmlInline(text) | NodeValue::Raw(text) => push(&text, style),
        NodeValue::SoftBreak => push(" ", style),
        NodeValue::LineBreak => push("\n", style),
        NodeValue::Image(_) => {
            // The description, never the image itself.
            let mut alt = Vec::new();
            for child in node.children() {
                inline(child, &Run::default(), &mut alt);
            }
            let text: String = alt.into_iter().map(|run| run.text).collect();
            let text = if text.trim().is_empty() {
                "Image".to_owned()
            } else {
                text
            };
            push(
                &text,
                &Run {
                    emphasis: true,
                    link: None,
                    ..style.clone()
                },
            );
        }
        other => {
            let mut style = style.clone();
            match other {
                NodeValue::Strong => style.strong = true,
                NodeValue::Emph => style.emphasis = true,
                NodeValue::Strikethrough => style.strike = true,
                NodeValue::Link(link) => style.link = Some(link.url),
                _ => {}
            }
            for child in node.children() {
                inline(child, &style, runs);
            }
        }
    }
}

fn draw_blocks(ui: &mut Ui, blocks: &[Block], clicked: &mut Option<String>) {
    for block in blocks {
        draw_block(ui, block, clicked);
    }
}

fn draw_block(ui: &mut Ui, block: &Block, clicked: &mut Option<String>) {
    let palette = Palette::of(ui.ctx());
    match block {
        Block::Heading(level, runs) => {
            let size = match level {
                1 => TITLE_SIZE + 6.0,
                2 => TITLE_SIZE + 2.0,
                _ => TITLE_SIZE,
            };
            draw_runs(ui, runs, size, true, clicked);
        }
        Block::Paragraph(runs) => draw_runs(ui, runs, BODY_SIZE, false, clicked),
        Block::Code(code) => {
            Frame::new()
                .fill(palette.muted)
                .corner_radius(radius(0.6))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(code).monospace());
                });
        }
        Block::Quote(blocks) => {
            Frame::new()
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(radius(0.6))
                .inner_margin(Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    draw_blocks(ui, blocks, clicked);
                });
        }
        Block::List { start, items } => {
            for (index, item) in items.iter().enumerate() {
                let marker = match start {
                    Some(start) => format!("{}.", start + index),
                    None => "\u{2022}".to_owned(),
                };
                ui.horizontal_top(|ui| {
                    ui.label(marker);
                    ui.vertical(|ui| draw_blocks(ui, item, clicked));
                });
            }
        }
        Block::Rule => {
            ui.separator();
        }
    }
    ui.add_space(4.0);
}

fn draw_runs(ui: &mut Ui, runs: &[Run], size: f32, heading: bool, clicked: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for run in runs {
            let mut text = RichText::new(&run.text).size(size);
            if run.strong || heading {
                text = text.strong();
            }
            if run.emphasis {
                text = text.italics();
            }
            if run.code {
                text = text.monospace();
            }
            if run.strike {
                text = text.strikethrough();
            }
            match &run.link {
                Some(url) => {
                    if ui.link(text).on_hover_text(url).clicked() {
                        *clicked = Some(url.clone());
                    }
                }
                None => {
                    ui.label(text);
                }
            }
        }
    });
}
