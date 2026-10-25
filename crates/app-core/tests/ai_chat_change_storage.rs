//! Seam 1: the AGENTS.md storage matrix for the chat changes of tool
//! calls, renames, retention and recovery. A crash between a chat change's
//! two files completes on the next read of the workspace, and each change
//! is on disk before the call that made it returns.

mod support;

use std::{path::Path, sync::Arc};

use app_core::App;
use local_core::AiToolCall;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins, tasks,
};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";

/// The chat Garden plans, as an earlier session wrote it.
fn garden() -> Vec<(String, Vec<u8>)> {
    let chat = format!(
        "---\nid: chat_{A}\ntype: chat\ntitle: Garden plans\nretention: permanent\n\
         retention_days: null\ncreated: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:05:00Z\n---\n"
    );
    let message = format!(
        "---\nid: chat-message_{B}\ntype: chat-message\nchat_id: chat_{A}\nrun_id: run_1\n\
         kind: user\nstatus: completed\ncontent_type: text/markdown\nprovider_id: null\n\
         model_id: null\ntool_call_id: null\ntool_name: null\nerror_code: null\n\
         summarizes_through_message_id: null\ncreated: 2026-10-20T10:01:00Z\n\
         updated: 2026-10-20T10:01:00Z\n---\n\nWhat should I plant?\n"
    );
    vec![
        ("chats/garden--wxyz0a/chat.md".to_owned(), chat.into_bytes()),
        (
            format!("chats/garden--wxyz0a/messages/2026-10-20/chat-message_{B}.md"),
            message.into_bytes(),
        ),
    ]
}

fn fixture_with(files: &[(String, Vec<u8>)]) -> Fixture {
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    Fixture::new(&borrowed)
}

/// The pending chat changes Noura recorded before writing their files.
fn intents(root: &Path) -> Vec<serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(root.join(".noura/chat-mutations")) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap())
        .collect()
}

#[test]
fn a_tool_result_a_crash_cut_between_its_two_files_completes_on_the_next_read() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let provider = FakeProvider::new();
    let Some(_tasks) = tasks::with_tasks(&mut app) else {
        return;
    };
    let Some(_chat) = ai::with_ai(&mut app, &provider) else {
        return;
    };
    let id = app
        .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
        .unwrap();
    app.grant_ai_consent(&id).unwrap();
    support::settle(&mut app);
    ai::send(&mut app, "Add a task to water the tomatoes", &id);
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    provider.call_tools(
        0,
        vec![AiToolCall {
            call_id: "call_1".into(),
            name: "tasks__create".into(),
            arguments: serde_json::json!({ "title": "Water the tomatoes" }),
        }],
    );
    let prompt = ai::tool_prompt(&mut app);

    // The result's message file reaches the disk, and writing its chat.md
    // fails, as a crash between the two files would stop it.
    app.workspace().unwrap().fail_chat_writes("chat", 1);
    app.answer_ai_tool(&prompt, true);
    let result = ai::tool_message(&mut app, &fixture.root, "tool-result", "call_1");
    ai::wait_until(&mut app, "the run fails", |app| {
        ai::lines(app)
            .iter()
            .any(|line| line.starts_with("The answer failed"))
    });

    // The next read of the workspace finishes the change from its intent:
    // the chat.md takes the time of the result it holds.
    let root = fixture.root.clone();
    let created = ai::frontmatter(&result, "created");
    ai::wait_until(&mut app, "the intent is gone", |_| {
        intents(&root).is_empty()
    });
    let updated = ai::frontmatter(&ai::chat_file(&root).1, "updated");
    let time = |value: Option<String>| value.unwrap().parse::<jiff::Timestamp>().unwrap();
    assert!(time(updated) >= time(created));
    // The call the failed run left in progress reads as completed beside
    // its result.
    ai::wait_until(&mut app, "the call reads as completed", |_| {
        ai::message_files(&root).iter().any(|text| {
            ai::frontmatter(text, "kind").as_deref() == Some("tool-call")
                && ai::frontmatter(text, "status").as_deref() == Some("completed")
        })
    });
    let completed = ai::chat_file(&fixture.root);
    app.close_window();
    drop(app);

    // It stays complete after a relaunch.
    let mut app = fixture.app_with(system);
    app.set_ai_client(FakeProvider::new());
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_ids(app).len() == 1
    });
    let listed = ai::chat_ids(&app)[0].clone();
    ai::open_chat(&mut app, &listed);
    assert!(
        ai::lines(&app)
            .iter()
            .any(|line| line.starts_with("Result: {")),
        "{:?}",
        ai::lines(&app)
    );
    assert_eq!(ai::chat_file(&fixture.root), completed);
}

/// The app on `fixture` with the AI plugin on and the probe as the plugin
/// `chatter`, holding `ai`, which calls the host AI interface as its
/// buttons say.
fn with_probe(
    fixture: &Fixture,
    system: &Arc<FakeSystem>,
) -> Option<(App, [plugins::PluginFolder; 2])> {
    let mut app = fixture.app_with(system.clone());
    let chat = ai::with_ai(&mut app, &FakeProvider::new())?;
    let probe = plugins::folder_for(
        "probe_plugin",
        "chatter",
        &["name = \"views\"", "name = \"ai\""],
    )?;
    plugins::install_and_enable(&mut app, probe.path(), "chatter");
    Some((app, [chat, probe]))
}

