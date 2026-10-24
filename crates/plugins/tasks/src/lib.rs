//! The Tasks plugin. It registers the `task` object type with today's v1
//! fields, draws a task's properties panel beside its body in the note
//! editor, and adds the Tasks page: the views Today, Upcoming, All and
//! Completed and one per project and per folder in the sidebar, filters,
//! and a list of tasks you check off. New task names a task before its
//! file exists. Other plugins change tasks through its `create`,
//! `update`, `complete` and `reopen` commands, which [`commands`]
//! documents.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `tasks_plugin.wasm`.

mod commands;
mod inbox;
mod menus;
mod task;
mod views;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use jiff::civil::Date;
use plugin_sdk::{
    Command, Error, Event, InboxSection, Node, ObjectType, Page, Plugin, Value, View,
    navigation::{self, Placement},
    objects::{self, Changes, NewObject},
    workspace,
};
use serde_json::json;

use task::{PRIORITIES, SCHEMA, STATUSES, TASK, Task, label};
use views::{Choice, Filters, Now, ProjectFilter, VIEWS};

/// The Tasks page.
const PAGE: &str = "tasks";

/// What a list shows when a write met a file that changed since.
const CHANGED_ELSEWHERE: &str = "It changed elsewhere. Showing the latest version.";

/// The most tasks one list draws. A view holds at most 10,000 nodes, and
/// each task's row takes nine with its menu.
const MOST_SHOWN: usize = 1_000;

/// What the page and the panels show, between calls.
struct State {
    choice: Choice,
    filters: Filters,
    /// The name field for a new task is open.
    naming: bool,
    /// Why the last create failed, while the field stays open.
    naming_problem: Option<String>,
    /// What the page says about the last change from a list.
    notice: Option<String>,
    /// The task the list shows selected.
    selected: Option<String>,
    /// The revision of each task as the list drew it, which a check
    /// writes against.
    listed: BTreeMap<String, String>,
    /// The revision of each task as its panel drew it.
    panels: BTreeMap<String, String>,
    /// What each task's panel says about its last change.
    panel_notices: BTreeMap<String, String>,
    /// The revision of each task as the Inbox's Due today drew it.
    due_listed: BTreeMap<String, String>,
    /// What Due today says about the last change from it.
    due_notice: Option<String>,
    /// The task whose row shows the rename field.
    renaming: Option<String>,
    /// Why the last rename didn't go in, while the field stays open.
    rename_problem: Option<String>,
    /// How many tasks Due today's field added, which names its next field.
    added: u32,
    /// Why the last task the field added couldn't be created.
    add_problem: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    choice: Choice::Today,
    filters: Filters::NONE,
    naming: false,
    naming_problem: None,
    notice: None,
    selected: None,
    listed: BTreeMap::new(),
    panels: BTreeMap::new(),
    panel_notices: BTreeMap::new(),
    due_listed: BTreeMap::new(),
    due_notice: None,
    renaming: None,
    rename_problem: None,
    added: 0,
    add_problem: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

struct Tasks;

impl Plugin for Tasks {
    fn object_types() -> Vec<ObjectType> {
        vec![ObjectType {
            name: TASK.into(),
            schema: SCHEMA.into(),
        }]
    }

