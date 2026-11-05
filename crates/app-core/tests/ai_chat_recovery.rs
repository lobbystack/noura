//! Seam 1 with a process stopped mid-stream: Noura quits or crashes while
//! an answer streams in, or while a tool call waits for you. When it
//! starts again, the answer reopens as interrupted and the chat offers to
//! resume from your last message. A tool call reopens as completed only
//! when the chat holds its result from the same run, with the same call ID
//! and tool name, and otherwise as interrupted, and Noura never runs it
//! again by itself.
//!
//! The test runs a copy of this test binary as the process that stops:
//! the ignored test `stops_mid_stream` chats on the same app data until
//! the test kills it.

mod support;

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use app_core::App;
use local_core::AiToolCall;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins,
};

/// Where the stopping process finds the app data, and what it does.
const APP_DATA: &str = "NOURA_TEST_STOPPING_APP_DATA";
const MODE: &str = "NOURA_TEST_STOPPING_MODE";

/// What the stopping process prints once the files hold what the test
/// stops it on.
const READY: &str = "noura-test-stopping-ready";

/// The process the tests stop: it launches Noura on the app data the
/// parent test set up, sends a message from the AI page, and waits once
/// the answer streams in, or once Noura asks about a tool call.
#[test]
#[ignore = "the recovery tests run it as a process of its own"]
fn stops_mid_stream() {
    let (Some(app_data), Some(mode)) = (std::env::var_os(APP_DATA), std::env::var_os(MODE)) else {
        return;
    };
    let mut app = App::launch_with(Path::new(&app_data), FakeSystem::new());
    support::settle(&mut app);
    let provider = FakeProvider::new();
    app.set_ai_client(provider.clone());
    ai::wait_until(&mut app, "the AI page shows", |app| {
        app.plugin_view(ai::PAGE).is_some_and(|view| view.is_ok())
    });
    let id = app.ai_providers()[0].id.clone();
    if mode == "tool" {
        ai::send(&mut app, "Echo hi three times", &id);
        ai::wait_until(&mut app, "the request leaves", |_| {
            provider.requests().len() == 1
        });
        let echo = |call: &str| AiToolCall {
            call_id: call.into(),
            name: "echoer__echo".into(),
            arguments: serde_json::json!({ "text": "hi" }),
        };
        provider.call_tools(0, vec![echo("call_1"), echo("call_2"), echo("call_3")]);
        ai::tool_prompt(&mut app);
    } else {
        ai::send(&mut app, "Plan the garden", &id);
        ai::wait_until(&mut app, "the request leaves", |_| {
            provider.requests().len() == 1
        });
        provider.say(0, "Start with");
        ai::wait_until(&mut app, "the answer streams", |app| {
            ai::lines(app).contains(&"md: Start with".to_owned())
        });
    }
    println!("{READY}");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Run [`stops_mid_stream`] on `app_data` in `mode`, wait until it's
/// ready, then kill it, as a crash or a forced quit stops Noura.
fn stop_mid_stream(app_data: &Path, mode: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "stops_mid_stream",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(APP_DATA, app_data)
        .env(MODE, mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready, said) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                return;
            };
            if line.contains(READY) {
                let _ = ready.send(());
            }
        }
    });
    let waited = said.recv_timeout(Duration::from_secs(60));
    child.kill().unwrap();
    child.wait().unwrap();
    waited.expect("the stopping process got ready");
}

/// A workspace whose app data has the AI plugin, and in `tools` the
/// probe offering its echo tool as the plugin `echoer`, installed and on,
/// and the provider Local allowed. No app holds it open.
struct Stopped {
    fixture: Fixture,
    _plugins: Vec<plugins::PluginFolder>,
}

impl Stopped {
    fn set_up(tools: bool) -> Option<Self> {
        let fixture = Fixture::new(&[]);
        let mut app = fixture.app_with(FakeSystem::new());
        let mut folders = Vec::new();
        if tools {
            let probe = plugins::folder_for(
                "probe_plugin",
                "echoer",
                &["name = \"views\"", "name = \"tools\""],
            )?;
            plugins::install_and_enable(&mut app, probe.path(), "echoer");
            folders.push(probe);
        }
        folders.push(ai::with_ai(&mut app, &FakeProvider::new())?);
        let id = app
            .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
        app.grant_ai_consent(&id).unwrap();
        support::settle(&mut app);
        app.close_window();
        drop(app);
        Some(Self {
            fixture,
            _plugins: folders,
        })
    }

    /// Noura again on the same app data, with the AI page drawn, talking
    /// to `provider`.
    fn relaunch(&self, provider: &std::sync::Arc<FakeProvider>) -> App {
        let mut app = self.fixture.app_with(FakeSystem::new());
        app.set_ai_client(provider.clone());
        ai::wait_until(&mut app, "the chat is listed", |app| {
            ai::chat_ids(app).len() == 1
        });
        let chat = ai::chat_ids(&app)[0].clone();
        ai::open_chat(&mut app, &chat);
        app
    }

    /// The message files of `kind`.
    fn messages(&self, kind: &str) -> Vec<String> {
        ai::message_files(&self.fixture.root)
            .into_iter()
            .filter(|text| ai::frontmatter(text, "kind").as_deref() == Some(kind))
            .collect()
    }
}

