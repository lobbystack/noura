//! Input method composition, as egui delivers it from an input method such
//! as macOS's Japanese one: the composition shows inline at the caret but
//! never reaches the note, the commit lands at the caret as typed text, a
//! cancelled composition leaves the note's bytes as they were, and the
//! candidate window opens at the marked text. Checked through
//! `editor-core`'s document and what the editor drew, inside bold text, a
//! heading and a list item.

use editor_core::MdDoc;
use editor_core::buffer::TextChange;
use editor_core::offset_types::Grapheme;
use editor_egui::input::{Event, Location, Region};
use editor_egui::{Editor, MdConfig};
use egui::ImeEvent;
use egui_kittest::Harness;

const NOTE: &str = "# Heading\n\nSome **bold** text.\n\n- item one\n- item two\n";

/// The editor, once the first frame created it, and the change events its
/// frames reported.
type State = (Option<Editor>, Vec<TextChange>);

type EditorHarness = Harness<'static, State>;

fn harness() -> EditorHarness {
    harness_on(NOTE)
}

fn harness_on(note: &'static str) -> EditorHarness {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            move |ui, (editor, changes): &mut State| {
                let response = editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), note, MdConfig::default()))
                    .show(ui);
                changes.extend(response.changes);
            },
            (None, Vec::new()),
        );
    harness.run_steps(2);
    harness
}

/// The note's document in `editor-core`.
fn doc(harness: &EditorHarness) -> &MdDoc {
    &harness
        .state()
        .0
        .as_ref()
        .expect("the first frame created the editor")
        .edit
        .renderer
        .doc
}

/// The change events reported since the last call.
fn changes(harness: &mut EditorHarness) -> Vec<TextChange> {
    std::mem::take(&mut harness.state_mut().1)
}

/// Puts the caret right after the first `before` in the note. The note is
/// ASCII, so its byte offsets are its grapheme offsets.
fn place_caret_after(harness: &mut EditorHarness, before: &str) -> Grapheme {
    let text = doc(harness).text();
    let caret = Grapheme(text.find(before).expect("the note holds it") + before.len());
    harness
        .state_mut()
        .0
        .as_mut()
        .unwrap()
        .push_event(Event::Select {
            region: Region::Location(Location::Grapheme(caret)),
        });
    harness.run_steps(1);
    assert_eq!(doc(harness).selection(), (caret, caret));
    caret
}

/// Sends one input method event and runs the frame that receives it.
fn send(harness: &mut EditorHarness, event: ImeEvent) {
    harness.event(egui::Event::Ime(event));
    harness.run_steps(1);
}

fn preedit(text: &str) -> ImeEvent {
    ImeEvent::Preedit {
        text: text.into(),
        active_range_chars: (!text.is_empty()).then(|| 0..text.chars().count()),
    }
}

/// Composes にほん, checking after each update that the note still holds
/// only its own text with the caret where it was.
fn compose(harness: &mut EditorHarness, caret: Grapheme) {
    #[expect(
        deprecated,
        reason = "egui no longer sends Enabled, but a platform may"
    )]
    send(harness, ImeEvent::Enabled);
    for text in ["n", "に", "にh", "にほ", "にほn", "にほん"] {
        send(harness, preedit(text));
        assert_eq!(
            doc(harness).text(),
            NOTE,
            "the composition {text:?} stayed out"
        );
        assert_eq!(doc(harness).selection(), (caret, caret));
    }
}

fn disable(harness: &mut EditorHarness) {
    #[expect(
        deprecated,
        reason = "egui no longer sends Disabled, but a platform may"
    )]
    send(harness, ImeEvent::Disabled);
}

#[test]
fn a_committed_composition_inserts_only_its_text_inside_bold_a_heading_and_a_list_item() {
    for (before, expected) in [
        (
            "**bo",
            "# Heading\n\nSome **bo日本ld** text.\n\n- item one\n- item two\n",
        ),
        (
            "# Head",
            "# Head日本ing\n\nSome **bold** text.\n\n- item one\n- item two\n",
        ),
        (
            "- item",
            "# Heading\n\nSome **bold** text.\n\n- item日本 one\n- item two\n",
        ),
    ] {
        let mut harness = harness();
        let caret = place_caret_after(&mut harness, before);

        compose(&mut harness, caret);
        assert_eq!(
            changes(&mut harness),
            [],
            "the composition after {before:?} made no change event"
        );
        send(&mut harness, ImeEvent::Commit("日本".into()));
        disable(&mut harness);

        assert_eq!(doc(&harness).text(), expected, "committed after {before:?}");
        let at = NOTE.find(before).unwrap() + before.len();
        assert_eq!(
            changes(&mut harness),
            [TextChange {
                range: at..at,
                text: "日本".into(),
                local: true,
            }],
            "the commit after {before:?} is one change event"
        );
        let after = caret + 2;
        assert_eq!(
            doc(&harness).selection(),
            (after, after),
            "the caret follows the committed text after {before:?}"
        );
    }
}

