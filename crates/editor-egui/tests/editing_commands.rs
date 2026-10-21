//! The editor's toolbars, find bar and editing keys in an egui frame
//! driven by `egui_kittest`, read through the accessibility tree.

use editor_core::offset_types::Grapheme;
use editor_egui::input::{Event, Location, Region};
use editor_egui::{Editor, MdConfig};
use egui::accesskit::{Role, Toggled};
use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

type EditorHarness = Harness<'static, Option<Editor>>;

/// A harness showing an editor with its toolbars on `text`, on Windows so
/// shortcuts read as `Ctrl+B`.
fn harness(text: &'static str) -> EditorHarness {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 600.0))
        .with_os(OperatingSystem::Windows)
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| {
                        let config = MdConfig {
                            toolbar: true,
                            ..MdConfig::default()
                        };
                        Editor::new(ui.ctx(), text, config)
                    })
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
        .expect("the first frame created it")
}

fn editor_mut(harness: &mut EditorHarness) -> &mut Editor {
    harness
        .state_mut()
        .as_mut()
        .expect("the first frame created it")
}

fn text(harness: &EditorHarness) -> String {
    editor(harness).text().to_owned()
}

/// Selects `from..to` with the caret at `to`, and runs the frames that
/// show it.
fn select(harness: &mut EditorHarness, from: usize, to: usize) {
    editor_mut(harness).push_event(Event::Select {
        region: Region::BetweenLocations {
            start: Location::Grapheme(Grapheme(from)),
            end: Location::Grapheme(Grapheme(to)),
        },
    });
    harness.run_steps(2);
}

fn press(harness: &mut EditorHarness, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(2);
}

fn toggled(harness: &EditorHarness, label: &str) -> Option<Toggled> {
    harness.get_by_label(label).accesskit_node().toggled()
}

fn click(harness: &mut EditorHarness, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(2);
}

const TOOLBAR: [&str; 13] = [
    "Bold (Ctrl+B)",
    "Italic (Ctrl+I)",
    "Underline (Ctrl+U)",
    "Strikethrough (Ctrl+Shift+X)",
    "Link (Ctrl+K)",
    "Quote (Ctrl+Alt+Q)",
    "Inline code (Ctrl+Shift+C)",
    "Bulleted list (Ctrl+Shift+8)",
    "Numbered list (Ctrl+Shift+7)",
    "Checklist (Ctrl+Shift+9)",
    "Undo (Ctrl+Z)",
    "Redo (Ctrl+Shift+Z)",
    "Find (Ctrl+F)",
];

#[test]
fn the_toolbar_offers_every_command_labeled_with_its_shortcut() {
    let harness = harness("plain text\n");

    harness.get_by_role_and_label(Role::Toolbar, "Formatting");
    harness.get_by_label("Paragraph style");
    for label in TOOLBAR {
        harness.get_by_label(label);
    }
}

#[test]
fn toolbar_buttons_show_pressed_states_that_follow_the_selection() {
    let mut harness = harness("a **bold** word\n\n- [ ] task\n");

    select(&mut harness, 5, 7);
    assert_eq!(toggled(&harness, "Bold (Ctrl+B)"), Some(Toggled::True));
    assert_eq!(toggled(&harness, "Italic (Ctrl+I)"), Some(Toggled::False));
    assert_eq!(
        toggled(&harness, "Checklist (Ctrl+Shift+9)"),
        Some(Toggled::False)
    );
    // Undo, redo and find are actions, not formats.
    assert_eq!(toggled(&harness, "Undo (Ctrl+Z)"), None);

    select(&mut harness, 24, 24);
    assert_eq!(toggled(&harness, "Bold (Ctrl+B)"), Some(Toggled::False));
    assert_eq!(
        toggled(&harness, "Checklist (Ctrl+Shift+9)"),
        Some(Toggled::True)
    );
}

#[test]
fn toolbar_buttons_format_the_selection_and_undo_and_redo_step_through_it() {
    let mut harness = harness("make this bold\n");
    select(&mut harness, 10, 14);

    click(&mut harness, "Bold (Ctrl+B)");
    assert_eq!(text(&harness), "make this **bold**\n");
    assert_eq!(toggled(&harness, "Bold (Ctrl+B)"), Some(Toggled::True));
    assert!(
        editor(&harness).focused(&harness.ctx),
        "the text keeps the keyboard"
    );

    click(&mut harness, "Undo (Ctrl+Z)");
    assert_eq!(text(&harness), "make this bold\n");
    click(&mut harness, "Redo (Ctrl+Shift+Z)");
    assert_eq!(text(&harness), "make this **bold**\n");
}

