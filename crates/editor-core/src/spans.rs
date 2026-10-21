//! Obsidian-style syntax inside text that comrak doesn't parse: comments
//! (`%%comment%%`), tags (`#tag`), block IDs at the end of a line
//! (`^block-id`), `<u>` underline, and callout markers and titles. The
//! editor finds them in the text comrak parsed as paragraphs, headings and
//! table cells, and never inside code spans, math, links' destinations,
//! bare URLs, embeds or HTML it doesn't draw.

use comrak::nodes::{AstNode, NodeValue};

use crate::callouts::CalloutFamily;
use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _, RangeIterExt as _};

/// A kind of syntax found in text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanKind {
    /// A callout's `[!type]` marker, drawn as `label` away from the caret.
    CalloutMarker {
        label: String,
        family: CalloutFamily,
    },
    /// A callout's title, after its marker.
    CalloutTitle { family: CalloutFamily },
    /// `%%comment%%`, hidden away from the caret.
    Comment,
    /// `#tag`, drawn styled.
    Tag,
    /// `^block-id` at the end of a line, drawn styled.
    BlockId,
    /// `<u>text</u>`: the text draws underlined and the tags hide away
    /// from the caret.
    Underline,
}

/// Syntax found in text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineSpan {
    pub kind: SpanKind,
    /// The span's source, syntax included.
    pub range: (Grapheme, Grapheme),
    /// The parts hidden away from the caret.
    pub syntax: Vec<(Grapheme, Grapheme)>,
    /// The range the caret reveals the syntax from: the span itself, or a
    /// callout's whole quote for its marker.
    pub reveal_range: (Grapheme, Grapheme),
}

impl InlineSpan {
    fn new(kind: SpanKind, range: (Grapheme, Grapheme), syntax: Vec<(Grapheme, Grapheme)>) -> Self {
        Self {
            kind,
            range,
            syntax,
            reveal_range: range,
        }
    }
}

/// A match in a line's text, in bytes from the line's start.
type Found = (
    SpanKind,
    std::ops::Range<usize>,
    Vec<std::ops::Range<usize>>,
);

/// Whether `ch` can be part of a tag.
fn tag_char(ch: char) -> bool {
    ch.is_alphabetic() || ch.is_numeric() || matches!(ch, '_' | '/' | '-')
}

/// `#tag`s in `text`: a `#` at the start or after whitespace, then letters,
/// digits, `_`, `/` and `-`, with at least one that isn't a digit.
fn tags(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    for (index, ch) in text.char_indices() {
        let starts = ch == '#' && previous.is_none_or(char::is_whitespace);
        previous = Some(ch);
        if !starts {
            continue;
        }
        let rest = &text[index + 1..];
        let len: usize = rest
            .chars()
            .take_while(|ch| tag_char(*ch))
            .map(char::len_utf8)
            .sum();
        let name = &rest[..len];
        if name.chars().any(|ch| !ch.is_numeric()) {
            found.push((SpanKind::Tag, index..index + 1 + len, Vec::new()));
        }
    }
    found
}

/// A `^block-id` that ends `text`, after its start or whitespace.
fn block_id(text: &str) -> Option<Found> {
    let caret = text.rfind('^')?;
    let id = &text[caret + 1..];
    let word = |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-');
    let after_space = text[..caret]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (!id.is_empty() && id.chars().all(word) && after_space)
        .then(|| (SpanKind::BlockId, caret..text.len(), Vec::new()))
}

/// Each `open … close` pair in `text`, the shortest from each opening.
fn pairs(text: &str, open: &str, close: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = text[from..].find(open).map(|index| from + index) {
        let inner = start + open.len();
        let Some(end) = text[inner..]
            .find(close)
            .map(|index| inner + index + close.len())
        else {
            break;
        };
        found.push((start, end));
        from = end;
    }
    found
}

fn comments(text: &str) -> Vec<Found> {
    pairs(text, "%%", "%%")
        .into_iter()
        .map(|(start, end)| (SpanKind::Comment, start..end, vec![start..end]))
        .collect()
}

fn underlines(text: &str) -> Vec<Found> {
    pairs(text, "<u>", "</u>")
        .into_iter()
        .map(|(start, end)| {
            (
                SpanKind::Underline,
                start..end,
                vec![start..start + 3, end - 4..end],
            )
        })
        .collect()
}

