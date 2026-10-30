//! Seam 1: the AGENTS.md storage matrix for the chat changes of tool
//! calls, context summaries, renames, retention and recovery: malformed
//! input, round trips, external edits, stale indexes, interrupted index
//! updates, moves, duplicate IDs, path traversal and rebuilds. A crash
//! between a chat change's two files completes on the next read of the
//! workspace, and each change is on disk before the call that made it
//! returns.

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

/// Open the workspace's index on a second connection and stop it from
/// changing its file list, as a full disk or a broken index would.
fn refuse_index_writes(fixture: &Fixture, app: &App) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(&support::index_files(fixture, app)[0]).unwrap();
    refuse(&connection);
    connection
}

/// Stop the index on `connection` from changing its file list again.
fn refuse(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();
}

/// Let the index take writes again.
fn take_index_writes(connection: &rusqlite::Connection) {
    connection
        .execute_batch("DROP TRIGGER refuse_insert; DROP TRIGGER refuse_update;")
        .unwrap();
}

/// What a plugin hears when Noura can't look a chat up in an index that
/// refuses what it finds on disk.
const INDEX_FAILED: &str = "hello ai error failed The local index operation failed";

/// The revision in a chat line the probe printed.
fn revision_in(line: &str) -> String {
    line.split_whitespace().nth(4).unwrap().to_owned()
}

#[test]
fn renames_and_retention_changes_land_on_the_current_file_past_a_stale_index() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, _plugins)) = with_probe(&fixture, &system) else {
        return;
    };
    let chat = format!("chat_{A}");
    let path = "chats/garden--wxyz0a/chat.md";
    let chat_md = || String::from_utf8(fixture.read(path)).unwrap();
    let before = revision(&mut app, &chat);
    let index = support::index_files(&fixture, &app);
    app.close_window();
    drop(app);

    // While Noura is closed, another app renames the chat and has it
    // removed after 30 days. The index stays as it was.
    let edited = chat_md()
        .replace("title: Garden plans", "title: Kitchen garden")
        .replace(
            "retention: permanent\nretention_days: null",
            "retention: ephemeral\nretention_days: 30",
        );
    support::write(&fixture.root, path, edited.as_bytes());
    assert!(index.iter().any(|file| file.exists()), "the index stays");

    let mut app = fixture.app_with(system);
    plugins::wait_until(&mut app, "the probe draws", |app| {
        app.plugin_view("plugin:chatter:events").is_some()
    });
    let read = press(&mut app, &format!("ai:read-chat:{chat}"));
    assert!(
        read[0].ends_with("Retention::Ephemeral Kitchen garden"),
        "{read:?}"
    );
    let current = revision_in(&read[0]);
    assert_ne!(current, before);

    // The revision from before the outside edit writes nothing.
    let unchanged = fixture.snapshot();
    for call in [
        format!("ai:rename-chat:{chat}:{before}:Garlic plans"),
        format!("ai:set-retention:{chat}:{before}:permanent"),
    ] {
        assert_eq!(
            press(&mut app, &call),
            [format!("hello ai error revision-conflict {chat}")],
            "{call}"
        );
    }
    assert_eq!(fixture.snapshot(), unchanged);

    // The revision read-chat reports changes the file as the other app
    // left it, and keeps what that app changed.
    let renamed = press(
        &mut app,
        &format!("ai:rename-chat:{chat}:{current}:Garlic plans"),
    );
    assert!(
        renamed[0].ends_with("Retention::Ephemeral Garlic plans"),
        "{renamed:?}"
    );
    let text = chat_md();
    assert!(text.contains("title: Garlic plans"), "{text}");
    assert!(
        text.contains("retention: ephemeral\nretention_days: 30"),
        "{text}"
    );
    let kept = press(
        &mut app,
        &format!(
            "ai:set-retention:{chat}:{}:permanent",
            revision_in(&renamed[0])
        ),
    );
    assert!(
        kept[0].ends_with("Retention::Permanent Garlic plans"),
        "{kept:?}"
    );
    let text = chat_md();
    assert!(text.contains("title: Garlic plans"), "{text}");
    assert!(
        text.contains("retention: permanent\nretention_days: null"),
        "{text}"
    );
}

#[test]
fn renames_and_retention_changes_reach_the_disk_while_the_index_refuses_writes() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, _plugins)) = with_probe(&fixture, &system) else {
        return;
    };
    let chat = format!("chat_{A}");
    let chat_md =
        || std::fs::read_to_string(fixture.root.join("chats/garden--wxyz0a/chat.md")).unwrap();
    let current = revision(&mut app, &chat);

    // The index refuses to record the rename, which is on disk once its
    // call returns.
    let connection = refuse_index_writes(&fixture, &app);
    let renamed = press(
        &mut app,
        &format!("ai:rename-chat:{chat}:{current}:Garlic plans"),
    );
    assert!(
        renamed[0].ends_with("Retention::Permanent Garlic plans"),
        "{renamed:?}"
    );
    assert!(chat_md().contains("title: Garlic plans"));

    // Until the index takes writes again, Noura can't look the chat up,
    // and writes nothing.
    let unchanged = fixture.snapshot();
    assert_eq!(
        press(
            &mut app,
            &format!(
                "ai:set-retention:{chat}:{}:ephemeral",
                revision_in(&renamed[0])
            )
        ),
        [INDEX_FAILED]
    );
    assert_eq!(fixture.snapshot(), unchanged);

    // Then the next read shows the rename.
    take_index_writes(&connection);
    let read = press(&mut app, &format!("ai:read-chat:{chat}"));
    assert_eq!(read[0], renamed[0]);

    // The same for a retention change.
    refuse(&connection);
    let changed = press(
        &mut app,
        &format!(
            "ai:set-retention:{chat}:{}:ephemeral",
            revision_in(&renamed[0])
        ),
    );
    assert!(
        changed[0].ends_with("Retention::Ephemeral Garlic plans"),
        "{changed:?}"
    );
    let text = chat_md();
    assert!(text.contains("title: Garlic plans"), "{text}");
    assert!(
        text.contains("retention: ephemeral\nretention_days: 30"),
        "{text}"
    );
    take_index_writes(&connection);
    let read = press(&mut app, &format!("ai:read-chat:{chat}"));
    assert_eq!(read[0], changed[0]);

    // An index rebuilt from the files shows both.
    drop(connection);
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
    assert_eq!(read[0], changed[0]);
    assert!(read[1].ends_with("What should I plant?"), "{read:?}");
}

