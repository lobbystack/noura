//! The real plugin pages as tabs, through AccessKit: Calendar, Tasks and
//! the Inbox share the preview tab, Open in new tab keeps one, closing the
//! last page tab reopens nothing, and a relaunch comes back on the plugin
//! page you left, drawn.

mod support;

use std::path::Path;

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    FakePicker, create_workspace, harness,
    kit::{Ui, labels, open_page, run},
    plugins, settle,
};

/// Each first-party plugin with a page: its ID, library and manifest.
const PLUGINS: [(&str, &str, &str); 3] = [
    (
        "tasks",
        "tasks_plugin",
        include_str!("../../plugins/tasks/plugin.toml"),
    ),
    (
        "calendar",
        "calendar_plugin",
        include_str!("../../plugins/calendar/plugin.toml"),
    ),
    (
        "projects",
        "projects_plugin",
        include_str!("../../plugins/projects/plugin.toml"),
    ),
];

struct Folders {
    app_data: tempfile::TempDir,
    _parent: tempfile::TempDir,
}

/// A workspace with Tasks, Calendar and Projects installed and on, or
/// `None` when they aren't built.
fn workspace() -> Option<Folders> {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
    for (id, library, manifest) in PLUGINS {
        let component = plugins::built(library)?;
        let place = parent.path().join(format!("{id}-plugin"));
        let folder = plugins::folder(&place, manifest, &format!("{library}.wasm"), &component);
        plugins::install_and_enable(app_data.path(), &folder, id);
    }
    Some(Folders {
        app_data,
        _parent: parent,
    })
}

fn launch(app_data: &Path) -> Ui {
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    wait_until(&mut harness, "the plugins add their pages", |harness| {
        ["Tasks", "Calendar", "Projects"]
            .iter()
            .all(|page| rail_page(harness, page).is_some())
    });
    harness
}

fn rail_page<'a>(harness: &'a Ui, title: &'a str) -> Option<egui_kittest::Node<'a>> {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .query_by_role_and_label(Role::Link, title)
}

fn wait_until(harness: &mut Ui, what: &str, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done(harness) {
        assert!(
            std::time::Instant::now() < deadline,
            "never happened: {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.step();
    }
    run(harness);
}

fn tabs(harness: &Ui) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::TabList, "Tabs")
        .map(|list| labels(list.get_all_by_role(Role::Tab)))
        .unwrap_or_default()
}

