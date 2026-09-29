//! Reaching outside the app: the system file manager, the browser, a
//! terminal, and the command an MCP client runs to reach the workspace.

use local_core::{CoreError, ErrorCategory};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{AppState, os_files, state_unavailable, with_engine};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowInFolderInput {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceFolder {
    Root,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceShowInFolderInput {
    folder: WorkspaceFolder,
}

#[tauri::command(async)]
pub fn workspace_show_in_folder(
    state: State<AppState>,
    input: WorkspaceShowInFolderInput,
) -> Result<(), CoreError> {
    with_engine(&state, "workspace_show_in_folder", |engine| {
        let path = match input.folder {
            WorkspaceFolder::Root => engine.root().to_path_buf(),
        };
        os_files::open_directory(&path, "workspace_show_in_folder")
    })
}

#[tauri::command(async)]
pub fn app_open_link(url: String) -> Result<(), CoreError> {
    os_files::open_web_link(&url, "app_open_link")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnection {
    command: String,
    args: Vec<String>,
}

/// The command an MCP client runs to reach the open workspace through this
/// app's own binary. An AppImage runs from a temporary mount, so use the
/// AppImage file itself there.
#[tauri::command(async)]
pub fn mcp_connection(state: State<AppState>) -> Result<McpConnection, CoreError> {
    with_engine(&state, "mcp_connection", |engine| {
        let command = std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)
            .map_err(|error| CoreError::io(error, "mcp_connection", None))?;
        let unsupported = || {
            CoreError::validation(
                "unsupported_path",
                "This path can't be represented for an MCP client",
                "mcp_connection",
            )
        };
        Ok(McpConnection {
            command: command.to_str().ok_or_else(unsupported)?.to_owned(),
            args: vec![
                "mcp".into(),
                "--workspace".into(),
                engine.root().to_str().ok_or_else(unsupported)?.to_owned(),
            ],
        })
    })
}

/// Start the MCP command exactly as a client would and wait for its
/// initialize reply, so users know the configuration works before copying it.
#[tauri::command]
pub async fn mcp_test_connection(state: State<'_, AppState>) -> Result<bool, CoreError> {
    let connection = mcp_connection(state)?;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, Write};
        let failed = || {
            CoreError::new(
                "mcp_unavailable",
                ErrorCategory::Transient,
                "The MCP server did not start",
                "mcp_test_connection",
            )
        };
        let mut child = std::process::Command::new(&connection.command)
            .args(&connection.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| failed())?;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "noura-settings", "version": "1" }
            }
        });
        let mut stdin = child.stdin.take().ok_or_else(failed)?;
        let stdout = child.stdout.take().ok_or_else(failed)?;
        writeln!(stdin, "{request}").map_err(|_| failed())?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let ok = std::io::BufReader::new(stdout).read_line(&mut line).is_ok()
                && serde_json::from_str::<serde_json::Value>(&line)
                    .is_ok_and(|reply| reply.get("result").is_some());
            let _ = sender.send(ok);
        });
        let ok = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or(false);
        drop(stdin);
        // The probe already exited when it closed stdout early; a failed
        // kill is expected then.
        if let Err(error) = child.kill() {
            log::debug!("the MCP probe process had already stopped: {error}");
        }
        if let Err(error) = child.wait() {
            log::debug!("could not reap the MCP probe process: {error}");
        }
        Ok(ok)
    })
    .await
    .map_err(|_| state_unavailable("mcp_test_connection"))?
}

#[tauri::command(async)]
pub fn object_show_in_folder(
    state: State<AppState>,
    input: ShowInFolderInput,
) -> Result<(), CoreError> {
    with_engine(&state, "object_show_in_folder", |engine| {
        os_files::reveal_in_file_manager(engine, &input.id)
    })
}

#[tauri::command(async)]
pub fn object_open_terminal(
    state: State<AppState>,
    input: ShowInFolderInput,
) -> Result<(), CoreError> {
    with_engine(&state, "object_open_terminal", |engine| {
        os_files::open_in_terminal(engine, &input.id)
    })
}
