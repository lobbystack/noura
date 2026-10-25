//! Seam 1: what each request carries besides the chat. Noura's
//! instructions come first, followed by the body of the workspace's root
//! AGENTS.md when it has one, and a chat that no longer fits the
//! provider's context window gets a context summary that later requests
//! send in place of the messages it covers.

mod support;

use std::sync::Arc;

use app_core::App;
use support::{
    FakeSystem, Fixture,
    ai::{self, FakeProvider},
    plugins,
};

/// The first words of Noura's instructions.
const INSTRUCTIONS: &str = "You are Noura, the local-first workspace assistant.";

/// A workspace holding `files` with the AI plugin on and the provider
/// Local allowed, which reads `window` tokens at most. The app comes
/// first, so it closes before its folders go.
struct Chatting {
    app: App,
    provider: Arc<FakeProvider>,
    id: String,
    _plugin: plugins::PluginFolder,
    fixture: Fixture,
}

impl Chatting {
    fn new(files: &[(&str, &[u8])], window: Option<u32>) -> Option<Self> {
        let fixture = Fixture::new(files);
        let mut app = fixture.app_with(FakeSystem::new());
        let provider = FakeProvider::new();
        let plugin = ai::with_ai(&mut app, &provider)?;
        let mut input = ai::provider("Local", "ollama", "llama3", None);
        input.context_window = window;
        let id = app.save_ai_provider(input).unwrap();
        app.grant_ai_consent(&id).unwrap();
        support::settle(&mut app);
        Some(Self {
            app,
            provider,
            id,
            _plugin: plugin,
            fixture,
        })
    }

    fn chat_once(&mut self, text: &str, answer: &str) {
        let provider = self.provider.clone();
        ai::chat_once(&mut self.app, &provider, &self.id, text, answer);
    }
}

#[test]
fn each_request_sends_noura_s_instructions_then_the_workspace_s_agents_md() {
    let Some(mut chatting) = Chatting::new(
        &[("AGENTS.md", b"Keep tasks short.\nWrite in French.\n")],
        None,
    ) else {
        return;
    };

    chatting.chat_once("Plan the garden", "Plant garlic.");

    let system = chatting.provider.requests()[0].system.clone();
    assert_eq!(system.len(), 1, "{system:?}");
    assert!(system[0].starts_with(INSTRUCTIONS), "{system:?}");
    assert!(
        system[0].ends_with(
            "\n\n## Workspace instructions: AGENTS.md\nKeep tasks short.\nWrite in French."
        ),
        "{system:?}"
    );

    // Each request reads the file again.
    std::fs::write(chatting.fixture.root.join("AGENTS.md"), "Be brief.\n").unwrap();
    chatting.chat_once("And then?", "Water it.");
    let system = chatting.provider.requests()[1].system.clone();
    assert!(
        system[0].ends_with("\n\n## Workspace instructions: AGENTS.md\nBe brief."),
        "{system:?}"
    );
}

#[test]
fn a_missing_or_unreadable_agents_md_leaves_noura_s_instructions_alone() {
    let Some(mut chatting) = Chatting::new(&[], None) else {
        return;
    };
    chatting.chat_once("Plan the garden", "Plant garlic.");
    let alone = chatting.provider.requests()[0].system.clone();
    assert_eq!(alone.len(), 1);
    assert!(alone[0].starts_with(INSTRUCTIONS));
    assert!(!alone[0].contains("Workspace instructions"), "{alone:?}");

    // Bytes that aren't UTF-8 can't be read as Markdown.
    std::fs::write(chatting.fixture.root.join("AGENTS.md"), [0xff, 0xfe, 0x00]).unwrap();
    chatting.chat_once("And then?", "Water it.");
    assert_eq!(chatting.provider.requests()[1].system, alone);
}

#[test]
fn a_chat_that_outgrows_the_context_window_sends_a_summary_in_place_of_its_earlier_messages() {
    let Some(mut chatting) = Chatting::new(&[], Some(300)) else {
        return;
    };
    let first = "a".repeat(100);
    let answer = "b".repeat(200);
    chatting.chat_once(&first, &answer);
    assert_eq!(chatting.provider.requests().len(), 1, "the chat still fits");

    let long = "c".repeat(400);
    ai::send(&mut chatting.app, &long, &chatting.id);
    let provider = chatting.provider.clone();
    ai::wait_until(&mut chatting.app, "the summary is asked for", |_| {
        provider.requests().len() == 2
    });
    let asked = provider.requests()[1].clone();
    assert_eq!(asked.messages.len(), 3, "{:?}", asked.messages);
    assert_eq!(asked.messages[0], format!("user: {first}"));
    assert_eq!(asked.messages[1], format!("assistant: {answer}"));
    assert!(asked.messages[2].starts_with("user: Summarize our conversation so far"));
    assert!(asked.tools.is_empty());
    provider.say(1, "You asked about a, and I answered b.");
    provider.finish(1);

    ai::wait_until(&mut chatting.app, "the answer is asked for", |_| {
        provider.requests().len() == 3
    });
    let answering = provider.requests()[2].clone();
    assert!(answering.system[0].starts_with(INSTRUCTIONS));
    assert_eq!(
        answering.system[1],
        "## Summary of the earlier conversation\nYou asked about a, and I answered b."
    );
    assert_eq!(answering.messages, [format!("user: {long}")]);
    let files = ai::message_files(&chatting.fixture.root);
    let summary = files
        .iter()
        .find(|text| ai::frontmatter(text, "kind").as_deref() == Some("context-summary"))
        .expect("the summary is in the chat");
    let covered = files
        .iter()
        .find(|text| text.contains(&answer))
        .expect("the covered answer stays on disk");
    assert_eq!(
        ai::frontmatter(summary, "summarizes_through_message_id"),
        ai::frontmatter(covered, "id")
    );
    assert!(files.iter().any(|text| text.contains(&first)));
    provider.say(2, "Noted.");
    provider.finish(2);
    ai::wait_until(&mut chatting.app, "the answer shows", |app| {
        ai::lines(app).contains(&"md: Noted.".to_owned()) && ai::buttons(app).is_empty()
    });
    assert!(
        ai::lines(&chatting.app).contains(&"md: You asked about a, and I answered b.".to_owned())
    );

    // Later requests keep the summary in place of what it covers.
    chatting.chat_once("d", "e");
    let later = provider.requests()[3].clone();
    assert_eq!(later.system.len(), 2);
    assert_eq!(
        later.messages,
        [
            format!("user: {long}"),
            "assistant: Noted.".to_owned(),
            "user: d".to_owned()
        ]
    );
}