#[test]
fn an_answer_a_crash_cut_off_reopens_as_interrupted_and_resumes_from_your_message() {
    let Some(stopped) = Stopped::set_up(false) else {
        return;
    };
    stop_mid_stream(stopped.fixture.app_data.path(), "answer");
    let answer = stopped.messages("assistant").remove(0);
    assert_eq!(
        ai::frontmatter(&answer, "status").as_deref(),
        Some("in-progress"),
        "the crash left the answer in progress"
    );

    let provider = FakeProvider::new();
    let mut app = stopped.relaunch(&provider);

    ai::wait_until(&mut app, "the answer reads as interrupted", |app| {
        ai::lines(app).contains(&"Noura stopped before this answer finished.".to_owned())
    });
    let answer = stopped.messages("assistant").remove(0);
    assert_eq!(
        ai::frontmatter(&answer, "status").as_deref(),
        Some("interrupted")
    );
    assert_eq!(ai::buttons(&app), ["Resume"]);
    std::thread::sleep(Duration::from_millis(200));
    assert!(provider.requests().is_empty(), "nothing resumes by itself");

    ai::press(&mut app, "resume");
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert_eq!(provider.requests()[0].messages, ["user: Plan the garden"]);
    provider.say(0, "Plant garlic.");
    provider.finish(0);
    ai::wait_until(&mut app, "the answer ends", |app| {
        ai::lines(app).contains(&"md: Plant garlic.".to_owned()) && ai::buttons(app).is_empty()
    });
    assert_eq!(
        stopped.messages("user").len(),
        1,
        "resuming writes no message of yours"
    );
}

#[test]
fn a_tool_call_reopens_as_completed_only_beside_its_own_result_and_never_runs_again() {
    let Some(stopped) = Stopped::set_up(true) else {
        return;
    };
    stop_mid_stream(stopped.fixture.app_data.path(), "tool");
    let calls = stopped.messages("tool-call");
    assert_eq!(calls.len(), 3);
    assert!(stopped.messages("tool-result").is_empty());
    // As a crash just after Noura wrote a call's result leaves it: the
    // first call's result names its run, its call and its tool, the
    // second's names another tool, and the third's another run.
    for (call, tool, run) in [
        ("call_1", "echoer__echo", None),
        ("call_2", "echoer__other", None),
        ("call_3", "echoer__echo", Some("run_other")),
    ] {
        let text = calls
            .iter()
            .find(|text| ai::frontmatter(text, "tool_call_id").as_deref() == Some(call))
            .unwrap();
        result_beside(&stopped.fixture.root, text, tool, run);
    }

    let provider = FakeProvider::new();
    let mut app = stopped.relaunch(&provider);

    ai::wait_until(&mut app, "the calls read as Noura left them", |app| {
        ai::lines(app).contains(
            &"Noura stopped before this call finished, and won't run it on its own.".to_owned(),
        )
    });
    let status = |call: &str| {
        let text = stopped
            .messages("tool-call")
            .into_iter()
            .find(|text| ai::frontmatter(text, "tool_call_id").as_deref() == Some(call))
            .unwrap();
        ai::frontmatter(&text, "status")
    };
    assert_eq!(status("call_1").as_deref(), Some("completed"));
    assert_eq!(status("call_2").as_deref(), Some("interrupted"));
    assert_eq!(status("call_3").as_deref(), Some("interrupted"));
    assert_eq!(app.ai_tool_prompt(), None, "Noura asks about nothing");
    assert_eq!(ai::buttons(&app), ["Resume"]);
    std::thread::sleep(Duration::from_millis(200));
    assert!(provider.requests().is_empty(), "nothing resumes by itself");
    assert_eq!(stopped.messages("tool-result").len(), 3);

    // The resumed request tells the model what already ran, so it doesn't
    // ask for the first call again, and leaves out the calls without
    // their own result.
    ai::press(&mut app, "resume");
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert_eq!(
        provider.requests()[0].messages,
        [
            "user: Echo hi three times",
            "assistant: call call_1 echoer__echo {\"text\":\"hi\"}",
            "tool: result call_1 {\"text\":\"hi\"}",
        ]
    );
}

/// Write a completed result for the tool call `call`, a tool-call file's
/// text, beside it in its chat, naming `tool`, and the run `run` when
/// given, or the call's own.
fn result_beside(root: &Path, call: &str, tool: &str, run: Option<&str>) {
    let field = |key: &str| ai::frontmatter(call, key).unwrap();
    let run = run.map_or_else(|| field("run_id"), str::to_owned);
    let id = local_core::new_object_id("chat-message");
    let now = jiff::Timestamp::now().to_string();
    let text = format!(
        "---\nid: {id}\ntype: chat-message\nchat_id: {}\nrun_id: {}\nkind: tool-result\n\
         status: completed\ncontent_type: application/json\nprovider_id: null\nmodel_id: null\n\
         tool_call_id: {}\ntool_name: {tool}\nerror_code: null\n\
         summarizes_through_message_id: null\ncreated: {now}\nupdated: {now}\n---\n\n\
         {{\"text\":\"hi\"}}\n",
        field("chat_id"),
        run,
        field("tool_call_id"),
    );
    let folder = ai::chat_files(root)
        .into_iter()
        .find(|(_, text)| text == call)
        .map(|(path, _)| root.join(path).parent().unwrap().to_owned())
        .unwrap();
    std::fs::write(folder.join(format!("{id}.md")), text).unwrap();
}
