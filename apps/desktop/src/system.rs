//! The operating system services the files service uses: the system
//! trash, the file manager, a terminal and the clipboard.

use std::{
    io,
    path::Path,
    process::{Child, Command},
};

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

    fn reveal(&self, path: &Path) -> io::Result<()> {
        reveal(path)
    }

    fn open_folder(&self, folder: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        let command = Command::new("/usr/bin/open").arg(folder).spawn();
        #[cfg(target_os = "windows")]
        let command = Command::new("explorer.exe").arg(folder).spawn();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let command = Command::new("xdg-open").arg(folder).spawn();
        reap(command?);
        Ok(())
    }

    fn open_terminal(&self, folder: &Path) -> io::Result<()> {
        open_terminal(folder)
    }

    fn copy_text(&self, text: &str) {
        self.ctx.copy_text(text.to_owned());
    }
}

/// Show `path` selected in Finder.
#[cfg(target_os = "macos")]
fn reveal(path: &Path) -> io::Result<()> {
    reap(Command::new("/usr/bin/open").arg("-R").arg(path).spawn()?);
    Ok(())
}

/// Show `path` selected in Explorer. Explorer needs `/select,` and the
/// quoted path as one argument, or it opens the default folder instead.
#[cfg(target_os = "windows")]
fn reveal(path: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut select = std::ffi::OsString::from("/select,\"");
    select.push(path.as_os_str());
    select.push("\"");
    reap(Command::new("explorer.exe").raw_arg(select).spawn()?);
    Ok(())
}

/// Linux file managers have no common way to select an item, so `app-core`
/// opens the folder that holds it instead.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn reveal(path: &Path) -> io::Result<()> {
    reap(
        Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()?,
    );
    Ok(())
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
