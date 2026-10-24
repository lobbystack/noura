//! A project's page: the panel Noura shows beside the project's overview
//! in the note editor. It holds the project's status, a board with a
//! column per task status, the project's tasks, this month's dated work,
//! the notes and files in the project's folder, and Delete project.
//!
//! Each move on the board writes one task, through the Tasks plugin's
//! `update` command, with the revision the board drew and an order key
//! between the card's new neighbors.

use plugin_sdk::{
    Error, Event, Node, Value, View,
    navigation::{self, Placement},
    objects::{self, Changes},
    workspace::{self, EntryKind},
};
use serde_json::json;

use crate::{
    CHANGED_ELSEWHERE,
    dates::{self, Now},
    order,
    project::{Project, STATUSES as PROJECT_STATUSES},
    reason, state,
    tasks::{self, Refused, STATUSES, Task, status_label},
};

/// What a task section says while the Tasks plugin is off.
const TASKS_OFF: &str = "Tasks is off. Turn it on in Settings to see this project's tasks.";

/// What the board says when a move met a task that changed since.
const MOVE_CHANGED: &str = "Could not move the task. The task changed while you were viewing it.";

/// What a project's panel remembers between calls.
#[derive(Debug, Default)]
pub(crate) struct Remembered {
    /// The project file's revision as the panel drew it, which a status
    /// change or a delete writes against.
    revision: String,
    /// Each board column's status and its cards as the board drew them,
    /// top to bottom.
    columns: Vec<(String, Vec<Card>)>,
    /// The name field for a new task is open.
    naming: bool,
    /// Why the last new task wasn't added, while the field stays open.
    naming_problem: Option<String>,
    /// What the panel says about the last change.
    notice: Option<String>,
    /// Delete project asks first.
    confirming: bool,
}

/// A card as the board drew it.
#[derive(Debug, Clone)]
struct Card {
    task: String,
    order: String,
    revision: String,
}

/// The panel of the project `id`.
pub(crate) fn render(id: &str) -> Vec<Node> {
    let mut view = View::panel("project", Some("Project"));
    let object = match objects::get(id) {
        Ok(object) => object,
        Err(error) => {
            view.text(
                View::ROOT,
                "problem",
                &format!("Noura couldn't read the project: {}", reason(&error)),
            );
            return view.into_nodes();
        }
    };
    let project = Project::of(&object);
    let now = Now::read();
    let listed = tasks::list(Some(&project.id));
    let mut state = state();
    let remembered = state.panels.entry(project.id.clone()).or_default();
    remembered.revision = project.revision.clone();
    if let Some(notice) = &remembered.notice {
        view.text(View::ROOT, "notice", notice);
    }
    status(&mut view, &project);
    if let Some(problem) = &object.problem {
        view.text(
            View::ROOT,
            "schema-problem",
            &format!(
                "{} doesn't fit the project fields: {}",
                problem.property, problem.code
            ),
        );
    }
    let section = section(&mut view, "board", "Board");
    match &listed {
        Ok(tasks) => board(&mut view, section, tasks, remembered, &now),
        Err(refused) => {
            remembered.columns.clear();
            view.text(section, "board-off", &unlisted(refused));
        }
    }
    drop(state);
    task_list(&mut view, &listed);
    this_month(&mut view, &project, &listed, &now);
    notes_and_files(&mut view, &project);
    delete_button(&mut view, &project.id);
    view.into_nodes()
}

/// What a task section says when Tasks didn't list the tasks.
fn unlisted(refused: &Refused) -> String {
    match refused {
        Refused::Off => TASKS_OFF.to_owned(),
        Refused::Changed => CHANGED_ELSEWHERE.to_owned(),
        Refused::Other(message) => format!("Noura couldn't list the tasks. {message}"),
    }
}

/// A section of the panel headed `title`.
fn section(view: &mut View, id: &str, title: &str) -> u32 {
    let section = view.add_panel(View::ROOT, &format!("{id}-section"), Some(title));
    view.text(section, &format!("{id}-heading"), title);
    section
}

