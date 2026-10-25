//! Seam 1: consent before a provider receives the workspace's content.
//! The fake provider gets no request until you consent for this workspace
//! and provider in Noura's own prompt or in Settings. Noura keeps the
//! consent on this device and never in the workspace. It lapses when the
//! provider's kind, endpoint or model changes or the policy version does,
//! and revoking it cancels the provider's running requests.

mod support;

use std::sync::Arc;

use app_core::{AI_POLICY_VERSION, AiProviderInput, App};
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
};

/// A workspace with the AI plugin on and one provider set up, without
/// consent.
struct Asking {
    fixture: Fixture,
    system: Arc<FakeSystem>,
    app: App,
    provider: Arc<FakeProvider>,
    id: String,
    _plugin: support::plugins::PluginFolder,
}

impl Asking {
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
            system,
            app,
            provider,
            id,
            _plugin: plugin,
        })
    }

    fn message_files(&self) -> usize {
        ai::chat_files(&self.fixture.root)
            .iter()
            .filter(|(path, _)| path.contains("/messages/"))
            .count()
    }
}

fn asking_line() -> String {
    "Noura is asking you whether Local may receive this workspace's content.".to_owned()
}

#[test]
fn nothing_goes_to_a_provider_before_you_consent_in_noura_s_prompt() {
    let Some(mut asking) = Asking::new() else {
        return;
    };
    let id = asking.id.clone();
    assert!(!asking.app.ai_providers()[0].consented);

    ai::send(&mut asking.app, "Plan the garden", &id);

    let prompt = asking.app.ai_consent_prompt().expect("Noura asks");
    assert_eq!(prompt.provider_id, id);
    assert_eq!(prompt.provider_name, "Local");
    assert_eq!(prompt.kind, "ollama");
    assert_eq!(prompt.model, "llama3");
    assert_eq!(prompt.workspace, "Garden");
    assert!(ai::lines(&asking.app).contains(&asking_line()));
    assert_eq!(asking.provider.requests().len(), 0);
    assert!(
        ai::chat_files(&asking.fixture.root).is_empty(),
        "nothing is written yet, not even the new chat"
    );

    // Saying no sends and writes nothing, and the message comes back to
    // the box.
    asking.app.deny_ai_consent();
    support::settle(&mut asking.app);
    assert_eq!(asking.app.ai_consent_prompt(), None);
    ai::wait_until(&mut asking.app, "the page says you said no", |app| {
        ai::lines(app).contains(
            &"You didn't let Local receive this workspace's content, so Noura sent nothing."
                .to_owned(),
        )
    });
    assert_eq!(asking.provider.requests().len(), 0);
    assert!(ai::chat_files(&asking.fixture.root).is_empty());
    assert!(
        ai::chat_titles(&asking.app).is_empty(),
        "no empty chat is left"
    );
    let view = ai::page(&asking.app);
    let composer = ai::find(view.root(), "composer").expect("the message box shows");
    assert!(
        composer.children.iter().any(|child| matches!(
            &child.kind,
            app_core::ViewKind::Input { value: app_core::InputValue::Multiline(text), .. }
                if text == "Plan the garden"
        )),
        "{composer:?}"
    );

    // Allowing sends it.
    ai::send(&mut asking.app, "Plan the garden", &id);
    assert!(asking.app.ai_consent_prompt().is_some());
    asking.app.allow_ai_consent();
    let provider = asking.provider.clone();
    ai::wait_until(&mut asking.app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert_eq!(asking.app.ai_consent_prompt(), None);
    assert!(asking.app.ai_providers()[0].consented);
    assert_eq!(asking.message_files(), 2);
    assert_eq!(ai::chat_files(&asking.fixture.root).len(), 3);

    // The consent lives in this device's app data, never in the
    // workspace.
    let consents =
        std::fs::read_to_string(asking.fixture.app_data.path().join("ai/consents.json")).unwrap();
    let workspace_id = asking.app.workspace().unwrap().id().to_owned();
    assert!(consents.contains(&workspace_id));
    assert!(consents.contains(AI_POLICY_VERSION));
    for (path, bytes) in asking.fixture.snapshot() {
        let text = String::from_utf8_lossy(bytes.as_deref().unwrap_or_default()).into_owned();
        assert!(
            !text.contains("consent") && !text.contains("policyVersion"),
            "{path}: {text}"
        );
    }
    let noura = asking.fixture.root.join(".noura");
    let mut found = Vec::new();
    holding(&noura, "consent", &mut found);
    assert_eq!(found, Vec::<String>::new());
    let _ = asking.system;
}

fn holding(folder: &std::path::Path, text: &str, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            holding(&path, text, found);
        } else if String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).contains(text)
        {
            found.push(path.to_string_lossy().into_owned());
        }
    }
}

