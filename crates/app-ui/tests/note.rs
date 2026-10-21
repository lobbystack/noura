//! The note screen through AccessKit: a note opens from the tree into the
//! editor, the properties panel labels each property, the name field's
//! keyboard path renames the file, and the prompt about a note that
//! couldn't save offers its three choices.

mod support;

use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const NOTE: &str = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\ncolor: green\n\
                    done: false\ncreated: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n\
                    ---\n# Heading\n\nSome **bold** text.\n";

/// A workspace whose `Docs` folder holds the note `Plan.md`, opened.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    opened_with(app_data, folder, &[])
}

/// A workspace whose `Docs` folder holds the note `Plan.md` and `files`,
/// each a name and its text, opened.
fn opened_with(app_data: &std::path::Path, folder: &std::path::Path, files: &[(&str, &str)]) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::create_dir_all(folder.join("Docs")).unwrap();
    std::fs::write(folder.join("Docs/Plan.md"), NOTE).unwrap();
    for (name, text) in files {
        std::fs::write(folder.join("Docs").join(name), text).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

/// Open `Docs/Plan.md` from the tree.
fn open_plan(harness: &mut Ui) {
    harness
        .get_by_role_and_label(Role::TreeItem, "Docs")
        .click();
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
}

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    harness.run_steps(4);
}

fn name_field(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::TextInput, "File name")
}

#[test]
fn a_note_opens_from_the_tree_in_the_editor_with_its_name_and_properties() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));

    open_plan(&mut harness);

    harness.get_by_role_and_label(Role::Document, "Note text");
    assert_eq!(
        name_field(&harness).accesskit_node().value().as_deref(),
        Some("Plan")
    );
    let panel = harness.get_by_role_and_label(Role::Group, "Properties");
    let labels: Vec<String> = panel
        .get_all_by_role(Role::Label)
        .filter_map(|label| {
            label
                .accesskit_node()
                .value()
                .or(label.accesskit_node().label())
        })
        .collect();
    for key in ["id", "type", "color", "done", "created", "updated"] {
        assert!(
            labels.iter().any(|label| label == key),
            "{key} in {labels:?}"
        );
    }
    // Noura's own properties read as read-only, and the others edit in place.
    for key in ["id", "type", "created", "updated"] {
        let field = panel.get_by_role_and_label(Role::TextInput, key);
        assert!(field.accesskit_node().is_read_only(), "{key}");
    }
    let color = panel.get_by_role_and_label(Role::TextInput, "color");
    assert!(!color.accesskit_node().is_read_only());
    assert_eq!(color.accesskit_node().value().as_deref(), Some("green"));
    panel.get_by_role_and_label(Role::CheckBox, "done");
}

#[test]
fn a_markdown_file_without_an_id_opens_without_a_properties_panel() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::write(folder.join("Plain.md"), "---\ntags: [a]\n---\nText\n").unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    harness
        .get_by_role_and_label(Role::TreeItem, "Plain")
        .click();
    harness.run_steps(4);

    harness.get_by_role_and_label(Role::Document, "Note text");
    assert!(
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_none()
    );
}

/// Frontmatter with a lone carriage return once made the live preview read
/// every line after it one line off, and a callout there crashed it. Such
/// a file now opens in the live preview and keeps its bytes.
#[test]
fn a_file_with_a_lone_carriage_return_in_its_frontmatter_opens_in_the_preview() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let bytes = "---\na\rb\n---\n> [!note] A\n";
    std::fs::write(folder.join("Odd.md"), bytes).unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    harness.get_by_role_and_label(Role::TreeItem, "Odd").click();
    harness.run_steps(4);

    harness.get_by_role_and_label(Role::Document, "Note text");
    assert_eq!(
        std::fs::read_to_string(folder.join("Odd.md")).unwrap(),
        bytes
    );
}

#[test]
fn f2_moves_from_the_text_to_the_name_and_enter_renames_the_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    open_plan(&mut harness);
    assert!(!name_field(&harness).accesskit_node().is_focused());

    harness.key_press(Key::F2);
    harness.run_steps(4);
    assert!(name_field(&harness).accesskit_node().is_focused());
    // The whole name is selected, so typing replaces it.
    harness.event(Event::Text("Roadmap".into()));
    harness.run_steps(4);
    harness.key_press(Key::Enter);
    harness.run_steps(4);

    assert!(!folder.join("Docs/Plan.md").exists());
    assert_eq!(
        std::fs::read_to_string(folder.join("Docs/Roadmap.md")).unwrap(),
        NOTE
    );
    assert_eq!(
        name_field(&harness).accesskit_node().value().as_deref(),
        Some("Roadmap")
    );
    assert!(!name_field(&harness).accesskit_node().is_focused());
}

