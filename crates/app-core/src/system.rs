//! What Noura asks of the operating system: its trash, its file manager,
//! a terminal and the clipboard. The desktop app passes the real system;
//! tests pass a fake that records each request.

use std::{io, path::Path};

/// The desktop platform, which decides what the file tree's menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this build runs on. Every system that isn't macOS or
    /// Windows behaves as Linux.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// The menu item that shows a file or folder in the file manager.
    pub fn reveal_label(self) -> &'static str {
        match self {
            Self::MacOs => "Reveal in Finder",
            Self::Windows => "Show in Explorer",
            Self::Linux => "Show in file manager",
        }
    }

    /// Whether the file manager can select an item. Where it can't, Noura
    /// opens the folder that holds the item instead.
    pub fn reveal_selects(self) -> bool {
        matches!(self, Self::MacOs | Self::Windows)
    }

    /// Whether the platform has a terminal Noura can rely on: Terminal on
    /// macOS, and Windows Terminal or PowerShell on Windows. Linux has no
    /// one terminal, so Noura doesn't offer Open in Terminal there.
    pub fn has_terminal(self) -> bool {
        matches!(self, Self::MacOs | Self::Windows)
    }
}

/// The operating system services the files service uses.
pub trait System: Send + Sync {
    /// The platform whose conventions the menus follow.
    fn platform(&self) -> Platform {
        Platform::current()
    }

    /// Whether the system has a trash. Without one, deleted items go to the
    /// workspace's own trash folder.
    fn has_trash(&self) -> bool;

    /// Move a file or folder to the system trash. An error leaves the item
    /// where it was, and Noura uses the workspace's trash folder instead.
    fn trash(&self, path: &Path) -> io::Result<()>;

    /// Show `path` selected in the file manager.
    fn reveal(&self, path: &Path) -> io::Result<()>;

    /// Open `folder` in the file manager.
    fn open_folder(&self, folder: &Path) -> io::Result<()>;

    /// Open a terminal whose working folder is `folder`.
    fn open_terminal(&self, folder: &Path) -> io::Result<()>;

    /// Put `text` on the clipboard.
    fn copy_text(&self, text: &str);
}

/// No operating system services: deleted items go to the workspace's trash
/// folder, and showing files, terminals and the clipboard are unavailable.
pub struct NoSystem;

impl System for NoSystem {
    fn has_trash(&self) -> bool {
        false
    }

    fn trash(&self, _path: &Path) -> io::Result<()> {
        Err(unavailable())
    }

    fn reveal(&self, _path: &Path) -> io::Result<()> {
        Err(unavailable())
    }

    fn open_folder(&self, _folder: &Path) -> io::Result<()> {
        Err(unavailable())
    }

    fn open_terminal(&self, _folder: &Path) -> io::Result<()> {
        Err(unavailable())
    }

    fn copy_text(&self, _text: &str) {}
}

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "no operating system services here",
    )
}

/// Move `path` to this computer's trash: the macOS Trash, the Windows
/// Recycle Bin or the freedesktop.org trash on Linux. The desktop app's
/// [`System::trash`] calls this.
pub fn move_to_system_trash(path: &Path) -> io::Result<()> {
    match local_core::os_trash() {
        Some(trash) => trash(path),
        None => Err(unavailable()),
    }
}
