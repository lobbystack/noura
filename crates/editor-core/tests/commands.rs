//! Seam 2: pasting and checking tasks through `editor-core` alone, with
//! no layout and no UI.

use editor_core::MdDoc;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;

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
fn pasting_a_url_onto_selected_text_links_it() {
    let mut doc = MdDoc::new("see the docs here");
    select(&mut doc, 8, 12);

    paste(&mut doc, "https://example.com");

    assert_eq!(doc.text(), "see the [docs](https://example.com) here");
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
fn unfolding_a_range_that_holds_no_fold_tag_changes_nothing() {
    let mut doc = MdDoc::new("# Heading\n\ntext\n");

    doc.apply([Event::Unfold { tag: (g(2), g(9)) }], &NoLayout);

    assert_eq!(doc.text(), "# Heading\n\ntext\n");
}
