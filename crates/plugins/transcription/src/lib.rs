//! The Transcription plugin. It records a meeting through the kernel's
//! speech, writes its Transcript, adds a Summary below your notes with
//! your AI provider, and creates a task through Tasks for each action
//! item, linked back to the Transcription by its `source`.
//!
//! A Transcription is a folder, `Meetings/<title>/` or the chosen
//! project's folder, holding `<title>.md`, the `transcription` object
//! with your notes, and `Transcript.md`, a `transcript` object with one
//! paragraph per speaker turn. A recording starts from the panel beside
//! a Transcription, the command palette, the `transcription` shortcut,
//! the menu bar, or the menu bar item that shows while another app uses
//! the microphone. Nothing records by itself.
//!
//! Where this plugin settles something the spec leaves open, it follows
//! anarlog: the `enhance` prompt templates, the Summary's length policy
//! and the `language` setting. See `docs/provenance/anarlog.md`.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `transcription_plugin.wasm`.

mod length;
mod meeting;
mod notes;
mod panel;
mod prompt;
mod record;
mod settings;
mod summarize;
mod turns;
mod types;

use std::{collections::BTreeMap, sync::Mutex};

use plugin_sdk::{Command, Error, Event, KernelEvent, Node, ObjectType, Plugin, navigation};

use types::{
    SETTINGS, SETTINGS_SCHEMA, TRANSCRIPT, TRANSCRIPT_SCHEMA, TRANSCRIPTION, TRANSCRIPTION_SCHEMA,
};

/// Where a Summary stands, for the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SummaryStatus {
    /// The Summary is in the body. `tasks` says why no tasks came, when
    /// they didn't.
    Done {
        tasks: Option<String>,
    },
    /// You have no AI provider, so Summarize waits for one.
    NoProvider,
    Failed(String),
}

/// A recording in progress, and the Transcription it writes.
#[derive(Debug, Clone)]
pub(crate) struct Active {
    pub(crate) recording: String,
    pub(crate) object: String,
    /// When it started, in RFC 3339 with this device's offset.
    pub(crate) start: String,
    /// You stopped it, and Noura is transcribing what's left.
    pub(crate) stopping: bool,
}

/// What the plugin keeps between calls. Everything durable is in files.
pub(crate) struct State {
    pub(crate) active: Option<Active>,
    /// Another app uses the microphone, so the menu offers a recording.
    pub(crate) offer: bool,
    /// The last problem of each Transcription, by its ID.
    pub(crate) problems: BTreeMap<String, String>,
    /// The last Summary of each Transcription this session.
    pub(crate) summaries: BTreeMap<String, SummaryStatus>,
    /// The project the AI suggested for a Transcription without one, as
    /// its ID and title.
    pub(crate) suggestions: BTreeMap<String, (String, String)>,
    /// The menu bar entries last set, so an unchanged menu isn't set
    /// again.
    pub(crate) menu: Option<Vec<String>>,
}

static STATE: Mutex<State> = Mutex::new(State {
    active: None,
    offer: false,
    problems: BTreeMap::new(),
    summaries: BTreeMap::new(),
    suggestions: BTreeMap::new(),
    menu: None,
});

pub(crate) fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

/// What a host error means, in words.
pub(crate) fn reason(error: &Error) -> String {
    match error {
        Error::Failed(message) | Error::NetworkRefused(message) => message.clone(),
        Error::RevisionConflict(_) => "it changed elsewhere. Try again.".to_owned(),
        Error::NotFound(_) => "it no longer exists.".to_owned(),
        Error::AlreadyExists(path) => format!("{path} already exists."),
        Error::IdentityConflict(_) => "two files hold its ID.".to_owned(),
        Error::InvalidProperty(problem) => {
            format!("{} doesn't fit ({}).", problem.property, problem.code)
        }
        Error::CapabilityNotDeclared(capability) => format!("it needs {capability}."),
        Error::TargetNotDeclared(target) | Error::TargetUnavailable(target) => {
            format!("{target} isn't available. Turn it on in Settings.")
        }
        Error::Busy(plugin) => format!("{plugin} is busy. Try again."),
        Error::InvalidPath(path) => format!("{path} isn't a path Noura writes."),
        Error::UnsupportedPath => "a file name can't be read.".to_owned(),
    }
}

struct Transcription;

impl Plugin for Transcription {
    fn object_types() -> Vec<ObjectType> {
        // Noura calls this first, when the plugin starts, so the menu
        // bar's Start Transcription shows from then on.
        record::refresh_menu();
        [
            (TRANSCRIPTION, TRANSCRIPTION_SCHEMA),
            (TRANSCRIPT, TRANSCRIPT_SCHEMA),
            (SETTINGS, SETTINGS_SCHEMA),
        ]
        .into_iter()
        .map(|(name, schema)| ObjectType {
            name: name.into(),
            schema: schema.into(),
        })
        .collect()
    }

    fn render_panel(object: String) -> Vec<Node> {
        panel::render(&object)
    }

    fn handle_panel_event(object: String, event: Event) {
        panel::handle(&object, event);
    }

    fn settings_page() -> Option<String> {
        Some("Transcription".into())
    }

    fn render_settings() -> Vec<Node> {
        settings::render()
    }

    fn handle_settings_event(event: Event) {
        settings::handle(event);
    }

    fn commands() -> Vec<Command> {
        let palette = |id: &str, title: &str| Command {
            id: id.into(),
            title: title.into(),
            palette: true,
            plugins: false,
        };
        vec![
            palette("new-transcription", "New Transcription"),
            palette("stop-transcription", "Stop Transcription"),
        ]
    }

    fn run_command(id: String, _input: String) -> Result<String, String> {
        match id.as_str() {
            "new-transcription" => record::start_new().map(|_| String::new()),
            "stop-transcription" => record::stop().map(|()| String::new()),
            _ => Err(format!("Transcription has no command {id}.")),
        }
    }

    fn handle_kernel_event(event: KernelEvent) {
        record::handle_kernel_event(event);
    }
}

/// Open the object `id` in a tab of its own, as after you made it.
pub(crate) fn open(id: &str) {
    let _ = navigation::open_object(id, navigation::Placement::NewTab);
}

plugin_sdk::export!(Transcription);
