//! The editor draws notes the way today's editor does: each line of a
//! paragraph keeps its row, and syntax the editor doesn't know stays on
//! screen as the text you wrote.

mod support;

use support::{drawn_at, drew_text, editor, find, harness};

/// Today's editor shows each line of a paragraph on its own row, and Return
/// in a paragraph writes one line break, so the editor keeps that row.
#[test]
fn a_soft_line_break_keeps_its_row_as_in_todays_editor() {
    let harness = harness("intro\n\nfirst line\nsecond line\n");
    let editor = editor(&harness);

    let first = drawn_at(editor, find(editor, "first"));
    let second = drawn_at(editor, find(editor, "second"));

    assert!(
        second.top() >= first.bottom(),
        "the second line starts a row"
    );
}

#[test]
fn syntax_the_editor_does_not_know_stays_visible() {
    let note = "intro\n\n\
        A ||spoiler|| and :smile: and x^2^ and H~2~O.\n\n\
        A footnote[^1] and {{template}} and <span class=\"x\">html</span>.\n\n\
        [^1]: The footnote.\n\n\
        Term\n: Definition\n\n\
        [ref]: https://example.com\n\n\
        # Folded <!-- {\"fold\":true} -->\n\n\
        hidden\n";
    let harness = harness(note);
    let editor = editor(&harness);

    for needle in [
        "||spoiler||",
        ":smile:",
        "x^2^",
        "H~2~O",
        "[^1]",
        "{{template}}",
        "<span class=\"x\">html</span>",
        "[^1]: The footnote.",
        ": Definition",
        "[ref]: https://example.com",
        "<!-- {\"fold\":true} -->",
        "hidden",
    ] {
        assert!(
            drew_text(editor, find(editor, needle)),
            "{needle:?} wasn't drawn as written"
        );
    }
}
