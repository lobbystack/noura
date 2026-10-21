//! Seam 2: input method composition through `editor-core` alone. Marked
//! text shows at the caret while you compose, but the note's text, its
//! change events and so the file hold nothing of it until the input method
//! commits; the commit writes one edit, and a cancelled composition leaves
//! nothing behind.

use editor_core::MdDoc;
use editor_core::buffer::TextChange;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;

const NOTE: &str = "# Heading\n\nSome **bold** text.\n\n- item one\n";

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

/// A note with the caret right after the first `before`. The note is ASCII,
/// so its byte offsets are its grapheme offsets.
fn note_with_caret_after(before: &str) -> (MdDoc, Grapheme) {
    let mut doc = MdDoc::new(NOTE);
    let caret = g(NOTE.find(before).expect("the note holds it") + before.len());
    doc.apply(
        [Event::Select {
            region: Region::Location(Location::Grapheme(caret)),
        }],
        &NoLayout,
    );
    (doc, caret)
}

fn compose(doc: &mut MdDoc, text: &str) {
    doc.apply(
        [Event::Compose {
            text: text.into(),
            selected: None,
        }],
        &NoLayout,
    );
}

fn commit(doc: &mut MdDoc, text: &str) {
    doc.apply([Event::Commit { text: text.into() }], &NoLayout);
}

/// Composes each of `steps` in turn, checking after each that the note
/// still holds its own text with the caret where it was, that no change
/// event went out, and that the marked text is the step.
fn compose_steps(doc: &mut MdDoc, caret: Grapheme, steps: &[&str]) {
    for step in steps {
        compose(doc, step);
        assert_eq!(doc.text(), NOTE, "{step:?} stayed out of the text");
        assert_eq!(doc.selection(), (caret, caret));
        assert_eq!(doc.take_changes(), [], "{step:?} made no change event");
        assert_eq!(
            doc.composition().map(|marked| marked.text.as_str()),
            Some(*step)
        );
    }
}

#[test]
fn japanese_marked_text_stays_out_of_the_note_until_it_commits_as_one_edit() {
    let (mut doc, caret) = note_with_caret_after("**bo");

    compose_steps(
        &mut doc,
        caret,
        &["n", "に", "にh", "にほ", "にほn", "にほん", "日本"],
    );
    commit(&mut doc, "日本");

    assert_eq!(
        doc.text(),
        "# Heading\n\nSome **bo日本ld** text.\n\n- item one\n"
    );
    assert_eq!(doc.selection(), (caret + 2, caret + 2));
    assert_eq!(doc.composition(), None);
    let at = NOTE.find("**bo").unwrap() + 4;
    assert_eq!(
        doc.take_changes(),
        [TextChange {
            range: at..at,
            text: "日本".into(),
            local: true,
        }]
    );
}

#[test]
fn chinese_pinyin_commits_its_characters_in_a_heading_as_one_edit() {
    let (mut doc, caret) = note_with_caret_after("# Head");

    compose_steps(
        &mut doc,
        caret,
        &["n", "ni", "ni h", "ni ha", "ni hao", "你好"],
    );
    commit(&mut doc, "你好");

    assert_eq!(
        doc.text(),
        "# Head你好ing\n\nSome **bold** text.\n\n- item one\n"
    );
    assert_eq!(doc.selection(), (caret + 2, caret + 2));
    assert_eq!(doc.take_changes().len(), 1);
}

#[test]
fn korean_commits_each_syllable_as_the_next_one_starts() {
    let (mut doc, caret) = note_with_caret_after("- item");

    compose_steps(&mut doc, caret, &["ㅎ", "하", "한"]);
    commit(&mut doc, "한");
    assert_eq!(doc.composition(), None);
    let first = doc.take_changes();
    compose(&mut doc, "ㄱ");
    compose(&mut doc, "그");
    compose(&mut doc, "글");
    assert_eq!(
        doc.text(),
        "# Heading\n\nSome **bold** text.\n\n- item한 one\n"
    );
    assert_eq!(
        doc.take_changes(),
        [],
        "the second syllable is still marked"
    );
    commit(&mut doc, "글");

    assert_eq!(
        doc.text(),
        "# Heading\n\nSome **bold** text.\n\n- item한글 one\n"
    );
    assert_eq!(doc.selection(), (caret + 2, caret + 2));
    assert_eq!(first.len(), 1);
    assert_eq!(doc.take_changes().len(), 1);
}

#[test]
fn a_cancelled_composition_leaves_nothing_behind() {
    for before in ["**bo", "# Head", "- item"] {
        let (mut doc, caret) = note_with_caret_after(before);

        compose_steps(&mut doc, caret, &["n", "に", "にほ"]);
        // Escape, or deleting every composed character.
        compose(&mut doc, "");

        assert_eq!(doc.text().as_bytes(), NOTE.as_bytes());
        assert_eq!(doc.selection(), (caret, caret));
        assert_eq!(doc.composition(), None);
        assert_eq!(doc.take_changes(), []);
    }
}

/// An input method that loses the keyboard mid-composition drops its marked
/// text without an empty composition or a commit; the editor ends the
/// composition itself and the note keeps its bytes.
#[test]
fn an_ended_composition_leaves_nothing_behind() {
    let (mut doc, caret) = note_with_caret_after("# Head");
    compose_steps(&mut doc, caret, &["n", "に", "にほ"]);

    doc.end_composition();

    assert_eq!(doc.text().as_bytes(), NOTE.as_bytes());
    assert_eq!(doc.selection(), (caret, caret));
    assert_eq!(doc.composition(), None);
    assert_eq!(doc.take_changes(), []);

    // Typing afterwards inserts only what you type.
    doc.apply(
        [Event::Replace {
            region: Region::Selection,
            text: "x".into(),
            advance_cursor: true,
        }],
        &NoLayout,
    );
    assert_eq!(
        doc.text(),
        "# Headxing\n\nSome **bold** text.\n\n- item one\n"
    );
    assert_eq!(doc.composition(), None);
}

#[test]
fn one_undo_takes_back_a_committed_composition() {
    let (mut doc, caret) = note_with_caret_after("**bo");
    compose_steps(&mut doc, caret, &["n", "に", "にほ", "にほん", "日本"]);
    commit(&mut doc, "日本");

    doc.apply([Event::Undo], &NoLayout);

    assert_eq!(doc.text(), NOTE);
    assert_eq!(doc.selection(), (caret, caret));
}

#[test]
fn a_read_only_note_shows_no_marked_text_and_takes_no_commit() {
    let (mut doc, _) = note_with_caret_after("**bo");
    doc.readonly = true;

    compose(&mut doc, "にほん");
    assert_eq!(doc.composition(), None);
    commit(&mut doc, "日本");

    assert_eq!(doc.text(), NOTE);
    assert_eq!(doc.take_changes(), []);
}
