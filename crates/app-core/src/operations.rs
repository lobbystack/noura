//! The files service's changes to the workspace: new notes and folders.
//! Every change runs through `local-core`, which writes durably before it
//! returns.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use local_core::CreateObjectInput;

use crate::{
    AppError, Workspace,
    files::{has_icloud_placeholder, icloud_placeholder},
};

/// The name new notes and folders start with.
const UNTITLED: &str = "Untitled";

/// What an import copied into the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// The workspace paths of the copied files and folders, in the order
    /// given.
    pub paths: Vec<String>,
    /// Entries left out: links, hidden files, names that aren't UTF-8 and
    /// anything that vanished before Noura could copy it.
    pub skipped: u32,
}

impl Workspace {
    /// Create an empty managed note named Untitled, or Untitled 1 and so on,
    /// in `folder` ("" is the top of the workspace). Returns its workspace
    /// path once the note is on disk.
    pub fn create_note(&self, folder: &str) -> Result<String, AppError> {
        let directory = self.folder_path(folder)?;
        let name = free_name(&directory, UNTITLED, ".md");
        let relative_path = join(folder, &name);
        self.engine()
            .create_object(CreateObjectInput {
                object_type: "note".to_owned(),
                title: name.trim_end_matches(".md").to_owned(),
                body: String::new(),
                relative_path: Some(relative_path.clone()),
                properties: BTreeMap::new(),
            })
            .map_err(|error| AppError::from_change(error, &relative_path))?;
        Ok(relative_path)
    }

    /// Create a folder named Untitled, or Untitled 1 and so on, in `folder`.
    /// Returns its workspace path once the folder is on disk.
    pub fn create_folder(&self, folder: &str) -> Result<String, AppError> {
        let directory = self.folder_path(folder)?;
        let relative_path = join(folder, &free_name(&directory, UNTITLED, ""));
        self.engine()
            .create_folder(&relative_path)
            .map_err(|error| AppError::from_change(error, &relative_path))?;
        Ok(relative_path)
    }

    /// Rename the file or folder at `relative_path` to `name`. A Markdown
    /// file keeps its `.md`, which `name` leaves out. A managed object keeps
    /// its ID and every byte, because its title is its file name. Returns
    /// the new workspace path.
    pub fn rename(&self, relative_path: &str, name: &str) -> Result<String, AppError> {
        name_problem(name)?;
        let source = self.existing(relative_path)?;
        let current = file_name(relative_path);
        let destination = renamed_path(relative_path, name, source.is_dir());
        let new_name = file_name(&destination);
        if new_name == current {
            return Ok(relative_path.to_owned());
        }
        let folder = parent(relative_path);
        let case_only = new_name.to_lowercase() == current.to_lowercase();
        if !case_only && taken_names(&self.folder_path(folder)?).contains(&new_name.to_lowercase())
        {
            return Err(name_taken(name, IN_THIS_FOLDER));
        }
        self.move_item(relative_path, &destination)
            .map_err(|error| or_taken(error, name, IN_THIS_FOLDER))?;
        Ok(destination)
    }

    /// Move the file or folder at `relative_path` into `folder` (`""` is
    /// the top of the workspace), keeping its name. A managed object keeps
    /// its ID. Returns the new workspace path.
    pub fn move_into(&self, relative_path: &str, folder: &str) -> Result<String, AppError> {
        let is_folder = self.existing(relative_path)?.is_dir();
        if crate::files::remap(folder, relative_path, relative_path).is_some() {
            return Err(AppError::new(
                "move_into_itself",
                "A folder can't move into itself or a folder inside it.",
            ));
        }
        if parent(relative_path) == folder {
            return Ok(relative_path.to_owned());
        }
        let name = file_name(relative_path);
        let shown_name = if is_folder { name } else { shown(name) };
        let place = match folder {
            "" => "at the top of the workspace".to_owned(),
            _ => format!("in \u{201c}{}\u{201d}", file_name(folder)),
        };
        if taken_names(&self.folder_path(folder)?).contains(&name.to_lowercase()) {
            return Err(name_taken(shown_name, &place));
        }
        let destination = join(folder, name);
        self.move_item(relative_path, &destination)
            .map_err(|error| or_taken(error, shown_name, &place))?;
        Ok(destination)
    }

