//! Seam 1: AI providers in Settings. You add several, each with a kind
//! Noura's provider client supports, a model and an optional endpoint
//! that must use HTTPS unless it points at this computer. Their settings
//! stay in this device's app data, and their keys only in the credential
//! store: after a chat, no key is in the workspace, the index, the app
//! data or the logs, and the AI page never shows one.

mod support;

use std::sync::{Arc, Mutex};

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

/// A log that keeps every line the app writes while a test runs.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_key_reaches_only_the_credential_store_and_the_provider() {
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    // Every thread logs here, the AI run's own included. No other test in
    // this file sets a subscriber.
    tracing::subscriber::set_global_default(subscriber).unwrap();

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
    app.grant_ai_consent(&id).unwrap();
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
    let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
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
