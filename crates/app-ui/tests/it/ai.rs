//! The AI page, Noura's tool prompt and the AI providers section of
//! Settings through AccessKit: an answer streaming in shows as Markdown,
//! Stop ends it, a message goes out without a prompt, the tool prompt
//! names the tool, its plugin and its input and starts on Deny, and the
//! provider form labels each field and hides the key.

use crate::support;
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
    App, Platform, ViewKind, ViewNode,
    ai_client::{
        AiProviderEventStream, AiProviderStreamEvent, AiStreamer, AiToolCall,
        ResolvedAiStreamRequest, Result,
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

const MANIFEST: &str = include_str!("../../../plugins/ai/plugin.toml");

/// The default Orb's message box.
const MESSAGE: &str = "Message Noura";

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
            // Holds the guard until Noura drops the stream.
            let waiting = futures::stream::unfold(dropped, |dropped| async move {
                futures::future::pending::<()>().await;
                drop(dropped);
                None
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

/// A workspace with the AI plugin on and the provider Local set up,
/// opened at launch on the AI page.
fn opened() -> Option<Opened> {
    let component = plugins::built("ai_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
    let plugin = plugins::folder(parent.path(), MANIFEST, "ai_plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "ai");
    set_up_provider(app_data.path());
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
            .query_by_role_and_label(Role::Form, MESSAGE)
            .is_some()
    });
    Some(Opened {
        harness,
        provider,
        _folders: (app_data, parent),
    })
}

/// Set up the provider Local in the app data, before the harness starts.
pub(crate) fn set_up_provider(app_data: &std::path::Path) {
    let mut app = App::launch(app_data);
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.is_busy() || app.workspace().is_none() {
        app.poll();
        assert!(Instant::now() < deadline, "the workspace didn't open");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.save_ai_provider(app_core::AiProviderInput {
        id: None,
        name: "Local".into(),
        kind: "ollama".into(),
        model: "llama3".into(),
        endpoint: None,
        enabled: true,
        key: None,
        context_window: None,
    })
    .unwrap();
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
        .get_by_role_and_label(Role::Form, MESSAGE)
        .get_by_role_and_label(Role::MultilineTextInput, MESSAGE)
        .focus();
    run(harness);
    harness
        .get_by_role_and_label(Role::Form, MESSAGE)
        .get_by_role_and_label(Role::MultilineTextInput, MESSAGE)
        .type_text(text);
    run(harness);
    harness
        .get_by_role_and_label(Role::Form, MESSAGE)
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
    let Some(mut opened) = opened() else {
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
    // Your message and the answer, each named for screen readers by who
    // wrote it.
    let yours = harness.get_by_role_and_label(Role::Article, "You");
    assert_eq!(
        texts(yours.query_all(By::new().role(Role::Label))),
        ["When do seeds sprout?"]
    );
    harness.get_by_role_and_label(Role::Article, "Noura");
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    // Stop takes Send's place in the message box while the answer streams.
    let form = harness.get_by_role_and_label(Role::Form, MESSAGE);
    assert!(form.query_by_role_and_label(Role::Button, "Send").is_none());

    form.get_by_role_and_label(Role::Button, "Stop").click();
    run(harness);
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"Stopped.".to_owned())
    });
    assert!(opened.provider.dropped.load(Ordering::SeqCst));
    let harness = &mut opened.harness;
    let form = harness.get_by_role_and_label(Role::Form, MESSAGE);
    assert!(form.query_by_role_and_label(Role::Button, "Stop").is_none());
    form.get_by_role_and_label(Role::Button, "Send");
    // The partial answer stays.
    assert!(all_labels(harness).contains(&"sprout".to_owned()));
}

#[test]
fn an_empty_thread_shows_one_line_and_the_message_box_with_no_icon_or_provider() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    assert!(harness.query_by_label("How can I help?").is_some());
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    // The empty state is its line and nothing else: no icon.
    let view = harness
        .state()
        .app()
        .plugin_view("plugin:ai:ai")
        .expect("the page shows")
        .expect("the page draws");
    let empty = find_kind(view.root(), &|kind| matches!(kind, ViewKind::Empty { .. }))
        .expect("the empty thread shows its line");
    assert!(
        matches!(
            empty,
            ViewKind::Empty {
                icon: None,
                detail: None,
                ..
            }
        ),
        "{empty:?}"
    );
    // The model lives in the Orb's panel, not in the message box.
    let form = harness.get_by_role_and_label(Role::Form, MESSAGE);
    assert!(form.query_by_role(Role::ComboBox).is_none());
    // Send waits until the box holds more than spaces.
    assert!(
        form.get_by_role_and_label(Role::Button, "Send")
            .accesskit_node()
            .is_disabled()
    );
    // The box has the keyboard once you click it, and Enter sends.
    harness
        .get_by_role_and_label(Role::Form, MESSAGE)
        .get_by_role_and_label(Role::MultilineTextInput, MESSAGE)
        .focus();
    run(harness);
    harness
        .get_by_role_and_label(Role::Form, MESSAGE)
        .get_by_role_and_label(Role::MultilineTextInput, MESSAGE)
        .type_text("Hello");
    run(harness);
    press(harness, Key::Enter);
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"sprout".to_owned())
    });
    assert_eq!(opened.provider.requests.load(Ordering::SeqCst), 1);
    let harness = &mut opened.harness;
    assert!(harness.query_by_label("How can I help?").is_none());
    // The box empties once it sends.
    assert_eq!(
        harness
            .get_by_role_and_label(Role::Form, MESSAGE)
            .get_by_role_and_label(Role::MultilineTextInput, MESSAGE)
            .value()
            .as_deref(),
        Some("")
    );
}

