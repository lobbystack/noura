//! The AI page, Noura's consent prompt and the AI providers section of
//! Settings through AccessKit: an answer streaming in shows as Markdown,
//! Stop ends it, the prompt names the provider and starts on Deny, and
//! the provider form labels each field and hides the key.

mod support;

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use app_core::{
    App, Platform,
    ai_client::{
        AiProviderEventStream, AiProviderStreamEvent, AiStreamer, ResolvedAiStreamRequest, Result,
    },
};
use egui::{Key, accesskit::Role};
use egui_kittest::kittest::{By, NodeT, Queryable};
use futures::StreamExt;
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, focused, open_page, press, run},
    plugins, settle,
};

const MANIFEST: &str = include_str!("../../plugins/ai/plugin.toml");

/// A provider that streams one piece of text, then waits until Noura
/// cancels the request.
#[derive(Default)]
struct Waiting {
    requests: AtomicUsize,
    dropped: Arc<AtomicBool>,
}

struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl AiStreamer for Waiting {
    fn stream(
        &self,
        _request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let dropped = Dropped(self.dropped.clone());
        Box::pin(async move {
            let text = futures::stream::iter([Ok(AiProviderStreamEvent::TextDelta(
                "Seeds **sprout** in spring".to_owned(),
            ))]);
            let waiting = futures::stream::pending().map(move |event| {
                let _ = &dropped;
                event
            });
            let stream: AiProviderEventStream = Box::pin(text.chain(waiting));
            Ok(stream)
        })
    }
}

struct Opened {
    harness: Ui,
    provider: Arc<Waiting>,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with the AI plugin on and the provider Local set up, with
/// your consent when `consent` holds, opened at launch on the AI page.
fn opened(consent: bool) -> Option<Opened> {
    let component = plugins::built("ai_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let plugin = plugins::folder(parent.path(), MANIFEST, "ai_plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "ai");
    set_up_provider(app_data.path(), consent);
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    let provider = Arc::new(Waiting::default());
    harness
        .state_mut()
        .app_mut()
        .set_ai_client(provider.clone());
    settle(&mut harness);
    open_page(&mut harness, "AI");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Form, "Message")
            .is_some()
    });
    Some(Opened {
        harness,
        provider,
        _folders: (app_data, parent),
    })
}

/// Set up the provider Local in the app data, before the harness starts.
fn set_up_provider(app_data: &std::path::Path, consent: bool) {
    let mut app = App::launch(app_data);
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.is_busy() || app.workspace().is_none() {
        app.poll();
        assert!(Instant::now() < deadline, "the workspace didn't open");
        std::thread::sleep(Duration::from_millis(5));
    }
    let id = app
        .save_ai_provider(app_core::AiProviderInput {
            id: None,
            name: "Local".into(),
            kind: "ollama".into(),
            model: "llama3".into(),
            endpoint: None,
            enabled: true,
            key: None,
        })
        .unwrap();
    if consent {
        app.grant_ai_consent(&id).unwrap();
    }
}

/// Run frames, waiting in real time for answers, until `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(
            Instant::now() < deadline,
            "timed out; the window shows {:?}",
            all_labels(harness)
        );
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    run(harness);
}

/// Write `text` in the message box and press Send.
fn send(harness: &mut Ui, text: &str) {
    harness
        .get_by_role_and_label(Role::Form, "Message")
        .get_by_role_and_label(Role::MultilineTextInput, "Message")
        .focus();
    run(harness);
    harness
        .get_by_role_and_label(Role::Form, "Message")
        .get_by_role_and_label(Role::MultilineTextInput, "Message")
        .type_text(text);
    run(harness);
    harness
        .get_by_role_and_label(Role::Form, "Message")
        .get_by_role_and_label(Role::Button, "Send")
        .click();
    run(harness);
}

/// The text of every label in the window.
fn all_labels(harness: &Ui) -> Vec<String> {
    texts(harness.query_all(By::new().role(Role::Label)))
}

/// The text the labels among `nodes` show.
fn texts<'a>(nodes: impl Iterator<Item = egui_kittest::Node<'a>>) -> Vec<String> {
    nodes
        .filter_map(|node| node.accesskit_node().value())
        .collect()
}

