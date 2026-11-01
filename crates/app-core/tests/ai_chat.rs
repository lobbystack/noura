//! Seam 1: chatting on the AI page with a fake provider. Each message and
//! its in-progress answer reach the chat's files before the request
//! leaves, the answer streams onto the page in order, its text and status
//! reach the files before the run ends, Stop keeps the partial answer, and
//! chats come back after a relaunch.

mod support;

use std::time::Duration;

use app_core::App;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
};

/// A workspace with the AI plugin on, one provider set up and consented,
/// talking to a fake provider.
/// The app comes first, so it closes before its folders go.
struct Chatting {
    app: App,
    provider: std::sync::Arc<FakeProvider>,
    id: String,
    system: std::sync::Arc<FakeSystem>,
    _plugin: support::plugins::PluginFolder,
    fixture: Fixture,
}

impl Chatting {
    fn new() -> Option<Self> {
        let fixture = Fixture::new(&[]);
        let system = FakeSystem::new();
        let mut app = fixture.app_with(system.clone());
        let provider = FakeProvider::new();
        let plugin = ai::with_ai(&mut app, &provider)?;
        let id = app
            .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
        app.grant_ai_consent(&id).unwrap();
        support::settle(&mut app);
        Some(Self {
            fixture,
            system,
            app,
            provider,
            id,
            _plugin: plugin,
        })
    }

    fn wait(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        ai::wait_until(&mut self.app, what, done);
    }

    fn requests(&self) -> usize {
        self.provider.requests().len()
    }
}

/// The files of the chat, as (path ending, text).
fn messages(fixture: &Fixture) -> Vec<String> {
    ai::chat_files(&fixture.root)
        .into_iter()
        .filter(|(path, _)| path.contains("/messages/"))
        .map(|(_, text)| text)
        .collect()
}

fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .map(str::to_owned)
}

#[test]
fn a_message_and_its_answer_reach_the_chat_files_in_order() {
    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let provider = chat.provider.clone();
    let id = chat.id.clone();

    ai::send(&mut chat.app, "Plan the **garden**\nwith tomatoes", &id);
    chat.wait("the request leaves", |_| provider.requests().len() == 1);

    // Before any text arrives, the message and an in-progress answer are
    // on disk, and the chat took its title from the first line.
    let files = messages(&chat.fixture);
    assert_eq!(files.len(), 2, "{files:?}");
    let user = files
        .iter()
        .find(|text| text.contains("kind: user"))
        .unwrap();
    assert!(user.contains("status: completed"));
    assert!(user.contains("Plan the **garden**\nwith tomatoes"));
    let answer = files
        .iter()
        .find(|text| text.contains("kind: assistant"))
        .unwrap();
    assert_eq!(
        frontmatter_value(answer, "status").as_deref(),
        Some("in-progress")
    );
    assert_eq!(
        frontmatter_value(answer, "provider_id").as_deref(),
        Some(id.as_str())
    );
    assert_eq!(
        frontmatter_value(answer, "model_id").as_deref(),
        Some("llama3")
    );
    let chat_file = ai::chat_files(&chat.fixture.root)
        .into_iter()
        .find(|(path, _)| path.ends_with("/chat.md"))
        .unwrap();
    // A new chat's files wait for its first message, so its folder takes
    // the title too.
    assert!(
        chat_file.0.starts_with("chats/plan-the-garden--"),
        "{}",
        chat_file.0
    );
    assert_eq!(
        frontmatter_value(&chat_file.1, "title").as_deref(),
        Some("Plan the **garden**")
    );
    let request = &provider.requests()[0];
    assert_eq!(
        request.messages,
        ["user: Plan the **garden**\nwith tomatoes"]
    );
    assert_eq!(request.model, "llama3");

    // The answer streams onto the page in order, as Markdown.
    provider.say(0, "Start with ");
    chat.wait("the first words show", |app| {
        ai::lines(app).contains(&"md: Start with ".to_owned())
    });
    assert_eq!(ai::buttons(&chat.app), ["Stop"]);
    provider.say(0, "**seedlings**.");
    chat.wait("the rest shows", |app| {
        ai::lines(app).contains(&"md: Start with **seedlings**.".to_owned())
    });
    provider.finish(0);
    chat.wait("the message box comes back", |app| {
        !ai::buttons(app).contains(&"Stop".to_owned())
    });

    let files = messages(&chat.fixture);
    let answer = files
        .iter()
        .find(|text| text.contains("kind: assistant"))
        .unwrap();
    assert_eq!(
        frontmatter_value(answer, "status").as_deref(),
        Some("completed")
    );
    assert!(
        answer.ends_with("\nStart with **seedlings**.\n"),
        "{answer}"
    );
    assert_eq!(
        ai::chat_titles(&chat.app),
        ["Plan the **garden**"],
        "the sidebar lists the chat"
    );
    let _ = chat.system;
}

