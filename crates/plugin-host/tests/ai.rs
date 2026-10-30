//! Seam 3: the host AI interface and the AI plugin in the real host. A
//! plugin reaches providers and chats only with the `ai` capability, a
//! provider's failure reaches it as a code and a public message, and the
//! AI plugin registers the chat and chat-message types with the schemas
//! the shared fixtures hold.

mod support;

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use plugin_host::{
    AiChat, AiChats, AiFailure, AiMessage, AiProvider, AiRequest, AiRetention, AiRun, AiRunStatus,
    AiToolCall, AiToolDefinition, AiToolResult, AiWebPage, AiWebResult, Capability, ObjectTypes,
    RunningPlugin,
    ServiceError, Services, read_manifest,
};
use serde_json::Value;
use support::plugins::{self, FakeObjects};
use workspace_format::{PropertySchema, WorkspaceObject};

/// The AI plugin's manifest, as its crate ships it.
const MANIFEST: &str = include_str!("../../plugins/ai/plugin.toml");

const SCHEMAS: &str =
    include_str!("../../../docs/workspace-format/fixtures/object-envelope-v1.json");

const CONFORMANCE: &str =
    include_str!("../../../docs/workspace-format/fixtures/conformance-v1.json");

/// An AI service that answers from fixed values and records each call.
#[derive(Default)]
struct FakeAi {
    calls: Mutex<Vec<String>>,
}

impl FakeAi {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }

    fn run(&self, id: &str) -> AiRun {
        AiRun {
            id: id.to_owned(),
            chat: "chat_01jbcdefghjkmnpqrstvwxyz0a".to_owned(),
            provider: "local".to_owned(),
            status: AiRunStatus::Failed,
            text: "Part of an answer".to_owned(),
            message: Some("chat-message_01jbcdefghjkmnpqrstvwxyz0b".to_owned()),
            message_written: true,
            tool_calls: vec![AiToolCall {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                arguments: r#"{"text":"hi"}"#.to_owned(),
            }],
            failure: Some(AiFailure {
                code: "provider_request_failed".to_owned(),
                message: "The AI provider request failed".to_owned(),
            }),
        }
    }
}

impl AiChats for FakeAi {
    fn providers(&self) -> Result<Vec<AiProvider>, ServiceError> {
        self.record("providers");
        Ok(vec![AiProvider {
            id: "local".to_owned(),
            name: "Local".to_owned(),
            kind: "ollama".to_owned(),
            model: "llama3".to_owned(),
        }])
    }

    fn chats(&self) -> Result<Vec<AiChat>, ServiceError> {
        self.record("chats");
        Ok(Vec::new())
    }