/// The project's status, which you change in place.
fn status(view: &mut View, project: &Project) {
    let mut choices: Vec<(&str, &str)> = PROJECT_STATUSES.to_vec();
    if !choices.iter().any(|(id, _)| *id == project.status) {
        choices.push((project.status.as_str(), project.status.as_str()));
    }
    view.choice_input(
        View::ROOT,
        "status",
        "Status",
        &choices,
        Some(&project.status),
    );
}

/// The name people see for a task priority.
fn priority_label(value: &str) -> &str {
    match value {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "urgent" => "Urgent",
        other => other,
    }
}

/// A card's detail: when the task is due, and its priority unless it is
/// medium.
fn card_detail(task: &Task, now: &Now) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(due) = task
        .properties
        .get("due")
        .and_then(serde_json::Value::as_str)
        .and_then(|due| dates::when(due, now))
    {
        parts.push(format!("Due {due}"));
    }
    if task.priority != "medium" {
        parts.push(priority_label(&task.priority).to_owned());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The board: Add task, then a column per task status, each card at its
/// board order key, then by stable ID, with a menu that moves it.
fn board(view: &mut View, section: u32, tasks: &[Task], remembered: &mut Remembered, now: &Now) {
    view.button(section, "add-task", "Add task");
    if remembered.naming {
        view.name_field(section, "new-task-name", "New task name", "");
        if let Some(problem) = &remembered.naming_problem {
            view.text(section, "new-task-problem", problem);
        }
    }
    let board = view.board(section, "board", "Task board");
    remembered.columns.clear();
    for (status, label) in STATUSES {
        let column = view.board_column(board, &format!("column:{status}"), label);
        let mut cards: Vec<&Task> = tasks.iter().filter(|task| task.status == status).collect();
        cards.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        for (index, task) in cards.iter().enumerate() {
            let card = view.card(
                column,
                &format!("card:{}", task.id),
                &task.title,
                card_detail(task, now).as_deref(),
            );
            let menu = view.menu(
                card,
                &format!("card-menu:{}", task.id),
                &format!("Move {}", task.title),
            );
            view.menu_item(menu, &format!("move-up:{}", task.id), "Move up", index > 0);
            view.menu_item(
                menu,
                &format!("move-down:{}", task.id),
                "Move down",
                index + 1 < cards.len(),
            );
            view.separator(menu, &format!("card-gap:{}", task.id));
            for (other, other_label) in STATUSES {
                view.menu_item(
                    menu,
                    &format!("move-to:{other}:{}", task.id),
                    other_label,
                    other != status,
                );
            }
        }
        let drawn = cards
            .iter()
            .map(|task| Card {
                task: task.id.clone(),
                order: task.order.clone(),
                revision: task.revision.clone(),
            })
            .collect();
        remembered.columns.push((status.to_owned(), drawn));
    }
}

/// The project's tasks, by title, each with its status.
fn task_list(view: &mut View, listed: &Result<Vec<Task>, Refused>) {
    let section = section(view, "tasks", "Tasks");
    let tasks = match listed {
        Ok(tasks) => tasks,
        Err(refused) => {
            view.text(section, "tasks-off", &unlisted(refused));
            return;
        }
    };
    if tasks.is_empty() {
        view.text(section, "tasks-empty", "No tasks in this project yet");
        return;
    }
    let mut tasks: Vec<&Task> = tasks.iter().collect();
    tasks.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    let list = view.list(section, "tasks", "Project tasks");
    for task in tasks {
        view.item(
            list,
            &format!("task:{}", task.id),
            &task.title,
            Some(&status_label(&task.status)),
        );
    }
}

/// This month's dated work for the project and its tasks.
fn this_month(view: &mut View, project: &Project, listed: &Result<Vec<Task>, Refused>, now: &Now) {
    let section = section(view, "dated", "This month");
    let mut dated = dates::of(&project.id, &project.title, &project.properties);
    if let Ok(tasks) = listed {
        for task in tasks {
            dated.extend(dates::of(&task.id, &task.title, &task.properties));
        }
    }
    let dated = dates::this_month(dated, now);
    if dated.is_empty() {
        view.text(section, "dated-empty", "No dated work this month");
    } else {
        let list = view.list(section, "dated", "This month's dated work");
        for entry in &dated {
            view.item(
                list,
                &format!("dated:{}:{}", entry.source, entry.property),
                &entry.title,
                Some(&dates::label(entry, now)),
            );
        }
    }
    if matches!(listed, Err(Refused::Off)) {
        view.text(
            section,
            "dated-off",
            "Tasks is off, so this list leaves out the project's tasks.",
        );
    }
}

/// A file's name, and the folder inside the project's folder that holds
/// it, if any.
fn name_and_place<'a>(path: &'a str, folder: &str) -> (&'a str, Option<&'a str>) {
    let inside = path.strip_prefix(folder).unwrap_or(path);
    match inside.rsplit_once('/') {
        Some((place, name)) => (name, Some(place)),
        None => (inside, None),
    }
}

