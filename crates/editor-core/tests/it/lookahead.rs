//! Seam 2: frontmatter shown in the text and `$$` math count only when
//! their closing line falls within a 256 KiB lookahead, and otherwise parse
//! as ordinary Markdown. Frontmatter keys get no heading, tag or link
//! styling.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;

const LOOKAHEAD: usize = 256 * 1024;

fn constructs(text: &str) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    let constructs = doc.constructs();
    assert_eq!(doc.text(), text);
    constructs
}

fn kinds(text: &str) -> Vec<ConstructKind> {
    constructs(text)
        .into_iter()
        .map(|construct| construct.kind)
        .collect()
}

/// The source text of `range` in an ASCII note.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    &text[range.0.0..range.1.0]
}

/// Lines of 1 KiB, `kib` of them, each starting with `start`.
fn filler(start: &str, kib: usize) -> String {
    let line = format!("{start}{}\n", "x".repeat(1023 - start.len()));
    line.repeat(kib)
}

#[test]
fn frontmatter_keys_get_no_heading_tag_or_link_styling() {
    let text =
        "---\ntitle: Plan\ntags: [a, \"#b\"]\nlink: \"[[Note]]\"\nmore: text\n---\n\nBody #tag\n";

    let found: Vec<(ConstructKind, &str, Vec<&str>)> = constructs(text)
        .iter()
        .map(|construct| {
            (
                construct.kind,
                source(text, construct.range),
                construct
                    .syntax
                    .iter()
                    .map(|range| source(text, *range))
                    .collect(),
            )
        })
        .collect();

    assert_eq!(
        found,
        vec![
            (
                ConstructKind::Frontmatter,
                "---\ntitle: Plan\ntags: [a, \"#b\"]\nlink: \"[[Note]]\"\nmore: text\n---",
                vec!["---", "---"]
            ),
            (ConstructKind::Tag, "#tag", vec![]),
        ]
    );
}

#[test]
fn frontmatter_closes_with_dashes_or_dots_and_spaces_after_them() {
    for text in [
        "---  \ntitle: x\n---\nBody\n",
        "---\ntitle: x\n...\nBody\n",
        "---\ntitle: x\n---\t\nBody\n",
        "---\r\ntitle: x\r\n---\r\nBody\n",
    ] {
        assert_eq!(kinds(text)[0], ConstructKind::Frontmatter, "{text:?}");
    }
}

#[test]
fn an_unclosed_opening_rule_is_ordinary_markdown() {
    assert_eq!(
        kinds("---\n# Heading\n"),
        vec![ConstructKind::Heading { level: 1 }]
    );
    assert!(!kinds("---\ntext\n").contains(&ConstructKind::Frontmatter));
}

#[test]
fn frontmatter_closed_past_the_lookahead_is_ordinary_markdown() {
    let near = format!("---\n{}---\n# After\n", filler("key: ", 200));
    let far = format!("---\n{}---\n# After\n", filler("key: ", 300));
    assert!(near.len() < LOOKAHEAD && far.len() > LOOKAHEAD);

    assert_eq!(
        kinds(&near),
        vec![
            ConstructKind::Frontmatter,
            ConstructKind::Heading { level: 1 }
        ]
    );
    let far_kinds = kinds(&far);
    assert!(!far_kinds.contains(&ConstructKind::Frontmatter));
    assert!(far_kinds.contains(&ConstructKind::Heading { level: 1 }));
}

#[test]
fn display_math_closed_past_the_lookahead_is_ordinary_markdown() {
    let near = format!("$$\n*a*\n{}$$\n", filler("b ", 200));
    let far = format!("$$\n*a*\n{}$$\n", filler("b ", 300));

    assert_eq!(kinds(&near), vec![ConstructKind::Math { display: true }]);
    let far_kinds = kinds(&far);
    assert!(!far_kinds.contains(&ConstructKind::Math { display: true }));
    assert!(far_kinds.contains(&ConstructKind::Emphasis));
}

/// A lone carriage return inside frontmatter used to make the editor read
/// every line after it one line off, and a callout there drew its type
/// from the line above.
#[test]
fn a_lone_carriage_return_in_frontmatter_shifts_nothing_after_it() {
    let text = "---\na\rb\n---\n> [!note] A\n> body\n";

    let found: Vec<(ConstructKind, &str, Option<String>)> = constructs(text)
        .into_iter()
        .map(|construct| {
            (
                construct.kind,
                source(text, construct.range),
                construct.label,
            )
        })
        .collect();

    assert_eq!(
        found,
        vec![
            (ConstructKind::Frontmatter, "---\na\rb\n---", None),
            (
                ConstructKind::Callout,
                "> [!note] A\n> body",
                Some("Note".to_string())
            ),
        ]
    );
}
