//! Native file-manager, terminal, and browser affordances for the desktop host.

use std::process::Command;

use local_core::{CoreError, ErrorCategory, WorkspaceEngine};
use serde::Serialize;

/// What this build can do on this operating system, so the interface can
/// hide actions that would only fail.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCapabilities {
    /// "Open in Terminal" works here.
    pub open_terminal: bool,
    /// Revealing a file selects it in the file manager instead of only
    /// opening its folder.
    pub reveal_selects_file: bool,
}

#[tauri::command]
pub fn app_capabilities() -> AppCapabilities {
    AppCapabilities {
        open_terminal: cfg!(target_os = "macos"),
        reveal_selects_file: cfg!(any(target_os = "macos", target_os = "windows")),
    }
}

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

/// Reveal the object's file in the OS file manager, selecting it where the
/// platform supports it. Takes an object ID because IDs are stable identity;
/// relative paths can change after a rename, move, or reconciliation.
pub fn reveal_in_file_manager(engine: &WorkspaceEngine, id: &str) -> Result<(), CoreError> {
    let operation = "object_show_in_folder";
    let path = resolve_object(engine, id, operation)?;
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/open").arg("-R").arg(&path).spawn();
    #[cfg(target_os = "windows")]
    let result = {
        // `explorer /select,<path>` must receive the flag and path as one
        // argument, or Explorer opens the default folder instead.
        use std::os::windows::process::CommandExt;
        let mut select = std::ffi::OsString::from("/select,\"");
        select.push(path.as_os_str());
        select.push("\"");
        Command::new("explorer.exe").raw_arg(select).spawn()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(&path))
        .spawn();
    result
        .map(|_| ())
        .map_err(|error| CoreError::io(error, operation, path.to_str()))
}

/// Open a terminal at the object's enclosing folder. Only macOS has one
/// terminal the app can rely on; other platforms get a structured error and
/// `app_capabilities` tells the interface to hide the action there.
pub fn open_in_terminal(engine: &WorkspaceEngine, id: &str) -> Result<(), CoreError> {
    let operation = "object_open_terminal";
    let path = resolve_object(engine, id, operation)?;
    let dir = path.parent().unwrap_or_else(|| path.as_ref());
    if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open")
            .arg("-a")
            .arg("Terminal")
            .arg(dir)
            .spawn()
            .map(|_| ())
            .map_err(|error| CoreError::io(error, operation, dir.to_str()))
    } else {
        Err(CoreError::new(
            "unsupported_platform",
            ErrorCategory::Validation,
            "Opening a terminal isn't available on this system.",
            operation,
        ))
    }
}

/// Open a folder in the OS file manager.
pub fn open_directory(path: &std::path::Path, operation: &str) -> Result<(), CoreError> {
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer.exe").arg(path).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = Command::new("xdg-open").arg(path).spawn();
    result
        .map(|_| ())
        .map_err(|error| CoreError::io(error, operation, path.to_str()))
}

/// Open an explicitly clicked PDF link through the OS browser boundary.
pub fn open_http_link(value: &str) -> Result<(), CoreError> {
    open_web_link(value, "pdf_open_link")
}

/// Open an HTTP or HTTPS link in the default browser. This is the only
/// place the host hands a URL to the operating system.
pub fn open_web_link(value: &str, operation: &str) -> Result<(), CoreError> {
    let url = tauri::Url::parse(value)
        .map_err(|_| CoreError::validation("invalid_link", "Invalid web link", operation))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(CoreError::validation(
            "invalid_link",
            "Only HTTP and HTTPS links can be opened",
            operation,
        ));
    }
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/open").arg(url.as_str()).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url.as_str()])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = Command::new("xdg-open").arg(url.as_str()).spawn();
    result
        .map(|_| ())
        .map_err(|error| CoreError::io(error, operation, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_reach_the_operating_system() {
        for value in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:secret@example.com/",
            "not a url",
            "mailto:someone@example.com",
        ] {
            let error = open_web_link(value, "app_open_link").unwrap_err();
            assert_eq!(error.code, "invalid_link", "{value}");
        }
    }

    #[test]
    fn capabilities_match_the_platform() {
        let capabilities = app_capabilities();
        assert_eq!(capabilities.open_terminal, cfg!(target_os = "macos"));
    }
}
