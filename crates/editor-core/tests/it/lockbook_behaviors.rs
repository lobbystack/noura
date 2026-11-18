//! Seam 2: the Lockbook behaviors that changed files or departed from
//! today's preview are gone. Setext headings are headings, `__` is strong,
//! pastes keep non-breaking spaces, and no command writes a fold marker.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;

fn constructs(text: &str) -> Vec<Construct> {
    MdDoc::new(text).constructs()
}

/// The source text of `range` in an ASCII note.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    &text[range.0.0..range.1.0]
}

fn caret_at(doc: &mut MdDoc, offset: usize) {
    doc.apply(
        [Event::Select {
            region: Region::Location(Location::Grapheme(Grapheme(offset))),
        }],
        &NoLayout,
    );
    doc.reveal_selection = Some((Grapheme(offset), Grapheme(offset)));
    doc.bump_reveal_seq();
}

#[test]
fn setext_headings_draw_as_headings() {
    let text = "Title\n=====\n\nSubtitle\n---\n";

    let headings: Vec<(ConstructKind, &str, Vec<&str>)> = constructs(text)
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
        headings,
        vec![
            (
                ConstructKind::Heading { level: 1 },
                "Title\n=====",
                vec!["====="]
            ),
            (
                ConstructKind::Heading { level: 2 },
                "Subtitle\n---",
                vec!["---"]
            ),
        ]
    );
}

#[test]
fn double_underscores_draw_as_strong() {
    let text = "a __bold__ word";

    let found: Vec<(ConstructKind, Vec<&str>)> = constructs(text)
        .iter()
        .map(|construct| {
            (
                construct.kind,
                construct
                    .syntax
                    .iter()
                    .map(|range| source(text, *range))
                    .collect(),
            )
        })
        .collect();

    assert_eq!(found, vec![(ConstructKind::Strong, vec!["__", "__"])]);
}

#[test]
fn pasted_non_breaking_spaces_stay_non_breaking() {
    let mut doc = MdDoc::new("ab");
    caret_at(&mut doc, 1);

    doc.apply(
        [Event::Paste {
            text: "x\u{a0}y".into(),
        }],
        &NoLayout,
    );

    assert_eq!(doc.text(), "ax\u{a0}yb");
}

#[test]
fn folding_commands_write_nothing() {
    let text = "# Heading\n\nbody\n\n- item\n  - child\n";
    let mut doc = MdDoc::new(text);

    caret_at(&mut doc, 3);
    doc.apply([Event::ToggleFold { node: None }], &NoLayout);
    caret_at(&mut doc, 18);
    doc.apply([Event::ToggleFold { node: None }], &NoLayout);
    doc.apply(
        [Event::ToggleFold {
            node: Some((Grapheme(0), Grapheme(9))),
        }],
        &NoLayout,
    );

    assert_eq!(doc.text(), text);
}

#[test]
fn a_fold_marker_in_the_file_stays_as_written() {
    let text = "# Heading <!-- {\"fold\":true} -->\n\nbody\n";
    let mut doc = MdDoc::new(text);

    for offset in 0..=text.len() {
        caret_at(&mut doc, offset);
        doc.apply(
            [Event::Unfold {
                tag: (Grapheme(10), Grapheme(32)),
            }],
            &NoLayout,
        );
    }

    assert_eq!(doc.text(), text);
}
