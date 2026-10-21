//! The operating system services `app-core` uses: the system trash, the
//! file manager, a terminal, the clipboard and the web browser.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::{Child, Command};
use std::{io, path::Path};

use app_core::System;
use eframe::egui;

/// This computer's trash, file manager and terminal, and the clipboard of
/// the window that `ctx` draws.
pub struct DesktopSystem {
    ctx: egui::Context,
}

impl DesktopSystem {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }
}

impl System for DesktopSystem {
    fn has_trash(&self) -> bool {
        true
    }

    fn trash(&self, path: &Path) -> io::Result<()> {
        app_core::move_to_system_trash(path)
    }

    /// Finder and Explorer select the item. On Linux, `app-core` opens
    /// the folder that holds it through [`Self::open_folder`] instead.
    fn reveal(&self, path: &Path) -> io::Result<()> {
        opener::reveal(path).map_err(io::Error::other)
    }

    fn open_folder(&self, folder: &Path) -> io::Result<()> {
        opener::open(folder).map_err(io::Error::other)
    }

    fn open_terminal(&self, folder: &Path) -> io::Result<()> {
        open_terminal(folder)
    }

    fn copy_text(&self, text: &str) {
        self.ctx.copy_text(text.to_owned());
    }

    fn open_url(&self, url: &str) -> io::Result<()> {
        opener::open_browser(url).map_err(io::Error::other)
    }
}

/// Open Terminal in `folder`.
#[cfg(target_os = "macos")]
fn open_terminal(folder: &Path) -> io::Result<()> {
    reap(
        Command::new("/usr/bin/open")
            .args(["-a", "Terminal"])
            .arg(folder)
            .spawn()?,
    );
    Ok(())
}

/// Open Windows Terminal in `folder` when it's installed, and PowerShell in
/// a new console window otherwise (owner decision, 2026-10-20).
#[cfg(target_os = "windows")]
fn open_terminal(folder: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // Windows Terminal starts in its own working folder: `-d .` names it,
    // so the path needs no escaping.
    match Command::new("wt.exe")
        .args(["-d", "."])
        .current_dir(folder)
        .spawn()
    {
        Ok(child) => {
            reap(child);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            reap(
                Command::new("powershell.exe")
                    .arg("-NoExit")
                    .current_dir(folder)
                    .creation_flags(CREATE_NEW_CONSOLE)
                    .spawn()?,
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Linux has no terminal Noura can rely on, so `app-core` hides Open in
/// Terminal there.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn open_terminal(_folder: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no terminal Noura can rely on",
    ))
}

/// Wait for a launcher process off the UI thread, so it doesn't linger as
/// a zombie once it exits.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn reap(mut child: Child) {
    let waited = std::thread::Builder::new()
        .name("reap-launcher".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(error) = waited {
        tracing::debug!(%error, "could not wait for a launcher process");
    }
}

// Only macOS runs these: `open` fails on a missing path without showing a
// window, while Explorer and xdg-open might.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_missing_item_reports_an_error_instead_of_success() {
        let folder = tempfile::tempdir().unwrap();
        let missing = folder.path().join("Plan.md");
        let system = DesktopSystem::new(egui::Context::default());

        assert!(system.reveal(&missing).is_err());
        assert!(system.open_folder(&missing).is_err());
    }
}
