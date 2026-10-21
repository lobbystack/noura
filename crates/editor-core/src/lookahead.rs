//! Frontmatter and `$$` display math count only when their closing line
//! falls within [`LOOKAHEAD`] bytes, as in today's editor; otherwise their
//! text parses as ordinary Markdown.
//!
//! The editor reads frontmatter itself instead of through comrak's front
//! matter extension, which only knows `---` closers without trailing
//! spaces, and which counts lines by line feeds alone where the rest of
//! comrak also ends a line at a lone carriage return. comrak parses a copy
//! of the text in which the frontmatter's delimiter lines read as code
//! fences, byte for byte, so every source position lines up with the note.

use std::ops::Range;

use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};

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

/// The byte offset where each line of `text` starts, as comrak numbers
/// lines.
fn line_starts(text: &str) -> Vec<usize> {
    lines(text).map(|line| line.start).collect()
}

/// The display math in `root` whose closing `$$` sits past the lookahead,
/// by the byte offset of its opening `$$`.
fn distant_display_math<'a>(root: &'a AstNode<'a>, starts: &[usize]) -> Vec<usize> {
    let byte = |line: usize, column: usize| {
        starts
            .get(line.saturating_sub(1))
            .map_or(0, |start| start + column.saturating_sub(1))
    };
    root.descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            let NodeValue::Math(math) = &data.value else {
                return None;
            };
            let sourcepos = data.sourcepos;
            let start = byte(sourcepos.start.line, sourcepos.start.column);
            let end = byte(sourcepos.end.line, sourcepos.end.column);
            (math.display_math && end.saturating_sub(start) > LOOKAHEAD).then_some(start)
        })
        .collect()
}

impl MdDoc {
    /// Parses the note with comrak: frontmatter reads as a code block, and
    /// `$$` math whose closing signs sit past the lookahead reads as
    /// ordinary Markdown. The caller owns the [`Arena`].
    pub fn parse<'a>(&self, arena: &'a Arena<'a>) -> &'a AstNode<'a> {
        let options = Self::comrak_options();
        let mut text = self.buffer.current.text_with("\n");
        mask_frontmatter(&mut text);
        let mut root = comrak::parse_document(arena, &text, &options);
        // Each pass can only turn more `$$` into text; a few suffice.
        for _ in 0..4 {
            let distant = distant_display_math(root, &line_starts(&text));
            if distant.is_empty() {
                break;
            }
            let mut bytes = std::mem::take(&mut text).into_bytes();
            for start in distant {
                // A stand-in comrak reads as plain text; the editor draws
                // the note's own `$$` there.
                bytes[start..start + 2].copy_from_slice(b"\x01\x01");
            }
            text = String::from_utf8(bytes).expect("ASCII stand-ins keep the text UTF-8");
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