    /// Copy the file at `relative_path` next to itself: `Plan.md` becomes
    /// `Plan 1.md`, or the next free number. A managed object's copy gets a
    /// new ID with the same properties and body, and any other file copies
    /// byte for byte. Returns the copy's workspace path once it is on disk.
    pub fn duplicate(&self, relative_path: &str) -> Result<String, AppError> {
        if self.existing(relative_path)?.is_dir() {
            return Err(AppError::new("not_a_file", "You can only duplicate files."));
        }
        let name = file_name(relative_path);
        let (stem, suffix) = match name.rfind('.') {
            Some(dot) if dot > 0 => name.split_at(dot),
            _ => (name, ""),
        };
        let folder = parent(relative_path);
        let copy = join(folder, &free_name(&self.folder_path(folder)?, stem, suffix));
        self.engine()
            .copy_file(relative_path, &copy)
            .map_err(|error| AppError::from_change(error, relative_path))
            .map_err(|error| or_taken(error, shown(file_name(&copy)), IN_THIS_FOLDER))?;
        Ok(copy)
    }

    /// Move the file or folder at `relative_path` to the system trash, or,
    /// when the system has none or refuses, into a folder named for the
    /// time inside the workspace's `.noura/trash` folder, keeping its
    /// workspace path. Returns that path in the workspace when the item
    /// went there, and `None` when it went to the system trash.
    pub fn trash(&self, relative_path: &str) -> Result<Option<String>, AppError> {
        self.existing(relative_path)?;
        self.engine()
            .trash_path(relative_path)
            .map_err(|error| AppError::from_change(error, relative_path))
    }

    /// Whether deleted items go to the system trash, as far as Noura can
    /// tell before it tries.
    pub fn has_system_trash(&self) -> bool {
        self.system().has_trash()
    }

    /// Copy files and folders from elsewhere on the computer, given by
    /// absolute path, into `folder` (`""` is the top of the workspace). The
    /// sources stay where they are, and a taken name gets the next free
    /// number. Noura checks every source first: it refuses a folder copied
    /// into itself or a folder that holds the workspace, and then copies
    /// nothing.
    pub fn import(&self, sources: &[PathBuf], folder: &str) -> Result<Imported, AppError> {
        self.folder_path(folder)?;
        let result = self
            .engine()
            .import_external(sources, folder)
            .map_err(|error| AppError::from_change(error, folder))?;
        Ok(Imported {
            paths: result.imported,
            skipped: result.skipped,
        })
    }

    /// Show the file or folder at `relative_path` in the file manager:
    /// selected on macOS and Windows, or by opening the folder that holds
    /// it on Linux.
    pub fn reveal(&self, relative_path: &str) -> Result<(), AppError> {
        let path = self.on_disk(relative_path)?;
        self.reveal_path(&path)
    }

    /// Show the workspace folder in the file manager, as [`Self::reveal`]
    /// does for an item.
    pub fn reveal_root(&self) -> Result<(), AppError> {
        self.reveal_path(self.engine().root())
    }

    fn reveal_path(&self, path: &Path) -> Result<(), AppError> {
        let system = self.system();
        let shown = if system.platform().reveal_selects() {
            system.reveal(path)
        } else {
            system.open_folder(path.parent().unwrap_or(path))
        };
        shown.map_err(|error| {
            tracing::warn!(%error, "the file manager didn't open");
            AppError::new("reveal_failed", "Noura couldn't open the file manager.")
        })
    }

