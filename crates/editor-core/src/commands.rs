//! Formatting commands: what the toolbar's buttons, Mod-Enter and Tab do to
//! the text.
//!
//! Ported from the legacy app's `packages/editor/src/commands.ts`. Like
//! the legacy commands, these read the text and its lines rather than the
//! parse, so a command does the same thing whatever the parser makes of
//! half-typed syntax. Each command plans its edits as byte ranges of the
//! text and where the selection goes, then applies them as one undo step.

use std::ops::Range;
use std::sync::LazyLock;

use comrak::nodes::{AstNode, NodeValue};
use regex::Regex;

use crate::buffer;
use crate::doc::MdDoc;
use crate::offset_types::{Byte, Grapheme, RangeExt as _};
use crate::operation_types::{Operation, Replace};

/// A formatting command, and a format the toolbar shows as pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Bold,
    Italic,
    /// `<u>` and `</u>` around the text.
    Underline,
    Strikethrough,
    /// Inline code.
    Code,
    Link,
    BulletList,
    NumberedList,
    CheckList,
    BlockQuote,
    /// A heading of this level, 1 to 6, or plain text for 0.
    Heading(u8),
}

impl Format {
    /// What the toolbar and the shortcuts list call it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bold => "Bold",
            Self::Italic => "Italic",
            Self::Underline => "Underline",
            Self::Strikethrough => "Strikethrough",
            Self::Code => "Inline code",
            Self::Link => "Link",
            Self::BulletList => "Bulleted list",
            Self::NumberedList => "Numbered list",
            Self::CheckList => "Checklist",
            Self::BlockQuote => "Quote",
            Self::Heading(0) => "Text",
            Self::Heading(1) => "Heading 1",
            Self::Heading(2) => "Heading 2",
            Self::Heading(3) => "Heading 3",
            Self::Heading(4) => "Heading 4",
            Self::Heading(5) => "Heading 5",
            Self::Heading(_) => "Heading 6",
        }
    }
}

/// Edits planned against the text as it is: byte ranges of the text in
/// order, none overlapping, and what replaces each.
type Edits = Vec<(Range<usize>, String)>;

/// A command's edits and the selection after them, as `(anchor, caret)`
/// byte offsets into the edited text. Without a selection, the current one
/// moves with the text around it.
#[derive(Debug, Default)]
struct Plan {
    edits: Edits,
    selection: Option<(usize, usize)>,
}

/// Where byte `pos` of the text lands after `edits`. Text inserted at the
/// position goes before it, so a caret stays with the text it was in; a
/// position inside replaced text moves to the end of the replacement.
fn map_position(edits: &Edits, pos: usize) -> usize {
    let mut delta = 0isize;
    for (range, text) in edits {
        if pos < range.start || (pos == range.start && !range.is_empty()) {
            break;
        }
        if pos < range.end {
            return range.start.saturating_add_signed(delta) + text.len();
        }
        delta += text.len() as isize - range.len() as isize;
    }
    pos.saturating_add_signed(delta)
}

/// The byte range of the line holding byte `pos`, without its `\n`.
fn line_at(text: &str, pos: usize) -> Range<usize> {
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    start..end
}

/// The lines from the one holding `from` to the one holding `to`.
fn lines_between(text: &str, from: usize, to: usize) -> Vec<Range<usize>> {
    let mut lines = vec![line_at(text, from)];
    while let Some(last) = lines.last()
        && last.end < to.max(from)
        && last.end < text.len()
    {
        let next = line_at(text, last.end + 1);
        lines.push(next);
    }
    lines
}

/// The selected lines a line command changes. Blank lines inside a
/// selection of several lines keep no marker.
fn selected_lines(text: &str, from: usize, to: usize) -> Vec<Range<usize>> {
    let lines = lines_between(text, from, to);
    if lines.len() == 1 {
        return lines;
    }
    lines
        .into_iter()
        .filter(|line| !text[line.clone()].trim().is_empty())
        .collect()
}

/// A list marker with an optional checkbox, or a quote marker.
static LINE_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*)(?:([-*+])|(\d+)([.)]))(\s+)(\[[ xX]\](?:\s+|$))?|^(\s*)(>\s?)")
        .expect("the line prefix pattern compiles")
});