/// The first view node under `node` whose kind `wanted` matches.
fn find_kind(node: &ViewNode, wanted: &dyn Fn(&ViewKind) -> bool) -> Option<ViewKind> {
    if wanted(&node.kind) {
        return Some(node.kind.clone());
    }
    node.children
        .iter()
        .find_map(|child| find_kind(child, wanted))
}

/// Open the open Orb's side panel from the pill with its name.
fn open_panel(harness: &mut Ui, name: &str) {
    let pill = harness.get_by_role_and_label(Role::Button, name);
    if pill.accesskit_node().data().is_expanded() != Some(true) {
        pill.click();
        run(harness);
    }
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::Complementary, name)
            .is_some()
    });
}

#[test]
fn a_chat_renames_and_expires_from_its_menu_in_the_default_orbs_panel() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    send(harness, "When do seeds sprout?");
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"sprout".to_owned())
    });
    harness
        .get_by_role_and_label(Role::Form, MESSAGE)
        .get_by_role_and_label(Role::Button, "Stop")
        .click();
    run(harness);
    open_panel(harness, "Noura");
    let chats = || (Role::ListBox, "Chats");
    // Once the stopped answer is written, the chat stays as the menu saw it.
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"Stopped.".to_owned())
            && harness
                .query_by_role_and_label(chats().0, chats().1)
                .is_some()
    });
    let title = labels_of(harness, chats());
    assert_eq!(title.len(), 1, "{title:?}");

    // Pointing at the chat shows the button that opens its menu.
    harness
        .get_by_role_and_label(chats().0, chats().1)
        .get_by_role_and_label(Role::ListBoxOption, &title[0])
        .hover();
    run(harness);
    harness
        .get_by_role_and_label(Role::Button, "More actions")
        .click();
    run(harness);
    harness
        .get_by_label("Chat options")
        .get_by_role_and_label(Role::Button, "Rename")
        .click();
    run(harness);
    let field = harness.get_by_role_and_label(Role::TextInput, "Chat name");
    assert!(field.is_focused());
    harness
        .get_by_role_and_label(Role::TextInput, "Chat name")
        .type_text(" in spring");
    run(harness);
    press(harness, Key::Enter);
    let renamed = format!("{} in spring", title[0]);
    wait_until(harness, |harness| {
        labels_of(harness, chats()) == [renamed.clone()]
    });

    // Remove after 30 days, from a right-click, says so over the thread.
    harness
        .get_by_role_and_label(chats().0, chats().1)
        .get_by_role_and_label(Role::ListBoxOption, &renamed)
        .click_secondary();
    run(harness);
    harness
        .get_by_label("Chat options")
        .get_by_role_and_label(Role::Button, "Remove after 30 days")
        .click();
    run(harness);
    wait_until(harness, |harness| {
        harness
            .query_by_label("This chat moves to the trash 30 days after it started.")
            .is_some()
    });
}