/// Press the probe's button `node` on its page `hello`, and return the
/// lines its `events` page added.
fn press(app: &mut App, node: &str) -> Vec<String> {
    let events = "plugin:chatter:events";
    let seen = plugins::texts(app, events).unwrap().len();
    app.press_plugin_button("plugin:chatter:hello", node);
    support::settle(app);
    plugins::texts(app, events).unwrap()[seen..].to_vec()
}

/// The revision of the chat `chat`'s `chat.md`, as the probe reads it.
fn revision(app: &mut App, chat: &str) -> String {
    let read = press(app, &format!("ai:read-chat:{chat}"));
    read[0].split_whitespace().nth(4).unwrap().to_owned()
}

#[test]
fn renames_and_retention_changes_pass_the_storage_matrix() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, _plugins)) = with_probe(&fixture, &system) else {
        return;
    };
    let chat = format!("chat_{A}");
    let chat_md =
        || std::fs::read_to_string(fixture.root.join("chats/garden--wxyz0a/chat.md")).unwrap();
    let message = format!("chats/garden--wxyz0a/messages/2026-10-20/chat-message_{B}.md");
    let message_bytes = fixture.read(&message);
    let current = revision(&mut app, &chat);

    // Malformed titles, stale revisions and paths outside the workspace
    // write nothing.
    let before = fixture.snapshot();
    for (call, answer) in [
        (
            format!("ai:rename-chat:{chat}:{current}:   "),
            "hello ai error failed Give the chat a name.".to_owned(),
        ),
        (
            format!("ai:rename-chat:{chat}:{current}:Bell\u{7}"),
            "hello ai error failed Give the chat a name.".to_owned(),
        ),
        (
            format!("ai:rename-chat:{chat}:stale:Garlic"),
            format!("hello ai error revision-conflict {chat}"),
        ),
        (
            format!("ai:set-retention:{chat}:stale:ephemeral"),
            format!("hello ai error revision-conflict {chat}"),
        ),
        (
            "ai:rename-chat:../../Outside/chat:x:Garlic".to_owned(),
            "hello ai error not-found ../../Outside/chat".to_owned(),
        ),
        (
            format!("ai:set-retention:chat_{A}/../../x:x:ephemeral"),
            format!("hello ai error not-found chat_{A}/../../x"),
        ),
    ] {
        assert_eq!(press(&mut app, &call), [answer], "{call}");
    }
    assert_eq!(fixture.snapshot(), before);

    // Each change is on disk once the call returns, and the messages keep
    // their bytes.
    press(
        &mut app,
        &format!("ai:rename-chat:{chat}:{current}:Garlic plans"),
    );
    assert!(chat_md().contains("title: Garlic plans"));
    let current = revision(&mut app, &chat);
    press(
        &mut app,
        &format!("ai:set-retention:{chat}:{current}:ephemeral"),
    );
    assert!(chat_md().contains("retention: ephemeral"));
    assert!(chat_md().contains("retention_days: 30"));
    assert_eq!(fixture.read(&message), message_bytes);
    assert_eq!(
        press(&mut app, &format!("ai:read-chat:{chat}"))[0],
        format!(
            "hello ai chat {chat} {} Retention::Ephemeral Garlic plans",
            revision(&mut app, &chat)
        )
    );

    // A chat another app moved renames where it is now.
    std::fs::create_dir_all(fixture.root.join("Archive")).unwrap();
    std::fs::rename(
        fixture.root.join("chats/garden--wxyz0a"),
        fixture.root.join("Archive/garden"),
    )
    .unwrap();
    plugins::wait_until(&mut app, "the move is seen", |app| {
        app.tree()
            .is_some_and(|tree| tree.rows().iter().any(|row| row.name == "Archive"))
    });
    let current = revision(&mut app, &chat);
    press(
        &mut app,
        &format!("ai:rename-chat:{chat}:{current}:Moved plans"),
    );
    let moved = std::fs::read_to_string(fixture.root.join("Archive/garden/chat.md")).unwrap();
    assert!(moved.contains("title: Moved plans"), "{moved}");
    assert!(!fixture.exists("chats/garden--wxyz0a"));

    // A copy that shares the chat's ID stops renames until it goes.
    let current = revision(&mut app, &chat);
    support::write(&fixture.root, "Copies/garden/chat.md", moved.as_bytes());
    plugins::wait_until(&mut app, "the copy shows as a problem", |app| {
        !app.problems().is_empty()
    });
    let before = fixture.snapshot();
    assert_eq!(
        press(&mut app, &format!("ai:rename-chat:{chat}:{current}:Twice")),
        [format!(
            "hello ai error Error::IdentityConflict(\"{chat}\")"
        )]
    );
    assert_eq!(fixture.snapshot(), before);
    std::fs::remove_dir_all(fixture.root.join("Copies")).unwrap();
    plugins::wait_until(&mut app, "the problem goes", |app| {
        app.problems().is_empty()
    });

    // A deleted index rebuilds the same chat.
    let index = support::index_files(&fixture, &app);
    app.close_window();
    drop(app);
    for file in index {
        let _ = std::fs::remove_file(file);
    }
    let mut app = fixture.app_with(system);
    plugins::wait_until(&mut app, "the probe draws", |app| {
        app.plugin_view("plugin:chatter:events").is_some()
    });
    let read = press(&mut app, &format!("ai:read-chat:{chat}"));
    assert!(
        read[0].ends_with("Retention::Ephemeral Moved plans"),
        "{read:?}"
    );
    assert!(read[1].ends_with("What should I plant?"), "{read:?}");
}
