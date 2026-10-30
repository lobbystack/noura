//! Seam 1: web search and page reads for the AI, with a fake provider and
//! a fake web provider. You add a Brave Search key in Settings, which
//! goes only to the credential store, and allow web search for the
//! workspace in Noura's own prompt, apart from a provider's consent.
//! Until both are there, web calls fail closed and the fake web provider
//! gets nothing. Once they are, the model's searches and page reads reach
//! it, and the chat records each call and its sources. A plugin's search
//! or page read goes out only to answer a call its own model made, for
//! the query or address the model named. Revoking web
//! consent fails the next call and the chat goes on, and failures reach
//! the chat as public errors without the key or what Brave Search
//! answered.

mod support;

use std::sync::{Arc, Mutex, OnceLock};

use app_core::{App, ViewKind, ViewNode};
use local_core::AiToolCall;
use serde_json::{Value, json};
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins::{self, PluginFolder},
    web::{FakeWeb, FakeWebClient, RAW_WEB_FAILURE, WebCall},
};

const KEY: &str = "BSA-live-0123456789abcdef";

/// A workspace with the AI plugin on and one provider, talking to a fake
/// provider and a fake web provider. The app comes first, so it closes
/// before its folders go.
struct Searching {
    app: App,
    provider: Arc<FakeProvider>,
    web: Arc<FakeWeb>,
    id: String,
    system: Arc<FakeSystem>,
    _plugin: PluginFolder,
    _tasks: Option<PluginFolder>,
    fixture: Fixture,
}

/// What a test sets up before the chat.
#[derive(Clone, Copy)]
struct Setup {
    key: Option<&'static str>,
    web_consent: bool,
    provider_consent: bool,
}

const READY: Setup = Setup {
    key: Some(KEY),
    web_consent: true,
    provider_consent: true,
};

impl Searching {
    fn new(setup: Setup) -> Option<Self> {
        Self::with(setup, false)
    }

    /// The same, with the Tasks plugin on too when `tasks` says so.
    fn with(setup: Setup, tasks: bool) -> Option<Self> {
        let fixture = Fixture::new(&[]);
        let system = FakeSystem::new();
        let mut app = fixture.app_with(system.clone());
        let tasks = if tasks {
            Some(support::tasks::with_tasks(&mut app)?)
        } else {
            None
        };
        let provider = FakeProvider::new();
        let web = FakeWeb::new();
        app.set_web_client(Arc::new(FakeWebClient(web.clone())));
        let plugin = ai::with_ai(&mut app, &provider)?;
        let id = app
            .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
        if setup.provider_consent {
            app.grant_ai_consent(&id).unwrap();
        }
        if let Some(key) = setup.key {
            app.save_web_search_key(key).unwrap();
        }
        if setup.web_consent {
            app.ask_web_consent().unwrap();
            app.allow_web_consent();
        }
        support::settle(&mut app);
        Some(Self {
            app,
            provider,
            web,
            id,
            system,
            _plugin: plugin,
            _tasks: tasks,
            fixture,
        })
    }

    fn send(&mut self, text: &str) {
        let id = self.id.clone();
        ai::send(&mut self.app, text, &id);
    }

    /// Wait until the fake provider has `count` requests.
    fn requests(&mut self, count: usize) {
        let provider = self.provider.clone();
        ai::wait_until(&mut self.app, &format!("request {count} leaves"), |_| {
            provider.requests().len() >= count
        });
    }

    /// End request `index`'s answer with a call to `tool`.
    fn call(&mut self, index: usize, call_id: &str, tool: &str, arguments: Value) {
        self.provider.call_tools(
            index,
            vec![AiToolCall {
                call_id: call_id.into(),
                name: tool.into(),
                arguments,
            }],
        );
        self.requests(index + 2);
    }

    /// The last result request `index` carried for the call `call_id`, as
    /// JSON.
    fn result(&self, index: usize, call_id: &str) -> Value {
        let prefix = format!("tool: result {call_id} ");
        let line = self.provider.requests()[index]
            .messages
            .iter()
            .rev()
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
            .unwrap_or_else(|| panic!("request {index} carries no result for {call_id}"));
        serde_json::from_str(&line).unwrap()
    }