/// A list item's marker and the space after it.
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*)([-*+]|\d+[.)])(\s+)").expect("the list item pattern compiles")
});

/// A task's marker up to its checkbox.
static TASK_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*)([-*+]|\d+[.)])(\s+)\[([ xX])\]").expect("the task pattern compiles")
});

/// An ordered item's number.
static ORDERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)(\d+)([.)])").expect("the ordered pattern compiles"));

/// Text that reads as a web address or a mail link.
static URL_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:[a-z][a-z\d+.-]*://|mailto:|www\.)\S+$").expect("the URL pattern compiles")
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Bullet,
    Numbered,
    Check,
    Quote,
}

impl LineKind {
    fn format(self) -> Format {
        match self {
            Self::Bullet => Format::BulletList,
            Self::Numbered => Format::NumberedList,
            Self::Check => Format::CheckList,
            Self::Quote => Format::BlockQuote,
        }
    }
}

/// A line's leading whitespace, the length of its list or quote prefix
/// (indent included), and which kind of prefix it is.
struct LineShape {
    indent: usize,
    prefix: usize,
    kind: Option<LineKind>,
}

fn line_shape(line: &str) -> LineShape {
    let Some(captures) = LINE_PREFIX.captures(line) else {
        let indent = line.len() - line.trim_start().len();
        return LineShape {
            indent,
            prefix: indent,
            kind: None,
        };
    };
    let prefix = captures[0].len();
    if captures.get(8).is_some() {
        return LineShape {
            indent: captures.get(7).map_or(0, |m| m.len()),
            prefix,
            kind: Some(LineKind::Quote),
        };
    }
    let kind = if captures.get(6).is_some() {
        LineKind::Check
    } else if captures.get(2).is_some() {
        LineKind::Bullet
    } else {
        LineKind::Numbered
    };
    LineShape {
        indent: captures.get(1).map_or(0, |m| m.len()),
        prefix,
        kind: Some(kind),
    }
}

/// A task's checkbox as a byte range of its line, and whether it's checked.
fn task_marker(line: &str) -> Option<(Range<usize>, bool)> {
    let captures = TASK_LINE.captures(line)?;
    let end = captures[0].len();
    // The box must end the line or stand before whitespace.
    if line[end..]
        .chars()
        .next()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    Some((end - 3..end, &captures[4] != " "))
}

/// The inline formats that wrap text in a run of one marker character.
#[derive(Clone, Copy)]
struct Inline {
    chars: &'static [u8],
    insert: &'static str,
    wrapped: fn(usize) -> bool,
}

const BOLD: Inline = Inline {
    // A run of two asterisks is bold, one italic, three both.
    chars: b"*_",
    insert: "**",
    wrapped: |run| run >= 2,
};
const ITALIC: Inline = Inline {
    chars: b"*_",
    insert: "*",
    wrapped: |run| run % 2 == 1,
};
const STRIKETHROUGH: Inline = Inline {
    chars: b"~",
    insert: "~~",
    wrapped: |run| run >= 2,
};
const CODE: Inline = Inline {
    chars: b"`",
    insert: "`",
    wrapped: |run| run >= 1,
};

fn run_before(text: &str, pos: usize, char: u8) -> usize {
    text.as_bytes()[..pos]
        .iter()
        .rev()
        .take_while(|b| **b == char)
        .count()
}

fn run_after(text: &str, pos: usize, char: u8) -> usize {
    text.as_bytes()[pos..]
        .iter()
        .take_while(|b| **b == char)
        .count()
}

/// Selections usually cover only the visible text because the editor hides
/// the markers. When the markers were selected too, move them outside the
/// range so both cases toggle the same way.
fn normalize_inline(text: &str, from: usize, to: usize, char: u8) -> (usize, usize) {
    let selected = &text.as_bytes()[from..to];
    let leading = selected.iter().take_while(|b| **b == char).count();
    let trailing = selected.iter().rev().take_while(|b| **b == char).count();
    let run = leading.min(trailing);
    if run == 0 || run * 2 >= selected.len() {
        return (from, to);
    }
    if selected[run..selected.len() - run].contains(&char) {
        return (from, to);
    }
    (from + run, to - run)
}

