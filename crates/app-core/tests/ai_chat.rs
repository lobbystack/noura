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
struct Chatting {
    fixture: Fixture,
    system: std::sync::Arc<FakeSystem>,
    app: App,
    provider: std::sync::Arc<FakeProvider>,
    id: String,
    _plugin: support::plugins::PluginFolder,
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
    // The folder keeps the name it got as a new chat, as renaming a chat
    // does today.
    assert!(
        chat_file.0.starts_with("chats/new-chat--"),
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
    let _ = chat.requests();
}