    /// Answer request `index` with `text` and wait for the answer to end.
    fn answer(&mut self, index: usize, text: &str) {
        self.provider.say(index, text);
        self.provider.finish(index);
        ai::wait_until(&mut self.app, "the answer ends", |app| {
            !ai::buttons(app).contains(&"Stop".to_owned())
        });
    }

    /// The chat's message files' text.
    fn files(&self) -> Vec<String> {
        ai::chat_files(&self.fixture.root)
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }
}

fn search(query: &str) -> Value {
    json!({ "query": query })
}

/// The labels of the list items on the AI page, top to bottom.
fn items(app: &App) -> Vec<(String, Option<String>)> {
    fn under(node: &ViewNode, found: &mut Vec<(String, Option<String>)>) {
        if let ViewKind::Item { label, detail, .. } = &node.kind {
            found.push((label.clone(), detail.clone()));
        }
        for child in &node.children {
            under(child, found);
        }
    }
    let mut found = Vec::new();
    under(ai::page(app).root(), &mut found);
    found
}

#[test]
fn a_scripted_search_gets_titles_addresses_and_snippets_and_the_chat_records_each_call() {
    let Some(mut chat) = Searching::new(READY) else {
        return;
    };

    chat.send("Find a tomato guide");
    chat.requests(1);
    assert_eq!(
        chat.provider.requests()[0].tools,
        ["web_search", "web_fetch"]
    );

    chat.provider.say(0, "Let me look.");
    chat.call(0, "call_1", "web_search", search("tomato care"));
    assert_eq!(chat.web.calls(), [WebCall::Search("tomato care".into())]);
    assert_eq!(
        chat.result(1, "call_1"),
        json!({
            "query": "tomato care",
            "results": [
                {
                    "title": "tomato care guide",
                    "url": "https://garden.example.com/tomatoes",
                    "snippet": "Water and feed tomatoes weekly."
                },
                {
                    "title": "Pruning",
                    "url": "https://notes.example.com/prune",
                    "snippet": "Pinch the suckers."
                }
            ]
        })
    );
    assert_eq!(
        chat.provider.requests()[1].tools,
        ["web_search", "web_fetch"]
    );

    chat.call(
        1,
        "call_2",
        "web_fetch",
        json!({ "url": "https://garden.example.com/tomatoes" }),
    );
    assert_eq!(
        chat.web.calls()[1],
        WebCall::Fetch("https://garden.example.com/tomatoes".into())
    );
    assert_eq!(
        chat.result(2, "call_2"),
        json!({
            "url": "https://garden.example.com/tomatoes",
            "contentType": "text/html",
            "truncated": false,
            "text": "Growing tomatoes\n\nPlant them in full sun."
        })
    );
    chat.answer(2, "Water them weekly.");

    // The chat's files hold each call as a completed tool call and its
    // result, sources included, in today's chat format.
    let files = chat.files();
    let calls: Vec<&String> = files
        .iter()
        .filter(|text| text.contains("kind: tool-call"))
        .collect();
    assert_eq!(calls.len(), 2, "{files:?}");
    for (call, tool) in [("call_1", "web_search"), ("call_2", "web_fetch")] {
        let message = calls
            .iter()
            .find(|text| text.contains(&format!("tool_call_id: {call}")))
            .unwrap();
        assert!(message.contains(&format!("tool_name: {tool}")), "{message}");
        assert!(message.contains("status: completed"), "{message}");
    }
    let results: Vec<&String> = files
        .iter()
        .filter(|text| text.contains("kind: tool-result"))
        .collect();
    assert_eq!(results.len(), 2, "{files:?}");
    assert!(
        results
            .iter()
            .any(|text| text.contains("tool_name: web_search")
                && text.contains("https://garden.example.com/tomatoes")
                && text.contains("https://notes.example.com/prune")
                && text.contains("Pinch the suckers.")),
        "{results:?}"
    );

    // The page shows what the model searched for and read, and the
    // sources it found.
    let lines = ai::lines(&chat.app);
    assert!(
        lines.contains(&"Searched the web for \u{201c}tomato care\u{201d}.".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"Read https://garden.example.com/tomatoes.".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"md: Water them weekly.".to_owned()),
        "{lines:?}"
    );
    assert_eq!(
        items(&chat.app),
        [
            (
                "tomato care guide".to_owned(),
                Some("https://garden.example.com/tomatoes".to_owned())
            ),
            (
                "Pruning".to_owned(),
                Some("https://notes.example.com/prune".to_owned())
            ),
        ]
    );
}