/// The labels of the options of the list `list`.
fn labels_of(harness: &Ui, list: (Role, &str)) -> Vec<String> {
    harness
        .get_by_role_and_label(list.0, list.1)
        .query_all(By::new().role(Role::ListBoxOption))
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

#[test]
fn a_message_goes_to_the_provider_without_a_prompt() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;

    send(harness, "When do seeds sprout?");
    wait_until(harness, |harness| {
        all_labels(harness).contains(&"sprout".to_owned())
    });

    assert!(harness.query_by_role(Role::AlertDialog).is_none());
    assert_eq!(opened.provider.requests.load(Ordering::SeqCst), 1);
}

/// Scroll the node `role` labeled `label` in the group `group` into view,
/// then click it, as a click there does.
fn click_in(harness: &mut Ui, group: (Role, &str), role: Role, label: &str) {
    harness
        .get_by_role_and_label(group.0, group.1)
        .get_by_role_and_label(role, label)
        .scroll_to_me();
    run(harness);
    harness
        .get_by_role_and_label(group.0, group.1)
        .get_by_role_and_label(role, label)
        .click();
    run(harness);
}

/// Scroll the field into view, focus it and type `text`.
fn type_in(harness: &mut Ui, group: (Role, &str), role: Role, label: &str, text: &str) {
    click_in(harness, group, role, label);
    harness
        .get_by_role_and_label(group.0, group.1)
        .get_by_role_and_label(role, label)
        .type_text(text);
    run(harness);
}

#[test]
fn a_listed_provider_connects_with_a_pasted_key() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    harness
        .state_mut()
        .app_mut()
        .open_settings_at("ai-providers");
    run(harness);
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());

    // Each provider Noura connects to shows, with no badge until it's
    // connected.
    let openai = harness.get_by_role_and_label(Role::Group, "OpenAI");
    let shown = texts(openai.query_all(By::new().role(Role::Label)));
    assert!(
        !shown.iter().any(|text| text.contains("onnected")),
        "{shown:?}"
    );
    harness.get_by_role_and_label(Role::Group, "Anthropic");
    harness.get_by_role_and_label(Role::Group, "Local or custom");

    // Opening it shows one key field and Save key.
    let openai = (Role::Group, "OpenAI");
    click_in(harness, openai, Role::Button, "OpenAI");
    harness
        .get_by_role_and_label(openai.0, openai.1)
        .get_by_role_and_label(Role::PasswordInput, "API key");
    type_in(
        harness,
        openai,
        Role::PasswordInput,
        "API key",
        "sk-ui-test-key",
    );
    click_in(harness, openai, Role::Button, "Save key");

    let openai = harness.get_by_role_and_label(Role::Group, "OpenAI");
    assert!(texts(openai.query_all(By::new().role(Role::Label))).contains(&"Connected".to_owned()));
    let row = harness
        .state()
        .app()
        .ai_providers()
        .into_iter()
        .find(|row| row.kind == "openai")
        .expect("OpenAI is set up");
    assert!(row.has_key && row.enabled);
    assert_eq!(row.model, "gpt-4.1-mini");
    assert!(
        !all_labels(harness)
            .iter()
            .any(|label| label.contains("sk-ui-test-key"))
    );

    // A connected provider offers Remove key.
    click_in(harness, (Role::Group, "OpenAI"), Role::Button, "OpenAI");
    click_in(harness, (Role::Group, "OpenAI"), Role::Button, "Remove key");
    let openai = harness.get_by_role_and_label(Role::Group, "OpenAI");
    let shown = texts(openai.query_all(By::new().role(Role::Label)));
    assert!(!shown.contains(&"Connected".to_owned()), "{shown:?}");
}

