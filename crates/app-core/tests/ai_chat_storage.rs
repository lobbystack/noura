//! Seam 1: the AGENTS.md storage matrix for chats on the AI page:
//! malformed input, round trips, external edits, stale indexes,
//! interrupted index updates, moves, duplicate IDs, path traversal and
//! rebuilds. Chat files keep today's v1 layout, and each check reads the
//! disk once the page shows the result.

mod support;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use app_core::App;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins,
};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";
const C: &str = "01jbcdefghjkmnpqrstvwxyz0c";
const D: &str = "01jbcdefghjkmnpqrstvwxyz0d";
const E: &str = "01jbcdefghjkmnpqrstvwxyz0e";

/// A chat's `chat.md`, as an earlier session wrote it.
fn chat(suffix: &str, title: &str) -> Vec<u8> {
    format!(
        "---\nid: chat_{suffix}\ntype: chat\ntitle: {title}\nretention: permanent\n\
         retention_days: null\ncreated: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:05:00Z\n---\n"
    )
    .into_bytes()
}

/// A message of the chat `chat`, as an earlier session wrote it.
fn message(suffix: &str, chat: &str, kind: &str, minute: u32, content: &str) -> Vec<u8> {
    let provider = if kind == "assistant" {
        "provider_id: local\nmodel_id: llama3\n"
    } else {
        "provider_id: null\nmodel_id: null\n"
    };
    format!(
        "---\nid: chat-message_{suffix}\ntype: chat-message\nchat_id: chat_{chat}\nrun_id: run_1\n\
         kind: {kind}\nstatus: completed\ncontent_type: text/markdown\n{provider}tool_call_id: null\n\
         tool_name: null\nerror_code: null\nsummarizes_through_message_id: null\n\
         created: 2026-10-20T10:0{minute}:00Z\nupdated: 2026-10-20T10:0{minute}:00Z\n---\n\n{content}\n"
    )
    .into_bytes()
}

/// The garden chat's three files.
fn garden() -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("chats/garden--wxyz0a/chat.md"),
            chat(A, "Garden plans"),
        ),
        (
            format!("chats/garden--wxyz0a/messages/2026-10-20/chat-message_{B}.md"),
            message(B, A, "user", 1, "What should I plant?"),
        ),
        (
            format!("chats/garden--wxyz0a/messages/2026-10-20/chat-message_{C}.md"),
            message(C, A, "assistant", 2, "Plant **garlic**."),
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

/// The app on `fixture` with the AI plugin on, a provider set up and
/// consented, and the fake provider it talks to.
fn open(
    fixture: &Fixture,
    system: &Arc<FakeSystem>,
) -> Option<(App, Arc<FakeProvider>, String, plugins::PluginFolder)> {
    let mut app = fixture.app_with(system.clone());
    let provider = FakeProvider::new();
    let plugin = ai::with_ai(&mut app, &provider)?;
    let id = app
        .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
        .unwrap();
    app.grant_ai_consent(&id).unwrap();
    support::settle(&mut app);
    Some((app, provider, id, plugin))
}

/// Send `text` and stream `answer` back, and wait until it shows.
fn chat_once(app: &mut App, provider: &FakeProvider, id: &str, text: &str, answer: &str) {
    let sent = provider.requests().len();
    ai::send(app, text, id);
    ai::wait_until(app, "the request leaves", |_| {
        provider.requests().len() == sent + 1
    });
    provider.say(sent, answer);
    provider.finish(sent);
    let shown = format!("md: {answer}");
    ai::wait_until(app, "the answer shows", |app| {
        ai::lines(app).contains(&shown) && ai::buttons(app).is_empty()
    });
}

fn index_files(fixture: &Fixture, app: &App) -> Vec<PathBuf> {
    support::index_files(fixture, app)
}

#[test]
fn malformed_chat_files_stay_as_they_are_and_hide_no_other_chat() {
    let mut files = garden();
    files.push((
        "chats/broken--wxyz0d/chat.md".to_owned(),
        b"---\nid: chat_x\ntype: [unclosed\n---\n".to_vec(),
    ));
    files.push((
        "chats/forever--wxyz0e/chat.md".to_owned(),
        String::from_utf8(chat(E, "Forever"))
            .unwrap()
            .replace("retention: permanent", "retention: forever")
            .into_bytes(),
    ));
    files.push((
        format!("chats/garden--wxyz0a/messages/2026-10-20/chat-message_{D}.md"),
        String::from_utf8(message(D, A, "user", 3, "Odd"))
            .unwrap()
            .replace("kind: user", "kind: shouting")
            .into_bytes(),
    ));
    files.push(("chats/notes.md".to_owned(), b"\xff\xfe not UTF-8".to_vec()));
    let fixture = fixture_with(&files);
    let system = FakeSystem::new();
    let before = fixture.snapshot();
    let Some((mut app, _provider, _id, _plugin)) = open(&fixture, &system) else {
        return;
    };

    ai::wait_until(&mut app, "the good chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));
    let lines = ai::lines(&app);
    assert!(
        lines.contains(&"md: What should I plant?".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"md: Plant **garlic**.".to_owned()),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|line| line.contains("Odd")), "{lines:?}");
    drop(app);

    assert_eq!(fixture.snapshot(), before, "no file changed");
}

#[test]
fn chats_round_trip_and_reading_one_changes_no_byte() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, provider, id, _plugin)) = open(&fixture, &system) else {
        return;
    };
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    let before = fixture.snapshot();
    ai::open_chat(&mut app, &format!("chat_{A}"));
    ai::new_chat(&mut app);
    ai::open_chat(&mut app, &format!("chat_{A}"));
    assert_eq!(fixture.snapshot(), before, "reading a chat writes nothing");

    chat_once(&mut app, &provider, &id, "And when?", "In **October**.");
    // What Noura wrote reads back through the workspace format as the
    // chat's objects, in today's layout.
    let written: Vec<(String, String)> = ai::chat_files(&fixture.root)
        .into_iter()
        .filter(|(path, _)| !before.iter().any(|(known, _)| known == path))
        .collect();
    assert_eq!(written.len(), 2, "{written:?}");
    for (path, text) in &written {
        let local_core::ParsedMarkdown::Managed(object) =
            local_core::parse_markdown(path, text.as_bytes())
        else {
            panic!("{path} isn't a managed object");
        };
        assert_eq!(object.object_type, "chat-message");
        assert_eq!(object.properties["chat_id"], format!("chat_{A}"));
        let keys: Vec<&str> = text
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .filter_map(|line| line.split_once(':').map(|(key, _)| key))
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "type",
                "chat_id",
                "run_id",
                "kind",
                "status",
                "content_type",
                "provider_id",
                "model_id",
                "tool_call_id",
                "tool_name",
                "error_code",
                "summarizes_through_message_id",
                "created",
                "updated"
            ],
            "{path}"
        );
    }
    let chat_md = String::from_utf8(fixture.read("chats/garden--wxyz0a/chat.md")).unwrap();
    assert!(chat_md.starts_with(&format!(
        "---\nid: chat_{A}\ntype: chat\ntitle: Garden plans\nretention: permanent\nretention_days: null\ncreated: 2026-10-20T10:00:00Z\nupdated: "
    )));
}