/// The model sees the workspace's tools beside the web tools in every
/// request. The AI plugin answers a turn of web calls itself, with no
/// prompt, and hands a turn with a workspace tool call to Noura, which
/// asks you first. Noura runs only workspace tools, so a web call in that
/// turn gets `tool-not-found`.
#[test]
fn web_calls_skip_the_tool_prompt_and_workspace_tools_stay_offered_beside_them() {
    let Some(mut chat) = Searching::with(READY, true) else {
        return;
    };
    let offered = |chat: &Searching, index: usize| {
        let tools = chat.provider.requests()[index].tools.clone();
        for name in [
            "tasks__create",
            "tasks__complete",
            "web_search",
            "web_fetch",
        ] {
            assert!(tools.iter().any(|tool| tool == name), "{name}: {tools:?}");
        }
    };
    chat.send("Find a tomato guide and add a task");
    chat.requests(1);
    offered(&chat, 0);

    chat.call(0, "call_1", "web_search", search("tomato care"));
    assert_eq!(chat.web.calls(), [WebCall::Search("tomato care".into())]);
    assert!(chat.result(1, "call_1")["results"].is_array());
    assert!(chat.app.ai_tool_prompt().is_none());
    offered(&chat, 1);

    chat.provider.call_tools(
        1,
        vec![
            AiToolCall {
                call_id: "call_2".into(),
                name: "tasks__create".into(),
                arguments: json!({ "title": "Water the tomatoes" }),
            },
            AiToolCall {
                call_id: "call_3".into(),
                name: "web_search".into(),
                arguments: search("tomato pruning"),
            },
        ],
    );
    let prompt = ai::tool_prompt(&mut chat.app);
    assert_eq!(prompt.tool, "create");
    chat.app.answer_ai_tool(&prompt, true);
    chat.requests(3);
    assert!(chat.result(2, "call_2").get("error").is_none());
    assert_eq!(chat.result(2, "call_3")["error"]["code"], "tool-not-found");
    assert_eq!(chat.web.calls().len(), 1);
    offered(&chat, 2);
    chat.answer(2, "Done.");
    assert!(
        chat.files()
            .iter()
            .any(|text| text.contains("tool_name: tasks__create")),
    );
}

/// Gemini's adapter numbers its calls again in each answer, so a run's
/// second search can carry the first one's call ID.
#[test]
fn a_model_that_gives_each_turns_calls_the_same_ids_gets_each_answered() {
    let Some(mut chat) = Searching::new(READY) else {
        return;
    };
    chat.send("Find a tomato guide");
    chat.requests(1);

    chat.call(0, "call#web_search#0", "web_search", search("tomato care"));
    assert!(chat.result(1, "call#web_search#0")["results"].is_array());
    chat.call(
        1,
        "call#web_search#0",
        "web_search",
        search("tomato pruning"),
    );
    assert_eq!(
        chat.result(2, "call#web_search#0")["query"],
        "tomato pruning"
    );
    chat.answer(2, "Water them weekly.");

    assert_eq!(
        chat.web.calls(),
        [
            WebCall::Search("tomato care".into()),
            WebCall::Search("tomato pruning".into())
        ]
    );
    assert!(ai::lines(&chat.app).contains(&"md: Water them weekly.".to_owned()));
}

/// Press the button `node` of the probe installed as `plugin`, and return
/// the lines its `events` page added.
fn press(app: &mut App, plugin: &str, node: &str) -> Vec<String> {
    let events = format!("plugin:{plugin}:events");
    let seen = plugins::texts(app, &events).unwrap().len();
    app.press_plugin_button(&format!("plugin:{plugin}:hello"), node);
    support::settle(app);
    plugins::texts(app, &events).unwrap()[seen..].to_vec()
}

/// The probe plugin installed as `id`, holding `ai`.
fn probe(app: &mut App, id: &str) -> Option<PluginFolder> {
    let folder = plugins::folder_for("probe_plugin", id, &["name = \"views\"", "name = \"ai\""])?;
    plugins::install_and_enable(app, folder.path(), id);
    Some(folder)
}

