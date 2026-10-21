//! Seam 2: the constructs `editor-core` reports for the editor to draw in
//! place, with their syntax, and which syntax shows as the caret walks
//! through a note.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;

/// A note with one of each construct the editor draws in place.
const NOTE: &str = "# Heading one\n\
\n\
Some *emphasis*, **strong**, ~~struck~~, `code` and [a link](https://example.com).\n\
\n\
- item\n\
- [ ] open task\n\
- [x] done task\n\
\n\
> quoted\n\
\n\
```rust\n\
fn main() {}\n\
```\n";

fn constructs_with_caret(text: &str, selection: Option<(usize, usize)>) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.reveal_selection = selection.map(|(anchor, caret)| (Grapheme(anchor), Grapheme(caret)));
    doc.bump_reveal_seq();
    doc.constructs()
}

/// The source text of `range` in an ASCII note.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    &text[range.0.0..range.1.0]
}

#[test]
fn the_editor_reports_each_construct_it_draws_in_place_with_its_syntax() {
    let constructs = constructs_with_caret(NOTE, None);
    let found: Vec<(ConstructKind, &str, Vec<&str>)> = constructs
        .iter()
        .map(|construct| {
            (
                construct.kind,
                source(NOTE, construct.range),
                construct
                    .syntax
                    .iter()
                    .map(|range| source(NOTE, *range))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            (
                ConstructKind::Heading { level: 1 },
                "# Heading one",
                vec!["# "]
            ),
            (ConstructKind::Emphasis, "*emphasis*", vec!["*", "*"]),
            (ConstructKind::Strong, "**strong**", vec!["**", "**"]),
            (ConstructKind::Strikethrough, "~~struck~~", vec!["~~", "~~"]),
            (ConstructKind::InlineCode, "`code`", vec!["`", "`"]),
            (
                ConstructKind::Link,
                "[a link](https://example.com)",
                vec!["[", "](https://example.com)"]
            ),
            (ConstructKind::ListItem, "- item", vec!["- "]),
            (
                ConstructKind::TaskItem { checked: false },
                "- [ ] open task",
                vec!["- [ ] "]
            ),
            (
                ConstructKind::TaskItem { checked: true },
                "- [x] done task",
                vec!["- [x] "]
            ),
            (ConstructKind::BlockQuote, "> quoted", vec!["> "]),
            (
                ConstructKind::CodeBlock,
                "```rust\nfn main() {}\n```",
                vec!["```rust", "```"]
            ),
        ]
    );
    assert!(
        constructs.iter().all(|construct| !construct.revealed),
        "nothing shows its syntax without a caret"
    );
}

/// Whether the caret at `caret` should show `construct`'s syntax: inside
/// or at either end of an inline construct, anywhere on a heading's or a
/// code block's lines, and strictly inside the marker of a list item, task
/// item or quote, whose marker draws as one bullet, box or bar.
fn expected(text: &str, construct: &Construct, caret: usize) -> bool {
    let (start, end) = (construct.range.0.0, construct.range.1.0);
    match construct.kind {
        ConstructKind::Heading { .. } | ConstructKind::CodeBlock => {
            let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
            let line_end = text[end..]
                .find('\n')
                .map_or(text.len(), |index| end + index);
            (line_start..=line_end).contains(&caret)
        }
        ConstructKind::ListItem | ConstructKind::TaskItem { .. } | ConstructKind::BlockQuote => {
            let marker = construct.syntax[0];
            marker.0.0 < caret && caret < marker.1.0
        }
        _ => (start..=end).contains(&caret),
    }
}

/// Walks the caret through every position of `text` and checks, at each,
/// which constructs show their syntax.
fn walk(text: &str) {
    let kinds = constructs_with_caret(text, None).len();
    for caret in 0..=text.len() {
        let constructs = constructs_with_caret(text, Some((caret, caret)));
        assert_eq!(constructs.len(), kinds, "the caret changes no construct");
        for construct in &constructs {
            assert_eq!(
                construct.revealed,
                expected(text, construct, caret),
                "{:?} {:?} with the caret at {caret}",
                construct.kind,
                source(text, construct.range),
            );
        }
    }
}

#[test]
fn syntax_shows_while_the_caret_is_in_a_construct_and_hides_when_it_leaves() {
    walk(NOTE);
}

#[test]
fn long_constructs_reveal_and_hide_like_short_ones() {
    let long_word = "word ".repeat(120);
    let long_url = format!("https://example.com/{}", "segment/".repeat(60));
    let text = format!(
        "## {long_word}\n\nA **{long_word}** then *a* and [{long_word}]({long_url}) end.\n\n`{long_word}`\n"
    );
    walk(&text);
}

#[test]
fn a_selection_that_reaches_into_a_construct_shows_its_syntax() {
    let text = "plain **bold** plain";
    let strong = |selection| {
        constructs_with_caret(text, Some(selection))
            .into_iter()
            .find(|construct| construct.kind == ConstructKind::Strong)
            .unwrap()
            .revealed
    };
    assert!(strong((0, 9)), "a selection from before into the bold");
    assert!(strong((18, 9)), "a backward selection from after into it");
    assert!(strong((0, 20)), "a selection over the whole of it");
    assert!(!strong((0, 4)), "a selection that ends before it");
    assert!(!strong((16, 20)), "a selection that starts after it");
}

/// The type and title ranges of the first callout in `doc`.
fn callout_ranges(doc: &mut MdDoc) -> ((Grapheme, Grapheme), (Grapheme, Grapheme)) {
    use editor_core::comrak::{Arena, nodes::NodeValue};

    let arena = Arena::new();
    let root = doc.reparse(&arena);
    let callout = root
        .descendants()
        .find(|node| matches!(node.data.borrow().value, NodeValue::Alert(_)))
        .expect("the note holds a callout");
    doc.alert_type_title_ranges(callout)
}

#[test]
fn a_callout_reads_its_type_and_title() {
    let text = "> [!note] A title\n";
    let mut doc = MdDoc::new(text);

    let (kind, title) = callout_ranges(&mut doc);

    assert_eq!(source(text, kind), "[!note]");
    assert_eq!(source(text, title), "A title");
}

/// Typing `---` and a line break at the start of a note whose first line
/// holds a lone carriage return makes frontmatter that comrak counts one
/// line short, so the editor reads the callout's first line from the line
/// above it. The callout then has no type and its whole line as a title,
/// where the editor used to panic.
#[test]
fn a_callout_read_from_the_wrong_line_has_no_type() {
    use editor_core::input::{Event, Location, NoLayout, Region};

    let mut doc = MdDoc::new("a\rb\n---\n> [!note] A\n");
    doc.apply(
        [Event::Select {
            region: Region::Location(Location::Grapheme(Grapheme(0))),
        }],
        &NoLayout,
    );
    for typed in ["-", "-", "-", "\n"] {
        doc.apply(
            [Event::Replace {
                region: Region::Selection,
                text: typed.into(),
                advance_cursor: true,
            }],
            &NoLayout,
        );
    }
    assert_eq!(doc.text(), "---\na\rb\n---\n> [!note] A\n");

    let (kind, title) = callout_ranges(&mut doc);

    assert_eq!(kind.0, kind.1, "no type");
    assert!(title.0 <= title.1 && title.1.0 <= doc.text().len());
}
