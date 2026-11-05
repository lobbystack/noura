//! The Inbox page through AccessKit: where it sits, what it shows in what
//! order, its labels and its keyboard path.

mod support;

use std::{path::Path, sync::Arc};

use app_core::{App, AppError, Platform};
use egui::{Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{By, NodeT, Queryable},
};
use support::{FakePicker, FakeSystem, create_workspace, harness_on, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

struct Opened {
    harness: Ui,
    system: Arc<FakeSystem>,
    /// The app data and the workspace's parent folder, which last as long
    /// as the test binds them.
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace named Garden holding `files`, opened at launch after
/// `register` adds test pages and Inbox sections.
fn opened(files: &[(&str, &str)], register: impl FnOnce(&mut App)) -> Opened {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    for (relative, text) in files {
        write(&folder, relative, text);
    }
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_on(
        app_data.path(),
        FakePicker::default(),
        system.clone(),
        register,
    );
    settle(&mut harness);
    Opened {
        harness,
        system,
        _folders: (app_data, parent),
    }
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A plugin page and its Inbox section, which shows `lines`.
fn plugin(app: &mut App, id: &str, page: &str, title: &str, lines: &[&str]) {
    let page_id = format!("test-{id}");
    app.register_page(&page_id, page);
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    app.add_inbox_section(
        id,
        Some(&page_id),
        title,
        Arc::new(move || Ok(lines.clone())),
    );
}

fn rail_page<'a>(harness: &'a Ui, title: &'a str) -> egui_kittest::Node<'a> {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, title)
}

/// The rail's page titles, top to bottom.
fn rail(harness: &Ui) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_all_by_role(Role::Link)
        .filter_map(|link| link.accesskit_node().label())
        .collect()
}

fn open_inbox(harness: &mut Ui) {
    rail_page(harness, "Inbox").click();
    harness.run();
}

fn document(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::Region, "Document")
}

/// The Inbox's headings, top to bottom.
fn headings(harness: &Ui) -> Vec<String> {
    document(harness)
        .get_all_by_role(Role::Heading)
        .filter_map(|heading| heading.accesskit_node().label())
        .collect()
}

/// The labels of the Inbox's links, top to bottom.
fn links(harness: &Ui) -> Vec<String> {
    document(harness)
        .get_all_by_role(Role::Link)
        .filter_map(|link| link.accesskit_node().label())
        .collect()
}

/// Each tab's label, left to right.
fn tabs(harness: &Ui) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::TabList, "Tabs")
        .map(|list| {
            list.get_all_by_role(Role::Tab)
                .filter_map(|tab| tab.accesskit_node().label())
                .collect()
        })
        .unwrap_or_default()
}

fn selected_tab(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

/// Every switch, checkbox and radio button in every Settings section.
fn settings_switches(harness: &mut Ui) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    let sections: Vec<String> = harness
        .get_by_role_and_label(Role::Navigation, "Settings sections")
        .get_all_by_role(Role::Button)
        .filter_map(|button| button.accesskit_node().label())
        .collect();
    let mut switches = Vec::new();
    for section in sections {
        harness
            .get_by_role_and_label(Role::Navigation, "Settings sections")
            .get_by_role_and_label(Role::Button, &section)
            .click();
        harness.run();
        switches.extend(
            harness
                .get_by_role_and_label(Role::Dialog, "Settings")
                .query_all(By::new().predicate(|node| {
                    matches!(
                        node.role(),
                        Role::Switch | Role::CheckBox | Role::RadioButton
                    )
                }))
                .filter_map(|node| node.accesskit_node().label()),
        );
    }
    switches
}

