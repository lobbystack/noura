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

#[test]
fn command_u_toggles_u_tags_and_keys_for_unread_styles_write_nothing() {
    let mut harness = harness("word\n");
    press(&mut harness, Modifiers::SHIFT, Key::End);

    press(&mut harness, Modifiers::COMMAND, Key::U);
    assert_eq!(editor(&harness).text(), "<u>word</u>\n");
    press(&mut harness, Modifiers::COMMAND, Key::U);
    assert_eq!(editor(&harness).text(), "word\n");

    // Lockbook wrapped the selection in spoiler, subscript and superscript
    // markers, which the editor no longer reads.
    for key in [Key::P, Key::S, Key::E] {
        press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, key);
    }
    assert_eq!(editor(&harness).text(), "word\n");
}

/// Where the last frame drew the first grapheme of the first `needle`,
/// if it drew it.
fn drawn_at(editor: &Editor, needle: &str) -> Option<egui::Rect> {
    let start = find(editor, needle).0;
    editor
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0 <= start && start < span.source_range.1)
        .map(|span| span.rect)
}

/// Scroll the editor with the mouse wheel, `points` towards the start,
/// and let the scroll settle.
fn wheel_up(harness: &mut EditorHarness, points: f32) {
    harness.hover_at(egui::pos2(400.0, 300.0));
    harness.run_steps(1);
    for _ in 0..10 {
        harness.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, points / 10.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_steps(60);
}

#[test]
fn an_outside_edit_keeps_the_caret_and_the_scroll_position() {
    let note: String = (1..=120)
        .map(|n| format!("Paragraph {n} of the note.\n\n"))
        .collect();
    let mut harness = harness(Box::leak(note.clone().into_boxed_str()));
    // Cmd+End (Ctrl+End elsewhere) puts the caret at the end of the note,
    // then the wheel scrolls it out of view.
    press(&mut harness, Modifiers::COMMAND, Key::End);
    wheel_up(&mut harness, 1500.0);
    let caret = editor(&harness).selection();
    let top = (1..=120)
        .find(|n| {
            drawn_at(editor(&harness), &format!("Paragraph {n} "))
                .is_some_and(|rect| rect.top() >= 0.0)
        })
        .expect("a paragraph shows");
    assert!(top > 10, "the view shows the middle of the note");
    assert!(
        drawn_at(editor(&harness), "Paragraph 120 ").is_none_or(|rect| rect.top() > 600.0),
        "the caret's paragraph is out of view"
    );
    let needle = format!("Paragraph {top} ");
    let shown = drawn_at(editor(&harness), &needle);

    // Another app edits a paragraph above the view and one inside it.
    let inside = top + 2;
    let outside = note
        .replace("Paragraph 3 of", "Paragraph 3, edited, of")
        .replace(
            &format!("Paragraph {inside} of"),
            &format!("Paragraph {inside}, edited, of"),
        );
    harness
        .state_mut()
        .as_mut()
        .expect("the first frame created the editor")
        .apply_disk_version(outside.clone());
    harness.run_steps(2);

    let editor = editor(&harness);
    assert_eq!(editor.text(), outside);
    let added = 2 * ", edited,".len();
    assert_eq!(
        editor.selection(),
        (caret.0 + added, caret.1 + added),
        "the caret stays at the end of the note"
    );
    assert_eq!(
        drawn_at(editor, &needle),
        shown,
        "the view stays where it was"
    );
    assert!(drew_text(
        editor,
        find(editor, &format!("Paragraph {inside}, edited,"))
    ));
}
