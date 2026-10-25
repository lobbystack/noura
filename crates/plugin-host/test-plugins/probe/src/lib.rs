//! A test plugin for seam 3. The page ID passed to `render` names what to
//! do, and the view reports what happened as one text node per line:
//!
//! - `list`: list the workspace's entries.
//! - `workspace`: list the workspace's entries, read each file and each
//!   object the listing names, then try two paths outside what plugins
//!   may read.
//! - `read-file:<path>`: read a file.
//! - `read-object:<id>`: read an object by its stable ID.
//! - `read-object-at:<path>`: read the object in the file at a path.
//! - `create-object:<type>:<title>`: create an object with an empty body.
//! - `update-object:<id>:<revision>:<properties>`: set the properties,
//!   a JSON object, of the object with that ID and revision.
//! - `fetch:<url>`: send a GET request to `url`.
//! - `call:<plugin>/<command>:<input>`: run another plugin's command.
//! - `ai:providers`, `ai:chats`, `ai:create-chat`, `ai:read-chat:<id>`,
//!   `ai:send:<chat>:<provider>:<message>`,
//!   `ai:send-with-tool:<chat>:<provider>:<message>`, which offers the
//!   `echo` tool, `ai:answer-tools:<run>:<content>`, which answers each
//!   call of the run with `content`, `ai:read-run:<id>` and `ai:stop:<id>`:
//!   call the host AI interface, one line per provider, chat, message or
//!   run. A press on a node whose ID is `ai:<call>` makes the call too, and
//!   the `events` page shows each line as `<page> ai <line>`.
//! - `clock`: the clock's reading, as `now <instant> <time zone>`.
//! - `markdown`: a Markdown node with a heading, emphasis, a link, a
//!   list and a remote image.
//! - `markdown:deep`: Markdown nodes with emphasis, quotes and lists
//!   nested a hundred thousand deep.
//! - `nodes:<case>`: return a node list that breaks a rule of views.
//! - `events`: the events, commands and tool calls the plugin received,
//!   oldest first: a press as `<page> pressed <node>`, any other event in
//!   its debug form. A press on a node whose ID is
//!   `call:<plugin>/<command>:<input>` runs that command instead, and
//!   shows as `<page> call` and what came back. A press on
//!   `open-object:<id>:<preview or new-tab>` or `open-page:<page>` asks
//!   Noura to open it, and one on `trash-object:<id>` to move the object
//!   to the trash; each shows as `<page> open` and what came back.
//!
//! - `opens`: ask Noura to open the page `events` while drawing, and say
//!   what came back as `draw open <answer>`.
//!
//! The page `hello` adds a sidebar section with a line of text and a
//! button; the other pages add none.
//! - anything else: a panel with a line of text and a button.
//!
//! Its pages are `hello`, titled Probe, `workspace`, `markdown`,
//! `markdown:deep`, titled Deep, `nodes:cycle`, titled Broken, `events`
//! and `opens`. The first shares the hello plugin's page ID, so a test can
//! swap one build for the other under an open page.
//!
//! It offers the commands `say-hello`, in the command palette;
//! `describe`, which other plugins may call and which lists the workspace
//! with the probe's own capabilities; `relay`, which other plugins may
//! call with `<plugin>/<command>:<input>` to run a command in turn; and
//! `open`, which other plugins may call with `open-object:<id>:<placement>`
//! or `open-page:<page>` to ask Noura to open it. It adds
//! an Inbox section, `probe`, a page in Settings with a button that counts
//! presses, and a tool, `echo`, whose schema asks for a `text` string.

use std::sync::Mutex;

