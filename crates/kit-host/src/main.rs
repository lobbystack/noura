//! The real host for the TypeScript author kit's tests: Noura's plugin
//! service in `app-core`, with a workspace open, driven over JSON lines.
//!
//! ```text
//! noura-kit-host --app-data <folder> --workspace <folder>
//! ```
//!
//! The host creates a workspace in the workspace folder, keeping the files
//! there, opens it as the app does, and prints `{"ready":true}`. Each line
//! on stdin is then a request, `{"id":1,"op":"pages"}`, and each answer is
//! one line on stdout, `{"id":1,"ok":...}` or `{"id":1,"error":"..."}`.
//! The host stops at the end of stdin. Plugins install as Settings
//! installs them, approving every capability their manifest declares, and
//! run on this device's plugin host, which compiles them itself.

mod json;
mod session;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

fn main() {
    let (app_data, workspace) = match arguments() {
        Ok(folders) => folders,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("Usage: noura-kit-host --app-data <folder> --workspace <folder>");
            std::process::exit(2);
        }
    };
    let mut session = match session::Session::open(&app_data, &workspace) {
        Ok(session) => session,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{}", json!({ "ready": true }));
    let _ = out.flush();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                match session.handle(&request) {
                    Ok(value) => json!({ "id": id, "ok": value }),
                    Err(message) => json!({ "id": id, "error": message }),
                }
            }
            Err(error) => {
                json!({ "id": null, "error": format!("The request isn't JSON: {error}") })
            }
        };
        if writeln!(out, "{answer}")
            .and_then(|()| out.flush())
            .is_err()
        {
            break;
        }
    }
}

/// The app data and workspace folders the arguments name.
fn arguments() -> Result<(PathBuf, PathBuf), String> {
    let mut app_data = None;
    let mut workspace = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().map(PathBuf::from);
        match arg.to_str() {
            Some("--app-data") => app_data = value,
            Some("--workspace") => workspace = value,
            _ => return Err(format!("Unknown argument {}.", arg.to_string_lossy())),
        }
    }
    match (app_data, workspace) {
        (Some(app_data), Some(workspace)) => Ok((app_data, workspace)),
        _ => Err("Both folders are needed.".to_owned()),
    }
}
