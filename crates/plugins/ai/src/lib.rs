//! The AI plugin. It registers the `chat` and `chat-message` object types
//! with today's v1 fields, and the `orb` and `orb-run` types of Orbs, and
//! adds the Orbs page. The sidebar lists your Orbs, the default Orb first,
//! each with its avatar and a dot when it works, waits or failed, and New
//! Orb creates one. Each Orb keeps one thread: your messages and its
//! answers, drawn as Markdown while they stream in, its tool calls as
//! quiet rows, and a box at the bottom to write to it, with Stop while an
//! answer comes in. The pill with the Orb's name at the top opens its side
//! panel: its name, the model it asks and how hard that model reasons, its
//! recent runs and its triggers. The default Orb's plain chats are listed
//! there, each with a menu to rename it or have it removed after 30 days.
//!
//! An Orb runs the tools its file lists without asking. A plain chat
//! offers the model the tools of the plugins on in the workspace, and the
//! plugin hands the calls the model asks for to Noura, which asks you
//! about each one. Noura keeps the providers and their keys, and writes
//! the chat and Orb files; the plugin reaches them only through the host
//! AI interface. Once you add a Brave Search key, a plain chat's model may
//! also search the web and read pages, which Noura does for the plugin.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `ai_plugin.wasm`.

mod look;
mod page;
mod panel;
mod schemas;
mod sidebar;
mod triggers;
mod web;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use plugin_sdk::{Error, Event, Node, ObjectType, Page, Plugin, View, navigation, objects};

use schemas::{
    CHAT, CHAT_MESSAGE, CHAT_MESSAGE_SCHEMA, CHAT_SCHEMA, ORB, ORB_RUN, ORB_RUN_SCHEMA, ORB_SCHEMA,
};

/// The AI page.
const PAGE: &str = "ai";

/// What the AI page shows, between calls.
struct State {
    /// The Orb the page shows, or none for the default Orb.
    orb: Option<String>,
    /// The default Orb's plain chat the page shows: none for its latest,
    /// or a new chat when `new_chat` says so, which the first message you
    /// send creates.
    chat: Option<String>,
    new_chat: bool,
    /// Whether the Orb's side panel shows.
    panel: bool,
    /// The run of the last message you sent in each chat, by chat ID.
    runs: BTreeMap<String, String>,
    /// The runs that are an Orb's, whose tool calls Noura runs itself.
    orb_runs: BTreeSet<String>,
    /// What the message box holds when it next draws: a message that
    /// didn't go out. The box empties itself when you send.
    draft: String,
    /// The message of each run Noura hasn't written to the chat yet, by
    /// run ID. It comes back to the message box when the run ends without
    /// writing it: you said no, stopped it while Noura asked, or the write
    /// failed.
    unsent: BTreeMap<String, String>,
    /// Why the last thing you did in the thread failed.
    notice: Option<String>,
    /// Why the last change in the side panel failed.
    panel_notice: Option<String>,
    /// The chat whose item shows as a field you rename it in, and the
    /// revision of its `chat.md` when you chose Rename.
    renaming: Option<(String, String)>,
    /// The sidebar shows a field to name a new Orb.
    naming: bool,
    /// Why the last Orb you named wasn't created.
    sidebar_notice: Option<String>,
    /// The turns of the model's whose tool calls the plugin couldn't
    /// answer, by run ID and call IDs, so a failed answer doesn't search
    /// the web again each time the page draws. An answer that goes in
    /// leaves no entry: the run streams again, and its next calls are a
    /// new turn even when the model gives them the same IDs.
    failed_turns: BTreeSet<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    orb: None,
    chat: None,
    new_chat: false,
    panel: false,
    runs: BTreeMap::new(),
    orb_runs: BTreeSet::new(),
    draft: String::new(),
    unsent: BTreeMap::new(),
    notice: None,
    panel_notice: None,
    renaming: None,
    naming: false,
    sidebar_notice: None,
    failed_turns: BTreeSet::new(),
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
            ObjectType {
                name: ORB.into(),
                schema: ORB_SCHEMA.into(),
            },
            ObjectType {
                name: ORB_RUN.into(),
                schema: ORB_RUN_SCHEMA.into(),
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
        sidebar::sidebar(&state())
    }

    fn handle_event(_page: String, event: Event) {
        page::handle(&mut state(), event);
    }

    fn render_panel(object: String) -> Vec<Node> {
        let mut view = View::panel("chat-panel", Some("Chat"));
        match chat_of(&object) {
            Some(_) => {
                view.button(View::ROOT, "open-chat", "Open chat");
            }
            None => {
                view.text(View::ROOT, "where", "Chat not found");
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
        page::open_chat(&mut state, &chat);
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
        Error::AlreadyExists(_) => "An Orb already has that name.".to_owned(),
        Error::InvalidProperty(problem) => match problem.property.as_str() {
            "triggers" => "Noura can't read that trigger.".to_owned(),
            "model" => "Noura can't use that model name.".to_owned(),
            _ => format!("The Orb's {} isn't valid.", problem.property),
        },
        Error::IdentityConflict(_) => "This chat is in more than one file, as after a copy in \
            another app, so it takes no messages until you remove the copy."
            .to_owned(),
        Error::Failed(message) => message.clone(),
        _ => "Noura couldn't do that.".to_owned(),
    }
}

plugin_sdk::export!(Ai);
