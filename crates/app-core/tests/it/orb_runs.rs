//! An Orb's runs: you write to an Orb and it answers in its one thread,
//! with its instructions and memory, running the tools its file lists
//! without asking. Each run leaves a record. A run Noura's quit cut off
//! answers again once at the next launch, and one a quit stopped during a
//! tool call ends as interrupted without running the call again. Plugins
//! reach all of it through the `ai` interface.

use crate::support;
use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    time::Duration,
};

use app_core::{AiOrbRunStatus, AiOrbStart, AiOrbState, App};
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    orbs::{self, MEETINGS, MEMORY_FILE, Reply, Scripted},
    plugins,
};

fn fixture_with(files: &[(String, Vec<u8>)]) -> Fixture {
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    Fixture::new(&files)
}

/// The app on `fixture`, talking to `provider` through the provider Local.
fn app_with(fixture: &Fixture, provider: Arc<dyn local_core::AiStreamer>) -> App {
    let system = FakeSystem::new();
    system.set_clock("2026-11-16T14:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    app.set_ai_client(provider);
    if app.ai_providers().is_empty() {
        app.save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
    }
    app
}

#[test]
fn an_orb_runs_the_tools_it_lists_without_asking_and_records_each_call() {
    let fixture = fixture_with(&orbs::meetings(
        &["tasks__create"],
        &[],
        "2026-11-01T10:00:00Z",
    ));
    let provider = Scripted::new(|request| {
        if orbs::has_result(request) {
            Reply::Text("Done: one task.".to_owned())
        } else {
            Reply::Call(
                "tasks__create".to_owned(),
                serde_json::json!({ "title": "Send the deck" }),
            )
        }
    });
    let mut app = app_with(&fixture, provider.clone());
    let Some(_tasks) = support::tasks::with_tasks(&mut app) else {
        return;
    };

    let run = app
        .send_to_orb(MEETINGS, "Ada will send the deck on Friday.")
        .unwrap();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);

    assert!(app.ai_tool_prompt().is_none());
    assert!(fixture.exists("Tasks/Send the deck.md"));
    assert_eq!(runs[0].id, run.id);
    assert_eq!(runs[0].status, AiOrbRunStatus::Succeeded);
    assert_eq!(runs[0].started_by, AiOrbStart::You);
    assert_eq!(runs[0].trigger, None);
    assert_eq!(runs[0].tool_calls, 1);
    let asked = provider.requests();
    assert_eq!(asked.len(), 2);
    // Only the tools the file lists, though Tasks offers four.
    assert_eq!(asked[0].tools, ["tasks__create"]);
    assert!(asked[0].system[0].starts_with("You are Meetings, an Orb"));
    assert!(asked[0].system[0].contains("## Your instructions\nYou turn meetings"));
    assert!(asked[0].system[0].contains(&format!(
        "## Your memory: {MEMORY_FILE}\n- Ada owns the deck."
    )));
    assert_eq!(
        asked[1].messages,
        [
            "user: Ada will send the deck on Friday.",
            &format!(
                "assistant: call call_1 tasks__create {}",
                serde_json::json!({ "title": "Send the deck" })
            ),
            &format!(
                "tool: result call_1 {}",
                support::ai::tool_result(&mut app, &fixture.root, "call_1")
            ),
        ]
    );

    // A second message goes to the same thread.
    app.send_to_orb(MEETINGS, "Thanks.").unwrap();
    orbs::wait_for_runs(&mut app, MEETINGS, 2);
    let chats: Vec<String> = ai::chat_files(&fixture.root)
        .into_iter()
        .filter(|(path, _)| path.ends_with("/chat.md"))
        .map(|(_, text)| text)
        .collect();
    assert_eq!(chats.len(), 1, "one thread: {chats:?}");
    assert_eq!(
        ai::frontmatter(&chats[0], "orb_id").as_deref(),
        Some(MEETINGS)
    );
    let state = orbs::orb(&app, MEETINGS).state;
    assert_eq!(state, AiOrbState::Idle);
}

