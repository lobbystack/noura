//! Seam 2: typing into a note through `editor-core` alone, with no layout
//! and no UI.

use editor_core::MdDoc;
use editor_core::comrak::nodes::NodeValue;
use editor_core::input::{Advance, Event, Increment, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

fn caret_at(doc: &mut MdDoc, offset: usize) {
    doc.apply(
        [Event::Select {
            region: Region::Location(Location::Grapheme(g(offset))),
        }],
        &NoLayout,
    );
}

fn type_text(doc: &mut MdDoc, text: &str) {
    for ch in text.chars() {
        doc.apply(
            [Event::Replace {
                region: Region::Selection,
                text: ch.to_string(),
                advance_cursor: true,
            }],
            &NoLayout,
        );
    }
}

fn backspace() -> Event {
    Event::Delete {
        region: Region::SelectionOrAdvance {
            advance: Advance::By(Increment::Char),
            backwards: true,
        },
    }
}

#[test]
fn typing_inserts_at_the_caret_and_moves_it() {
    let mut doc = MdDoc::new("# Title\n");
    caret_at(&mut doc, 7);

    type_text(&mut doc, " here");

    assert_eq!(doc.text(), "# Title here\n");
    assert_eq!(doc.selection(), (g(12), g(12)));
}

#[test]
fn return_in_a_list_item_starts_the_next_item() {
    let mut doc = MdDoc::new("- one");
    caret_at(&mut doc, 5);

    doc.apply([Event::Newline { shift: false }], &NoLayout);
    type_text(&mut doc, "two");

    assert_eq!(doc.text(), "- one\n- two");
    assert_eq!(doc.selection(), (g(11), g(11)));
}

#[test]
fn return_in_an_empty_list_item_ends_the_list() {
    let mut doc = MdDoc::new("- one\n- ");
    caret_at(&mut doc, 8);

    doc.apply([Event::Newline { shift: false }], &NoLayout);

    assert_eq!(doc.text(), "- one\n");
    assert_eq!(doc.selection(), (g(6), g(6)));
}

#[test]
fn backspace_deletes_the_character_before_the_caret() {
    let mut doc = MdDoc::new("abc");
    caret_at(&mut doc, 3);

    doc.apply([backspace()], &NoLayout);

    assert_eq!(doc.text(), "ab");
    assert_eq!(doc.selection(), (g(2), g(2)));
}

#[test]
fn typing_over_a_selection_replaces_it() {
    let mut doc = MdDoc::new("hello world");
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(g(6)),
                end: Location::Grapheme(g(11)),
            },
        }],
        &NoLayout,
    );

    type_text(&mut doc, "there");

    assert_eq!(doc.text(), "hello there");
    assert_eq!(doc.selection(), (g(11), g(11)));
}

#[test]
fn bold_wraps_the_selection_in_strong_markers() {
    let mut doc = MdDoc::new("make this bold");
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(g(10)),
                end: Location::Grapheme(g(14)),
            },
        }],
        &NoLayout,
    );

    doc.apply(
        [Event::ToggleStyle {
            region: Region::Selection,
            style: NodeValue::Strong,
        }],
        &NoLayout,
    );

    assert_eq!(doc.text(), "make this **bold**");
}

#[test]
fn undo_takes_back_the_typing() {
    let mut doc = MdDoc::new("note");
    caret_at(&mut doc, 4);
    type_text(&mut doc, "s");

    doc.apply([Event::Undo], &NoLayout);

    assert_eq!(doc.text(), "note");
    assert_eq!(doc.selection(), (g(4), g(4)));
}