    fn read_chat(&self, id: &str) -> Result<(AiChat, Vec<AiMessage>), ServiceError> {
        self.record(format!("read-chat {id}"));
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn create_chat(&self) -> Result<AiChat, ServiceError> {
        self.record("create-chat");
        Ok(chat("New chat", AiRetention::Permanent))
    }

    fn rename_chat(
        &self,
        id: &str,
        title: &str,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError> {
        self.record(format!("rename-chat {id} {expected_revision} {title}"));
        Err(ServiceError::RevisionConflict(id.to_owned()))
    }

    fn set_retention(
        &self,
        id: &str,
        retention: AiRetention,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError> {
        self.record(format!(
            "set-retention {id} {expected_revision} {retention:?}"
        ));
        Ok(chat("Plan the garden", retention))
    }

    fn workspace_tools(&self) -> Result<Vec<AiToolDefinition>, ServiceError> {
        self.record("workspace-tools");
        Ok(vec![AiToolDefinition {
            name: "tasks__create".to_owned(),
            description: "Create a task.".to_owned(),
            input_schema: "{}".to_owned(),
        }])
    }

    fn send(&self, request: AiRequest) -> Result<AiRun, ServiceError> {
        self.record(format!(
            "send {} {} {} {}",
            request.chat,
            request.provider,
            request.message,
            request.tools.len()
        ));
        Ok(self.run("run_1"))
    }

    fn answer_tools(
        &self,
        run: &str,
        results: Vec<AiToolResult>,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        let results: Vec<String> = results
            .iter()
            .map(|result| format!("{}={}", result.call_id, result.content))
            .collect();
        self.record(format!(
            "answer-tools {run} {} {}",
            results.join(","),
            tools.len()
        ));
        Ok(self.run(run))
    }

    fn run_tools(&self, run: &str) -> Result<AiRun, ServiceError> {
        self.record(format!("run-tools {run}"));
        Ok(self.run(run))
    }

    fn resume(
        &self,
        chat: &str,
        provider: &str,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        self.record(format!("resume {chat} {provider} {}", tools.len()));
        Ok(self.run("run_2"))
    }

    fn read_run(&self, id: &str) -> Result<AiRun, ServiceError> {
        self.record(format!("read-run {id}"));
        Ok(self.run(id))
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        self.record(format!("stop {id}"));
        Ok(())
    }

    fn web_ready(&self) -> Result<bool, ServiceError> {
        self.record("web-ready");
        Ok(true)
    }

    fn web_search(
        &self,
        query: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<AiWebResult>, AiFailure> {
        self.record(format!("web-search {query} {max_results:?}"));
        if query == "too often" {
            return Err(AiFailure {
                code: "web_search_rate_limited".to_owned(),
                message: "Brave Search's limit for your key was reached.".to_owned(),
            });
        }
        Ok(vec![AiWebResult {
            title: "Tomatoes".to_owned(),
            url: "https://garden.example.com/tomatoes".to_owned(),
            snippet: "Grow them in sun.".to_owned(),
        }])
    }

    fn web_fetch(&self, url: &str) -> Result<AiWebPage, AiFailure> {
        self.record(format!("web-fetch {url}"));
        Ok(AiWebPage {
            url: url.to_owned(),
            content_type: "text/html".to_owned(),
            text: "Growing tomatoes".to_owned(),
            truncated: false,
        })
    }
}

/// A chat titled `title`, kept as `retention` says.
fn chat(title: &str, retention: AiRetention) -> AiChat {
    AiChat {
        id: "chat_01jbcdefghjkmnpqrstvwxyz0a".to_owned(),
        title: title.to_owned(),
        updated: "2026-10-24T10:00:00Z".to_owned(),
        revision: "rev1".to_owned(),
        retention,
    }
}

/// The probe, as a plugin holding `capabilities`, with `ai` as its AI
/// service.
fn probe(
    capabilities: &[&str],
    ai: &Arc<FakeAi>,
) -> Option<(Arc<RunningPlugin>, [tempfile::TempDir; 2])> {
    let probe = plugins::built("probe_plugin")?;
    let service: Arc<dyn AiChats> = ai.clone();
    Some(plugins::running_as(
        "chatter",
        &probe,
        capabilities,
        Services {
            ai: Some(service),
            ..Services::default()
        },
    ))
}

fn lines(plugin: &RunningPlugin, page: &str) -> Vec<String> {
    plugins::texts(&plugin.render(page).expect("the probe renders"))
}

#[test]
fn a_plugin_without_the_ai_capability_reaches_no_provider_or_chat() {
    let ai = Arc::new(FakeAi::default());
    let Some((plugin, _folders)) = probe(&["name = \"views\""], &ai) else {
        return;
    };

    for call in [
        "ai:providers",
        "ai:chats",
        "ai:create-chat",
        "ai:read-chat:chat_01jbcdefghjkmnpqrstvwxyz0a",
        "ai:send:chat_01jbcdefghjkmnpqrstvwxyz0a:local:Hello",
        "ai:answer-tools:run_1:{}",
        "ai:read-run:run_1",
        "ai:stop:run_1",
        "ai:rename-chat:chat_01jbcdefghjkmnpqrstvwxyz0a:rev1:Garden",
        "ai:set-retention:chat_01jbcdefghjkmnpqrstvwxyz0a:rev1:ephemeral",
        "ai:workspace-tools",
        "ai:run-tools:run_1",
        "ai:resume:chat_01jbcdefghjkmnpqrstvwxyz0a:local",
        "ai:web-ready",
        "ai:web-search:tomatoes",
        "ai:web-fetch:https://garden.example.com/tomatoes",
    ] {
        assert_eq!(
            lines(&plugin, call),
            ["error capability-not-declared ai"],
            "{call}"
        );
    }
    assert_eq!(ai.calls(), Vec::<String>::new());
}

#[test]
fn only_a_plugin_approved_for_ai_reaches_web_search_and_page_reads() {
    let ai = Arc::new(FakeAi::default());
    let Some((plugin, _folders)) = probe(&["name = \"views\"", "name = \"ai\""], &ai) else {
        return;
    };

    assert_eq!(lines(&plugin, "ai:web-ready"), ["web-ready true"]);
    assert_eq!(
        lines(&plugin, "ai:web-search:tomatoes"),
        ["web-result Tomatoes https://garden.example.com/tomatoes Grow them in sun."]
    );
    assert_eq!(
        lines(&plugin, "ai:web-fetch:https://garden.example.com/tomatoes"),
        ["web-page https://garden.example.com/tomatoes text/html false Growing tomatoes"]
    );
    assert_eq!(
        lines(&plugin, "ai:web-search:too often"),
        ["web-failed web_search_rate_limited Brave Search's limit for your key was reached."]
    );
    assert_eq!(
        ai.calls(),
        [
            "web-ready",
            "web-search tomatoes Some(3)",
            "web-fetch https://garden.example.com/tomatoes",
            "web-search too often Some(3)",
        ]
    );
}

/// A service with no web access, as before web search came: every web
/// call fails closed.
struct NoWeb;

impl AiChats for NoWeb {
    fn providers(&self) -> Result<Vec<AiProvider>, ServiceError> {
        Ok(Vec::new())
    }

    fn chats(&self) -> Result<Vec<AiChat>, ServiceError> {
        Ok(Vec::new())
    }

    fn read_chat(&self, id: &str) -> Result<(AiChat, Vec<AiMessage>), ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn create_chat(&self) -> Result<AiChat, ServiceError> {
        Err(ServiceError::Failed("No chats here.".to_owned()))
    }

    fn send(&self, _request: AiRequest) -> Result<AiRun, ServiceError> {
        Err(ServiceError::Failed("No providers here.".to_owned()))
    }

    fn answer_tools(
        &self,
        run: &str,
        _results: Vec<AiToolResult>,
        _tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        Err(ServiceError::NotFound(run.to_owned()))
    }

    fn read_run(&self, id: &str) -> Result<AiRun, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn rename_chat(&self, id: &str, _title: &str, _revision: &str) -> Result<AiChat, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn set_retention(
        &self,
        id: &str,
        _retention: AiRetention,
        _revision: &str,
    ) -> Result<AiChat, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn workspace_tools(&self) -> Result<Vec<AiToolDefinition>, ServiceError> {
        Ok(Vec::new())
    }

    fn run_tools(&self, run: &str) -> Result<AiRun, ServiceError> {
        Err(ServiceError::NotFound(run.to_owned()))
    }

    fn resume(
        &self,
        chat: &str,
        _provider: &str,
        _tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        Err(ServiceError::NotFound(chat.to_owned()))
    }
}

#[test]
fn a_service_without_web_access_refuses_every_web_call() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running_as(
        "chatter",
        &probe,
        &["name = \"views\"", "name = \"ai\""],
        Services {
            ai: Some(Arc::new(NoWeb)),
            ..Services::default()
        },
    );

    assert_eq!(lines(&plugin, "ai:web-ready"), ["web-ready false"]);
    for call in [
        "ai:web-search:tomatoes",
        "ai:web-fetch:https://garden.example.com/",
    ] {
        assert_eq!(
            lines(&plugin, call),
            ["web-failed web_access_unavailable Noura can't search the web here."],
            "{call}"
        );
    }
}

/// The AI plugin reaches the web only through the host AI interface: its
/// component imports no WASI socket or HTTP interface, which the host
/// never links anyway.
#[test]
fn the_ai_plugin_imports_no_socket_or_http_interface() {
    let Some(component) = plugins::built("ai_plugin") else {
        return;
    };
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let component = wasmtime::component::Component::new(&engine, &component).unwrap();

    let imports: Vec<String> = component
        .component_type()
        .imports(&engine)
        .map(|(name, _)| name.to_owned())
        .collect();

    assert!(
        imports
            .iter()
            .any(|import| import == "noura:plugin/ai@0.1.0"),
        "{imports:?}"
    );
    assert!(
        imports
            .iter()
            .all(|import| !import.starts_with("wasi:sockets/") && !import.starts_with("wasi:http/")),
        "{imports:?}"
    );
    assert!(
        !imports
            .iter()
            .any(|import| import.starts_with("noura:plugin/network")),
        "{imports:?}"
    );
}

#[test]
fn a_plugin_with_the_ai_capability_sends_and_hears_failures_as_public_codes() {
    let ai = Arc::new(FakeAi::default());
    let Some((plugin, _folders)) = probe(&["name = \"views\"", "name = \"ai\""], &ai) else {
        return;
    };

    assert_eq!(
        lines(&plugin, "ai:providers"),
        ["provider local Local ollama llama3"]
    );
    let sent = lines(
        &plugin,
        "ai:send-with-tool:chat_01jbcdefghjkmnpqrstvwxyz0a:local:What grows in October?",
    );
    assert_eq!(
        sent,
        [
            "run run_1 RunStatus::Failed message chat-message_01jbcdefghjkmnpqrstvwxyz0b calls \
             [call_1=echo({\"text\":\"hi\"})] text Part of an answer failure \
             provider_request_failed The AI provider request failed"
        ]
    );
    lines(&plugin, "ai:answer-tools:run_1:{\"text\":\"hi\"}");
    assert_eq!(lines(&plugin, "ai:stop:run_1"), ["stopped"]);
    assert_eq!(
        lines(&plugin, "ai:workspace-tools"),
        ["tool tasks__create Create a task."]
    );
    lines(&plugin, "ai:run-tools:run_1");
    lines(&plugin, "ai:resume:chat_01jbcdefghjkmnpqrstvwxyz0a:local");
    assert_eq!(
        lines(
            &plugin,
            "ai:rename-chat:chat_01jbcdefghjkmnpqrstvwxyz0a:rev1:Garden"
        ),
        ["error revision-conflict chat_01jbcdefghjkmnpqrstvwxyz0a"]
    );
    assert_eq!(
        lines(
            &plugin,
            "ai:set-retention:chat_01jbcdefghjkmnpqrstvwxyz0a:rev1:ephemeral"
        ),
        ["chat chat_01jbcdefghjkmnpqrstvwxyz0a rev1 Retention::Ephemeral Plan the garden"]
    );
    assert_eq!(
        lines(&plugin, "ai:read-chat:chat_01jbcdefghjkmnpqrstvwxyz0z"),
        ["error not-found chat_01jbcdefghjkmnpqrstvwxyz0z"]
    );
    assert_eq!(
        ai.calls(),
        [
            "providers",
            "send chat_01jbcdefghjkmnpqrstvwxyz0a local What grows in October? 1",
            "read-run run_1",
            "answer-tools run_1 call_1={\"text\":\"hi\"} 1",
            "stop run_1",
            "workspace-tools",
            "run-tools run_1",
            "workspace-tools",
            "resume chat_01jbcdefghjkmnpqrstvwxyz0a local 1",
            "rename-chat chat_01jbcdefghjkmnpqrstvwxyz0a rev1 Garden",
            "set-retention chat_01jbcdefghjkmnpqrstvwxyz0a rev1 Retention::Ephemeral",
            "read-chat chat_01jbcdefghjkmnpqrstvwxyz0z",
        ]
    );
}

#[test]
fn a_plugin_holding_ai_with_no_service_hears_that_noura_cant_reach_providers() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running_as(
        "chatter",
        &probe,
        &["name = \"views\"", "name = \"ai\""],
        Services::default(),
    );
    assert_eq!(
        lines(&plugin, "ai:providers"),
        ["error failed Noura can't reach AI providers here."]
    );
}

/// The AI plugin, started with what its manifest declares.
fn ai_plugin(types: &ObjectTypes) -> Option<(Arc<RunningPlugin>, [tempfile::TempDir; 2])> {
    let component = plugins::built("ai_plugin")?;
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "ai_plugin.wasm", &component);
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load("ai").unwrap();
    let objects: Arc<dyn plugin_host::WorkspaceObjects> = Arc::new(FakeObjects::default());
    let plugin = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services {
            objects: Some(objects),
            types: types.clone(),
            ..Services::default()
        },
    );
    Some((plugin, [folder, library]))
}