#[test]
fn an_answer_longer_than_a_plugin_calls_deadline_keeps_streaming() {
    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let provider = chat.provider.clone();
    let id = chat.id.clone();
    ai::send(&mut chat.app, "Tell me a long story", &id);
    chat.wait("the request leaves", |_| provider.requests().len() == 1);

    // Each part comes after the plugin's own call deadline of a second
    // has passed since the message went out.
    let mut told = String::new();
    for part in ["Once ", "upon ", "a time."] {
        std::thread::sleep(Duration::from_millis(600));
        provider.say(0, part);
        told.push_str(part);
        let expected = format!("md: {told}");
        chat.wait("the part shows", |app| ai::lines(app).contains(&expected));
    }
    provider.finish(0);
    chat.wait("the answer ends", |app| {
        !ai::buttons(app).contains(&"Stop".to_owned())
    });
    let files = messages(&chat.fixture);
    assert!(
        files
            .iter()
            .any(|text| text.contains("status: completed") && text.contains("Once upon a time.")),
        "{files:?}"
    );
}

#[test]
fn stop_keeps_the_partial_answer_as_cancelled_and_the_chat_answers_again() {
    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let provider = chat.provider.clone();
    let id = chat.id.clone();
    ai::send(&mut chat.app, "Count to ten", &id);
    chat.wait("the request leaves", |_| provider.requests().len() == 1);
    provider.say(0, "One, two,");
    chat.wait("the first words show", |app| {
        ai::lines(app).contains(&"md: One, two,".to_owned())
    });

    ai::press(&mut chat.app, "stop");
    chat.wait("the answer stops", |app| {
        ai::lines(app).contains(&"Stopped.".to_owned())
    });

    assert!(provider.cancelled(0), "the provider saw the request end");
    let files = messages(&chat.fixture);
    let answer = files
        .iter()
        .find(|text| text.contains("kind: assistant"))
        .unwrap();
    assert_eq!(
        frontmatter_value(answer, "status").as_deref(),
        Some("cancelled")
    );
    assert!(answer.ends_with("\nOne, two,\n"), "{answer}");
    assert!(ai::buttons(&chat.app).is_empty(), "no Stop once it stopped");

    ai::send(&mut chat.app, "Try again", &id);
    chat.wait("the next request leaves", |_| {
        provider.requests().len() == 2
    });
    assert_eq!(
        provider.requests()[1].messages,
        [
            "user: Count to ten",
            "assistant: One, two,",
            "user: Try again"
        ]
    );
    provider.say(1, "Ten.");
    provider.finish(1);
    chat.wait("the next answer ends", |app| {
        ai::lines(app).contains(&"md: Ten.".to_owned()) && ai::buttons(app).is_empty()
    });
}

#[test]
fn chats_come_back_after_a_relaunch_and_continue_with_another_provider() {
    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let provider = chat.provider.clone();
    let first = chat.id.clone();
    let second = chat
        .app
        .save_ai_provider(ai::provider(
            "Cloud",
            "openai",
            "gpt-test",
            Some("sk-test-key"),
        ))
        .unwrap();
    chat.app.grant_ai_consent(&second).unwrap();
    ai::send(&mut chat.app, "First question", &first);
    chat.wait("the request leaves", |_| provider.requests().len() == 1);
    provider.say(0, "First answer.");
    provider.finish(0);
    chat.wait("the answer ends", |app| {
        ai::lines(app).contains(&"md: First answer.".to_owned()) && ai::buttons(app).is_empty()
    });

    let Chatting {
        fixture,
        system,
        app,
        _plugin,
        ..
    } = chat;
    drop(app);

    // Noura starts again with the same app data and workspace.
    let mut app = fixture.app_with(system.clone());
    let provider = FakeProvider::new();
    app.set_ai_client(provider.clone());
    ai::wait_until(&mut app, "the AI page shows", |app| {
        app.plugin_view(ai::PAGE).is_some()
    });
    ai::wait_until(&mut app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["First question"]
    });
    let chat_id = ai::chat_ids(&app)[0].clone();
    ai::open_chat(&mut app, &chat_id);
    assert!(ai::lines(&app).contains(&"md: First answer.".to_owned()));

    ai::send(&mut app, "Second question", &second);
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    let request = &provider.requests()[0];
    assert_eq!(request.provider, second);
    assert_eq!(request.kind, "openai");
    assert_eq!(request.key.as_deref(), Some("sk-test-key"));
    assert_eq!(
        request.messages,
        [
            "user: First question",
            "assistant: First answer.",
            "user: Second question"
        ]
    );
    provider.say(0, "Second answer.");
    provider.finish(0);
    ai::wait_until(&mut app, "the answer ends", |app| {
        ai::lines(app).contains(&"md: Second answer.".to_owned()) && ai::buttons(app).is_empty()
    });
    // Each answer records the provider and model that wrote it.
    let answers: Vec<(Option<String>, Option<String>)> = messages(&fixture)
        .iter()
        .filter(|text| text.contains("kind: assistant"))
        .map(|text| {
            (
                frontmatter_value(text, "provider_id"),
                frontmatter_value(text, "model_id"),
            )
        })
        .collect();
    assert_eq!(answers.len(), 2);
    assert!(answers.contains(&(Some(first.clone()), Some("llama3".to_owned()))));
    assert!(answers.contains(&(Some(second.clone()), Some("gpt-test".to_owned()))));
    assert!(ai::lines(&app).contains(&"Local, llama3".to_owned()));
    assert!(ai::lines(&app).contains(&"Cloud, gpt-test".to_owned()));
}