#[test]
fn the_inbox_is_in_the_rail_whatever_plugins_are_on_and_has_no_switch_to_turn_it_off() {
    type Plugins = fn(&mut App);
    let combinations: [(Plugins, &[&str]); 4] = [
        (|_| {}, &["Inbox", "Files"]),
        (
            |app| plugin(app, "tasks", "Tasks", "Due today", &[]),
            &["Inbox", "Files", "Tasks"],
        ),
        (
            |app| plugin(app, "calendar", "Calendar", "Next 7 days", &[]),
            &["Inbox", "Files", "Calendar"],
        ),
        (
            |app| {
                plugin(app, "tasks", "Tasks", "Due today", &[]);
                plugin(app, "calendar", "Calendar", "Next 7 days", &[]);
            },
            &["Inbox", "Files", "Tasks", "Calendar"],
        ),
    ];

    for (plugins, expected) in combinations {
        let Opened {
            mut harness,
            _folders,
            ..
        } = opened(&[], plugins);

        assert_eq!(rail(&harness), expected);
        open_inbox(&mut harness);
        assert_eq!(headings(&harness)[0], "Good morning");
        let switches = settings_switches(&mut harness);
        assert!(
            !switches.iter().any(|label| label.contains("Inbox")),
            "{switches:?}"
        );
    }
}

#[test]
fn the_inbox_greets_you_then_shows_needs_attention_the_plugin_sections_and_recent() {
    let Opened {
        mut harness,
        system,
        _folders,
    } = opened(
        &[
            ("Plan.md", "# Plan\n"),
            ("Broken.md", "---\nid: [broken\n---\n"),
        ],
        |app| plugin(app, "tasks", "Tasks", "Due today", &["Water the plants"]),
    );

    open_inbox(&mut harness);

    assert_eq!(
        headings(&harness),
        ["Good morning", "Needs attention", "Due today", "Recent"]
    );
    assert!(
        document(&harness)
            .query_by_label("Water the plants")
            .is_some()
    );

    system.set_hour(4);
    harness.run();
    assert_eq!(headings(&harness)[0], "Working late");
    system.set_hour(20);
    harness.run();
    assert_eq!(headings(&harness)[0], "Good evening");
}

#[test]
fn recent_opens_a_note_on_click_or_enter_as_the_file_tree_does() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(
        &[("Ideas/Apples.md", "# Apples\n"), ("Pears.md", "# Pears\n")],
        |_| {},
    );
    open_inbox(&mut harness);
    assert_eq!(headings(&harness), ["Good morning", "Recent"]);

    document(&harness)
        .get_by_role_and_label(Role::Link, "Apples")
        .click();
    harness.run();
    assert_eq!(tabs(&harness), ["Apples (preview)"]);
    assert_eq!(note_text(&harness), "# Apples\n");

    open_inbox(&mut harness);
    document(&harness)
        .get_by_role_and_label(Role::Link, "Pears")
        .focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(tabs(&harness), ["Pears (preview)"]);
    assert_eq!(selected_tab(&harness).as_deref(), Some("Pears (preview)"));
    assert_eq!(note_text(&harness), "# Pears\n");
}

/// The text the open note shows.
fn note_text(harness: &Ui) -> String {
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .value()
        .expect("the note reports its text")
}

/// Cmd-click (Ctrl-click on Windows and Linux) or a middle-click keeps a
/// tab, as in the file tree.
#[test]
fn cmd_click_or_a_middle_click_on_a_recent_note_keeps_its_tab() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(
        &[("Ideas/Apples.md", "# Apples\n"), ("Pears.md", "# Pears\n")],
        |_| {},
    );
    open_inbox(&mut harness);

    document(&harness)
        .get_by_role_and_label(Role::Link, "Apples")
        .click_modifiers(Modifiers::COMMAND);
    harness.run();
    assert!(tabs(&harness).contains(&"Apples".to_owned()));
    assert_eq!(selected_tab(&harness).as_deref(), Some("Apples"));

    open_inbox(&mut harness);
    document(&harness)
        .get_by_role_and_label(Role::Link, "Pears")
        .click_button(PointerButton::Middle);
    harness.run();
    assert!(tabs(&harness).contains(&"Pears".to_owned()));
    assert_eq!(selected_tab(&harness).as_deref(), Some("Pears"));
    assert!(tabs(&harness).contains(&"Apples".to_owned()));
}

/// Two notes with the same name show their paths, so their links don't
/// read the same.
#[test]
fn recent_notes_that_share_a_title_show_their_paths() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(
        &[("Plan.md", "# Plan\n"), ("Ideas/Plan.md", "# Plan\n")],
        |_| {},
    );
    open_inbox(&mut harness);

    let mut shown = links(&harness);
    shown.sort();
    assert_eq!(shown, ["Plan, Ideas/Plan.md", "Plan, Plan.md"]);
    assert!(document(&harness).query_by_label("Ideas/Plan.md").is_some());

    document(&harness)
        .get_by_role_and_label(Role::Link, "Plan, Ideas/Plan.md")
        .click();
    harness.run();
    assert_eq!(tabs(&harness), ["Plan (preview)"]);
}

