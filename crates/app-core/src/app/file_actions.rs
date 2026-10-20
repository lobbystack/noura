//! What people do to files from the tree: each action runs through the
//! files service, then updates the tree, the open file and the notice.

use super::{App, Change, Notice, NoticeKind};
use crate::{
    AppError, EntryKind, OpenedFile, Platform, TreeAction, TreeKey,
    files::{remap, shown_name},
};

fn file_name(relative_path: &str) -> &str {
    relative_path.rsplit('/').next().unwrap_or(relative_path)
}

/// What Noura asks before an action that needs confirming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub title: String,
    pub message: String,
}

/// A tree row being renamed in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub relative_path: String,
    /// The name the field starts with: the row's shown name.
    pub name: String,
    /// Why Noura refused the last name typed.
    pub problem: Option<String>,
}

impl App {
    /// Create a note in `folder`, or in the current row's folder when
    /// `folder` is `None`, then select and open it.
    pub fn new_note(&mut self, folder: Option<&str>) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let folder = folder.map_or_else(|| self.target_folder(), str::to_owned);
        match workspace.create_note(&folder) {
            Ok(path) => {
                self.refresh_tree(None);
                self.select(&path);
                self.open_file(&path);
            }
            Err(error) => self.show_notice("Couldn't create the note", error, None),
        }
    }

    /// Create a folder in `folder`, or in the current row's folder when
    /// `folder` is `None`, then select it and start renaming it.
    pub fn new_folder(&mut self, folder: Option<&str>) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let folder = folder.map_or_else(|| self.target_folder(), str::to_owned);
        match workspace.create_folder(&folder) {
            Ok(path) => {
                self.refresh_tree(None);
                self.select(&path);
                self.start_rename(&path);
            }
            Err(error) => self.show_notice("Couldn't create the folder", error, None),
        }
    }

    /// Move the item at `relative_path` into `folder` (`""` is the top of
    /// the workspace), then open that folder in the tree and select the
    /// item.
    pub fn move_into(&mut self, relative_path: &str, folder: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.move_into(relative_path, folder) {
            Ok(to) if to == relative_path => {}
            Ok(to) => self.follow_move(relative_path, &to),
            Err(error) => {
                let title = format!("Couldn't move {}", self.quoted(relative_path));
                self.show_notice(&title, error, None);
            }
        }
    }

    /// Copy the file at `relative_path` next to itself, then select and
    /// open the copy.
    pub fn duplicate(&mut self, relative_path: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.duplicate(relative_path) {
            Ok(copy) => {
                self.refresh_tree(None);
                self.select(&copy);
                self.open_file(&copy);
            }
            Err(error) => {
                let title = format!("Couldn't duplicate {}", self.quoted(relative_path));
                self.show_notice(&title, error, None);
            }
        }
    }

    /// The platform whose conventions the menus follow.
    pub fn platform(&self) -> Platform {
        self.system.platform()
    }

    /// Show the item at `relative_path` in the file manager.
    pub fn reveal(&mut self, relative_path: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if let Err(error) = workspace.reveal(relative_path) {
            let title = format!("Couldn't show {}", self.quoted(relative_path));
            self.show_notice(&title, error, None);
        }
    }

    /// Show the workspace folder in the file manager.
    pub fn reveal_workspace(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if let Err(error) = workspace.reveal_root() {
            self.show_notice("Couldn't show the workspace", error, None);
        }
    }

    /// Put the workspace path of the item at `relative_path` on the
    /// clipboard.
    pub fn copy_path(&mut self, relative_path: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if let Err(error) = workspace.copy_path(relative_path) {
            let title = format!("Couldn't copy the path of {}", self.quoted(relative_path));
            self.show_notice(&title, error, None);
        }
    }

    /// Whether the menus offer Open in Terminal.
    pub fn can_open_terminal(&self) -> bool {
        self.platform().has_terminal()
    }

    /// Open a terminal in the folder at `relative_path`, or in the folder
    /// that holds the file there.
    pub fn open_terminal(&mut self, relative_path: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if let Err(error) = workspace.open_terminal(relative_path) {
            self.show_notice("Couldn't open a terminal", error, None);
        }
    }

    /// Handle a key pressed while the tree has focus. Opening, renaming
    /// and trashing happen here; the tree's owner gets the actions it has
    /// to draw, such as a row's menu.
    pub fn press_tree_key(&mut self, key: TreeKey) -> Option<TreeAction> {
        let tree = self.tree.as_ref()?;
        if key == TreeKey::Enter {
            let current = tree.focused().map(|row| &tree.rows()[row]);
            let open = self.document.as_ref().map(|file| &file.relative_path);
            if let Some(row) = current
                && row.kind == EntryKind::File
                && Some(&row.relative_path) == open
            {
                let path = row.relative_path.clone();
                self.start_rename(&path);
                return None;
            }
        }
        let action = self.tree.as_mut()?.press(key)?;
        self.apply(action)
    }

    /// Carry out what the tree asked for. Returns the actions the tree's
    /// owner has to carry out itself.
    pub fn apply(&mut self, action: TreeAction) -> Option<TreeAction> {
        match action {
            TreeAction::OpenFile { relative_path } => self.open_file(&relative_path),
            TreeAction::Rename { relative_path } => self.start_rename(&relative_path),
            TreeAction::Trash { relative_path } => self.request_trash(&relative_path),
            TreeAction::Move {
                relative_path,
                folder,
            } => self.move_into(&relative_path, &folder),
            menu @ TreeAction::Menu { .. } => return Some(menu),
        }
        None
    }

    /// Close every folder in the tree.
    pub fn collapse_all(&mut self) {
        if let Some(tree) = &mut self.tree {
            tree.collapse_all();
            self.listeners.emit(Change::Tree);
        }
    }

    /// Move to the trash, as Move to Trash does: a file goes at once, and
    /// a folder waits for [`App::confirm_trash`] after
    /// [`App::pending_trash`] asks.
    pub fn request_trash(&mut self, relative_path: &str) {
        match self.trash_question(relative_path) {
            Some(question) => {
                self.pending_trash = Some((relative_path.to_owned(), question));
                self.listeners.emit(Change::Tree);
            }
            None => self.trash(relative_path),
        }
    }

    /// The folder waiting to go to the trash, and what to ask first.
    pub fn pending_trash(&self) -> Option<(&str, &Question)> {
        self.pending_trash
            .as_ref()
            .map(|(path, question)| (path.as_str(), question))
    }

    /// Move the folder waiting in [`App::pending_trash`] to the trash.
    pub fn confirm_trash(&mut self) {
        if let Some((path, _)) = self.pending_trash.take() {
            self.trash(&path);
        }
    }

    /// Keep the folder waiting in [`App::pending_trash`].
    pub fn cancel_trash(&mut self) {
        if self.pending_trash.take().is_some() {
            self.listeners.emit(Change::Tree);
        }
    }

    /// What to ask before moving the item at `relative_path` to the trash:
    /// a folder asks first and says where it goes, and a file goes without
    /// asking.
    pub fn trash_question(&self, relative_path: &str) -> Option<Question> {
        let workspace = self.workspace.as_ref()?;
        if !self.tree.as_ref()?.is_folder(relative_path) {
            return None;
        }
        let message = if workspace.has_system_trash() {
            "The folder and everything in it go to your system's trash, where you can \
             restore them."
        } else {
            "The folder and everything in it move to .noura/trash in this workspace."
        };
        Some(Question {
            title: format!("Move {} to the trash?", self.quoted(relative_path)),
            message: message.to_owned(),
        })
    }

    /// Move the item at `relative_path` to the trash and say which trash it
    /// went to. A file open in the document area closes, and the next row
    /// becomes current.
    pub fn trash(&mut self, relative_path: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let quoted = self.quoted(relative_path);
        let next = self.row_after_removing(relative_path);
        match workspace.trash(relative_path) {
            Ok(place) => {
                let inside = |path: &str| remap(path, relative_path, relative_path).is_some();
                if self
                    .document
                    .as_ref()
                    .is_some_and(|file| inside(&file.relative_path))
                {
                    self.document = None;
                    self.listeners.emit(Change::Document);
                }
                if self
                    .renaming
                    .as_ref()
                    .is_some_and(|rename| inside(&rename.relative_path))
                {
                    self.renaming = None;
                }
                self.refresh_tree(None);
                if let Some(next) = next {
                    self.select(&next);
                }
                match place {
                    None => self.show_done(
                        &format!("Moved {quoted} to the trash"),
                        "You can restore it from your system's trash.".to_owned(),
                    ),
                    Some(place) => self.show_done(
                        &format!("Moved {quoted} to .noura/trash"),
                        format!(
                            "Noura couldn't use the system trash and moved it to {place} in this \
                             workspace."
                        ),
                    ),
                }
            }
            Err(error) => {
                self.show_notice(&format!("Couldn't move {quoted} to the trash"), error, None);
            }
        }
    }

    /// The row that becomes current once the item at `relative_path`
    /// leaves: the next row outside it, or else the row before it.
    fn row_after_removing(&self, relative_path: &str) -> Option<String> {
        let rows = self.tree.as_ref()?.rows();
        let index = rows
            .iter()
            .position(|row| row.relative_path == relative_path)?;
        rows[index + 1..]
            .iter()
            .find(|row| remap(&row.relative_path, relative_path, relative_path).is_none())
            .or_else(|| index.checked_sub(1).map(|before| &rows[before]))
            .map(|row| row.relative_path.clone())
    }

    /// The row being renamed in place.
    pub fn renaming(&self) -> Option<&Rename> {
        self.renaming.as_ref()
    }

    /// Start renaming the item at `relative_path` in place.
    pub fn start_rename(&mut self, relative_path: &str) {
        let Some(tree) = &mut self.tree else {
            return;
        };
        if !tree.select(relative_path) {
            return;
        }
        let name = tree
            .focused()
            .map(|row| tree.rows()[row].name.clone())
            .unwrap_or_default();
        self.renaming = Some(Rename {
            relative_path: relative_path.to_owned(),
            name,
            problem: None,
        });
        self.listeners.emit(Change::Tree);
    }

    /// Rename the row being renamed to `name`. Returns whether the rename
    /// finished. When Noura refuses the name, renaming goes on and
    /// [`Rename::problem`] says why.
    pub fn commit_rename(&mut self, name: &str) -> bool {
        let (Some(workspace), Some(rename)) = (&self.workspace, &mut self.renaming) else {
            return false;
        };
        if name == rename.name {
            self.cancel_rename();
            return true;
        }
        let from = rename.relative_path.clone();
        match workspace.rename(&from, name) {
            Ok(to) => {
                self.renaming = None;
                self.follow_move(&from, &to);
                true
            }
            Err(error) => {
                rename.problem = Some(error.message);
                self.listeners.emit(Change::Tree);
                false
            }
        }
    }

    /// Rename the row being renamed to `name` and stop renaming, as when
    /// you leave the field. When Noura refuses the name, the row keeps its
    /// old name and a notice says why.
    pub fn finish_rename(&mut self, name: &str) {
        let Some(path) = self.renaming.as_ref().map(|rename| rename.relative_path.clone()) else {
            return;
        };
        if self.commit_rename(name) {
            return;
        }
        let problem = self
            .renaming
            .take()
            .and_then(|rename| rename.problem)
            .unwrap_or_default();
        let title = format!("Couldn't rename {}", self.quoted(&path));
        self.show_notice(&title, AppError::new("rename_failed", problem), None);
        self.listeners.emit(Change::Tree);
    }

    /// Stop renaming and keep the old name.
    pub fn cancel_rename(&mut self) {
        if self.renaming.take().is_some() {
            self.listeners.emit(Change::Tree);
        }
    }

    /// The item's shown name in curly quotes, for notices.
    fn quoted(&self, relative_path: &str) -> String {
        let is_folder = self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.is_folder(relative_path));
        let kind = if is_folder {
            EntryKind::Folder
        } else {
            EntryKind::File
        };
        format!(
            "\u{201c}{}\u{201d}",
            shown_name(file_name(relative_path), kind)
        )
    }

    fn target_folder(&self) -> String {
        self.tree
            .as_ref()
            .map(|tree| tree.target_folder())
            .unwrap_or_default()
    }

    fn select(&mut self, relative_path: &str) {
        if let Some(tree) = &mut self.tree {
            tree.select(relative_path);
        }
    }

    /// After the item at `from` moved to `to`: read the tree, select the
    /// item, and keep the open file open at its new path.
    fn follow_move(&mut self, from: &str, to: &str) {
        self.refresh_tree(Some((from, to)));
        self.select(to);
        let moved = self
            .document
            .as_ref()
            .and_then(|file| remap(&file.relative_path, from, to));
        if let Some(path) = moved {
            self.document = Some(OpenedFile {
                name: shown_name(file_name(&path), EntryKind::File),
                relative_path: path,
            });
            self.listeners.emit(Change::Document);
        }
    }

    /// Read the tree again after a change, keeping open folders and the
    /// current row. `moved` names an item that moved, so its open folders
    /// and the current row follow it.
    pub(super) fn refresh_tree(&mut self, moved: Option<(&str, &str)>) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.file_tree() {
            Ok(mut tree) => {
                if let Some(old) = &self.tree {
                    tree.carry_over(old, moved);
                }
                self.tree = Some(tree);
                self.listeners.emit(Change::Tree);
            }
            Err(error) => self.show_notice("Couldn't read the files", error, None),
        }
    }

    /// Say where an action's result went.
    fn show_done(&mut self, title: &str, message: String) {
        self.notice = Some(Notice {
            title: title.to_owned(),
            message,
            remove_recent: None,
            kind: NoticeKind::Done,
        });
        self.listeners.emit(Change::Notice);
    }
}
