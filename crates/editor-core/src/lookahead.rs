//! Frontmatter and `$$` display math count only when their closing line
//! falls within [`LOOKAHEAD`] bytes, as in today's editor; otherwise their
//! text parses as ordinary Markdown. Inside display math, lines that look
//! like block syntax stay part of the math.
//!
//! The editor reads frontmatter itself instead of through comrak's front
//! matter extension, which only knows `---` closers without trailing
//! spaces, and which counts lines by line feeds alone where the rest of
//! comrak also ends a line at a lone carriage return. comrak parses a copy
//! of the text in which the frontmatter's delimiter lines read as code
//! fences, byte for byte, so every source position lines up with the note.

use std::ops::Range;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options};

use crate::ast::{Ast, Reparsed, common_prefix, common_suffix};
use crate::doc::MdDoc;

/// How far a construct's closing line may sit from its start, in bytes.
pub const LOOKAHEAD: usize = 256 * 1024;

/// The lines of `text` with their byte ranges, line endings left out. A
/// line ends at `\r\n`, `\r` or `\n`, as comrak reads it.
fn lines(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let bytes = text.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        if start > bytes.len() {
            return None;
        }
        let end = bytes[start..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(bytes.len(), |offset| start + offset);
        let line = start..end;
        start = if bytes.get(end) == Some(&b'\r') && bytes.get(end + 1) == Some(&b'\n') {
            end + 2
        } else {
            end + 1
        };
        Some(line)
    })
}

/// Whether `line` is a frontmatter delimiter: `---`, or `...` when it
/// closes, with spaces or tabs after it.
fn is_delimiter(line: &str, closing: bool) -> bool {
    let rest = line
        .strip_prefix("---")
        .or_else(|| closing.then(|| line.strip_prefix("...")).flatten());
    rest.is_some_and(|rest| rest.chars().all(|ch| ch == ' ' || ch == '\t'))
}

/// The opening and closing lines of the frontmatter `text` starts with,
/// when its closing line ends within the lookahead.
pub fn frontmatter_lines(text: &str) -> Option<(Range<usize>, Range<usize>)> {
    let mut lines = lines(text);
    let open = lines.next()?;
    if !is_delimiter(&text[open.clone()], false) {
        return None;
    }
    lines
        .take_while(|line| line.end <= LOOKAHEAD)
        .find(|line| is_delimiter(&text[line.clone()], true))
        .map(|close| (open, close))
}

/// Rewrites the frontmatter `text` starts with so comrak reads it as a
/// fenced code block, byte for byte: each delimiter starts with three
/// backticks and no backtick inside can close the fence. Returns the
/// frontmatter's byte range, delimiters included.
fn mask_frontmatter(text: &mut String) -> Option<Range<usize>> {
    let (open, close) = frontmatter_lines(text)?;
    let mut bytes = std::mem::take(text).into_bytes();
    bytes[open.start..open.start + 3].copy_from_slice(b"```");
    for byte in &mut bytes[open.end..close.start] {
        if *byte == b'`' {
            *byte = b'\'';
        }
    }
    bytes[close.start..close.start + 3].copy_from_slice(b"```");
    // Only ASCII bytes changed, to other ASCII bytes.
    *text = String::from_utf8(bytes).expect("ASCII stand-ins keep the text UTF-8");
    Some(open.start..close.end)
}

/// The stand-in for a byte comrak must read as plain text.
const PLAIN: u8 = 0x01;

/// Whether `line` closes display math: it ends with `$$`, then only spaces
/// or tabs, as in today's editor.
fn closes_math(line: &[u8]) -> bool {
    let end = line
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(0, |last| last + 1);
    line[..end].ends_with(b"$$")
}

/// Whether `line` holds all of its display math, as `$$x$$` does.
fn math_on_one_line(after_opening: &[u8]) -> bool {
    closes_math(after_opening)
        && after_opening
            .iter()
            .rev()
            .skip_while(|byte| matches!(byte, b' ' | b'\t'))
            .skip(2)
            .any(|byte| !byte.is_ascii_whitespace())
}

