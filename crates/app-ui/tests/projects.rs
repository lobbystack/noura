//! The Projects plugin through AccessKit: a project's page, its overview
//! in the note editor beside the Projects panel's labeled status, board,
//! tasks, dated work, notes and Delete project, and moving a card on the
//! board by dragging it and from the keyboard.

mod support;

use std::path::{Path, PathBuf};

use egui::{Key, Modifiers, Pos2, accesskit::Role, vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    FakePicker, create_workspace, harness,
    kit::{Ui, focused, labels, press, press_with, run},
    plugins, settle,
};

const TASKS_MANIFEST: &str = include_str!("../../plugins/tasks/plugin.toml");
const PROJECTS_MANIFEST: &str = include_str!("../../plugins/projects/plugin.toml");

const GARDEN: &str = "project_01jbcdefghjkmnpqrstvwxyz0h";

const PROJECT: &str = "---\nid: project_01jbcdefghjkmnpqrstvwxyz0h\ntype: project\nstatus: active\n\
                       created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\n\
                       Grow vegetables.\n";

const NOTE: &str = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n\
                    created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nLoam.\n";

fn task(suffix: &str, properties: &str) -> String {
    format!(
        "---\nid: task_01jbcdefghjkmnpqrstvwxyz0{suffix}\ntype: task\n{properties}\
         project: {GARDEN}\ncreated: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n\
         ---\nNotes.\n"
    )
}

struct Opened {
    harness: Ui,
    folder: PathBuf,
    _folders: [tempfile::TempDir; 4],
}

/// A workspace with Garden, its note and three of its tasks, with the
/// Tasks and Projects plugins installed and on, and Garden open. `None`
/// when the plugins aren't built.
fn opened() -> Option<Opened> {
    let tasks = plugins::built("tasks_plugin")?;
    let projects = plugins::built("projects_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let write = |path: &str, text: &str| {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("projects/Garden/Garden.md", PROJECT);
    write("projects/Garden/Soil.md", NOTE);
    write(
        "tasks/Water.md",
        &task("a", "status: todo\npriority: medium\nkanban_order: a0\n"),
    );
    write(
        "tasks/Weed.md",
        &task("b", "status: todo\npriority: high\nkanban_order: a1\n"),
    );
    write(
        "tasks/Rake.md",
        &task("c", "status: in-progress\npriority: medium\n"),
    );
    let tasks_parent = tempfile::tempdir().unwrap();
    let projects_parent = tempfile::tempdir().unwrap();
    let tasks_folder = plugins::folder(
        tasks_parent.path(),
        TASKS_MANIFEST,
        "tasks_plugin.wasm",
        &tasks,
    );
    let projects_folder = plugins::folder(
        projects_parent.path(),
        PROJECTS_MANIFEST,
        "projects_plugin.wasm",
        &projects,
    );
    plugins::install_and_enable(app_data.path(), &tasks_folder, "tasks");
    plugins::install_and_enable(app_data.path(), &projects_folder, "projects");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    harness
        .state_mut()
        .app_mut()
        .open_file("projects/Garden/Garden.md", app_core::OpenAs::Kept);
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Group, "Task board")
            .is_some()
    });
    Some(Opened {
        harness,
        folder,
        _folders: [app_data, parent, tasks_parent, projects_parent],
    })
}

/// Run frames, waiting in real time for the plugins' answers, until
/// `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done(harness) {
        assert!(
            std::time::Instant::now() < deadline,
            "the screen never got there; the active tab is {:?}",
            harness
                .state()
                .app()
                .active_tab()
                .map(|tab| tab.title().to_owned()),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.step();
    }
    run(harness);
}

fn board(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::Group, "Task board")
}

/// The labels of the cards in the column `column`, top to bottom.
fn cards(harness: &Ui, column: &str) -> Vec<String> {
    labels(
        board(harness)
            .get_by_role_and_label(Role::List, column)
            .get_all_by_role(Role::ListItem),
    )
}

fn card<'a>(harness: &'a Ui, label: &'a str) -> egui_kittest::Node<'a> {
    board(harness).get_by_role_and_label(Role::ListItem, label)
}

