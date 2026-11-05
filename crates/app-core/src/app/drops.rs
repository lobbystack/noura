//! Dragging between the tree and the file manager: files dropped on the
//! tree are copied in, and a row dragged out of the window hands the file
//! manager a copy.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use super::{App, Change};
use crate::{AppError, DropSpot, EntryKind, FileTree, PendingSaves, files::shown_name};

/// A native drag of a file or folder for the desktop shell to start, so
/// the file manager or another app gets a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragOut {
    /// The item's absolute path.
    pub path: PathBuf,
    /// The name the tree shows for it.
    pub name: String,
}

impl App {
    /// Use `saves` to save changes that are still waiting, before a file
    /// goes to another app.
    pub fn set_pending_saves(&mut self, saves: Arc<dyn PendingSaves>) {
        self.pending_saves = saves;
    }

    /// The row being dragged left the window. On macOS and Windows the
    /// in-tree drag ends, any pending save of the item reaches disk, and
    /// [`App::take_drag_out`] then hands the desktop shell a native drag of
    /// its file or folder. Elsewhere the row stays an in-tree drag.
    pub fn drag_row_out(&mut self) {
        if !self.platform().drags_out() {
            return;
        }
        let Some(relative_path) = self
            .tree
            .as_ref()
            .and_then(FileTree::dragging)
            .map(str::to_owned)
        else {
            return;
        };
        self.cancel_tree_drag();
        self.listeners.emit(Change::Tree);
        // A new action replaces the last one's notice.
        self.dismiss_notice();
        let quoted = self.quoted(&relative_path);
        let is_folder = self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.is_folder(&relative_path));
        if !self.save_before(&relative_path, "dragging it out") {
            return;
        }
        if let Err(error) = self.pending_saves.save_now(&relative_path) {
            let title = format!("Couldn't save {quoted} before dragging it out");
            self.show_notice(&title, error, None);
            return;
        }
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.path_of(&relative_path) {
            Ok(path) => {
                let kind = if is_folder {
                    EntryKind::Folder
                } else {
                    EntryKind::File
                };
                self.drag_out = Some(DragOut {
                    path,
                    name: shown_name(file_name(&relative_path), kind),
                });
                self.dragged_out = Some(relative_path);
            }
            Err(error) => {
                self.follow_failed_change(&relative_path, None);
                self.show_notice(&format!("Couldn't drag {quoted} out"), error, None);
            }
        }
    }

    /// The native drag the desktop shell should start now, once.
    pub fn take_drag_out(&mut self) -> Option<DragOut> {
        self.drag_out.take()
    }

    /// The desktop shell couldn't start the native drag from
    /// [`App::take_drag_out`].
    pub fn drag_out_failed(&mut self) {
        let Some(relative_path) = self.dragged_out.take() else {
            return;
        };
        let title = format!("Couldn't drag {} out", self.quoted(&relative_path));
        let error = AppError::new("drag_failed", "Noura couldn't start the drag. Try again.");
        self.show_notice(&title, error, None);
    }

    /// The native drag from [`App::take_drag_out`] ended, wherever it
    /// landed or when it was cancelled. From now on a drop of the same item
    /// from the file manager copies it, like any other drop.
    pub fn drag_out_ended(&mut self) {
        self.dragged_out = None;
    }

    /// Files dragged in from the file manager, at `paths`, are at `spot`
    /// at time `at`. The folder a drop would copy them into lights up, and
    /// a closed folder the pointer rests on for
    /// [`crate::SPRING_OPEN_DELAY`] opens. When they are the row Noura
    /// dragged out of the window coming back, only the folders that row
    /// could move into light up, as in an in-tree drag.
    pub fn drag_files_over(&mut self, paths: &[PathBuf], spot: DropSpot, at: Instant) {
        let returning = self
            .dragged_out
            .clone()
            .filter(|relative_path| self.is_only(paths, relative_path));
        self.change_tree(|tree| {
            tree.files_over(spot, at, returning.as_deref());
            Some(())
        });
    }

    /// The files dragged in from the file manager left the window, or
    /// their drag ended without a drop: nothing lights up any more, and
    /// the folders the drag opened close again.
    pub fn files_left(&mut self) {
        if self.tree.as_ref().is_some_and(FileTree::takes_files) {
            self.change_tree(|tree| {
                tree.files_left();
                Some(())
            });
        }
    }

    /// Files dragged in from the file manager were dropped on the tree.
    /// Copy them, given by absolute path, into the folder under the pointer
    /// that [`App::drag_files_over`] last found, then select the first copy
    /// and say what arrived. The originals stay where they were. The row
    /// Noura dragged out of the window, dropped back before its native drag
    /// ended, moves instead, as an in-tree drag does. A drop outside the
    /// tree copies nothing.
    pub fn drop_files(&mut self, paths: &[PathBuf]) {
        let dragged_out = self.dragged_out.take();
        let Some(folder) = self.change_tree(FileTree::drop_files) else {
            return;
        };
        if let Some(relative_path) = dragged_out
            && self.is_only(paths, &relative_path)
        {
            if crate::files::can_move_into(&relative_path, &folder) {
                self.move_into(&relative_path, &folder);
            }
            return;
        }
        // A new action replaces the last one's notice.
        self.dismiss_notice();
        let Some(workspace) = &self.workspace else {
            return;
        };
        let imported = workspace.import(paths, &folder);
        // Whatever landed before a failure is on disk, so the tree shows it.
        self.refresh_tree(None);
        let imported = match imported {
            Ok(imported) => imported,
            Err(error) => {
                self.show_notice("Couldn't copy the dropped files", error, None);
                return;
            }
        };
        if let Some(first) = imported.paths.first() {
            self.select(first);
        }
        let place = match folder.as_str() {
            "" => "the top of the workspace".to_owned(),
            _ => format!("\u{201c}{}\u{201d}", file_name(&folder)),
        };
        let title = match imported.paths.as_slice() {
            [] => "Nothing to copy".to_owned(),
            [only] => format!("Copied {} to {place}", self.quoted(only)),
            many => format!("Copied {} items to {place}", many.len()),
        };
        let skipped = match imported.skipped {
            0 => String::new(),
            1 => "Skipped 1 hidden file or link.".to_owned(),
            count => format!("Skipped {count} hidden files or links."),
        };
        self.show_done(&title, skipped);
    }

    /// Whether `paths` is exactly the item at `relative_path` in the open
    /// workspace.
    fn is_only(&self, paths: &[PathBuf], relative_path: &str) -> bool {
        let (Some(workspace), [path]) = (&self.workspace, paths) else {
            return false;
        };
        let Ok(item) = workspace.path_of(relative_path) else {
            return false;
        };
        same_file(path, &item)
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn file_name(relative_path: &str) -> &str {
    relative_path.rsplit('/').next().unwrap_or(relative_path)
}