fn toggle_inline(text: &str, from: usize, to: usize, inline: Inline) -> Plan {
    let size = inline.insert.len();
    for &char in inline.chars {
        let (from, to) = normalize_inline(text, from, to, char);
        let before = run_before(text, from, char);
        let after = run_after(text, to, char);
        if before > 0 && after > 0 && (inline.wrapped)(before) && (inline.wrapped)(after) {
            return Plan {
                edits: vec![
                    (from - size..from, String::new()),
                    (to..to + size, String::new()),
                ],
                selection: Some((from - size, to - size)),
            };
        }
    }
    Plan {
        edits: vec![
            (from..from, inline.insert.to_owned()),
            (to..to, inline.insert.to_owned()),
        ],
        selection: Some((from + size, to + size)),
    }
}

fn toggle_pair(text: &str, from: usize, to: usize, opening: &str, closing: &str) -> Plan {
    let selected = &text[from..to];
    if selected.len() >= opening.len() + closing.len()
        && selected.starts_with(opening)
        && selected.ends_with(closing)
    {
        return Plan {
            edits: vec![
                (from..from + opening.len(), String::new()),
                (to - closing.len()..to, String::new()),
            ],
            selection: Some((from, to - opening.len() - closing.len())),
        };
    }
    if text[..from].ends_with(opening) && text[to..].starts_with(closing) {
        return Plan {
            edits: vec![
                (from - opening.len()..from, String::new()),
                (to..to + closing.len(), String::new()),
            ],
            selection: Some((from - opening.len(), to - opening.len())),
        };
    }
    Plan {
        edits: vec![
            (from..from, opening.to_owned()),
            (to..to, closing.to_owned()),
        ],
        selection: Some((from + opening.len(), to + opening.len())),
    }
}

/// Turn the selected lines into one kind of list or quote, or back into
/// plain paragraphs when they already all are that kind.
fn toggle_lines(text: &str, from: usize, to: usize, kind: LineKind) -> Plan {
    let lines = selected_lines(text, from, to);
    let shapes: Vec<LineShape> = lines
        .iter()
        .map(|line| line_shape(&text[line.clone()]))
        .collect();
    let remove = shapes.iter().all(|shape| shape.kind == Some(kind));
    let mut edits = Edits::new();
    for (index, (line, shape)) in lines.iter().zip(&shapes).enumerate() {
        let start = line.start + shape.indent;
        let end = line.start + shape.prefix;
        if remove {
            edits.push((start..end, String::new()));
            continue;
        }
        if shape.kind == Some(kind) {
            continue;
        }
        // Quotes wrap whatever the line is; lists replace another list marker.
        if kind == LineKind::Quote {
            edits.push((start..start, "> ".into()));
            continue;
        }
        let replaced = if shape.kind == Some(LineKind::Quote) {
            start
        } else {
            end
        };
        let insert = match kind {
            LineKind::Bullet => "- ".to_owned(),
            LineKind::Numbered => format!("{}. ", index + 1),
            _ if shape.kind == Some(LineKind::Numbered) => {
                format!("{}[ ] ", list_marker(&text[line.clone()]))
            }
            _ => "- [ ] ".to_owned(),
        };
        edits.push((start..replaced, insert));
    }
    Plan {
        edits,
        selection: None,
    }
}

/// A numbered item's marker with the space after it, as in `1. `.
fn list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &trimmed[digits..];
    if digits == 0 || !(rest.starts_with('.') || rest.starts_with(')')) {
        return "- ";
    }
    let spaces = rest[1..].len() - rest[1..].trim_start().len();
    &trimmed[..digits + 1 + spaces]
}

