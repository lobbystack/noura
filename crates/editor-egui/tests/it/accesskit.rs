//! The note in the accessibility tree: one multi-line text node whose value
//! is the note's text, with the caret and selection a screen reader reads
//! following the keyboard.

use editor_core::offset_types::Grapheme;
use editor_egui::input::{Event, Location, Region};
use editor_egui::{Editor, MdConfig};
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

const NOTE: &str = "# Heading\n\nSome **bold** text and a [link](https://example.com).\n\n\
- one\n- two\n\n```\ncode\n```\n";

type EditorHarness = Harness<'static, Option<Editor>>;

fn harness_on(note: &'static str) -> EditorHarness {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), note, MdConfig::default()))
                    .show(ui);
            },
            None,
        );
    harness.run_steps(2);
    harness
}

fn place_caret(harness: &mut EditorHarness, offset: usize) {
    harness
        .state_mut()
        .as_mut()
        .expect("the first frame created the editor")
        .push_event(Event::Select {
            region: Region::Location(Location::Grapheme(Grapheme(offset))),
        });
    harness.run_steps(1);
}

fn press(harness: &mut EditorHarness, modifiers: Modifiers, key: Key, times: usize) {
    for _ in 0..times {
        harness.key_press_modifiers(modifiers, key);
        harness.run_steps(1);
    }
}

/// The note's text node's selection as `(anchor, focus)` character
/// offsets into its value.
fn selection(harness: &EditorHarness) -> (usize, usize) {
    let node = harness.get_by_role(Role::MultilineTextInput);
    let node = node.accesskit_node();
    let anchor = node
        .text_selection_anchor()
        .expect("the note reports its selection");
    let focus = node
        .text_selection_focus()
        .expect("the note reports its caret");
    (anchor.to_global_usv_index(), focus.to_global_usv_index())
}

#[test]
fn the_note_is_one_multi_line_text_node_whose_value_is_its_text() {
    let harness = harness_on(NOTE);

    let node = harness.get_by_role(Role::MultilineTextInput);

    assert_eq!(node.value().as_deref(), Some(NOTE));
    assert_eq!(
        node.accesskit_node().document_range().text(),
        NOTE,
        "its text runs hold the note's text, syntax included"
    );
}

#[test]
fn the_caret_and_the_selection_follow_keyboard_moves() {
    let mut harness = harness_on(NOTE);
    let bold = NOTE.find("**bold").unwrap();
    place_caret(&mut harness, bold);
    assert_eq!(selection(&harness), (bold, bold));

    press(&mut harness, Modifiers::NONE, Key::ArrowRight, 3);
    assert_eq!(selection(&harness), (bold + 3, bold + 3));

    press(&mut harness, Modifiers::SHIFT, Key::ArrowRight, 2);
    assert_eq!(selection(&harness), (bold + 3, bold + 5));

    // Down a line, into the list.
    let one = NOTE.find("- one").unwrap();
    place_caret(&mut harness, one + 5);
    press(&mut harness, Modifiers::NONE, Key::ArrowDown, 1);
    let two_end = NOTE.find("- two").unwrap() + 5;
    assert_eq!(selection(&harness), (two_end, two_end));
}

#[test]
fn lines_off_screen_still_read_and_hold_the_caret() {
    let note: &'static str = Box::leak(
        (1..=300)
            .map(|line| format!("Line {line} of a long note.\r\n"))
            .collect::<String>()
            .into_boxed_str(),
    );
    let mut harness = harness_on(note);

    let node = harness.get_by_role(Role::MultilineTextInput);
    assert_eq!(node.accesskit_node().document_range().text(), note);

    // Each CRLF is one character, as AccessKit counts line breaks.
    let line_250 = note.find("Line 250 ").unwrap();
    let crlf_before = note[..line_250].matches("\r\n").count();
    let graphemes_before = line_250 - crlf_before;
    place_caret(&mut harness, graphemes_before);
    assert_eq!(selection(&harness), (line_250, line_250));
}
