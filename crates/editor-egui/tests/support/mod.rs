//! Shared helpers for the tests that draw a note in an `egui_kittest`
//! harness and read back what the editor painted.

#![allow(dead_code)]

use editor_core::offset_types::Grapheme;
use editor_core::unicode_segs;
use editor_egui::{Editor, MdConfig};
use egui_kittest::Harness;

pub type EditorHarness = Harness<'static, Option<Editor>>;

/// A harness showing an editor on `text`, with the caret at its start.
pub fn harness(text: &'static str) -> EditorHarness {
    harness_with(text, |_| {})
}

/// A harness showing an editor on `text` that `setup` configures when it's
/// created, with the caret at its start.
pub fn harness_with(text: &'static str, setup: impl Fn(&mut Editor) + 'static) -> EditorHarness {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| {
                        let mut editor = Editor::new(ui.ctx(), text, MdConfig::default());
                        setup(&mut editor);
                        editor
                    })
                    .show(ui);
            },
            None,
        );
    harness.run_steps(2);
    harness
}

pub fn editor(harness: &EditorHarness) -> &Editor {
    harness
        .state()
        .as_ref()
        .expect("the first frame created the editor")
}

pub fn editor_mut(harness: &mut EditorHarness) -> &mut Editor {
    harness
        .state_mut()
        .as_mut()
        .expect("the first frame created the editor")
}

/// The grapheme range of the first `needle` in the editor's text.
pub fn find(editor: &Editor, needle: &str) -> (Grapheme, Grapheme) {
    find_after(editor, needle, 0)
}

/// The grapheme range of the first `needle` at or after byte `from`.
pub fn find_after(editor: &Editor, needle: &str, from: usize) -> (Grapheme, Grapheme) {
    let text = editor.text();
    let start = from
        + text[from..]
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
pub fn drew_text(editor: &Editor, range: (Grapheme, Grapheme)) -> bool {
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

/// Whether the last frame drew none of `range` as its own text.
pub fn hid_text(editor: &Editor, range: (Grapheme, Grapheme)) -> bool {
    let spans = editor.painted_spans();
    (range.0.0..range.1.0).all(|offset| {
        !spans.iter().any(|span| {
            span.glyphs
                && span.shown_as_written
                && span.source_range.0.0 <= offset
                && offset < span.source_range.1.0
                && !span.text.trim().is_empty()
        })
    })
}

/// The text the last frame drew in place of source, such as a label, one
/// entry per source range, its words joined by spaces.
pub fn drawn_in_place(editor: &Editor) -> Vec<String> {
    let mut drawn: Vec<((Grapheme, Grapheme), String)> = Vec::new();
    for span in editor.painted_spans() {
        let text = span.text.trim();
        if !span.glyphs || span.shown_as_written || text.is_empty() {
            continue;
        }
        match drawn.last_mut() {
            Some((range, words)) if *range == span.source_range => {
                words.push(' ');
                words.push_str(text);
            }
            _ => drawn.push((span.source_range, text.to_string())),
        }
    }
    drawn.into_iter().map(|(_, words)| words).collect()
}

/// Where the last frame drew the first grapheme of `range`.
pub fn drawn_at(editor: &Editor, range: (Grapheme, Grapheme)) -> egui::Rect {
    editor
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0 <= range.0 && range.0 < span.source_range.1)
        .expect("the text was drawn")
        .rect
}

/// Moves the caret to `offset` and draws a frame.
pub fn caret_at(harness: &mut EditorHarness, offset: Grapheme) {
    editor_mut(harness).push_event(editor_egui::input::Event::Select {
        region: editor_egui::input::Region::Location(editor_egui::input::Location::Grapheme(
            offset,
        )),
    });
    harness.run_steps(1);
}