/// Make the caret's line a heading of `level`, or plain text for 0.
fn set_heading(text: &str, from: usize, level: u8) -> Plan {
    let line = line_at(text, from);
    let content = &text[line.clone()];
    let hashes = content.bytes().take_while(|b| *b == b'#').count();
    let spaces = content[hashes..].len() - content[hashes..].trim_start().len();
    let existing = if (1..=6).contains(&hashes) && spaces > 0 {
        hashes + spaces
    } else {
        0
    };
    let marker = if level == 0 {
        String::new()
    } else {
        "#".repeat(level as usize) + " "
    };
    let edits = if text[line.start..line.start + existing] == marker {
        Edits::new()
    } else {
        vec![(line.start..line.start + existing, marker)]
    };
    Plan {
        edits,
        selection: None,
    }
}

fn insert_link(text: &str, from: usize, to: usize) -> Plan {
    let selected = &text[from..to];
    if URL_LIKE.is_match(selected) {
        // A selected URL becomes the target; type the label next.
        return Plan {
            edits: vec![(from..to, format!("[]({selected})"))],
            selection: Some((from + 1, from + 1)),
        };
    }
    // Selected text becomes the label; type the target next.
    let caret = from + selected.len() + if selected.is_empty() { 1 } else { 3 };
    Plan {
        edits: vec![(from..to, format!("[{selected}]()"))],
        selection: Some((caret, caret)),
    }
}

/// Mod-Enter: check or uncheck each selected task, or turn the line into a
/// task first, like Obsidian's "Toggle checkbox status".
fn toggle_tasks(text: &str, from: usize, to: usize) -> Plan {
    let mut edits = Edits::new();
    for line in selected_lines(text, from, to) {
        let content = &text[line.clone()];
        if let Some((check, checked)) = task_marker(content) {
            let check = line.start + check.start..line.start + check.end;
            edits.push((check, if checked { "[ ]" } else { "[x]" }.into()));
            continue;
        }
        let shape = line_shape(content);
        let (at, insert) = match shape.kind {
            Some(LineKind::Bullet | LineKind::Numbered) => (shape.prefix, "[ ] "),
            Some(LineKind::Quote) => (shape.prefix, "- [ ] "),
            _ => (shape.indent, "- [ ] "),
        };
        edits.push((line.start + at..line.start + at, insert.into()));
    }
    Plan {
        edits,
        selection: None,
    }
}

/// How wide whitespace draws, with a tab four columns wide.
fn visual_width(whitespace: &str) -> usize {
    whitespace
        .chars()
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// What a child's indent holds past its item's own indent.
fn deeper_than<'a>(child_indent: &'a str, own: &str) -> &'a str {
    child_indent.get(own.len()..).unwrap_or("")
}

/// The text's lines as byte ranges without their `\n`.
fn all_lines(text: &str) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(start..index);
            start = index + 1;
        }
    }
    lines.push(start..text.len());
    lines
}

/// The item a list line belongs under, searching upward for an item indented
/// at most `below` columns: its line, its marker prefix's length and its
/// indent's width.
fn previous_item(
    text: &str,
    lines: &[Range<usize>],
    index: usize,
    below: usize,
) -> Option<(usize, usize, usize)> {
    for number in (0..index).rev() {
        let line = &text[lines[number].clone()];
        if line.trim().is_empty() {
            continue;
        }
        let width = visual_width(indent_of(line));
        let Some(item) = LIST_ITEM.find(line) else {
            // Paragraph continuation lines are indented under their item.
            if width > 0 {
                continue;
            }
            return None;
        };
        if width <= below {
            return Some((number, item.len(), width));
        }
    }
    None
}

/// The last line of a list item with its nested children.
fn item_block(text: &str, lines: &[Range<usize>], index: usize) -> usize {
    let width = visual_width(indent_of(&text[lines[index].clone()]));
    let mut last = index;
    for (number, line) in lines.iter().enumerate().skip(index + 1) {
        let line = &text[line.clone()];
        if line.trim().is_empty() {
            continue;
        }
        if visual_width(indent_of(line)) <= width {
            break;
        }
        last = number;
    }
    last
}