/// The notes in the project's folder and its subfolders, and the other
/// files there besides the project's own file and tasks.
fn notes_and_files(view: &mut View, project: &Project) {
    let notes_section = section(view, "notes", "Notes");
    let Some(folder) = project.folder() else {
        view.text(
            notes_section,
            "notes-none",
            "This project's file isn't in a folder, so it has no notes or files.",
        );
        return;
    };
    let mut entries: Vec<workspace::Entry> = match workspace::list_entries() {
        Ok(entries) => entries
            .into_iter()
            .flatten()
            .filter(|entry| entry.kind == EntryKind::File && entry.path.starts_with(&folder))
            .collect(),
        Err(error) => {
            view.text(
                notes_section,
                "notes-problem",
                &format!(
                    "Noura couldn't list the project's folder: {}",
                    reason(&error)
                ),
            );
            return;
        }
    };
    entries.sort_by(|a, b| {
        a.path
            .to_lowercase()
            .cmp(&b.path.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    let is_note = |entry: &workspace::Entry| {
        entry.object_type.as_deref() == Some("note") && entry.object_id.is_some()
    };
    let notes: Vec<&workspace::Entry> = entries.iter().filter(|entry| is_note(entry)).collect();
    if notes.is_empty() {
        view.text(
            notes_section,
            "notes-empty",
            "No notes in this project's folder",
        );
    } else {
        let list = view.list(notes_section, "notes", "Project notes");
        for note in notes {
            let (name, place) = name_and_place(&note.path, &folder);
            let title = name.strip_suffix(".md").unwrap_or(name);
            let id = note.object_id.as_deref().unwrap_or_default();
            view.item(list, &format!("note:{id}"), title, place);
        }
    }
    let files_section = section(view, "files", "Files");
    let files: Vec<&workspace::Entry> = entries
        .iter()
        .filter(|entry| {
            !is_note(entry)
                && entry.path != project.path
                && entry.object_type.as_deref() != Some("task")
        })
        .collect();
    if files.is_empty() {
        view.text(
            files_section,
            "files-empty",
            "No other files in this project's folder",
        );
        return;
    }
    let list = view.list(files_section, "files", "Project files");
    for file in files {
        let (name, place) = name_and_place(&file.path, &folder);
        view.item(list, &format!("file:{}", file.path), name, place);
    }
}

/// Delete project, which asks first.
fn delete_button(view: &mut View, project: &str) {
    let confirming = state()
        .panels
        .get(project)
        .is_some_and(|remembered| remembered.confirming);
    if !confirming {
        view.button(View::ROOT, "delete", "Delete project");
        return;
    }
    let ask = view.add_panel(View::ROOT, "delete-section", Some("Delete this project?"));
    view.text(ask, "delete-question", "Delete this project?");
    view.text(
        ask,
        "delete-warning",
        "The project file moves to the trash. Its tasks stay where they are.",
    );
    view.button(ask, "delete-confirm", "Delete project");
    view.button(ask, "delete-cancel", "Cancel");
}

/// Change what the panel of the project `project` remembers.
fn remember(project: &str, change: impl FnOnce(&mut Remembered)) {
    change(state().panels.entry(project.to_owned()).or_default());
}

/// Something happened in the panel of the project `project`.
pub(crate) fn handle(project: &str, event: Event) {
    match event {
        Event::Changed(field) if field.node == "status" => {
            if let Value::Choice(Some(status)) = field.value {
                set_status(project, &status);
            }
        }
        Event::Pressed(node) => match node.as_str() {
            "add-task" => remember(project, |remembered| {
                remembered.naming = true;
                remembered.naming_problem = None;
            }),
            "delete" => remember(project, |remembered| remembered.confirming = true),
            "delete-cancel" => remember(project, |remembered| remembered.confirming = false),
            "delete-confirm" => delete(project),
            _ => {}
        },
        // A name reaches the field's plugin once; another for a field that
        // closed adds nothing.
        Event::Named(naming)
            if naming.node == "new-task-name"
                && !naming.name.trim().is_empty()
                && state()
                    .panels
                    .get(project)
                    .is_some_and(|remembered| remembered.naming) =>
        {
            add_task(project, naming.name.trim());
        }
        Event::Named(naming) if naming.node == "new-task-name" => stop_naming(project),
        Event::Cancelled(node) if node == "new-task-name" => stop_naming(project),
        Event::Moved(moved) if moved.node == "board" => {
            if let (Some(task), Some(status)) = (
                moved.item.strip_prefix("card:"),
                moved.to.strip_prefix("column:"),
            ) {
                move_card(project, task, status, moved.position as usize);
            }
        }
        Event::Chosen(item) => choose(project, &item),
        Event::Activated(target) => open(project, &target.item),
        _ => {}
    }
}

fn stop_naming(project: &str) {
    remember(project, |remembered| {
        remembered.naming = false;
        remembered.naming_problem = None;
    });
}

/// Open what an item stands for: a card's or a row's task, a note, or the
/// object a dated value dates.
fn open(project: &str, item: &str) {
    let id = if let Some(task) = item
        .strip_prefix("card:")
        .or_else(|| item.strip_prefix("task:"))
        .or_else(|| item.strip_prefix("note:"))
    {
        task
    } else if let Some(dated) = item.strip_prefix("dated:") {
        match dated.rsplit_once(':') {
            Some((source, _)) if source != project => source,
            _ => return,
        }
    } else {
        return;
    };
    if let Err(error) = navigation::open_object(id, Placement::Preview) {
        remember(project, |remembered| {
            remembered.notice = Some(format!("Couldn't open it: {}", reason(&error)));
        });
    }
}

/// Write the project's new status against the revision the panel drew.
fn set_status(project: &str, status: &str) {
    let revision = state()
        .panels
        .get(project)
        .map(|remembered| remembered.revision.clone())
        .unwrap_or_default();
    let written = objects::update(
        project,
        &revision,
        &Changes {
            body: None,
            properties: json!({ "status": status }).to_string(),
            remove: Vec::new(),
        },
    );
    let notice = match written {
        Ok(_) => None,
        Err(Error::RevisionConflict(_)) => Some(CHANGED_ELSEWHERE.to_owned()),
        Err(error) => Some(format!("Couldn't change the status: {}", reason(&error))),
    };
    remember(project, |remembered| remembered.notice = notice);
}

/// Move the project's file to the trash. Its tasks stay where they are.
fn delete(project: &str) {
    let revision = state()
        .panels
        .get(project)
        .map(|remembered| remembered.revision.clone())
        .unwrap_or_default();
    match objects::delete(project, &revision) {
        Ok(()) => {
            state().panels.remove(project);
        }
        Err(error) => {
            let notice = match error {
                Error::RevisionConflict(_) => {
                    "The project changed elsewhere, so it wasn't deleted. Showing the latest version."
                        .to_owned()
                }
                other => format!("Couldn't delete the project: {}", reason(&other)),
            };
            remember(project, |remembered| {
                remembered.confirming = false;
                remembered.notice = Some(notice);
            });
        }
    }
}

/// Add a task named `name` to the end of the board's To do column, with
/// this project, status todo and priority medium.
fn add_task(project: &str, name: &str) {
    let last = state().panels.get(project).and_then(|remembered| {
        remembered
            .columns
            .iter()
            .find(|(status, _)| status == "todo")
            .and_then(|(_, cards)| cards.last())
            .map(|card| card.order.clone())
    });
    let properties = json!({
        "project": project,
        "status": "todo",
        "priority": "medium",
        "kanban_order": order::between(last.as_deref(), None),
    });
    let problem = match tasks::create(name, properties) {
        Ok(()) => None,
        Err(Refused::Off) => Some(TASKS_OFF.to_owned()),
        Err(Refused::Changed) => Some(format!(
            "Couldn't add \u{201c}{name}\u{201d}. {CHANGED_ELSEWHERE}"
        )),
        Err(Refused::Other(message)) => {
            Some(format!("Couldn't add \u{201c}{name}\u{201d}. {message}"))
        }
    };
    remember(project, |remembered| {
        remembered.naming = problem.is_some();
        remembered.naming_problem = problem;
    });
}

/// Where the card of `task` sits on the board as drawn: its column's
/// status and its place there.
fn place_of(remembered: &Remembered, task: &str) -> Option<(String, usize)> {
    remembered.columns.iter().find_map(|(status, cards)| {
        let index = cards.iter().position(|card| card.task == task)?;
        Some((status.clone(), index))
    })
}

/// A choice in a card's menu: up, down, or to the end of another column.
fn choose(project: &str, item: &str) {
    let place = |task: &str| {
        state()
            .panels
            .get(project)
            .and_then(|remembered| place_of(remembered, task))
    };
    if let Some(task) = item.strip_prefix("move-up:") {
        if let Some((status, index)) = place(task)
            && index > 0
        {
            move_card(project, task, &status, index - 1);
        }
    } else if let Some(task) = item.strip_prefix("move-down:") {
        if let Some((status, index)) = place(task) {
            move_card(project, task, &status, index + 1);
        }
    } else if let Some((status, task)) = item
        .strip_prefix("move-to:")
        .and_then(|rest| rest.split_once(':'))
    {
        move_card(project, task, status, usize::MAX);
    }
}

/// Move the card of `task` to `position` among the other cards of the
/// column `status`, as the board drew them: one write of the task, through
/// Tasks, with its new status when the column changes and an order key
/// between its new neighbors.
fn move_card(project: &str, task: &str, status: &str, position: usize) {
    let planned = {
        let state = state();
        state.panels.get(project).and_then(|remembered| {
            let (from, card) = remembered.columns.iter().find_map(|(status, cards)| {
                let card = cards.iter().find(|card| card.task == task)?;
                Some((status.clone(), card.clone()))
            })?;
            let (_, column) = remembered
                .columns
                .iter()
                .find(|(column, _)| column == status)?;
            let others: Vec<&Card> = column.iter().filter(|card| card.task != task).collect();
            let position = position.min(others.len());
            let lower = position
                .checked_sub(1)
                .map(|before| others[before].order.clone());
            // Cards that share the lower neighbor's key sort by stable ID,
            // and no key fits between them, so the card goes after them,
            // before the next key up.
            let upper = others[position..]
                .iter()
                .find(|after| lower.as_ref().is_none_or(|lower| after.order > *lower))
                .map(|after| after.order.clone());
            Some((from, card.revision, lower, upper))
        })
    };
    let Some((from, revision, lower, upper)) = planned else {
        return;
    };
    let key = order::between(lower.as_deref(), upper.as_deref());
    let properties = if from == status {
        json!({ "kanban_order": key })
    } else {
        json!({ "status": status, "kanban_order": key })
    };
    let notice = match tasks::update(task, &revision, properties) {
        Ok(()) => None,
        Err(Refused::Changed) => Some(MOVE_CHANGED.to_owned()),
        Err(Refused::Off) => Some(TASKS_OFF.to_owned()),
        Err(Refused::Other(message)) => Some(format!("Could not move the task. {message}")),
    };
    remember(project, |remembered| remembered.notice = notice);
}
