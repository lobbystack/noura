//! Seam 2: another app's edit of the open note reaches the buffer as a
//! change that isn't local. The caret stays on the text it was on, and
//! undo and redo step over your own edits only.

use std::time::Instant;

use editor_core::buffer::Buffer;
use editor_core::offset_types::Grapheme;
use editor_core::operation_types::{Operation, Replace};

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