/// The selected lines, when every one of them is a list item.
fn list_lines(text: &str, lines: &[Range<usize>], from: usize, to: usize) -> Option<Vec<usize>> {
    let first = lines.partition_point(|line| line.end < from);
    let last = lines.partition_point(|line| line.end < to.max(from));
    let selected: Vec<usize> = (first..=last.min(lines.len() - 1)).collect();
    selected
        .iter()
        .all(|number| LIST_ITEM.is_match(&text[lines[*number].clone()]))
        .then_some(selected)
}

/// Tab on a list item nests it under the item above, aligned with that
/// item's text so ordered lists nest too (two spaces aren't enough under
/// `1. `). Nested children move with their item. `None` outside a list.
fn indent_list_item(text: &str, from: usize, to: usize) -> Option<Plan> {
    let lines = all_lines(text);
    let selected = list_lines(text, &lines, from, to)?;
    let mut edits = Edits::new();
    let mut moved = vec![false; lines.len()];
    for number in selected {
        if moved[number] {
            continue;
        }
        let line = &text[lines[number].clone()];
        let own = indent_of(line);
        let Some((sibling, marker, sibling_width)) =
            previous_item(text, &lines, number, visual_width(own))
        else {
            continue;
        };
        if sibling_width != visual_width(own) {
            continue;
        }
        let sibling_line = &text[lines[sibling].clone()];
        let sibling_indent = indent_of(sibling_line);
        let target = if sibling_indent.contains('\t') || own.contains('\t') {
            format!("{sibling_indent}\t")
        } else {
            " ".repeat(marker)
        };
        if visual_width(&target) <= visual_width(own) {
            continue;
        }
        let last = item_block(text, &lines, number);
        for (child, flag) in moved.iter_mut().enumerate().take(last + 1).skip(number) {
            *flag = true;
            let child_range = lines[child].clone();
            let child_line = &text[child_range.clone()];
            if child_line.trim().is_empty() {
                continue;
            }
            let child_indent = indent_of(child_line);
            let next = if child == number {
                target.clone()
            } else {
                format!("{target}{}", deeper_than(child_indent, own))
            };
            edits.push((
                child_range.start..child_range.start + child_indent.len(),
                next,
            ));
            // A nested ordered item starts its own numbering.
            if child == number
                && let Some(ordered) = ORDERED.captures(line)
                && sibling_line
                    .trim_start()
                    .starts_with(|c: char| c.is_ascii_digit())
            {
                let digits = ordered.get(2).expect("the pattern has a number");
                let start = child_range.start + digits.start();
                edits.push((start..start + digits.len(), "1".into()));
            }
        }
    }
    Some(Plan {
        edits,
        selection: None,
    })
}

/// Shift-Tab on a list item moves it and its children out one level.
/// `None` outside a list.
fn outdent_list_item(text: &str, from: usize, to: usize) -> Option<Plan> {
    let lines = all_lines(text);
    let selected = list_lines(text, &lines, from, to)?;
    let mut edits = Edits::new();
    let mut moved = vec![false; lines.len()];
    for number in selected {
        if moved[number] {
            continue;
        }
        let own = indent_of(&text[lines[number].clone()]);
        if own.is_empty() {
            continue;
        }
        let target = match previous_item(text, &lines, number, visual_width(own) - 1) {
            Some((parent, _, _)) => indent_of(&text[lines[parent].clone()]).to_owned(),
            None => String::new(),
        };
        let last = item_block(text, &lines, number);
        for (child, flag) in moved.iter_mut().enumerate().take(last + 1).skip(number) {
            *flag = true;
            let child_range = lines[child].clone();
            let child_line = &text[child_range.clone()];
            if child_line.trim().is_empty() {
                continue;
            }
            let child_indent = indent_of(child_line);
            let next = format!("{target}{}", deeper_than(child_indent, own));
            edits.push((
                child_range.start..child_range.start + child_indent.len(),
                next,
            ));
        }
    }
    Some(Plan {
        edits,
        selection: None,
    })
}

impl MdDoc {
    /// The selection as byte offsets `(anchor, caret)`.
    fn selection_bytes(&self) -> (usize, usize) {
        let segs = &self.buffer.current.segs;
        let (anchor, caret) = self.buffer.current.selection;
        (segs.offset_to_byte(anchor).0, segs.offset_to_byte(caret).0)
    }

