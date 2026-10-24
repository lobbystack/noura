//! The Projects plugin. It registers the `project` object type with
//! today's statuses and adds the Projects page, which lists every project
//! with its status and task count and names new ones first. A project
//! opens in the note editor with its overview as the body and the
//! Projects panel beside it: the status, a board with a column per task
//! status, the project's tasks, this month's dated work, the notes and
//! files in the project's folder, and Delete project.
//!
//! Projects reads and changes tasks only through the Tasks plugin's
//! commands, which keep each card's place in its task's file. Other
//! plugins create and change projects through its `create` and `update`
//! commands, which [`commands`] documents.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `projects_plugin.wasm`.

mod commands;
mod dates;
mod order;
mod panel;
mod project;
mod tasks;

use std::{collections::BTreeMap, sync::Mutex};

use plugin_sdk::{
    Command, Error, Event, Node, ObjectType, Page, Plugin, View,
    navigation::{self, Placement},
    objects,
};
use serde_json::{Map, json};

use project::{PLANNED, PROJECT, Project, SCHEMA, status_label};
use tasks::Refused;

/// The Projects page.
const PAGE: &str = "projects";

/// What a page or a panel shows when a write met a file that changed
/// since.
const CHANGED_ELSEWHERE: &str = "It changed elsewhere. Showing the latest version.";

/// What the Projects page shows, between calls.
struct State {
    /// The name field for a new project is open.
    naming: bool,
    /// Why the last create failed, while the field stays open.
    naming_problem: Option<String>,
    /// The project the list shows selected.
    selected: Option<String>,
    /// What each project's panel remembers, by stable ID.
    panels: BTreeMap<String, panel::Remembered>,
}

