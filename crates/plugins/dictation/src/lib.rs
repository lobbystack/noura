//! The Dictation plugin. Hold or tap its shortcut, Alt+Space on macOS
//! and Ctrl+Space on Windows and Linux to start with, speak, and it pastes what you said into the app you're typing
//! in, as Handy does. Noura records and transcribes on this device with
//! the model you pick; the plugin applies your custom words and pastes
//! the text. Its menu bar or tray entries copy the last dictation and
//! pick the model and the language. Its page in Settings holds your
//! custom words, how the shortcut starts and stops a recording, and how
//! the text goes in, all kept in the workspace file
//! `Settings/Dictation.md`.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `dictation_plugin.wasm`.

mod activation;
mod dictation;
mod host;
mod languages;
mod menu;
mod settings;
mod words;

use std::sync::Mutex;

use plugin_sdk::{Event, KernelEvent, Node, ObjectType, Plugin, View};

use dictation::{Dictation, SHORTCUT};
use host::Kernel;
use settings::{SCHEMA, SETTINGS_TYPE};

static STATE: Mutex<Dictation> = Mutex::new(Dictation::new());

fn state() -> std::sync::MutexGuard<'static, Dictation> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

struct DictationPlugin;

impl Plugin for DictationPlugin {
    fn object_types() -> Vec<ObjectType> {
        vec![ObjectType {
            name: SETTINGS_TYPE.into(),
            schema: SCHEMA.into(),
        }]
    }

    /// The settings file needs no panel beside it.
    fn render_panel(_object: String) -> Vec<Node> {
        View::panel("root", None).into_nodes()
    }

    fn settings_page() -> Option<String> {
        // Noura asks for the page when the plugin starts, which is when the
        // menu first shows.
        state().refresh_menu(&Kernel);
        Some("Dictation".into())
    }

    fn render_settings() -> Vec<Node> {
        let mut state = state();
        // Noura draws the page after every event, so the menu keeps up
        // with models you download and the last dictation.
        state.refresh_menu(&Kernel);
        state.render_settings(&Kernel)
    }

    fn handle_settings_event(event: Event) {
        state().settings_event(&Kernel, event);
    }

    fn handle_kernel_event(event: KernelEvent) {
        let mut state = state();
        match event {
            KernelEvent::ShortcutPressed(event) if event.id == SHORTCUT => {
                state.shortcut(&Kernel, true, event.at_ms);
            }
            KernelEvent::ShortcutReleased(event) if event.id == SHORTCUT => {
                state.shortcut(&Kernel, false, event.at_ms);
            }
            KernelEvent::NotificationChosen(choice) => {
                state.notification_chosen(&Kernel, &choice.id);
            }
            KernelEvent::MenuItemChosen(id) => state.menu_chosen(&Kernel, &id),
            KernelEvent::RecordingChanged(id) => state.recording_changed(&Kernel, &id),
            _ => {}
        }
        state.refresh_menu(&Kernel);
    }
}

plugin_sdk::export!(DictationPlugin);