#[test]
fn an_edit_made_elsewhere_shows_on_the_page_and_wins() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, provider, id, _plugin)) = open(&fixture, &system) else {
        return;
    };
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));

    // Another app renames the chat.
    let path = "chats/garden--wxyz0a/chat.md";
    let renamed = String::from_utf8(fixture.read(path))
        .unwrap()
        .replace("title: Garden plans", "title: Kitchen garden");
    support::write(&fixture.root, path, renamed.as_bytes());
    ai::wait_until(&mut app, "the new title shows", |app| {
        ai::chat_titles(app) == ["Kitchen garden"]
            && ai::lines(app).contains(&"Kitchen garden".to_owned())
    });

    // It renames the chat again while an answer streams in. The answer
    // lands, and the title stays as the other app wrote it.
    ai::send(&mut app, "And when?", &id);
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    let renamed = String::from_utf8(fixture.read(path))
        .unwrap()
        .replace("title: Kitchen garden", "title: Winter garden");
    support::write(&fixture.root, path, renamed.as_bytes());
    provider.say(0, "In **October**.");
    provider.finish(0);
    ai::wait_until(&mut app, "the answer shows", |app| {
        ai::lines(app).contains(&"md: In **October**.".to_owned()) && ai::buttons(app).is_empty()
    });
    let chat_md = String::from_utf8(fixture.read(path)).unwrap();
    assert!(chat_md.contains("title: Winter garden"), "{chat_md}");
    assert!(
        ai::chat_files(&fixture.root)
            .iter()
            .any(|(_, text)| text.contains("status: completed") && text.contains("In **October**.")),
    );
}

#[test]
fn a_deleted_or_stale_index_rebuilds_the_same_chats() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, provider, id, plugin)) = open(&fixture, &system) else {
        return;
    };
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));
    chat_once(&mut app, &provider, &id, "And when?", "In October.");
    let index = index_files(&fixture, &app);
    drop(app);

    // Every index file goes, and another app adds a message meanwhile.
    for file in &index {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
    support::write(
        &fixture.root,
        &format!("chats/garden--wxyz0a/messages/2026-10-21/chat-message_{D}.md"),
        &message(D, A, "user", 9, "Added elsewhere"),
    );

    let mut app = fixture.app_with(system.clone());
    let provider = FakeProvider::new();
    app.set_ai_client(provider.clone());
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));
    let lines = ai::lines(&app);
    for expected in [
        "md: What should I plant?",
        "md: Plant **garlic**.",
        "md: And when?",
        "md: In October.",
        "md: Added elsewhere",
    ] {
        assert!(
            lines.contains(&expected.to_owned()),
            "{expected}: {lines:?}"
        );
    }
    drop(plugin);
}

