//! Seam 2: every change the editor makes to a note's text comes out of
//! `editor-core` as a change event: the bytes it replaced and the text it
//! inserted. Ticket 54 sends these into the collaboration sandbox, so
//! replaying them on the text the note opened with must give the text the
//! editor holds, after every edit.

use editor_core::MdDoc;
use editor_core::buffer::TextChange;
use editor_core::input::{Advance, Bound, Event, Increment, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use proptest::prelude::*;

/// Applies `changes` to `text`, in order.
fn replay(text: &mut String, changes: &[TextChange]) {
    for change in changes {
        text.replace_range(change.range.clone(), &change.text);
    }
}

#[derive(Debug, Clone)]
enum Step {
    /// Put the caret at this fraction of the text.
    Caret(f64),
    /// Select from one fraction of the text to another.
    Select(f64, f64),
    Type(String),
    Backspace,
    DeleteWord,
    Newline,
    Paste(String),
    Undo,
    Redo,
    Bold,
    Indent,
}

/// Text that exercises graphemes, line breaks and Markdown syntax.
fn snippet() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(
            &[
                "a", "é", "日本", "👍🏽", " ", "\n", "- ", "**", "# ", "[x](y)", "> ", "`", "\t",
                "e\u{301}",
            ][..],
        ),
        1..6,
    )
    .prop_map(|pieces| pieces.concat())
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => (0.0..=1.0f64).prop_map(Step::Caret),
        1 => ((0.0..=1.0f64), (0.0..=1.0f64)).prop_map(|(a, b)| Step::Select(a, b)),
        4 => snippet().prop_map(Step::Type),
        2 => Just(Step::Backspace),
        1 => Just(Step::DeleteWord),
        1 => Just(Step::Newline),
        1 => snippet().prop_map(Step::Paste),
        2 => Just(Step::Undo),
        1 => Just(Step::Redo),
        1 => Just(Step::Bold),
        1 => Just(Step::Indent),
    ]
}

fn at(doc: &MdDoc, fraction: f64) -> Grapheme {
    let last = doc.buffer.current.segs.last_cursor_position().0;
    Grapheme((last as f64 * fraction).round() as usize)
}

fn events(doc: &MdDoc, step: &Step) -> Vec<Event> {
    match step {
        Step::Caret(fraction) => vec![Event::Select {
            region: Region::Location(Location::Grapheme(at(doc, *fraction))),
        }],
        Step::Select(start, end) => vec![Event::Select {
            region: (at(doc, *start), at(doc, *end)).into(),
        }],
        Step::Type(text) => text
            .chars()
            .map(|character| Event::Replace {
                region: Region::Selection,
                text: character.to_string(),
                advance_cursor: true,
            })
            .collect(),
        Step::Backspace => vec![Event::Delete {
            region: Region::SelectionOrAdvance {
                advance: Advance::By(Increment::Char),
                backwards: true,
            },
        }],
        Step::DeleteWord => vec![Event::Delete {
            region: Region::SelectionOrAdvance {
                advance: Advance::Next(Bound::Word),
                backwards: false,
            },
        }],
        Step::Newline => vec![Event::Newline { shift: false }],
        Step::Paste(text) => vec![Event::Paste { text: text.clone() }],
        Step::Undo => vec![Event::Undo],
        Step::Redo => vec![Event::Redo],
        Step::Bold => vec![Event::ToggleStyle {
            region: Region::Selection,
            style: editor_core::comrak::nodes::NodeValue::Strong,
        }],
        Step::Indent => vec![Event::Indent { deindent: false }],
    }
}

fn config() -> ProptestConfig {
    if std::env::var_os("PROPTEST_CASES").is_some() {
        ProptestConfig::default()
    } else {
        ProptestConfig::with_cases(512)
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn replaying_the_change_events_gives_the_editors_text(
        opening in prop::collection::vec(snippet(), 0..6).prop_map(|pieces| pieces.concat()),
        steps in prop::collection::vec(step(), 1..24),
    ) {
        let mut doc = MdDoc::new(&opening);
        let mut replayed = opening.clone();
        for step in &steps {
            let events = events(&doc, step);
            let before = doc.text().to_owned();
            // Each step applies as one frame of input, the way the editor
            // applies a frame's events.
            doc.apply(events, &NoLayout);
            let changes = doc.take_changes();
            replay(&mut replayed, &changes);
            prop_assert_eq!(&replayed, doc.text(), "after {:?}", step);
            if doc.text() != before {
                prop_assert!(!changes.is_empty(), "{:?} changed the text without an event", step);
            }
            prop_assert!(changes.iter().all(|change| change.local));
        }
    }
}

#[test]
fn one_typed_character_is_one_change_event() {
    let mut doc = MdDoc::new("ab");
    doc.apply(
        [Event::Select {
            region: Region::Location(Location::Grapheme(Grapheme(1))),
        }],
        &NoLayout,
    );
    assert!(
        doc.take_changes().is_empty(),
        "moving the caret changes nothing"
    );
    doc.apply(
        [Event::Replace {
            region: Region::Selection,
            text: "é".into(),
            advance_cursor: true,
        }],
        &NoLayout,
    );
    assert_eq!(
        doc.take_changes(),
        vec![TextChange {
            range: 1..1,
            text: "é".into(),
            local: true,
        }]
    );
    doc.apply([Event::Undo], &NoLayout);
    assert_eq!(
        doc.take_changes(),
        vec![TextChange {
            range: 1..3,
            text: String::new(),
            local: true,
        }]
    );
}

#[test]
fn a_merged_outside_edit_comes_out_as_a_change_that_isnt_local() {
    let mut doc = MdDoc::new("one\ntwo\n");
    doc.buffer.reload("one\ntwo\nthree\n".into());
    doc.buffer.update();
    assert_eq!(
        doc.take_changes(),
        vec![TextChange {
            range: 8..8,
            text: "three\n".into(),
            local: false,
        }]
    );
}

/// An event is one replacement in the rope. A command that makes several,
/// such as bold over a selection, sends one event for each, in the order
/// it made them, and replaying them gives the text.
#[test]
fn bold_over_a_selection_comes_out_as_one_event_per_replacement() {
    let mut doc = MdDoc::new("one two\n");
    doc.apply(
        [Event::Select {
            region: (Grapheme(0), Grapheme(3)).into(),
        }],
        &NoLayout,
    );
    doc.apply(
        [Event::ToggleStyle {
            region: Region::Selection,
            style: editor_core::comrak::nodes::NodeValue::Strong,
        }],
        &NoLayout,
    );

    let changes = doc.take_changes();
    assert_eq!(doc.text(), "**one** two\n");
    assert_eq!(changes.len(), 2, "{changes:?}");
    let mut replayed = "one two\n".to_owned();
    replay(&mut replayed, &changes);
    assert_eq!(replayed, doc.text());
}
