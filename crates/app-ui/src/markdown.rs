//! Markdown that plugins hand Noura, drawn with egui widgets in the theme's
//! type scale. It parses with comrak, the editor's parser.
//!
//! Images show as their description and never load, so a plugin can't
//! reach a network host or a local file it didn't declare through an image
//! address. HTML shows as its text. Links open in the browser through
//! `app-core`, which opens only web addresses.
//!
//! A plugin's Markdown may nest as deep as it likes, so nothing here
//! recurses once per level of the document: blocks nest at most
//! [`MAX_NESTING`] deep, and inline content is walked with a loop.

use std::sync::Arc;

use comrak::{
    Arena, Options,
    arena_tree::NodeEdge,
    nodes::{AstNode, ListType, NodeValue},
};
use egui::{
    Frame, Margin, RichText, Stroke, Ui,
    cache::{ComputerMut, FrameCache},
};

use crate::theme::{BODY_SIZE, Palette, TITLE_SIZE, radius};

/// The deepest that quotes and lists nest before their content shows as
/// plain text, which keeps parsing and drawing from recursing without
/// bound.
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

impl Run {
    fn same_style(&self, other: &Self) -> bool {
        self.strong == other.strong
            && self.emphasis == other.emphasis
            && self.code == other.code
            && self.strike == other.strike
            && self.link == other.link
    }
}

/// Parses each Markdown text once, and again only after a frame that
/// didn't draw it.
#[derive(Default)]
struct Parser;

impl ComputerMut<&str, Arc<[Block]>> for Parser {
    fn compute(&mut self, markdown: &str) -> Arc<[Block]> {
        parse(markdown).into()
    }
}

type Parsed = FrameCache<Arc<[Block]>, Parser>;

/// Draw `markdown`. Returns the address of a link that was clicked.
pub(crate) fn show(ui: &mut Ui, markdown: &str) -> Option<String> {
    let blocks = ui
        .ctx()
        .memory_mut(|memory| memory.caches.cache::<Parsed>().get(markdown).clone());
    let mut clicked = None;
    draw_blocks(ui, &blocks, &mut clicked);
    clicked
}

fn parse(markdown: &str) -> Vec<Block> {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, markdown, &options());
    blocks(root, 0)
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

/// The styled runs of the content under `node`. A loop walks it, so
/// emphasis nested a hundred thousand deep draws as one run. Blocks under
/// `node`, as in a quote nested deeper than [`MAX_NESTING`], flatten into
/// text.
fn runs<'a>(node: &'a AstNode<'a>) -> Vec<Run> {
    let mut runs = Runs::default();
    for edge in node.traverse() {
        match edge {
            NodeEdge::Start(child) if !std::ptr::eq(child, node) => runs.start(child),
            NodeEdge::End(child) if !std::ptr::eq(child, node) => runs.end(child),
            _ => {}
        }
    }
    runs.runs
}

/// The runs gathered so far, and the styles open where the walk is.
#[derive(Default)]
struct Runs {
    runs: Vec<Run>,
    strong: usize,
    emphasis: usize,
    strike: usize,
    /// The links the walk is inside, innermost last.
    links: Vec<String>,
    /// How many images the walk is inside. Their text gathers in `alt`.
    images: usize,
    alt: String,
}