/// Turns the first byte of a line inside display math into [`PLAIN`] when
/// it could start a block: a list item, quote, heading, setext underline,
/// thematic break, code fence, HTML block or table row, or a line of only
/// spaces, which would end the paragraph that holds the math.
fn keep_inside_paragraph(line: &mut [u8]) {
    let Some(first) = line.iter().position(|byte| !matches!(byte, b' ' | b'\t')) else {
        if let Some(byte) = line.first_mut() {
            *byte = PLAIN;
        }
        return;
    };
    let starts_block = match line[first] {
        b'-' | b'+' | b'*' | b'>' | b'#' | b'=' | b'_' | b'`' | b'~' | b'<' | b'|' => true,
        b'0'..=b'9' => {
            let digits = line[first..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            matches!(line.get(first + digits), Some(b'.' | b')'))
        }
        _ => false,
    };
    if starts_block {
        line[first] = PLAIN;
    }
}

/// Keeps each `$$` display math block that closes within the lookahead in
/// one paragraph, as today's editor reads it as one block: comrak reads
/// `$$` as inline math, so a line inside that looks like block syntax would
/// split it. Only blocks outside quotes count, and none inside fenced code.
/// A block with an empty line inside stays as comrak reads it, as no
/// stand-in fills an empty line. Bytes change only to [`PLAIN`], so offsets
/// stay put.
fn mask_display_math(text: &mut String, from: usize) {
    let lines: Vec<Range<usize>> = lines(text).filter(|line| line.start >= from).collect();
    let mut bytes = std::mem::take(text).into_bytes();
    // For each line, the first line from it on that closes math and the
    // first that is empty, so each opening looks ahead in constant time.
    let mut next_close = vec![usize::MAX; lines.len() + 1];
    let mut next_empty = vec![usize::MAX; lines.len() + 1];
    for index in (0..lines.len()).rev() {
        let line = &bytes[lines[index].clone()];
        next_close[index] = if closes_math(line) {
            index
        } else {
            next_close[index + 1]
        };
        next_empty[index] = if line.is_empty() {
            index
        } else {
            next_empty[index + 1]
        };
    }
    let mut fence: Option<(u8, usize)> = None;
    let mut index = 0;
    while index < lines.len() {
        let line = &bytes[lines[index].clone()];
        let indent = line.iter().take_while(|byte| **byte == b' ').count();
        let rest = &line[indent.min(line.len())..];
        index += 1;
        if indent >= 4 {
            continue;
        }
        let run = |ch: u8| rest.iter().take_while(|byte| **byte == ch).count();
        if let Some((ch, len)) = fence {
            let closing = run(ch);
            if closing >= len
                && rest[closing..]
                    .iter()
                    .all(|byte| matches!(byte, b' ' | b'\t'))
            {
                fence = None;
            }
            continue;
        }
        if run(b'`') >= 3 && !rest[run(b'`')..].contains(&b'`') {
            fence = Some((b'`', run(b'`')));
            continue;
        }
        if run(b'~') >= 3 {
            fence = Some((b'~', run(b'~')));
            continue;
        }
        if !rest.starts_with(b"$$") || math_on_one_line(&rest[2..]) {
            continue;
        }
        let start = lines[index - 1].start + indent;
        let close = next_close[index];
        // An empty line ends the paragraph, so comrak can't read the block
        // as math whatever the lines around it hold.
        if close < lines.len() && lines[close].end <= start + LOOKAHEAD && next_empty[index] > close
        {
            for inside in index..=close {
                keep_inside_paragraph(&mut bytes[lines[inside].clone()]);
            }
            index = close + 1;
        }
    }
    *text = String::from_utf8(bytes).expect("ASCII stand-ins keep the text UTF-8");
}

/// The byte offset where each line of `text` starts, as comrak numbers
/// lines.
fn line_starts(text: &str) -> Vec<usize> {
    lines(text).map(|line| line.start).collect()
}

/// The display math in `nodes` and everything inside them whose closing
/// `$$` sits past the lookahead, as byte ranges from its opening `$$`
/// through its closing one. `line_start` gives where each 0-based line
/// starts.
fn distant_display_math<'a>(
    nodes: impl IntoIterator<Item = &'a AstNode<'a>>,
    line_start: impl Fn(usize) -> Option<usize>,
) -> Vec<Range<usize>> {
    let byte = |line: usize, column: usize| {
        line_start(line.saturating_sub(1)).map_or(0, |start| start + column.saturating_sub(1))
    };
    nodes
        .into_iter()
        .flat_map(|node| node.descendants())
        .filter_map(|node| {
            let data = node.data.borrow();
            let NodeValue::Math(math) = &data.value else {
                return None;
            };
            if !math.display_math {
                return None;
            }
            let sourcepos = data.sourcepos;
            let start = byte(sourcepos.start.line, sourcepos.start.column);
            let end = byte(sourcepos.end.line, sourcepos.end.column);
            (end.saturating_sub(start) > LOOKAHEAD).then_some(start..end + 1)
        })
        .collect()
}