#[test]
fn an_orb_remembers_and_reads_only_what_its_reads_allow() {
    let mut files = orbs::meetings(
        &["noura__remember", "noura__read"],
        &[],
        "2026-11-01T10:00:00Z",
    );
    // Its reads name the project Garden, whose folder holds a note it may
    // read, while the note Secret lies outside.
    files[0].1 = String::from_utf8(files[0].1.clone())
        .unwrap()
        .replace("memory:", "reads:\n- '[[Garden]]'\nmemory:")
        .into_bytes();
    files.push((
        "Projects/Garden/Garden.md".to_owned(),
        b"---\nid: project_01jd8x3k9q2m7v4c6b1n0p5r8g\ntype: project\nstatus: active\n---\nGrow.\n"
            .to_vec(),
    ));
    files.push((
        "Projects/Garden/Beds.md".to_owned(),
        b"Tomatoes by the wall.\n".to_vec(),
    ));
    files.push(("Notes/Secret.md".to_owned(), b"The safe's code.\n".to_vec()));
    let fixture = fixture_with(&files);
    let provider = Scripted::new(|request| {
        let results = request
            .messages
            .iter()
            .filter(|message| message.starts_with("tool: result"))
            .count();
        match results {
            0 => Reply::Call(
                "noura__remember".to_owned(),
                serde_json::json!({ "text": "Ada prefers Fridays." }),
            ),
            1 => Reply::Call(
                "noura__read".to_owned(),
                serde_json::json!({ "link": "[[Beds]]" }),
            ),
            2 => Reply::Call(
                "noura__read".to_owned(),
                serde_json::json!({ "link": "Secret" }),
            ),
            _ => Reply::Text("Noted.".to_owned()),
        }
    });
    let mut app = app_with(&fixture, provider.clone());

    app.send_to_orb(MEETINGS, "Remember that Ada prefers Fridays.")
        .unwrap();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);

    assert_eq!(runs[0].status, AiOrbRunStatus::Succeeded, "{runs:?}");
    assert_eq!(runs[0].tool_calls, 3);
    let memory = String::from_utf8(fixture.read(MEMORY_FILE)).unwrap();
    assert!(
        memory.ends_with("---\n- Ada owns the deck.\n- Ada prefers Fridays.\n"),
        "{memory}"
    );
    let asked = provider.requests();
    assert!(
        asked[1].system[0].contains("- Ada prefers Fridays."),
        "memory is read again"
    );
    let read = support::ai::tool_result(&mut app, &fixture.root, "call_2");
    assert_eq!(read["body"], "Tomatoes by the wall.\n", "{read}");
    let refused = support::ai::tool_result(&mut app, &fixture.root, "call_3");
    assert_eq!(refused["error"]["code"], "tool-failed", "{refused}");
    assert!(!refused.to_string().contains("safe"), "{refused}");
}

#[test]
fn the_default_orb_comes_first_and_holds_plain_chats() {
    let fixture = fixture_with(&orbs::meetings(&[], &[], "2026-11-01T10:00:00Z"));
    let app = app_with(&fixture, FakeProvider::new());

    let listed = app.orbs().unwrap();

    assert_eq!(listed[0].id, workspace_format::orb::DEFAULT_ORB_ID);
    assert_eq!(listed[0].title, "Noura");
    assert_eq!(listed[0].revision, "");
    assert_eq!(listed[1].id, MEETINGS);
    assert_eq!(listed[1].title, "Meetings");
    assert_eq!(listed[1].thread, None);
    assert_eq!(listed.len(), 2);
}

/// Where the stopping process finds the app data.
const APP_DATA: &str = "NOURA_TEST_ORB_STOPPING_APP_DATA";

/// What the stopping process prints once the Orb's request left.
const READY: &str = "noura-test-orb-stopping-ready";

