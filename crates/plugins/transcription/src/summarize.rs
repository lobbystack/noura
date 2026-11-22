//! The Summary and the tasks: anarlog's `enhance` prompt through the
//! chatless `ai` completion, written as the `## Summary` section below
//! your notes, then a JSON list of action items, each created through
//! Tasks' `create` with `source` linking back to the Transcription and
//! `assignee` naming its owner.

use plugin_sdk::{
    ai::{self, CompletionError, CompletionRequest},
    command_calls,
    objects::{self, Changes},
};
use serde_json::{Value, json};

use crate::{
    SummaryStatus, length, meeting, notes, prompt, reason, settings::Settings, state, turns,
};

/// What the second request asks the provider for.
const ACTION_ITEMS: &str = "Find the action items of this meeting: what someone agreed to do. \
Answer with one JSON object and nothing else:
{\"action_items\": [{\"title\": \"...\", \"owner\": \"...\", \"due\": \"YYYY-MM-DD\"}], \"project\": \"...\"}

- title: a short task in the imperative, in {language}, without the owner's name.
- owner: the person who agreed to do it. When the meeting lists Participants, use one of those names exactly as written. Otherwise use the name said in the meeting. Use null when no name fits. The speaker labels Me and Others aren't names.
- due: the date it is due when someone stated one, otherwise null.
- Include only explicit or unambiguous commitments. Don't turn a proposal into a task.
- project: the exact title of the project in the Projects list that this meeting is about, or null when none fits.
- An empty list is a fine answer.";

/// A task whose `source` leads to a Transcription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRow {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) done: bool,
}

/// The tasks whose `source` leads to the Transcription `id`, by title.
pub(crate) fn tasks_of(id: &str) -> Result<Vec<TaskRow>, String> {
    let answer = command_calls::call("tasks", "list", "{}").map_err(|error| reason(&error))?;
    let answer: Value = serde_json::from_str(&answer).unwrap_or_default();
    let mut rows: Vec<TaskRow> = answer["tasks"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|task| {
            let (Some(source), Some(path)) =
                (task["properties"]["source"].as_str(), task["path"].as_str())
            else {
                return false;
            };
            meeting::resolve(source, path).is_some_and(|source| source == id)
        })
        .filter_map(|task| {
            Some(TaskRow {
                id: task["id"].as_str()?.to_owned(),
                title: task["title"].as_str()?.to_owned(),
                done: task["properties"]["status"] == "done",
            })
        })
        .collect();
    rows.sort_by_key(|row| row.title.to_lowercase());
    Ok(rows)
}

fn failed(error: &CompletionError) -> (bool, String) {
    match error {
        CompletionError::Refused(error) => (false, reason(error)),
        CompletionError::Failed(failure) => {
            (failure.code == "no_provider", failure.message.clone())
        }
    }
}

/// An action item the provider found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionItem {
    pub(crate) title: String,
    /// Who agreed to do it, by name.
    pub(crate) owner: Option<String>,
    pub(crate) due: Option<String>,
}