#[test]
fn consent_lapses_when_the_provider_s_kind_endpoint_or_model_changes() {
    let Some(mut asking) = Asking::new() else {
        return;
    };
    let id = asking.id.clone();
    let changes = [
        AiProviderInput {
            id: Some(id.clone()),
            ..ai::provider("Local", "ollama", "llama3.1", None)
        },
        AiProviderInput {
            id: Some(id.clone()),
            endpoint: Some("http://127.0.0.1:11434/v1/".into()),
            ..ai::provider("Local", "ollama", "llama3.1", None)
        },
        AiProviderInput {
            id: Some(id.clone()),
            endpoint: Some("http://127.0.0.1:11434/v1/".into()),
            ..ai::provider("Local", "openai", "llama3.1", None)
        },
    ];
    let mut sent = 0;
    for change in changes {
        asking.app.grant_ai_consent(&id).unwrap();
        assert!(asking.app.ai_providers()[0].consented);
        asking.app.save_ai_provider(change.clone()).unwrap();
        support::settle(&mut asking.app);
        assert!(!asking.app.ai_providers()[0].consented, "{change:?}");

        ai::new_chat(&mut asking.app);
        ai::send(&mut asking.app, "Hello", &id);
        assert!(asking.app.ai_consent_prompt().is_some(), "{change:?}");
        assert_eq!(asking.provider.requests().len(), sent, "{change:?}");
        asking.app.allow_ai_consent();
        sent += 1;
        let provider = asking.provider.clone();
        ai::wait_until(&mut asking.app, "the request leaves", |_| {
            provider.requests().len() == sent
        });
        provider.finish(sent - 1);
        ai::wait_until(&mut asking.app, "the answer ends", |app| {
            ai::buttons(app).is_empty()
        });
    }
    let last = &asking.provider.requests()[2];
    assert_eq!(last.kind, "openai");
    assert_eq!(last.endpoint.as_deref(), Some("http://127.0.0.1:11434/v1/"));
    assert_eq!(last.model, "llama3.1");
}

#[test]
fn consent_lapses_when_noura_s_ai_policy_version_changes() {
    let Some(mut asking) = Asking::new() else {
        return;
    };
    let id = asking.id.clone();
    asking.app.grant_ai_consent(&id).unwrap();
    let Asking {
        fixture,
        system,
        app,
        _plugin,
        ..
    } = asking;
    drop(app);

    // The grant on this device names an earlier policy, as after an update
    // to a Noura with a new one.
    let path = fixture.app_data.path().join("ai/consents.json");
    let consents = std::fs::read_to_string(&path).unwrap();
    assert!(consents.contains(AI_POLICY_VERSION));
    std::fs::write(&path, consents.replace(AI_POLICY_VERSION, "2025-01")).unwrap();

    let mut app = fixture.app_with(system);
    let provider = FakeProvider::new();
    app.set_ai_client(provider.clone());
    ai::wait_until(&mut app, "the AI page shows", |app| {
        app.plugin_view(ai::PAGE).is_some()
    });
    assert!(!app.ai_providers()[0].consented);
    ai::wait_until(&mut app, "the message box shows", |app| {
        ai::buttons(app).is_empty() && ai::lines(app).contains(&"New chat".to_owned())
    });
    ai::send(&mut app, "Hello", &id);
    assert!(app.ai_consent_prompt().is_some());
    assert_eq!(provider.requests().len(), 0);
}

