//! Seam 1: AI providers in Settings. You add several, each with a kind
//! Noura's provider client supports, a model and an optional endpoint
//! that must use HTTPS unless it points at this computer. Their settings
//! stay in this device's app data, and their keys only in the credential
//! store: after a chat, no key is in the workspace, the index, the app
//! data or the logs, and the AI page never shows one.

use crate::support;

use app_core::AiProviderInput;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
};

const KEY: &str = "sk-live-0123456789abcdef";

#[test]
fn you_add_several_providers_each_with_a_supported_kind_model_and_endpoint() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();

    let cloud = app
        .save_ai_provider(ai::provider("Cloud", "openai", "gpt-test", None))
        .unwrap();
    let local = app
        .save_ai_provider(AiProviderInput {
            endpoint: Some("http://localhost:11434/v1/".into()),
            ..ai::provider("Local", "ollama", "llama3", None)
        })
        .unwrap();
    let hosted = app
        .save_ai_provider(AiProviderInput {
            endpoint: Some("https://models.example.com/v1/".into()),
            ..ai::provider("Hosted", "anthropic", "claude-test", None)
        })
        .unwrap();
    let rows: Vec<(String, String, String, Option<String>)> = app
        .ai_providers()
        .into_iter()
        .map(|row| (row.id, row.kind, row.model, row.endpoint))
        .collect();
    assert_eq!(
        rows,
        [
            (cloud.clone(), "openai".into(), "gpt-test".into(), None),
            (
                local,
                "ollama".into(),
                "llama3".into(),
                Some("http://localhost:11434/v1/".into())
            ),
            (
                hosted,
                "anthropic".into(),
                "claude-test".into(),
                Some("https://models.example.com/v1/".into())
            ),
        ]
    );
    assert!(
        app.ai_provider_kinds()
            .iter()
            .any(|(kind, _)| *kind == "ollama")
    );

    // A kind the client doesn't support, and plain HTTP to another
    // computer, are refused, and nothing changes.
    let unknown = app
        .save_ai_provider(ai::provider("Odd", "not-a-provider", "m", None))
        .unwrap_err();
    assert_eq!(unknown.code, "provider_kind_invalid");
    let insecure = app
        .save_ai_provider(AiProviderInput {
            endpoint: Some("http://192.168.1.20:11434/".into()),
            ..ai::provider("Office", "ollama", "llama3", None)
        })
        .unwrap_err();
    assert_eq!(insecure.code, "endpoint_insecure");
    let changed = app
        .save_ai_provider(AiProviderInput {
            id: Some(cloud.clone()),
            endpoint: Some("http://api.example.com/v1/".into()),
            ..ai::provider("Cloud", "openai", "gpt-test", None)
        })
        .unwrap_err();
    assert_eq!(changed.code, "endpoint_insecure");
    assert_eq!(app.ai_providers().len(), 3);
    assert_eq!(app.ai_providers()[0].endpoint, None);

    // The settings live in this device's app data, outside the workspace.
    let settings =
        std::fs::read_to_string(fixture.app_data.path().join("ai/providers.json")).unwrap();
    assert!(settings.contains("gpt-test"));
    let workspace = fixture.snapshot();
    assert!(
        workspace.iter().all(|(_, bytes)| bytes
            .as_deref()
            .is_none_or(|bytes| !String::from_utf8_lossy(bytes).contains("gpt-test"))),
        "{workspace:?}"
    );

    // A relaunch reads them back.
    drop(app);
    let app = fixture.app();
    assert_eq!(app.ai_providers().len(), 3);

    // Removing one takes it out.
    let mut app = app;
    app.remove_ai_provider(&cloud).unwrap();
    assert_eq!(app.ai_providers().len(), 2);
}

/// Every file under `folder` that holds `secret`.
fn files_holding(folder: &std::path::Path, secret: &[u8], found: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_holding(&path, secret, found);
        } else if std::fs::read(&path)
            .unwrap_or_default()
            .windows(secret.len())
            .any(|window| window == secret)
        {
            found.push(path);
        }
    }
}

