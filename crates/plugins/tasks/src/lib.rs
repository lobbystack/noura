//! The Tasks plugin. It registers the `task` object type with today's v1
//! fields, and adds the Tasks page: the views Today, Next 7 days, All and
//! Completed and one per project and per folder in the sidebar, with how
//! many open tasks each holds; filters; a field that adds a task as you
//! press Return; the list grouped into Overdue, Today, Upcoming, No date
//! and Completed, each task with a menu to open, rename, check off or
//! trash it; and the selected task beside the list, edited in place: its
//! due date, status, priority, title, description, subtasks and project.
//! The Inbox's Due today section lists what is due by tonight and adds a
//! task due today in one step. Other plugins list and change tasks through
//! its `create`, `update`, `complete`, `reopen` and `list` commands, which
//! [`commands`] documents, and AI assistants through its `list`, `create`,
//! `update` and `complete` tools, which [`tools`] lists.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `tasks_plugin.wasm`.

mod body;
mod commands;
mod details;
mod inbox;
mod links;
mod menus;
mod task;
mod tools;
mod views;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use jiff::civil::Date;
use plugin_sdk::{
    Command, EditableStyle, Error, Event, Heading, HeadingLevel, Icon, InboxSection, Node,
    NodeKind, ObjectType, Page, Plugin, Tool, Value, View, navigation,
    objects::{self, Changes, NewObject},
    workspace,
};
use serde_json::json;

use task::{PRIORITIES, SCHEMA, STATUSES, TASK, Task, label};
use views::{Choice, Filters, Now, ProjectFilter, Section, VIEWS};

/// The Tasks page.
const PAGE: &str = "tasks";

/// What a list shows when a write met a file that changed since.
const CHANGED_ELSEWHERE: &str = "It changed elsewhere. Showing the latest version.";

/// The most tasks one list draws. A view holds at most 10,000 nodes, and
/// each task's row takes nine with its menu.
const MOST_SHOWN: usize = 1_000;

/// The field at the top of the list that adds a task on Return.
const ADD_TASK: &str = "add-task";

/// What the page and the panels show, between calls.
struct State {
    choice: Choice,
    filters: Filters,
    /// The add field takes the keyboard the next time the page draws, as
    /// after New task.
    focus_add: bool,
    /// Why the last task the add field named couldn't be created.
    create_problem: Option<String>,
    /// What the page says about the last change from a list.
    notice: Option<String>,
    /// The task the list shows selected, whose details show beside it.
    selected: Option<String>,
    /// The selected task's title takes the keyboard the next time the
    /// page draws, as after Return on a task in the list.
    focus_title: Option<String>,
    /// The revision of each task as the list drew it, which a check
    /// writes against.
    listed: BTreeMap<String, String>,
    /// The revision of each task as its details drew it.
    panels: BTreeMap<String, String>,
    /// What each task's details say about its last change.
    panel_notices: BTreeMap<String, String>,
    /// The revision of each task as the Inbox's Due today drew it.
    due_listed: BTreeMap<String, String>,
    /// What Due today says about the last change from it.
    due_notice: Option<String>,
    /// The task whose row shows the rename field.
    renaming: Option<String>,
    /// Why the last rename didn't go in, while the field stays open.
    rename_problem: Option<String>,
    /// The subtask of the selected task that shows the rename field.
    renaming_subtask: Option<usize>,
    /// How many tasks Due today's field added, which names its next field.
    added: u32,
    /// Why the last task the field added couldn't be created.
    add_problem: Option<String>,
    /// The filters show under the heading.
    filters_shown: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    choice: Choice::Today,
    filters: Filters::NONE,
    focus_add: false,
    create_problem: None,
    notice: None,
    selected: None,
    focus_title: None,
    listed: BTreeMap::new(),
    panels: BTreeMap::new(),
    panel_notices: BTreeMap::new(),
    due_listed: BTreeMap::new(),
    due_notice: None,
    renaming: None,
    rename_problem: None,
    renaming_subtask: None,
    added: 0,
    add_problem: None,
    filters_shown: false,
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
            offered("list", "List tasks"),
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
            start_adding();
            navigation::open_page(PAGE).map_err(|error| reason(&error))?;
            return Ok(String::new());
        }
        commands::run(&id, &input)
    }

    fn tools() -> Vec<Tool> {
        tools::tools()
    }

    fn call_tool(name: String, arguments: String) -> Result<String, String> {
        tools::call(&name, &arguments)
    }
}