    /// The selection as byte offsets, smaller first.
    fn selection_span(&self) -> (usize, usize) {
        let (anchor, caret) = self.selection_bytes();
        (anchor.min(caret), anchor.max(caret))
    }

    /// Runs a formatting command on the selection as one undo step.
    pub(crate) fn run_format(&mut self, format: Format) -> buffer::Response {
        let text = self.text();
        let (from, to) = self.selection_span();
        let plan = match format {
            Format::Bold => toggle_inline(text, from, to, BOLD),
            Format::Italic => toggle_inline(text, from, to, ITALIC),
            Format::Strikethrough => toggle_inline(text, from, to, STRIKETHROUGH),
            Format::Code => toggle_inline(text, from, to, CODE),
            Format::Underline => toggle_pair(text, from, to, "<u>", "</u>"),
            Format::Link => insert_link(text, from, to),
            Format::BulletList => toggle_lines(text, from, to, LineKind::Bullet),
            Format::NumberedList => toggle_lines(text, from, to, LineKind::Numbered),
            Format::CheckList => toggle_lines(text, from, to, LineKind::Check),
            Format::BlockQuote => toggle_lines(text, from, to, LineKind::Quote),
            Format::Heading(level) => set_heading(text, from, level.min(6)),
        };
        self.commit_plan(plan)
    }

    /// Mod-Enter: checks or unchecks the task on each selected line, adds a
    /// checkbox to a list item, and turns a plain line into a task.
    pub(crate) fn run_toggle_task_line(&mut self) -> buffer::Response {
        let (from, to) = self.selection_span();
        let plan = toggle_tasks(self.text(), from, to);
        self.commit_plan(plan)
    }

    /// Tab or Shift-Tab on list items. Changes nothing outside a list.
    pub(crate) fn run_indent(&mut self, deindent: bool) -> buffer::Response {
        let (from, to) = self.selection_span();
        let plan = if deindent {
            outdent_list_item(self.text(), from, to)
        } else {
            indent_list_item(self.text(), from, to)
        };
        match plan {
            Some(plan) => self.commit_plan(plan),
            None => buffer::Response::default(),
        }
    }

    /// Whether Tab and Shift-Tab restructure the list the selection is in.
    /// Outside a list they change no text, and the window can move the
    /// keyboard to the next control instead.
    pub fn tab_indents(&self) -> bool {
        let (from, to) = self.selection_span();
        let text = self.text();
        list_lines(text, &all_lines(text), from, to).is_some()
    }

    /// The formats in effect at the selection, for the toolbar's pressed
    /// states.
    pub fn formats(&mut self) -> Vec<Format> {
        let arena = comrak::Arena::new();
        let root = self.reparse(&arena);
        self.formats_in(root)
    }