/// The text comrak parses for a note's `text`: frontmatter reads as a code
/// block, and lines inside `$$` math that closes within the lookahead stay
/// inside it. Bytes change only to other ASCII bytes, so every offset and
/// line stays put.
fn comrak_source(mut text: String) -> String {
    let frontmatter = mask_frontmatter(&mut text);
    if memchr::memmem::find(text.as_bytes(), b"$$").is_some() {
        mask_display_math(&mut text, frontmatter.map_or(0, |range| range.end));
    }
    text
}

/// `text` with the opening `$$` of each of `math` turned into a stand-in
/// comrak reads as plain text; the editor draws the note's own `$$` there.
fn unpair(text: String, math: &[Range<usize>]) -> String {
    let mut bytes = text.into_bytes();
    for math in math {
        bytes[math.start..math.start + 2].copy_from_slice(b"\x01\x01");
    }
    String::from_utf8(bytes).expect("ASCII stand-ins keep the text UTF-8")
}

/// How many times a parse turns distant `$$` into text and parses again.
/// Each pass can only turn more `$$` into text; a few suffice.
const DISTANT_MATH_PASSES: usize = 4;

/// Parses a note's `text` into an [`Ast`] the way the editor reads it:
/// frontmatter reads as a code block, and `$$` math whose closing signs sit
/// past the lookahead reads as ordinary Markdown.
pub(crate) fn parse_ast(text: String, options: &Options, text_seq: u64) -> Ast {
    let mut ast = Ast::parse(comrak_source(text), options, text_seq);
    let mut unpaired = Vec::new();
    for _ in 0..DISTANT_MATH_PASSES {
        let distant =
            ast.with_root(|root| distant_display_math([root], |line| ast.line_start(line)));
        if distant.is_empty() {
            break;
        }
        let text = unpair(ast.source().to_owned(), &distant);
        unpaired.extend(distant);
        ast = Ast::parse(text, options, text_seq);
    }
    unpaired.sort_unstable_by_key(|math| math.start);
    ast.set_distant_math(unpaired);
    ast
}

/// Brings `ast` up to date with a note's `text` through [`Ast::update`],
/// or returns `None` when only a full parse ([`parse_ast`]) reads the
/// result right: the edit reaches `$$` math whose closing signs sit past
/// the lookahead, or the blocks it reparsed hold new such math. `ast` is
/// then a partial parse to throw away.
pub(crate) fn update_ast(
    ast: &mut Ast,
    text: String,
    options: &Options,
    text_seq: u64,
) -> Option<Reparsed> {
    let source = comrak_source(text);
    let carried = carry_distant_math(ast, &source)?;
    let math: Vec<Range<usize>> = carried.iter().map(|(math, _)| math.clone()).collect();
    let reparsed = ast.update(unpair(source, &math), options, text_seq);
    // The signs pair as before only while the block that holds them
    // starts where it did and still takes in the closing ones. A paragraph
    // that took in the lines above it, as when the empty line between
    // them goes, can pair them anew, and one whose last line turned into
    // a table's header lost them.
    let held = carried.iter().all(|(math, start)| {
        holding_block(ast, math.start)
            .is_some_and(|block| block.start == *start && block.end >= math.end)
    });
    if !held {
        return None;
    }
    let distant = ast.with_root(|root| {
        let blocks = root
            .children()
            .skip(reparsed.blocks.start)
            .take(reparsed.blocks.len());
        distant_display_math(blocks, |line| ast.line_start(line))
    });
    if !distant.is_empty() {
        return None;
    }
    ast.set_distant_math(math);
    Some(reparsed)
}

