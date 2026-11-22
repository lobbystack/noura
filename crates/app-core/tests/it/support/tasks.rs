//! The Tasks plugin in a running app, installed from the folder its crate
//! ships with the build that `crates/plugin-sdk/build-plugins.sh` makes,
//! and helpers that read its page, its sidebar section and its panels.

use app_core::{App, PluginEvent, Target, ViewKind, ViewNode};

use super::plugins::{self, PluginFolder};

/// The Tasks page.
pub const PAGE: &str = "plugin:tasks:tasks";

/// The Tasks plugin's manifest, as its crate ships it.
pub const MANIFEST: &str = include_str!("../../../../plugins/tasks/plugin.toml");

/// The ID suffixes the tests' tasks and projects use.
pub const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
pub const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";
pub const C: &str = "01jbcdefghjkmnpqrstvwxyz0c";
pub const D: &str = "01jbcdefghjkmnpqrstvwxyz0d";
pub const E: &str = "01jbcdefghjkmnpqrstvwxyz0e";
pub const F: &str = "01jbcdefghjkmnpqrstvwxyz0f";
pub const G: &str = "01jbcdefghjkmnpqrstvwxyz0g";
pub const H: &str = "01jbcdefghjkmnpqrstvwxyz0h";

/// The Tasks plugin's folder, or `None` when it isn't built.
pub fn folder() -> Option<PluginFolder> {
    plugins::built("tasks_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "tasks_plugin.wasm", &component))
}

/// The Tasks plugin installed and turned on in `app`'s workspace, with its
/// page drawn, or `None` when it isn't built. The folder lasts as long as
/// the guard.
pub fn with_tasks(app: &mut App) -> Option<PluginFolder> {
    let folder = folder()?;
    plugins::install_and_enable(app, folder.path(), "tasks");
    plugins::wait_until(app, "the Tasks page shows", |app| {
        app.plugin_view(PAGE).is_some()
    });
    if let Some(Err(problem)) = app.plugin_view(PAGE) {
        panic!("the Tasks page doesn't draw: {problem}");
    }
    Some(folder)
}

/// A task's file, as another app or an earlier session wrote it, with
/// `properties` as frontmatter lines and `updated` as its last change.
pub fn task_file(suffix: &str, properties: &str, updated: &str) -> Vec<u8> {
    format!(
        "---\nid: task_{suffix}\ntype: task\n{properties}created: 2026-10-01T10:00:00Z\n\
         updated: {updated}\n---\nNotes.\n"
    )
    .into_bytes()
}

/// A project's file, as the Projects plugin or the legacy app writes it.
pub fn project_file(suffix: &str) -> Vec<u8> {
    format!(
        "---\nid: project_{suffix}\ntype: project\nstatus: active\n\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nOverview.\n"
    )
    .into_bytes()
}

/// A task as a Tasks list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub checked: Option<bool>,
    pub warning: Option<String>,
}

/// The node with `id` under `node`.
pub fn find<'a>(node: &'a ViewNode, id: &str) -> Option<&'a ViewNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, id))
}

/// The page's view, which must draw.
pub fn page(app: &App) -> app_core::View {
    app.plugin_view(PAGE)
        .expect("the Tasks page shows")
        .expect("the Tasks page draws")
}

/// The page's task lists, one per section, top to bottom.
fn task_lists(view: &app_core::View) -> Vec<ViewNode> {
    fn collect(node: &ViewNode, lists: &mut Vec<ViewNode>) {
        if node.id.starts_with(LISTS) && matches!(node.kind, ViewKind::List { .. }) {
            lists.push(node.clone());
        }
        for child in &node.children {
            collect(child, lists);
        }
    }
    let mut lists = Vec::new();
    collect(view.root(), &mut lists);
    lists
}

/// What the node IDs of the page's task lists start with, before their
/// section, such as `tasks-overdue`.
pub const LISTS: &str = "tasks-";

/// The sections the page's list shows, top to bottom, each with its
/// tasks' labels.
pub fn sections(app: &App) -> Vec<(String, Vec<String>)> {
    task_lists(&page(app))
        .iter()
        .map(|list| {
            let labels = list
                .children
                .iter()
                .filter_map(|item| match &item.kind {
                    ViewKind::Item { label, .. } => Some(label.clone()),
                    _ => None,
                })
                .collect();
            (list.id[LISTS.len()..].to_owned(), labels)
        })
        .collect()
}