fn selected(harness: &Ui) -> Option<String> {
    harness
        .query_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

/// What shows the page's own view has drawn.
fn page_drew(harness: &Ui, title: &str) -> bool {
    match title {
        "Tasks" => harness
            .query_by_role_and_label(Role::ListBox, "Views")
            .is_some(),
        "Calendar" => harness
            .query_by_role_and_label(Role::Button, "Today")
            .is_some(),
        "Projects" => harness
            .query_by_role_and_label(Role::Button, "New project")
            .is_some(),
        other => panic!("no check for {other}"),
    }
}

fn close(harness: &mut Ui, title: &str) {
    harness
        .get_by_role_and_label(Role::Button, &format!("Close {title}"))
        .click();
    run(harness);
}

#[test]
fn app_pages_open_as_tabs_and_open_in_new_tab_keeps_one() {
    let Some(folders) = workspace() else {
        return;
    };
    let mut harness = launch(folders.app_data.path());

    open_page(&mut harness, "Calendar");
    assert_eq!(selected(&harness).as_deref(), Some("Calendar (preview)"));
    wait_until(&mut harness, "Calendar draws", |harness| {
        page_drew(harness, "Calendar")
    });

    // A plain click shows the page in the preview tab, which the next page
    // replaces.
    open_page(&mut harness, "Tasks");
    assert_eq!(tabs(&harness), ["Tasks (preview)"]);
    wait_until(&mut harness, "Tasks draws", |harness| {
        page_drew(harness, "Tasks")
    });

    rail_page(&harness, "Calendar").unwrap().click_secondary();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Open in new tab")
        .click();
    run(&mut harness);
    assert_eq!(tabs(&harness), ["Tasks (preview)", "Calendar"]);
    assert_eq!(selected(&harness).as_deref(), Some("Calendar"));

    open_page(&mut harness, "Inbox");
    assert_eq!(tabs(&harness), ["Inbox (preview)", "Calendar"]);

    // Closing the last page tab doesn't open it again.
    close(&mut harness, "Inbox");
    close(&mut harness, "Calendar");
    assert!(tabs(&harness).is_empty());
    run(&mut harness);
    assert!(tabs(&harness).is_empty());
}

/// A reload of a page in the legacy app is a quit and a relaunch here.
#[test]
fn a_relaunch_stays_on_a_plugin_page() {
    let Some(folders) = workspace() else {
        return;
    };
    for title in ["Tasks", "Calendar", "Projects"] {
        let mut harness = launch(folders.app_data.path());
        open_page(&mut harness, title);
        wait_until(&mut harness, "the page draws", |harness| {
            page_drew(harness, title)
        });
        drop(harness);

        let mut harness = launch(folders.app_data.path());

        wait_until(&mut harness, "the page comes back", |harness| {
            page_drew(harness, title)
        });
        assert_eq!(
            selected(&harness).as_deref(),
            Some(format!("{title} (preview)").as_str())
        );
        assert_eq!(tabs(&harness).len(), 1);
    }
}

/// The top of the file tree's Files heading.
fn files_heading_top(harness: &Ui) -> f32 {
    harness
        .get_by_role_and_label(Role::Button, "Files")
        .rect()
        .top()
}

#[test]
fn a_page_without_a_sidebar_section_leaves_the_file_tree_where_it_was() {
    let Some(folders) = workspace() else {
        return;
    };
    let mut harness = launch(folders.app_data.path());
    let top = files_heading_top(&harness);

    open_page(&mut harness, "Calendar");
    wait_until(&mut harness, "Calendar draws", |harness| {
        page_drew(harness, "Calendar")
    });

    assert_eq!(files_heading_top(&harness), top);
}

/// The rect of the button labelled `label`.
fn button_rect(harness: &Ui, label: &str) -> egui::Rect {
    harness.get_by_role_and_label(Role::Button, label).rect()
}

/// Each page's header is the same header: its buttons sit on one line at
/// one height, and a control drawn inside the header's row adds no gap of
/// its own.
#[test]
fn every_page_header_puts_its_buttons_on_the_same_line() {
    let Some(folders) = workspace() else {
        return;
    };
    let mut harness = launch(folders.app_data.path());
    let mut tops = Vec::new();
    for (page, button) in [
        ("Tasks", "New task"),
        ("Calendar", "Today"),
        ("Projects", "New project"),
    ] {
        open_page(&mut harness, page);
        wait_until(&mut harness, "the page draws", |harness| {
            page_drew(harness, page)
        });
        let rect = button_rect(&harness, button);
        tops.push((page, rect.top(), rect.height()));
        if page == "Calendar" {
            // Month, Week and Day sit as far from the arrows as the arrows
            // from Today.
            let day = harness
                .get_by_role_and_label(Role::RadioButton, "Day")
                .rect();
            let previous = button_rect(&harness, "Previous month");
            assert_eq!(
                previous.left() - day.right(),
                rect.left() - previous.right()
            );
        }
    }
    assert!(
        tops.iter()
            .all(|(_, top, height)| (*top, *height) == (tops[0].1, tops[0].2)),
        "{tops:?}"
    );
}

/// The rail lists Noura's own pages in the legacy order, whichever plugin
/// started first.
#[test]
fn the_rail_lists_noura_s_pages_in_one_order() {
    let Some(folders) = workspace() else {
        return;
    };
    let harness = launch(folders.app_data.path());
    let pages = labels(
        harness
            .get_by_role_and_label(Role::Navigation, "Pages")
            .query_all_by_role(Role::Link),
    );
    assert_eq!(pages, ["Inbox", "Files", "Tasks", "Calendar", "Projects"]);
}

/// The Inbox's sections share one column: what one section draws at its
/// end lines up with the others'.
#[test]
fn the_inbox_sections_end_on_one_edge() {
    let Some(folders) = workspace() else {
        return;
    };
    let mut harness = launch(folders.app_data.path());
    open_page(&mut harness, "Inbox");
    wait_until(&mut harness, "both sections draw", |harness| {
        harness
            .query_by_role_and_label(Role::Button, "All tasks")
            .is_some()
            && harness
                .query_by_role_and_label(Role::Button, "Calendar")
                .is_some()
    });
    let ends: Vec<f32> = ["Add", "All tasks", "Calendar"]
        .iter()
        .map(|label| button_rect(&harness, label).right())
        .collect();
    assert!(ends.iter().all(|end| *end == ends[0]), "{ends:?}");
}
