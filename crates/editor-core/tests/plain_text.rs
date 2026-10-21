//! Seam 2: a plain-text or code file in the editor. It shows its source
//! as it is: no Markdown construct is drawn in place, and Return,
//! Backspace and Tab edit the text without Markdown's list rules.

use editor_core::MdDoc;
use editor_core::input::{Advance, Event, Increment, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

/// A document in the editor's plain-text mode holding `text`.
fn plain(text: &str) -> MdDoc {
    let mut doc = MdDoc::new(text);
    doc.plaintext = true;
    doc
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
fn a_plain_text_file_draws_no_markdown_in_place() {
    let text = "# not a heading\n\nSome *stars*, `ticks` and [brackets](x).\n\n\
                - dash\n- [ ] box\n\n> angle\n\n```\nfence\n```\n";
    let mut doc = plain(text);
    doc.reveal_selection = None;
    doc.bump_reveal_seq();

    assert_eq!(doc.constructs(), Vec::new());
    // The same text as a note draws its constructs in place.
    assert!(!MdDoc::new(text).constructs().is_empty());
}

#[test]
fn return_after_a_dash_line_starts_a_plain_line() {
    let mut doc = plain("- one");
    caret_at(&mut doc, 5);

    doc.apply([Event::Newline { shift: false }], &NoLayout);
    type_text(&mut doc, "two");

    assert_eq!(doc.text(), "- one\ntwo");
    assert_eq!(doc.selection(), (g(9), g(9)));
}

#[test]
fn return_after_an_empty_dash_line_keeps_the_dash() {
    let mut doc = plain("- one\n- ");
    caret_at(&mut doc, 8);

    doc.apply([Event::Newline { shift: false }], &NoLayout);

    assert_eq!(doc.text(), "- one\n- \n");
    assert_eq!(doc.selection(), (g(9), g(9)));
}

#[test]
fn return_in_a_quoted_line_adds_no_quote_marker() {
    let mut doc = plain("> quoted");
    caret_at(&mut doc, 8);

    doc.apply([Event::Newline { shift: false }], &NoLayout);

    assert_eq!(doc.text(), "> quoted\n");
}

#[test]
fn backspace_after_a_dash_deletes_one_character() {
    let mut doc = plain("- one\n- two");
    caret_at(&mut doc, 8);

    doc.apply([backspace()], &NoLayout);

    assert_eq!(doc.text(), "- one\n-two");
    assert_eq!(doc.selection(), (g(7), g(7)));
}

#[test]
fn tab_never_nests_dash_lines() {
    let mut doc = plain("- one\n- two");
    caret_at(&mut doc, 11);

    assert!(!doc.tab_indents());
    doc.apply([Event::Indent { deindent: false }], &NoLayout);
    doc.apply([Event::Indent { deindent: true }], &NoLayout);

    assert_eq!(doc.text(), "- one\n- two");
}
