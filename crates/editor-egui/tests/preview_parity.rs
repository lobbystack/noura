//! The editor draws notes the way today's editor does: each line of a
//! paragraph keeps its row, and syntax the editor doesn't know stays on
//! screen as the text you wrote.

use editor_core::offset_types::Grapheme;
use editor_core::unicode_segs;
use editor_egui::{Editor, MdConfig};
use egui_kittest::Harness;

type EditorHarness = Harness<'static, Option<Editor>>;

/// A harness showing an editor on `text`, with the caret at its start.
fn harness(text: &'static str) -> EditorHarness {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), text, MdConfig::default()))
                    .show(ui);
            },
            None,
        );
    harness.run_steps(2);
    harness
}

fn editor(harness: &EditorHarness) -> &Editor {
    harness
        .state()
        .as_ref()
        .expect("the first frame created the editor")
}

/// The grapheme range of the first `needle` in the editor's text.
fn find(editor: &Editor, needle: &str) -> (Grapheme, Grapheme) {
    let text = editor.text();
    let start = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} isn't in the note"));
    let segs = unicode_segs::calc(text);
    (
        segs.byte_to_char_floor(start.into()),
        segs.byte_to_char_floor((start + needle.len()).into()),
    )
}

/// Whether the last frame drew `range` as its own text, grapheme for
/// grapheme.
fn drew_text(editor: &Editor, range: (Grapheme, Grapheme)) -> bool {
    let spans = editor.painted_spans();
    (range.0.0..range.1.0).all(|offset| {
        spans.iter().any(|span| {
            span.glyphs
                && span.shown_as_written
                && span.source_range.0.0 <= offset
                && offset < span.source_range.1.0
        })
    })
}

/// Where the last frame drew the first grapheme of `range`.
fn drawn_at(editor: &Editor, range: (Grapheme, Grapheme)) -> egui::Rect {
    editor
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0 <= range.0 && range.0 < span.source_range.1)
        .expect("the text was drawn")
        .rect
}

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
