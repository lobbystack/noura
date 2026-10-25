//! The AI plugin. It registers the `chat` and `chat-message` object types
//! with today's v1 fields and adds the AI page: the chats in the sidebar,
//! the open chat's messages with each answer drawn as Markdown while it
//! streams in, Stop, and a box to write the next message to the provider
//! you pick. Noura keeps the providers, their keys and your consent, and
//! writes the chat files; the plugin reaches them only through the host
//! AI interface.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `ai_plugin.wasm`.

mod page;
mod schemas;

use std::{collections::BTreeMap, sync::Mutex};

use plugin_sdk::{Error, Event, Node, ObjectType, Page, Plugin, View, navigation, objects};

use schemas::{CHAT, CHAT_MESSAGE, CHAT_MESSAGE_SCHEMA, CHAT_SCHEMA};

/// The AI page.
const PAGE: &str = "ai";

/// What the AI page shows, between calls.
struct State {
    /// The chat the page shows, or none for a new chat, which the first
    /// message you send creates.
    chat: Option<String>,
    /// The run of the last message you sent in each chat, by chat ID.
    runs: BTreeMap<String, String>,
    /// The provider you picked last.
    provider: Option<String>,
    /// How many messages the page sent, which names the message box, so
    /// the box empties after each one.
    sent: u32,
    /// What the message box holds when it next draws: a message that
    /// didn't go out.
    draft: String,
    /// The message of each run Noura hasn't written to the chat yet, by
    /// run ID. It comes back to the message box when the run ends without
    /// writing it: you said no, stopped it while Noura asked, or the write
    /// failed.
    unsent: BTreeMap<String, String>,
    /// Why the last thing you did failed.
    notice: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    chat: None,
    runs: BTreeMap::new(),
    provider: None,
    sent: 0,
    draft: String::new(),
    unsent: BTreeMap::new(),
    notice: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

struct Ai;

impl Plugin for Ai {
    fn object_types() -> Vec<ObjectType> {
        vec![
            ObjectType {
                name: CHAT.into(),
                schema: CHAT_SCHEMA.into(),
            },
            ObjectType {
                name: CHAT_MESSAGE.into(),
                schema: CHAT_MESSAGE_SCHEMA.into(),
            },
        ]
    }

    fn pages() -> Vec<Page> {
        vec![Page {
            id: PAGE.into(),
            title: "AI".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        page::render(&mut state())
    }

    fn sidebar(_page: String) -> Vec<Node> {
        page::sidebar(&state())
    }

    fn handle_event(_page: String, event: Event) {
        page::handle(&mut state(), event);
    }

    fn render_panel(object: String) -> Vec<Node> {
        let mut view = View::panel("chat-panel", Some("Chat"));
        match chat_of(&object) {
            Some(_) => {
                view.text(
                    View::ROOT,
                    "where",
                    "You read and continue this chat on the AI page.",
                );
                view.button(View::ROOT, "open-chat", "Open on the AI page");
            }
            None => {
                view.text(View::ROOT, "where", "This message's chat can't be found.");
            }
        }
        view.into_nodes()
    }

    fn handle_panel_event(object: String, event: Event) {
        if !matches!(&event, Event::Pressed(node) if node == "open-chat") {
            return;
        }
        let Some(chat) = chat_of(&object) else {
            return;
        };
        let opened = navigation::open_page(PAGE);
        let mut state = state();
        state.chat = Some(chat);
        state.notice = opened.err().map(|error| reason(&error));
    }
}

/// The chat an object of the plugin's types belongs to: a chat itself, or
/// a message's chat.
fn chat_of(object: &str) -> Option<String> {
    if object.starts_with("chat_") {
        return Some(object.to_owned());
    }
    let message = objects::get(object).ok()?;
    let properties: serde_json::Value = serde_json::from_str(&message.properties).ok()?;
    properties
        .get("chat_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Why a call to the host failed, in words for the page.
fn reason(error: &Error) -> String {
    match error {
        Error::CapabilityNotDeclared(capability) => {
            format!("The AI plugin isn't allowed the {capability} capability.")
        }
        Error::NotFound(_) => "Noura can't find it.".to_owned(),
        Error::RevisionConflict(_) => {
            "The chat changed elsewhere. Showing the latest version.".to_owned()
        }
        Error::IdentityConflict(_) => "This chat is in more than one file, as after a copy in \
            another app, so it takes no messages until you remove the copy."
            .to_owned(),
        Error::Failed(message) => message.clone(),
        _ => "Noura couldn't do that.".to_owned(),
    }
}

plugin_sdk::export!(Ai);
