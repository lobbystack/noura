//! Seam 1: tools in AI chats. A plugin offers tools with a request, the
//! run lists the calls the model asked for once Noura wrote each one to
//! the chat, and the plugin's results go back to the model in the same
//! run, written to the chat before the next request leaves. The AI page
//! offers the tools that plugins on in the workspace register, such as
//! the Tasks plugin's, and each call waits for Noura's prompt: it runs
//! only once you allow it, and a denial, Stop or another workspace keeps
//! it from running.

mod support;

use std::{sync::Arc, time::Duration};

use app_core::App;
use local_core::AiToolCall;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins, tasks,
};

/// Press the probe's button `node` on its page `hello`, and return the
/// lines its `events` page added.
fn press(app: &mut App, node: &str) -> Vec<String> {
    let events = "plugin:chatter:events";
    let seen = plugins::texts(app, events).unwrap().len();
    app.press_plugin_button("plugin:chatter:hello", node);
    support::settle(app);
    plugins::texts(app, events).unwrap()[seen..].to_vec()
}

/// The ID that follows `prefix` in `line`.
fn id_after<'a>(line: &'a str, prefix: &str) -> &'a str {
    let rest = &line[line.find(prefix).expect(prefix) + prefix.len()..];
    rest.split_whitespace().next().unwrap()
}