impl<'ast> MdDoc {
    /// Recompute [`crate::bounds::Bounds::spans`]. Depends on text only.
    pub fn calc_inline_spans(&mut self, root: &'ast AstNode<'ast>) {
        let mut spans = Vec::new();
        for node in root.descendants() {
            if let Some(callout) = self.callout(node) {
                let family = callout.family();
                let mut marker = InlineSpan::new(
                    SpanKind::CalloutMarker {
                        label: callout.label(),
                        family,
                    },
                    callout.marker,
                    vec![callout.marker],
                );
                marker.reveal_range = self.node_range(node);
                spans.push(marker);
                if !callout.title.is_empty() {
                    let mut title = InlineSpan::new(
                        SpanKind::CalloutTitle { family },
                        callout.title,
                        Vec::new(),
                    );
                    title.reveal_range = self.node_range(node);
                    spans.push(title);
                }
            }
            if matches!(
                node.data.borrow().value,
                NodeValue::Paragraph | NodeValue::Heading(_) | NodeValue::TableCell
            ) {
                self.text_spans(node, &mut spans);
            }
        }
        spans.sort_by_key(|span| span.range);
        self.bounds.spans = spans;
    }

    /// The comments, tags, block IDs and underlines in an inline container.
    fn text_spans(&self, container: &'ast AstNode<'ast>, spans: &mut Vec<InlineSpan>) {
        let excluded = self.literal_ranges(container);
        let inlines: Vec<_> = container
            .descendants()
            .skip(1)
            .map(|node| {
                let text = matches!(
                    node.data.borrow().value,
                    NodeValue::Text(_) | NodeValue::SoftBreak
                );
                (self.node_range(node), text)
            })
            .collect();
        let lines = if matches!(container.data.borrow().value, NodeValue::TableCell) {
            vec![self.node_range(container)]
        } else {
            self.node_lines(container)
                .iter()
                .map(|line_idx| self.node_line(container, self.bounds.source_lines[line_idx]))
                .collect()
        };
        for line in lines {
            let text = &self.buffer[line];
            let line_start = self.offset_to_byte(line.start());
            let to_range = |bytes: &std::ops::Range<usize>| {
                self.range_to_char_ceil((line_start + bytes.start, line_start + bytes.end))
            };
            let mut comments_here: Vec<(Grapheme, Grapheme)> = Vec::new();
            let mut found = comments(text);
            found.extend(underlines(text));
            found.extend(tags(text));
            found.extend(block_id(text));
            for (kind, bytes, syntax) in found {
                let range = to_range(&bytes);
                if excluded.iter().any(|ex| range.intersects(ex, false)) {
                    continue;
                }
                // A span may cut through text, but not through other syntax.
                let crosses = inlines.iter().any(|(node, text)| {
                    !text
                        && node.intersects(&range, false)
                        && !range.contains_range(node, true, true)
                        && !node.contains_range(&range, true, true)
                });
                if crosses {
                    continue;
                }
                if kind == SpanKind::Comment {
                    comments_here.push(range);
                } else if comments_here
                    .iter()
                    .any(|comment| comment.contains_range(&range, true, true))
                {
                    // Syntax inside a comment is part of the comment.
                    continue;
                }
                let syntax = syntax.iter().map(to_range).collect();
                spans.push(InlineSpan::new(kind, range, syntax));
            }
        }
    }

    /// Ranges inside an inline container whose text is literal: code
    /// spans, math, images, wikilinks, bare URLs, link destinations,
    /// escapes and HTML other than `<u>` and `</u>`.
    fn literal_ranges(&self, container: &'ast AstNode<'ast>) -> Vec<(Grapheme, Grapheme)> {
        let mut ranges = Vec::new();
        for node in container.descendants().skip(1) {
            let range = self.node_range(node);
            match &node.data.borrow().value {
                NodeValue::Code(_)
                | NodeValue::Math(_)
                | NodeValue::Image(_)
                | NodeValue::WikiLink(_)
                | NodeValue::Escaped => ranges.push(range),
                NodeValue::HtmlInline(html) if html != "<u>" && html != "</u>" => {
                    ranges.push(range)
                }
                NodeValue::Link(link) => {
                    if self.link_is_auto(node, &link.url) || node.children().next().is_none() {
                        ranges.push(range);
                    } else if let Some(postfix) = self.postfix_range(node) {
                        ranges.push(postfix);
                    }
                }
                _ => {}
            }
        }
        ranges
    }

    /// The spans whose range touches `range`, in source order.
    pub fn spans_in(&self, range: (Grapheme, Grapheme)) -> impl Iterator<Item = &InlineSpan> {
        let spans = &self.bounds.spans;
        let first = spans.partition_point(|span| span.range.end() < range.start());
        spans[first..]
            .iter()
            .take_while(move |span| span.range.start() <= range.end())
            .filter(move |span| span.range.intersects(&range, true))
    }

    /// Whether `span` shows its syntax: while the caret, the selection, a
    /// find match or a search preview touches its reveal range.
    pub fn span_revealed(&self, span: &InlineSpan) -> bool {
        self.range_revealed(span.reveal_range, true)
    }
}

#[cfg(test)]
mod tests {
    use super::{block_id, comments, tags};

    fn ranges(found: Vec<super::Found>) -> Vec<std::ops::Range<usize>> {
        found.into_iter().map(|(_, range, _)| range).collect()
    }

    #[test]
    fn tags_need_a_letter_and_a_space_before() {
        assert_eq!(
            ranges(tags("#a b#c #1 #1a #é_x/y-z,")),
            vec![0..2, 10..13, 14..23]
        );
        assert_eq!(
            ranges(tags("# heading ## x")),
            Vec::<std::ops::Range<usize>>::new()
        );
    }

    #[test]
    fn block_ids_end_the_line() {
        assert_eq!(block_id("text ^id-1").map(|f| f.1), Some(5..10));
        assert_eq!(block_id("^id").map(|f| f.1), Some(0..3));
        assert_eq!(block_id("a^id").map(|f| f.1), None);
        assert_eq!(block_id("^id text").map(|f| f.1), None);
        assert_eq!(block_id("text ^").map(|f| f.1), None);
    }

    #[test]
    fn comments_take_the_shortest_pair() {
        assert_eq!(ranges(comments("a %%b%% c %%d%% %%")), vec![2..7, 10..15]);
        assert_eq!(ranges(comments("%%%%")), vec![0..4]);
    }
}