/// The process the resume test stops: Noura on the app data the parent
/// set up sends a message to the Meetings Orb and waits while the answer
/// streams in.
#[test]
#[ignore = "the resume test runs it as a process of its own"]
fn orb_stops_mid_answer() {
    let Some(app_data) = std::env::var_os(APP_DATA) else {
        return;
    };
    let system = FakeSystem::new();
    system.set_clock("2026-11-16T14:00:00Z", "America/Toronto");
    let mut app = App::launch_with(Path::new(&app_data), system);
    support::settle(&mut app);
    let provider = FakeProvider::new();
    app.set_ai_client(provider.clone());
    app.send_to_orb(MEETINGS, "Plan Friday.").unwrap();
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    provider.say(0, "First,");
    std::thread::sleep(Duration::from_millis(200));
    println!("{READY}");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Run [`orb_stops_mid_answer`] on `app_data`, wait until it's ready, then
/// kill it, as a crash or a forced quit stops Noura.
fn stop_mid_answer(app_data: &Path) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "orb_runs::orb_stops_mid_answer",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(APP_DATA, app_data)
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

#[test]
fn a_run_a_quit_cut_off_answers_again_once_at_the_next_launch() {
    let fixture = fixture_with(&orbs::meetings(&[], &[], "2026-11-01T10:00:00Z"));
    {
        let mut app = app_with(&fixture, FakeProvider::new());
        support::settle(&mut app);
        app.close_window();
    }
    stop_mid_answer(fixture.app_data.path());
    let records = orbs::run_files(&fixture.root);
    assert_eq!(records.len(), 1);
    assert_eq!(
        ai::frontmatter(&records[0].1, "status").as_deref(),
        Some("running"),
        "the quit left the run running"
    );

    let provider = Scripted::new(|_| Reply::Text("Start with the deck.".to_owned()));
    let mut app = app_with(&fixture, provider.clone());
    app.check_orbs();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);

    assert_eq!(runs.len(), 1, "the same run, taken up");
    assert_eq!(runs[0].status, AiOrbRunStatus::Succeeded);
    let record = &orbs::run_files(&fixture.root)[0].1;
    assert_eq!(ai::frontmatter(record, "resumed").as_deref(), Some("1"));
    // It answers your message again, which it doesn't write twice.
    assert_eq!(provider.requests()[0].messages, ["user: Plan Friday."]);
    let statuses: Vec<String> = ai::message_files(&fixture.root)
        .iter()
        .filter(|text| ai::frontmatter(text, "kind").as_deref() == Some("assistant"))
        .map(|text| ai::frontmatter(text, "status").unwrap())
        .collect();
    let mut statuses = statuses;
    statuses.sort();
    assert_eq!(statuses, ["completed", "interrupted"]);

    // A later check takes up nothing more.
    app.check_orbs();
    support::settle(&mut app);
    assert_eq!(provider.requests().len(), 1);
}

/// A thread with your message of the run `run`, then the model's call
/// `call_1`, which a quit left in progress without its result.
fn stopped_during_a_call(run: &str, resumed: u32) -> Vec<(String, Vec<u8>)> {
    let chat = "chat_01jd8x3k9q2m7v4c6b1n0p5r8c";
    let folder = "Chats/meetings--5r8c0c";
    let message = |id: &str, lines: &str, body: &str| {
        (
            format!("{folder}/messages/2026-11-16/chat-message_{id}.md"),
            format!(
                "---\nid: chat-message_{id}\ntype: chat-message\nchat_id: {chat}\nrun_id: {run}\n{lines}\
                 created: 2026-11-16T13:00:00Z\nupdated: 2026-11-16T13:00:00Z\n---\n{body}"
            )
            .into_bytes(),
        )
    };
    let mut files = orbs::meetings(&["tasks__create"], &[], "2026-11-01T10:00:00Z");
    files.push((
        format!("{folder}/chat.md"),
        format!(
            "---\nid: {chat}\ntype: chat\ntitle: Meetings\norb_id: {MEETINGS}\nretention: permanent\n\
             retention_days: null\ncreated: 2026-11-16T13:00:00Z\nupdated: 2026-11-16T13:00:00Z\n---\n"
        )
        .into_bytes(),
    ));
    files.push(message(
        "01jd8x3k9q2m7v4c6b1n0p5r81",
        "kind: user\nstatus: completed\ncontent_type: text/markdown\n",
        "\nAda sends the deck.\n",
    ));
    files.push(message(
        "01jd8x3k9q2m7v4c6b1n0p5r82",
        "kind: assistant\nstatus: completed\ncontent_type: text/markdown\nprovider_id: local\nmodel_id: llama3\n",
        "",
    ));
    files.push(message(
        "01jd8x3k9q2m7v4c6b1n0p5r83",
        "kind: tool-call\nstatus: in-progress\ncontent_type: application/json\ntool_call_id: call_1\ntool_name: tasks__create\n",
        "{\"title\":\"Send the deck\"}\n",
    ));
    files.push((
        format!("Orbs/Meetings/Runs/2026-11-16/{run}.md"),
        format!(
            "---\nid: {run}\ntype: orb-run\norb_id: {MEETINGS}\nchat_id: {chat}\nstarted_by: you\n\
             status: running\nresumed: {resumed}\ncreated: 2026-11-16T13:00:00Z\n\
             updated: 2026-11-16T13:00:00Z\n---\n"
        )
        .into_bytes(),
    ));
    files
}

