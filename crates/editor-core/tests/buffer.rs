//! The text buffer: edits, undo and redo, and merging outside edits.

use std::time::{Duration, Instant};

use editor_core::buffer::Buffer;
use editor_core::offset_types::Grapheme;
use editor_core::operation_types::{Operation, Replace};

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

fn replace(start: usize, end: usize, text: &str) -> Operation {
    Operation::Replace(Replace {
        range: (g(start), g(end)),
        text: text.into(),
    })
}

fn select(start: usize, end: usize) -> Operation {
    Operation::Select((g(start), g(end)))
}

/// Types `text` one grapheme at a time at the caret, each keystroke its own
/// frame, `gap` apart. Like the editor, each frame places the caret at its
/// old offset and lets the insertion carry it forward.
fn type_text(buffer: &mut Buffer, text: &str, start: Instant, gap: Duration) -> Instant {
    let mut at = start;
    for ch in text.chars() {
        let caret = buffer.current.selection;
        buffer.queue_at(
            vec![
                Operation::Replace(Replace {
                    range: caret,
                    text: ch.to_string(),
                }),
                Operation::Select((caret.0, caret.0)),
            ],
            at,
        );
        buffer.update();
        at += gap;
    }
    at
}

#[test]
fn a_new_buffer_holds_its_text_with_the_caret_at_the_start() {
    let buffer = Buffer::from("# Notes\n");
    assert_eq!(buffer.current.text(), "# Notes\n");
    assert_eq!(buffer.current.rope().to_string(), "# Notes\n");
    assert_eq!(buffer.current.selection, (g(0), g(0)));
}

#[test]
fn replacing_text_updates_text_selection_and_rope() {
    let mut buffer = Buffer::from("hello world");
    buffer.queue(vec![replace(0, 5, "goodbye"), select(5, 5)]);
    let response = buffer.update();

    assert!(response.text_updated);
    assert_eq!(buffer.current.text(), "goodbye world");
    assert_eq!(buffer.current.rope().to_string(), "goodbye world");
    assert_eq!(buffer.current.selection, (g(7), g(7)));
}

#[test]
fn operations_queued_together_apply_against_the_text_they_were_made_from() {
    // Both inserts were made against "abc": "X" before "a" and "Y" after "c".
    let mut buffer = Buffer::from("abc");
    buffer.queue(vec![replace(0, 0, "X"), replace(3, 3, "Y")]);
    buffer.update();
    assert_eq!(buffer.current.text(), "XabcY");
}

#[test]
fn a_selection_after_an_edit_in_the_same_frame_follows_the_edit() {
    // A newline typed at the caret, then the caret placed at the old offset,
    // ends up after the newline.
    let mut buffer = Buffer::from("ab");
    buffer.queue(vec![select(1, 1)]);
    buffer.update();

    buffer.queue(vec![replace(1, 1, "\n"), select(1, 1)]);
    buffer.update();

    assert_eq!(buffer.current.text(), "a\nb");
    assert_eq!(buffer.current.selection, (g(2), g(2)));
}

#[test]
fn undo_and_redo_restore_text_and_selection() {
    let mut buffer = Buffer::from("one");
    buffer.queue(vec![replace(3, 3, " two"), select(3, 3)]);
    buffer.update();

    buffer.undo();
    assert_eq!(buffer.current.text(), "one");
    assert_eq!(buffer.current.selection, (g(0), g(0)));
    assert!(!buffer.can_undo());

    buffer.redo();
    assert_eq!(buffer.current.text(), "one two");
    assert_eq!(buffer.current.selection, (g(7), g(7)));
    assert!(!buffer.can_redo());
}

#[test]
fn quick_typing_undoes_as_one_unit_and_a_pause_starts_another() {
    let mut buffer = Buffer::from("");
    let start = Instant::now();
    let after_first_word = type_text(&mut buffer, "hi", start, Duration::from_millis(50));
    type_text(
        &mut buffer,
        " you",
        after_first_word + Duration::from_secs(2),
        Duration::from_millis(50),
    );
    assert_eq!(buffer.current.text(), "hi you");

    buffer.undo();
    assert_eq!(buffer.current.text(), "hi");
    buffer.undo();
    assert_eq!(buffer.current.text(), "");
}

#[test]
fn editing_after_undo_drops_the_redo_history() {
    let mut buffer = Buffer::from("a");
    buffer.queue(vec![replace(1, 1, "b"), select(1, 1)]);
    buffer.update();
    buffer.undo();

    buffer.queue(vec![replace(1, 1, "c"), select(1, 1)]);
    buffer.update();

    assert!(!buffer.can_redo());
    assert_eq!(buffer.current.text(), "ac");
}

#[test]
fn reload_merges_an_outside_edit_with_unsaved_typing() {
    let mut buffer = Buffer::from("hello world");
    buffer.queue(vec![replace(11, 11, "!"), select(11, 11)]);
    buffer.update();

    buffer.reload("goodbye world".into());
    buffer.update();

    assert_eq!(buffer.current.text(), "goodbye world!");
    assert_eq!(buffer.current.selection, (g(14), g(14)));
}

#[test]
fn reload_after_save_diffs_against_the_saved_text() {
    let mut buffer = Buffer::from("draft");
    buffer.queue(vec![replace(5, 5, " one"), select(5, 5)]);
    buffer.update();
    let seq = buffer.current.seq;
    buffer.saved(seq, "draft one".into());

    buffer.reload("draft one two".into());
    buffer.update();

    assert_eq!(buffer.current.text(), "draft one two");
}

#[test]
fn transform_range_follows_edits_before_the_range_and_fails_on_edits_inside_it() {
    let mut buffer = Buffer::from("abc def");
    let seq = buffer.current.seq;
    buffer.queue(vec![replace(0, 0, "xx")]);
    buffer.update();

    let mut def = (g(4), g(7));
    assert!(buffer.transform_range(seq, &mut def));
    assert_eq!(&buffer[def], "def");

    let seq = buffer.current.seq;
    buffer.queue(vec![replace(7, 8, "E")]);
    buffer.update();
    assert!(!buffer.transform_range(seq, &mut def));
}

#[test]
fn offsets_count_grapheme_clusters() {
    let mut buffer = Buffer::from("e\u{301}x");
    buffer.queue(vec![replace(1, 2, "y")]);
    buffer.update();
    assert_eq!(buffer.current.text(), "e\u{301}y");
    assert_eq!(&buffer[(g(0), g(1))], "e\u{301}");
}