#[test]
fn a_run_lists_the_tool_calls_and_carries_their_results_to_the_model() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::new());
    let provider = FakeProvider::new();
    let Some(probe) = plugins::folder_for(
        "probe_plugin",
        "chatter",
        &["name = \"views\"", "name = \"ai\""],
    ) else {
        return;
    };
    app.set_ai_client(provider.clone());
    plugins::install_and_enable(&mut app, probe.path(), "chatter");
    let id = app
        .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
        .unwrap();
    app.grant_ai_consent(&id).unwrap();

    let created = press(&mut app, "ai:create-chat");
    let chat = id_after(&created[0], "hello ai chat ").to_owned();
    let sent = press(&mut app, &format!("ai:send-with-tool:{chat}:{id}:Echo hi"));
    let run = id_after(&sent[0], "hello ai run ").to_owned();
    plugins::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert_eq!(provider.requests()[0].tools, ["echo"]);

    provider.say(0, "Let me echo that.");
    provider.call_tools(
        0,
        vec![AiToolCall {
            call_id: "call_1".into(),
            name: "echo".into(),
            arguments: serde_json::json!({ "text": "hi" }),
        }],
    );
    let completed = format!("hello ai run {run} RunStatus::Completed message");
    let mut line = String::new();
    for _ in 0..200 {
        line = press(&mut app, &format!("ai:read-run:{run}"))
            .pop()
            .unwrap_or_default();
        if line.starts_with(&completed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(line.starts_with(&completed), "{line}");
    assert!(
        line.contains("calls [call_1=echo({\"text\":\"hi\"})]"),
        "{line}"
    );
    let call = ai::chat_files(&fixture.root)
        .into_iter()
        .map(|(_, text)| text)
        .find(|text| text.contains("kind: tool-call"))
        .expect("the call is in the chat");
    assert!(call.contains("status: in-progress"), "{call}");
    assert!(call.contains("tool_call_id: call_1") && call.contains("tool_name: echo"));

    let answered = press(
        &mut app,
        &format!("ai:answer-tools:{run}:{{\"echoed\":\"hi\"}}"),
    );
    assert!(answered[0].contains("RunStatus::Streaming"), "{answered:?}");
    plugins::wait_until(&mut app, "the next request leaves", |_| {
        provider.requests().len() == 2
    });
    assert_eq!(
        provider.requests()[1].messages,
        [
            "user: Echo hi",
            "assistant: Let me echo that. | call call_1 echo {\"text\":\"hi\"}",
            "tool: result call_1 {\"echoed\":\"hi\"}",
        ]
    );
    let files: Vec<String> = ai::chat_files(&fixture.root)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    let call = files
        .iter()
        .find(|text| text.contains("kind: tool-call"))
        .unwrap();
    assert!(call.contains("status: completed"), "{call}");
    let result = files
        .iter()
        .find(|text| text.contains("kind: tool-result"))
        .expect("the result is in the chat");
    assert!(result.contains("{\"echoed\":\"hi\"}"), "{result}");

    provider.say(1, "hi");
    provider.finish(1);
    let done = format!("hello ai run {run} RunStatus::Completed message");
    let mut line = String::new();
    for _ in 0..200 {
        line = press(&mut app, &format!("ai:read-run:{run}"))
            .pop()
            .unwrap_or_default();
        if line.starts_with(&done) && line.contains("calls []") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(line.ends_with("calls [] text hi"), "{line}");
}

/// The Tasks plugin and the AI plugin on, with the provider Local allowed
/// for the workspace. The app comes first, so it closes before its
/// folders go.
struct Tooled {
    app: App,
    provider: Arc<FakeProvider>,
    id: String,
    _plugins: (plugins::PluginFolder, plugins::PluginFolder),
    fixture: Fixture,
}

impl Tooled {
    fn new() -> Option<Self> {
        let fixture = Fixture::new(&[]);
        let mut app = fixture.app_with(FakeSystem::new());
        let provider = FakeProvider::new();
        let tasks = tasks::with_tasks(&mut app)?;
        let chat = ai::with_ai(&mut app, &provider)?;
        let id = app
            .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
        app.grant_ai_consent(&id).unwrap();
        support::settle(&mut app);
        Some(Self {
            app,
            provider,
            id,
            _plugins: (tasks, chat),
            fixture,
        })
    }

    /// Send `text` from the AI page and wait for request `index` to leave.
    fn send(&mut self, text: &str, index: usize) {
        ai::send(&mut self.app, text, &self.id);
        self.wait_for_request(index);
    }

    fn wait_for_request(&mut self, index: usize) {
        let provider = self.provider.clone();
        ai::wait_until(&mut self.app, &format!("request {index} leaves"), |_| {
            provider.requests().len() > index
        });
    }

    /// End request `index` with one call to the tool `name`.
    fn call(&self, index: usize, call: &str, name: &str, arguments: serde_json::Value) {
        self.provider.call_tools(
            index,
            vec![AiToolCall {
                call_id: call.into(),
                name: name.into(),
                arguments,
            }],
        );
    }

    /// End request `index` with a call that creates the task Water the
    /// tomatoes.
    fn create(&self, index: usize, call: &str) {
        self.call(
            index,
            call,
            "tasks__create",
            serde_json::json!({ "title": "Water the tomatoes" }),
        );
    }

    fn task_file(&self) -> String {
        String::from_utf8(self.fixture.read("tasks/Water the tomatoes.md")).unwrap()
    }

    fn has_task(&self) -> bool {
        self.fixture.exists("tasks/Water the tomatoes.md")
    }

    fn result(&mut self, call: &str) -> serde_json::Value {
        ai::tool_result(&mut self.app, &self.fixture.root, call)
    }

    fn call_file(&mut self, call: &str) -> String {
        ai::tool_message(&mut self.app, &self.fixture.root, "tool-call", call)
    }

    /// The tool-call message of `call`, once its status is `status`.
    fn call_ending(&mut self, call: &str, status: &str) -> String {
        let root = self.fixture.root.clone();
        let read = |root: &std::path::Path| {
            ai::message_files(root).into_iter().find(|text| {
                ai::frontmatter(text, "kind").as_deref() == Some("tool-call")
                    && ai::frontmatter(text, "tool_call_id").as_deref() == Some(call)
                    && ai::frontmatter(text, "status").as_deref() == Some(status)
            })
        };
        ai::wait_until(&mut self.app, &format!("{call} is {status}"), |_| {
            read(&root).is_some()
        });
        read(&root).unwrap()
    }
}

#[test]
fn the_model_creates_and_completes_a_real_task_once_you_allow_each_call() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes, then check it off", 0);
    let offered = tooled.provider.requests()[0].tools.clone();
    assert!(
        offered.contains(&"tasks__create".to_owned())
            && offered.contains(&"tasks__complete".to_owned()),
        "{offered:?}"
    );

    tooled.create(0, "call_1");
    let prompt = ai::tool_prompt(&mut tooled.app);
    assert_eq!(prompt.tool, "create");
    assert_eq!(prompt.plugin_id, "tasks");
    assert_eq!(prompt.plugin_name, "Tasks");
    assert_eq!(prompt.input, "{\n  \"title\": \"Water the tomatoes\"\n}");
    assert!(!tooled.has_task(), "nothing runs before you allow it");
    tooled.app.answer_ai_tool(&prompt, true);

    let created = tooled.result("call_1");
    let task = created["id"].as_str().unwrap().to_owned();
    let revision = created["revision"].as_str().unwrap().to_owned();
    assert!(task.starts_with("task_"), "{created}");
    assert!(tooled.task_file().contains("status: todo"));

    tooled.wait_for_request(1);
    let carried = tooled.provider.requests()[1].messages.clone();
    assert_eq!(
        carried[0],
        "user: Add a task to water the tomatoes, then check it off"
    );
    assert!(carried[1].starts_with("assistant: call call_1 tasks__create"));
    assert!(
        carried[2].starts_with("tool: result call_1 {"),
        "{carried:?}"
    );
    tooled.call(
        1,
        "call_2",
        "tasks__complete",
        serde_json::json!({ "id": task, "expected_revision": revision }),
    );
    let prompt = ai::tool_prompt(&mut tooled.app);
    assert_eq!(prompt.tool, "complete");
    tooled.app.answer_ai_tool(&prompt, true);

    let completed = tooled.result("call_2");
    assert_eq!(completed["properties"]["status"], "done", "{completed}");
    assert!(tooled.task_file().contains("status: done"));
    tooled.wait_for_request(2);
    tooled.provider.say(2, "Done.");
    tooled.provider.finish(2);
    ai::wait_until(&mut tooled.app, "the answer ends", |app| {
        ai::lines(app).contains(&"md: Done.".to_owned()) && ai::buttons(app).is_empty()
    });
}

#[test]
fn a_denied_call_never_runs_and_the_model_hears_so() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes", 0);
    tooled.create(0, "call_1");
    let prompt = ai::tool_prompt(&mut tooled.app);
    tooled.app.answer_ai_tool(&prompt, false);

    assert_eq!(
        tooled.result("call_1"),
        serde_json::json!({ "denied": true })
    );
    tooled.call_ending("call_1", "completed");
    assert!(!tooled.has_task());
    // The model hears the denial and answers again.
    tooled.wait_for_request(1);
    assert_eq!(tooled.app.ai_tool_prompt(), None);
    assert_eq!(
        tooled.provider.requests()[1].messages[2],
        "tool: result call_1 {\"denied\":true}"
    );
}

