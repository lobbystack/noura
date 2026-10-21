//! Seam 2: find and replace in a note through `editor-core` alone. The
//! matching cases come from the legacy app's `packages/editor/src/find.test.ts`.

use editor_core::MdDoc;
use editor_core::find::{Find, MATCH_LIMIT};
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

#[test]
fn find_counts_and_steps_through_the_matches() {
    let doc = MdDoc::new("beta alpha beta gamma beta");
    let mut find = Find::default();

    find.set_query(&doc, "beta");
    assert_eq!(find.query(), "beta");
    assert_eq!(find.matches().len(), 3);
    assert_eq!(find.count_label(), "1/3");
    assert_eq!(find.current(), Some((g(0), g(4))));

    find.step(false);
    assert_eq!(find.count_label(), "2/3");
    find.step(false);
    find.step(false);
    assert_eq!(find.count_label(), "1/3", "next wraps to the first match");
    find.step(true);
    assert_eq!(
        find.count_label(),
        "3/3",
        "previous wraps to the last match"
    );

    find.set_query(&doc, "delta");
    assert_eq!(find.count_label(), "No results");
    assert!(!find.step(false));

    find.set_query(&doc, "");
    assert_eq!(find.count_label(), "");
}

#[test]
fn a_query_starts_at_the_nearest_match_at_or_after_the_caret() {
    let mut doc = MdDoc::new("cat dog cat dog");
    caret_at(&mut doc, 5);
    let mut find = Find::default();

    find.set_query(&doc, "cat");

    assert_eq!(find.current(), Some((g(8), g(11))));
}

#[test]
fn queries_are_literal_text_that_ignores_case() {
    let doc = MdDoc::new("a.b axb A.B (x) [x] *x*");
    let mut find = Find::default();

    find.set_query(&doc, "a.b");
    assert_eq!(find.matches(), [(g(0), g(3)), (g(8), g(11))]);

    for query in ["(x)", "[x]", "*x*"] {
        find.set_query(&doc, query);
        assert_eq!(find.matches().len(), 1, "{query}");
    }
}

#[test]
fn case_folding_keeps_offsets_in_the_original_text() {
    // "É" folds to "é"; "ß" stays one character.
    let doc = MdDoc::new("Été straße ÉTÉ");
    let mut find = Find::default();

    find.set_query(&doc, "été");

    assert_eq!(find.matches(), [(g(0), g(3)), (g(11), g(14))]);
}

#[test]
fn the_count_stops_at_a_thousand() {
    let doc = MdDoc::new(&"x ".repeat(MATCH_LIMIT + 5));
    let mut find = Find::default();

    find.set_query(&doc, "x");

    assert_eq!(find.count_label(), "1/1000+");
    assert_eq!(find.matches().len(), MATCH_LIMIT + 5);
}

#[test]
fn replace_changes_the_current_match_and_moves_to_the_next() {
    let mut doc = MdDoc::new("cat dog cat dog cat");
    let mut find = Find::default();
    find.set_query(&doc, "cat");
    find.step(false);

    let edits = find.replace_current("cats");
    doc.apply(edits, &NoLayout);
    find.refresh(&doc);

    assert_eq!(doc.text(), "cat dog cats dog cat");
    // The replacement holds the query again; the next match is the one after.
    assert_eq!(find.current(), Some((g(17), g(20))));
    assert_eq!(find.count_label(), "3/3");
}

#[test]
fn replace_all_changes_every_match_as_one_undo_step() {
    let mut doc = MdDoc::new("Cat dog CAT dog cat");
    let mut find = Find::default();
    find.set_query(&doc, "cat");

    let edits = find.replace_all("bird");
    doc.apply(edits, &NoLayout);
    find.refresh(&doc);

    assert_eq!(doc.text(), "bird dog bird dog bird");
    assert_eq!(find.count_label(), "No results");

    doc.apply([Event::Undo], &NoLayout);
    assert_eq!(doc.text(), "Cat dog CAT dog cat");
}

#[test]
fn matches_follow_edits_to_the_text() {
    let mut doc = MdDoc::new("one two one");
    let mut find = Find::default();
    find.set_query(&doc, "one");
    assert_eq!(find.matches().len(), 2);

    caret_at(&mut doc, 0);
    doc.apply(
        [Event::Replace {
            region: Region::Selection,
            text: "one ".into(),
            advance_cursor: true,
        }],
        &NoLayout,
    );
    find.refresh(&doc);

    assert_eq!(find.matches().len(), 3);
}
