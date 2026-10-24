//! The app with a workspace open, and the requests the kit's tests send.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_core::{App, NoSystem, PluginRow, WorkspaceService};
use serde_json::{Value, json};

use crate::json as convert;

/// How long background work may take before a request gives up on it.
const SETTLE_LIMIT: Duration = Duration::from_secs(60);

pub(crate) struct Session {
    app: App,
}

impl Session {
    /// Create a workspace in `workspace`, keeping its files, and open the
    /// app on it with `app_data` as its app data folder.
    pub(crate) fn open(app_data: &Path, workspace: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(workspace)
            .map_err(|error| format!("The workspace folder can't be made: {error}"))?;
        drop(
            WorkspaceService::new(app_data)
                .create("Kit", workspace)
                .map_err(|error| error.message)?,
        );
        let mut app = App::launch_with(app_data, Arc::new(NoSystem));
        settle(&mut app)?;
        if app.workspace().is_none() {
            return Err("The workspace didn't open.".to_owned());
        }
        Ok(Self { app })
    }

    pub(crate) fn handle(&mut self, request: &Value) -> Result<Value, String> {
        settle(&mut self.app)?;
        let op = text(request, "op")?;
        let answer = match op {
            "install" => {
                let folder = PathBuf::from(text(request, "folder")?);
                self.app.review_plugin_folder(folder);
                self.approve()?
            }
            "installArchive" => {
                let archive = PathBuf::from(text(request, "archive")?);
                self.app.review_plugin_archive(archive);
                self.approve()?
            }
            "plugin" => {
                let id = text(request, "plugin")?;
                self.row(id).map_or(Value::Null, |row| plugin_row(&row))
            }
            "pages" => {
                let pages: Vec<Value> = self
                    .app
                    .rail()
                    .into_iter()
                    .filter(|page| self.app.is_plugin_page(&page.id))
                    .map(|page| json!({ "id": page.id, "title": page.title }))
                    .collect();
                Value::Array(pages)
            }
            "view" => match self.app.plugin_view(text(request, "page")?) {
                None => Value::Null,
                Some(Ok(view)) => json!({ "view": convert::view_node(view.root()) }),
                Some(Err(problem)) => json!({ "problem": problem }),
            },
            "event" => {
                let page = text(request, "page")?.to_owned();
                let event = convert::event(request.get("event").unwrap_or(&Value::Null))?;
                self.app.send_plugin_event(&page, &event);
                settle(&mut self.app)?;
                Value::Null
            }
            "tools" => Value::Array(
                self.app
                    .plugin_tools()
                    .into_iter()
                    .map(|tool| {
                        json!({
                            "plugin": tool.plugin_id,
                            "name": tool.name,
                            "description": tool.description,
                            "inputSchema": tool.input_schema,
                        })
                    })
                    .collect(),
            ),
            "callTool" => {
                let answer = self.app.call_plugin_tool(
                    text(request, "plugin")?,
                    text(request, "name")?,
                    text(request, "arguments")?,
                );
                settle(&mut self.app)?;
                match answer {
                    Ok(answer) => json!({ "answer": answer }),
                    Err(error) => json!({ "failure": error.message }),
                }
            }
            // The object types the host keeps from plugins, which the mock
            // host's lists must match.
            "heldTypes" => json!({
                "kernel": plugin_host::KERNEL_TYPES,
                "plugins": plugin_host::PLUGIN_TYPES
                    .iter()
                    .map(|(object_type, plugin)| ((*object_type).to_owned(), json!(plugin)))
                    .collect::<serde_json::Map<_, _>>(),
            }),
            // How the name of a release archive on GitHub ends, which the
            // kit's package command must match.
            "archiveSuffix" => json!(plugin_host::ARCHIVE_SUFFIX),
            "settle" => Value::Null,
            other => return Err(format!("The host has no request {other}.")),
        };
        Ok(answer)
    }

    /// Approve the plugin the install dialog shows, turn it on in the
    /// workspace and wait until it started.
    fn approve(&mut self) -> Result<Value, String> {
        let Some(review) = self.app.plugin_install().cloned() else {
            return Err("The install dialog didn't open.".to_owned());
        };
        if let Some(problem) = review.problem {
            self.app.cancel_plugin_install();
            return Ok(json!({ "problem": problem }));
        }
        if !self.app.approve_plugin_install() {
            let problem = self
                .app
                .plugin_install()
                .and_then(|review| review.problem.clone())
                .unwrap_or_else(|| "The plugin didn't install.".to_owned());
            self.app.cancel_plugin_install();
            return Ok(json!({ "problem": problem }));
        }
        if !self.app.set_plugin_enabled(&review.id, true) {
            return Ok(json!({ "problem": "The plugin didn't turn on." }));
        }
        settle(&mut self.app)?;
        Ok(self
            .row(&review.id)
            .map_or(Value::Null, |row| plugin_row(&row)))
    }

    fn row(&self, id: &str) -> Option<PluginRow> {
        self.app.plugins().into_iter().find(|row| row.id == id)
    }
}

fn plugin_row(row: &PluginRow) -> Value {
    let names = |lines: &[app_core::CapabilityLine]| -> Vec<String> {
        let mut names: Vec<String> = lines.iter().map(|line| line.name.clone()).collect();
        names.dedup();
        names
    };
    json!({
        "id": row.id,
        "name": row.name,
        "version": row.version,
        "running": row.running,
        "failed": row.failed,
        "problem": row.problem,
        "approved": names(&row.approved),
    })
}

/// The text field `name` of `request`.
fn text<'a>(request: &'a Value, name: &str) -> Result<&'a str, String> {
    request
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("The request needs {name} as text."))
}

/// Poll the app until its background work and its plugins' calls finish.
fn settle(app: &mut App) -> Result<(), String> {
    let deadline = Instant::now() + SETTLE_LIMIT;
    loop {
        app.poll();
        if !app.is_busy() && !app.is_plugin_work_pending() {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("Background work didn't finish within a minute.".to_owned());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
