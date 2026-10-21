//! A test plugin for seam 3. The page ID passed to `render` names what to
//! do, and the view reports what happened as one text node per line:
//!
//! - `list`: list the workspace's entries.
//! - `workspace`: list the workspace's entries, read each file and each
//!   object the listing names, then try two paths outside what plugins
//!   may read.
//! - `read-file:<path>`: read a file.
//! - `read-object:<id>`: read an object by its stable ID.
//! - `fetch:<url>`: send a GET request to `url`.
//! - `call:<plugin>/<command>:<input>`: run another plugin's command.
//! - `markdown`: a Markdown node with a heading, emphasis, a link, a
//!   list and a remote image.
//! - `markdown:deep`: Markdown nodes with emphasis, quotes and lists
//!   nested a hundred thousand deep.
//! - `nodes:<case>`: return a node list that breaks a rule of views.
//! - `events`: the events, commands and tool calls the plugin received,
//!   oldest first.
//! - anything else: a panel with a line of text and a button.
//!
//! Its pages are `hello`, titled Probe, `workspace`, `markdown`,
//! `markdown:deep`, titled Deep, `nodes:cycle`, titled Broken, and
//! `events`. The first shares the hello plugin's page ID, so a test can
//! swap one build for the other under an open page.
//!
//! It offers the commands `say-hello`, in the command palette;
//! `describe`, which other plugins may call and which lists the workspace
//! with the probe's own capabilities; and `relay`, which other plugins may
//! call with `<plugin>/<command>:<input>` to run a command in turn. It adds
//! an Inbox section, `probe`, a page in Settings with a button that counts
//! presses, and a tool, `echo`, whose schema asks for a `text` string.

use std::sync::Mutex;

use plugin_sdk::{
    Command, Error, Event, InboxSection, Node, NodeKind, Page, Plugin, Tool, View, command_calls,
    network, workspace,
};

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// How often the button on the Settings page was pressed.
static SETTINGS_PRESSES: Mutex<u32> = Mutex::new(0);

const MARKDOWN: &str = "# Tomatoes\n\nGrow **tomatoes** in *full sun*. Read [the guide](https://example.com/guide).\n\n![A ripe tomato](https://example.com/tomato.png)\n\n- Water daily\n- Pick when red\n";

const ECHO_SCHEMA: &str = r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}"#;

struct Probe;

impl Plugin for Probe {
    fn pages() -> Vec<Page> {
        vec![
            Page {
                id: "hello".into(),
                title: "Probe".into(),
            },
            Page {
                id: "workspace".into(),
                title: "Workspace".into(),
            },
            Page {
                id: "markdown".into(),
                title: "Markdown".into(),
            },
            Page {
                id: "markdown:deep".into(),
                title: "Deep".into(),
            },
            Page {
                id: "nodes:cycle".into(),
                title: "Broken".into(),
            },
            Page {
                id: "events".into(),
                title: "Events".into(),
            },
        ]
    }

    fn render(page: String) -> Vec<Node> {
        if let Some(case) = page.strip_prefix("nodes:") {
            return broken(case);
        }
        if page == "markdown" {
            let mut view = View::panel("root", Some("Markdown"));
            view.markdown(View::ROOT, "notes", MARKDOWN);
            return view.into_nodes();
        }
        if page == "markdown:deep" {
            return deep();
        }
        let lines = if page == "list" {
            list()
        } else if page == "workspace" {
            everything()
        } else if let Some(path) = page.strip_prefix("read-file:") {
            vec![read_file(path)]
        } else if let Some(id) = page.strip_prefix("read-object:") {
            vec![read_object(id)]
        } else if let Some(url) = page.strip_prefix("fetch:") {
            vec![fetch(url)]
        } else if let Some(call) = page.strip_prefix("call:") {
            vec![relay(call)]
        } else if page == "events" {
            EVENTS.lock().unwrap().clone()
        } else {
            let mut view = View::panel("root", Some("Probe"));
            view.text(View::ROOT, "line", "Probe is running.");
            view.button(View::ROOT, "press", "Press");
            return view.into_nodes();
        };
        lines_view(&lines)
    }

    fn handle_event(page: String, event: Event) {
        let Event::Pressed(node) = event;
        record(format!("{page} pressed {node}"));
    }