#[test]
fn escape_in_the_name_field_keeps_the_old_name() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    open_plan(&mut harness);

    harness.key_press(Key::F2);
    harness.run_steps(4);
    harness.event(Event::Text("Something else".into()));
    harness.run_steps(4);
    harness.key_press(Key::Escape);
    harness.run_steps(4);

    assert!(folder.join("Docs/Plan.md").exists());
    assert_eq!(
        name_field(&harness).accesskit_node().value().as_deref(),
        Some("Plan")
    );
}

#[test]
fn a_taken_name_says_so_under_the_field() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    std::fs::write(folder.join("Docs/Taken.md"), "x").unwrap();
    open_plan(&mut harness);

    harness.key_press(Key::F2);
    harness.run_steps(4);
    harness.event(Event::Text("Taken".into()));
    harness.run_steps(4);
    harness.key_press(Key::Enter);
    harness.run_steps(4);

    assert!(folder.join("Docs/Plan.md").exists());
    harness.get_by_label("A file with that name already exists in this folder.");
}

#[test]
fn typing_in_the_note_saves_it_after_a_pause() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    open_plan(&mut harness);

    harness.event(Event::Text("Typed. ".into()));
    harness.run_steps(4);

    let path = folder.join("Docs/Plan.md");
    wait_until(&mut harness, |_| {
        std::fs::read_to_string(&path).unwrap() != NOTE
    });
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(written.matches("Typed. ").count(), 1, "{written:?}");
    assert_eq!(written.replace("Typed. ", ""), NOTE);
}

/// Click the property field `key` and type `typed` at its end.
fn type_in_property(harness: &mut Ui, key: &str, typed: &str) {
    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::TextInput, key)
        .click();
    harness.run_steps(4);
    harness.key_press(Key::End);
    harness.event(Event::Text(typed.into()));
    harness.run_steps(4);
}

/// Close Tab runs before a field could report losing the keyboard, so
/// what the field holds goes in first: a property's value, then the name.
#[test]
fn closing_the_tab_from_a_field_keeps_what_you_typed_in_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    open_plan(&mut harness);
    type_in_property(&mut harness, "color", "ish");
    let path = folder.join("Docs/Plan.md");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), NOTE);

    harness.key_press_modifiers(Modifiers::COMMAND, Key::W);
    harness.run_steps(4);

    wait_until(&mut harness, |harness| {
        harness.state().app().tabs().is_empty()
    });
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("\ncolor: greenish\n"), "{written:?}");

    // Docs stays open in the tree.
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    harness.key_press(Key::F2);
    harness.run_steps(4);
    harness.event(Event::Text("Roadmap".into()));
    harness.run_steps(4);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::W);
    harness.run_steps(4);

    wait_until(&mut harness, |harness| {
        harness.state().app().tabs().is_empty()
    });
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(folder.join("Docs/Roadmap.md")).unwrap(),
        written
    );
}

/// A click on another tab hides the field without it reporting the lost
/// keyboard: the value goes in anyway, and shows when you come back.
#[test]
fn a_property_typed_before_switching_tabs_goes_in() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    std::fs::write(folder.join("Docs/Other.md"), "Other text\n").unwrap();
    let app = harness.state_mut().app_mut();
    app.open_file("Docs/Other.md", app_core::OpenAs::Kept);
    app.open_file("Docs/Plan.md", app_core::OpenAs::Kept);
    harness.run_steps(4);
    type_in_property(&mut harness, "color", "ish");

    harness.get_by_role_and_label(Role::Tab, "Other").click();
    harness.run_steps(4);

    let path = folder.join("Docs/Plan.md");
    wait_until(&mut harness, |_| {
        std::fs::read_to_string(&path).unwrap() != NOTE
    });
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("\ncolor: greenish\n"), "{written:?}");
    harness.get_by_role_and_label(Role::Tab, "Plan").click();
    harness.run_steps(4);
    let color = harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::TextInput, "color");
    assert_eq!(color.accesskit_node().value().as_deref(), Some("greenish"));
}