#[test]
fn revoking_consent_cancels_the_running_request_and_the_next_one_asks_again() {
    let Some(mut asking) = Asking::new() else {
        return;
    };
    let id = asking.id.clone();
    asking.app.grant_ai_consent(&id).unwrap();
    support::settle(&mut asking.app);
    ai::send(&mut asking.app, "Tell me about soil", &id);
    let provider = asking.provider.clone();
    ai::wait_until(&mut asking.app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    provider.say(0, "Soil holds");
    ai::wait_until(&mut asking.app, "the first words show", |app| {
        ai::lines(app).contains(&"md: Soil holds".to_owned())
    });

    asking.app.revoke_ai_consent(&id).unwrap();
    ai::wait_until(&mut asking.app, "the answer stops", |app| {
        ai::lines(app).contains(&"Stopped.".to_owned())
    });

    assert!(provider.cancelled(0), "the provider saw the cancellation");
    assert!(!asking.app.ai_providers()[0].consented);
    let answer = ai::chat_files(&asking.fixture.root)
        .into_iter()
        .map(|(_, text)| text)
        .find(|text| text.contains("kind: assistant"))
        .unwrap();
    assert!(answer.contains("status: cancelled"), "{answer}");
    assert!(answer.ends_with("\nSoil holds\n"), "{answer}");

    // The next request waits for consent again.
    ai::send(&mut asking.app, "And now?", &id);
    assert!(asking.app.ai_consent_prompt().is_some());
    assert_eq!(provider.requests().len(), 1);
    assert!(ai::lines(&asking.app).contains(&asking_line()));
    assert_eq!(ai::buttons(&asking.app), ["Stop"]);

    // Stop while it waits writes nothing.
    ai::press(&mut asking.app, "stop");
    assert_eq!(asking.app.ai_consent_prompt(), None);
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(asking.message_files(), 2);
}

/// The message box's text, as the page draws it.
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

#[test]
fn removing_a_provider_ends_its_wait_moves_the_prompt_on_and_forgets_its_consent() {
    let Some(mut asking) = Asking::new() else {
        return;
    };
    let local = asking.id.clone();
    let cloud = asking
        .app
        .save_ai_provider(ai::provider("Cloud", "openai", "gpt-test", None))
        .unwrap();
    support::settle(&mut asking.app);

    // A message waits for consent to Local, and one in another chat for
    // consent to Cloud.
    ai::send(&mut asking.app, "For Local", &local);
    assert_eq!(
        asking
            .app
            .ai_consent_prompt()
            .map(|prompt| prompt.provider_id),
        Some(local.clone())
    );
    ai::new_chat(&mut asking.app);
    ai::send(&mut asking.app, "For Cloud", &cloud);
    assert_eq!(
        asking
            .app
            .ai_consent_prompt()
            .map(|prompt| prompt.provider_id),
        Some(local.clone())
    );

    // Local goes: its run ends, and the prompt asks about Cloud.
    asking.app.remove_ai_provider(&local).unwrap();
    support::settle(&mut asking.app);
    assert_eq!(
        asking
            .app
            .ai_consent_prompt()
            .map(|prompt| prompt.provider_id),
        Some(cloud.clone())
    );
    asking.app.deny_ai_consent();
    support::settle(&mut asking.app);
    assert_eq!(asking.app.ai_consent_prompt(), None);
    ai::wait_until(&mut asking.app, "Cloud's message comes back", |app| {
        draft(app).as_deref() == Some("For Cloud")
    });
    assert_eq!(asking.provider.requests().len(), 0);
    assert!(ai::chat_files(&asking.fixture.root).is_empty());

    // A provider you add again under the same name asks again: its old
    // consent went with it.
    asking.app.grant_ai_consent(&cloud).unwrap();
    assert!(
        asking
            .app
            .ai_providers()
            .iter()
            .any(|row| row.id == cloud && row.consented)
    );
    asking.app.remove_ai_provider(&cloud).unwrap();
    let again = asking
        .app
        .save_ai_provider(ai::provider("Cloud", "openai", "gpt-test", None))
        .unwrap();
    assert_eq!(again, cloud, "the same name gives the same ID");
    support::settle(&mut asking.app);
    assert!(
        asking
            .app
            .ai_providers()
            .iter()
            .any(|row| row.id == cloud && !row.consented)
    );
    ai::send(&mut asking.app, "For Cloud", &cloud);
    assert_eq!(
        asking
            .app
            .ai_consent_prompt()
            .map(|prompt| prompt.provider_id),
        Some(cloud)
    );
    assert_eq!(asking.provider.requests().len(), 0);
}

/// A new chat waits for its first message to go out before Noura lists
/// it, and the chats already there stay in the sidebar meanwhile.
#[test]
fn a_new_chat_waiting_for_consent_leaves_the_other_chats_listed() {
    let Some(mut asking) = Asking::new() else {
        return;
    };
    let id = asking.id.clone();
    ai::send(&mut asking.app, "Plan the garden", &id);
    asking.app.allow_ai_consent();
    let provider = asking.provider.clone();
    ai::wait_until(&mut asking.app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    provider.finish(0);
    ai::wait_until(&mut asking.app, "the chat is listed", |app| {
        ai::chat_titles(app) == ["Plan the garden"] && ai::buttons(app).is_empty()
    });
    asking.app.revoke_ai_consent(&id).unwrap();

    ai::new_chat(&mut asking.app);
    ai::send(&mut asking.app, "Plan the orchard", &id);
    assert!(asking.app.ai_consent_prompt().is_some());
    assert!(
        matches!(asking.app.plugin_sidebar(ai::PAGE), Some(Ok(_))),
        "the sidebar draws"
    );
    assert_eq!(ai::chat_titles(&asking.app), ["Plan the garden"]);

    asking.app.deny_ai_consent();
    support::settle(&mut asking.app);
    assert!(matches!(asking.app.plugin_sidebar(ai::PAGE), Some(Ok(_))));
    assert_eq!(ai::chat_titles(&asking.app), ["Plan the garden"]);
    assert_eq!(asking.message_files(), 2);
}