    fn commands() -> Vec<Command> {
        vec![
            Command {
                id: "say-hello".into(),
                title: "Say hello".into(),
                palette: true,
                plugins: false,
            },
            Command {
                id: "describe".into(),
                title: "Describe the workspace".into(),
                palette: false,
                plugins: true,
            },
            Command {
                id: "relay".into(),
                title: "Relay a command".into(),
                palette: false,
                plugins: true,
            },
        ]
    }

    fn run_command(id: String, input: String) -> Result<String, String> {
        record(format!("command {id} {input}").trim_end().to_owned());
        match id.as_str() {
            "say-hello" => Ok("hello".into()),
            "describe" => Ok(match workspace::list_entries() {
                Ok(entries) => format!("probe read {} entries", entries.len()),
                Err(error) => describe(&error),
            }),
            "relay" => Ok(relay(&input)),
            _ => Err(format!("The probe has no command {id}.")),
        }
    }

    fn inbox_sections() -> Vec<InboxSection> {
        vec![InboxSection {
            id: "probe".into(),
            title: "Probe section".into(),
        }]
    }

    fn render_inbox_section(section: String) -> Vec<Node> {
        let mut view = View::panel("root", None);
        view.text(
            View::ROOT,
            "line",
            &format!("Section {section} is running."),
        );
        view.button(View::ROOT, "press", "Press in the section");
        view.into_nodes()
    }

    fn handle_inbox_event(section: String, event: Event) {
        let Event::Pressed(node) = event;
        record(format!("inbox {section} pressed {node}"));
    }

    fn settings_page() -> Option<String> {
        Some("Probe settings".into())
    }

    fn render_settings() -> Vec<Node> {
        let presses = *SETTINGS_PRESSES.lock().unwrap();
        let mut view = View::panel("root", Some("Probe settings"));
        view.text(View::ROOT, "presses", &format!("Pressed {presses} times"));
        view.button(View::ROOT, "count", "Count a press");
        view.into_nodes()
    }

    fn handle_settings_event(event: Event) {
        let Event::Pressed(node) = event;
        if node == "count" {
            *SETTINGS_PRESSES.lock().unwrap() += 1;
        }
        record(format!("settings pressed {node}"));
    }

    fn tools() -> Vec<Tool> {
        vec![Tool {
            name: "echo".into(),
            description: "Repeats the text it gets.".into(),
            input_schema: ECHO_SCHEMA.into(),
        }]
    }

    fn call_tool(name: String, arguments: String) -> Result<String, String> {
        record(format!("tool {name} {arguments}"));
        match name.as_str() {
            "echo" => Ok(arguments),
            _ => Err(format!("The probe has no tool {name}.")),
        }
    }
}

fn record(event: String) {
    EVENTS.lock().unwrap().push(event);
}

/// A panel with one text node per line.
fn lines_view(lines: &[String]) -> Vec<Node> {
    let mut view = View::panel("root", Some("Probe"));
    for (index, line) in lines.iter().enumerate() {
        view.text(View::ROOT, &format!("line-{index}"), line);
    }
    view.into_nodes()
}

/// Markdown nested far deeper than anyone writes by hand, one node per
/// kind of nesting.
fn deep() -> Vec<Node> {
    const DEEP: usize = 100_000;
    let mut view = View::panel("root", Some("Deep"));
    let stars = "*".repeat(DEEP);
    view.markdown(
        View::ROOT,
        "emphasis",
        &format!("{stars}deep emphasis{stars}"),
    );
    view.markdown(
        View::ROOT,
        "quotes",
        &format!("{} deep quote\n", ">".repeat(DEEP)),
    );
    view.markdown(
        View::ROOT,
        "lists",
        &format!("{}deep list\n", "- ".repeat(DEEP)),
    );
    view.text(View::ROOT, "after", "Still drawing.");
    view.into_nodes()
}

fn read_file(path: &str) -> String {
    match workspace::read_file(path) {
        Ok(file) => format!(
            "file {} revision {} text {}",
            file.path,
            file.revision,
            String::from_utf8_lossy(&file.bytes).trim()
        ),
        Err(error) => describe(&error),
    }
}

