//! The selected task in the Tasks page's right pane, edited in place: its
//! due date, status and priority in a row, its title, its description,
//! its subtasks, its project and its assignee. Each change writes the task's file
//! against the revision the pane drew, so an edit made elsewhere since
//! isn't overwritten.
//!
//! Subtasks are the checklist that ends the task's body ([`crate::body`]),
//! so the file reads the same without Noura.

use plugin_sdk::{
    Color, EditableStyle, Error, Event, Value, View,
    navigation::{self, Placement},
    objects::{self, Changes, Object},
};
use serde_json::json;

use crate::{
    CHANGED_ELSEWHERE, State,
    body::{Body, Subtask},
    links, menus, reason, state,
    task::{self, PRIORITIES, STATUSES, Task},
};

/// What the pane's node IDs start with, apart from the page's own.
pub(crate) const PREFIX: &str = "selected-";

/// The subtasks' list, without the prefix.
const SUBTASKS: &str = "subtasks";

/// The name field a subtask shows while you rename it.
pub(crate) const SUBTASK_NAME: &str = "subtask-name";

/// The color of each priority's flag.
const FLAGS: [(&str, Color); 4] = [
    ("low", Color::Blue),
    ("medium", Color::Yellow),
    ("high", Color::Orange),
    ("urgent", Color::Red),
];

fn node(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// Draw the task `id` under `pane`, giving its title the keyboard when
/// `focus` is set.
pub(crate) fn render(
    view: &mut View,
    pane: u32,
    id: &str,
    projects: &[(String, String)],
    state: &mut State,
) {
    let object = match objects::get(id) {
        Ok(object) => object,
        Err(error) => {
            view.text(
                pane,
                &node("problem"),
                &format!("Noura couldn't read the task: {}", reason(&error)),
            );
            return;
        }
    };
    let task = Task::of(&object);
    state.panels.insert(task.id.clone(), task.revision.clone());

    // Due date, status and priority share the top row, with the task's
    // other actions at its end.
    let due = task.due.map(|due| match due {
        task::Due::Day(day) => day.to_string(),
        // A due time shows its day; picking a day drops the time.
        task::Due::At(_) => due.day(&crate::views::Now::read().zone).to_string(),
    });
    view.date_picker(pane, &node("due"), "Due date", due.as_deref());
    let statuses: Vec<(&str, &str, Option<Color>)> = STATUSES
        .iter()
        .map(|(id, name)| (*id, *name, None))
        .collect();
    view.picker(
        pane,
        &node("status"),
        "Status",
        &statuses,
        STATUSES
            .iter()
            .any(|(id, _)| *id == task.status)
            .then_some(task.status.as_str()),
    );
    let priorities: Vec<(&str, &str, Option<Color>)> = PRIORITIES
        .iter()
        .map(|(id, name)| {
            let flag = FLAGS.iter().find(|(flag, _)| flag == id).map(|(_, c)| *c);
            (*id, *name, flag)
        })
        .collect();
    view.picker(
        pane,
        &node("priority"),
        "Priority",
        &priorities,
        PRIORITIES
            .iter()
            .any(|(id, _)| *id == task.priority)
            .then_some(task.priority.as_str()),
    );
    let more = view.button(pane, &node("more"), "More");
    let menu = view.menu(more, &node("more-menu"), "Task actions");
    view.menu_item(menu, &node("open"), "Open in editor", true);
    view.menu_item(menu, &node("trash"), "Move to Trash", true);

    if let Some(notice) = state.panel_notices.get(&task.id) {
        view.text(pane, &node("notice"), notice);
    }
    let focus = state.focus_title.as_deref() == Some(task.id.as_str());
    view.editable(
        pane,
        &node("title"),
        "Title",
        &task.title,
        EditableStyle::Title,
        focus,
    );
    let body = Body::parse(&object.body);
    view.editable(
        pane,
        &node("description"),
        "Description",
        body.description(),
        EditableStyle::Body,
        false,
    );
    if !body.subtasks.is_empty() {
        let list = view.list(pane, &node(SUBTASKS), "Subtasks");
        for (index, subtask) in body.subtasks.iter().enumerate() {
            let item = view.checkbox_item(
                list,
                &format!("subtask-{index}"),
                &subtask.text,
                None,
                subtask.done,
            );
            if state.renaming_subtask == Some(index) {
                view.name_field(item, SUBTASK_NAME, "Subtask", &subtask.text);
            } else {
                let menu = view.menu(
                    item,
                    &format!("subtask-menu-{index}"),
                    &format!("Actions for {}", subtask.text),
                );
                view.menu_item(
                    menu,
                    &format!("subtask-rename:{index}"),
                    "Rename\u{2026}",
                    true,
                );
                view.menu_item(menu, &format!("subtask-delete:{index}"), "Delete", true);
            }
        }
    }
    view.editable(
        pane,
        &node("add-subtask"),
        "Add subtask",
        "",
        EditableStyle::Add,
        false,
    );

    project_picker(view, pane, &task, projects);
    view.editable(
        pane,
        &node("assignee"),
        "Assignee",
        task.assignee.as_deref().unwrap_or_default(),
        EditableStyle::Line,
        false,
    );

    if let Some(problem) = &object.problem {
        view.text(
            pane,
            &node("schema-problem"),
            &format!(
                "{} doesn't fit the task fields: {}",
                problem.property, problem.code
            ),
        );
    }
}

/// The project the task belongs to, as a picker. A link that leads to no
/// project, as after you delete the project, shows as an unknown one
/// until you pick another.
fn project_picker(view: &mut View, pane: u32, task: &Task, projects: &[(String, String)]) {
    let mut options: Vec<(String, String)> = vec![("none".to_owned(), "No project".to_owned())];
    options.extend(projects.iter().cloned());
    let chosen = match (&task.project, &task.project_link) {
        (Some(id), _) => id.clone(),
        (None, Some(_)) => "unknown".to_owned(),
        (None, None) => "none".to_owned(),
    };
    if !options.iter().any(|(id, _)| *id == chosen) {
        options.push((chosen.clone(), "Unknown project".to_owned()));
    }
    let options: Vec<(&str, &str, Option<Color>)> = options
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str(), None))
        .collect();
    view.picker(pane, &node("project"), "Project", &options, Some(&chosen));
}

