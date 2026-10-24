//! What Noura asks of the operating system: its trash, its file manager,
//! a terminal, the clipboard, the web browser, the apps that open files
//! and the clock. The desktop app passes the real system; tests pass a
//! fake that records each request.

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

    /// Whether a tree row dragged out of the window can become a native
    /// drag that hands the file manager a copy. Linux can't, because the
    /// window has no GTK side to start one from.
    pub fn drags_out(self) -> bool {
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

    /// Open the web page at `url` in the default browser. Noura checks
    /// that `url` is an `http` or `https` link first.
    fn open_url(&self, url: &str) -> io::Result<()>;

    /// Open the file at `path` in the app the system chooses for its
    /// type, such as the browser for an HTML page.
    fn open_file(&self, path: &Path) -> io::Result<()>;

    /// The hour of the day on this computer's clock, in its time zone,
    /// from 0 to 23.
    fn local_hour(&self) -> u8 {
        u8::try_from(jiff::Zoned::now().hour()).unwrap_or(12)
    }

    /// The instant now on this computer's clock, which plugins read.
    fn now(&self) -> jiff::Timestamp {
        jiff::Timestamp::now()
    }

    /// This computer's time zone, where each day of a calendar grid starts
    /// and ends.
    fn time_zone(&self) -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::system()
    }

    /// The language and region this computer formats dates and times for,
    /// as a BCP 47 tag such as `fr-CA`, when the system says.
    fn locale(&self) -> Option<String> {
        sys_locale::get_locale()
    }
}

/// No operating system services: deleted items go to the workspace's trash
/// folder, and showing files, terminals, the clipboard, the browser and
/// opening files are unavailable.
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

    fn open_url(&self, _url: &str) -> io::Result<()> {
        Err(unavailable())
    }

    fn open_file(&self, _path: &Path) -> io::Result<()> {
        Err(unavailable())
    }
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
