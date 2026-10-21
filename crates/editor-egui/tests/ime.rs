//! Input method composition, as egui delivers it from an input method such
//! as macOS's Japanese one: the composition never reaches the note, the
//! commit lands at the caret as typed text, and a cancelled composition
//! leaves the note's bytes as they were. Checked through `editor-core`'s
//! document, inside bold text, a heading and a list item.

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
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            |ui, (editor, changes): &mut State| {
                let response = editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), NOTE, MdConfig::default()))
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
    let caret = Grapheme(NOTE.find(before).expect("the note holds it") + before.len());
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
