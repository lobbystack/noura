//! The Tasks plugin in a running app, installed from the folder its crate
//! ships with the build that `crates/plugin-sdk/build-plugins.sh` makes,
//! and helpers that read its page, its sidebar section and its panels.

use app_core::{App, PluginEvent, PluginNaming, Target, ViewKind, ViewNode};

use super::plugins::{self, PluginFolder};

/// The Tasks page.
pub const PAGE: &str = "plugin:tasks:tasks";

/// The Tasks plugin's manifest, as its crate ships it.
pub const MANIFEST: &str = include_str!("../../../plugins/tasks/plugin.toml");

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

/// The tasks the page's list shows, top to bottom.
pub fn rows(app: &App) -> Vec<Row> {
    let view = page(app);
    let Some(list) = find(view.root(), "tasks") else {
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

/// The item the page's list shows selected.
pub fn selected(app: &App) -> Option<String> {
    let view = page(app);
    match &find(view.root(), "tasks")?.kind {
        ViewKind::List { selected, .. } => selected.clone(),
        _ => None,
    }
}

/// The text of the page's text node `id`.
pub fn text(app: &App, id: &str) -> Option<String> {
    let view = page(app);
    match &find(view.root(), id)?.kind {
        ViewKind::Text(text) => Some(text.clone()),
        _ => None,
    }
}

/// Whether the page shows the name field for a new task.
pub fn naming(app: &App) -> bool {
    let view = page(app);
    find(view.root(), "new-task-name")
        .is_some_and(|node| matches!(node.kind, ViewKind::NameField { .. }))
}

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

/// Send `event` to the page and wait for the plugin to draw.
pub fn send(app: &mut App, event: PluginEvent) {
    app.send_plugin_event(PAGE, &event);
    super::settle(app);
}

/// Open the name field and name the new task `name`, as Enter does.
pub fn name_new_task(app: &mut App, name: &str) {
    app.press_plugin_button(PAGE, "new-task");
    super::settle(app);
    send(
        app,
        PluginEvent::Named(PluginNaming {
            node: "new-task-name".to_owned(),
            name: name.to_owned(),
        }),
    );
}

/// The frontmatter value of `key` in a task file's bytes.
pub fn property(bytes: &[u8], key: &str) -> Option<serde_json::Value> {
    match local_core::parse_markdown("x.md", bytes) {
        local_core::ParsedMarkdown::Managed(object) => object.properties.get(key).cloned(),
        other => panic!("not a managed file: {other:?}"),
    }
}