#[test]
fn switching_workspace_mid_answer_stops_it_and_keeps_the_partial_answer() {
    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let provider = chat.provider.clone();
    let id = chat.id.clone();
    ai::send(&mut chat.app, "Count to ten", &id);
    chat.wait("the request leaves", |_| provider.requests().len() == 1);
    provider.say(0, "One, two,");
    chat.wait("the first words show", |app| {
        ai::lines(app).contains(&"md: One, two,".to_owned())
    });

    // Another folder opens as a plain workspace.
    let other = chat.fixture.outside();
    chat.app.open_folder(other.clone());
    let other = other.canonicalize().unwrap();
    ai::wait_until(&mut chat.app, "the other workspace opens", |app| {
        app.workspace().is_some_and(|workspace| {
            std::path::Path::new(workspace.root()).canonicalize().ok() == Some(other.clone())
        })
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let files = messages(&chat.fixture);
        if files
            .iter()
            .any(|text| text.contains("kind: assistant") && text.contains("status: cancelled"))
        {
            let answer = files
                .iter()
                .find(|text| text.contains("kind: assistant"))
                .unwrap();
            assert!(answer.ends_with("\nOne, two,\n"), "{answer}");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the answer never ended: {files:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(provider.cancelled(0));
}

#[test]
fn a_failed_answer_shows_a_public_reason_and_records_its_code() {
    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let provider = chat.provider.clone();
    let id = chat.id.clone();
    ai::send(&mut chat.app, "Hello", &id);
    chat.wait("the request leaves", |_| provider.requests().len() == 1);
    provider.say(0, "Partly");
    provider.fail(0);
    chat.wait("the failure shows", |app| {
        ai::lines(app).contains(&"This answer failed.".to_owned())
    });
    assert!(
        ai::lines(&chat.app)
            .contains(&"The answer failed: The provider couldn't answer. Try again.".to_owned())
    );
    let files = messages(&chat.fixture);
    let answer = files
        .iter()
        .find(|text| text.contains("kind: assistant"))
        .unwrap();
    assert_eq!(
        frontmatter_value(answer, "status").as_deref(),
        Some("failed")
    );
    assert_eq!(
        frontmatter_value(answer, "error_code").as_deref(),
        Some("provider_request_failed")
    );
    // Nothing the provider said, sent or was sent reaches the page or the
    // chat's files.
    for raw in ["bad key", "authorization", "sk-raw-key"] {
        assert!(
            ai::lines(&chat.app).iter().all(|line| !line.contains(raw)),
            "{raw}"
        );
        assert!(files.iter().all(|text| !text.contains(raw)), "{raw}");
    }
    assert!(ai::RAW_FAILURE.contains("sk-raw-key"));
    let _ = chat.requests();
}

/// The message box's text, as the page draws it.
#[cfg(unix)]
fn draft(app: &App) -> Option<String> {
    let view = ai::page(app);
    let composer = ai::find(view.root(), "composer")?;
    composer
        .children
        .iter()
        .find_map(|child| match &child.kind {
            app_core::ViewKind::Input {
                value: app_core::InputValue::Multiline(text),
                ..
            } => Some(text.clone()),
            _ => None,
        })
}

/// When Noura can't write your message to the chat, nothing goes to the
/// provider and the message comes back to the box.
#[cfg(unix)]
#[test]
fn a_message_noura_couldnt_write_comes_back_to_the_message_box() {
    use std::os::unix::fs::PermissionsExt;

    let Some(mut chat) = Chatting::new() else {
        return;
    };
    let id = chat.id.clone();
    // The chats folder takes no new file, as on a full or read-only disk.
    let chats = chat.fixture.root.join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o555)).unwrap();

    ai::send(&mut chat.app, "Plan the garden", &id);
    chat.wait("the failure shows", |app| {
        ai::lines(app)
            .iter()
            .any(|line| line.starts_with("The answer failed: "))
    });
    std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(chat.requests(), 0, "nothing went to the provider");
    assert!(ai::chat_files(&chat.fixture.root).is_empty());
    assert_eq!(draft(&chat.app).as_deref(), Some("Plan the garden"));

    // Once the folder takes files again, the message goes out.
    ai::send(&mut chat.app, "Plan the garden", &id);
    let provider = chat.provider.clone();
    chat.wait("the request leaves", |_| provider.requests().len() == 1);
    assert_eq!(messages(&chat.fixture).len(), 2);
}