#[test]
fn a_cancelled_composition_leaves_the_bytes_unchanged() {
    for before in ["**bo", "# Head", "- item"] {
        let mut harness = harness();
        let caret = place_caret_after(&mut harness, before);

        compose(&mut harness, caret);
        // Escape or deleting every composed character: the input method
        // clears the composition and commits nothing.
        send(&mut harness, preedit(""));
        disable(&mut harness);

        assert_eq!(doc(&harness).text().as_bytes(), NOTE.as_bytes());
        assert_eq!(doc(&harness).selection(), (caret, caret));
        assert_eq!(
            changes(&mut harness),
            [],
            "a cancelled composition made no change"
        );
    }
}

/// The spans the last frame drew for marked text: glyphs standing for no
/// source at the caret.
fn marked_spans(harness: &EditorHarness, caret: Grapheme) -> Vec<egui::Rect> {
    editor(harness)
        .painted_spans()
        .into_iter()
        .filter(|span| {
            span.glyphs && span.source_range == (caret, caret) && span.rect.width() > 0.0
        })
        .map(|span| span.rect)
        .collect()
}

fn editor(harness: &EditorHarness) -> &Editor {
    harness
        .state()
        .0
        .as_ref()
        .expect("the first frame created the editor")
}

/// Where the last frame drew the grapheme at `offset`.
fn drawn_at(harness: &EditorHarness, offset: Grapheme) -> egui::Rect {
    editor(harness)
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0 <= offset && offset < span.source_range.1)
        .unwrap_or_else(|| panic!("nothing drew {offset:?}"))
        .rect
}

/// Where the focused editor told the platform its caret is.
fn ime_cursor(harness: &EditorHarness) -> egui::Rect {
    harness
        .output()
        .platform_output
        .ime
        .as_ref()
        .expect("the focused editor reports where text goes")
        .cursor_rect
}

#[test]
fn marked_text_draws_inline_at_the_caret_and_moves_the_rest_of_the_line_along() {
    let mut harness = harness();
    let caret = place_caret_after(&mut harness, "**bo");
    let caret_x = ime_cursor(&harness).left();

    send(&mut harness, preedit("ni hao"));

    let marked = marked_spans(&harness, caret);
    assert!(!marked.is_empty(), "the marked text drew");
    assert!(
        editor(&harness)
            .painted_spans()
            .iter()
            .filter(|span| span.source_range == (caret, caret) && span.glyphs)
            .all(|span| span.underlined),
        "the marked text is underlined"
    );
    let left = marked
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);
    let right = marked
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);
    assert!((left - caret_x).abs() < 0.5, "it starts at the caret");
    assert!(right > left + 10.0, "it takes room");
    let after = drawn_at(&harness, caret);
    assert!(
        (after.left() - right).abs() < 0.5,
        "the text after the caret moved past it"
    );
    assert_eq!(doc(&harness).text(), NOTE);

    send(&mut harness, preedit(""));
    assert!(
        marked_spans(&harness, caret).is_empty(),
        "cancelling clears it"
    );
    assert!((ime_cursor(&harness).left() - caret_x).abs() < 0.5);
}

#[test]
fn the_candidate_window_sits_at_the_marked_text() {
    let mut harness = harness();
    let caret = place_caret_after(&mut harness, "- item");

    send(&mut harness, preedit("ni hao"));

    let marked = marked_spans(&harness, caret);
    let cursor = ime_cursor(&harness);
    assert!((cursor.left() - marked[0].left()).abs() < 0.5);
    assert!(cursor.top() <= marked[0].center().y);
    assert!(marked[0].center().y <= cursor.bottom());
}

#[test]
fn the_candidate_window_follows_the_marked_text_when_the_note_scrolls() {
    let note: &'static str = Box::leak(
        (1..=80)
            .map(|line| format!("Line {line} of a long note.\n\n"))
            .collect::<String>()
            .into_boxed_str(),
    );
    let mut harness = harness_on(note);
    let caret = place_caret_after(&mut harness, "Line 12 of");
    send(&mut harness, preedit("ni hao"));
    let before = ime_cursor(&harness);

    harness.event(egui::Event::PointerMoved(egui::pos2(400.0, 300.0)));
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -120.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(2);

    let marked = marked_spans(&harness, caret);
    let after = ime_cursor(&harness);
    assert!(after.top() < before.top() - 60.0, "the note scrolled up");
    assert!((after.left() - marked[0].left()).abs() < 0.5);
    assert!(after.top() <= marked[0].center().y);
    assert!(marked[0].center().y <= after.bottom());
}

#[test]
fn marked_text_draws_on_a_blank_line_and_in_an_empty_note() {
    let mut harness = harness();
    let caret = place_caret_after(&mut harness, "# Heading\n");
    send(&mut harness, preedit("ni hao"));
    assert!(!marked_spans(&harness, caret).is_empty(), "on a blank line");

    let mut harness = harness_on("");
    send(&mut harness, preedit("ni hao"));
    assert!(
        !marked_spans(&harness, Grapheme(0)).is_empty(),
        "in an empty note"
    );
    assert_eq!(doc(&harness).text(), "");
}
