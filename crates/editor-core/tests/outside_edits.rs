//! Seam 2: another app's edit of the open note reaches the buffer as a
//! change that isn't local. The caret stays on the text it was on, and
//! undo and redo step over your own edits only.

use std::time::{Duration, Instant};

use editor_core::buffer::{Buffer, TextChange};
use editor_core::offset_types::Grapheme;
use editor_core::operation_types::{Operation, Replace};
use proptest::prelude::*;
use unicode_segmentation::UnicodeSegmentation as _;

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

/// Put the caret at `offset`, as a click does.
fn click(buffer: &mut Buffer, offset: usize) {
    buffer.queue(vec![Operation::Select((g(offset), g(offset)))]);
    buffer.update();
}

/// Type `text` at the caret in one frame at `at`, as a paste does.
fn type_at(buffer: &mut Buffer, text: &str, at: Instant) {
    let caret = buffer.current.selection;
    buffer.queue_at(
        vec![
            Operation::Replace(Replace {
                range: caret,
                text: text.into(),
            }),
            Operation::Select((caret.0, caret.0)),
        ],
        at,
    );
    buffer.update();
}

/// The file now holds `text`, written by another app: the editor merges
/// it in, as the note screen does with a note that has nothing unsaved.
fn outside_edit(buffer: &mut Buffer, text: &str) {
    let saved = buffer.current.rope().to_string();
    buffer.saved(buffer.current.seq, saved);
    buffer.reload(text.into());
    buffer.update();
}

#[test]
fn an_outside_edit_before_the_caret_moves_it_with_its_text() {
    let mut buffer = Buffer::from("one\ntwo\nthree\n");
    // The caret sits after "tw".
    click(&mut buffer, 6);

    outside_edit(&mut buffer, "zero\none\ntwo\nthree\n");

    assert_eq!(buffer.current.text(), "zero\none\ntwo\nthree\n");
    assert_eq!(buffer.current.selection, (g(11), g(11)));
}

#[test]
fn an_outside_edit_after_the_caret_leaves_it_where_it_is() {
    let mut buffer = Buffer::from("one\ntwo\nthree\n");
    buffer.queue(vec![Operation::Select((g(4), g(7)))]);
    buffer.update();

    outside_edit(&mut buffer, "one\ntwo\nthree, outside\n");

    assert_eq!(buffer.current.text(), "one\ntwo\nthree, outside\n");
    assert_eq!(
        buffer.current.selection,
        (g(4), g(7)),
        "\"two\" stays selected"
    );
}

/// The file now holds `text`, written by another app while the note had
/// unsaved typing: the editor merges it in, as after a save that merged.
fn merge_from_file(buffer: &mut Buffer, text: &str) {
    buffer.reload(text.into());
    buffer.update();
}

/// Far enough apart that two pieces of typing undo one at a time.
const APART: Duration = Duration::from_secs(1);

#[test]
fn undo_after_a_merge_takes_back_only_your_typing_and_redo_brings_it_back() {
    let mut buffer = Buffer::from("one\ntwo\nthree\n");
    click(&mut buffer, 3);
    type_at(&mut buffer, ", mine", Instant::now());

    merge_from_file(&mut buffer, "one\ntwo\nthree, theirs\n");
    assert_eq!(buffer.current.text(), "one, mine\ntwo\nthree, theirs\n");

    buffer.undo();
    assert_eq!(buffer.current.text(), "one\ntwo\nthree, theirs\n");
    assert!(
        !buffer.can_undo(),
        "the other app's edit isn't yours to undo"
    );
    buffer.undo();
    assert_eq!(buffer.current.text(), "one\ntwo\nthree, theirs\n");

    buffer.redo();
    assert_eq!(buffer.current.text(), "one, mine\ntwo\nthree, theirs\n");
    assert!(!buffer.can_redo());
}

#[test]
fn undo_steps_back_through_your_typing_on_both_sides_of_a_merge() {
    let start = Instant::now();
    let mut buffer = Buffer::from("one\ntwo\nthree\n");
    click(&mut buffer, 3);
    type_at(&mut buffer, " A", start);
    click(&mut buffer, 9);
    type_at(&mut buffer, " B", start + APART);
    merge_from_file(&mut buffer, "zero\none\ntwo\nthree\n");
    click(&mut buffer, 22);
    type_at(&mut buffer, " C", start + APART * 2);
    assert_eq!(buffer.current.text(), "zero\none A\ntwo B\nthree C\n");

    buffer.undo();
    assert_eq!(buffer.current.text(), "zero\none A\ntwo B\nthree\n");
    buffer.undo();
    assert_eq!(buffer.current.text(), "zero\none A\ntwo\nthree\n");
    buffer.undo();
    assert_eq!(buffer.current.text(), "zero\none\ntwo\nthree\n");
    assert!(!buffer.can_undo());

    buffer.redo();
    buffer.redo();
    buffer.redo();
    assert_eq!(buffer.current.text(), "zero\none A\ntwo B\nthree C\n");
}

#[test]
fn undo_after_an_outside_edit_of_a_saved_note_keeps_the_outside_edit() {
    let mut buffer = Buffer::from("one\ntwo\n");
    click(&mut buffer, 3);
    type_at(&mut buffer, ", mine", Instant::now());
    outside_edit(&mut buffer, "one, mine\ntwo, theirs\n");

    buffer.undo();

    assert_eq!(buffer.current.text(), "one\ntwo, theirs\n");
}