    /// [`Self::formats`] against a parse of the current text.
    pub fn formats_in<'a>(&self, root: &'a AstNode<'a>) -> Vec<Format> {
        let mut formats = Vec::new();
        let mut add = |format| {
            if !formats.contains(&format) {
                formats.push(format);
            }
        };
        let text = self.text();
        let (from, to) = self.selection_span();
        let segs = &self.buffer.current.segs;
        let selection = (
            segs.offset_to_char(Byte(from)),
            segs.offset_to_char(Byte(to)),
        );
        // Only the nodes around the selection can hold it, so the walk
        // descends into those alone.
        let mut around = vec![root];
        while let Some(node) = around.pop() {
            for child in node.children() {
                if !self
                    .node_range(child)
                    .contains_range(&selection, true, true)
                {
                    continue;
                }
                match child.data.borrow().value {
                    NodeValue::Strong => add(Format::Bold),
                    NodeValue::Emph => add(Format::Italic),
                    NodeValue::Strikethrough => add(Format::Strikethrough),
                    NodeValue::Code(_) => add(Format::Code),
                    NodeValue::Link(_) => add(Format::Link),
                    _ => {}
                }
                around.push(child);
            }
        }
        for &char in b"*_" {
            let (from, to) = normalize_inline(text, from, to, char);
            let before = run_before(text, from, char);
            let after = run_after(text, to, char);
            if before >= 2 && after >= 2 {
                add(Format::Bold);
            }
            if before % 2 == 1 && after % 2 == 1 {
                add(Format::Italic);
            }
        }
        if text[..from].ends_with("<u>") && text[to..].starts_with("</u>") {
            add(Format::Underline);
        }
        let (_, caret) = self.selection_bytes();
        if let Some(kind) = line_shape(&text[line_at(text, caret)]).kind {
            add(kind.format());
        }
        formats
    }

    /// The caret line's paragraph style: its heading level, or 0 for text.
    pub fn paragraph_style(&self) -> u8 {
        let text = self.text();
        let (_, caret) = self.selection_bytes();
        let line = &text[line_at(text, caret)];
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        let spaced = line[hashes..].starts_with(char::is_whitespace);
        if (1..=6).contains(&hashes) && spaced {
            hashes as u8
        } else {
            0
        }
    }

    /// Applies a plan's edits as one batch and then sets the selection in a
    /// batch of its own, which undo and redo join to the edits.
    fn commit_plan(&mut self, plan: Plan) -> buffer::Response {
        let (anchor, caret) = self.selection_bytes();
        let selection = match plan.selection {
            // A selection made leftward stays leftward.
            Some((start, end)) if caret < anchor => (end, start),
            Some(selection) => selection,
            None => (
                map_position(&plan.edits, anchor),
                map_position(&plan.edits, caret),
            ),
        };
        let segs = &self.buffer.current.segs;
        let to_range = |range: &Range<usize>| -> (Grapheme, Grapheme) {
            let start = segs.byte_to_char_floor(Byte(range.start));
            let end = segs.byte_to_char_ceil(Byte(range.end)).max(start);
            (start, end)
        };
        let ops: Vec<Operation> = plan
            .edits
            .iter()
            .filter(|(range, text)| !range.is_empty() || !text.is_empty())
            .map(|(range, text)| {
                Operation::Replace(Replace {
                    range: to_range(range),
                    text: text.clone(),
                })
            })
            .collect();
        let mut response = buffer::Response::default();
        if !ops.is_empty() {
            self.buffer.queue(ops);
            response |= self.buffer.update();
        }
        let segs = &self.buffer.current.segs;
        let selection = (
            segs.byte_to_char_floor(Byte(selection.0)),
            segs.byte_to_char_floor(Byte(selection.1)),
        );
        if selection != self.buffer.current.selection {
            self.buffer.queue(vec![Operation::Select(selection)]);
            response |= self.buffer.update();
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_move_with_the_text_around_them() {
        let edits: Edits = vec![(0..0, "- ".into()), (4..6, "x".into())];
        assert_eq!(map_position(&edits, 0), 2);
        assert_eq!(map_position(&edits, 3), 5);
        assert_eq!(map_position(&edits, 4), 6);
        assert_eq!(map_position(&edits, 5), 7);
        assert_eq!(map_position(&edits, 6), 7);
        assert_eq!(map_position(&edits, 9), 10);
    }

    #[test]
    fn line_shapes_name_their_prefix() {
        let shape = line_shape("  - [x] done");
        assert_eq!(
            (shape.indent, shape.prefix, shape.kind),
            (2, 8, Some(LineKind::Check))
        );
        let shape = line_shape("> quote");
        assert_eq!((shape.prefix, shape.kind), (2, Some(LineKind::Quote)));
        assert_eq!(line_shape("12) twelve").kind, Some(LineKind::Numbered));
        assert_eq!(line_shape("-dash").kind, None);
    }

    #[test]
    fn a_task_marker_needs_space_or_the_end_after_its_box() {
        assert_eq!(task_marker("- [ ] a"), Some((2..5, false)));
        assert_eq!(task_marker("1. [X]"), Some((3..6, true)));
        assert_eq!(task_marker("- [ ]a"), None);
    }

    #[test]
    fn list_markers_keep_their_number_and_spacing() {
        assert_eq!(list_marker("  3.  three"), "3.  ");
        assert_eq!(list_marker("- bullet"), "- ");
    }
}