#[test]
fn a_key_reaches_only_the_credential_store_and_the_provider() {
    // Every thread logs here, the app's own included.
    let log = support::logs::Capture::all();

    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let provider = FakeProvider::new();
    let Some(_plugin) = ai::with_ai(&mut app, &provider) else {
        return;
    };
    let id = app
        .save_ai_provider(ai::provider("Cloud", "openai", "gpt-test", Some(KEY)))
        .unwrap();
    support::settle(&mut app);
    assert!(app.ai_providers()[0].has_key);
    assert_eq!(system.credentials.secrets(), [KEY.to_owned()]);

    ai::send(&mut app, "Hello", &id);
    ai::wait_until(&mut app, "the request leaves", |_| {
        provider.requests().len() == 1
    });
    assert_eq!(provider.requests()[0].key.as_deref(), Some(KEY));
    provider.say(0, "Hi.");
    provider.fail(0);
    ai::wait_until(&mut app, "the answer ends", |app| {
        ai::buttons(app).is_empty()
    });

    // No plugin view shows it.
    let mut shown = ai::lines(&app);
    shown.extend(ai::chat_titles(&app));
    assert!(shown.iter().all(|line| !line.contains(KEY)), "{shown:?}");
    drop(app);

    let mut found = Vec::new();
    for folder in [fixture.app_data.path(), fixture.root.as_path()] {
        files_holding(folder, KEY.as_bytes(), &mut found);
    }
    assert_eq!(found, Vec::<std::path::PathBuf>::new());
    let log = String::from_utf8(log.bytes()).unwrap();
    assert!(!log.contains(KEY), "{log}");
    // Nor does what the provider answered when it failed.
    assert!(!log.contains("sk-raw-key"), "{log}");
    let mut found = Vec::new();
    for folder in [fixture.app_data.path(), fixture.root.as_path()] {
        files_holding(folder, b"sk-raw-key", &mut found);
    }
    assert_eq!(found, Vec::<std::path::PathBuf>::new());

    // Removing the key takes it out of the store.
    let mut app = fixture.app_with(system.clone());
    app.remove_ai_provider_key(&id).unwrap();
    assert!(system.credentials.secrets().is_empty());
    assert!(!app.ai_providers()[0].has_key);
}

#[test]
fn a_listed_provider_connects_with_only_its_key_and_gets_its_default_model() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());

    let choices = app.ai_provider_choices();
    let kinds: Vec<&str> = choices.iter().map(|choice| choice.kind).collect();
    assert_eq!(kinds, app_core::AI_KEY_PROVIDERS.map(|(kind, _)| kind));
    assert!(choices.iter().all(|choice| !choice.connected));
    assert_eq!(choices[0].name, "OpenAI");

    assert!(app.connect_ai_provider("anthropic", "  ").is_err());
    let id = app.connect_ai_provider("anthropic", KEY).unwrap();

    let row = app
        .ai_providers()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(
        (row.kind.as_str(), row.model.as_str(), row.name.as_str()),
        ("anthropic", "claude-sonnet-5-5", "Anthropic")
    );
    assert!(row.enabled && row.has_key && row.endpoint.is_none());
    assert_eq!(system.credentials.secrets(), [KEY.to_owned()]);
    let anthropic = |app: &app_core::App| {
        app.ai_provider_choices()
            .into_iter()
            .find(|choice| choice.kind == "anthropic")
            .unwrap()
    };
    assert!(anthropic(&app).connected);
    assert_eq!(anthropic(&app).provider.as_deref(), Some(id.as_str()));

    // A new key replaces the old one and keeps the provider's settings.
    app.save_ai_provider(AiProviderInput {
        id: Some(id.clone()),
        ..ai::provider("Mine", "anthropic", "claude-other", None)
    })
    .unwrap();
    app.connect_ai_provider("anthropic", "sk-second").unwrap();
    let row = app
        .ai_providers()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(
        (row.name.as_str(), row.model.as_str()),
        ("Mine", "claude-other")
    );
    assert_eq!(system.credentials.secrets(), ["sk-second".to_owned()]);

    app.remove_ai_provider_key(&id).unwrap();
    assert!(!anthropic(&app).connected);
    assert!(system.credentials.secrets().is_empty());
    assert!(app.connect_ai_provider("ollama", KEY).is_err());
}
