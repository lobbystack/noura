//! The properties panel beside a Transcription: Start and Stop, the
//! timer, the project picker, the attendees and the live Transcript while
//! it records;
//! after you stop, the Summary's status with Summarize, the tasks it
//! created and the project the AI suggests.

use plugin_sdk::{
    Event, Node, Submission, Value, View,
    navigation::{self, SettingsSection},
    objects::{self, Object},
    speech::{self, RecordingState},
};

use crate::{
    SummaryStatus, meeting, notes, reason, record, state, summarize, turns, types::TRANSCRIPTION,
};

/// The panel's nodes for the object `id`: none for the plugin's other
/// types, so their body takes the editor's width.
pub(crate) fn render(id: &str) -> Vec<Node> {
    record::refresh_menu();
    let empty = || View::panel("root", None).into_nodes();
    let Ok(object) = objects::get(id) else {
        return empty();
    };
    if object.object_type != TRANSCRIPTION {
        return empty();
    }
    let mut view = View::panel("root", Some("Transcription"));
    let active = state().active.clone().filter(|active| active.object == id);
    match active {
        Some(active) => recording(&mut view, &active.recording, active.stopping),
        None => stopped(&mut view, &object),
    }
    project_picker(&mut view, &object);
    attendees_field(&mut view, &object);
    if let Some(problem) = state().problems.get(id) {
        view.text(View::ROOT, "problem", problem);
    }
    view.into_nodes()
}

/// Stop, the timer and the live Transcript.
fn recording(view: &mut View, recording: &str, stopping: bool) {
    let Ok(now) = speech::read_recording(recording) else {
        return;
    };
    if stopping || now.state == RecordingState::Finishing {
        view.text(View::ROOT, "status", "Finishing…");
    } else {
        view.button(View::ROOT, "stop", "Stop");
    }
    view.text(View::ROOT, "timer", &turns::clock(now.elapsed_ms));
    let mut heard = now.segments.clone();
    heard.extend(now.pending.iter().cloned());
    let live = turns::markdown(&turns::turns(&heard));
    if !live.is_empty() {
        view.section_heading(View::ROOT, "transcript-heading", "Transcript", None);
        view.markdown(View::ROOT, "transcript", &live);
    }
}

/// Start before anything is recorded; afterwards the Summary, its tasks
/// and the suggested project.
fn stopped(view: &mut View, object: &Object) {
    let transcript = meeting::transcript_of(object);
    let recorded = transcript
        .as_ref()
        .is_some_and(|transcript| !turns::lines(&transcript.body).is_empty());
    if !recorded {
        view.button(View::ROOT, "start", "Start");
        return;
    }
    let heading = view.section_heading(View::ROOT, "summary-heading", "Summary", None);
    view.button(heading, "summarize", "Summarize");
    let status = state().summaries.get(&object.id).cloned();
    match status {
        Some(SummaryStatus::Done { tasks }) => {
            view.text(View::ROOT, "summary-status", "Summary added");
            if let Some(problem) = tasks {
                view.text(View::ROOT, "tasks-problem", &problem);
            }
        }
        Some(SummaryStatus::NoProvider) => {
            view.text(
                View::ROOT,
                "summary-status",
                "Add an AI provider to summarize.",
            );
            view.button(View::ROOT, "ai-settings", "AI settings");
        }
        Some(SummaryStatus::Failed(problem)) => {
            view.text(
                View::ROOT,
                "summary-status",
                &format!("The Summary failed: {problem}"),
            );
        }
        None if notes::summary(&object.body).is_some() => {
            view.text(View::ROOT, "summary-status", "Summary added");
        }
        None => {}
    }
    let suggestion = state().suggestions.get(&object.id).cloned();
    if let Some((project, title)) = suggestion
        && meeting::project_of(object).as_deref() != Some(project.as_str())
    {
        view.button(View::ROOT, "use-project", &format!("Add to {title}"));
    }
    if let Ok(tasks) = summarize::tasks_of(&object.id)
        && !tasks.is_empty()
    {
        view.section_heading(View::ROOT, "tasks-heading", "Tasks", None);
        let list = view.list(View::ROOT, "tasks", "Tasks");
        view.open_on_click(list);
        for task in &tasks {
            let detail = task.done.then_some("Done");
            view.item(list, &task.id, &task.title, detail);
        }
    }
}