/// What the provider's summary says.
const SUMMARY: &str = "You asked about a, and I answered b.";

/// The message files under `root` whose kind is `kind`.
fn of_kind(root: &Path, kind: &str) -> Vec<String> {
    ai::message_files(root)
        .into_iter()
        .filter(|text| ai::frontmatter(text, "kind").as_deref() == Some(kind))
        .collect()
}

#[test]
fn a_summary_and_a_tool_result_reach_the_disk_while_the_index_refuses_writes() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
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
    let mut input = ai::provider("Local", "ollama", "llama3", None);
    input.context_window = Some(300);
    let id = app.save_ai_provider(input).unwrap();
    app.grant_ai_consent(&id).unwrap();
    support::settle(&mut app);
    let root = fixture.root.clone();
    ai::chat_once(&mut app, &provider, &id, &"a".repeat(100), &"b".repeat(200));
    let chat = ai::chat_ids(&app)[0].clone();
    let failed = |app: &App| {
        ai::buttons(app).is_empty()
            && ai::lines(app)
                .iter()
                .any(|line| line.starts_with("The answer failed"))
    };

    // The next message no longer fits, and Noura asks for a summary. The
    // index refuses to record the summary, which reaches the disk.
    let long = "c".repeat(400);
    ai::send(&mut app, &long, &id);
    ai::wait_until(&mut app, "the summary is asked for", |_| {
        provider.requests().len() == 2
    });
    let connection = refuse_index_writes(&fixture, &app);
    provider.say(1, SUMMARY);
    provider.finish(1);
    ai::wait_until(&mut app, "the summary is on disk", |_| {
        of_kind(&root, "context-summary")
            .iter()
            .any(|text| ai::body(text).trim() == SUMMARY)
    });
    // Noura can't read the chat again until the index takes writes, so
    // the answer fails before its request leaves.
    ai::wait_until(&mut app, "the answer fails", failed);
    assert_eq!(provider.requests().len(), 2);

    // Then the chat shows the summary, and the answer it cut off resumes
    // with the summary in place of what it covers.
    take_index_writes(&connection);
    ai::new_chat(&mut app);
    ai::open_chat(&mut app, &chat);
    ai::wait_until(&mut app, "the chat shows the summary", |app| {
        ai::lines(app).contains(&format!("md: {SUMMARY}")) && ai::buttons(app) == ["Resume"]
    });
    ai::press(&mut app, "resume");
    ai::wait_until(&mut app, "the answer is asked for", |_| {
        provider.requests().len() == 3
    });
    let answering = provider.requests()[2].clone();
    assert_eq!(
        answering.system[1],
        format!("## Summary of the earlier conversation\n{SUMMARY}")
    );
    assert_eq!(answering.messages, [format!("user: {long}")]);

    // The model calls a tool. While Noura asks, the index stops taking
    // writes again, and the result Noura writes once you allow the call
    // reaches the disk.
    provider.call_tools(
        2,
        vec![AiToolCall {
            call_id: "call_1".into(),
            name: "echoer__echo".into(),
            arguments: serde_json::json!({ "text": "hi" }),
        }],
    );
    let prompt = ai::tool_prompt(&mut app);
    refuse(&connection);
    app.answer_ai_tool(&prompt, true);
    ai::wait_until(&mut app, "the result is on disk", |_| {
        of_kind(&root, "tool-result")
            .iter()
            .any(|text| ai::body(text).trim() == "{\"text\":\"hi\"}")
    });
    ai::wait_until(&mut app, "the answer fails", failed);
    assert_eq!(provider.requests().len(), 3, "no request follows");

    // Then the call reads as completed beside its result.
    take_index_writes(&connection);
    ai::new_chat(&mut app);
    ai::open_chat(&mut app, &chat);
    let shown = |app: &App| {
        let lines = ai::lines(app);
        lines.contains(&format!("md: {SUMMARY}"))
            && lines.contains(&"Result: {\"text\":\"hi\"}".to_owned())
    };
    ai::wait_until(&mut app, "the chat shows the result", shown);
    let calls = of_kind(&root, "tool-call");
    assert_eq!(
        ai::frontmatter(&calls[0], "status").as_deref(),
        Some("completed")
    );

    // An index rebuilt from the files shows them too.
    drop(connection);
    let index = support::index_files(&fixture, &app);
    app.close_window();
    drop(app);
    for file in index {
        let _ = std::fs::remove_file(file);
    }
    let mut app = fixture.app_with(system);
    app.set_ai_client(FakeProvider::new());
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_ids(app).len() == 1
    });
    ai::open_chat(&mut app, &chat);
    ai::wait_until(&mut app, "the rebuilt chat shows them", shown);
    drop(app);
    drop(probe);
}
