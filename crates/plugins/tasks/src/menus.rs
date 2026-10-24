//! A task row's menu in the Tasks views: Open, Open in new tab, Rename…,
//! Mark as done or Mark as not done, and Move to Trash. Rename shows a
//! name field in the row and saves the name against the task's latest
//! revision; Move to Trash asks Noura to move the file to the trash as
//! the file tree does.

use plugin_sdk::{
    Error, View,
    navigation::{self, Placement},
    objects,
};

use crate::{complete, reason, state, task::Task};

/// The node ID of the name field a row shows while you rename its task.
pub(crate) const RENAME: &str = "rename";

/// Give the row at `item` its menu, or the rename field while you rename
/// its task.
pub(crate) fn add(view: &mut View, item: u32, task: &Task, renaming: bool) {
    if renaming {
        view.name_field(item, RENAME, "Task name", &task.title);
        return;
    }
    let id = &task.id;
    let menu = view.menu(
        item,
        &format!("menu:{id}"),
        &format!("Actions for {}", task.title),
    );
    view.menu_item(menu, &format!("open:{id}"), "Open", true);
    view.menu_item(menu, &format!("open-new:{id}"), "Open in new tab", true);
    view.separator(menu, &format!("gap-open:{id}"));
    view.menu_item(menu, &format!("rename:{id}"), "Rename\u{2026}", true);
    if task.done() {
        view.menu_item(menu, &format!("undone:{id}"), "Mark as not done", true);
    } else {
        view.menu_item(menu, &format!("done:{id}"), "Mark as done", true);
    }
    view.separator(menu, &format!("gap-trash:{id}"));
    view.menu_item(menu, &format!("trash:{id}"), "Move to Trash", true);
}

/// Do what the menu item with the node ID `node` says.
pub(crate) fn choose(node: &str) {
    let Some((action, id)) = node.split_once(':') else {
        return;
    };
    match action {
        "open" => open(id, Placement::Preview),
        "open-new" => open(id, Placement::NewTab),
        "rename" => {
            let mut state = state();
            state.renaming = Some(id.to_owned());
            state.rename_problem = None;
            state.selected = Some(id.to_owned());
        }
        "done" => complete(id, true),
        "undone" => complete(id, false),
        "trash" => trash(id),
        _ => {}
    }
}

fn open(id: &str, placement: Placement) {
    state().selected = Some(id.to_owned());
    if let Err(error) = navigation::open_object(id, placement) {
        state().notice = Some(format!("Couldn't open the task: {}", reason(&error)));
    }
}

/// Ask Noura to move the task to the trash, which saves what you typed in
/// it first, closes its tabs and says where the file went.
fn trash(id: &str) {
    stop_renaming();
    if let Err(error) = navigation::trash_object(id) {
        state().notice = Some(format!(
            "Couldn't move the task to the trash: {}",
            reason(&error)
        ));
    }
}

/// Stop renaming, keeping the task's name.
pub(crate) fn stop_renaming() {
    let mut state = state();
    state.renaming = None;
    state.rename_problem = None;
}

/// Rename the task being renamed to `name`, in the folder it is in,
/// against its latest revision. An empty or unchanged name keeps the old
/// one. A rename that fails keeps the field open and says why.
pub(crate) fn rename(name: &str) {
    let Some(id) = state().renaming.clone() else {
        return;
    };
    let name = name.trim();
    let latest = match objects::get(&id) {
        Ok(latest) => latest,
        Err(error) => {
            state().rename_problem = Some(format!("Couldn't rename the task: {}", reason(&error)));
            return;
        }
    };
    if name.is_empty() || name == latest.title {
        stop_renaming();
        return;
    }
    if let Some(problem) = refused(name) {
        state().rename_problem = Some(problem.to_owned());
        return;
    }
    let path = match latest.path.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{name}.md"),
        None => format!("{name}.md"),
    };
    match objects::move_to(&id, &latest.revision, &path) {
        Ok(_) => stop_renaming(),
        Err(Error::AlreadyExists(_)) => {
            state().rename_problem = Some(format!(
                "Something named \u{201c}{name}\u{201d} is already in this folder."
            ));
        }
        Err(error) => {
            state().rename_problem = Some(format!(
                "Couldn't rename \u{201c}{}\u{201d}: {}",
                latest.title,
                reason(&error)
            ));
        }
    }
}

/// Why `name` can't name a task's file, as the file tree says it.
fn refused(name: &str) -> Option<&'static str> {
    const FORBIDDEN: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    if name.contains(FORBIDDEN) {
        Some("Names can't contain \\ / : * ? \" < > or |.")
    } else if name.starts_with('.') {
        Some("Names can't start with a dot.")
    } else if name.chars().any(char::is_control) {
        Some("Names can't contain control characters.")
    } else {
        None
    }
}
