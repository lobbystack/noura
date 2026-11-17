//! Recording: starting and stopping, the menu bar entries, the kernel
//! events, and what happens once the kernel finishes the Transcript:
//! `Transcript.md`, the end time, the audio, and the Summary.

use plugin_sdk::{
    KernelEvent,
    menu_bar::{self, Entry, EntryKind},
    notifications::{self, Action, Notification},
    objects::Changes,
    speech::{self, RecordingOptions, RecordingState, Source},
};
use serde_json::json;

use crate::{
    Active, meeting, open, reason,
    settings::{Audio, Settings},
    state, summarize, turns,
};

/// The shortcut the manifest declares.
pub(crate) const SHORTCUT: &str = "transcription";

/// The notification that offers to transcribe a call.
const OFFER: &str = "offer";

/// Offer to transcribe the call another app, named `app` when the system
/// says, turned the microphone on for.
fn offer(app: Option<&str>) {
    if state().active.is_some() {
        return;
    }
    let body = app.map_or_else(String::new, |app| format!("{app} is using the microphone."));
    let _ = notifications::notify(&Notification {
        id: OFFER.to_owned(),
        title: "Transcribe this call?".to_owned(),
        body,
        actions: vec![Action {
            id: "transcribe".to_owned(),
            label: "Transcribe".to_owned(),
        }],
    });
}

/// Show the menu bar entries that fit the state now: Start or Stop
/// Transcription, and Transcribe This Call while another app uses the
/// microphone.
pub(crate) fn refresh_menu() {
    let mut state = state();
    let mut entries: Vec<(&str, &str, bool)> = Vec::new();
    match &state.active {
        Some(active) if !active.stopping => entries.push(("stop", "Stop Transcription", true)),
        Some(_) => entries.push(("start", "Start Transcription", false)),
        None => {
            if state.offer {
                entries.push(("offer", "Transcribe This Call", true));
            }
            entries.push(("start", "Start Transcription", true));
        }
    }
    let key: Vec<String> = entries
        .iter()
        .map(|(id, _, enabled)| format!("{id}:{enabled}"))
        .collect();
    if state.menu.as_ref() == Some(&key) {
        return;
    }
    let entries: Vec<Entry> = entries
        .into_iter()
        .map(|(id, label, enabled)| Entry {
            id: id.into(),
            kind: EntryKind::Item,
            label: label.into(),
            enabled,
            checked: None,
            parent: None,
        })
        .collect();
    if menu_bar::set_entries(&entries).is_ok() {
        state.menu = Some(key);
    }
}

/// Start recording into the Transcription `object`.
pub(crate) fn start(object: &str) -> Result<(), String> {
    if state().active.is_some() {
        return Err("A Transcription is already recording.".to_owned());
    }
    let preferences = speech::read_preferences().map_err(|error| reason(&error))?;
    let mut sources = vec![Source::Microphone];
    if preferences.system_audio {
        sources.push(Source::SystemAudio);
    }
    let recording = speech::start(&RecordingOptions {
        sources,
        model: None,
        microphone: None,
        language: None,
    })
    .map_err(|error| reason(&error))?;
    let start = meeting::rfc3339(&meeting::now());
    if let Err(error) = meeting::set(object, json!({ "start": start })) {
        let _ = speech::cancel(&recording);
        return Err(format!(
            "Noura couldn't save the start time: {}",
            reason(&error)
        ));
    }
    let _ = notifications::withdraw(OFFER);
    {
        let mut state = state();
        state.problems.remove(object);
        state.active = Some(Active {
            recording,
            object: object.to_owned(),
            start,
            stopping: false,
        });
    }
    refresh_menu();
    Ok(())
}

/// Make a Transcription, open it and start recording into it.
pub(crate) fn start_new() -> Result<String, String> {
    if state().active.is_some() {
        return Err("A Transcription is already recording.".to_owned());
    }
    let object = meeting::create(None)
        .map_err(|error| format!("Noura couldn't make the Transcription: {}", reason(&error)))?;
    open(&object.id);
    if let Err(problem) = start(&object.id) {
        state().problems.insert(object.id.clone(), problem.clone());
        return Err(problem);
    }
    Ok(object.id)
}

/// Stop the recording. Noura transcribes what's left, then
/// [`finish`] writes the Transcript.
pub(crate) fn stop() -> Result<(), String> {
    let recording = {
        let mut state = state();
        let Some(active) = state.active.as_mut().filter(|active| !active.stopping) else {
            return Err("No Transcription is recording.".to_owned());
        };
        active.stopping = true;
        active.recording.clone()
    };
    let stopped = speech::stop(&recording).map_err(|error| reason(&error));
    refresh_menu();
    stopped?;
    // A recording with nothing left to transcribe may already be done.
    changed(&recording);
    Ok(())
}