#[test]
fn a_run_a_quit_stopped_during_a_tool_call_ends_interrupted_and_runs_nothing_again() {
    let run = "orb-run_01jd8x3k9q2m7v4c6b1n0p5r8r";
    let fixture = fixture_with(&stopped_during_a_call(run, 0));
    let provider = FakeProvider::new();
    let mut app = app_with(&fixture, provider.clone());
    let Some(_tasks) = support::tasks::with_tasks(&mut app) else {
        return;
    };

    app.check_orbs();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);

    assert_eq!(runs[0].status, AiOrbRunStatus::Interrupted);
    assert_eq!(runs[0].error_code.as_deref(), Some("orb_interrupted"));
    assert!(runs[0].finished.is_some());
    std::thread::sleep(Duration::from_millis(200));
    assert!(provider.requests().is_empty(), "nothing asks again");
    assert!(
        !fixture.exists("Tasks/Send the deck.md"),
        "the call never runs again"
    );
    let call = ai::message_files(&fixture.root)
        .into_iter()
        .find(|text| ai::frontmatter(text, "kind").as_deref() == Some("tool-call"))
        .unwrap();
    assert_eq!(
        ai::frontmatter(&call, "status").as_deref(),
        Some("interrupted")
    );
}

#[test]
fn a_run_noura_took_up_once_ends_interrupted_after_another_quit() {
    let run = "orb-run_01jd8x3k9q2m7v4c6b1n0p5r8s";
    let mut files = stopped_during_a_call(run, 1);
    // No call waits this time: only your message, and the answer the quit
    // cut off.
    files.retain(|(path, _)| !path.ends_with("p5r83.md"));
    let fixture = fixture_with(&files);
    let provider = FakeProvider::new();
    let mut app = app_with(&fixture, provider.clone());

    app.check_orbs();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);

    assert_eq!(runs[0].status, AiOrbRunStatus::Interrupted);
    assert!(provider.requests().is_empty());
}

#[test]
fn a_plugin_reaches_orbs_through_the_ai_interface() {
    let fixture = fixture_with(&orbs::meetings(
        &["probe__echo"],
        &["event probe.ping"],
        "2026-11-01T10:00:00Z",
    ));
    let provider = Scripted::new(|request| {
        if orbs::has_result(request) {
            Reply::Text("Echoed.".to_owned())
        } else {
            Reply::Call(
                "probe__echo".to_owned(),
                serde_json::json!({ "text": "hi" }),
            )
        }
    });
    let mut app = app_with(&fixture, provider);
    let Some(probe) = plugins::folder_for(
        "probe_plugin",
        "probe",
        &["name = \"views\"", "name = \"ai\"", "name = \"tools\""],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "probe");
    let lines = |app: &App, page: &str| plugins::texts(app, &format!("plugin:probe:{page}"));
    let press = |app: &mut App, node: &str| {
        let seen = lines(app, "events").unwrap().len();
        app.press_plugin_button("plugin:probe:hello", node);
        support::settle(app);
        lines(app, "events").unwrap()[seen..].to_vec()
    };

    let listed = press(&mut app, "ai:orbs");
    assert!(
        listed[0].starts_with("hello ai orb orb_00000000000000000000000000 OrbState::Idle  Noura"),
        "{listed:?}"
    );
    assert!(
        listed[1].starts_with(&format!(
            "hello ai orb {MEETINGS} OrbState::Idle {} Meetings thread - tools [probe__echo] triggers [event probe.ping]",
            orbs::orb(&app, MEETINGS).revision
        )),
        "{listed:?}"
    );
    let sent = press(&mut app, &format!("ai:send-to-orb:{MEETINGS}:Echo hi"));
    assert!(sent[0].starts_with("hello ai run orb-run_"), "{sent:?}");
    orbs::wait_for_runs(&mut app, MEETINGS, 1);
    let history = press(&mut app, &format!("ai:orb-runs:{MEETINGS}"));
    assert_eq!(history.len(), 1);
    assert!(
        history[0].contains("OrbStart::You OrbRunStatus::Succeeded - calls 1 quiet false error -"),
        "{history:?}"
    );
    let read = press(&mut app, &format!("ai:read-orb:{MEETINGS}"));
    assert!(read[1].starts_with("hello ai chat chat_"), "{read:?}");
    assert!(read[1].ends_with("Meetings"), "{read:?}");
    assert_eq!(read.len(), 2 + 5, "the thread's five messages: {read:?}");
    let revision = orbs::orb(&app, MEETINGS).revision;
    let paused = press(
        &mut app,
        &format!("ai:set-orb-paused:{MEETINGS}:{revision}:true"),
    );
    assert!(paused[0].contains(" OrbState::Paused "), "{paused:?}");
    let stale = press(
        &mut app,
        &format!("ai:set-orb-paused:{MEETINGS}:{revision}:false"),
    );
    assert!(stale[0].contains("revision-conflict"), "{stale:?}");
    let held = press(&mut app, "ai:orbs-held");
    assert_eq!(held, ["hello ai orbs-held false"]);
}
