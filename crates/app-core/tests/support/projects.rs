//! The Projects plugin in a running app, installed from the folder its
//! crate ships with the build that `crates/plugin-sdk/build-plugins.sh`
//! makes, and helpers that read its page and a project's panel.

use app_core::{App, PluginEvent, PluginNaming, TabId, ViewKind, ViewNode};

use super::{
    plugins::{self, PluginFolder},
    tasks::find,
};

/// The Projects page.
pub const PAGE: &str = "plugin:projects:projects";

/// The Projects plugin's manifest, as its crate ships it.
pub const MANIFEST: &str = include_str!("../../../plugins/projects/plugin.toml");

/// The Projects plugin's folder, or `None` when it isn't built.
pub fn folder() -> Option<PluginFolder> {
    plugins::built("projects_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "projects_plugin.wasm", &component))
}

/// The Projects plugin installed and turned on in `app`'s workspace, with
/// its page drawn, or `None` when it isn't built. The folder lasts as long
/// as the guard.
pub fn with_projects(app: &mut App) -> Option<PluginFolder> {
    let folder = folder()?;
    plugins::install_and_enable(app, folder.path(), "projects");
    plugins::wait_until(app, "the Projects page shows", |app| {
        app.plugin_view(PAGE).is_some()
    });
    if let Some(Err(problem)) = app.plugin_view(PAGE) {
        panic!("the Projects page doesn't draw: {problem}");
    }
    Some(folder)
}

/// A project's file with `properties` as frontmatter lines and `body`.
pub fn project_file(suffix: &str, properties: &str, body: &str) -> Vec<u8> {
    format!(
        "---\nid: project_{suffix}\ntype: project\n{properties}created: 2026-10-01T10:00:00Z\n\
         updated: 2026-10-01T10:00:00Z\n---\n{body}"
    )
    .into_bytes()
}

/// The page's view, which must draw.
pub fn page(app: &App) -> app_core::View {
    app.plugin_view(PAGE)
        .expect("the Projects page shows")
        .expect("the Projects page draws")
}

/// Each item of the list `list` under `root`: its node ID, label and
/// detail.
pub fn items(root: &ViewNode, list: &str) -> Vec<(String, String, Option<String>)> {
    let Some(list) = find(root, list) else {
        return Vec::new();
    };
    list.children
        .iter()
        .filter_map(|item| match &item.kind {
            ViewKind::Item { label, detail, .. } => {
                Some((item.id.clone(), label.clone(), detail.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The projects the page lists, as (label, detail).
pub fn listed(app: &App) -> Vec<(String, Option<String>)> {
    items(page(app).root(), "projects")
        .into_iter()
        .map(|(_, label, detail)| (label, detail))
        .collect()
}

/// The text of the text node `id` under `root`.
pub fn text_of(root: &ViewNode, id: &str) -> Option<String> {
    match &find(root, id)?.kind {
        ViewKind::Text(text) => Some(text.clone()),
        _ => None,
    }
}

/// Send `event` to the page and wait for the plugin to draw.
pub fn send(app: &mut App, event: PluginEvent) {
    app.send_plugin_event(PAGE, &event);
    super::settle(app);
}

/// Open the name field and name the new project `name`, as Enter does.
pub fn name_new_project(app: &mut App, name: &str) {
    app.press_plugin_button(PAGE, "new-project");
    super::settle(app);
    send(
        app,
        PluginEvent::Named(PluginNaming {
            node: "new-project-name".to_owned(),
            name: name.to_owned(),
        }),
    );
}

/// The panel the Projects plugin draws for the project open in `tab`,
/// once it shows one that holds `node`.
pub fn panel_with(app: &mut App, tab: TabId, node: &str) -> app_core::View {
    plugins::wait_until(app, &format!("the project's panel shows {node}"), |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| find(view.root(), node).is_some())
    });
    app.plugin_panel(tab).unwrap().unwrap()
}

/// The panel the Projects plugin draws for the project open in `tab` now.
pub fn panel(app: &App, tab: TabId) -> app_core::View {
    app.plugin_panel(tab)
        .expect("the project has a plugin panel")
        .expect("the panel draws")
}

/// Send `event` to the panel of the project open in `tab` and wait for the
/// plugin to draw.
pub fn send_panel(app: &mut App, tab: TabId, event: PluginEvent) {
    app.send_panel_event(tab, &event);
    super::settle(app);
}

/// Each column of the panel's board: its node ID, label and its cards'
/// labels, top to bottom.
pub fn board(app: &App, tab: TabId) -> Vec<(String, String, Vec<String>)> {
    let view = panel(app, tab);
    let Some(board) = find(view.root(), "board") else {
        return Vec::new();
    };
    board
        .children
        .iter()
        .filter_map(|column| {
            let ViewKind::BoardColumn { label } = &column.kind else {
                return None;
            };
            let cards = column
                .children
                .iter()
                .filter_map(|card| match &card.kind {
                    ViewKind::Card { label, .. } => Some(label.clone()),
                    _ => None,
                })
                .collect();
            Some((column.id.clone(), label.clone(), cards))
        })
        .collect()
}