/// Where the distant math `ast` reads as text sits in `source`, the new
/// text comrak parses, with where the innermost block that holds it
/// starts there, or `None` when an edit reaches it. Math before the edit
/// stays put. Math after it moves by the bytes the edit added, when the
/// edit ends before its block: what comes before the math in its block
/// decides how its signs pair.
fn carry_distant_math(ast: &Ast, source: &str) -> Option<Vec<(Range<usize>, usize)>> {
    let distant = ast.distant_math();
    if distant.is_empty() {
        return Some(Vec::new());
    }
    // The text before the edit as comrak would read it without stand-ins.
    let mut old = ast.source().as_bytes().to_vec();
    for math in distant {
        old[math.start..math.start + 2].copy_from_slice(b"$$");
    }
    let new = source.as_bytes();
    let prefix = common_prefix(&old, new);
    let suffix = common_suffix(&old[prefix..], &new[prefix..]);
    let old_end = old.len() - suffix;
    let delta = new.len() as isize - old.len() as isize;
    let moved = |at: usize| (at as isize + delta) as usize;
    distant
        .iter()
        .map(|math| {
            let block = holding_block(ast, math.start)?;
            // A byte right after the closing signs can still change how
            // they close, as a third `$` does.
            if math.end < prefix {
                Some((math.clone(), block.start))
            } else if old_end < block.start {
                Some((moved(math.start)..moved(math.end), moved(block.start)))
            } else {
                None
            }
        })
        .collect()
}

/// The byte range of the innermost block of `ast` that holds byte `at`.
fn holding_block(ast: &Ast, at: usize) -> Option<Range<usize>> {
    let byte = |line: usize, column: usize| {
        ast.line_start(line.saturating_sub(1))
            .map(|start| start + column.saturating_sub(1))
    };
    ast.with_root(|root| {
        let mut holder = None;
        let mut node = root;
        while let Some((child, range)) = node.children().find_map(|child| {
            let data = child.data.borrow();
            if !data.value.block() {
                return None;
            }
            let pos = data.sourcepos;
            let range =
                byte(pos.start.line, pos.start.column)?..byte(pos.end.line, pos.end.column)? + 1;
            range.contains(&at).then_some((child, range))
        }) {
            holder = Some(range);
            node = child;
        }
        holder
    })
}

impl MdDoc {
    /// Parses the note with comrak, as [`parse_ast`] does. The caller owns
    /// the [`Arena`].
    pub fn parse<'a>(&self, arena: &'a Arena<'a>) -> &'a AstNode<'a> {
        let options = Self::comrak_options();
        let mut text = comrak_source(self.buffer.current.text_with("\n"));
        let mut root = comrak::parse_document(arena, &text, &options);
        for _ in 0..DISTANT_MATH_PASSES {
            let starts = line_starts(&text);
            let distant = distant_display_math([root], |line| starts.get(line).copied());
            if distant.is_empty() {
                break;
            }
            text = unpair(text, &distant);
            root = comrak::parse_document(arena, &text, &options);
        }
        root
    }

    /// The range of the frontmatter the note starts with, delimiters
    /// included, when it closes within the lookahead.
    pub fn frontmatter_range(
        &self,
    ) -> Option<(crate::offset_types::Grapheme, crate::offset_types::Grapheme)> {
        let text = self.buffer.current.text();
        let (open, close) = frontmatter_lines(text)?;
        Some(self.range_to_char((open.start.into(), close.end.into())))
    }
}

#[cfg(test)]
mod tests {
    use super::{frontmatter_lines, lines};

    #[test]
    fn lines_end_at_any_line_ending() {
        let text = "a\rb\r\nc\nd";
        let found: Vec<&str> = lines(text).map(|line| &text[line]).collect();
        assert_eq!(found, vec!["a", "b", "c", "d"]);
        let text = "a\n";
        let found: Vec<&str> = lines(text).map(|line| &text[line]).collect();
        assert_eq!(found, vec!["a", ""]);
    }

    #[test]
    fn frontmatter_needs_a_closing_line() {
        assert_eq!(
            frontmatter_lines("---\na: 1\n---\nbody"),
            Some((0..3, 9..12))
        );
        assert_eq!(frontmatter_lines("--- \na\n... \n"), Some((0..4, 7..11)));
        assert_eq!(frontmatter_lines("---\na: 1\n"), None);
        assert_eq!(frontmatter_lines("----\na\n---\n"), None);
        assert_eq!(frontmatter_lines("...\na\n---\n"), None);
        assert_eq!(frontmatter_lines("x\n---\na\n---\n"), None);
    }
}