#[test]
fn local_or_custom_takes_only_an_address_a_model_and_for_a_custom_server_a_key() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    harness
        .state_mut()
        .app_mut()
        .open_settings_at("ai-providers");
    run(harness);
    click_in(
        harness,
        (Role::Group, "Local or custom"),
        Role::Button,
        "Local or custom",
    );
    let form = || "Add a local or custom provider";
    // Ollama needs no key.
    assert!(
        harness
            .get_by_role_and_label(Role::Form, form())
            .query_by_role_and_label(Role::PasswordInput, "API key")
            .is_none()
    );
    click_in(
        harness,
        (Role::Form, form()),
        Role::RadioButton,
        "OpenAI-compatible",
    );
    for (role, field, text) in [
        (Role::TextInput, "Address", "https://models.example.com/v1/"),
        (Role::TextInput, "Model", "house-model"),
        (Role::PasswordInput, "API key", "sk-custom"),
    ] {
        type_in(harness, (Role::Form, form()), role, field, text);
    }
    click_in(harness, (Role::Form, form()), Role::Button, "Save provider");

    let custom = harness
        .state()
        .app()
        .ai_providers()
        .into_iter()
        .find(|row| row.name == "Custom")
        .expect("the custom provider is saved");
    assert_eq!(
        (
            custom.kind.as_str(),
            custom.model.as_str(),
            custom.endpoint.as_deref()
        ),
        (
            "openai",
            "house-model",
            Some("https://models.example.com/v1/")
        )
    );
    assert!(custom.has_key);
}

const TASKS_MANIFEST: &str = include_str!("../../../plugins/tasks/plugin.toml");

/// A provider whose first answer asks to create a task with `arguments`,
/// and whose later answers say Done.
struct CallsTool {
    requests: AtomicUsize,
    arguments: serde_json::Value,
}

impl AiStreamer for CallsTool {
    fn stream(
        &self,
        _request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
        let first = self.requests.fetch_add(1, Ordering::SeqCst) == 0;
        let arguments = self.arguments.clone();
        Box::pin(async move {
            let events = if first {
                vec![Ok(AiProviderStreamEvent::Completed {
                    usage: None,
                    stop_reason: None,
                    tool_calls: vec![AiToolCall {
                        call_id: "call_1".into(),
                        name: "tasks__create".into(),
                        arguments,
                    }],
                })]
            } else {
                vec![
                    Ok(AiProviderStreamEvent::TextDelta("Done.".into())),
                    Ok(AiProviderStreamEvent::Completed {
                        usage: None,
                        stop_reason: None,
                        tool_calls: Vec::new(),
                    }),
                ]
            };
            let stream: AiProviderEventStream = Box::pin(futures::stream::iter(events));
            Ok(stream)
        })
    }
}

struct Tooled {
    harness: Ui,
    provider: Arc<CallsTool>,
    workspace: std::path::PathBuf,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with the Tasks and AI plugins on and the provider Local
/// allowed, opened on the AI page, with the message sent that makes the
/// model ask to create the task Water the tomatoes.
fn asking_for_a_tool() -> Option<Tooled> {
    asking_with(serde_json::json!({ "title": "Water the tomatoes" }))
}

/// As [`asking_for_a_tool`], with the model asking to create a task with
/// `arguments`.
fn asking_with(arguments: serde_json::Value) -> Option<Tooled> {
    let component = plugins::built("ai_plugin")?;
    let tasks = plugins::built("tasks_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("Garden");
    create_workspace(app_data.path(), &workspace);
    let tasks_folder = parent.path().join("tasks-plugin");
    std::fs::create_dir_all(&tasks_folder).unwrap();
    let tasks = plugins::folder(&tasks_folder, TASKS_MANIFEST, "tasks_plugin.wasm", &tasks);
    plugins::install_and_enable(app_data.path(), &tasks, "tasks");
    let plugin = plugins::folder(parent.path(), MANIFEST, "ai_plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "ai");
    set_up_provider(app_data.path());
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    let provider = Arc::new(CallsTool {
        requests: AtomicUsize::new(0),
        arguments,
    });
    harness
        .state_mut()
        .app_mut()
        .set_ai_client(provider.clone());
    settle(&mut harness);
    open_page(&mut harness, "AI");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Form, MESSAGE)
            .is_some()
    });
    send(&mut harness, "Add a task to water the tomatoes");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::AlertDialog, TOOL_PROMPT)
            .is_some()
    });
    Some(Tooled {
        harness,
        provider,
        workspace,
        _folders: (app_data, parent),
    })
}

const TOOL_PROMPT: &str = "Allow this tool call?";

