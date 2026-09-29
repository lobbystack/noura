//! File tree actions that reach outside the workspace engine: duplicating a
//! file, showing it in the system file manager, and opening it with the
//! default app. Every path is validated inside the open workspace first.

use std::path::Path;
use std::process::Command;

use local_core::{CoreError, ErrorCategory};
use serde::Deserialize;
use tauri::State;

use super::{AppState, with_engine};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePathInput {
    relative_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCopyInput {
    from: String,
    to: String,
}

#[tauri::command(async)]
pub fn files_copy(state: State<AppState>, input: FileCopyInput) -> Result<(), CoreError> {
    with_engine(&state, "files_copy", |engine| {
        engine.copy_file(&input.from, &input.to)
    })
}

#[tauri::command(async)]
pub fn files_reveal(state: State<AppState>, input: FilePathInput) -> Result<(), CoreError> {
    let operation = "files_reveal";
    with_engine(&state, operation, |engine| {
        let path = existing_path(engine, &input.relative_path, operation)?;
        reveal(&path, operation)
    })
}

#[tauri::command(async)]
pub fn files_open_default(state: State<AppState>, input: FilePathInput) -> Result<(), CoreError> {
    let operation = "files_open_default";
    with_engine(&state, operation, |engine| {
        let path = existing_path(engine, &input.relative_path, operation)?;
        let metadata = std::fs::metadata(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&input.relative_path)))?;
        if !metadata.is_file() || could_run_code(&path, &metadata) {
            return Err(CoreError::new(
                "open_not_allowed",
                ErrorCategory::Permission,
                "noura doesn't open apps or scripts. Show the file in the file manager to open it there.",
                operation,
            ));
        }
        open_with_default_app(&path, operation)
    })
}

fn existing_path(
    engine: &local_core::WorkspaceEngine,
    relative_path: &str,
    operation: &str,
) -> Result<std::path::PathBuf, CoreError> {
    let path = engine.resolve_managed_path(relative_path, operation)?;
    std::fs::symlink_metadata(&path)
        .map_err(|error| CoreError::io(error, operation, Some(relative_path)))?;
    Ok(path)
}

/// Extensions the system would run or install rather than show. Opening one
/// from a workspace someone else wrote would run their code.
const RUNNABLE_EXTENSIONS: &[&str] = &[
    "app",
    "appimage",
    "applescript",
    "bash",
    "bat",
    "bin",
    "cmd",
    "com",
    "command",
    "cpl",
    "csh",
    "deb",
    "desktop",
    "dmg",
    "exe",
    "fish",
    "gadget",
    "hta",
    "inf",
    "ins",
    "jar",
    "js",
    "jse",
    "ksh",
    "lnk",
    "mpkg",
    "msc",
    "msi",
    "msp",
    "pif",
    "pkg",
    "ps1",
    "psm1",
    "py",
    "pyw",
    "rb",
    "reg",
    "rpm",
    "run",
    "scf",
    "scpt",
    "scptd",
    "scr",
    "sh",
    "shortcut",
    "terminal",
    "tool",
    "url",
    "vb",
    "vbe",
    "vbs",
    "webloc",
    "workflow",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "zsh",
];

fn could_run_code(path: &Path, metadata: &std::fs::Metadata) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if extension
        .as_deref()
        .is_some_and(|extension| RUNNABLE_EXTENSIONS.contains(&extension))
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // macOS opens an executable file in Terminal, which runs it.
        if metadata.permissions().mode() & 0o111 != 0 {
            return true;
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    false
}

fn reveal(path: &Path, operation: &str) -> Result<(), CoreError> {
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = {
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(path);
        Command::new("explorer.exe").arg(argument).spawn()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    result
        .map(|_| ())
        .map_err(|error| CoreError::io(error, operation, None))
}

fn open_with_default_app(path: &Path, operation: &str) -> Result<(), CoreError> {
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer.exe").arg(path).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = Command::new("xdg-open").arg(path).spawn();
    result
        .map(|_| ())
        .map_err(|error| CoreError::io(error, operation, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_directory(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("noura-file-actions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn apps_and_scripts_are_never_opened() {
        let directory = scratch_directory("runnable");
        for name in ["setup.EXE", "run.sh", "install.command", "Tool.app.zip.js"] {
            let path = directory.join(name);
            std::fs::write(&path, "x").unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            assert!(could_run_code(&path, &metadata), "{name}");
        }
        for name in ["report.pdf", "photo.JPG", "data.csv", "notes.md"] {
            let path = directory.join(name);
            std::fs::write(&path, "x").unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            assert!(!could_run_code(&path, &metadata), "{name}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn executable_files_are_never_opened() {
        use std::os::unix::fs::PermissionsExt;
        let directory = scratch_directory("executable");
        let path = directory.join("script");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert!(could_run_code(&path, &metadata));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