/// A plugin holding `ai` can't use web search to send what it likes to a
/// site of its choosing: Noura searches and reads pages only to answer a
/// call the plugin's own model made, for the query or address it named.
#[test]
fn a_plugin_reaches_the_web_only_to_answer_its_own_models_call() {
    let Some(mut chat) = Searching::new(READY) else {
        return;
    };
    let (Some(_asker), Some(_other)) =
        (probe(&mut chat.app, "asker"), probe(&mut chat.app, "other"))
    else {
        return;
    };
    let refused = |plugin: &str| {
        format!(
            "hello ai web-failed web_call_not_requested Noura searches the web or reads a page \
             only when the model asks for that query or address. ({plugin})"
        )
    };
    let shown = |lines: Vec<String>, plugin: &str| {
        lines
            .into_iter()
            .map(|line| format!("{line} ({plugin})"))
            .collect::<Vec<_>>()
    };

    // No model asked for anything yet.
    for plugin in ["asker", "other"] {
        assert_eq!(
            shown(
                press(&mut chat.app, plugin, "ai:web-search:tomato care"),
                plugin
            ),
            [refused(plugin)]
        );
        assert_eq!(
            shown(
                press(
                    &mut chat.app,
                    plugin,
                    "ai:web-fetch:https://collector.example.com/?chat=secret"
                ),
                plugin
            ),
            [refused(plugin)]
        );
    }

    // The asker's model asks for a search.
    let created = press(&mut chat.app, "asker", "ai:create-chat");
    let id = created[0]
        .strip_prefix("hello ai chat ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_owned();
    let provider = chat.id.clone();
    let sent = press(
        &mut chat.app,
        "asker",
        &format!("ai:send:{id}:{provider}:Find tomatoes"),
    );
    let run = sent[0]
        .strip_prefix("hello ai run ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_owned();
    chat.requests(1);
    chat.provider.call_tools(
        0,
        vec![AiToolCall {
            call_id: "call_1".into(),
            name: "web_search".into(),
            arguments: search("tomato care"),
        }],
    );
    let completed = format!("hello ai run {run} RunStatus::Completed");
    let mut line = String::new();
    for _ in 0..200 {
        line = press(&mut chat.app, "asker", &format!("ai:read-run:{run}"))
            .pop()
            .unwrap_or_default();
        if line.starts_with(&completed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(line.starts_with(&completed), "{line}");

    // Only the asker, and only for that query, reaches the web.
    assert_eq!(
        shown(
            press(&mut chat.app, "other", "ai:web-search:tomato care"),
            "other"
        ),
        [refused("other")]
    );
    assert_eq!(
        shown(
            press(&mut chat.app, "asker", "ai:web-search:tomato secrets"),
            "asker"
        ),
        [refused("asker")]
    );
    assert_eq!(
        shown(
            press(
                &mut chat.app,
                "asker",
                "ai:web-fetch:https://garden.example.com/tomatoes"
            ),
            "asker"
        ),
        [refused("asker")]
    );
    assert!(chat.web.calls().is_empty());
    let found = press(&mut chat.app, "asker", "ai:web-search:tomato care");
    assert!(
        found[0].starts_with("hello ai web-result tomato care guide https://garden.example.com/"),
        "{found:?}"
    );
    assert_eq!(chat.web.calls(), [WebCall::Search("tomato care".into())]);
}

#[test]
fn without_a_key_or_web_consent_web_calls_fail_closed_and_reach_no_web_provider() {
    for (key, web_consent, code) in [
        (None, false, "web_search_key_missing"),
        (None, true, "web_search_key_missing"),
        (Some(KEY), false, "web_consent_required"),
    ] {
        let Some(mut chat) = Searching::new(Setup {
            key,
            web_consent,
            provider_consent: true,
        }) else {
            return;
        };

        chat.send("Find a tomato guide");
        chat.requests(1);
        // The model isn't offered the web tools...
        assert!(chat.provider.requests()[0].tools.is_empty(), "{code}");
        // ...and when it asks for one anyway, the call fails closed.
        chat.call(0, "call_1", "web_search", search("tomato care"));
        assert_eq!(chat.result(1, "call_1")["error"]["code"], code);
        chat.call(
            1,
            "call_2",
            "web_fetch",
            json!({ "url": "https://garden.example.com/" }),
        );
        assert_eq!(chat.result(2, "call_2")["error"]["code"], code);
        chat.answer(2, "I can't search the web here.");

        assert!(chat.web.calls().is_empty(), "{code}");
        assert!(chat.web.keys().is_empty(), "{code}");
    }
}

#[test]
fn web_consent_alone_sends_nothing_to_an_ai_provider() {
    let Some(mut chat) = Searching::new(Setup {
        provider_consent: false,
        ..READY
    }) else {
        return;
    };
    assert!(chat.app.web_search().consented);

    chat.send("Find a tomato guide");

    assert!(chat.app.ai_consent_prompt().is_some());
    assert!(chat.provider.requests().is_empty());
    assert!(chat.web.calls().is_empty());
    assert!(ai::chat_files(&chat.fixture.root).is_empty());
}

#[test]
fn web_consent_has_its_own_prompt_and_stays_on_this_device_apart_from_provider_consent() {
    let Some(mut chat) = Searching::new(Setup {
        web_consent: false,
        ..READY
    }) else {
        return;
    };
    assert_eq!(chat.app.web_consent_prompt(), None);

    // Saying no stores nothing.
    chat.app.ask_web_consent().unwrap();
    let prompt = chat.app.web_consent_prompt().expect("Noura asks");
    assert_eq!(prompt.provider_name, "Brave Search");
    assert_eq!(prompt.workspace, "Garden");
    chat.app.deny_web_consent();
    assert_eq!(chat.app.web_consent_prompt(), None);
    assert!(!chat.app.web_search().consented);

    chat.app.ask_web_consent().unwrap();
    chat.app.allow_web_consent();
    assert_eq!(chat.app.web_consent_prompt(), None);
    assert!(chat.app.web_search().consented);

    // The consent lives in this device's app data, in a file of its own,
    // and the workspace holds none.
    let workspace = chat.app.workspace().unwrap().id().to_owned();
    let app_data = chat.fixture.app_data.path();
    let web_consents = std::fs::read_to_string(app_data.join("ai/web-consents.json")).unwrap();
    assert!(web_consents.contains(&workspace), "{web_consents}");
    assert!(web_consents.contains("brave-search"), "{web_consents}");
    let provider_consents = std::fs::read_to_string(app_data.join("ai/consents.json")).unwrap();
    assert!(
        !provider_consents.contains("brave-search"),
        "{provider_consents}"
    );
    assert!(chat.fixture.snapshot().iter().all(|(_, bytes)| {
        bytes
            .as_deref()
            .is_none_or(|bytes| !String::from_utf8_lossy(bytes).contains("brave-search"))
    }),);

    // Revoking the provider's consent leaves web consent, and the reverse.
    let id = chat.id.clone();
    chat.app.revoke_ai_consent(&id).unwrap();
    assert!(chat.app.web_search().consented);
    chat.app.grant_ai_consent(&id).unwrap();
    chat.app.revoke_web_consent().unwrap();
    assert!(!chat.app.web_search().consented);
    assert!(chat.app.ai_providers()[0].consented);

    // A relaunch reads the consent back.
    chat.app.ask_web_consent().unwrap();
    chat.app.allow_web_consent();
    let system = chat.system.clone();
    let Searching {
        app,
        fixture,
        _plugin,
        ..
    } = chat;
    drop(app);
    let app = fixture.app_with(system);
    assert!(app.web_search().consented);
    assert!(app.web_search().has_key);
}

#[test]
fn revoking_web_consent_fails_the_next_web_call_and_the_chat_goes_on() {
    let Some(mut chat) = Searching::new(READY) else {
        return;
    };
    chat.send("Find a tomato guide");
    chat.requests(1);
    chat.call(0, "call_1", "web_search", search("tomato care"));
    assert!(chat.result(1, "call_1")["results"].is_array());

    chat.app.revoke_web_consent().unwrap();
    support::settle(&mut chat.app);
    assert!(!chat.app.web_search().consented);
    chat.call(1, "call_2", "web_search", search("more tomatoes"));

    assert_eq!(
        chat.result(2, "call_2")["error"]["code"],
        "web_consent_required"
    );
    assert!(chat.provider.requests()[2].tools.is_empty());
    assert_eq!(chat.web.calls(), [WebCall::Search("tomato care".into())]);
    chat.answer(2, "Water them weekly.");
    assert!(ai::lines(&chat.app).contains(&"md: Water them weekly.".to_owned()));

    // The next message goes out without the web tools.
    chat.send("Thanks");
    chat.requests(4);
    assert!(chat.provider.requests()[3].tools.is_empty());
    assert_eq!(chat.web.calls().len(), 1);
}

/// A log that keeps every line the app writes while the tests run. Each
/// test of this file that reads it looks only for secrets, which no test
/// may log.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The log every thread of this test binary writes to.
fn log() -> Captured {
    static LOG: OnceLock<Captured> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = Captured::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        log
    })
    .clone()
}

/// Every file under `folder` that holds `secret`.
fn files_holding(folder: &std::path::Path, secret: &str, found: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_holding(&path, secret, found);
        } else if std::fs::read(&path)
            .unwrap_or_default()
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
        {
            found.push(path);
        }
    }
}

