//! Native file-manager and terminal affordances for the desktop host.

use std::process::Command;

use local_core::{CoreError, WorkspaceEngine};

/// Resolve the canonical path for the object referenced by its stable ID.
fn resolve_object(
    engine: &WorkspaceEngine,
    id: &str,
    operation: &str,
) -> Result<std::path::PathBuf, CoreError> {
    let object = engine.get_object(id)?.ok_or_else(|| {
        CoreError::validation("object_not_found", "The object does not exist", operation)
    })?;
    engine.resolve_managed_path(&object.relative_path, operation)
}

/// Reveal the object's file in the OS file manager, selecting it when the
/// platform supports it. Takes an object ID because IDs are stable identity;
/// relative paths can change after a rename, move, or reconciliation.
pub fn reveal_in_file_manager(engine: &WorkspaceEngine, id: &str) -> Result<(), CoreError> {
    let path = resolve_object(engine, id, "object_show_in_folder")?;
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(program);
    if cfg!(target_os = "macos") {
        cmd.arg("-R").arg(&path);
    } else if let Some(parent) = path.parent() {
        cmd.arg(parent);
    } else {
        cmd.arg(&path);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|error| CoreError::io(error, "object_show_in_folder", path.to_str()))
}

/// Open a terminal at the object's enclosing folder.
pub fn open_in_terminal(engine: &WorkspaceEngine, id: &str) -> Result<(), CoreError> {
    let path = resolve_object(engine, id, "object_open_terminal")?;
    let dir = path.parent().unwrap_or_else(|| path.as_ref());
    if cfg!(target_os = "macos") {
        Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(dir)
            .spawn()
            .map(|_| ())
            .map_err(|error| CoreError::io(error, "object_open_terminal", dir.to_str()))
    } else {
        Err(CoreError::validation(
            "terminal_not_supported",
            "Opening a terminal is not yet wired for this platform",
            "object_open_terminal",
        ))
    }
}

/// Open an explicitly clicked PDF link through the OS browser boundary.
pub fn open_http_link(value: &str) -> Result<(), CoreError> {
    let url = tauri::Url::parse(value)
        .map_err(|_| CoreError::validation("invalid_link", "Invalid web link", "pdf_open_link"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(CoreError::validation(
            "invalid_link",
            "Only HTTP and HTTPS links can be opened",
            "pdf_open_link",
        ));
    }
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/open").arg(url.as_str()).spawn();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open").arg(url.as_str()).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url.as_str()])
        .spawn();
    result
        .map(|_| ())
        .map_err(|error| CoreError::io(error, "pdf_open_link", None))
}