#[test]
fn noura_s_tool_prompt_names_the_tool_its_plugin_and_input_and_escape_denies() {
    let Some(mut tooled) = asking_for_a_tool() else {
        return;
    };
    let harness = &mut tooled.harness;
    let dialog = harness.get_by_role_and_label(Role::AlertDialog, TOOL_PROMPT);
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    // The prompt names the call as the chat does, and shows its input.
    // The tool's description, written for the model, stays out of it.
    let shown = texts(dialog.query_all(By::new().role(Role::Label)));
    assert_eq!(
        shown,
        [
            TOOL_PROMPT,
            "create from Tasks",
            "Input",
            "{\n  \"title\": \"Water the tomatoes\"\n}",
        ],
    );
    dialog.get_by_role_and_label(Role::Button, "Deny");
    dialog.get_by_role_and_label(Role::Button, "Allow once");
    assert_eq!(focused(harness).as_deref(), Some("Deny"));

    press(harness, Key::Escape);
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::AlertDialog, TOOL_PROMPT)
            .is_none()
            && all_labels(harness).contains(&"Done.".to_owned())
    });
    // The call shows as a quiet row that says you denied it, and opens to
    // say what that did and what the model asked for.
    let row = harness.get_by_role_and_label(Role::Button, "create from Tasks");
    assert_eq!(
        row.accesskit_node().description().as_deref(),
        Some("Denied")
    );
    assert_eq!(row.accesskit_node().data().is_expanded(), Some(false));
    assert!(!all_labels(harness).contains(&"You denied this call, so it didn't run.".to_owned()));
    row.click();
    run(harness);
    assert!(all_labels(harness).contains(&"You denied this call, so it didn't run.".to_owned()));
    assert_eq!(
        harness
            .get_by_role_and_label(Role::Button, "create from Tasks")
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(true)
    );
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    assert!(
        !tooled
            .workspace
            .join("Tasks/Water the tomatoes.md")
            .exists()
    );
    assert_eq!(tooled.provider.requests.load(Ordering::SeqCst), 2);
}

#[test]
fn tab_and_enter_allow_the_call_once() {
    let Some(mut tooled) = asking_for_a_tool() else {
        return;
    };
    let harness = &mut tooled.harness;
    assert_eq!(focused(harness).as_deref(), Some("Deny"));

    press(harness, Key::Tab);
    assert_eq!(focused(harness).as_deref(), Some("Allow once"));
    press(harness, Key::Enter);
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::AlertDialog, TOOL_PROMPT)
            .is_none()
            && all_labels(harness).contains(&"Done.".to_owned())
    });
    assert!(
        tooled
            .workspace
            .join("Tasks/Water the tomatoes.md")
            .exists()
    );
}

#[test]
fn a_long_input_scrolls_and_keeps_deny_and_allow_once_in_view() {
    let body = "Water the tomatoes at dawn, before the sun reaches the bed. ".repeat(120);
    let Some(mut tooled) = asking_with(serde_json::json!({
        "title": "Water the tomatoes",
        "body": body,
    })) else {
        return;
    };
    let harness = &mut tooled.harness;
    let dialog = harness.get_by_role_and_label(Role::AlertDialog, TOOL_PROMPT);
    assert!(
        texts(dialog.query_all(By::new().role(Role::Label)))
            .iter()
            .any(|label| label.contains(&body)),
        "the whole input stays readable"
    );
    // The window is 700 points tall.
    for button in ["Deny", "Allow once"] {
        let rect = dialog.get_by_role_and_label(Role::Button, button).rect();
        assert!(rect.max.y <= 700.0, "{button} at {rect:?}");
    }
    assert_eq!(focused(harness).as_deref(), Some("Deny"));
}

#[test]
fn without_a_provider_the_page_offers_open_ai_settings_which_opens_settings_on_ai() {
    let Some(component) = plugins::built("ai_plugin") else {
        return;
    };
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
    let plugin = plugins::folder(parent.path(), MANIFEST, "ai_plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "ai");
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    settle(&mut harness);
    open_page(&mut harness, "AI");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Open AI settings")
            .is_some()
    });
    harness.get_by_label("Connect an AI provider");

    harness
        .get_by_role_and_label(Role::Button, "Open AI settings")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Dialog, "Settings")
            .is_some()
    });
    assert_eq!(
        harness.state().app().settings().unwrap().section,
        app_core::settings_section::AI
    );
}