/// Fails if the page, the workspace (index included), the app data or
/// the log hold any of `secrets`.
fn assert_nowhere(chat: &Searching, log: &Captured, secrets: &[&str]) {
    let shown = ai::lines(&chat.app);
    let logged = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    for secret in secrets {
        assert!(shown.iter().all(|line| !line.contains(secret)), "{shown:?}");
        let mut found = Vec::new();
        for folder in [chat.fixture.app_data.path(), chat.fixture.root.as_path()] {
            files_holding(folder, secret, &mut found);
        }
        assert_eq!(found, Vec::<std::path::PathBuf>::new(), "{secret}");
        assert!(!logged.contains(secret), "{secret} is in the log");
    }
}

#[test]
fn the_brave_search_key_goes_only_to_the_credential_store_and_the_web_provider() {
    let log = log();
    let Some(mut chat) = Searching::new(Setup { key: None, ..READY }) else {
        return;
    };
    assert!(!chat.app.web_search().has_key);

    chat.app.save_web_search_key(KEY).unwrap();

    assert!(chat.app.web_search().has_key);
    assert_eq!(chat.system.credentials.secrets(), [KEY.to_owned()]);
    let settings =
        std::fs::read_to_string(chat.fixture.app_data.path().join("ai/web.json")).unwrap();
    assert!(settings.contains("credential_"), "{settings}");

    chat.send("Find a tomato guide");
    chat.requests(1);
    chat.call(0, "call_1", "web_search", search("tomato care"));
    chat.answer(1, "Water them weekly.");
    assert_eq!(chat.web.keys(), [KEY.to_owned()]);
    assert_nowhere(&chat, &log, &[KEY]);

    chat.app.remove_web_search_key().unwrap();
    assert!(!chat.app.web_search().has_key);
    assert!(chat.system.credentials.secrets().is_empty());
}