/// Every task, read from its file.
fn tasks() -> Result<Vec<Task>, Error> {
    let mut resolver = links::Resolver::default();
    Ok(objects::list(TASK)?
        .iter()
        .map(|object| Task::read(object, &mut resolver))
        .collect())
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

/// The button that shows and hides the filters.
const FILTERS_TOGGLE: &str = "toggle-filters";

/// How wide the list pane is. The details take the rest.
const LIST_WIDTH: u32 = 440;

/// The Tasks page: its heading with the view's name, New task and the
/// filters' button, the filters while they show, then the list with its
/// add field beside the selected task.
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
    let name = view_name(&state.choice, &projects);
    let heading = view.page_heading(View::ROOT, "view", &name, None, Some(Icon::Tasks));
    view.button(heading, "new-task", "New task");
    // Filters that narrow the view stay in sight.
    let shown = state.filters_shown || state.filters.active();
    let toggle = if shown { "Hide filters" } else { "Filters" };
    view.button(heading, FILTERS_TOGGLE, toggle);
    if let Some(notice) = &state.notice {
        view.text(View::ROOT, "notice", notice);
    }
    if let Some(problem) = &state.rename_problem {
        view.text(View::ROOT, "rename-problem", problem);
    }
    if shown {
        filters(&mut view, &state.filters, &projects, &folders(&all));
    }
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
    let (list_pane, detail_pane) = view.split(View::ROOT, "panes", LIST_WIDTH);
    let focus = std::mem::take(&mut state.focus_add);
    view.editable(
        list_pane,
        ADD_TASK,
        "Add task",
        "",
        EditableStyle::Add,
        focus,
    );
    if let Some(problem) = &state.create_problem {
        view.text(list_pane, "add-problem", problem);
    }
    let selected = state.selected.clone().filter(|selected| {
        shown
            .iter()
            .take(MOST_SHOWN)
            .any(|task| task.id == *selected)
    });
    if selected.is_none() {
        state.renaming_subtask = None;
    }
    match &selected {
        Some(id) => details::render(&mut view, detail_pane, id, &projects, &mut state),
        None => {
            view.empty_state(
                detail_pane,
                "no-selection",
                "No task selected",
                None,
                Some(Icon::Tasks),
            );
        }
    }
    state.focus_title = None;
    if shown.is_empty() {
        view.empty_state(list_pane, "empty", "No tasks here", None, Some(Icon::Tasks));
        return view.into_nodes();
    }
    let in_project = match &state.choice {
        Choice::Project(id) => Some(id.as_str()),
        _ => None,
    };
    let mut sections: BTreeMap<Section, Vec<&Task>> = BTreeMap::new();
    for task in shown.iter().take(MOST_SHOWN) {
        sections
            .entry(views::section(task, &now))
            .or_default()
            .push(task);
    }
    for section in Section::ALL {
        let Some(tasks) = sections.get(&section) else {
            continue;
        };
        // A section alone and named like the view, as Today in Today with
        // nothing overdue, needs no title.
        if sections.len() > 1 || section.title() != name {
            view.add(
                list_pane,
                &format!("section-{}", section.key()),
                NodeKind::Heading(Heading {
                    text: section.title().to_owned(),
                    level: HeadingLevel::Section,
                    detail: Some(tasks.len().to_string()),
                    icon: None,
                }),
            );
        }
        let list = view.list(
            list_pane,
            &format!("{LIST_PREFIX}{}", section.key()),
            section.title(),
        );
        for task in tasks {
            let (detail, overdue) = detail(task, &now, &projects, in_project);
            let item =
                view.checkbox_item(list, &task.id, &task.title, detail.as_deref(), task.done());
            if let Some(overdue) = overdue {
                view.warn(item, &overdue);
            }
            let renaming = state.renaming.as_deref() == Some(task.id.as_str());
            menus::add(&mut view, item, task, renaming);
        }
        if let Some(selected) = selected
            .as_deref()
            .filter(|selected| tasks.iter().any(|task| task.id == *selected))
        {
            view.select(list, selected);
        }
    }
    if shown.len() > MOST_SHOWN {
        view.text(
            list_pane,
            "more",
            &format!("Showing the first {MOST_SHOWN} tasks. Narrow the list with the filters."),
        );
    }
    view.into_nodes()
}

/// What the node IDs of the page's task lists start with, before their
/// section's key.
const LIST_PREFIX: &str = "tasks-";