    /// Put the workspace path of the item at `relative_path` on the
    /// clipboard, with `/` between folders on every platform.
    pub fn copy_path(&self, relative_path: &str) -> Result<(), AppError> {
        self.on_disk(relative_path)?;
        self.system().copy_text(relative_path);
        Ok(())
    }

    /// Whether Open in Terminal works on this platform.
    pub fn can_open_terminal(&self) -> bool {
        self.system().platform().has_terminal()
    }

    /// Open a terminal in the folder at `relative_path`, or in the folder
    /// that holds the file at `relative_path`.
    pub fn open_terminal(&self, relative_path: &str) -> Result<(), AppError> {
        if !self.can_open_terminal() {
            return Err(AppError::new(
                "terminal_unavailable",
                "Noura can't open a terminal on this system.",
            ));
        }
        let path = self.on_disk(relative_path)?;
        let folder = if path.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or(&path)
        };
        self.system().open_terminal(folder).map_err(|error| {
            tracing::warn!(%error, "the terminal didn't open");
            AppError::new("terminal_failed", "Noura couldn't open a terminal.")
        })
    }

    /// Move the file or folder at `from` to `to` through `local-core`.
    fn move_item(&self, from: &str, to: &str) -> Result<(), AppError> {
        let moved = if self.existing(from)?.is_dir() {
            self.engine().move_folder(from, to)
        } else {
            self.engine().move_file(from, to)
        };
        moved.map_err(|error| AppError::from_change(error, from))
    }

    /// Whether a file or folder is at `relative_path` in the workspace.
    pub(crate) fn contains(&self, relative_path: &str) -> bool {
        self.existing(relative_path).is_ok()
    }

    /// Whether nothing is left at `relative_path`, a path inside the
    /// workspace. A path Noura refuses, such as a link, hasn't left.
    pub(crate) fn has_left(&self, relative_path: &str) -> bool {
        matches!(self.existing(relative_path), Err(error) if error.code == "file_missing")
    }

    /// The file or folder at `relative_path` on disk, which must be inside
    /// the workspace and not a link. A file that only an iCloud
    /// placeholder stands for isn't downloaded yet.
    fn existing(&self, relative_path: &str) -> Result<PathBuf, AppError> {
        let path = self
            .engine()
            .resolve_managed_path(relative_path, "file_change")
            .map_err(|error| AppError::from_change(error, relative_path))?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => Ok(path),
            Err(_) if has_icloud_placeholder(&path) => Err(AppError::not_downloaded()),
            Err(_) => Err(gone(relative_path)),
        }
    }

    /// The file or folder at `relative_path` on disk, as [`Self::existing`]
    /// finds it, or the iCloud placeholder that stands for a file not
    /// downloaded yet. Showing an item in the file manager, copying its
    /// path and opening a terminal beside it read nothing, so they work on
    /// such a file too.
    fn on_disk(&self, relative_path: &str) -> Result<PathBuf, AppError> {
        match self.existing(relative_path) {
            Err(error) if error.code == "file_not_downloaded" => self
                .engine()
                .resolve_managed_path(relative_path, "file_change")
                .ok()
                .and_then(|path| icloud_placeholder(&path))
                .ok_or(error),
            found => found,
        }
    }

    /// The folder at `folder` on disk: the workspace folder for "", or a
    /// folder inside the workspace that isn't a link and isn't in a linked
    /// folder.
    fn folder_path(&self, folder: &str) -> Result<PathBuf, AppError> {
        if folder.is_empty() {
            return Ok(self.engine().root().to_owned());
        }
        let path = self
            .engine()
            .resolve_managed_path(folder, "folder_create")
            .map_err(|error| AppError::from_change(error, folder))?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => Ok(path),
            Ok(_) => Err(AppError::new(
                "not_a_folder",
                format!("{folder} isn't a folder."),
            )),
            Err(_) => Err(gone(folder)),
        }
    }
}