/// Your typing was saved, and another app then changed part of it: undo
/// takes back your typing around it and keeps what the other app wrote.
#[test]
fn undo_keeps_an_outside_edit_made_inside_your_typing() {
    let mut buffer = Buffer::from("one\n");
    click(&mut buffer, 3);
    type_at(&mut buffer, " hello world", Instant::now());
    outside_edit(&mut buffer, "one hello there\n");

    buffer.undo();

    assert_eq!(buffer.current.text(), "onethere\n");
    buffer.redo();
    assert_eq!(buffer.current.text(), "one hello there\n");
}

/// A keystroke right after a word another app replaced stays after the
/// new word.
#[test]
fn typing_at_the_end_of_a_word_another_app_replaced_keeps_the_keystroke() {
    let mut buffer = Buffer::from("one two\n");
    click(&mut buffer, 7);
    type_at(&mut buffer, "X", Instant::now());

    merge_from_file(&mut buffer, "one three\n");

    assert_eq!(buffer.current.text(), "one threeX\n");
}

#[derive(Debug, Clone)]
enum Step {
    /// Type this text at a fraction of the text, one frame apart from the
    /// last if `apart`.
    Type(f64, String, bool),
    /// Delete up to this many clusters at a fraction of the text.
    Delete(f64, usize),
    /// The file now holds the editor's text.
    Save,
    /// Another app types this text at a fraction of the file's text.
    Outside(f64, String),
    Undo,
    Redo,
}

fn snippet() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(&["a", "é", "日本", "👍🏽", " ", "\n", "e\u{301}", "🇯🇵"][..]),
        1..4,
    )
    .prop_map(|pieces| pieces.concat())
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => ((0.0..=1.0f64), snippet(), any::<bool>()).prop_map(|(at, text, apart)| Step::Type(at, text, apart)),
        1 => ((0.0..=1.0f64), 1..4usize).prop_map(|(at, count)| Step::Delete(at, count)),
        1 => Just(Step::Save),
        2 => ((0.0..=1.0f64), snippet()).prop_map(|(at, text)| Step::Outside(at, text)),
        2 => Just(Step::Undo),
        1 => Just(Step::Redo),
    ]
}

/// The cluster offset at `fraction` of `text`.
fn cluster_at(text: &str, fraction: f64) -> usize {
    (text.graphemes(true).count() as f64 * fraction).round() as usize
}

fn replay(text: &mut String, changes: &[TextChange]) {
    for change in changes {
        text.replace_range(change.range.clone(), &change.text);
    }
}

fn config() -> ProptestConfig {
    if std::env::var_os("PROPTEST_CASES").is_some() {
        ProptestConfig::default()
    } else {
        ProptestConfig::with_cases(256)
    }
}

proptest! {
    #![proptest_config(config())]

    /// Typing, saves, another app's edits, undo and redo in any order:
    /// the change events still replay to the buffer's text, and undoing
    /// everything then redoing it all comes back to the same text.
    #[test]
    fn undo_and_redo_around_outside_edits_stay_consistent(
        opening in prop::collection::vec(snippet(), 0..5).prop_map(|pieces| pieces.concat()),
        steps in prop::collection::vec(step(), 1..24),
    ) {
        let mut buffer = Buffer::from(opening.as_str());
        let mut file = opening.clone();
        let mut replayed = opening.clone();
        let mut at = Instant::now();
        for step in &steps {
            match step {
                Step::Type(fraction, text, apart) => {
                    let caret = g(cluster_at(buffer.current.text(), *fraction));
                    at += if *apart { APART } else { Duration::from_millis(10) };
                    buffer.queue_at(
                        vec![
                            Operation::Replace(Replace { range: (caret, caret), text: text.clone() }),
                            Operation::Select((caret, caret)),
                        ],
                        at,
                    );
                    buffer.update();
                }
                Step::Delete(fraction, count) => {
                    let last = buffer.current.segs.last_cursor_position().0;
                    let start = cluster_at(buffer.current.text(), *fraction).min(last);
                    let end = (start + count).min(last);
                    at += APART;
                    buffer.queue_at(
                        vec![Operation::Replace(Replace { range: (g(start), g(end)), text: String::new() })],
                        at,
                    );
                    buffer.update();
                }
                Step::Save => {
                    file = buffer.current.rope().to_string();
                    buffer.saved(buffer.current.seq, file.clone());
                }
                Step::Outside(fraction, text) => {
                    let mut clusters: Vec<&str> = file.graphemes(true).collect();
                    let index = cluster_at(&file, *fraction).min(clusters.len());
                    clusters.insert(index, text);
                    file = clusters.concat();
                    buffer.reload(file.clone());
                    buffer.update();
                }
                Step::Undo => {
                    buffer.undo();
                }
                Step::Redo => {
                    buffer.redo();
                }
            }
            replay(&mut replayed, &buffer.take_changes());
            prop_assert_eq!(&replayed, buffer.current.text(), "after {:?}", step);
        }

        for _ in 0..steps.len() {
            buffer.redo();
        }
        let top = buffer.current.text().to_owned();
        for _ in 0..steps.len() {
            buffer.undo();
        }
        prop_assert!(!buffer.can_undo());
        for _ in 0..steps.len() {
            buffer.redo();
        }
        prop_assert_eq!(buffer.current.text(), top.as_str());
        replay(&mut replayed, &buffer.take_changes());
        prop_assert_eq!(&replayed, buffer.current.text());
    }
}
