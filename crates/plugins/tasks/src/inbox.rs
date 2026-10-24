//! The Due today section Tasks adds to the Inbox: how many tasks not
//! marked done are due by the end of today, overdue included, and the
//! first six of them in the Tasks list order. One click opens a task, its
//! checkbox completes it against the revision the section drew, All tasks
//! opens the Tasks page, and a field adds a task due on this device's
//! date.

use plugin_sdk::{
    Event, InboxSection, Node, Value, View,
    navigation::{self, Placement},
    objects::{self, NewObject},
};
use serde_json::json;

use crate::{
    PAGE, reason, set_done, state,
    task::TASK,
    tasks,
    views::{self, Choice, Now},
};

/// The section's ID.
pub(crate) const SECTION: &str = "due-today";

/// The most tasks the section shows.
const MOST_SHOWN: usize = 6;

/// The list of tasks due.
const LIST: &str = "due";

/// The button that opens the Tasks page.
const ALL_TASKS: &str = "all-tasks";

/// The form that adds a task due today.
const ADD: &str = "add-today";

pub(crate) fn sections() -> Vec<InboxSection> {
    vec![InboxSection {
        id: SECTION.into(),
        title: "Due today".into(),
    }]
}

/// The section: the count and the tasks due by tonight, or a line that
/// says nothing is due.
pub(crate) fn render() -> Vec<Node> {
    let now = Now::read();
    let mut view = View::panel("due-today", Some("Due today"));
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
    let mut due: Vec<_> = all
        .into_iter()
        .filter(|task| views::holds(&Choice::Today, task, &now))
        .collect();
    views::order(&mut due, &now);
    let shown = &due[..due.len().min(MOST_SHOWN)];
    let mut state = state();
    state.due_listed = shown
        .iter()
        .map(|task| (task.id.clone(), task.revision.clone()))
        .collect();
    if let Some(notice) = &state.due_notice {
        view.text(View::ROOT, "due-notice", notice);
    }
    drop(state);
    if due.is_empty() {
        view.text(View::ROOT, "nothing-due", "Nothing due today.");
    } else {
        view.text(View::ROOT, "due-count", &count(due.len()));
        let list = view.list(View::ROOT, LIST, "Due today");
        view.open_on_click(list);
        for task in shown {
            let due = task.due.map(|due| views::due_label(due, &now));
            let item = view.checkbox_item(list, &task.id, &task.title, due.as_deref(), false);
            if views::overdue(task, &now) {
                view.warn(item, "Overdue");
            }
        }
    }
    view.button(View::ROOT, ALL_TASKS, "All tasks");
    let state = self::state();
    let form = view.form(View::ROOT, ADD, "Add a task for today", "Add");
    // Each task added starts a new field, so the field you typed in
    // clears; one that failed keeps what you typed.
    let input = format!("today-name-{}", state.added);
    view.input(form, &input, "Task", Value::Text(String::new()));
    view.require(form, &input);
    if let Some(problem) = &state.add_problem {
        view.text(View::ROOT, "add-problem", problem);
    }
    view.into_nodes()
}

/// Something you did in the section.
pub(crate) fn handle(event: Event) {
    match event {
        Event::Activated(target) if target.node == LIST => {
            if let Err(error) = navigation::open_object(&target.item, Placement::Preview) {
                state().due_notice = Some(format!("Couldn't open the task: {}", reason(&error)));
            }
        }
        Event::Pressed(node) if node == ALL_TASKS => {
            if let Err(error) = navigation::open_page(PAGE) {
                state().due_notice = Some(format!("Couldn't open Tasks: {}", reason(&error)));
            }
        }
        Event::Submitted(submission) if submission.node == ADD => {
            let name = submission
                .fields
                .iter()
                .find_map(|field| match &field.value {
                    Value::Text(text) => Some(text.trim().to_owned()),
                    _ => None,
                });
            if let Some(name) = name.filter(|name| !name.is_empty()) {
                add(&name);
            }
        }
        Event::Checked(check) if check.node == LIST => {
            let Some(revision) = state().due_listed.get(&check.item).cloned() else {
                return;
            };
            state().due_notice = set_done(&check.item, &revision, check.checked);
        }
        _ => {}
    }
}

/// Create a task named `name` due on this device's date today. A create
/// that failed says why and keeps the field as you left it.
fn add(name: &str) {
    let today = Now::read().today;
    let created = objects::create(&NewObject {
        object_type: TASK.to_owned(),
        title: name.to_owned(),
        path: None,
        body: String::new(),
        properties: json!({ "due": today.to_string() }).to_string(),
    });
    let mut state = state();
    match created {
        Ok(_) => {
            state.added += 1;
            state.add_problem = None;
        }
        Err(error) => {
            state.add_problem = Some(format!(
                "Couldn't add \u{201c}{name}\u{201d}. {}",
                reason(&error)
            ));
        }
    }
}

/// "1 task" or "7 tasks".
fn count(tasks: usize) -> String {
    if tasks == 1 {
        "1 task".to_owned()
    } else {
        format!("{tasks} tasks")
    }
}
