//! Seam 2: pasting, checking tasks and unfolding sections through
//! `editor-core` alone, with no layout and no UI.

use editor_core::MdDoc;
use editor_core::fold::FOLD_TAG;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use proptest::prelude::*;

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

fn select(doc: &mut MdDoc, start: usize, end: usize) {
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(g(start)),
                end: Location::Grapheme(g(end)),
            },
        }],
        &NoLayout,
    );
}

fn paste(doc: &mut MdDoc, text: &str) {
    doc.apply([Event::Paste { text: text.into() }], &NoLayout);
}

/// The range of the first task item in the note, as the checkbox names it.
fn first_task(doc: &mut MdDoc) -> (Grapheme, Grapheme) {
    let arena = editor_core::comrak::Arena::new();
    let root = doc.reparse(&arena);
    let node = root
        .descendants()
        .find(|node| {
            matches!(
                node.data.borrow().value,
                editor_core::comrak::nodes::NodeValue::TaskItem(_)
            )
        })
        .expect("the note has a task");
    doc.node_range(node)
}

#[test]
fn pasting_onto_a_selection_replaces_it_and_puts_the_caret_after() {
    let mut doc = MdDoc::new("one two three");
    select(&mut doc, 4, 7);

    paste(&mut doc, "2");

    assert_eq!(doc.text(), "one 2 three");
    assert_eq!(doc.selection(), (g(5), g(5)));
}

#[test]
fn pasting_keeps_non_breaking_spaces() {
    let mut doc = MdDoc::new("ab");
    select(&mut doc, 1, 1);

    paste(&mut doc, "x\u{a0}y");

    assert_eq!(doc.text(), "ax\u{a0}yb");
}

#[test]
fn pasting_a_url_onto_selected_text_replaces_the_text_with_the_url() {
    let mut doc = MdDoc::new("see the docs here");
    select(&mut doc, 8, 12);

    paste(&mut doc, "https://example.com");

    assert_eq!(doc.text(), "see the https://example.com here");
}

#[test]
fn pasting_a_url_with_nothing_selected_inserts_it() {
    let mut doc = MdDoc::new("go ");
    select(&mut doc, 3, 3);

    paste(&mut doc, "https://example.com");

    assert_eq!(doc.text(), "go https://example.com");
}

#[test]
fn pasting_a_url_inside_a_link_replaces_the_selection() {
    let mut doc = MdDoc::new("[docs](https://old.example)");
    select(&mut doc, 7, 26);

    paste(&mut doc, "https://new.example");

    assert_eq!(doc.text(), "[docs](https://new.example)");
}

#[test]
fn pasted_windows_line_endings_become_the_newline_return_types() {
    let mut doc = MdDoc::new("ab");
    select(&mut doc, 1, 1);

    paste(&mut doc, "one\r\ntwo\r\n");

    assert_eq!(doc.text(), "aone\ntwo\nb");
    assert_eq!(doc.selection(), (g(9), g(9)));
}

#[test]
fn pasted_markdown_lands_as_copied() {
    let mut doc = MdDoc::new("- item\n");
    select(&mut doc, 6, 6);

    paste(&mut doc, " **bold** [[link]]\n\n# heading\t");

    assert_eq!(doc.text(), "- item **bold** [[link]]\n\n# heading\t\n");
}

proptest! {
    /// Pasting replaces only the selection: every byte before and after
    /// it stays as it was, and the pasted text lands as copied, with
    /// Windows line endings as `\n`.
    #[test]
    fn pasting_changes_no_byte_outside_the_selection(
        before in "[ -~\u{a0}é\n\r*#>\\[\\]-]{0,40}",
        selected in "[ -~é\n]{0,10}",
        after in "[ -~\u{a0}é\n\r*#>\\[\\]-]{0,40}",
        pasted in "[ -~\u{a0}é\n\r\t]{0,30}",
    ) {
        let text = format!("{before}{selected}{after}");
        let mut doc = MdDoc::new(&text);
        let segs = editor_core::unicode_segs::calc(&text);
        // Only whole clusters can be selected.
        let start = segs.byte_to_char_floor(before.len().into());
        let end = segs.byte_to_char_ceil((before.len() + selected.len()).into());
        let (start_byte, end_byte) = (segs.offset_to_byte(start).0, segs.offset_to_byte(end).0);
        select(&mut doc, start.0, end.0);

        paste(&mut doc, &pasted);

        let result = doc.text().as_bytes();
        let expected = pasted.replace("\r\n", "\n");
        prop_assert_eq!(&result[..start_byte], &text.as_bytes()[..start_byte]);
        prop_assert_eq!(&result[result.len() - (text.len() - end_byte)..], &text.as_bytes()[end_byte..]);
        prop_assert_eq!(
            &result[start_byte..result.len() - (text.len() - end_byte)],
            expected.as_bytes()
        );
    }
}

#[test]
fn toggling_a_task_checks_and_unchecks_it() {
    let mut doc = MdDoc::new("- [ ] write the plan\n");
    let task = first_task(&mut doc);

    doc.apply([Event::ToggleTask { node: task }], &NoLayout);
    assert_eq!(doc.text(), "- [x] write the plan\n");

    let task = first_task(&mut doc);
    doc.apply([Event::ToggleTask { node: task }], &NoLayout);
    assert_eq!(doc.text(), "- [ ] write the plan\n");
}

#[test]
fn toggling_an_indented_task_writes_inside_its_brackets() {
    let mut doc = MdDoc::new("- parent\n  - [ ] child\n");
    let task = first_task(&mut doc);

    doc.apply([Event::ToggleTask { node: task }], &NoLayout);

    assert_eq!(doc.text(), "- parent\n  - [x] child\n");
}

#[test]
fn a_read_only_note_ignores_task_toggles() {
    let mut doc = MdDoc::new("- [ ] stay\n");
    doc.readonly = true;
    let task = first_task(&mut doc);

    doc.apply([Event::ToggleTask { node: task }], &NoLayout);

    assert_eq!(doc.text(), "- [ ] stay\n");
}

#[test]
fn unfolding_removes_the_fold_tag() {
    let text = format!("# Heading {FOLD_TAG}\n\nhidden\n");
    let mut doc = MdDoc::new(&text);
    let arena = editor_core::comrak::Arena::new();
    doc.reparse(&arena);
    let tag = doc.bounds.folds[0].tag;

    doc.apply([Event::Unfold { tag }], &NoLayout);

    assert_eq!(doc.text(), "# Heading \n\nhidden\n");
}

#[test]
fn unfolding_a_range_that_holds_no_fold_tag_changes_nothing() {
    let mut doc = MdDoc::new("# Heading\n\ntext\n");

    doc.apply([Event::Unfold { tag: (g(2), g(9)) }], &NoLayout);

    assert_eq!(doc.text(), "# Heading\n\ntext\n");
}
