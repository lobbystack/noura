//! Local logging and the redacted report behind "Copy diagnostics".

use std::path::Path;

use local_core::CoreError;
use tauri::{AppHandle, Manager, Runtime, State, plugin::TauriPlugin};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

use crate::AppState;

/// Base name of the log file in the app's log folder (`noura.log`).
const LOG_FILE: &str = "noura";
/// Rotate the log at 1 MB and keep a few old files for support.
const MAX_LOG_BYTES: u128 = 1_000_000;
/// How many recent log lines a diagnostics report includes.
const REPORT_LINES: usize = 200;

/// Logging to a rotating file in the app log folder, plus the terminal in
/// development builds. Workspace and sync code log through `tracing`, which
/// forwards into the same logger.
pub fn log_plugin<R: Runtime>() -> TauriPlugin<R> {
    let mut targets = vec![Target::new(TargetKind::LogDir {
        file_name: Some(LOG_FILE.into()),
    })];
    if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stdout));
    }
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .targets(targets)
        .level(log::LevelFilter::Info)
        .level_for("tao", log::LevelFilter::Warn)
        .level_for("wry", log::LevelFilter::Warn)
        .level_for("reqwest", log::LevelFilter::Warn)
        .level_for("hyper", log::LevelFilter::Warn)
        .rotation_strategy(RotationStrategy::KeepSome(3))
        .max_file_size(MAX_LOG_BYTES)
        .build()
}

/// A plain-text report for bug reports: app version, platform, workspace
/// health, and recent log lines. Paths, file names, and quoted text are
/// replaced before the report leaves the app.
#[tauri::command(async)]
pub fn app_diagnostics(app: AppHandle, state: State<AppState>) -> Result<String, CoreError> {
    let mut report = String::new();
    report.push_str(&format!("noura {}\n", app.package_info().version));
    report.push_str(&format!(
        "Platform: {} {} ({})\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY
    ));
    let engine = state.engine.lock().ok().and_then(|engine| engine.clone());
    match engine {
        Some(engine) => {
            let workspace = engine.state();
            report.push_str(&format!(
                "Workspace: open, {} indexed files, sync plugin {}\n",
                workspace.indexed_files,
                if engine.sync_plugin_enabled() {
                    "on"
                } else {
                    "off"
                }
            ));
            let mut codes = workspace
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>();
            codes.sort_unstable();
            let mut counts = Vec::<(String, usize)>::new();
            for code in codes {
                match counts.last_mut() {
                    Some((last, count)) if last == code => *count += 1,
                    _ => counts.push((code.to_owned(), 1)),
                }
            }
            if !counts.is_empty() {
                let summary = counts
                    .iter()
                    .map(|(code, count)| format!("{code} x{count}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                report.push_str(&format!("Workspace issues: {summary}\n"));
            }
        }
        None => report.push_str("Workspace: none open\n"),
    }
    report.push_str("\nRecent log:\n");
    let log_path = app
        .path()
        .app_log_dir()
        .map(|folder| folder.join(format!("{LOG_FILE}.log")));
    match log_path.map(|path| recent_lines(&path)) {
        Ok(Ok(lines)) if !lines.is_empty() => {
            for line in lines {
                report.push_str(&redact(&line));
                report.push('\n');
            }
        }
        _ => report.push_str("(no log entries)\n"),
    }
    Ok(report)
}

fn recent_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    let start = lines.len().saturating_sub(REPORT_LINES);
    Ok(lines[start..].to_vec())
}

/// Remove anything that could name a person's files or content: absolute
/// and relative paths, `key=value` fields that carry paths, and quoted text.
fn redact(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut word = String::new();
    let flush = |word: &mut String, output: &mut String| {
        if word.is_empty() {
            return;
        }
        let path_like = word.contains('/') || word.contains('\\');
        let field = word.split_once('=').map(|(key, _)| key.to_owned());
        match field {
            Some(key) if path_like || key.contains("path") => {
                output.push_str(&key);
                output.push_str("=<redacted>");
            }
            _ if path_like => output.push_str("<path>"),
            _ => output.push_str(word),
        }
        word.clear();
    };
    while let Some(character) = chars.next() {
        if character == '"' || character == '\'' || character == '`' {
            flush(&mut word, &mut output);
            for next in chars.by_ref() {
                if next == character {
                    break;
                }
            }
            output.push_str("<text>");
        } else if character.is_whitespace() {
            flush(&mut word, &mut output);
            output.push(character);
        } else {
            word.push(character);
        }
    }
    flush(&mut word, &mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn reports_hide_paths_file_names_and_quoted_text() {
        let line = r#"[2026-09-28][WARN][local_core::engine::scan] a workspace file could not be read path=Journal/today.md reason="Permission denied (os error 13)""#;
        let redacted = redact(line);
        assert!(!redacted.contains("Journal"));
        assert!(!redacted.contains("today.md"));
        assert!(!redacted.contains("Permission denied"));
        assert!(redacted.contains("path=<redacted>"));
        assert!(redacted.contains("could not be read"));
    }

    #[test]
    fn reports_hide_absolute_and_windows_paths() {
        let redacted = redact(r"open failed for /Users/someone/Notes and C:\Users\someone\Notes");
        assert!(!redacted.contains("someone"));
        assert_eq!(redacted.matches("<path>").count(), 2);
    }
}