pub(crate) fn handle_kernel_event(event: KernelEvent) {
    match event {
        KernelEvent::ShortcutPressed(event) if event.id == SHORTCUT => {
            let recording = state()
                .active
                .as_ref()
                .is_some_and(|active| !active.stopping);
            let _ = if recording {
                stop()
            } else {
                start_new().map(drop)
            };
        }
        KernelEvent::MenuItemChosen(id) => {
            let _ = match id.as_str() {
                "start" | "offer" => start_new().map(drop),
                "stop" => stop(),
                _ => Ok(()),
            };
        }
        // Noura never records by itself: a notification and the menu
        // offer a recording.
        KernelEvent::MicrophoneInUse(using) => {
            state().offer = true;
            offer(using.app.as_deref());
            refresh_menu();
        }
        KernelEvent::MicrophoneFree => {
            state().offer = false;
            let _ = notifications::withdraw(OFFER);
            refresh_menu();
        }
        // Choosing the notification records, as its action does.
        KernelEvent::NotificationChosen(choice) if choice.id == OFFER => {
            let _ = notifications::withdraw(OFFER);
            if state().active.is_none() {
                let _ = start_new();
            }
        }
        KernelEvent::RecordingChanged(id) => changed(&id),
        _ => {}
    }
    refresh_menu();
}

/// The recording `id` changed state: once it finished or failed, write
/// what it heard.
fn changed(id: &str) {
    let Some(active) = state()
        .active
        .clone()
        .filter(|active| active.recording == id)
    else {
        return;
    };
    let Ok(recording) = speech::read_recording(id) else {
        state().active = None;
        refresh_menu();
        return;
    };
    match recording.state {
        RecordingState::Finished | RecordingState::Failed => finish(&active, &recording),
        RecordingState::Cancelled => {
            state().active = None;
            refresh_menu();
        }
        RecordingState::Recording | RecordingState::Finishing => {}
    }
}

/// Write the Transcript, the end time and the audio of a recording that
/// ended, forget it, then write the Summary when you have a provider.
fn finish(active: &Active, recording: &speech::Recording) {
    let object = active.object.clone();
    let mut problem = recording
        .failure
        .clone()
        .map(|failure| format!("The recording stopped: {failure}"));
    let written = write_transcript(active, recording);
    if let Err(error) = &written {
        problem = Some(error.clone());
    }
    if let Err(error) = keep_audio(&active.recording, &object) {
        problem.get_or_insert(error);
    }
    let _ = speech::forget(&active.recording);
    {
        let mut state = state();
        state.active = None;
        match &problem {
            Some(problem) => {
                state.problems.insert(object.clone(), problem.clone());
            }
            None => {
                state.problems.remove(&object);
            }
        }
    }
    refresh_menu();
    if written.is_ok_and(|turns| turns > 0) {
        let status = summarize::run(&object);
        state().summaries.insert(object, status);
    }
}

/// Write the recording's turns to the Transcript and its end to the
/// Transcription. Returns how many turns it wrote.
fn write_transcript(active: &Active, recording: &speech::Recording) -> Result<usize, String> {
    let failed = |error: plugin_sdk::Error| {
        format!("Noura couldn't save the Transcript: {}", reason(&error))
    };
    let turns = turns::turns(&recording.segments);
    let transcription = plugin_sdk::objects::get(&active.object).map_err(failed)?;
    let transcript = meeting::transcript_or_new(&transcription).map_err(failed)?;
    let body = turns::markdown(&turns);
    meeting::update(&transcript.id, |_| Changes {
        body: Some(body.clone()),
        properties: "{}".to_owned(),
        remove: Vec::new(),
    })
    .map_err(failed)?;
    if let Some(end) = meeting::later(&active.start, recording.elapsed_ms) {
        meeting::set(&active.object, json!({ "end": end })).map_err(failed)?;
    }
    Ok(turns.len())
}

/// Keep the audio as the setting says: nowhere, beside the Transcription,
/// or on this device for some days.
fn keep_audio(recording: &str, object: &str) -> Result<(), String> {
    let settings = Settings::read();
    let kept = match settings.audio {
        Audio::None => return Ok(()),
        Audio::Folder => speech::save_audio(recording, object).map(drop),
        Audio::Device => speech::keep_audio(recording, settings.audio_days),
    };
    kept.map_err(|error| format!("Noura couldn't keep the audio: {}", reason(&error)))
}