const IN_THIS_FOLDER: &str = "in this folder";

/// The error for a name something in the destination folder already uses.
/// `place` says where: "in this folder", "in “Docs”" or "at the top of
/// the workspace".
fn name_taken(name: &str, place: &str) -> AppError {
    AppError::new(
        "name_taken",
        format!("Something named \u{201c}{name}\u{201d} is already {place}."),
    )
}

/// `local-core` found something at the destination after Noura checked the
/// name, as when another app took it meanwhile: name the name that's
/// taken, not the item being changed.
fn or_taken(error: AppError, name: &str, place: &str) -> AppError {
    if error.code == "path_exists" {
        name_taken(name, place)
    } else {
        error
    }
}

/// Why Noura refuses `name` for a file or folder. These are the names
/// Windows, macOS and Obsidian refuse, so a workspace stays portable.
fn name_problem(name: &str) -> Result<(), AppError> {
    let problem = if name.is_empty() {
        "Enter a name."
    } else if name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
        "Names can't contain \\ / : * ? \" < > or |."
    } else if name.starts_with('.') {
        "Names can't start with a dot."
    } else if name.trim() != name {
        "Names can't start or end with a space."
    } else {
        return Ok(());
    };
    Err(AppError::new("invalid_name", problem))
}

/// Where renaming the item at `relative_path` to `name` puts it. A Markdown
/// file keeps its `.md`, which `name` leaves out.
pub(crate) fn renamed_path(relative_path: &str, name: &str, is_folder: bool) -> String {
    let current = file_name(relative_path);
    let new_name = if !is_folder && is_markdown(current) {
        format!("{name}{}", &current[current.len() - 3..])
    } else {
        name.to_owned()
    };
    join(parent(relative_path), &new_name)
}

fn is_markdown(file_name: &str) -> bool {
    file_name.len() > 3
        && file_name.is_char_boundary(file_name.len() - 3)
        && file_name[file_name.len() - 3..].eq_ignore_ascii_case(".md")
}

/// The name the tree shows for the item named `file_name`: Markdown files
/// without `.md`.
fn shown(file_name: &str) -> &str {
    if is_markdown(file_name) {
        &file_name[..file_name.len() - 3]
    } else {
        file_name
    }
}

fn file_name(relative_path: &str) -> &str {
    relative_path
        .rsplit_once('/')
        .map_or(relative_path, |(_, name)| name)
}

/// The folder holding `relative_path`: `""` at the top of the workspace.
fn parent(relative_path: &str) -> &str {
    crate::files::parent_of(relative_path).unwrap_or("")
}

/// The error for an item that left the workspace.
pub(crate) fn gone(relative_path: &str) -> AppError {
    AppError::new(
        "file_missing",
        format!("{relative_path} isn't in the workspace anymore."),
    )
}

/// `folder/name`, or `name` at the top of the workspace.
pub(crate) fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// The first of `stem suffix`, `stem 1suffix`, `stem 2suffix` and so on
/// that no entry in `directory` uses, ignoring case so the name is free on
/// case-insensitive volumes too. Hidden files count, and so do files cloud
/// storage hasn't downloaded.
pub(crate) fn free_name(directory: &Path, stem: &str, suffix: &str) -> String {
    let taken = taken_names(directory);
    (0u32..)
        .map(|number| match number {
            0 => format!("{stem}{suffix}"),
            _ => format!("{stem} {number}{suffix}"),
        })
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .unwrap_or_else(|| format!("{stem}{suffix}"))
}

/// The lowercase names in `directory`. iCloud's `.Name.icloud` stands for
/// `Name`.
fn taken_names(directory: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return HashSet::new();
    };
    let mut taken = HashSet::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_lowercase) else {
            continue;
        };
        if let Some(real) = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".icloud"))
        {
            taken.insert(real.to_owned());
        }
        taken.insert(name);
    }
    taken
}