/// The two kinds of field next to a note's text.
#[derive(Debug, Clone, Copy)]
enum Field {
    /// F2, then `Roadmap` over the name `Plan`.
    Name,
    /// `ish` at the end of `color: green`.
    Property,
}

const FIELDS: [Field; 2] = [Field::Name, Field::Property];

/// Type in `field` of the note the window shows, without leaving it.
fn type_in(harness: &mut Ui, field: Field) {
    match field {
        Field::Name => {
            harness.key_press(Key::F2);
            harness.run_steps(4);
            harness.event(Event::Text("Roadmap".into()));
            harness.run_steps(4);
        }
        Field::Property => type_in_property(harness, "color", "ish"),
    }
}

/// Whether what `type_in` typed in `field` reached the file.
fn went_in(folder: &std::path::Path, field: Field) -> bool {
    match field {
        Field::Name => {
            !folder.join("Docs/Plan.md").exists() && folder.join("Docs/Roadmap.md").exists()
        }
        Field::Property => std::fs::read_to_string(folder.join("Docs/Plan.md"))
            .is_ok_and(|text| text.contains("\ncolor: greenish\n")),
    }
}

/// Typing in a field of the note `Plan`, then `leave`, keeps the typing,
/// for the name field and a property field.
fn leaving_keeps_the_typing(leave: impl Fn(&mut Ui)) {
    for field in FIELDS {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        let mut harness = opened_with(app_data.path(), &folder, &[("Other.md", "Other text\n")]);
        open_plan(&mut harness);
        type_in(&mut harness, field);
        assert!(!went_in(&folder, field), "{field:?} waits for leaving");

        leave(&mut harness);
        harness.run_steps(4);

        wait_until(&mut harness, |_| went_in(&folder, field));
    }
}

/// The close button reacts to the click before the note draws its fields
/// again, so what a field holds goes in as the click lands.
#[test]
fn the_tab_s_close_button_keeps_what_you_typed_in_a_field() {
    leaving_keeps_the_typing(|harness| {
        harness
            .get_by_role_and_label(Role::Button, "Close Plan")
            .click();
    });
}

#[test]
fn a_screen_reader_s_click_on_the_close_button_keeps_what_you_typed_in_a_field() {
    leaving_keeps_the_typing(|harness| {
        harness
            .get_by_role_and_label(Role::Button, "Close Plan")
            .click_accesskit();
    });
}

#[test]
fn a_middle_click_on_the_tab_keeps_what_you_typed_in_a_field() {
    leaving_keeps_the_typing(|harness| {
        harness
            .get_by_role_and_label(Role::Tab, "Plan (preview)")
            .click_button(egui::PointerButton::Middle);
    });
}

/// A note opened from the tree is a preview, which the next file opened
/// from the tree replaces.
#[test]
fn opening_another_file_in_the_preview_tab_keeps_what_you_typed_in_a_field() {
    leaving_keeps_the_typing(|harness| {
        harness
            .get_by_role_and_label(Role::TreeItem, "Other")
            .click();
    });
}

/// A click inside the field you type in moves the caret and leaves the
/// typing in the field, so Escape still cancels it.
#[test]
fn a_click_in_the_field_you_type_in_keeps_the_typing_there() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    open_plan(&mut harness);
    type_in(&mut harness, Field::Name);

    name_field(&harness).click();
    harness.run_steps(4);
    assert!(name_field(&harness).accesskit_node().is_focused());
    harness.key_press(Key::Escape);
    harness.run_steps(4);

    assert!(folder.join("Docs/Plan.md").exists());
    assert!(!folder.join("Docs/Roadmap.md").exists());
}

/// A file that isn't UTF-8 text says so, and Try again reads it again.
#[test]
fn try_again_opens_a_file_that_couldn_t_open_before() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::write(folder.join("Bytes.md"), b"\xff\xfe not UTF-8").unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Bytes")
        .click();
    harness.run_steps(4);
    harness.get_by_label("This file isn't UTF-8 text, so the editor can't open it.");

    std::fs::write(folder.join("Bytes.md"), "Text now\n").unwrap();
    harness
        .get_by_role_and_label(Role::Button, "Try again")
        .click();
    harness.run_steps(4);

    harness.get_by_role_and_label(Role::Document, "Note text");
}