/// Handle what happened in the pane of the selected task `id`. Returns
/// false when the event isn't the pane's.
pub(crate) fn handle(id: &str, event: &Event) -> bool {
    match event {
        Event::Changed(field) if field.node.starts_with(PREFIX) => {
            change(id, &field.node[PREFIX.len()..], field.value.clone());
        }
        Event::Checked(check) if check.node == node(SUBTASKS) => {
            if let Some(index) = subtask_index(&check.item, "subtask-") {
                let done = check.checked;
                edit_subtasks(id, |subtasks| {
                    if let Some(subtask) = subtasks.get_mut(index) {
                        subtask.done = done;
                    }
                });
            }
        }
        Event::Selected(target) | Event::Activated(target) if target.node == node(SUBTASKS) => {}
        Event::Chosen(item) if *item == node("open") => {
            if let Err(error) = navigation::open_object(id, Placement::Preview) {
                notice(id, format!("Couldn't open the task: {}", reason(&error)));
            }
        }
        Event::Chosen(item) if *item == node("trash") => {
            if let Err(error) = navigation::trash_object(id) {
                notice(
                    id,
                    format!("Couldn't move the task to the trash: {}", reason(&error)),
                );
            }
        }
        Event::Chosen(item) if item.starts_with("subtask-rename:") => {
            state().renaming_subtask = subtask_index(item, "subtask-rename:");
        }
        Event::Chosen(item) if item.starts_with("subtask-delete:") => {
            if let Some(index) = subtask_index(item, "subtask-delete:") {
                state().renaming_subtask = None;
                edit_subtasks(id, |subtasks| {
                    if index < subtasks.len() {
                        subtasks.remove(index);
                    }
                });
            }
        }
        Event::Named(naming) if naming.node == SUBTASK_NAME => {
            let renaming = state().renaming_subtask.take();
            if let Some(index) = renaming {
                let text = naming.name.trim().to_owned();
                edit_subtasks(id, |subtasks| {
                    if let Some(subtask) = subtasks.get_mut(index) {
                        subtask.text = text;
                    }
                });
            }
        }
        Event::Cancelled(node) if node == SUBTASK_NAME => state().renaming_subtask = None,
        _ => return false,
    }
    true
}