#[test]
fn stop_while_noura_asks_denies_the_call_and_ends_the_run() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes", 0);
    tooled.create(0, "call_1");
    ai::tool_prompt(&mut tooled.app);
    assert!(
        ai::lines(&tooled.app).contains(
            &"Noura is asking you whether to run a tool the model wants to use.".to_owned()
        )
    );

    ai::press(&mut tooled.app, "stop");

    assert_eq!(
        tooled.result("call_1"),
        serde_json::json!({ "denied": true })
    );
    ai::wait_until(&mut tooled.app, "the chat takes a message again", |app| {
        ai::buttons(app).is_empty() && app.ai_tool_prompt().is_none()
    });
    assert!(!tooled.has_task());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(tooled.provider.requests().len(), 1, "no request follows");
}

#[test]
fn opening_another_workspace_while_noura_asks_denies_the_call() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes", 0);
    tooled.create(0, "call_1");
    ai::tool_prompt(&mut tooled.app);

    let other = tooled.fixture.outside();
    tooled.app.open_folder(other.clone());
    let other = other.canonicalize().unwrap();
    ai::wait_until(&mut tooled.app, "the other workspace opens", |app| {
        app.workspace().is_some_and(|workspace| {
            std::path::Path::new(workspace.root()).canonicalize().ok() == Some(other.clone())
        })
    });

    assert_eq!(tooled.app.ai_tool_prompt(), None);
    assert_eq!(
        tooled.result("call_1"),
        serde_json::json!({ "denied": true })
    );
    assert!(!tooled.has_task());
    assert_eq!(tooled.provider.requests().len(), 1);
}

#[test]
fn the_call_is_on_disk_in_progress_before_it_runs_and_its_result_after() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes", 0);
    tooled.create(0, "call_1");
    let prompt = ai::tool_prompt(&mut tooled.app);

    let call = tooled.call_file("call_1");
    assert_eq!(
        ai::frontmatter(&call, "status").as_deref(),
        Some("in-progress")
    );
    assert_eq!(
        ai::frontmatter(&call, "tool_name").as_deref(),
        Some("tasks__create")
    );
    assert!(
        !ai::message_files(&tooled.fixture.root)
            .iter()
            .any(|text| ai::frontmatter(text, "kind").as_deref() == Some("tool-result")),
        "no result before the call runs"
    );

    tooled.app.answer_ai_tool(&prompt, true);
    tooled.result("call_1");
    assert!(tooled.has_task());
    tooled.call_ending("call_1", "completed");
}

#[test]
fn a_tool_write_with_a_stale_revision_fails_and_keeps_the_file() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes, then check it off", 0);
    tooled.create(0, "call_1");
    let prompt = ai::tool_prompt(&mut tooled.app);
    tooled.app.answer_ai_tool(&prompt, true);
    let created = tooled.result("call_1");
    let task = created["id"].as_str().unwrap().to_owned();
    let revision = created["revision"].as_str().unwrap().to_owned();

    // Another app changes the task after the model read it.
    let edited = tooled
        .task_file()
        .replace("status: todo", "status: in-progress");
    std::fs::write(
        tooled.fixture.root.join("tasks/Water the tomatoes.md"),
        &edited,
    )
    .unwrap();
    tooled.wait_for_request(1);
    tooled.call(
        1,
        "call_2",
        "tasks__complete",
        serde_json::json!({ "id": task, "expected_revision": revision }),
    );
    let prompt = ai::tool_prompt(&mut tooled.app);
    tooled.app.answer_ai_tool(&prompt, true);

    let refused = tooled.result("call_2");
    assert_eq!(refused["error"]["code"], "tool-failed", "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("revision_conflict"),
        "{refused}"
    );
    let call = tooled.call_ending("call_2", "failed");
    assert_eq!(
        ai::frontmatter(&call, "error_code").as_deref(),
        Some("tool-failed")
    );
    assert_eq!(tooled.task_file(), edited, "the outside edit stays");
}

