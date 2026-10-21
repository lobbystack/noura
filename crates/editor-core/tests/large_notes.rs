//! Seam 2: typing in a large note reparses only what the edit reaches, so
//! it never waits for a full parse; full parses run in the background, and
//! one of text that has changed since is dropped.

use std::sync::mpsc;
use std::time::Duration;

use editor_core::comrak::nodes::{AstNode, NodeValue};
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use editor_core::{Ast, MdDoc};
use proptest::prelude::*;

/// Trial S1's 1 MB note.
fn large_note() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../editor-egui/tests/fixtures/large-note.md"
    );
    std::fs::read_to_string(path).expect("the 1 MB fixture is checked in")
}

fn caret_at(doc: &mut MdDoc, offset: usize) {
    doc.apply(
        [Event::Select {
            region: Region::Location(Location::Grapheme(Grapheme(offset))),
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

/// The grapheme offset of the end of the first `anchor` in `doc`.
fn after(doc: &MdDoc, anchor: &str) -> usize {
    let byte = doc.text().find(anchor).expect("the note holds it") + anchor.len();
    doc.buffer
        .current
        .segs
        .offset_to_char(editor_core::offset_types::Byte(byte))
        .0
}

/// Whether the parse of `doc`'s text holds text containing `needle`.
fn parse_holds(doc: &mut MdDoc, needle: &str) -> bool {
    doc.with_ast(|_, root| {
        root.descendants().any(|node: &AstNode<'_>| {
            matches!(&node.data.borrow().value, NodeValue::Text(text) if text.contains(needle))
        })
    })
}

/// Every node of a tree, one line each.
fn dump(ast: &Ast) -> Vec<String> {
    let mut out = Vec::new();
    ast.with_root(|root| {
        for node in root.descendants() {
            let data = node.data.borrow();
            out.push(format!("{:?} {}", data.value, data.sourcepos));
        }
    });
    out
}

/// Asserts that `doc`'s parse and bounds are what a fresh document on its
/// text builds.
fn assert_matches_fresh_parse(doc: &mut MdDoc) {
    assert!(parses_as_fresh(doc), "the parse");
    assert_bounds_match_fresh_parse(doc);
}

/// Whether `doc`'s parse is the one a fresh document on its text makes.
fn parses_as_fresh(doc: &mut MdDoc) -> bool {
    let expected = MdDoc::new(doc.text()).take_ast();
    let got = doc.take_ast();
    let same = dump(&got) == dump(&expected);
    doc.put_ast(got);
    same
}

/// Asserts that `doc`'s bounds are the ones a fresh document on its text
/// builds.
fn assert_bounds_match_fresh_parse(doc: &mut MdDoc) {
    let mut fresh = MdDoc::new(doc.text());
    let ast = fresh.take_ast();
    fresh.put_ast(ast);
    assert_eq!(doc.bounds.source_lines, fresh.bounds.source_lines, "lines");
    assert_eq!(doc.bounds.words, fresh.bounds.words, "words");
    assert_eq!(
        doc.bounds.inline_paragraphs, fresh.bounds.inline_paragraphs,
        "inline paragraphs"
    );
    assert_eq!(doc.bounds.images, fresh.bounds.images, "images");
}

#[test]
fn typing_in_the_1_mb_note_never_waits_for_a_full_parse() {
    let mut doc = MdDoc::new(&large_note());
    doc.hold_background_parses(true);
    caret_at(&mut doc, 0);
    let opened = doc.parse_stats();
    assert_eq!(opened.full_parses, 1, "opening parses the note once");
    let middle = after(
        &doc,
        "Halfway through the year, the team reviewed every open plan.",
    );

    // Type with full parses held back: if typing waited for one, it would
    // never finish.
    let (done, finished) = mpsc::channel();
    let typing = std::thread::spawn(move || {
        caret_at(&mut doc, middle);
        type_text(&mut doc, " Zebras cross here.");
        doc.apply([Event::Newline { shift: false }], &NoLayout);
        type_text(&mut doc, "Quokkas");
        done.send(()).unwrap();
        doc
    });
    finished
        .recv_timeout(Duration::from_secs(300))
        .expect("typing finished while full parses were held back");
    let mut doc = typing.join().unwrap();

    let stats = doc.parse_stats();
    assert_eq!(stats.full_parses, opened.full_parses, "no full parse ran");
    assert_eq!(stats.installed, 0);
    assert!(stats.in_place >= 27, "{stats:?}");
    assert!(
        doc.text()
            .contains("open plan. Zebras cross here.\nQuokkas")
    );
    assert!(parse_holds(&mut doc, "Zebras cross here."));
    assert!(parse_holds(&mut doc, "Quokkas"));

    // The full parse catches up once it may.
    doc.hold_background_parses(false);
    assert!(doc.wait_for_background_parse(Duration::from_secs(300)));
    let ast = doc.take_ast();
    doc.put_ast(ast);
    assert_eq!(doc.parse_stats().installed, 1);
    assert_eq!(doc.parse_stats().full_parses, opened.full_parses);
}

#[test]
fn a_background_parse_of_text_that_changed_since_is_dropped() {
    let note: String = (0..400)
        .map(|n| format!("## Day {n}\n\nNotes for day {n}, with a [link](https://example.com/{n}).\n\n- one\n- two\n\n"))
        .collect();
    let mut doc = MdDoc::new(&note);
    doc.set_large_note_bytes(0);
    doc.hold_background_parses(true);
    let day = after(&doc, "Notes for day 200");
    caret_at(&mut doc, day);
    type_text(&mut doc, "!");

    // The parse of that text finishes, then the file changes outside the
    // editor before the editor takes the parse in.
    doc.hold_background_parses(false);
    assert!(doc.wait_for_background_parse(Duration::from_secs(60)));
    let outside = doc
        .text()
        .replace("Notes for day 3,", "Notes for day three,");
    doc.buffer.reload(outside.clone());
    doc.buffer.update();
    doc.bump_text_seq();

    let ast = doc.take_ast();
    doc.put_ast(ast);
    let stats = doc.parse_stats();
    assert_eq!(stats.dropped, 1, "{stats:?}");
    assert_eq!(stats.installed, 0, "{stats:?}");
    assert!(parse_holds(&mut doc, "day three"));

    assert!(doc.wait_for_background_parse(Duration::from_secs(60)));
    assert_matches_fresh_parse(&mut doc);
    assert_eq!(doc.parse_stats().installed, 1);
}

#[test]
fn the_background_parse_corrects_an_edit_that_reaches_past_its_blocks() {
    let note: String = (0..4000)
        .map(|n| format!("Paragraph {n} of the note.\n\n"))
        .collect();
    let mut doc = MdDoc::new(&note);
    doc.set_large_note_bytes(0);
    doc.hold_background_parses(true);
    let at = after(&doc, "of the note.\n\nParagraph 99 of the note.\n\n");
    caret_at(&mut doc, at);

    // An opening fence turns every paragraph after it into code.
    type_text(&mut doc, "```\n");
    assert!(
        !parses_as_fresh(&mut doc),
        "the in-place reparse stops at the blocks around the fence"
    );

    doc.hold_background_parses(false);
    assert!(doc.wait_for_background_parse(Duration::from_secs(60)));
    assert_matches_fresh_parse(&mut doc);
    assert!(doc.parse_stats().installed >= 1);
}

/// A note of blocks that commonly sit side by side.
fn note() -> impl Strategy<Value = String> {
    let block = prop_oneof![
        "[a-z ]{1,30}",
        "#{1,3} [a-z]{1,10}",
        "- [a-z]{1,8}\n  - [a-z]{1,8}\n- [a-z]{1,8}",
        "> [a-z ]{1,12}\n[a-z ]{1,12}",
        "1\\. [a-z]{1,8}\n2\\. [a-z]{1,8}",
        "\\*\\*[a-z]{1,6}\\*\\* !\\[i\\]\\(x\\) [a-z]{1,6}",
        "\\| a \\| b \\|\n\\|---\\|---\\|\n\\| [a-z]{1,4} \\| c \\|",
        "- \\[ \\] [a-z]{1,8}\n- \\[x\\] [a-z]{1,8}",
        "> \\[!note\\] [a-z]{1,8}\n> [a-z]{1,8}",
        "```\n[a-z]{1,8}\n```",
        "<!-- [a-z]{1,8} -->",
        "    [a-z]{1,8}",
        "---",
    ];
    prop::collection::vec((block, 0..3usize), 1..10).prop_map(|blocks| {
        blocks
            .into_iter()
            .map(|(block, blank)| format!("{block}\n{}", "\n".repeat(blank)))
            .collect()
    })
}

/// An edit the editor makes: typing, Return or Backspace somewhere.
fn edit() -> impl Strategy<Value = (prop::sample::Index, Event)> {
    let event = prop_oneof![
        "[a-z ]{1,3}".prop_map(|text| Event::Replace {
            region: Region::Selection,
            text,
            advance_cursor: true,
        }),
        Just(Event::Newline { shift: false }),
        Just(Event::Delete {
            region: Region::SelectionOrAdvance {
                advance: editor_core::input::Advance::By(editor_core::input::Increment::Char),
                backwards: true,
            },
        }),
    ];
    (any::<prop::sample::Index>(), event)
}

proptest! {
    #![proptest_config(ProptestConfig {
        max_global_rejects: 100_000,
        ..ProptestConfig::default()
    })]
    #[test]
    fn edits_reparsed_in_place_leave_the_parse_and_bounds_a_full_parse_builds(
        note in note(),
        edits in prop::collection::vec(edit(), 1..6),
    ) {
        let mut doc = MdDoc::new(&note);
        doc.set_large_note_bytes(0);
        doc.hold_background_parses(true);
        for (at, event) in edits {
            let at = at.index(doc.last_cursor_position().0 + 1);
            caret_at(&mut doc, at);
            doc.apply([event], &NoLayout);
        }
        // An edit that reaches past the reparsed blocks, as one that opens
        // a code fence does, leaves a different parse until the background
        // parse arrives.
        prop_assume!(parses_as_fresh(&mut doc));
        assert_bounds_match_fresh_parse(&mut doc);
    }
}
