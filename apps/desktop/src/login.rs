//! Open at login through the system's own list: a launch agent on macOS,
//! the Run key on Windows and an XDG autostart entry on Linux, written by
//! `auto-launch`.

use std::io;

use app_core::LoginItem;
use auto_launch::{AutoLaunch, AutoLaunchBuilder, MacOSLaunchMode};

/// The name the system lists Noura under. The legacy app used the same
/// launch agent name, so the setting carries over.
const NAME: &str = "Noura";

/// The bundle identifier that macOS shows the login item under.
const BUNDLE_ID: &str = "org.noura.desktop";

/// This computer's list of apps that open at login.
pub struct DesktopLoginItem {
    /// `None` when Noura can't name its own program to the system.
    launcher: Option<AutoLaunch>,
}

impl DesktopLoginItem {
    pub fn new() -> Self {
        let launcher = launcher()
            .inspect_err(|error| tracing::warn!(%error, "Open at login is unavailable"))
            .ok();
        Self { launcher }
    }

    fn launcher(&self) -> io::Result<&AutoLaunch> {
        self.launcher.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Noura couldn't name its program",
            )
        })
    }
}

impl LoginItem for DesktopLoginItem {
    fn is_enabled(&self) -> io::Result<bool> {
        self.launcher()?.is_enabled().map_err(io_error)
    }

    fn set_enabled(&self, enabled: bool) -> io::Result<()> {
        let launcher = self.launcher()?;
        if enabled {
            launcher.enable()
        } else {
            launcher.disable()
        }
        .map_err(io_error)
    }
}

fn launcher() -> io::Result<AutoLaunch> {
    let program = std::env::current_exe()?;
    let program = program.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Unsupported, "the program's path isn't UTF-8")
    })?;
    // auto-launch writes the path as it is into a launch agent's XML, the
    // Run key's command line or a desktop entry's Exec line, so a path
    // those would read differently never reaches it.
    if program.contains(['&', '<', '>', '"', '\'', '$', '`']) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the program's path has characters the login item can't hold",
        ));
    }
    // A command line splits at spaces, so Windows and Linux get the path
    // in quotes. A launch agent keeps it whole as one argument.
    let program = if cfg!(target_os = "macos") {
        program.to_owned()
    } else {
        format!("\"{program}\"")
    };
    AutoLaunchBuilder::new()
        .set_app_name(NAME)
        .set_app_path(&program)
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .set_bundle_identifiers(&[BUNDLE_ID])
        .build()
        .map_err(io_error)
}

/// Keep the kind of an I/O failure, such as a permission error, so the
/// switch can say why.
fn io_error(error: auto_launch::Error) -> io::Error {
    match error {
        auto_launch::Error::Io(error) => error,
        auto_launch::Error::UnsupportedOS => io::Error::new(io::ErrorKind::Unsupported, error),
        other => io::Error::other(other),
    }
}