static STATE: Mutex<State> = Mutex::new(State {
    naming: false,
    naming_problem: None,
    selected: None,
    panels: BTreeMap::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

struct Projects;

impl Plugin for Projects {
    fn object_types() -> Vec<ObjectType> {
        vec![ObjectType {
            name: PROJECT.into(),
            schema: SCHEMA.into(),
        }]
    }

    fn pages() -> Vec<Page> {
        vec![Page {
            id: PAGE.into(),
            title: "Projects".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        page()
    }

    fn handle_event(_page: String, event: Event) {
        handle(event);
    }

    fn render_panel(object: String) -> Vec<Node> {
        panel::render(&object)
    }

    fn handle_panel_event(object: String, event: Event) {
        panel::handle(&object, event);
    }

    fn commands() -> Vec<Command> {
        let offered = |id: &str, title: &str| Command {
            id: id.into(),
            title: title.into(),
            palette: false,
            plugins: true,
        };
        vec![
            offered("create", "Create a project"),
            offered("update", "Update a project"),
        ]
    }

    fn run_command(id: String, input: String) -> Result<String, String> {
        commands::run(&id, &input)
    }
}

/// Every project in the workspace, by title, then stable ID.
fn projects() -> Result<Vec<Project>, Error> {
    let mut projects: Vec<Project> = objects::list(PROJECT)?.iter().map(Project::of).collect();
    projects.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(projects)
}

/// `count` tasks, in words.
fn task_count(count: usize) -> String {
    match count {
        1 => "1 task".to_owned(),
        count => format!("{count} tasks"),
    }
}

/// The Projects page: New project, then every project with its status
/// and task count.
fn page() -> Vec<Node> {
    let mut view = View::panel("root", Some("Projects"));
    let projects = match projects() {
        Ok(projects) => projects,
        Err(error) => {
            view.text(
                View::ROOT,
                "problem",
                &format!("Noura couldn't list the projects: {}", reason(&error)),
            );
            return view.into_nodes();
        }
    };
    let state = state();
    view.button(View::ROOT, "new-project", "New project");
    if state.naming {
        view.name_field(View::ROOT, "new-project-name", "New project name", "");
        if let Some(problem) = &state.naming_problem {
            view.text(View::ROOT, "new-project-problem", problem);
        }
    }
    let counts = match tasks::list(None) {
        Ok(tasks) => {
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for task in tasks {
                if let Some(project) = task.properties.get("project").and_then(|p| p.as_str()) {
                    *counts.entry(project.to_owned()).or_default() += 1;
                }
            }
            Some(counts)
        }
        Err(Refused::Off) => {
            view.text(
                View::ROOT,
                "tasks-off",
                "Tasks is off, so the task counts don't show.",
            );
            None
        }
        Err(Refused::Changed | Refused::Other(_)) => None,
    };
    if projects.is_empty() {
        view.text(View::ROOT, "empty", "No projects yet");
        return view.into_nodes();
    }
    let list = view.list(View::ROOT, "projects", "Projects");
    for project in &projects {
        let status = status_label(&project.status);
        let detail = match &counts {
            Some(counts) => format!(
                "{status} · {}",
                task_count(counts.get(&project.id).copied().unwrap_or(0))
            ),
            None => status,
        };
        view.item(list, &project.id, &project.title, Some(&detail));
    }
    if let Some(selected) = state
        .selected
        .as_ref()
        .filter(|selected| projects.iter().any(|project| project.id == **selected))
    {
        view.select(list, selected);
    }
    view.into_nodes()
}

/// Open the name field for a new project.
fn start_naming() {
    let mut state = state();
    state.naming = true;
    state.naming_problem = None;
}

fn stop_naming() {
    let mut state = state();
    state.naming = false;
    state.naming_problem = None;
}

fn handle(event: Event) {
    match event {
        Event::Pressed(node) if node == "new-project" => start_naming(),
        // A name reaches the field's plugin once; another for a field
        // that closed names nothing.
        Event::Named(naming)
            if naming.node == "new-project-name"
                && !naming.name.trim().is_empty()
                && state().naming =>
        {
            create(naming.name.trim());
        }
        Event::Named(naming) if naming.node == "new-project-name" => stop_naming(),
        Event::Cancelled(node) if node == "new-project-name" => stop_naming(),
        Event::Selected(target) if target.node == "projects" => {
            state().selected = Some(target.item);
        }
        Event::Activated(target) if target.node == "projects" => {
            state().selected = Some(target.item.clone());
            let _ = navigation::open_object(&target.item, Placement::Preview);
        }
        _ => {}
    }
}

/// Create a project named `name` in a folder of its own, planned, and
/// open it. A failed create keeps the field open and says why.
fn create(name: &str) {
    let mut properties = Map::new();
    properties.insert("status".to_owned(), json!(PLANNED));
    let created = match commands::new_project(name, "", properties) {
        Ok(created) => created,
        Err(error) => {
            state().naming_problem = Some(format!(
                "Couldn't create \u{201c}{name}\u{201d}. {}",
                reason(&error)
            ));
            return;
        }
    };
    {
        let mut state = state();
        state.naming = false;
        state.naming_problem = None;
        state.selected = Some(created.id.clone());
    }
    let _ = navigation::open_object(&created.id, Placement::NewTab);
}

/// Why a call failed, in words.
fn reason(error: &Error) -> String {
    match error {
        Error::RevisionConflict(_) => CHANGED_ELSEWHERE.to_owned(),
        Error::InvalidProperty(problem) => format!(
            "{} doesn't fit the project fields: {}.",
            problem.property, problem.code
        ),
        Error::AlreadyExists(path) => format!("{path} already exists."),
        Error::NotFound(_) => "It isn't in the workspace anymore.".to_owned(),
        Error::Failed(message) => message.clone(),
        Error::CapabilityNotDeclared(capability) => {
            format!("Projects isn't allowed the {capability} capability.")
        }
        Error::IdentityConflict(_) => "More than one file holds this project's ID.".to_owned(),
        _ => "Noura couldn't finish it.".to_owned(),
    }
}

plugin_sdk::export!(Projects);