/// The tasks the page's lists show, top to bottom, across sections.
pub fn rows(app: &App) -> Vec<Row> {
    task_lists(&page(app))
        .iter()
        .flat_map(|list| list.children.iter())
        .filter_map(|item| match &item.kind {
            ViewKind::Item {
                label,
                detail,
                checked,
                warning,
                ..
            } => Some(Row {
                id: item.id.clone(),
                label: label.clone(),
                detail: detail.clone(),
                checked: *checked,
                warning: warning.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The labels of the tasks the page's list shows, top to bottom.
pub fn labels(app: &App) -> Vec<String> {
    rows(app).into_iter().map(|row| row.label).collect()
}

/// The task the page's lists show selected.
pub fn selected(app: &App) -> Option<String> {
    task_lists(&page(app))
        .iter()
        .find_map(|list| match &list.kind {
            ViewKind::List { selected, .. } => selected.clone(),
            _ => None,
        })
}

/// The text of the page's text node `id`, the title of its heading `id`
/// or that heading's detail when it has one, or the title of its empty
/// state `id`.
pub fn text(app: &App, id: &str) -> Option<String> {
    let view = page(app);
    match &find(view.root(), id)?.kind {
        ViewKind::Text(text) => Some(text.clone()),
        // A section's heading says how many tasks it holds.
        ViewKind::Heading {
            detail: Some(detail),
            ..
        } => Some(detail.clone()),
        // The page heading names the view.
        ViewKind::Heading { text, .. } => Some(text.clone()),
        // An empty state reads as its title.
        ViewKind::Empty { title, .. } => Some(title.clone()),
        _ => None,
    }
}

/// Whether the page's add field takes the keyboard when it draws next.
pub fn add_focused(app: &App) -> bool {
    let view = page(app);
    find(view.root(), ADD_TASK)
        .is_some_and(|node| matches!(node.kind, ViewKind::Editable { focus: true, .. }))
}

/// The field at the top of the list that adds a task on Return.
pub const ADD_TASK: &str = "add-task";

/// The page's sidebar section: each list's node ID, with its items' IDs,
/// labels and details, and the item it shows selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarList {
    pub id: String,
    pub items: Vec<(String, String, Option<String>)>,
    pub selected: Option<String>,
}

pub fn sidebar(app: &App) -> Vec<SidebarList> {
    let view = app
        .plugin_sidebar(PAGE)
        .expect("the Tasks page has a sidebar section")
        .expect("the sidebar section draws");
    let mut lists = Vec::new();
    collect_lists(view.root(), &mut lists);
    lists
}

fn collect_lists(node: &ViewNode, lists: &mut Vec<SidebarList>) {
    if let ViewKind::List { selected, .. } = &node.kind {
        lists.push(SidebarList {
            id: node.id.clone(),
            items: node
                .children
                .iter()
                .filter_map(|item| match &item.kind {
                    ViewKind::Item { label, detail, .. } => {
                        Some((item.id.clone(), label.clone(), detail.clone()))
                    }
                    _ => None,
                })
                .collect(),
            selected: selected.clone(),
        });
    }
    for child in &node.children {
        collect_lists(child, lists);
    }
}

/// Pick the sidebar item `item` of the list `list`, as one click or one
/// arrow key does, and wait for the plugin to draw.
pub fn choose(app: &mut App, list: &str, item: &str) {
    app.send_plugin_sidebar_event(
        PAGE,
        &PluginEvent::Selected(Target {
            node: list.to_owned(),
            item: item.to_owned(),
        }),
    );
    super::settle(app);
}

/// Open the page's filters, which stay hidden until you ask for them.
pub fn show_filters(app: &mut App) {
    send(app, PluginEvent::Pressed("toggle-filters".to_owned()));
}

/// Send `event` to the page and wait for the plugin to draw.
pub fn send(app: &mut App, event: PluginEvent) {
    app.send_plugin_event(PAGE, &event);
    super::settle(app);
}

/// Type `name` in the add field and press Return.
pub fn add_task(app: &mut App, name: &str) {
    send(
        app,
        PluginEvent::Changed(app_core::Field {
            node: ADD_TASK.to_owned(),
            value: app_core::InputValue::Text(name.to_owned()),
        }),
    );
}

/// The Inbox section the Tasks plugin adds.
pub const SECTION: &str = "due-today";

/// The Tasks plugin's Inbox section, while the plugin runs.
pub fn inbox_section(app: &App) -> Option<app_core::InboxSection> {
    app.inbox()
        .sections
        .into_iter()
        .find(|section| section.plugin_id == "tasks" && section.section_id == SECTION)
}

/// The Due today section's view, which must draw.
pub fn due_view(app: &App) -> app_core::View {
    inbox_section(app)
        .expect("the Inbox shows Due today")
        .content
        .expect("Due today draws")
}

/// The tasks Due today shows, top to bottom.
pub fn due_rows(app: &App) -> Vec<Row> {
    let view = due_view(app);
    let Some(list) = find(view.root(), "due") else {
        return Vec::new();
    };
    list.children
        .iter()
        .filter_map(|item| match &item.kind {
            ViewKind::Item {
                label,
                detail,
                checked,
                warning,
                ..
            } => Some(Row {
                id: item.id.clone(),
                label: label.clone(),
                detail: detail.clone(),
                checked: *checked,
                warning: warning.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The labels of the tasks Due today shows, top to bottom.
pub fn due_labels(app: &App) -> Vec<String> {
    due_rows(app).into_iter().map(|row| row.label).collect()
}

/// The text of the Due today section's text node `id`.
pub fn due_text(app: &App, id: &str) -> Option<String> {
    let view = due_view(app);
    match &find(view.root(), id)?.kind {
        ViewKind::Text(text) => Some(text.clone()),
        // The page heading names the view in its detail line.
        ViewKind::Heading {
            detail: Some(detail),
            ..
        } => Some(detail.clone()),
        // An empty state reads as its title.
        ViewKind::Empty { title, .. } => Some(title.clone()),
        _ => None,
    }
}

/// Send `event` to the Due today section and wait for the plugin to draw.
pub fn send_due(app: &mut App, event: PluginEvent) {
    app.send_inbox_event("tasks", SECTION, &event);
    super::settle(app);
}

/// The frontmatter value of `key` in a task file's bytes.
pub fn property(bytes: &[u8], key: &str) -> Option<serde_json::Value> {
    match local_core::parse_markdown("x.md", bytes) {
        local_core::ParsedMarkdown::Managed(object) => object.properties.get(key).cloned(),
        other => panic!("not a managed file: {other:?}"),
    }
}

/// What the Tasks page's right pane names its fields with, before the
/// field's own name.
pub const PANE: &str = "selected-";

/// Show the task `id` in the Tasks page's right pane, as a click on its row
/// in All does, where its fields edit it.
pub fn select_in_pane(app: &mut App, id: &str) {
    choose(app, "views", "view:all");
    send(
        app,
        PluginEvent::Selected(Target {
            node: format!("{LISTS}all"),
            item: id.to_owned(),
        }),
    );
}

/// The right pane's picker or date picker `field`: what it holds, and a
/// picker's options, once the pane shows it.
pub fn pane_input(app: &App, field: &str) -> Option<(app_core::InputValue, Vec<app_core::Choice>)> {
    let view = app.plugin_view(PAGE)?.ok()?;
    match &find(view.root(), &format!("{PANE}{field}"))?.kind {
        ViewKind::Picker {
            options, chosen, ..
        } => Some((
            app_core::InputValue::Choice(chosen.clone()),
            options
                .iter()
                .map(|option| app_core::Choice {
                    id: option.id.clone(),
                    label: option.label.clone(),
                })
                .collect(),
        )),
        ViewKind::DatePicker { value, .. } => {
            Some((app_core::InputValue::Date(value.clone()), Vec::new()))
        }
        ViewKind::Editable { value, .. } => {
            Some((app_core::InputValue::Text(value.clone()), Vec::new()))
        }
        _ => None,
    }
}

/// The right pane's text `node`, such as its notice.
pub fn pane_text(app: &App, node: &str) -> Option<String> {
    let view = app.plugin_view(PAGE)?.ok()?;
    match &find(view.root(), &format!("{PANE}{node}"))?.kind {
        ViewKind::Text(text) => Some(text.clone()),
        _ => None,
    }
}

/// Change the right pane's input `field` to `value`, as you do there.
pub fn pane_change(app: &mut App, field: &str, value: app_core::InputValue) {
    send(
        app,
        PluginEvent::Changed(app_core::Field {
            node: format!("{PANE}{field}"),
            value,
        }),
    );
}
