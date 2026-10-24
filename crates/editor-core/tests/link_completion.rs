//! Seam 2: completing a `[[` link. The text typed after `[[` is the query
//! until a `#`, `|`, `]` or a new line ends it, and picking a file puts its
//! link in place of the query and adds the closing `]]` unless it already
//! follows.

use editor_core::MdDoc;
use editor_core::input::{Event, Location, NoLayout, Region};
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

/// What the `[[` link at the caret has typed so far.
fn query(doc: &mut MdDoc) -> Option<String> {
    doc.link_query().map(|query| query.typed)
}

fn pick(doc: &mut MdDoc, link: &str) {
    doc.apply(
        [Event::CompleteLink {
            link: link.to_owned(),
        }],
        &NoLayout,
    );
}

#[test]
fn the_query_is_what_follows_the_brackets_until_it_ends() {
    let mut doc = MdDoc::new("");
    type_text(&mut doc, "See [");
    assert_eq!(query(&mut doc), None);
    type_text(&mut doc, "[");
    assert_eq!(query(&mut doc).as_deref(), Some(""));
    type_text(&mut doc, "Old no");
    assert_eq!(query(&mut doc).as_deref(), Some("Old no"));

    for end in ["#", "|", "]", "\n"] {
        let mut doc = MdDoc::new("");
        type_text(&mut doc, "See [[Plan");
        type_text(&mut doc, end);
        assert_eq!(query(&mut doc), None, "{end:?}");
    }
}

#[test]
fn there_is_no_query_in_code_in_plain_text_or_with_a_selection() {
    let mut doc = MdDoc::new("```\n\n```\n");
    caret_at(&mut doc, 4);
    type_text(&mut doc, "[[Plan");
    assert_eq!(query(&mut doc), None);

    let mut doc = MdDoc::new("`[[Plan` and more\n");
    caret_at(&mut doc, 7);
    assert_eq!(query(&mut doc), None);

    let mut doc = MdDoc::new("");
    doc.plaintext = true;
    type_text(&mut doc, "[[Plan");
    assert_eq!(query(&mut doc), None);

    let mut doc = MdDoc::new("[[Plan");
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(g(2)),
                end: Location::Grapheme(g(6)),
            },
        }],
        &NoLayout,
    );
    assert_eq!(query(&mut doc), None);
}

#[test]
fn picking_a_file_inserts_its_link_and_closes_the_brackets() {
    let mut doc = MdDoc::new("");
    type_text(&mut doc, "See [[pl");
    pick(&mut doc, "Projects/Plan");
    assert_eq!(doc.text(), "See [[Projects/Plan]]");
    assert_eq!(doc.selection(), (g(21), g(21)));
    assert_eq!(query(&mut doc), None);

    // Text after the caret stays after the link.
    let mut doc = MdDoc::new("[[ and more\n");
    caret_at(&mut doc, 2);
    type_text(&mut doc, "pl");
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "[[Plan]] and more\n");
    assert_eq!(doc.selection(), (g(8), g(8)));
}

#[test]
fn picking_a_file_keeps_brackets_that_already_follow() {
    // The closing brackets right after the caret.
    let mut doc = MdDoc::new("See [[]] here\n");
    caret_at(&mut doc, 6);
    type_text(&mut doc, "pl");
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "See [[Plan]] here\n");
    assert_eq!(doc.selection(), (g(12), g(12)));

    // Inside a closed link, the whole target goes.
    let mut doc = MdDoc::new("[[Old no]]\n");
    caret_at(&mut doc, 5);
    assert_eq!(query(&mut doc).as_deref(), Some("Old"));
    pick(&mut doc, "Old notes");
    assert_eq!(doc.text(), "[[Old notes]]\n");
    assert_eq!(doc.selection(), (g(13), g(13)));

    // A heading or display text after the target stays.
    let mut doc = MdDoc::new("[[pl|the plan]] and [[pl#Goals]]\n");
    caret_at(&mut doc, 4);
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "[[Plan|the plan]] and [[pl#Goals]]\n");
    assert_eq!(doc.selection(), (g(6), g(6)));
    caret_at(&mut doc, 26);
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "[[Plan|the plan]] and [[Plan#Goals]]\n");
    assert_eq!(doc.selection(), (g(28), g(28)));
}

#[test]
fn picking_a_file_keeps_later_text_on_the_line() {
    // A tag later in the sentence isn't the link's heading.
    let mut doc = MdDoc::new("See [[ about #budget\n");
    caret_at(&mut doc, 6);
    type_text(&mut doc, "pl");
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "See [[Plan]] about #budget\n");
    assert_eq!(doc.selection(), (g(12), g(12)));

    // A pipe later on the line isn't the link's display text.
    let mut doc = MdDoc::new("See [[ with a | pipe\n");
    caret_at(&mut doc, 6);
    type_text(&mut doc, "pl");
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "See [[Plan]] with a | pipe\n");

    // Nor is the next cell of a table row.
    let mut doc = MdDoc::new("| a | b |\n| - | - |\n| [[ x | y |\n");
    caret_at(&mut doc, 24);
    type_text(&mut doc, "pl");
    assert_eq!(query(&mut doc).as_deref(), Some("pl"));
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "| a | b |\n| - | - |\n| [[Plan]] x | y |\n");

    // A single bracket doesn't close the link, and stays.
    let mut doc = MdDoc::new("See [[] here\n");
    caret_at(&mut doc, 6);
    type_text(&mut doc, "pl");
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "See [[Plan]]] here\n");
    assert_eq!(doc.selection(), (g(12), g(12)));

    // A link that closes only on a later line isn't closed here.
    let mut doc = MdDoc::new("See [[\nmore]]\n");
    caret_at(&mut doc, 6);
    type_text(&mut doc, "pl");
    pick(&mut doc, "Plan");
    assert_eq!(doc.text(), "See [[Plan]]\nmore]]\n");
}

#[test]
fn undo_takes_back_the_picked_link() {
    let mut doc = MdDoc::new("");
    type_text(&mut doc, "[[pl");
    pick(&mut doc, "Plan");
    doc.apply([Event::Undo], &NoLayout);
    assert_eq!(doc.text(), "[[pl");
}