/// A task's quieter detail: its project unless the list is that
/// project's, its due date unless it's overdue, and its priority unless
/// it is medium. An overdue task's due date comes second, to show as a
/// warning.
fn detail(
    task: &Task,
    now: &Now,
    projects: &[(String, String)],
    in_project: Option<&str>,
) -> (Option<String>, Option<String>) {
    let mut parts = Vec::new();
    if let Some(project) = task
        .project
        .as_deref()
        .filter(|project| Some(*project) != in_project)
        && let Some((_, title)) = projects.iter().find(|(id, _)| id == project)
    {
        parts.push(title.clone());
    }
    let overdue = !task.closed() && views::overdue(task, now);
    if let Some(due) = task.due
        && !overdue
    {
        parts.push(views::due_label(due, now));
    }
    if task.priority != "medium" {
        parts.push(label(&PRIORITIES, &task.priority));
    }
    let detail = (!parts.is_empty()).then(|| parts.join(" \u{b7} "));
    let warning = task
        .due
        .filter(|_| overdue)
        .map(|due| views::due_label(due, now));
    (detail, warning)
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
/// folder that holds tasks, each with how many open tasks it holds.
fn sidebar() -> Vec<Node> {
    let now = Now::read();
    let projects = projects();
    let tasks = tasks().unwrap_or_default();
    let folders = folders(&tasks);
    let state = state();
    let open = |choice: &Choice| {
        tasks
            .iter()
            .filter(|task| !task.closed() && views::holds(choice, task, &now))
            .count()
    };
    let count = |count: usize| (count > 0).then(|| count.to_string());
    let mut view = View::panel("task-views", Some("Task views"));
    let views = view.list(View::ROOT, "views", "Views");
    for (choice, id, name) in &VIEWS {
        let shown = match choice {
            Choice::Completed => None,
            choice => count(open(choice)),
        };
        view.item(views, id, name, shown.as_deref());
        if *choice == state.choice {
            view.select(views, id);
        }
    }
    if !projects.is_empty() {
        view.text(View::ROOT, "projects-heading", "Projects");
        let list = view.list(View::ROOT, "projects", "Projects");
        for (id, title) in &projects {
            let node = format!("project:{id}");
            let shown = count(open(&Choice::Project(id.clone())));
            view.item(list, &node, title, shown.as_deref());
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
        for (folder, _) in &listed {
            let node = format!("folder:{folder}");
            let shown = count(open(&Choice::Folder(folder.clone())));
            view.item(list, &node, folder_name(folder), shown.as_deref());
            if current_folder.as_deref() == Some(folder.as_str()) {
                view.select(list, &node);
            }
        }
    }
    view.into_nodes()
}

/// Give the add field the keyboard.
fn start_adding() {
    let mut state = state();
    state.focus_add = true;
    state.create_problem = None;
}

fn handle(event: Event) {
    let selected = state().selected.clone();
    if let Some(id) = &selected
        && details::handle(id, &event)
    {
        return;
    }
    match event {
        Event::Pressed(node) if node == "new-task" => start_adding(),
        Event::Pressed(node) if node == FILTERS_TOGGLE => {
            let mut state = state();
            if state.filters_shown || state.filters.active() {
                state.filters_shown = false;
                state.filters = Filters::NONE;
            } else {
                state.filters_shown = true;
            }
        }
        Event::Changed(field) if field.node == ADD_TASK => {
            if let Value::Text(name) = field.value
                && !name.trim().is_empty()
            {
                create(name.trim());
            }
        }
        Event::Named(naming) if naming.node == menus::RENAME => menus::rename(&naming.name),
        Event::Cancelled(node) if node == menus::RENAME => menus::stop_renaming(),
        Event::Chosen(node) => menus::choose(&node),
        Event::Selected(target) | Event::Activated(target)
            if !target.node.starts_with(LIST_PREFIX) =>
        {
            choose(&target.item);
        }
        Event::Selected(target) => select(target.item),
        // Return on a task edits its title.
        Event::Activated(target) => {
            let id = target.item;
            select(id.clone());
            state().focus_title = Some(id);
        }
        Event::Checked(check) if check.node.starts_with(LIST_PREFIX) => {
            complete(&check.item, check.checked);
        }
        Event::Changed(field) => filter(&field.node, field.value),
        _ => {}
    }
}

/// Show the task `id` in the details.
fn select(id: String) {
    let mut state = state();
    if state.selected.as_deref() != Some(id.as_str()) {
        state.renaming_subtask = None;
    }
    state.selected = Some(id);
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

/// Create a task named `name` from the add field, in the list it was
/// named in: in a project view it joins the project, and in Today or Next
/// 7 days it's due today. It shows selected, with its details beside the
/// list, and the list shows All when the view or the filters would hide
/// it. The page stays where it is. A failed create says why under the
/// field.
fn create(name: &str) {
    let now = Now::read();
    let choice = state().choice.clone();
    let mut properties = serde_json::Map::new();
    if let Choice::Project(id) = &choice {
        match links::link_for(id) {
            Ok(link) => {
                properties.insert("project".to_owned(), json!(link));
            }
            Err(error) => {
                state().create_problem = Some(format!(
                    "Couldn't create \u{201c}{name}\u{201d}. {}",
                    reason(&error)
                ));
                return;
            }
        }
    }
    if matches!(choice, Choice::Today | Choice::Week) {
        properties.insert("due".to_owned(), json!(now.today.to_string()));
    }
    let created = objects::create(&NewObject {
        object_type: TASK.to_owned(),
        title: name.to_owned(),
        path: None,
        body: String::new(),
        properties: serde_json::Value::Object(properties).to_string(),
    });
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            state().create_problem = Some(format!(
                "Couldn't create \u{201c}{name}\u{201d}. {}",
                reason(&error)
            ));
            return;
        }
    };
    let task = Task::of(&created);
    let mut state = state();
    state.create_problem = None;
    state.notice = None;
    if !views::holds(&state.choice, &task, &now) {
        state.choice = Choice::All;
    }
    if !state.filters.keep(&task, &now) {
        state.filters = Filters::NONE;
    }
    state.renaming_subtask = None;
    state.selected = Some(task.id);
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

/// A task's properties panel. A task opened in the note editor shows no
/// panel beside its text, as in the legacy app: its fields are in the
/// Tasks page's details.
fn panel(_id: &str) -> Vec<Node> {
    View::panel("task", Some("Task")).into_nodes()
}

/// A task's properties panel takes no events.
fn panel_event(_id: &str, _event: Event) {}

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
