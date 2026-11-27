//! One short message to each real provider whose key you give in the
//! environment, to check that requests still work against the live APIs.
//! Ignored by default; run with
//! `NOURA_LIVE_ANTHROPIC_KEY=… cargo test -p local-core --test it ai_live -- --ignored`.
//! A provider without its variable is skipped. The keys stay in memory:
//! nothing here writes or prints them.

use std::sync::Arc;

use local_core::{
    AiFoundation, AiModelRef, AiProviderConfig, AiStreamEvent, AiStreamInput, MemoryCredentials,
};

use super::ai_provider::{OPERATION, answer_text, events, text};

/// Each provider kind, the variable that holds its key and a small model.
const LIVE: [(&str, &str, &str); 3] = [
    ("anthropic", "NOURA_LIVE_ANTHROPIC_KEY", "claude-haiku-5-5"),
    ("openai", "NOURA_LIVE_OPENAI_KEY", "gpt-6-luna"),
    ("gemini", "NOURA_LIVE_GEMINI_KEY", "gemini-4-hydrogen"),
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real providers; needs NOURA_LIVE_*_KEY"]
async fn each_live_provider_answers_a_short_message() {
    let mut asked = 0;
    for (kind, variable, model) in LIVE {
        let Ok(key) = std::env::var(variable) else {
            eprintln!("skipping {kind}: {variable} isn't set");
            continue;
        };
        let model = std::env::var(format!("{variable}_MODEL")).unwrap_or_else(|_| model.into());
        let folder = tempfile::tempdir().unwrap();
        let ai = AiFoundation::open_with(
            folder.path().join("providers.json"),
            Arc::new(MemoryCredentials::new()),
        )
        .unwrap();
        ai.save_provider(AiProviderConfig {
            id: "local".into(),
            kind: kind.into(),
            display_name: kind.into(),
            model: model.clone(),
            endpoint: None,
            credential_ref: None,
            enabled: true,
            context_window: None,
        })
        .unwrap();
        ai.set_credential("local", &key).unwrap();
        drop(key);
        let operation = ai
            .start_stream(AiStreamInput {
                operation_id: OPERATION.into(),
                model: AiModelRef {
                    provider_id: "local".into(),
                    model,
                },
                messages: vec![text("user", "Reply with the single word: ready")],
                tools: None,
                reasoning: None,
            })
            .unwrap();
        let events = events(operation).await;
        match events.last() {
            Some(AiStreamEvent::Completed { .. }) => {
                assert!(
                    !answer_text(&events).trim().is_empty(),
                    "{kind} said nothing"
                );
            }
            Some(AiStreamEvent::Error { error }) => {
                panic!("{kind} failed with {}", error.code)
            }
            other => panic!("{kind} ended with {other:?}"),
        }
        asked += 1;
    }
    eprintln!("asked {asked} live providers");
}
