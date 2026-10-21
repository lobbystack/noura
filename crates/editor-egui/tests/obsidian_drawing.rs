//! The editor draws Obsidian-style syntax: callouts labeled with their type,
//! comments hidden and `<u>` tags hidden away from the caret, and tags and
//! block IDs as written.

mod support;

use editor_core::offset_types::Grapheme;
use support::{caret_at, drawn_in_place, drew_text, editor, find, harness, hid_text};

const CALLOUT: &str = "intro\n\n> [!warning]- Careful now\n> body text\n\nafter\n";

#[test]
fn a_callout_draws_its_label_in_place_of_its_markers() {
    let harness = harness(CALLOUT);
    let editor = editor(&harness);

    assert_eq!(drawn_in_place(editor), vec!["Warning".to_string()]);
    assert!(hid_text(editor, find(editor, "[!warning]-")));
    assert!(hid_text(editor, find(editor, "> [!")));
    let quote = find(editor, "> body").0;
    assert!(hid_text(editor, (quote, quote + 2)));
    assert!(drew_text(editor, find(editor, "Careful now")));
    assert!(drew_text(editor, find(editor, "body text")));
}

#[test]
fn a_callout_draws_its_source_while_the_caret_is_inside() {
    let mut harness = harness(CALLOUT);
    let body = find(editor(&harness), "body");
    caret_at(&mut harness, Grapheme(body.0.0 + 2));
    let editor = editor(&harness);

    assert!(drawn_in_place(editor).is_empty());
    assert!(drew_text(editor, find(editor, "[!warning]- Careful now")));
    assert!(drew_text(editor, find(editor, "> body")));
}

#[test]
fn comments_and_underline_tags_hide_away_from_the_caret() {
    let mut harness = harness("intro\n\nbefore %%secret **bold**%% and <u>under</u> after\n");
    let comment = find(editor(&harness), "%%secret **bold**%%");
    {
        let editor = editor(&harness);
        assert!(hid_text(editor, comment));
        assert!(hid_text(editor, find(editor, "<u>")));
        assert!(hid_text(editor, find(editor, "</u>")));
        for shown in ["before ", " and ", "under", " after"] {
            assert!(drew_text(editor, find(editor, shown)), "{shown:?}");
        }
    }

    caret_at(&mut harness, Grapheme(comment.0.0 + 4));
    let editor = editor(&harness);
    assert!(drew_text(editor, find(editor, "%%secret ")));
    assert!(drew_text(editor, find(editor, "bold")));
}

#[test]
fn tags_and_block_ids_draw_as_written() {
    let harness = harness("intro\n\nA #tag and #nested/tag here ^block-1\n");
    let editor = editor(&harness);

    for shown in ["#tag", "#nested/tag", "^block-1"] {
        assert!(drew_text(editor, find(editor, shown)), "{shown:?}");
    }
}