#[test]
fn brave_search_failures_reach_the_chat_as_public_errors_without_the_key_or_its_answer() {
    let log = log();
    let Some(mut chat) = Searching::new(READY) else {
        return;
    };
    chat.send("Find a tomato guide");
    chat.requests(1);

    chat.call(0, "call_1", "web_search", search("rejected key"));
    assert_eq!(
        chat.result(1, "call_1"),
        json!({ "error": {
            "code": "web_search_key_rejected",
            "message": "Brave Search didn't accept the key. Check it in Settings, under Web search."
        }})
    );
    chat.call(1, "call_2", "web_search", search("rate limited"));
    assert_eq!(
        chat.result(2, "call_2"),
        json!({ "error": {
            "code": "web_search_rate_limited",
            "message": "Brave Search's limit for your key was reached. Try again in a moment."
        }})
    );
    chat.answer(2, "Brave Search isn't answering.");

    let lines = ai::lines(&chat.app);
    assert!(
        lines.contains(
            &"The web call failed: Brave Search didn't accept the key. Check it in Settings, \
              under Web search."
                .to_owned()
        ),
        "{lines:?}"
    );
    let results: Vec<String> = chat
        .files()
        .into_iter()
        .filter(|text| text.contains("kind: tool-result"))
        .collect();
    assert!(
        results
            .iter()
            .any(|text| text.contains("web_search_rate_limited")),
        "{results:?}"
    );
    assert_nowhere(
        &chat,
        &log,
        &[
            KEY,
            RAW_WEB_FAILURE,
            "SUBSCRIPTION_TOKEN_INVALID",
            "BSA-raw",
        ],
    );
}