#[test]
fn an_empty_workspace_says_it_has_no_notes_yet() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(&[], |_| {});

    open_inbox(&mut harness);

    assert_eq!(headings(&harness), ["Good morning", "Recent"]);
    assert!(document(&harness).query_by_label("No notes yet.").is_some());
}

#[test]
fn a_section_that_fails_shows_its_error_in_its_own_place_while_the_rest_draws() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(&[("Plan.md", "# Plan\n")], |app| {
        app.register_page("test-tasks", "Tasks");
        app.add_inbox_section(
            "tasks",
            Some("test-tasks"),
            "Due today",
            Arc::new(|| Err(AppError::new("plugin_failed", "Tasks stopped responding."))),
        );
        plugin(
            app,
            "calendar",
            "Calendar",
            "Next 7 days",
            &["Dentist on Thursday"],
        );
    });

    open_inbox(&mut harness);

    assert_eq!(
        headings(&harness),
        ["Good morning", "Due today", "Next 7 days", "Recent"]
    );
    assert!(
        document(&harness)
            .query_by_role_and_label(Role::Alert, "Tasks stopped responding.")
            .is_some()
    );
    assert!(
        document(&harness)
            .query_by_label("Dentist on Thursday")
            .is_some()
    );
    assert_eq!(links(&harness), ["Plan"]);
}

const BROKEN: &str = "---\nid: [broken\n---\n";
const COPY: &str = "---\nid: note_01k00000000000000000000030\ntype: note\n---\n\nA copy.\n";
const CALL: &str = "---\nid: task_01k00000000000000000000003\ntype: task\nstatus: todo\n\
                    project: project_01k00000000000000000000099\n---\n\nCall.\n";

#[test]
fn problems_and_links_carry_labels_that_name_their_files_and_the_keyboard_reaches_each_link() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(
        &[
            ("Notes/broken.md", BROKEN),
            ("Notes/dup-a.md", COPY),
            ("Notes/dup-b.md", COPY),
            ("Tasks/call.md", CALL),
        ],
        |app| plugin(app, "tasks", "Tasks", "Due today", &["Water the plants"]),
    );
    open_inbox(&mut harness);

    let problems: Vec<String> = document(&harness)
        .get_by_role_and_label(Role::List, "Needs attention")
        .get_all_by_role(Role::ListItem)
        .filter_map(|item| item.accesskit_node().label())
        .collect();
    assert_eq!(
        problems,
        [
            "Tasks/call.md links to an object that no longer exists.",
            "Notes/dup-a.md and Notes/dup-b.md use the same ID. Remove the id line from the copy.",
            "Noura can't read the properties at the top of Notes/broken.md.",
        ]
    );
    let expected = [
        "Open Tasks/call.md",
        "Open Notes/dup-a.md",
        "Open Notes/dup-b.md",
        "Open Notes/broken.md",
        "broken",
        "dup-a",
        "dup-b",
    ];
    let mut shown = links(&harness);
    // Recent lists the notes newest first; the files were written so
    // quickly that their order is the file system's.
    shown[4..].sort();
    assert_eq!(shown, expected);
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());

    // Tab walks the links in order, from the first.
    document(&harness)
        .get_by_role_and_label(Role::Link, "Open Tasks/call.md")
        .focus();
    harness.run();
    let mut reached = Vec::new();
    for _ in 0..expected.len() {
        let focused = document(&harness)
            .get_all_by_role(Role::Link)
            .find(|link| link.is_focused())
            .and_then(|link| link.accesskit_node().label());
        reached.extend(focused);
        harness.key_press(Key::Tab);
        harness.run();
    }
    reached[4..].sort();
    assert_eq!(reached, expected);

    // Enter follows the focused link.
    document(&harness)
        .get_by_role_and_label(Role::Link, "Open Notes/broken.md")
        .focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(selected_tab(&harness).as_deref(), Some("broken (preview)"));
}