use plugin_sdk::{
    Calendar, CalendarEntry, CalendarMode, Choice, Column, Command, Error, Event, InboxSection,
    Input, Item, ItemList, MenuItem, NameField, Node, NodeKind, Page, Plugin, Table, Tool, Value,
    View, VirtualList, VirtualRow, ai, clock, command_calls, navigation, network, objects,
    workspace,
};

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// An event as the `events` page lists it: a press as `pressed <node>`,
/// any other event in its debug form.
fn event_text(event: &Event) -> String {
    match event {
        Event::Pressed(node) => format!("pressed {node}"),
        other => format!("{other:?}"),
    }
}

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
            Page {
                id: "opens".into(),
                title: "Opens".into(),
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
        } else if let Some(path) = page.strip_prefix("read-object-at:") {
            vec![object_text(workspace::read_object_at(path))]
        } else if let Some(object) = page.strip_prefix("create-object:") {
            vec![create_object(object)]
        } else if let Some(update) = page.strip_prefix("update-object:") {
            vec![update_object(update)]
        } else if let Some(url) = page.strip_prefix("fetch:") {
            vec![fetch(url)]
        } else if let Some(call) = page.strip_prefix("call:") {
            vec![relay(call)]
        } else if let Some(call) = page.strip_prefix("ai:") {
            ai_call(call)
        } else if page == "clock" {
            let now = clock::now();
            vec![format!("now {} {}", now.instant, now.time_zone)]
        } else if page == "events" {
            EVENTS.lock().unwrap().clone()
        } else if page == "opens" {
            vec![format!(
                "draw open {}",
                open("open-page:events").unwrap_or_default()
            )]
        } else {
            let mut view = View::panel("root", Some("Probe"));
            view.text(View::ROOT, "line", "Probe is running.");
            view.button(View::ROOT, "press", "Press");
            return view.into_nodes();
        };
        lines_view(&lines)
    }

    fn sidebar(page: String) -> Vec<Node> {
        if page != "hello" {
            return Vec::new();
        }
        let mut view = View::panel("side", Some("Probe sidebar"));
        view.text(View::ROOT, "side-line", "Sidebar of hello.");
        view.button(View::ROOT, "side-press", "Press in the sidebar");
        view.into_nodes()
    }

    fn rows(_page: String, _node: String, _start: u32, _count: u32) -> Vec<VirtualRow> {
        Vec::new()
    }

    fn handle_event(page: String, event: Event) {
        if let Event::Pressed(node) = &event
            && let Some(call) = node.strip_prefix("call:")
        {
            record(format!("{page} call {}", relay(call)));
            return;
        }
        if let Event::Pressed(node) = &event
            && let Some(call) = node.strip_prefix("ai:")
        {
            for line in ai_call(call) {
                record(format!("{page} ai {line}"));
            }
            return;
        }
        if let Event::Pressed(node) = &event
            && let Some(answer) = open(node)
        {
            record(format!("{page} open {answer}"));
            return;
        }
        record(format!("{page} {}", event_text(&event)));
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
            Command {
                id: "open".into(),
                title: "Open something".into(),
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
            "open" => Ok(open(&input).unwrap_or_default()),
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
        record(format!("inbox {section} {}", event_text(&event)));
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
        if matches!(&event, Event::Pressed(node) if node == "count") {
            *SETTINGS_PRESSES.lock().unwrap() += 1;
        }
        record(format!("settings {}", event_text(&event)));
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

/// Call the host AI interface as `call` says, one line per result.
fn ai_call(call: &str) -> Vec<String> {
    let echo = || ai::ToolDefinition {
        name: "echo".into(),
        description: "Repeats the text it gets.".into(),
        input_schema: ECHO_SCHEMA.into(),
    };
    let (name, rest) = call.split_once(':').unwrap_or((call, ""));
    let lines = match name {
        "providers" => ai::providers().map(|providers| {
            providers
                .iter()
                .map(|p| format!("provider {} {} {} {}", p.id, p.name, p.kind, p.model))
                .collect()
        }),
        "chats" => ai::chats().map(|chats| chats.iter().map(chat_line).collect()),
        "create-chat" => ai::create_chat().map(|chat| vec![chat_line(&chat)]),
        "read-chat" => ai::read_chat(rest).map(|(chat, messages)| {
            std::iter::once(chat_line(&chat))
                .chain(messages.iter().map(|message| {
                    format!(
                        "message {:?} {:?} {} {} {} {}",
                        message.kind,
                        message.status,
                        message.provider.as_deref().unwrap_or("-"),
                        message.model.as_deref().unwrap_or("-"),
                        message.error_code.as_deref().unwrap_or("-"),
                        message.content
                    )
                }))
                .collect()
        }),
        "send" | "send-with-tool" => {
            let mut parts = rest.splitn(3, ':');
            let chat = parts.next().unwrap_or_default().to_owned();
            let provider = parts.next().unwrap_or_default().to_owned();
            let message = parts.next().unwrap_or_default().to_owned();
            let tools = if name == "send-with-tool" {
                vec![echo()]
            } else {
                Vec::new()
            };
            ai::send(&ai::Request {
                chat,
                provider,
                message,
                tools,
            })
            .map(|run| vec![run_line(&run)])
        }
        "answer-tools" => {
            let (run, content) = rest.split_once(':').unwrap_or((rest, "{}"));
            ai::read_run(run).and_then(|current| {
                let results: Vec<ai::ToolResult> = current
                    .tool_calls
                    .iter()
                    .map(|call| ai::ToolResult {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        content: content.to_owned(),
                    })
                    .collect();
                ai::answer_tools(run, &results, &[echo()]).map(|run| vec![run_line(&run)])
            })
        }
        "read-run" => ai::read_run(rest).map(|run| vec![run_line(&run)]),
        "stop" => ai::stop(rest).map(|()| vec!["stopped".to_owned()]),
        other => Ok(vec![format!("no ai call {other}")]),
    };
    lines.unwrap_or_else(|error| vec![describe(&error)])
}

fn chat_line(chat: &ai::Chat) -> String {
    format!("chat {} {}", chat.id, chat.title)
}

fn run_line(run: &ai::Run) -> String {
    let calls: Vec<String> = run
        .tool_calls
        .iter()
        .map(|call| format!("{}={}({})", call.id, call.name, call.arguments))
        .collect();
    let failure = run
        .failure
        .as_ref()
        .map(|failure| format!(" failure {} {}", failure.code, failure.message))
        .unwrap_or_default();
    format!(
        "run {} {:?} message {} calls [{}] text {}{failure}",
        run.id,
        run.status,
        run.message.as_deref().unwrap_or("-"),
        calls.join(","),
        run.text
    )
}

fn record(event: String) {
    EVENTS.lock().unwrap().push(event);
}

/// Ask Noura to open what `node` names, `open-object:<id>:<placement>` or
/// `open-page:<page>`, or to trash `trash-object:<id>`, and say what came
/// back. `None` for any other node.
fn open(node: &str) -> Option<String> {
    let answer = if let Some(id) = node.strip_prefix("trash-object:") {
        navigation::trash_object(id)
    } else if let Some(rest) = node.strip_prefix("open-object:") {
        let (id, placement) = rest.rsplit_once(':').unwrap_or((rest, "preview"));
        let placement = if placement == "new-tab" {
            navigation::Placement::NewTab
        } else {
            navigation::Placement::Preview
        };
        navigation::open_object(id, placement)
    } else {
        navigation::open_page(node.strip_prefix("open-page:")?)
    };
    Some(match answer {
        Ok(()) => "ok".to_owned(),
        Err(error) => describe(&error),
    })
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

fn create_object(object: &str) -> String {
    let (object_type, title) = object.split_once(':').unwrap_or((object, ""));
    match objects::create(&objects::NewObject {
        object_type: object_type.to_owned(),
        title: title.to_owned(),
        path: None,
        body: String::new(),
        properties: "{}".to_owned(),
    }) {
        Ok(object) => format!("object {}", object.id),
        Err(error) => describe(&error),
    }
}

fn update_object(update: &str) -> String {
    let mut parts = update.splitn(3, ':');
    let (id, revision, properties) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or("{}"),
    );
    let changes = objects::Changes {
        body: None,
        properties: properties.to_owned(),
        remove: Vec::new(),
    };
    match objects::update(id, revision, &changes) {
        Ok(object) => format!("object {} revision {}", object.id, object.revision),
        Err(error) => describe(&error),
    }
}

fn read_object(id: &str) -> String {
    object_text(workspace::read_object(id))
}

fn object_text(read: Result<workspace::Object, Error>) -> String {
    match read {
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
        "row-cells" => vec![
            root(&[1]),
            node("plants", table(&["Plant", "Bed"]), &[2]),
            node(
                "basil",
                NodeKind::Row(cells(&["Basil", "Herbs", "April"])),
                &[],
            ),
        ],
        "unlabeled-input" => vec![
            root(&[1]),
            node("name", input(" ", Value::Text(String::new())), &[]),
        ],
        "empty-menu" => vec![
            root(&[1]),
            node("more", NodeKind::Button("More".into()), &[2]),
            node("more-menu", NodeKind::Menu("More actions".into()), &[3]),
            node("gap", NodeKind::Separator, &[]),
        ],
        "item-outside-list" => vec![root(&[1]), node("north", item("North bed"), &[])],
        "panel-in-list" => vec![
            root(&[1]),
            node("beds", item_list("Beds"), &[2]),
            node("inner", NodeKind::Panel(None), &[]),
        ],
        "text-in-item" => vec![
            root(&[1]),
            node("beds", item_list("Beds"), &[2]),
            node("north", item("North bed"), &[3]),
            node("note", NodeKind::Text("Sunny".into()), &[]),
        ],
        "two-menus" => vec![
            root(&[1]),
            node("beds", item_list("Beds"), &[2]),
            node("north", item("North bed"), &[3, 5]),
            node("menu-a", NodeKind::Menu("A".into()), &[4]),
            node("water", menu_item("Water"), &[]),
            node("menu-b", NodeKind::Menu("B".into()), &[6]),
            node("weed", menu_item("Weed"), &[]),
        ],
        "menu-in-panel" => vec![
            root(&[1]),
            node("loose", NodeKind::Menu("Loose".into()), &[2]),
            node("water", menu_item("Water"), &[]),
        ],
        "bad-date" => vec![
            root(&[1]),
            node(
                "sown",
                input("Sown on", Value::Date(Some("2026-02-30".into()))),
                &[],
            ),
        ],
        "bad-date-time" => vec![
            root(&[1]),
            node(
                "at",
                input(
                    "Transplant at",
                    Value::DateTime(Some("2026-10-21 09:30".into())),
                ),
                &[],
            ),
        ],
        "unknown-choice" => vec![
            root(&[1]),
            node(
                "sun",
                NodeKind::Input(Input {
                    label: "Sun".into(),
                    value: Value::Choice(Some("moon".into())),
                    choices: vec![Choice {
                        id: "full".into(),
                        label: "Full sun".into(),
                    }],
                }),
                &[],
            ),
        ],
        "choices-on-text" => vec![
            root(&[1]),
            node(
                "name",
                NodeKind::Input(Input {
                    label: "Name".into(),
                    value: Value::Text(String::new()),
                    choices: vec![Choice {
                        id: "full".into(),
                        label: "Full sun".into(),
                    }],
                }),
                &[],
            ),
        ],
        "form-in-form" => vec![
            root(&[1]),
            node("outer", form("Outer"), &[2]),
            node("inner", form("Inner"), &[]),
        ],
        "required-checkbox" => vec![
            root(&[1]),
            node("new-bed", requiring("New bed", "shaded"), &[2]),
            node("shaded", input("Shaded", Value::Checkbox(false)), &[]),
        ],
        "required-elsewhere" => vec![
            root(&[1, 3]),
            node("new-bed", requiring("New bed", "name"), &[2]),
            node("size", input("Size", Value::Text(String::new())), &[]),
            node("name", input("Name", Value::Text(String::new())), &[]),
        ],
        "no-columns" => vec![root(&[1]), node("plants", table(&[]), &[])],
        "blank-row" => vec![
            root(&[1]),
            node("plants", table(&["Plant", "Bed"]), &[2]),
            node("gap", NodeKind::Row(cells(&["", "  "])), &[]),
        ],
        "long-virtual-list" => vec![
            root(&[1]),
            node(
                "seeds",
                NodeKind::VirtualList(VirtualList {
                    label: "Seeds".into(),
                    rows: 1_000_001,
                }),
                &[],
            ),
        ],
        "card-outside-column" => vec![root(&[1]), node("plan", card("Plan"), &[])],
        "column-outside-board" => vec![root(&[1]), node("todo", column("To do"), &[])],
        "item-in-board" => vec![
            root(&[1]),
            node("work", board(), &[2]),
            node("north", item("North bed"), &[]),
        ],
        "panel-in-column" => vec![
            root(&[1]),
            node("work", board(), &[2]),
            node("todo", column("To do"), &[3]),
            node("inner", NodeKind::Panel(None), &[]),
        ],
        "unlabeled-column" => vec![
            root(&[1]),
            node("work", board(), &[2]),
            node("todo", column(" "), &[]),
        ],
        "unlabeled-card" => vec![
            root(&[1]),
            node("work", board(), &[2]),
            node("todo", column("To do"), &[3]),
            node("plan", card(""), &[]),
        ],
        "board-without-columns" => vec![root(&[1]), node("work", board(), &[])],
        "card-with-two-menus" => vec![
            root(&[1]),
            node("work", board(), &[2]),
            node("todo", column("To do"), &[3]),
            node("plan", card("Plan"), &[4, 6]),
            node("menu-a", NodeKind::Menu("A".into()), &[5]),
            node("start", menu_item("Start"), &[]),
            node("menu-b", NodeKind::Menu("B".into()), &[7]),
            node("finish", menu_item("Finish"), &[]),
        ],
        "entry-outside-calendar" => vec![root(&[1]), node("standup", standup(), &[])],
        "item-in-calendar" => vec![
            root(&[1]),
            node("schedule", calendar("Schedule", "2026-10-21"), &[2]),
            node("north", item("North bed"), &[]),
        ],
        "unlabeled-calendar" => vec![
            root(&[1]),
            node("schedule", calendar("", "2026-10-21"), &[]),
        ],
        "untitled-entry" => vec![
            root(&[1]),
            node("schedule", calendar("Schedule", "2026-10-21"), &[2]),
            node("standup", entry(" ", "2026-10-21", None), &[]),
        ],
        "bad-calendar-date" => vec![
            root(&[1]),
            node("schedule", calendar("Schedule", "2026-10-32"), &[]),
        ],
        "bad-entry-start" => vec![
            root(&[1]),
            node("schedule", calendar("Schedule", "2026-10-21"), &[2]),
            node("standup", entry("Standup", "2026-10-21 09:00", None), &[]),
        ],
        "checklist" => vec![
            root(&[1, 5]),
            node(
                "chores",
                NodeKind::List(ItemList {
                    label: "Chores".into(),
                    reorder: false,
                    selected: Some("mulch".into()),
                    opens_on_click: false,
                }),
                &[2, 3, 4],
            ),
            node(
                "weed",
                check_item("Weed the beds", Some(false), Some("Overdue")),
                &[],
            ),
            node("mulch", check_item("Mulch", Some(true), None), &[]),
            node("plain", check_item("Look around", None, Some("  ")), &[]),
            node("new-chore", name_field("New chore name", ""), &[]),
        ],
        "unknown-selection" => vec![
            root(&[1]),
            node(
                "chores",
                NodeKind::List(ItemList {
                    label: "Chores".into(),
                    reorder: false,
                    selected: Some("missing".into()),
                    opens_on_click: false,
                }),
                &[2],
            ),
            node("weed", check_item("Weed the beds", Some(false), None), &[]),
        ],
        "card-with-checkbox" => vec![
            root(&[1]),
            node("work", board(), &[2]),
            node("todo", column("To do"), &[3]),
            node(
                "plan",
                NodeKind::Card(Item {
                    label: "Plan".into(),
                    detail: None,
                    checked: Some(false),
                    warning: None,
                }),
                &[],
            ),
        ],
        "unlabeled-name-field" => vec![root(&[1]), node("new-chore", name_field(" ", ""), &[])],
        "name-field-with-children" => vec![
            root(&[1]),
            node("new-chore", name_field("New chore name", ""), &[2]),
            text("inside"),
        ],
        "mixed-entry-end" => vec![
            root(&[1]),
            node("schedule", calendar("Schedule", "2026-10-21"), &[2]),
            node(
                "standup",
                entry("Standup", "2026-10-21T09:00:00-04:00", Some("2026-10-22")),
                &[],
            ),
        ],
        _ => vec![root(&[])],
    }
}

fn calendar(label: &str, date: &str) -> NodeKind {
    NodeKind::Calendar(Calendar {
        label: label.into(),
        mode: CalendarMode::Week,
        date: date.into(),
    })
}

fn entry(title: &str, start: &str, end: Option<&str>) -> NodeKind {
    NodeKind::Entry(CalendarEntry {
        title: title.into(),
        start: start.into(),
        end: end.map(Into::into),
    })
}

fn standup() -> NodeKind {
    entry("Standup", "2026-10-21T09:00:00-04:00", None)
}

fn item_list(label: &str) -> NodeKind {
    NodeKind::List(ItemList {
        label: label.into(),
        reorder: false,
        selected: None,
        opens_on_click: false,
    })
}

fn board() -> NodeKind {
    NodeKind::Board("Work".into())
}

fn column(label: &str) -> NodeKind {
    NodeKind::BoardColumn(label.into())
}

fn card(label: &str) -> NodeKind {
    NodeKind::Card(Item {
        label: label.into(),
        detail: None,
        checked: None,
        warning: None,
    })
}

fn cells(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|cell| (*cell).to_owned()).collect()
}

fn table(titles: &[&str]) -> NodeKind {
    NodeKind::Table(Table {
        label: "Plants".into(),
        columns: titles
            .iter()
            .map(|title| Column {
                id: title.to_lowercase(),
                title: (*title).to_owned(),
                sorted: None,
            })
            .collect(),
    })
}

fn input(label: &str, value: Value) -> NodeKind {
    NodeKind::Input(Input {
        label: label.into(),
        value,
        choices: Vec::new(),
    })
}

fn check_item(label: &str, checked: Option<bool>, warning: Option<&str>) -> NodeKind {
    NodeKind::Item(Item {
        label: label.into(),
        detail: None,
        checked,
        warning: warning.map(Into::into),
    })
}

fn name_field(label: &str, value: &str) -> NodeKind {
    NodeKind::NameField(NameField {
        label: label.into(),
        value: value.into(),
    })
}

fn item(label: &str) -> NodeKind {
    NodeKind::Item(Item {
        label: label.into(),
        detail: None,
        checked: None,
        warning: None,
    })
}

fn menu_item(label: &str) -> NodeKind {
    NodeKind::MenuItem(MenuItem {
        label: label.into(),
        enabled: true,
    })
}

fn form(label: &str) -> NodeKind {
    NodeKind::Form(plugin_sdk::Form {
        label: label.into(),
        submit: "Save".into(),
        required: Vec::new(),
    })
}

/// A form that waits for its input `input` to hold text.
fn requiring(label: &str, input: &str) -> NodeKind {
    NodeKind::Form(plugin_sdk::Form {
        label: label.into(),
        submit: "Save".into(),
        required: vec![input.into()],
    })
}

plugin_sdk::export!(Probe);
