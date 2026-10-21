//! The editor in an egui frame driven by `egui_kittest`: it renders a
//! Markdown fixture, takes key events, and keeps the text in step.

use editor_core::offset_types::Grapheme;
use editor_core::unicode_segs;
use editor_egui::{Editor, MdConfig};
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;

const FIXTURE: &str = "# Heading\n\
\n\
Some *emphasis* and **strong** text.\n\
\n\
- one\n\
- two\n\
\n\
| left | right |\n\
|---|---|\n\
| cell | value |\n\
\n\
```rust\n\
fn main() {}\n\
```\n";

type EditorHarness = Harness<'static, Option<Editor>>;

/// A harness showing an editor on `text`. The editor is created on the
/// first frame because it needs the harness's egui context.
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

/// Whether the last frame drew text for every grapheme of `range`.
fn drew_text(editor: &Editor, range: (Grapheme, Grapheme)) -> bool {
    let spans = editor.painted_spans();
    (range.0.0..range.1.0).all(|offset| {
        spans.iter().any(|span| {
            span.glyphs && span.source_range.0.0 <= offset && offset < span.source_range.1.0
        })
    })
}

fn type_text(harness: &mut EditorHarness, text: &str) {
    for ch in text.chars() {
        harness.event(Event::Text(ch.to_string()));
    }
    harness.run_steps(1);
}

fn press(harness: &mut EditorHarness, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(1);
}

#[test]
fn the_fixture_renders_every_block_without_changing_the_text() {
    let harness = harness(FIXTURE);
    let editor = editor(&harness);

    assert_eq!(editor.text(), FIXTURE);
    for needle in [
        "Heading",
        "emphasis",
        "strong",
        "one",
        "two",
        "left",
        "value",
        "fn main() {}",
    ] {
        assert!(
            drew_text(editor, find(editor, needle)),
            "{needle:?} wasn't drawn"
        );
    }
}

#[test]
fn syntax_shows_only_while_the_caret_is_inside_its_construct() {
    let mut harness = harness(FIXTURE);
    let heading_marker = (Grapheme(0), Grapheme(1));
    let strong_markers = {
        let strong = find(editor(&harness), "**strong**");
        (strong.0, strong.0 + 2)
    };

    // The caret starts in the heading, so its `#` shows and `**` doesn't.
    assert!(drew_text(editor(&harness), heading_marker));
    assert!(!drew_text(editor(&harness), strong_markers));

    // Down twice reaches the paragraph; End puts the caret after "text.".
    press(&mut harness, Modifiers::NONE, Key::ArrowDown);
    press(&mut harness, Modifiers::NONE, Key::ArrowDown);
    assert!(!drew_text(editor(&harness), heading_marker));

    // Moving into the bold text shows its markers.
    let inside_strong = find(editor(&harness), "strong").0.0 + 2;
    let caret = editor(&harness).selection().1.0;
    let (key, steps) = if caret < inside_strong {
        (Key::ArrowRight, inside_strong - caret)
    } else {
        (Key::ArrowLeft, caret - inside_strong)
    };
    for _ in 0..steps {
        press(&mut harness, Modifiers::NONE, key);
    }
    assert_eq!(editor(&harness).selection().1.0, inside_strong);
    assert!(drew_text(editor(&harness), strong_markers));
}

#[test]
fn typing_return_and_backspace_edit_the_text() {
    let mut harness = harness("- one");

    // Cmd+End (Ctrl+End elsewhere) moves to the end of the note.
    press(&mut harness, Modifiers::COMMAND, Key::End);
    type_text(&mut harness, "!");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "two");
    press(&mut harness, Modifiers::NONE, Key::Backspace);

    assert_eq!(editor(&harness).text(), "- one!\n- tw");
}

#[test]
fn selecting_copying_and_pasting_duplicates_text() {
    let mut harness = harness("alpha beta\n");

    // Shift+End selects to the end of the line.
    press(&mut harness, Modifiers::SHIFT, Key::End);
    harness.event(Event::Copy);
    harness.run_steps(1);
    let copied =
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            });
    assert_eq!(copied.as_deref(), Some("alpha beta"));

    press(&mut harness, Modifiers::NONE, Key::End);
    harness.event(Event::Paste(" alpha beta".into()));
    harness.run_steps(1);

    assert_eq!(editor(&harness).text(), "alpha beta alpha beta\n");
}

#[test]
fn an_input_method_commit_inserts_its_text() {
    let mut harness = harness("note\n");
    press(&mut harness, Modifiers::NONE, Key::End);

    for event in [
        egui::ImeEvent::Preedit {
            text: "に".into(),
            active_range_chars: None,
        },
        egui::ImeEvent::Preedit {
            text: "にほん".into(),
            active_range_chars: None,
        },
        egui::ImeEvent::Commit("日本".into()),
    ] {
        harness.event(Event::Ime(event));
    }
    harness.run_steps(1);

    assert_eq!(editor(&harness).text(), "note日本\n");
    assert_eq!(editor(&harness).selection(), (Grapheme(6), Grapheme(6)));
}

#[test]
fn undo_takes_back_typing() {
    let mut harness = harness("draft\n");

    press(&mut harness, Modifiers::NONE, Key::End);
    type_text(&mut harness, " one");
    press(&mut harness, Modifiers::COMMAND, Key::Z);

    assert_eq!(editor(&harness).text(), "draft\n");
}

/// Clicks the primary button at `pos`.
fn click_at(harness: &mut EditorHarness, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.run_steps(1);
    harness.drag_at(pos);
    harness.run_steps(1);
    harness.drop_at(pos);
    harness.run_steps(2);
}

#[test]
fn clicking_a_task_checkbox_checks_the_task() {
    // The caret sits in the first line, so the task's marker stays hidden
    // and its checkbox draws.
    let mut harness = harness("intro\n\n- [ ] write the plan\n");
    let text = find(editor(&harness), "write");
    let span = editor(&harness)
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0 <= text.0 && text.0 < span.source_range.1)
        .expect("the task's text was drawn");

    // The checkbox sits in the gutter left of the item's text.
    click_at(
        &mut harness,
        egui::pos2(
            span.rect.left() - span.rect.height() * 0.6,
            span.rect.center().y,
        ),
    );

    assert_eq!(editor(&harness).text(), "intro\n\n- [x] write the plan\n");
}