#[test]
fn the_paragraph_style_menu_shows_and_sets_the_line_style() {
    let mut harness = harness("## Section\n");
    select(&mut harness, 5, 5);
    assert_eq!(
        harness.get_by_label("Paragraph style").value().as_deref(),
        Some("Heading 2")
    );

    harness.get_by_label("Paragraph style").click();
    harness.run_steps(2);
    harness.get_by_label("Heading 1 (Ctrl+Alt+1)").click();
    harness.run_steps(2);

    assert_eq!(text(&harness), "# Section\n");
}

#[test]
fn a_selection_shows_its_own_toolbar() {
    let mut harness = harness("one two three\n");
    select(&mut harness, 4, 4);
    assert!(harness.query_by_label("Selection formatting").is_none());

    select(&mut harness, 4, 7);
    for label in [
        "Bold selection",
        "Italic selection",
        "Underline selection",
        "Strikethrough selection",
        "Link selection",
    ] {
        harness.get_by_label(label);
    }

    click(&mut harness, "Italic selection");
    assert_eq!(text(&harness), "one *two* three\n");
}

#[test]
fn the_selection_toolbar_hides_while_an_input_method_composes() {
    let mut harness = harness("one two three\n");
    select(&mut harness, 4, 7);
    harness.get_by_label("Bold selection");

    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "に".into(),
        active_range_chars: None,
    }));
    harness.run_steps(2);
    assert!(harness.query_by_label("Bold selection").is_none());

    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: String::new(),
        active_range_chars: None,
    }));
    harness.run_steps(2);
    harness.get_by_label("Bold selection");
}

/// Opening find mid-composition moves the keyboard to the query, and the
/// input method drops its marked text there; the editor ends the
/// composition, so the toolbar shows again once the keyboard comes back.
#[test]
fn the_selection_toolbar_shows_again_after_find_interrupts_a_composition() {
    let mut harness = harness("one two three\n");
    select(&mut harness, 4, 7);
    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "に".into(),
        active_range_chars: None,
    }));
    harness.run_steps(2);
    assert!(harness.query_by_label("Bold selection").is_none());

    press(&mut harness, Modifiers::COMMAND, Key::F);
    assert!(harness.get_by_label("Find in note").is_focused());
    assert!(editor(&harness).edit.renderer.doc.composition().is_none());

    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(editor(&harness).focused(&harness.ctx));
    assert_eq!(editor(&harness).selection(), (Grapheme(4), Grapheme(7)));
    assert_eq!(text(&harness), "one two three\n");
    harness.get_by_label("Bold selection");
}

#[test]
fn the_selection_toolbar_hides_during_a_pointer_drag() {
    let mut harness = harness("alpha beta gamma delta\n");
    let spans = editor(&harness).painted_spans();
    let row = spans
        .iter()
        .find(|span| span.glyphs)
        .expect("the text was drawn")
        .rect;
    let start = egui::pos2(row.left() + 2.0, row.center().y);
    let end = egui::pos2(row.left() + 80.0, row.center().y);

    harness.hover_at(start);
    harness.run_steps(1);
    harness.drag_at(start);
    harness.run_steps(1);
    harness.hover_at(end);
    harness.run_steps(2);
    assert!(
        harness.query_by_label("Bold selection").is_none(),
        "hidden while the pointer drags"
    );

    harness.drop_at(end);
    harness.run_steps(2);
    assert!(
        !editor(&harness)
            .selection()
            .0
            .eq(&editor(&harness).selection().1)
    );
    harness.get_by_label("Bold selection");
}

#[test]
fn shortcuts_format_the_text() {
    let mut harness = harness("word\n");
    select(&mut harness, 0, 4);

    press(&mut harness, Modifiers::COMMAND, Key::B);
    assert_eq!(text(&harness), "**word**\n");
    press(&mut harness, Modifiers::COMMAND, Key::B);
    press(&mut harness, Modifiers::COMMAND, Key::U);
    assert_eq!(text(&harness), "<u>word</u>\n");
    press(&mut harness, Modifiers::COMMAND, Key::Z);
    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::X);
    assert_eq!(text(&harness), "~~word~~\n");
    press(&mut harness, Modifiers::COMMAND, Key::Y);
    assert_eq!(text(&harness), "~~word~~\n", "nothing left to redo");
    press(&mut harness, Modifiers::COMMAND | Modifiers::ALT, Key::Num2);
    assert_eq!(text(&harness), "## ~~word~~\n");
}