#[test]
fn the_answer_streams_in_as_markdown_and_stop_ends_it() {
    let Some(mut opened) = opened(true) else {
        return;
    };
    let harness = &mut opened.harness;

    send(harness, "When do seeds sprout?");
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"sprout".to_owned())
    });
    let shown = all_labels(harness);
    assert!(shown.contains(&"Seeds ".to_owned()), "{shown:?}");
    assert!(shown.contains(&" in spring".to_owned()), "{shown:?}");
    assert!(
        !shown.iter().any(|label| label.contains("**")),
        "the Markdown draws: {shown:?}"
    );
    assert!(shown.contains(&"You".to_owned()));
    assert!(shown.contains(&"Local, llama3".to_owned()));
    assert!(
        harness
            .query_by_role_and_label(Role::Form, "Message")
            .is_none()
    );

    harness.get_by_role_and_label(Role::Button, "Stop").click();
    run(harness);
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"Stopped.".to_owned())
    });
    assert!(opened.provider.dropped.load(Ordering::SeqCst));
    let harness = &mut opened.harness;
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Stop")
            .is_none()
    );
    assert!(
        harness
            .query_by_role_and_label(Role::Form, "Message")
            .is_some()
    );
    // The partial answer stays.
    assert!(all_labels(harness).contains(&"sprout".to_owned()));
}

#[test]
fn noura_s_prompt_names_the_provider_starts_on_deny_and_escape_denies() {
    let Some(mut opened) = opened(false) else {
        return;
    };
    let harness = &mut opened.harness;
    let title = "Let Local receive this workspace's content?";

    send(harness, "When do seeds sprout?");
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::AlertDialog, title)
            .is_some()
    });
    let dialog = harness.get_by_role_and_label(Role::AlertDialog, title);
    dialog.get_by_role_and_label(Role::Button, "Deny");
    dialog.get_by_role_and_label(Role::Button, "Allow");
    assert!(
        texts(dialog.query_all(By::new().role(Role::Label))).contains(
            &"Your messages and the chat so far go from Garden to Local, which answers with \
              llama3 (Ollama)."
                .to_owned()
        )
    );
    assert_eq!(focused(harness).as_deref(), Some("Deny"));

    press(harness, Key::Escape);
    wait_until(harness, |harness| {
        all_labels(harness).contains(
            &"You didn't let Local receive this workspace's content, so Noura sent nothing."
                .to_owned(),
        )
    });
    assert!(
        harness
            .query_by_role_and_label(Role::AlertDialog, title)
            .is_none()
    );
    assert_eq!(opened.provider.requests.load(Ordering::SeqCst), 0);

    let harness = &mut opened.harness;
    harness
        .get_by_role_and_label(Role::Form, "Message")
        .get_by_role_and_label(Role::Button, "Send")
        .click();
    run(harness);
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::AlertDialog, title)
            .is_some()
    });
    harness
        .get_by_role_and_label(Role::AlertDialog, title)
        .get_by_role_and_label(Role::Button, "Allow")
        .click();
    run(harness);
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"sprout".to_owned())
    });
    assert_eq!(opened.provider.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn the_provider_form_labels_each_field_and_hides_the_key() {
    let Some(mut opened) = opened(false) else {
        return;
    };
    let harness = &mut opened.harness;
    harness
        .state_mut()
        .app_mut()
        .open_settings_at("ai-providers");
    run(harness);

    let local = harness.get_by_role_and_label(Role::Group, "Local");
    assert!(
        texts(local.query_all(By::new().role(Role::Label)))
            .contains(&"It can't receive Garden's content yet.".to_owned())
    );
    local
        .get_by_role_and_label(Role::Button, "Allow for this workspace")
        .click();
    run(harness);
    let local = harness.get_by_role_and_label(Role::Group, "Local");
    assert!(
        texts(local.query_all(By::new().role(Role::Label)))
            .contains(&"You allowed it to receive Garden's content.".to_owned())
    );
    local.get_by_role_and_label(Role::Button, "Revoke for this workspace");

    harness
        .get_by_role_and_label(Role::Button, "Add provider")
        .click();
    run(harness);
    harness
        .get_by_role_and_label(Role::Form, "Add provider")
        .get_by_role_and_label(Role::TextInput, "Endpoint (optional)");
    for (role, field, text) in [
        (Role::TextInput, "Name", "Cloud"),
        (Role::TextInput, "Model", "gpt-test"),
        (Role::PasswordInput, "Key", "sk-ui-test-key"),
    ] {
        harness
            .get_by_role_and_label(Role::Form, "Add provider")
            .get_by_role_and_label(role, field)
            .focus();
        run(harness);
        harness
            .get_by_role_and_label(Role::Form, "Add provider")
            .get_by_role_and_label(role, field)
            .type_text(text);
        run(harness);
    }
    harness
        .get_by_role_and_label(Role::Form, "Add provider")
        .get_by_role_and_label(Role::Button, "Save provider")
        .click_accesskit();
    run(harness);

    let cloud = harness.get_by_role_and_label(Role::Group, "Cloud");
    assert!(
        texts(cloud.query_all(By::new().role(Role::Label)))
            .contains(&"On, with its key saved".to_owned())
    );
    assert!(
        !all_labels(harness)
            .iter()
            .any(|label| label.contains("sk-ui-test-key"))
    );
}