fn project_picker(view: &mut View, object: &Object) {
    let projects = meeting::projects();
    let mut choices: Vec<(&str, &str)> = vec![("none", "None")];
    choices.extend(
        projects
            .iter()
            .map(|(id, title)| (id.as_str(), title.as_str())),
    );
    let current = meeting::project_of(object);
    view.choice_input(
        View::ROOT,
        "project",
        "Project",
        &choices,
        Some(current.as_deref().unwrap_or("none")),
    );
}

/// The form that names who takes part.
const ATTENDEES_FORM: &str = "attendees-form";

/// Who takes part, as names separated by commas, saved when you press
/// Save or Enter rather than on each key.
fn attendees_field(view: &mut View, object: &Object) {
    let names: Vec<String> = meeting::properties(object)
        .get("attendees")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect();
    let form = view.form(View::ROOT, ATTENDEES_FORM, "Attendees", "Save");
    view.input(
        form,
        "attendees",
        "Attendees",
        Value::Text(names.join(", ")),
    );
}

/// The names a submission of the attendees form holds: split at commas
/// and new lines, each on one line, each once whatever its case.
pub(crate) fn attendee_names(submission: &Submission) -> Vec<String> {
    let text = submission
        .fields
        .iter()
        .find_map(|field| match &field.value {
            Value::Text(text) | Value::Multiline(text) => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for name in text.split([',', '\n']) {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty()
            && !names
                .iter()
                .any(|known| known.to_lowercase() == name.to_lowercase())
        {
            names.push(name);
        }
    }
    names
}

/// Answer an event in the panel of the Transcription `id`.
pub(crate) fn handle(id: &str, event: Event) {
    let event_node = match &event {
        Event::Pressed(node) => Some(node.clone()),
        _ => None,
    };
    let outcome: Result<(), String> = match event {
        Event::Pressed(node) => match node.as_str() {
            "start" => record::start(id),
            "stop" => record::stop(),
            "summarize" => {
                let status = summarize::run(id);
                state().summaries.insert(id.to_owned(), status);
                Ok(())
            }
            "use-project" => {
                let suggestion = state().suggestions.get(id).cloned();
                match suggestion {
                    Some((project, _)) => meeting::set_project(id, Some(&project))
                        .map(|()| {
                            state().suggestions.remove(id);
                        })
                        .map_err(|error| {
                            format!("Noura couldn't set the project: {}", reason(&error))
                        }),
                    None => Ok(()),
                }
            }
            "ai-settings" => {
                navigation::open_settings(SettingsSection::Ai).map_err(|error| reason(&error))
            }
            _ => Ok(()),
        },
        Event::Changed(field) if field.node == "project" => {
            let project = match &field.value {
                Value::Choice(Some(choice)) if choice != "none" => Some(choice.clone()),
                _ => None,
            };
            meeting::set_project(id, project.as_deref())
                .map_err(|error| format!("Noura couldn't set the project: {}", reason(&error)))
        }
        Event::Submitted(submission) if submission.node == ATTENDEES_FORM => meeting::set(
            id,
            serde_json::json!({ "attendees": attendee_names(&submission) }),
        )
        .map(|_| ())
        .map_err(|error| format!("Noura couldn't save the attendees: {}", reason(&error))),
        Event::Activated(target) if target.node == "tasks" => {
            navigation::open_object(&target.item, navigation::Placement::NewTab)
                .map_err(|error| reason(&error))
        }
        _ => Ok(()),
    };
    let mut state = state();
    match outcome {
        // Start and Stop keep the problem a recording that ended left;
        // a recording that starts clears it.
        Ok(()) if matches!(&event_node, Some(node) if node == "start" || node == "stop") => {}
        Ok(()) => {
            state.problems.remove(id);
        }
        Err(problem) => {
            state.problems.insert(id.to_owned(), problem);
        }
    }
}

#[cfg(test)]
mod tests {
    use plugin_sdk::{Field, Submission, Value};

    use super::attendee_names;

    #[test]
    fn attendees_split_at_commas_and_lines_and_show_once() {
        let submission = Submission {
            node: "attendees-form".into(),
            fields: vec![Field {
                node: "attendees".into(),
                value: Value::Text(" Alice ,Bob\n  Grace   Hopper,, alice ".into()),
            }],
        };
        assert_eq!(
            attendee_names(&submission),
            ["Alice", "Bob", "Grace Hopper"]
        );
    }
}
