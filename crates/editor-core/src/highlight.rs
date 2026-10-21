//! Syntax highlighting for fenced code with a known language tag, through
//! arborium's tree-sitter grammars. Code without a tag, with a tag no
//! grammar knows, or indented, draws as plain monospace.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use arborium::{GrammarStore, Highlighter};
use comrak::Arena;
use comrak::nodes::{AstNode, NodeCodeBlock, NodeValue};

use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeIterExt as _};

/// How a piece of code draws. Each style maps to a color of the code
/// theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeStyle {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
}

/// A highlighted piece of code: its range in the note and its style.
pub type CodeHighlight = ((Grapheme, Grapheme), CodeStyle);

/// The style a tree-sitter capture name draws in, by the part before its
/// first dot, as in `function.method`.
fn style(capture: &str) -> Option<CodeStyle> {
    let kind = capture.split('.').next().unwrap_or(capture);
    Some(match kind {
        "keyword" | "include" | "conditional" | "repeat" | "exception" | "storageclass"
        | "operator.keyword" | "tag" | "label" | "attribute" => CodeStyle::Keyword,
        "function" | "method" | "constructor" | "macro" => CodeStyle::Function,
        "type" | "class" | "namespace" | "module" | "interface" => CodeStyle::Type,
        "string" | "character" | "escape" | "regexp" | "symbol" => CodeStyle::String,
        "number" | "float" | "boolean" | "constant" => CodeStyle::Number,
        "comment" => CodeStyle::Comment,
        _ => return None,
    })
}

/// The grammars, compiled once and shared by every note.
fn grammars() -> Arc<GrammarStore> {
    static STORE: OnceLock<Arc<GrammarStore>> = OnceLock::new();
    STORE.get_or_init(|| Arc::new(GrammarStore::new())).clone()
}

thread_local! {
    /// A parse context per thread over the shared grammars.
    static HIGHLIGHTER: RefCell<Highlighter> =
        RefCell::new(Highlighter::with_store(grammars()));
}

/// The style of each byte of `code` in `language`, or `None` when no
/// grammar knows the language. A shorter span inside a longer one wins.
fn styles(language: &str, code: &str) -> Option<Vec<Option<CodeStyle>>> {
    let language = language.to_ascii_lowercase();
    let mut spans = HIGHLIGHTER
        .with(|highlighter| highlighter.borrow_mut().highlight_spans(&language, code))
        .ok()?;
    spans.sort_by_key(|span| {
        (
            std::cmp::Reverse(span.end.saturating_sub(span.start)),
            span.pattern_index,
        )
    });
    let mut styles = vec![None; code.len()];
    for span in spans {
        let Some(style) = style(&span.capture) else {
            continue;
        };
        let end = (span.end as usize).min(code.len());
        let start = (span.start as usize).min(end);
        for slot in &mut styles[start..end] {
            *slot = Some(style);
        }
    }
    Some(styles)
}

/// How many code blocks' styles the cache keeps across edits.
const STYLES_KEPT: usize = 256;

/// Highlights already worked out: each block's pieces for the current
/// text, and the styles of code as written, which an edit elsewhere in
/// the note leaves as they were.
#[derive(Default)]
pub struct HighlightCache {
    text_seq: u64,
    blocks: HashMap<(Grapheme, Grapheme), Vec<CodeHighlight>>,
    styles: HashMap<(String, String), Option<Arc<Vec<Option<CodeStyle>>>>>,
}

impl<'ast> MdDoc {
    /// The highlighted pieces of a fenced code block's code, in order, or
    /// none when its language tag names no grammar.
    pub fn code_block_highlights(
        &self,
        node: &'ast AstNode<'ast>,
        code_block: &NodeCodeBlock,
    ) -> Vec<CodeHighlight> {
        let range = self.node_range(node);
        {
            let mut cache = self.highlight_cache.borrow_mut();
            if cache.text_seq != self.text_seq {
                cache.blocks.clear();
                cache.text_seq = self.text_seq;
            }
            if let Some(highlights) = cache.blocks.get(&range) {
                return highlights.clone();
            }
        }
        let highlights = self.highlight_code_block(node, code_block);
        self.highlight_cache
            .borrow_mut()
            .blocks
            .insert(range, highlights.clone());
        highlights
    }

    /// The style of each byte of `code`, kept across edits.
    fn cached_styles(&self, language: &str, code: String) -> Option<Arc<Vec<Option<CodeStyle>>>> {
        let key = (language.to_owned(), code);
        if let Some(styles) = self.highlight_cache.borrow().styles.get(&key) {
            return styles.clone();
        }
        let styles = styles(language, &key.1).map(Arc::new);
        let mut cache = self.highlight_cache.borrow_mut();
        if cache.styles.len() >= STYLES_KEPT {
            cache.styles.clear();
        }
        cache.styles.insert(key, styles.clone());
        styles
    }

    fn highlight_code_block(
        &self,
        node: &'ast AstNode<'ast>,
        code_block: &NodeCodeBlock,
    ) -> Vec<CodeHighlight> {
        // Frontmatter shown in the text is YAML.
        let language = if self.bounds.frontmatter == Some(self.node_range(node)) {
            "yaml"
        } else {
            code_block.info.split_whitespace().next().unwrap_or("")
        };
        if !code_block.fenced || language.is_empty() {
            return Vec::new();
        }
        // The code's lines, between the fences, joined for the grammar.
        let first = self.node_first_line_idx(node);
        let last = self.node_last_line_idx(node);
        let mut lines = Vec::new();
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            if line_idx == first
                || (line_idx == last && self.is_closing_fence(node, code_block, line))
            {
                continue;
            }
            let node_line = self.node_line(node, line);
            lines.push(self.code_line_range(node, code_block, node_line, false));
        }
        let mut code = String::new();
        let mut starts = Vec::new();
        for line in &lines {
            starts.push(code.len());
            code.push_str(&self.buffer[*line]);
            code.push('\n');
        }
        let Some(styles) = self.cached_styles(language, code) else {
            return Vec::new();
        };
        // Runs of one style within each line, back in the note's offsets.
        let mut highlights = Vec::new();
        for (line, start) in lines.iter().zip(starts) {
            let line_bytes = self.range_to_byte(*line);
            let len = line_bytes.1.0 - line_bytes.0.0;
            let mut run_start = 0;
            while run_start < len {
                let style = styles[start + run_start];
                let mut run_end = run_start + 1;
                while run_end < len && styles[start + run_end] == style {
                    run_end += 1;
                }
                if let Some(style) = style {
                    let range =
                        self.range_to_char_ceil((line_bytes.0 + run_start, line_bytes.0 + run_end));
                    if range.0 < range.1 {
                        highlights.push((range, style));
                    }
                }
                run_start = run_end;
            }
        }
        highlights
    }

    /// Every highlighted piece of code in the note, in order.
    pub fn code_highlights(&mut self) -> Vec<CodeHighlight> {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        root.descendants()
            .flat_map(|node| match &node.data.borrow().value {
                NodeValue::CodeBlock(code_block) => self.code_block_highlights(node, code_block),
                _ => Vec::new(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeStyle, style};

    #[test]
    fn captures_map_by_their_first_part() {
        assert_eq!(style("keyword.function"), Some(CodeStyle::Keyword));
        assert_eq!(style("function.method"), Some(CodeStyle::Function));
        assert_eq!(style("string"), Some(CodeStyle::String));
        assert_eq!(style("punctuation.bracket"), None);
    }
}