#[test]
fn the_ai_plugin_declares_the_chat_types_its_page_and_the_ai_capability() {
    let manifest = read_manifest(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/ai"
    )))
    .expect("the manifest reads");
    assert_eq!(manifest.id, "ai");
    assert_eq!(
        manifest.capabilities,
        [Capability::ObjectTypes, Capability::Views, Capability::Ai]
    );
    let types = ObjectTypes::new();
    let Some((plugin, _folders)) = ai_plugin(&types) else {
        return;
    };

    let registered = plugin.register_object_types().unwrap();

    assert_eq!(
        registered,
        [
            ("chat".to_owned(), Ok(())),
            ("chat-message".to_owned(), Ok(()))
        ]
    );
    let fixtures: Value = serde_json::from_str(SCHEMAS).unwrap();
    for object_type in ["chat", "chat-message"] {
        assert_eq!(types.owner(object_type).as_deref(), Some("ai"));
        let expected: PropertySchema =
            serde_json::from_value(fixtures["schemas"][object_type].clone()).unwrap();
        assert_eq!(
            types.schemas().schema(object_type),
            Some(&expected),
            "{object_type}"
        );
    }
    let pages: Vec<(String, String)> = plugin
        .pages()
        .unwrap()
        .into_iter()
        .map(|page| (page.id, page.title))
        .collect();
    assert_eq!(pages, [("ai".to_owned(), "AI".to_owned())]);
}