impl Runs {
    fn start<'a>(&mut self, node: &'a AstNode<'a>) {
        let value = &node.data.borrow().value;
        match value {
            NodeValue::Text(text) => self.push(text, false),
            NodeValue::HtmlInline(text) | NodeValue::Raw(text) => self.push(text, false),
            NodeValue::Code(code) => self.push(&code.literal, true),
            NodeValue::CodeBlock(code) => self.push(code.literal.trim_end(), true),
            NodeValue::HtmlBlock(html) => self.push(html.literal.trim_end(), false),
            NodeValue::SoftBreak => self.push(" ", false),
            NodeValue::LineBreak => self.push("\n", false),
            NodeValue::Strong => self.strong += 1,
            NodeValue::Emph => self.emphasis += 1,
            NodeValue::Strikethrough => self.strike += 1,
            NodeValue::Link(link) => self.links.push(link.url.clone()),
            NodeValue::Image(_) => self.images += 1,
            _ => {}
        }
    }

    fn end<'a>(&mut self, node: &'a AstNode<'a>) {
        let value = &node.data.borrow().value;
        match value {
            NodeValue::Strong => self.strong = self.strong.saturating_sub(1),
            NodeValue::Emph => self.emphasis = self.emphasis.saturating_sub(1),
            NodeValue::Strikethrough => self.strike = self.strike.saturating_sub(1),
            NodeValue::Link(_) => {
                self.links.pop();
            }
            NodeValue::Image(_) => {
                self.images = self.images.saturating_sub(1);
                if self.images == 0 {
                    // The description, never the image itself.
                    let alt = std::mem::take(&mut self.alt);
                    let text = if alt.trim().is_empty() {
                        "Image".to_owned()
                    } else {
                        alt
                    };
                    let link = self.links.pop();
                    self.emphasis += 1;
                    self.push(&text, false);
                    self.emphasis -= 1;
                    self.links.extend(link);
                }
            }
            // Keeps the words of flattened blocks apart.
            value if value.block() => self.push(" ", false),
            _ => {}
        }
    }

    fn push(&mut self, text: &str, code: bool) {
        if self.images > 0 {
            self.alt.push_str(text);
            return;
        }
        let run = Run {
            text: String::new(),
            strong: self.strong > 0,
            emphasis: self.emphasis > 0,
            code,
            strike: self.strike > 0,
            link: self.links.last().cloned(),
        };
        match self.runs.last_mut() {
            Some(last) if last.same_style(&run) => last.text.push_str(text),
            _ => self.runs.push(Run {
                text: text.to_owned(),
                ..run
            }),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// How deep the test documents nest, far deeper than anyone writes by
    /// hand.
    const DEEP: usize = 100_000;

    fn emphasis() -> String {
        format!("{}deep{}", "*".repeat(DEEP), "*".repeat(DEEP))
    }

    fn quotes() -> String {
        format!("{} deep quote\n", ">".repeat(DEEP))
    }

    fn lists() -> String {
        format!("{}deep list\n", "- ".repeat(DEEP))
    }

    fn mixed() -> String {
        format!(
            "{}*[link **deep** ![alt *text*](https://example.com/a.png)](https://example.com)*\n",
            "> - 1. ".repeat(DEEP / 10)
        )
    }

    /// Every run's text in `blocks`, in order.
    fn text(blocks: &[Block]) -> String {
        let mut text = String::new();
        let mut stack: Vec<&Block> = blocks.iter().rev().collect();
        while let Some(block) = stack.pop() {
            match block {
                Block::Heading(_, runs) | Block::Paragraph(runs) => {
                    text.extend(runs.iter().map(|run| run.text.as_str()));
                }
                Block::Code(code) => text.push_str(code),
                Block::Quote(blocks) => stack.extend(blocks.iter().rev()),
                Block::List { items, .. } => {
                    stack.extend(items.iter().rev().flat_map(|item| item.iter().rev()));
                }
                Block::Rule => {}
            }
        }
        text
    }

    #[test]
    fn deeply_nested_markdown_parses_and_draws_on_a_one_megabyte_stack() {
        // A Windows main thread, where Noura draws, has 1 MB of stack.
        let drawn = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let context = egui::Context::default();
                let mut texts = Vec::new();
                for markdown in [emphasis(), quotes(), lists(), mixed()] {
                    texts.push(text(&parse(&markdown)));
                    for _ in 0..2 {
                        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                            show(ui, &markdown);
                        });
                        output.textures_delta.clear();
                    }
                }
                texts
            })
            .expect("the drawing thread starts")
            .join()
            .expect("deep Markdown draws without a panic");
        assert!(drawn[0].contains("deep"), "{:?}", drawn[0]);
        assert!(drawn[1].contains("deep quote"), "{:?}", drawn[1]);
        assert!(drawn[2].contains("deep list"), "{:?}", drawn[2]);
        assert!(drawn[3].contains("link deep alt text"), "{:?}", drawn[3]);
    }

    #[test]
    fn nested_emphasis_draws_as_one_styled_run() {
        let blocks = parse(&format!("{}deep{}", "*".repeat(31), "*".repeat(31)));
        let [Block::Paragraph(runs)] = blocks.as_slice() else {
            panic!("one paragraph");
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "deep");
        assert!(runs[0].strong && runs[0].emphasis);
    }

    #[test]
    fn an_image_shows_its_description_outside_its_link() {
        let blocks =
            parse("[see ![a *ripe* tomato](https://example.com/t.png)](https://example.com)");
        let [Block::Paragraph(runs)] = blocks.as_slice() else {
            panic!("one paragraph");
        };
        let shown: Vec<(&str, Option<&str>, bool)> = runs
            .iter()
            .map(|run| (run.text.as_str(), run.link.as_deref(), run.emphasis))
            .collect();
        assert_eq!(
            shown,
            [
                ("see ", Some("https://example.com"), false),
                ("a ripe tomato", None, true),
            ]
        );
    }
}