/// The files under the chat's folders whose `kind` is `kind`, once one has
/// `status`.
fn message_with(app: &mut App, root: &std::path::Path, kind: &str, status: &str) -> String {
    let find = |root: &std::path::Path| {
        ai::message_files(root).into_iter().find(|text| {
            ai::frontmatter(text, "kind").as_deref() == Some(kind)
                && ai::frontmatter(text, "status").as_deref() == Some(status)
        })
    };
    ai::wait_until(app, &format!("a {kind} is {status}"), |_| {
        find(root).is_some()
    });
    find(root).unwrap()
}

const STOPPED: &str = "You turned off a plugin this answer relied on, so the answer stopped.";

#[test]
fn turning_off_a_plugin_mid_answer_ends_the_run_as_interrupted_and_keeps_its_files() {
    let Some(mut tooled) = Tooled::new() else {
        return;
    };
    tooled.send("Add a task to water the tomatoes", 0);
    tooled.create(0, "call_1");
    let prompt = ai::tool_prompt(&mut tooled.app);
    tooled.app.answer_ai_tool(&prompt, true);
    tooled.result("call_1");
    tooled.wait_for_request(1);
    tooled.provider.say(1, "I added");
    ai::wait_until(&mut tooled.app, "the answer streams", |app| {
        ai::lines(app).contains(&"md: I added".to_owned())
    });

    assert!(tooled.app.set_plugin_enabled("tasks", false));
    support::settle(&mut tooled.app);

    let answer = message_with(
        &mut tooled.app,
        &tooled.fixture.root,
        "assistant",
        "interrupted",
    );
    assert_eq!(
        ai::frontmatter(&answer, "error_code").as_deref(),
        Some("plugin-disabled")
    );
    assert!(answer.ends_with("\nI added\n"), "{answer}");
    ai::wait_until(&mut tooled.app, "the page says why", |app| {
        ai::lines(app).contains(&STOPPED.to_owned()) && ai::buttons(app).is_empty()
    });
    assert!(
        tooled.provider.cancelled(1),
        "the provider saw the request end"
    );
    assert!(tooled.has_task(), "the plugin's files stay");

    // The next message offers the model no Tasks tool.
    tooled.send("Thanks", 2);
    let offered = tooled.provider.requests()[2].tools.clone();
    assert!(
        !offered.iter().any(|tool| tool.starts_with("tasks__")),
        "{offered:?}"
    );
}

#[test]
fn turning_off_a_plugin_while_its_call_waits_interrupts_the_call_and_never_runs_it() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::new());
    let provider = FakeProvider::new();
    let Some(probe) = plugins::folder_for(
        "probe_plugin",
        "echoer",
        &["name = \"views\"", "name = \"tools\""],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "echoer");
    let Some(_chat) = ai::with_ai(&mut app, &provider) else {
        return;
    };
    let id = app
        .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
        .unwrap();
    app.grant_ai_consent(&id).unwrap();
    support::settle(&mut app);
    ai::send(&mut app, "Echo hi", &id);
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert_eq!(provider.requests()[0].tools, ["echoer__echo"]);
    provider.call_tools(
        0,
        vec![AiToolCall {
            call_id: "call_1".into(),
            name: "echoer__echo".into(),
            arguments: serde_json::json!({ "text": "hi" }),
        }],
    );
    let prompt = ai::tool_prompt(&mut app);
    assert_eq!(prompt.plugin_id, "echoer");

    assert!(app.set_plugin_enabled("echoer", false));
    support::settle(&mut app);

    let call = message_with(&mut app, &fixture.root, "tool-call", "interrupted");
    assert_eq!(
        ai::frontmatter(&call, "error_code").as_deref(),
        Some("plugin-disabled")
    );
    let result = ai::tool_result(&mut app, &fixture.root, "call_1");
    assert_eq!(result["error"]["code"], "plugin-disabled", "{result}");
    ai::wait_until(&mut app, "the page says why", |app| {
        ai::lines(app).contains(&STOPPED.to_owned())
    });
    assert_eq!(app.ai_tool_prompt(), None);
    assert_eq!(provider.requests().len(), 1, "no request follows");
}
