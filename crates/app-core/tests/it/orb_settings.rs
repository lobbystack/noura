//! An Orb's model and effort: the model and effort an Orb's file names
//! reach its provider's request, each provider takes the effort in its
//! own terms or not at all, the default Orb keeps its choice on this
//! device, and the models list each provider's own model first.

use crate::support;

use app_core::{AiEffort, AiOrbChanges, App};
use local_core::AiReasoning;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    orbs::{self, MEETINGS, MEETINGS_FILE, Reply, Scripted},
};
use workspace_format::orb::DEFAULT_ORB_ID;

fn fixture() -> Fixture {
    let files = orbs::meetings(&[], &[], "2026-11-01T10:00:00Z");
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    Fixture::new(&files)
}

fn launch(fixture: &Fixture, provider: std::sync::Arc<dyn local_core::AiStreamer>) -> App {
    let system = FakeSystem::new();
    system.set_clock("2026-11-16T14:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    app.set_ai_client(provider);
    app
}

fn changes() -> AiOrbChanges {
    AiOrbChanges {
        title: None,
        instructions: None,
        avatar: None,
        color: None,
        provider: None,
        model: None,
        effort: None,
        tools: None,
        triggers: None,
    }
}

/// Point the Meetings Orb at `model` of `provider`, reasoning with
/// `effort`.
fn choose(app: &mut App, provider: &str, model: &str, effort: AiEffort) {
    let revision = orbs::orb(app, MEETINGS).revision;
    app.update_orb(
        MEETINGS,
        AiOrbChanges {
            provider: Some(provider.to_owned()),
            model: Some(model.to_owned()),
            effort: Some(Some(effort)),
            ..changes()
        },
        &revision,
    )
    .unwrap();
}

#[test]
fn an_orbs_model_and_effort_reach_its_providers_request() {
    let fixture = fixture();
    let provider = Scripted::new(|_| Reply::Text("Done.".to_owned()));
    let mut app = launch(&fixture, provider.clone());
    let claude = app
        .save_ai_provider(ai::provider(
            "Claude",
            "anthropic",
            "claude-sonnet-4-5",
            Some("sk-ant-test"),
        ))
        .unwrap();

    choose(&mut app, &claude, "claude-opus-5-5", AiEffort::High);
    app.send_to_orb(MEETINGS, "What's next?").unwrap();
    orbs::wait_for_runs(&mut app, MEETINGS, 1);

    let asked = provider.orb_requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].provider, claude);
    assert_eq!(asked[0].model, "claude-opus-5-5");
    assert_eq!(asked[0].reasoning, Some(AiReasoning::High));
    let file = String::from_utf8(fixture.read(MEETINGS_FILE)).unwrap();
    assert_eq!(
        ai::frontmatter(&file, "model").as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(ai::frontmatter(&file, "effort").as_deref(), Some("high"));
    let listed = orbs::orb(&app, MEETINGS);
    assert_eq!(listed.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(listed.effort, Some(AiEffort::High));

    // Clearing the effort leaves the model's own default.
    let revision = listed.revision;
    app.update_orb(
        MEETINGS,
        AiOrbChanges {
            effort: Some(None),
            ..changes()
        },
        &revision,
    )
    .unwrap();
    app.send_to_orb(MEETINGS, "And after that?").unwrap();
    orbs::wait_for_runs(&mut app, MEETINGS, 2);
    assert_eq!(provider.orb_requests()[1].reasoning, None);
    let file = String::from_utf8(fixture.read(MEETINGS_FILE)).unwrap();
    assert_eq!(ai::frontmatter(&file, "effort").as_deref(), Some("null"));
}

#[test]
fn each_provider_takes_the_effort_in_its_own_terms() {
    let fixture = fixture();
    let provider = Scripted::new(|_| Reply::Text("Done.".to_owned()));
    let mut app = launch(&fixture, provider.clone());
    let mut save = |name: &str, kind: &str, model: &str| {
        app.save_ai_provider(ai::provider(name, kind, model, Some("test-key")))
            .unwrap()
    };
    let claude = save("Claude", "anthropic", "claude-sonnet-4-5");
    let openai = save("OpenAI", "openai", "gpt-4.1-mini");
    let gemini = save("Gemini", "gemini", "gemini-2.5-flash");
    let local = save("Local", "ollama", "llama3");
    let cases = [
        (
            &claude,
            "claude-opus-5-5",
            AiEffort::Medium,
            Some(AiReasoning::Medium),
        ),
        (
            &claude,
            "claude-haiku-5-5",
            AiEffort::Off,
            Some(AiReasoning::Zero),
        ),
        // Opus 5.5 can't turn its reasoning off, so Noura sends nothing.
        (&claude, "claude-opus-5-5", AiEffort::Off, None),
        (&openai, "gpt-5.1", AiEffort::Off, Some(AiReasoning::Zero)),
        (
            &openai,
            "gpt-5-mini",
            AiEffort::Off,
            Some(AiReasoning::Minimal),
        ),
        (&openai, "o4-mini", AiEffort::High, Some(AiReasoning::High)),
        (&openai, "gpt-4.1-mini", AiEffort::High, None),
        (
            &gemini,
            "gemini-3-pro",
            AiEffort::Low,
            Some(AiReasoning::Low),
        ),
        (&local, "llama3", AiEffort::High, None),
    ];

    for (index, (id, model, effort, expected)) in cases.into_iter().enumerate() {
        choose(&mut app, id, model, effort);
        app.send_to_orb(MEETINGS, &format!("Message {index}"))
            .unwrap();
        orbs::wait_for_runs(&mut app, MEETINGS, index + 1);
        let asked = provider.orb_requests();
        let last = asked.last().unwrap();
        assert_eq!(
            (last.provider.as_str(), last.model.as_str(), last.reasoning),
            (id.as_str(), model, expected),
            "{model} at {effort:?}"
        );
    }
}

#[test]
fn the_default_orb_keeps_its_model_and_effort_on_this_device() {
    let fixture = fixture();
    let mut app = launch(&fixture, FakeProvider::new());
    let claude = app
        .save_ai_provider(ai::provider(
            "Claude",
            "anthropic",
            "claude-sonnet-4-5",
            Some("sk-ant-test"),
        ))
        .unwrap();

    let changed = app
        .update_orb(
            DEFAULT_ORB_ID,
            AiOrbChanges {
                provider: Some(claude.clone()),
                model: Some("claude-sonnet-5-5".to_owned()),
                effort: Some(Some(AiEffort::Low)),
                ..changes()
            },
            "",
        )
        .unwrap();
    assert_eq!(changed.id, DEFAULT_ORB_ID);
    assert_eq!(changed.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(changed.effort, Some(AiEffort::Low));
    // It has no file to rename.
    let refused = app.update_orb(
        DEFAULT_ORB_ID,
        AiOrbChanges {
            title: Some("Helper".to_owned()),
            ..changes()
        },
        "",
    );
    assert!(refused.is_err());
    drop(app);

    // The workspace holds none of it, and the next launch keeps it.
    assert!(!fixture.root.join("Orbs/Noura").exists());
    let app = launch(&fixture, FakeProvider::new());
    let default = app.orbs().unwrap().remove(0);
    assert_eq!(default.title, "Noura");
    assert_eq!(default.provider.as_deref(), Some(claude.as_str()));
    assert_eq!(default.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(default.effort, Some(AiEffort::Low));
}

#[test]
fn the_models_list_each_providers_own_model_first() {
    let fixture = fixture();
    let mut app = launch(&fixture, FakeProvider::new());
    let claude = app
        .save_ai_provider(ai::provider(
            "Claude",
            "anthropic",
            "claude-opus-5-5",
            Some("sk-ant-test"),
        ))
        .unwrap();
    let local = app
        .save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
        .unwrap();

    let models = app.ai_models().unwrap();

    let listed: Vec<(&str, &str, &str)> = models
        .iter()
        .map(|model| {
            (
                model.provider.as_str(),
                model.id.as_str(),
                model.name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (claude.as_str(), "claude-opus-5-5", "Claude Opus 5.5"),
            (claude.as_str(), "claude-fable-5-2", "Claude Fable 5.2"),
            (claude.as_str(), "claude-sonnet-5-5", "Claude Sonnet 5.5"),
            (claude.as_str(), "claude-haiku-5-5", "Claude Haiku 5.5"),
            (local.as_str(), "llama3", "llama3"),
        ]
    );
    assert_eq!(
        models[0].efforts,
        [AiEffort::Low, AiEffort::Medium, AiEffort::High]
    );
    assert_eq!(models[0].default_effort, Some(AiEffort::Medium));
    assert!(models[4].efforts.is_empty());
}