/// Stop the index from changing its file list, as a full disk or a broken
/// index would, through triggers added by a second connection.
fn refuse_index_writes(fixture: &Fixture, app: &App) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(&index_files(fixture, app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();
    connection
}

#[test]
fn an_answer_reaches_its_files_while_the_index_refuses_writes() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, provider, id, _plugin)) = open(&fixture, &system) else {
        return;
    };
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));
    ai::send(&mut app, "And when?", &id);
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });

    // The index stops taking writes while the answer streams in.
    let connection = refuse_index_writes(&fixture, &app);
    provider.say(0, "In October.");
    provider.finish(0);
    ai::wait_until(&mut app, "the answer ends", |app| {
        ai::buttons(app).is_empty()
    });

    // The answer is on disk though the index couldn't record it.
    let files = ai::chat_files(&fixture.root);
    assert!(
        files
            .iter()
            .any(|(_, text)| text.contains("kind: assistant")
                && text.contains("status: completed")
                && text.contains("In October.")),
        "{files:?}"
    );

    // Once the index takes writes again, the chat shows it.
    connection
        .execute_batch("DROP TRIGGER refuse_insert; DROP TRIGGER refuse_update;")
        .unwrap();
    ai::new_chat(&mut app);
    ai::open_chat(&mut app, &format!("chat_{A}"));
    ai::wait_until(&mut app, "the answer shows", |app| {
        ai::lines(app).contains(&"md: In October.".to_owned())
    });
}

#[test]
fn a_chat_moved_elsewhere_keeps_its_messages_and_takes_new_ones_there() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, provider, id, _plugin)) = open(&fixture, &system) else {
        return;
    };
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });

    std::fs::create_dir_all(fixture.root.join("Archive")).unwrap();
    std::fs::rename(
        fixture.root.join("chats/garden--wxyz0a"),
        fixture.root.join("Archive/garden"),
    )
    .unwrap();
    ai::open_chat(&mut app, &format!("chat_{A}"));
    ai::wait_until(&mut app, "the moved chat shows", |app| {
        ai::chat_titles(app) == ["Garden plans"]
            && ai::lines(app).contains(&"md: Plant **garlic**.".to_owned())
    });

    chat_once(&mut app, &provider, &id, "And when?", "In October.");
    let moved = list(&fixture.root.join("Archive/garden"));
    assert_eq!(moved.len(), 5, "{moved:?}");
    assert!(!fixture.exists("chats/garden--wxyz0a"));
}

fn list(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(list(&path));
        } else {
            found.push(path.to_string_lossy().into_owned());
        }
    }
    found
}

#[test]
fn a_copied_chat_takes_no_message_until_one_copy_goes() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, provider, id, _plugin)) = open(&fixture, &system) else {
        return;
    };
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));

    // Another app copies the chat's folder, so two chat.md files hold one
    // stable ID. The workspace's problems list them, the chat leaves the
    // list, and the open chat says why it takes no message.
    for (path, bytes) in garden() {
        support::write(
            &fixture.root,
            &path.replace("chats/garden--wxyz0a", "Copies/garden"),
            &bytes,
        );
    }
    ai::wait_until(&mut app, "the copy shows as a problem", |app| {
        !app.problems().is_empty()
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));
    ai::wait_until(&mut app, "the page says why", |app| {
        ai::lines(app)
            .iter()
            .any(|line| line.contains("more than one file"))
    });
    assert!(ai::chat_titles(&app).is_empty());
    let before = fixture.snapshot();
    ai::send(&mut app, "And when?", &id);
    assert_eq!(provider.requests().len(), 0);
    assert_eq!(fixture.snapshot(), before, "nothing was written");
    assert!(
        ai::lines(&app)
            .iter()
            .any(|line| line.contains("more than one file"))
    );

    // Once the copy goes, the chat takes messages again.
    std::fs::remove_dir_all(fixture.root.join("Copies")).unwrap();
    ai::wait_until(&mut app, "the problem goes", |app| {
        app.problems().is_empty()
    });
    ai::wait_until(&mut app, "the chat is listed again", |app| {
        ai::chat_titles(app) == ["Garden plans"]
    });
    ai::open_chat(&mut app, &format!("chat_{A}"));
    chat_once(&mut app, &provider, &id, "And when?", "In October.");
}

#[test]
fn a_plugin_can_name_no_chat_outside_the_workspace() {
    let fixture = fixture_with(&garden());
    let system = FakeSystem::new();
    let Some((mut app, _provider, id, _plugin)) = open(&fixture, &system) else {
        return;
    };
    let Some(probe) = plugins::folder_for(
        "probe_plugin",
        "chatter",
        &["name = \"views\"", "name = \"ai\""],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "chatter");
    let outside = fixture.outside();
    let before = fixture.snapshot();

    for chat in [
        "../../Outside/chat",
        "chat_../../Outside",
        "/etc/passwd",
        &format!("chat_{A}/../../x"),
    ] {
        for call in [
            format!("ai:read-chat:{chat}"),
            format!("ai:send:{chat}:{id}:Hello"),
        ] {
            assert_eq!(
                press(&mut app, &call),
                [format!("hello ai error not-found {chat}")],
                "{call}"
            );
        }
    }
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
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