#[test]
fn mod_enter_checks_a_task_and_a_click_on_its_box_unchecks_it() {
    let mut harness = harness("intro\n\n- [ ] write the plan\n");
    select(&mut harness, 12, 12);

    press(&mut harness, Modifiers::COMMAND, Key::Enter);
    assert_eq!(text(&harness), "intro\n\n- [x] write the plan\n");

    // With the caret back in the first line, the box draws; a click on it
    // flips the task again.
    select(&mut harness, 0, 0);
    let write = 13;
    let span = editor(&harness)
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0.0 <= write && write < span.source_range.1.0)
        .expect("the task's text was drawn");
    let checkbox = egui::pos2(
        span.rect.left() - span.rect.height() * 0.6,
        span.rect.center().y,
    );
    harness.hover_at(checkbox);
    harness.run_steps(1);
    harness.drag_at(checkbox);
    harness.run_steps(1);
    harness.drop_at(checkbox);
    harness.run_steps(2);

    assert_eq!(text(&harness), "intro\n\n- [ ] write the plan\n");
}

#[test]
fn tab_outside_a_list_moves_the_keyboard_out_of_the_text() {
    let mut harness = harness("- a\n- b\n\nplain\n");
    select(&mut harness, 7, 7);
    press(&mut harness, Modifiers::NONE, Key::Tab);
    assert_eq!(text(&harness), "- a\n  - b\n\nplain\n");
    assert!(editor(&harness).focused(&harness.ctx));

    select(&mut harness, 13, 13);
    press(&mut harness, Modifiers::NONE, Key::Tab);
    assert_eq!(text(&harness), "- a\n  - b\n\nplain\n");
    assert!(!editor(&harness).focused(&harness.ctx));
}

#[test]
fn find_opens_with_the_selection_and_steps_through_matches() {
    let mut harness = harness("beta alpha beta gamma beta\n");
    select(&mut harness, 0, 4);

    press(&mut harness, Modifiers::COMMAND, Key::F);
    let field = harness.get_by_label("Find in note");
    assert_eq!(field.value().as_deref(), Some("beta"));
    assert!(field.is_focused());
    harness.get_by_label("1/3");
    for label in [
        "Previous match (Shift+Enter)",
        "Next match (Enter)",
        "Close (Esc)",
        "Replace with",
        "Replace",
        "Replace all",
    ] {
        harness.get_by_label(label);
    }

    press(&mut harness, Modifiers::NONE, Key::Enter);
    harness.get_by_label("2/3");
    press(&mut harness, Modifiers::SHIFT, Key::Enter);
    press(&mut harness, Modifiers::SHIFT, Key::Enter);
    harness.get_by_label("3/3");
    click(&mut harness, "Next match (Enter)");
    harness.get_by_label("1/3");

    // Escape closes the bar and hands the keyboard back with the match
    // selected.
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(harness.query_by_label("Find in note").is_none());
    assert!(editor(&harness).focused(&harness.ctx));
    assert_eq!(editor(&harness).selection(), (Grapheme(0), Grapheme(4)));
}

#[test]
fn typing_a_query_counts_literal_matches() {
    let mut harness = harness("a.b axb A.B\n");
    press(&mut harness, Modifiers::COMMAND, Key::F);

    harness.get_by_label("Find in note").type_text("a.b");
    harness.run_steps(2);
    harness.get_by_label("1/2");

    harness.get_by_label("Find in note").type_text("z");
    harness.run_steps(2);
    harness.get_by_label("No results");
}

#[test]
fn replace_and_replace_all_change_the_matches() {
    let mut harness = harness("cat dog cat dog cat\n");
    select(&mut harness, 0, 3);
    press(&mut harness, Modifiers::COMMAND, Key::F);

    harness.get_by_label("Replace with").focus();
    harness.run_steps(1);
    harness.get_by_label("Replace with").type_text("cow");
    harness.run_steps(1);
    click(&mut harness, "Replace");
    assert_eq!(text(&harness), "cow dog cat dog cat\n");
    harness.get_by_label("1/2");

    click(&mut harness, "Replace all");
    assert_eq!(text(&harness), "cow dog cow dog cow\n");
    harness.get_by_label("No results");

    // Replace all undoes in one step.
    press(&mut harness, Modifiers::NONE, Key::Escape);
    press(&mut harness, Modifiers::COMMAND, Key::Z);
    assert_eq!(text(&harness), "cow dog cat dog cat\n");
}