#[cfg(unix)]
mod failed_saves {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn read_only(folder: &std::path::Path, read_only: bool) {
        let mode = if read_only { 0o555 } else { 0o755 };
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The note open with typing that couldn't save, and the prompt that
    /// closing its tab brings up.
    fn prompt(folder: &std::path::Path, harness: &mut Ui) {
        open_plan(harness);
        read_only(&folder.join("Docs"), true);
        harness.event(Event::Text("Typed. ".into()));
        harness.run_steps(4);
        wait_until(harness, |harness| {
            harness
                .query_by_role_and_label(Role::Alert, "Changes aren\u{2019}t saved")
                .is_some()
        });
        harness.key_press_modifiers(Modifiers::COMMAND, Key::W);
        harness.run_steps(4);
        wait_until(harness, |harness| {
            harness
                .query_by_role_and_label(
                    Role::AlertDialog,
                    "Couldn\u{2019}t save \u{201c}Plan\u{201d}",
                )
                .is_some()
        });
    }

    #[test]
    fn a_failed_save_shows_why_in_the_note_with_try_again() {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        let mut harness = opened(app_data.path(), &folder);
        open_plan(&mut harness);
        read_only(&folder.join("Docs"), true);

        harness.event(Event::Text("Typed. ".into()));
        harness.run_steps(4);
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::Alert, "Changes aren\u{2019}t saved")
                .is_some()
        });

        harness.get_by_label("Noura doesn't have permission to change this file.");
        read_only(&folder.join("Docs"), false);
        harness
            .get_by_role_and_label(Role::Button, "Try again")
            .click();
        harness.run_steps(4);
        let path = folder.join("Docs/Plan.md");
        wait_until(&mut harness, |_| {
            std::fs::read_to_string(&path).unwrap() != NOTE
        });
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::Alert, "Changes aren\u{2019}t saved")
                .is_none()
        });
    }

    #[test]
    fn the_prompt_names_the_file_and_escape_keeps_editing() {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        let mut harness = opened(app_data.path(), &folder);
        prompt(&folder, &mut harness);

        harness.get_by_label(
            "Noura doesn't have permission to change this file. Your changes are still in the \
             note.",
        );
        let dialog = harness.get_by_role_and_label(
            Role::AlertDialog,
            "Couldn\u{2019}t save \u{201c}Plan\u{201d}",
        );
        dialog.get_by_role_and_label(Role::Button, "Keep editing");
        dialog.get_by_role_and_label(Role::Button, "Discard changes");
        assert!(
            dialog
                .get_by_role_and_label(Role::Button, "Try again")
                .accesskit_node()
                .is_focused()
        );
        // Tab moves along the prompt's buttons.
        harness.key_press(Key::Tab);
        harness.run_steps(4);
        let dialog = harness.get_by_role_and_label(
            Role::AlertDialog,
            "Couldn\u{2019}t save \u{201c}Plan\u{201d}",
        );
        assert!(
            !dialog
                .get_by_role_and_label(Role::Button, "Try again")
                .accesskit_node()
                .is_focused()
        );

        harness.key_press(Key::Escape);
        harness.run_steps(4);

        assert!(
            harness
                .query_by_role_and_label(
                    Role::AlertDialog,
                    "Couldn\u{2019}t save \u{201c}Plan\u{201d}"
                )
                .is_none()
        );
        harness.get_by_role_and_label(Role::Document, "Note text");
        read_only(&folder.join("Docs"), false);
        assert_eq!(
            std::fs::read_to_string(folder.join("Docs/Plan.md")).unwrap(),
            NOTE
        );
    }

    #[test]
    fn enter_on_try_again_saves_and_closes_the_tab() {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        let mut harness = opened(app_data.path(), &folder);
        prompt(&folder, &mut harness);
        read_only(&folder.join("Docs"), false);

        harness.key_press(Key::Enter);
        harness.run_steps(4);

        let path = folder.join("Docs/Plan.md");
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::Document, "Note text")
                .is_none()
        });
        assert_ne!(std::fs::read_to_string(&path).unwrap(), NOTE);
    }

    /// The title of the prompt on screen, once one shows.
    fn prompt_title(harness: &mut Ui) -> String {
        wait_until(harness, |harness| {
            harness.query_by_role(Role::AlertDialog).is_some()
        });
        harness
            .get_by_role(Role::AlertDialog)
            .accesskit_node()
            .label()
            .unwrap_or_default()
    }

    /// Two notes can't save as you quit. Discard changes on the first
    /// prompt, then Keep editing on the second: the first note stays open
    /// and its editor shows the file's text, so your next edit saves on top
    /// of it and never brings the discarded typing back.
    #[test]
    fn a_discarded_note_that_stays_open_drops_its_typing_from_the_editor() {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        create_workspace(app_data.path(), "Garden", &folder);
        std::fs::create_dir_all(folder.join("Docs")).unwrap();
        std::fs::write(folder.join("Docs/Plan.md"), NOTE).unwrap();
        std::fs::write(folder.join("Docs/Other.md"), "Other text\n").unwrap();
        let mut harness = harness(app_data.path(), FakePicker::default());
        settle(&mut harness);
        open_plan(&mut harness);
        read_only(&folder.join("Docs"), true);
        harness.event(Event::Text("Typed. ".into()));
        harness.run_steps(4);
        harness
            .get_by_role_and_label(Role::TreeItem, "Other")
            .click();
        harness.run_steps(4);
        harness.event(Event::Text("Typed. ".into()));
        harness.run_steps(4);
        wait_until(&mut harness, |harness| {
            harness.state().app().tabs().len() == 2
                && harness
                    .state()
                    .app()
                    .tabs()
                    .iter()
                    .filter_map(|tab| harness.state().app().note(tab.id()))
                    .all(|note| note.save_problem().is_some())
        });

        assert!(
            harness
                .state_mut()
                .run_menu_command(app_core::command_id::QUIT, Instant::now())
        );
        harness.run_steps(4);
        let first = prompt_title(&mut harness);
        harness
            .get_by_role(Role::AlertDialog)
            .get_by_role_and_label(Role::Button, "Discard changes")
            .click();
        harness.run_steps(4);
        let second = prompt_title(&mut harness);
        assert_ne!(first, second);
        harness
            .get_by_role(Role::AlertDialog)
            .get_by_role_and_label(Role::Button, "Keep editing")
            .click();
        harness.run_steps(4);
        assert!(harness.query_by_role(Role::AlertDialog).is_none());
        assert!(!harness.state().is_quitting());

        // Back to the discarded note, now that its folder takes writes.
        read_only(&folder.join("Docs"), false);
        let (name, original) = if first.contains("Plan") {
            ("Plan.md", NOTE)
        } else {
            ("Other.md", "Other text\n")
        };
        let discarded = harness
            .state()
            .app()
            .tabs()
            .iter()
            .find(|tab| {
                tab.file()
                    .is_some_and(|file| file.relative_path.ends_with(name))
            })
            .unwrap()
            .id();
        harness.state_mut().app_mut().activate_tab(discarded);
        harness.run_steps(4);
        harness
            .get_by_role_and_label(Role::MultilineTextInput, "Note text")
            .click();
        harness.run_steps(4);
        harness.event(Event::Text("More. ".into()));
        harness.run_steps(4);

        let path = folder.join("Docs").join(name);
        wait_until(&mut harness, |_| {
            std::fs::read_to_string(&path).unwrap() != original
        });
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("Typed. "), "{written:?}");
        assert_eq!(written.replace("More. ", ""), original, "{written:?}");
    }

    #[test]
    fn discard_changes_closes_the_tab_and_leaves_the_file() {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        let mut harness = opened(app_data.path(), &folder);
        prompt(&folder, &mut harness);

        harness
            .get_by_role_and_label(
                Role::AlertDialog,
                "Couldn\u{2019}t save \u{201c}Plan\u{201d}",
            )
            .get_by_role_and_label(Role::Button, "Discard changes")
            .click();
        harness.run_steps(4);

        assert!(
            harness
                .query_by_role_and_label(Role::Document, "Note text")
                .is_none()
        );
        read_only(&folder.join("Docs"), false);
        assert_eq!(
            std::fs::read_to_string(folder.join("Docs/Plan.md")).unwrap(),
            NOTE
        );
    }
}
