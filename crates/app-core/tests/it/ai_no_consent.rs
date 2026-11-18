//! Seam 1: choosing a provider is your say-so (ADR 0006). A message goes
//! to the provider you picked without a prompt, and nothing about consent
//! is kept in the app data or the workspace. Removing the provider stops
//! its running request, and the next message to it fails.

use crate::support;
use std::sync::Arc;

use app_core::App;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
};

/// A workspace with the AI plugin on and one provider set up.
struct Chatting {
    fixture: Fixture,
    _system: Arc<FakeSystem>,
    app: App,
    provider: Arc<FakeProvider>,
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
        support::settle(&mut app);
        Some(Self {
            fixture,
            _system: system,
            app,
            provider,
            id,
            _plugin: plugin,
        })
    }
}

/// Every file under `folder` whose text holds `text`.
fn holding(folder: &std::path::Path, text: &str, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            holding(&path, text, found);
        } else if std::fs::read_to_string(&path).is_ok_and(|bytes| bytes.contains(text)) {
            found.push(path.display().to_string());
        }
    }
}

#[test]
fn a_message_goes_to_the_provider_you_picked_without_asking() {
    let Some(mut chatting) = Chatting::new() else {
        return;
    };
    let id = chatting.id.clone();

    ai::send(&mut chatting.app, "Plan the garden", &id);

    let provider = chatting.provider.clone();
    ai::wait_until(&mut chatting.app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert!(chatting.app.ai_tool_prompt().is_none());
    provider.say(0, "Start with beans.");
    provider.finish(0);
    ai::wait_until(&mut chatting.app, "the answer shows", |app| {
        ai::lines(app).contains(&"md: Start with beans.".to_owned())
    });

    // Nothing about consent lands anywhere.
    let mut found = Vec::new();
    holding(chatting.fixture.app_data.path(), "consent", &mut found);
    holding(&chatting.fixture.root, "consent", &mut found);
    assert_eq!(found, Vec::<String>::new());
    assert!(
        !chatting
            .fixture
            .app_data
            .path()
            .join("ai/consents.json")
            .exists()
    );
}

#[test]
fn removing_a_provider_stops_its_request_and_the_next_message_fails() {
    let Some(mut chatting) = Chatting::new() else {
        return;
    };
    let id = chatting.id.clone();
    ai::send(&mut chatting.app, "Tell me about soil", &id);
    let provider = chatting.provider.clone();
    ai::wait_until(&mut chatting.app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    provider.say(0, "Soil holds");
    ai::wait_until(&mut chatting.app, "the first words show", |app| {
        ai::lines(app).contains(&"md: Soil holds".to_owned())
    });

    chatting.app.remove_ai_provider(&id).unwrap();
    ai::wait_until(&mut chatting.app, "the answer stops", |app| {
        ai::lines(app).contains(&"Stopped.".to_owned())
    });

    assert!(provider.cancelled(0), "the provider saw the cancellation");
    let answer = ai::chat_files(&chatting.fixture.root)
        .into_iter()
        .map(|(_, text)| text)
        .find(|text| text.contains("kind: assistant"))
        .unwrap();
    assert!(answer.contains("status: cancelled"), "{answer}");
    assert!(answer.ends_with("\nSoil holds\n"), "{answer}");
    assert_eq!(provider.requests().len(), 1);
}