/// Every chat and chat-message conformance fixture that the chat rules
/// accept fits the AI plugin's schemas. The chat rules refuse the others:
/// the AI service checks them on each chat it writes, and no plugin writes
/// a chat through the objects interface.
#[test]
fn every_valid_chat_conformance_fixture_fits_the_ai_plugins_schemas() {
    let types = ObjectTypes::new();
    let Some((plugin, _folders)) = ai_plugin(&types) else {
        return;
    };
    plugin.register_object_types().unwrap();
    let fixtures: Value = serde_json::from_str(CONFORMANCE).unwrap();
    for (section, object_type) in [("chat", "chat"), ("chat_message", "chat-message")] {
        for fixture in fixtures[section].as_array().unwrap() {
            let name = fixture["name"].as_str().unwrap();
            let mut properties = fixture["value"].as_object().unwrap().clone();
            let id = properties
                .remove("id")
                .and_then(|id| id.as_str().map(str::to_owned))
                .unwrap();
            properties.remove("type");
            properties.remove("created");
            properties.remove("updated");
            let object = WorkspaceObject {
                id,
                object_type: object_type.to_owned(),
                title: String::new(),
                body: String::new(),
                relative_path: "chats/test--000000/chat.md".to_owned(),
                revision: String::new(),
                created: None,
                updated: None,
                properties: properties.into_iter().collect(),
            };
            if fixture["valid"] == true {
                assert_eq!(types.problem(&object), None, "{name}");
            }
        }
    }
}