    fn pages() -> Vec<Page> {
        vec![Page {
            id: PAGE.into(),
            title: "Tasks".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        page()
    }

    fn sidebar(_page: String) -> Vec<Node> {
        sidebar()
    }

    fn handle_event(_page: String, event: Event) {
        handle(event);
    }

    fn render_panel(object: String) -> Vec<Node> {
        panel(&object)
    }

    fn handle_panel_event(object: String, event: Event) {
        panel_event(&object, event);
    }

    fn commands() -> Vec<Command> {
        let offered = |id: &str, title: &str| Command {
            id: id.into(),
            title: title.into(),
            palette: false,
            plugins: true,
        };
        vec![
            Command {
                id: "new-task".into(),
                title: "New task".into(),
                palette: true,
                plugins: false,
            },
            offered("create", "Create a task"),
            offered("update", "Update a task"),
            offered("complete", "Complete a task"),
            offered("reopen", "Reopen a task"),
        ]
    }

    fn inbox_sections() -> Vec<InboxSection> {
        inbox::sections()
    }

    fn render_inbox_section(_section: String) -> Vec<Node> {
        inbox::render()
    }

    fn handle_inbox_event(_section: String, event: Event) {
        inbox::handle(event);
    }

    fn run_command(id: String, input: String) -> Result<String, String> {
        if id == "new-task" {
            start_naming();
            navigation::open_page(PAGE).map_err(|error| reason(&error))?;
            return Ok(String::new());
        }
        commands::run(&id, &input)
    }
}

/// Every task, read from its file.
fn tasks() -> Result<Vec<Task>, Error> {
    Ok(objects::list(TASK)?.iter().map(Task::of).collect())
}

/// Every project in the workspace, as (stable ID, title), by title. Two
/// files with one ID, as after copying a project's folder outside Noura,
/// show as one project, under the title that sorts first.
fn projects() -> Vec<(String, String)> {
    let mut projects: Vec<(String, String)> = workspace::list_entries()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .filter(|entry| entry.object_type.as_deref() == Some("project"))
        .filter_map(|entry| {
            let id = entry.object_id?;
            let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            let title = name.strip_suffix(".md").unwrap_or(name).to_owned();
            Some((id, title))
        })
        .collect();
    projects.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut seen = BTreeSet::new();
    projects.retain(|(id, _)| seen.insert(id.clone()));
    projects
}

/// Each folder that holds tasks, with how many, sorted without case.
fn folders(tasks: &[Task]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for task in tasks {
        *counts.entry(task.folder().to_owned()).or_default() += 1;
    }
    let mut folders: Vec<(String, usize)> = counts.into_iter().collect();
    folders.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    folders
}

fn folder_name(folder: &str) -> &str {
    if folder.is_empty() {
        "Top level"
    } else {
        folder
    }
}

/// The name of the view `choice` shows.
fn view_name(choice: &Choice, projects: &[(String, String)]) -> String {
    match choice {
        Choice::Project(id) => projects
            .iter()
            .find(|(project, _)| project == id)
            .map_or_else(|| "Project".to_owned(), |(_, title)| title.clone()),
        Choice::Folder(folder) => folder_name(folder).to_owned(),
        other => VIEWS
            .iter()
            .find(|(choice, _, _)| choice == other)
            .map_or("Tasks", |(_, _, name)| name)
            .to_owned(),
    }
}

/// The Tasks page: the view's name, New task, the filters and the list.
fn page() -> Vec<Node> {
    let now = Now::read();
    let projects = projects();
    let mut view = View::panel("root", Some("Tasks"));
    let all = match tasks() {
        Ok(tasks) => tasks,
        Err(error) => {
            view.text(
                View::ROOT,
                "problem",
                &format!("Noura couldn't list the tasks: {}", reason(&error)),
            );
            return view.into_nodes();
        }
    };
    let mut state = state();
    view.text(View::ROOT, "view", &view_name(&state.choice, &projects));
    view.button(View::ROOT, "new-task", "New task");
    if state.naming {
        view.name_field(View::ROOT, "new-task-name", "New task name", "");
        if let Some(problem) = &state.naming_problem {
            view.text(View::ROOT, "new-task-problem", problem);
        }
    }
    if let Some(notice) = &state.notice {
        view.text(View::ROOT, "notice", notice);
    }
    if let Some(problem) = &state.rename_problem {
        view.text(View::ROOT, "rename-problem", problem);
    }
    filters(&mut view, &state.filters, &projects, &folders(&all));
    let mut shown: Vec<Task> = all
        .into_iter()
        .filter(|task| views::holds(&state.choice, task, &now) && state.filters.keep(task, &now))
        .collect();
    views::order(&mut shown, &now);
    state.listed = shown
        .iter()
        .map(|task| (task.id.clone(), task.revision.clone()))
        .collect();
    // A rename ends when its task leaves the list.
    if let Some(renaming) = &state.renaming
        && !shown
            .iter()
            .take(MOST_SHOWN)
            .any(|task| task.id == *renaming)
    {
        state.renaming = None;
        state.rename_problem = None;
    }
    if shown.is_empty() {
        view.text(View::ROOT, "empty", "No tasks here");
        return view.into_nodes();
    }
    let list = view.list(View::ROOT, "tasks", "Task list");
    let in_project = match &state.choice {
        Choice::Project(id) => Some(id.as_str()),
        _ => None,
    };
    for task in shown.iter().take(MOST_SHOWN) {
        let detail = detail(task, &now, &projects, in_project);
        let item = view.checkbox_item(list, &task.id, &task.title, detail.as_deref(), task.done());
        if views::overdue(task, &now) {
            view.warn(item, "Overdue");
        }
        let renaming = state.renaming.as_deref() == Some(task.id.as_str());
        menus::add(&mut view, item, task, renaming);
    }
    if let Some(selected) = state.selected.as_ref().filter(|selected| {
        shown
            .iter()
            .take(MOST_SHOWN)
            .any(|task| task.id == **selected)
    }) {
        view.select(list, selected);
    }
    if shown.len() > MOST_SHOWN {
        view.text(
            View::ROOT,
            "more",
            &format!("Showing the first {MOST_SHOWN} tasks. Narrow the list with the filters."),
        );
    }
    view.into_nodes()
}

/// A task's quieter detail: its due date, its project unless the list is
/// that project's, and its priority unless it is medium.
fn detail(
    task: &Task,
    now: &Now,
    projects: &[(String, String)],
    in_project: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(due) = task.due {
        parts.push(views::due_label(due, now));
    }
    if let Some(project) = task
        .project
        .as_deref()
        .filter(|project| Some(*project) != in_project)
        && let Some((_, title)) = projects.iter().find(|(id, _)| id == project)
    {
        parts.push(title.clone());
    }
    if task.priority != "medium" {
        parts.push(label(&PRIORITIES, &task.priority));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The filters for status, project, folder and due date.
fn filters(
    view: &mut View,
    filters: &Filters,
    projects: &[(String, String)],
    folders: &[(String, usize)],
) {
    let panel = view.add_panel(View::ROOT, "filters", Some("Filters"));
    let mut statuses = vec![("any".to_owned(), "Any status".to_owned())];
    statuses.extend(
        STATUSES
            .iter()
            .map(|(id, name)| ((*id).to_owned(), (*name).to_owned())),
    );
    let status = filters.status.clone().unwrap_or_else(|| "any".to_owned());
    choice(view, panel, "filter-status", "Status", statuses, &status);

    let mut project_choices = vec![
        ("any".to_owned(), "Any project".to_owned()),
        ("none".to_owned(), "No project".to_owned()),
    ];
    project_choices.extend(
        projects
            .iter()
            .map(|(id, title)| (format!("is:{id}"), title.clone())),
    );
    let project = match &filters.project {
        ProjectFilter::Any => "any".to_owned(),
        ProjectFilter::Without => "none".to_owned(),
        ProjectFilter::Is(id) => format!("is:{id}"),
    };
    choice(
        view,
        panel,
        "filter-project",
        "Project",
        project_choices,
        &project,
    );

    let mut folder_choices = vec![("any".to_owned(), "Any folder".to_owned())];
    folder_choices.extend(
        folders
            .iter()
            .map(|(folder, _)| (format!("in:{folder}"), folder_name(folder).to_owned())),
    );
    let folder = filters
        .folder
        .as_ref()
        .map_or_else(|| "any".to_owned(), |folder| format!("in:{folder}"));
    choice(
        view,
        panel,
        "filter-folder",
        "Folder",
        folder_choices,
        &folder,
    );

    view.input(
        panel,
        "filter-due",
        "Due on or before",
        Value::Date(filters.due_by.map(|day| day.to_string())),
    );
}

/// A choice input with `choices`, choosing `chosen`, which joins the
/// choices when they don't hold it, so the view stays valid.
fn choice(
    view: &mut View,
    parent: u32,
    id: &str,
    label: &str,
    mut choices: Vec<(String, String)>,
    chosen: &str,
) {
    if !choices.iter().any(|(choice, _)| choice == chosen) {
        choices.push((chosen.to_owned(), chosen.to_owned()));
    }
    let pairs: Vec<(&str, &str)> = choices
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str()))
        .collect();
    view.choice_input(parent, id, label, &pairs, Some(chosen));
}

/// The sidebar section: the views, then a view per project and per
/// folder that holds tasks.
fn sidebar() -> Vec<Node> {
    let projects = projects();
    let tasks = tasks().unwrap_or_default();
    let folders = folders(&tasks);
    let state = state();
    let mut view = View::panel("task-views", Some("Task views"));
    let views = view.list(View::ROOT, "views", "Views");
    for (choice, id, name) in &VIEWS {
        view.item(views, id, name, None);
        if *choice == state.choice {
            view.select(views, id);
        }
    }
    if !projects.is_empty() {
        view.text(View::ROOT, "projects-heading", "Projects");
        let list = view.list(View::ROOT, "projects", "Projects");
        for (id, title) in &projects {
            let node = format!("project:{id}");
            view.item(list, &node, title, None);
            if state.choice == Choice::Project(id.clone()) {
                view.select(list, &node);
            }
        }
    }
    let current_folder = match &state.choice {
        Choice::Folder(folder) => Some(folder.clone()),
        _ => None,
    };
    if !folders.is_empty() || current_folder.is_some() {
        view.text(View::ROOT, "folders-heading", "Folders");
        let list = view.list(View::ROOT, "folders", "Folders");
        let mut listed = folders;
        if let Some(folder) = &current_folder
            && !listed.iter().any(|(held, _)| held == folder)
        {
            listed.push((folder.clone(), 0));
        }
        for (folder, count) in &listed {
            let node = format!("folder:{folder}");
            view.item(list, &node, folder_name(folder), Some(&count.to_string()));
            if current_folder.as_deref() == Some(folder.as_str()) {
                view.select(list, &node);
            }
        }
    }
    view.into_nodes()
}

/// Open the name field for a new task.
fn start_naming() {
    let mut state = state();
    state.naming = true;
    state.naming_problem = None;
}

fn handle(event: Event) {
    match event {
        Event::Pressed(node) if node == "new-task" => start_naming(),
        // A name reaches the field's plugin once; another for a field
        // that closed names nothing.
        Event::Named(naming)
            if naming.node == "new-task-name"
                && !naming.name.trim().is_empty()
                && state().naming =>
        {
            create(naming.name.trim());
        }
        Event::Named(naming) if naming.node == "new-task-name" => {
            let mut state = state();
            state.naming = false;
            state.naming_problem = None;
        }
        Event::Cancelled(node) if node == "new-task-name" => {
            let mut state = state();
            state.naming = false;
            state.naming_problem = None;
        }
        Event::Named(naming) if naming.node == menus::RENAME => menus::rename(&naming.name),
        Event::Cancelled(node) if node == menus::RENAME => menus::stop_renaming(),
        Event::Chosen(node) => menus::choose(&node),
        Event::Selected(target) | Event::Activated(target) if target.node != "tasks" => {
            choose(&target.item);
        }
        Event::Selected(target) => state().selected = Some(target.item),
        Event::Activated(target) => {
            state().selected = Some(target.item.clone());
            if let Err(error) = navigation::open_object(&target.item, Placement::Preview) {
                state().notice = Some(format!("Couldn't open the task: {}", reason(&error)));
            }
        }
        Event::Checked(check) if check.node == "tasks" => complete(&check.item, check.checked),
        Event::Changed(field) => filter(&field.node, field.value),
        _ => {}
    }
}

/// Switch the list to the view a sidebar item names.
fn choose(item: &str) {
    let choice = if let Some(id) = item.strip_prefix("project:") {
        Choice::Project(id.to_owned())
    } else if let Some(folder) = item.strip_prefix("folder:") {
        Choice::Folder(folder.to_owned())
    } else if let Some((choice, _, _)) = VIEWS.iter().find(|(_, id, _)| *id == item) {
        choice.clone()
    } else {
        return;
    };
    let mut state = state();
    state.choice = choice;
    state.notice = None;
}

/// Change a filter from its input's new value.
fn filter(node: &str, value: Value) {
    let mut state = state();
    match (node, value) {
        ("filter-status", Value::Choice(chosen)) => {
            state.filters.status = chosen.filter(|chosen| chosen != "any");
        }
        ("filter-project", Value::Choice(chosen)) => {
            state.filters.project = match chosen.as_deref() {
                Some("none") => ProjectFilter::Without,
                Some(chosen) => chosen
                    .strip_prefix("is:")
                    .map_or(ProjectFilter::Any, |id| ProjectFilter::Is(id.to_owned())),
                None => ProjectFilter::Any,
            };
        }
        ("filter-folder", Value::Choice(chosen)) => {
            state.filters.folder = chosen
                .as_deref()
                .and_then(|chosen| chosen.strip_prefix("in:"))
                .map(str::to_owned);
        }
        ("filter-due", Value::Date(day)) => {
            state.filters.due_by = day.and_then(|day| day.parse::<Date>().ok());
        }
        _ => {}
    }
}

/// Create a task named `name`, in the project of the project view it
/// was named in. It opens selected, and the list shows All when the
/// view or the filters would hide it. A failed create keeps the field
/// open and says why.
fn create(name: &str) {
    let project = match &state().choice {
        Choice::Project(id) => Some(id.clone()),
        _ => None,
    };
    let properties = project.map_or_else(|| json!({}), |id| json!({ "project": id }));
    let created = objects::create(&NewObject {
        object_type: TASK.to_owned(),
        title: name.to_owned(),
        path: None,
        body: String::new(),
        properties: properties.to_string(),
    });
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            state().naming_problem = Some(format!(
                "Couldn't create \u{201c}{name}\u{201d}. {}",
                reason(&error)
            ));
            return;
        }
    };
    let task = Task::of(&created);
    let now = Now::read();
    {
        let mut state = state();
        state.naming = false;
        state.naming_problem = None;
        state.notice = None;
        if !views::holds(&state.choice, &task, &now) {
            state.choice = Choice::All;
        }
        if !state.filters.keep(&task, &now) {
            state.filters = Filters::NONE;
        }
        state.selected = Some(task.id.clone());
    }
    if let Err(error) = navigation::open_object(&task.id, Placement::NewTab) {
        state().notice = Some(format!("Couldn't open the new task: {}", reason(&error)));
    }
}

/// Mark the task `id` done, or back to todo, against the revision the
/// list drew it at.
fn complete(id: &str, done: bool) {
    let Some(revision) = state().listed.get(id).cloned() else {
        return;
    };
    state().notice = set_done(id, &revision, done);
}

/// Mark the task `id` done, or back to todo, against `revision`, and say
/// what the list should tell you: nothing when it worked.
fn set_done(id: &str, revision: &str, done: bool) -> Option<String> {
    let status = if done { "done" } else { "todo" };
    let written = objects::update(
        id,
        revision,
        &Changes {
            body: None,
            properties: json!({ "status": status }).to_string(),
            remove: Vec::new(),
        },
    );
    match written {
        Ok(_) => None,
        Err(Error::RevisionConflict(_)) => Some(CHANGED_ELSEWHERE.to_owned()),
        Err(error) => Some(format!("Couldn't change the task: {}", reason(&error))),
    }
}

/// A task's properties panel: status, priority, due date and project.
fn panel(id: &str) -> Vec<Node> {
    let mut view = View::panel("task", Some("Task"));
    let object = match objects::get(id) {
        Ok(object) => object,
        Err(error) => {
            view.text(
                View::ROOT,
                "problem",
                &format!("Noura couldn't read the task: {}", reason(&error)),
            );
            return view.into_nodes();
        }
    };
    let task = Task::of(&object);
    let mut state = state();
    state.panels.insert(task.id.clone(), task.revision.clone());
    if let Some(notice) = state.panel_notices.get(&task.id) {
        view.text(View::ROOT, "notice", notice);
    }
    drop(state);
    let pairs = |choices: &[(&str, &str)]| -> Vec<(String, String)> {
        choices
            .iter()
            .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
            .collect()
    };
    choice(
        &mut view,
        View::ROOT,
        "status",
        "Status",
        pairs(&STATUSES),
        &task.status,
    );
    choice(
        &mut view,
        View::ROOT,
        "priority",
        "Priority",
        pairs(&PRIORITIES),
        &task.priority,
    );
    let due = match task.due {
        Some(task::Due::At(_)) => Value::DateTime(due_text(&object.properties)),
        Some(task::Due::Day(day)) => Value::Date(Some(day.to_string())),
        None => Value::Date(None),
    };
    view.input(View::ROOT, "due", "Due", due);
    let mut projects: Vec<(String, String)> = vec![("none".to_owned(), "No project".to_owned())];
    projects.extend(self::projects());
    let chosen = task.project.clone().unwrap_or_else(|| "none".to_owned());
    if !projects.iter().any(|(id, _)| *id == chosen) {
        projects.push((chosen.clone(), "Unknown project".to_owned()));
    }
    choice(
        &mut view,
        View::ROOT,
        "project",
        "Project",
        projects,
        &chosen,
    );
    if let Some(problem) = &object.problem {
        view.text(
            View::ROOT,
            "schema-problem",
            &format!(
                "{} doesn't fit the task fields: {}",
                problem.property, problem.code
            ),
        );
    }
    view.into_nodes()
}

/// The task's due value as its file writes it.
fn due_text(properties: &str) -> Option<String> {
    let properties: serde_json::Value = serde_json::from_str(properties).ok()?;
    properties.get("due")?.as_str().map(str::to_owned)
}

/// A change in a task's panel, written against the revision the panel
/// drew.
fn panel_event(id: &str, event: Event) {
    let Event::Changed(field) = event else {
        return;
    };
    let (set, remove) = match (field.node.as_str(), field.value) {
        ("status", Value::Choice(Some(status))) => (json!({ "status": status }), Vec::new()),
        ("priority", Value::Choice(Some(priority))) => {
            (json!({ "priority": priority }), Vec::new())
        }
        ("due", Value::Date(Some(day)) | Value::DateTime(Some(day))) => {
            (json!({ "due": day }), Vec::new())
        }
        ("due", Value::Date(None) | Value::DateTime(None)) => (json!({}), vec!["due".to_owned()]),
        ("project", Value::Choice(Some(project))) if project == "none" => {
            (json!({}), vec!["project".to_owned()])
        }
        ("project", Value::Choice(Some(project))) => (json!({ "project": project }), Vec::new()),
        _ => return,
    };
    let Some(revision) = state().panels.get(id).cloned() else {
        return;
    };
    let written = objects::update(
        id,
        &revision,
        &Changes {
            body: None,
            properties: set.to_string(),
            remove,
        },
    );
    let mut state = state();
    match written {
        Ok(task) => {
            state.panels.insert(task.id.clone(), task.revision.clone());
            state.panel_notices.remove(id);
        }
        Err(Error::RevisionConflict(_)) => {
            state
                .panel_notices
                .insert(id.to_owned(), CHANGED_ELSEWHERE.to_owned());
        }
        Err(error) => {
            state.panel_notices.insert(
                id.to_owned(),
                format!("Couldn't change the task: {}", reason(&error)),
            );
        }
    }
}

/// Why a call failed, in words.
fn reason(error: &Error) -> String {
    match error {
        Error::RevisionConflict(_) => CHANGED_ELSEWHERE.to_owned(),
        Error::InvalidProperty(problem) => {
            format!(
                "{} doesn't fit the task fields: {}.",
                problem.property, problem.code
            )
        }
        Error::AlreadyExists(path) => format!("{path} already exists."),
        Error::NotFound(_) => "It isn't in the workspace anymore.".to_owned(),
        Error::Failed(message) => message.clone(),
        Error::CapabilityNotDeclared(capability) => {
            format!("Tasks isn't allowed the {capability} capability.")
        }
        other => format!("{other:?}"),
    }
}

plugin_sdk::export!(Tasks);