fn subtask_index(node: &str, prefix: &str) -> Option<usize> {
    node.strip_prefix(prefix)?.parse().ok()
}

/// A change to the pane's field `field`, written to the task `id`.
fn change(id: &str, field: &str, value: Value) {
    match (field, value) {
        ("title", Value::Text(name)) => {
            let renamed = menus::rename_task(id, &name);
            let mut state = state();
            match renamed {
                Ok(()) => state.panel_notices.remove(id),
                Err(problem) => state.panel_notices.insert(id.to_owned(), problem),
            };
        }
        ("description", Value::Multiline(text) | Value::Text(text)) => {
            edit_body(id, |body| body.with_description(&text));
        }
        // An empty name clears the assignee.
        ("assignee", Value::Text(name)) => {
            let name = name.trim();
            let change = if name.is_empty() {
                (json!({}), vec!["assignee".to_owned()])
            } else {
                (json!({ "assignee": name }), Vec::new())
            };
            write_properties(id, change);
        }
        ("add-subtask", Value::Text(text)) if !text.trim().is_empty() => {
            edit_subtasks(id, |subtasks| {
                subtasks.push(Subtask {
                    text: text.trim().to_owned(),
                    done: false,
                });
            });
        }
        (field, value) => set_property(id, field, value),
    }
}

/// Rewrite the task's subtasks with `edit`.
fn edit_subtasks(id: &str, edit: impl FnOnce(&mut Vec<Subtask>)) {
    edit_body(id, |body| {
        let mut subtasks = body.subtasks.clone();
        edit(&mut subtasks);
        body.with_subtasks(&subtasks)
    });
}

/// Write the body `edit` makes of the task's body, as the pane drew it.
fn edit_body(id: &str, edit: impl FnOnce(&Body) -> String) {
    let Some(revision) = state().panels.get(id).cloned() else {
        return;
    };
    let object = match objects::get(id) {
        Ok(object) => object,
        Err(error) => {
            notice(id, format!("Couldn't change the task: {}", reason(&error)));
            return;
        }
    };
    if object.revision != revision {
        notice(id, CHANGED_ELSEWHERE.to_owned());
        return;
    }
    let body = edit(&Body::parse(&object.body));
    if body == object.body {
        return;
    }
    write(
        &object,
        &Changes {
            body: Some(body),
            properties: "{}".to_owned(),
            remove: Vec::new(),
        },
    );
}

/// Set or clear the task field a status, priority, due date or project
/// pick names.
fn set_property(id: &str, field: &str, value: Value) {
    let (set, remove) = match (field, value) {
        ("status", Value::Choice(Some(status))) => (json!({ "status": status }), Vec::new()),
        ("priority", Value::Choice(Some(priority))) => {
            (json!({ "priority": priority }), Vec::new())
        }
        ("due", Value::Date(Some(day)) | Value::DateTime(Some(day))) => {
            (json!({ "due": day }), Vec::new())
        }
        ("due", Value::Date(None) | Value::DateTime(None)) => (json!({}), vec!["due".to_owned()]),
        ("project", Value::Choice(Some(project))) if project == "unknown" => return,
        ("project", Value::Choice(Some(project))) if project == "none" => {
            (json!({}), vec!["project".to_owned()])
        }
        ("project", Value::Choice(Some(project))) => match links::link_for(&project) {
            Ok(link) => (json!({ "project": link }), Vec::new()),
            Err(error) => {
                notice(
                    id,
                    format!("Couldn't change the project. {}", reason(&error)),
                );
                return;
            }
        },
        _ => return,
    };
    write_properties(id, (set, remove));
}

/// Set and clear fields of the task `id`, against the revision the pane
/// drew.
fn write_properties(id: &str, (set, remove): (serde_json::Value, Vec<String>)) {
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
    after_write(id, written);
}

fn write(object: &Object, changes: &Changes) {
    let written = objects::update(&object.id, &object.revision, changes);
    after_write(&object.id, written);
}

/// Note the revision a write left, or say why it failed.
fn after_write(id: &str, written: Result<Object, Error>) {
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

fn notice(id: &str, text: String) {
    state().panel_notices.insert(id.to_owned(), text);
}