/// A frontmatter value of the task file at `path`.
fn property(folder: &Path, path: &str, key: &str) -> String {
    let text = std::fs::read_to_string(folder.join(path)).unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_project_opens_beside_the_projects_panel_with_its_labeled_sections() {
    let Some(Opened {
        harness, _folders, ..
    }) = opened()
    else {
        return;
    };

    harness.get_by_role_and_label(Role::Document, "Note text");
    let panel = harness.get_by_role_and_label(Role::Group, "Properties");
    panel.get_by_role_and_label(Role::Group, "Project");
    let status = panel.get_by_role_and_label(Role::RadioGroup, "Status");
    assert_eq!(
        labels(status.get_all_by_role(Role::RadioButton)),
        ["Planned", "Active", "On hold", "Completed", "Cancelled"]
    );
    panel.get_by_role_and_label(Role::Button, "Add task");
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    assert_eq!(
        labels(board(&harness).get_all_by_role(Role::List)),
        ["To do", "In progress", "Done", "Cancelled"]
    );
    assert_eq!(cards(&harness, "To do"), ["Water", "Weed"]);
    assert_eq!(cards(&harness, "In progress"), ["Rake"]);
    assert_eq!(
        card(&harness, "Weed").accesskit_node().description(),
        Some("High".into())
    );
    let tasks = panel.get_by_role_and_label(Role::ListBox, "Project tasks");
    assert_eq!(
        labels(tasks.get_all_by_role(Role::ListBoxOption)),
        ["Rake", "Water", "Weed"]
    );
    let notes = panel.get_by_role_and_label(Role::ListBox, "Project notes");
    assert_eq!(labels(notes.get_all_by_role(Role::ListBoxOption)), ["Soil"]);
    for section in ["Board", "Tasks", "This month", "Notes", "Files"] {
        panel.get_by_role_and_label(Role::Group, section);
    }
    panel.get_by_role_and_label(Role::Button, "Delete project");
}

/// Drag from `from` to `to` in small steps, as a pointer does, and drop.
fn drag(harness: &mut Ui, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    run(harness);
    harness.drag_at(from);
    run(harness);
    for step in 1..=4 {
        harness.hover_at(from + (to - from) * (step as f32 / 4.0));
        run(harness);
    }
    harness.drop_at(to);
    run(harness);
}

#[test]
fn dragging_a_card_up_its_column_writes_its_new_order() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let water = std::fs::read(folder.join("tasks/Water.md")).unwrap();

    let from = card(&harness, "Weed").rect().center();
    let to = card(&harness, "Water").rect().center_top() + vec2(0.0, 4.0);
    drag(&mut harness, from, to);

    wait_until(&mut harness, |harness| {
        cards(harness, "To do") == ["Weed", "Water"]
    });
    assert!(property(&folder, "tasks/Weed.md", "kanban_order").as_str() < "a0");
    assert_eq!(property(&folder, "tasks/Weed.md", "status"), "todo");
    assert_eq!(std::fs::read(folder.join("tasks/Water.md")).unwrap(), water);
}

#[test]
fn a_cards_menu_moves_it_to_another_column_from_the_keyboard() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    card(&harness, "Water").focus();
    run(&mut harness);

    press_with(&mut harness, Modifiers::SHIFT, Key::F10);

    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    let menu = harness.get_by_label("Move Water");
    assert_eq!(
        labels(menu.get_all_by_role(Role::Button)),
        [
            "Move up",
            "Move down",
            "To do",
            "In progress",
            "Done",
            "Cancelled"
        ]
    );
    assert_eq!(focused(&harness).as_deref(), Some("Move down"));
    press(&mut harness, Key::ArrowDown);
    press(&mut harness, Key::ArrowDown);
    assert_eq!(focused(&harness).as_deref(), Some("Done"));
    press(&mut harness, Key::Enter);

    wait_until(&mut harness, |harness| cards(harness, "Done") == ["Water"]);
    assert_eq!(property(&folder, "tasks/Water.md", "status"), "done");
    assert_eq!(cards(&harness, "To do"), ["Weed"]);
}
