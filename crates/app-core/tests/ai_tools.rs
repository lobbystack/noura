//! Seam 1: the host AI interface's tool calls, which ticket 37 builds on.
//! A plugin offers tools with a request, the run lists the calls the model
//! asked for once Noura wrote each one to the chat, and the plugin's
//! results go back to the model in the same run, written to the chat
//! before the next request leaves.

mod support;

use app_core::App;
use local_core::AiToolCall;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins,
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