fn read_object(id: &str) -> String {
    match workspace::read_object(id) {
        Ok(object) => format!(
            "object {} type {} title {} path {} revision {} body {}",
            object.id,
            object.object_type,
            object.title,
            object.path,
            object.revision,
            object.body.trim()
        ),
        Err(error) => describe(&error),
    }
}

/// Send a GET request and say what came back.
fn fetch(url: &str) -> String {
    let request = network::Request {
        method: network::Method::Get,
        url: url.to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    match network::fetch(&request) {
        Ok(response) => format!(
            "status {} url {} body {}",
            response.status,
            response.url,
            String::from_utf8_lossy(&response.body)
        ),
        Err(error) => describe(&error),
    }
}

/// Run the command `<plugin>/<command>:<input>` names, and say what came
/// back.
fn relay(call: &str) -> String {
    let (target, input) = call.split_once(':').unwrap_or((call, ""));
    let (plugin, command) = target.split_once('/').unwrap_or((target, ""));
    match command_calls::call(plugin, command, input) {
        Ok(answer) => format!("answer {answer}"),
        Err(error) => describe(&error),
    }
}

/// The listing, then each file and object it names, then two paths a
/// plugin may not read.
fn everything() -> Vec<String> {
    let mut lines = list();
    if let Ok(entries) = workspace::list_entries() {
        for entry in entries.into_iter().flatten() {
            if entry.kind == workspace::EntryKind::File {
                lines.push(read_file(&entry.path));
            }
            if let Some(id) = &entry.object_id {
                lines.push(read_object(id));
            }
        }
    }
    lines.push(read_file(".noura/workspace.yaml"));
    lines.push(read_file("../outside.md"));
    lines
}

fn list() -> Vec<String> {
    match workspace::list_entries() {
        Ok(entries) => entries
            .into_iter()
            .map(|entry| match entry {
                Ok(entry) => format!(
                    "{} {} object {} type {} revision {}",
                    match entry.kind {
                        workspace::EntryKind::File => "file",
                        workspace::EntryKind::Folder => "folder",
                    },
                    entry.path,
                    entry.object_id.as_deref().unwrap_or("-"),
                    entry.object_type.as_deref().unwrap_or("-"),
                    entry.revision.as_deref().unwrap_or("-"),
                ),
                Err(error) => describe(&error),
            })
            .collect(),
        Err(error) => vec![describe(&error)],
    }
}

fn describe(error: &Error) -> String {
    match error {
        Error::CapabilityNotDeclared(capability) => {
            format!("error capability-not-declared {capability}")
        }
        Error::UnsupportedPath => "error unsupported-path".to_owned(),
        Error::InvalidPath(path) => format!("error invalid-path {path}"),
        Error::NotFound(what) => format!("error not-found {what}"),
        Error::Failed(message) => format!("error failed {message}"),
        Error::NetworkRefused(reason) => format!("error network-refused {reason}"),
        Error::TargetNotDeclared(target) => format!("error target-not-declared {target}"),
        Error::TargetUnavailable(target) => format!("error target-unavailable {target}"),
        Error::Busy(plugin) => format!("error busy {plugin}"),
        other => format!("error {other:?}"),
    }
}

fn node(id: &str, kind: NodeKind, children: &[u32]) -> Node {
    Node {
        id: id.to_owned(),
        kind,
        children: children.to_vec(),
    }
}

/// A node list that breaks one rule of views.
fn broken(case: &str) -> Vec<Node> {
    let root = |children: &[u32]| node("root", NodeKind::Panel(None), children);
    let text = |id: &str| node(id, NodeKind::Text("text".into()), &[]);
    match case {
        "out-of-range" => vec![root(&[1, 5]), text("a")],
        "cycle" => vec![
            root(&[1]),
            node("inner", NodeKind::Panel(None), &[2]),
            node("deeper", NodeKind::Panel(None), &[1]),
        ],
        "unlabeled" => vec![
            root(&[1]),
            node("press", NodeKind::Button("  ".into()), &[]),
        ],
        "duplicate-id" => vec![root(&[1, 2]), text("same"), text("same")],
        "children-of-text" => vec![root(&[1]), node("a", NodeKind::Text("a".into()), &[0])],
        "empty" => Vec::new(),
        _ => vec![root(&[])],
    }
}

plugin_sdk::export!(Probe);