/// `text` on one line, with single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether two names are the same but for case.
fn same_name(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// The owner an item names: its name on one line, or none for an empty
/// name or a `null` written as text.
fn owner_of(item: &Value) -> Option<String> {
    let owner = one_line(item.get("owner")?.as_str()?);
    (!owner.is_empty() && !owner.eq_ignore_ascii_case("null"))
        .then(|| owner.chars().take(100).collect())
}

/// `title` without a leading `Owner:`, which a provider may still write.
fn without_owner(title: String, owner: Option<&str>) -> String {
    let Some(owner) = owner else {
        return title;
    };
    let rest = title
        .get(..owner.len())
        .filter(|start| same_name(start, owner))
        .and_then(|_| title[owner.len()..].strip_prefix(':'))
        .map(str::trim_start)
        .filter(|rest| !rest.is_empty());
    match rest {
        Some(rest) => {
            let mut chars = rest.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
        None => title,
    }
}

/// The action items and the suggested project's title in the provider's
/// JSON. A malformed item drops out.
pub(crate) fn parse_answer(text: &str) -> (Vec<ActionItem>, Option<String>) {
    let value: Value = serde_json::from_str(text.trim()).unwrap_or_default();
    let list = value
        .get("action_items")
        .or_else(|| value.get("tasks"))
        .or(value.is_array().then_some(&value))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let items = list
        .iter()
        .filter_map(|item| {
            let title = match item {
                Value::String(title) => title.as_str(),
                item => item.get("title")?.as_str()?,
            };
            let owner = owner_of(item);
            let title = without_owner(one_line(title), owner.as_deref());
            (!title.is_empty()).then(|| ActionItem {
                title: title.chars().take(200).collect(),
                owner,
                due: item
                    .get("due")
                    .and_then(Value::as_str)
                    .filter(|due| due.parse::<jiff::civil::Date>().is_ok())
                    .map(str::to_owned),
            })
        })
        .collect();
    let project = value
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    (items, project)
}

/// Summarize the Transcription `id`: write its Summary, create its
/// tasks, and keep the AI's project suggestion.
pub(crate) fn run(id: &str) -> SummaryStatus {
    match ai::providers() {
        Ok(providers) if providers.is_empty() => return SummaryStatus::NoProvider,
        Err(error) => return SummaryStatus::Failed(reason(&error)),
        Ok(_) => {}
    }
    let settings = Settings::read();
    let provider = settings.provider.clone().filter(|chosen| {
        ai::providers()
            .unwrap_or_default()
            .iter()
            .any(|provider| &provider.id == chosen)
    });
    let Ok(object) = objects::get(id) else {
        return SummaryStatus::Failed("The Transcription no longer exists.".to_owned());
    };
    let transcript = meeting::transcript_of(&object).map(|transcript| transcript.body);
    let lines = turns::lines(transcript.as_deref().unwrap_or_default());
    if lines.is_empty() {
        return SummaryStatus::Failed("The Transcript is empty.".to_owned());
    }
    let properties = meeting::properties(&object);
    let attendees: Vec<String> = properties
        .get("attendees")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let written_notes = notes::notes(&object.body);
    let today = meeting::now().date().to_string();
    let language = settings.summary_language();
    let instructions = match prompt::system(language.as_deref(), &today, &settings.prompt) {
        Ok(instructions) => instructions,
        Err(problem) => return SummaryStatus::Failed(problem.to_string()),
    };
    let meeting_prompt = prompt::user(&prompt::Meeting {
        title: &object.title,
        attendees: &attendees,
        notes: &written_notes,
        lines: &lines,
    });
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    let policy = length::policy_for_texts(&texts);
    let summary = match ai::complete(&CompletionRequest {
        provider: provider.clone(),
        instructions: Some(instructions),
        prompt: length::with_guidance(
            &meeting_prompt,
            policy.as_ref(),
            prompt::is_custom(&settings.prompt),
        ),
        json: false,
    }) {
        Ok(completion) => completion.text,
        Err(error) => {
            return match failed(&error) {
                (true, _) => SummaryStatus::NoProvider,
                (false, message) => SummaryStatus::Failed(message),
            };
        }
    };
    if let Err(error) = meeting::update(id, |object| Changes {
        body: Some(notes::with_summary(&object.body, &summary)),
        properties: "{}".to_owned(),
        remove: Vec::new(),
    }) {
        return SummaryStatus::Failed(format!(
            "Noura couldn't save the Summary: {}",
            reason(&error)
        ));
    }
    SummaryStatus::Done {
        tasks: create_tasks(
            &object,
            provider,
            &language,
            &meeting_prompt,
            &today,
            &attendees,
        )
        .err(),
    }
}

/// Ask for the action items as JSON and create each one through Tasks
/// that the Transcription doesn't have yet, assigned to its owner. An
/// owner who matches an attendee but for case takes the attendee's
/// spelling.
fn create_tasks(
    object: &objects::Object,
    provider: Option<String>,
    language: &Option<String>,
    meeting_prompt: &str,
    today: &str,
    attendees: &[String],
) -> Result<(), String> {
    let projects = meeting::projects();
    let mut listed = format!("Current date: {today}\n\n# Projects\n\n");
    for (_, title) in &projects {
        listed.push_str(&format!("- {title}\n"));
    }
    let answer = ai::complete(&CompletionRequest {
        provider,
        instructions: Some(
            ACTION_ITEMS.replace("{language}", &prompt::language_name(language.as_deref())),
        ),
        prompt: format!("{listed}\n{meeting_prompt}"),
        json: true,
    })
    .map_err(|error| failed(&error).1)?;
    let (items, suggested) = parse_answer(&answer.text);
    let project = meeting::project_of(object);
    if project.is_none()
        && let Some(found) = suggested.and_then(|title| {
            projects
                .iter()
                .find(|(_, known)| known.eq_ignore_ascii_case(&title))
                .cloned()
        })
    {
        state().suggestions.insert(object.id.clone(), found);
    }
    let have: Vec<String> = tasks_of(&object.id)?
        .into_iter()
        .map(|task| task.title.to_lowercase())
        .collect();
    for item in items {
        if have.contains(&item.title.to_lowercase()) {
            continue;
        }
        let mut properties = json!({ "source": object.id });
        if let Some(project) = &project {
            properties["project"] = json!(project);
        }
        if let Some(due) = &item.due {
            properties["due"] = json!(due);
        }
        if let Some(owner) = &item.owner {
            let owner = attendees
                .iter()
                .find(|attendee| same_name(attendee, owner))
                .unwrap_or(owner);
            properties["assignee"] = json!(owner);
        }
        command_calls::call(
            "tasks",
            "create",
            &json!({ "title": item.title, "properties": properties }).to_string(),
        )
        .map_err(|error| format!("Noura couldn't create the tasks: {}", reason(&error)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_holds_action_items_and_a_project() {
        let (items, project) = parse_answer(
            r#" {"action_items":[{"title":"Ada: send the\n notes","due":"2026-11-20"},
               {"title":"Book a room","due":"Friday"},{"title":"  "},{"due":null},"Call Grace"],
               "project":" Garden "} "#,
        );
        assert_eq!(
            items,
            [
                ActionItem {
                    title: "Ada: send the notes".into(),
                    owner: None,
                    due: Some("2026-11-20".into())
                },
                ActionItem {
                    title: "Book a room".into(),
                    owner: None,
                    due: None
                },
                ActionItem {
                    title: "Call Grace".into(),
                    owner: None,
                    due: None
                },
            ]
        );
        assert_eq!(project.as_deref(), Some("Garden"));
        assert_eq!(parse_answer(r#"[{"title":"One"}]"#).0.len(), 1);
        assert_eq!(parse_answer("not json"), (Vec::new(), None));

        // An owner goes with the item, and out of the title when a
        // provider still writes it there.
        let (items, _) = parse_answer(
            r#"{"action_items":[{"title":"Write the launch post","owner":" Alice "},
               {"title":"élodie: book the venue","owner":"Élodie"},
               {"title":"Bob:","owner":"Bob"},
               {"title":"Tidy up","owner":"null"},{"title":"Order chairs","owner":null}]}"#,
        );
        let pairs: Vec<(&str, Option<&str>)> = items
            .iter()
            .map(|item| (item.title.as_str(), item.owner.as_deref()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("Write the launch post", Some("Alice")),
                ("Book the venue", Some("Élodie")),
                ("Bob:", Some("Bob")),
                ("Tidy up", None),
                ("Order chairs", None),
            ]
        );
        assert_eq!(
            parse_answer(r#"{"action_items":[],"project":null}"#),
            (Vec::new(), None)
        );
    }
}
