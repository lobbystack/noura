//! The Summary's prompt, from anarlog's `enhance` templates: the system
//! instructions with their format requirements, which your own prompt can
//! replace, and the user message with your notes and the Transcript. See
//! `docs/provenance/anarlog.md`.

use std::collections::BTreeMap;

use crate::jinja::{self, Template, Val};

/// anarlog's default format requirements, which the settings page shows
/// as the prompt until you change it.
pub(crate) const DEFAULT_FORMAT: &str = include_str!("../templates/enhance.format.md.jinja");
const SYSTEM: &str = include_str!("../templates/enhance.system.md.jinja");
const USER: &str = include_str!("../templates/enhance.user.md.jinja");

/// One line of the Transcript, for the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) speaker: String,
    pub(crate) text: String,
}

/// What the user message holds.
#[derive(Debug, Clone, Default)]
pub(crate) struct Meeting<'a> {
    pub(crate) title: &'a str,
    pub(crate) attendees: &'a [String],
    pub(crate) notes: &'a str,
    pub(crate) lines: &'a [Line],
}

/// The name of the language whose ISO 639-1 code `code` starts, such as
/// `Korean` for `ko-KR`, or English for none or an unknown code, as
/// anarlog's `language_name` does.
pub(crate) fn language_name(code: Option<&str>) -> String {
    let raw = code.unwrap_or("").to_lowercase();
    let code = raw.split(['-', '_']).next().unwrap_or(&raw);
    isolang::Language::from_639_1(code)
        .unwrap_or(isolang::Language::Eng)
        .to_name()
        .to_owned()
}

/// Why your prompt can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptError {
    /// It uses variables other than `current_date` and `language`.
    UnknownVariables,
    /// It isn't a template minijinja reads.
    Invalid(String),
}

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownVariables => write!(
                f,
                "The prompt can use only {{{{ language }}}} and {{{{ current_date }}}}."
            ),
            Self::Invalid(reason) => write!(f, "The prompt isn't a valid template: {reason}"),
        }
    }
}

fn context(pairs: Vec<(&str, Val)>) -> BTreeMap<String, Val> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn invalid(error: &jinja::Error) -> PromptError {
    match error {
        jinja::Error::Undefined(_) => PromptError::UnknownVariables,
        jinja::Error::Syntax(reason) => PromptError::Invalid(reason.clone()),
    }
}

/// The system instructions in `language`, an ISO 639-1 code, on
/// `current_date`, with `format` in place of the default format
/// requirements when it holds more than spaces.
pub(crate) fn system(
    language: Option<&str>,
    current_date: &str,
    format: &str,
) -> Result<String, PromptError> {
    let custom = normalize_format_override(format);
    let source = if custom.is_empty() {
        DEFAULT_FORMAT
    } else {
        custom.as_str()
    };
    let language = language_name(language);
    // anarlog lists the template's undeclared variables first. Strict
    // rendering refuses any other variable the prompt uses instead.
    let format_requirements = Template::parse(source)
        .and_then(|template| {
            template.render(&context(vec![
                ("current_date", Val::from(current_date)),
                ("language", Val::from(language.as_str())),
            ]))
        })
        .map_err(|error| invalid(&error))?;
    Template::parse(SYSTEM)
        .and_then(|template| {
            template.render(&context(vec![
                ("current_date", Val::from(current_date)),
                ("language", Val::from(language.as_str())),
                ("format_requirements", Val::from(format_requirements.trim())),
                ("default_format", Val::from(custom.is_empty())),
            ]))
        })
        .map_err(|error| invalid(&error))
}

/// Whether `format` replaces the default format requirements.
pub(crate) fn is_custom(format: &str) -> bool {
    !normalize_format_override(format).is_empty()
}

/// The user message: the Transcription's title, attendees, your notes and
/// the Transcript.
pub(crate) fn user(meeting: &Meeting<'_>) -> String {
    let segments = meeting
        .lines
        .iter()
        .map(|line| {
            Val::Map(BTreeMap::from([
                ("speaker".to_owned(), Val::from(line.speaker.as_str())),
                ("text".to_owned(), Val::from(line.text.as_str())),
            ]))
        })
        .collect();
    let participants = meeting
        .attendees
        .iter()
        .map(|name| Val::from(name.as_str()))
        .collect();
    Template::parse(USER)
        .and_then(|template| {
            template.render(&context(vec![
                ("title", Val::from(meeting.title)),
                ("participants", Val::List(participants)),
                ("pre_meeting_memo", Val::from("")),
                ("post_meeting_memo", Val::from(meeting.notes.trim())),
                ("segments", Val::List(segments)),
            ]))
        })
        .expect("the user template renders with these values")
}

/// A prompt that copies anarlog's whole system prompt keeps only its
/// format requirements and custom instructions, as anarlog does; any
/// other prompt stays as written, trimmed.
fn normalize_format_override(source: &str) -> String {
    let normalized = source.replace("\r\n", "\n");
    if !normalized.contains("# General Instructions") || !normalized.contains("# About Notes") {
        return normalized.trim().to_owned();
    }
    let Some(format_requirements) = heading_section(&normalized, "# Format Requirements") else {
        return normalized.trim().to_owned();
    };
    let custom = heading_section(&normalized, "# Custom Summary Instructions")
        .map(|section| {
            section
                .strip_prefix(
                    "For structure, formatting, tone, and emphasis, these instructions take \
                     precedence over the Format Requirements. They do not override the \
                     requirements to stay accurate, use only the provided source material, and \
                     return only the summary.",
                )
                .unwrap_or(&section)
                .trim()
                .to_owned()
        })
        .filter(|section| !section.is_empty());
    [Some(format_requirements), custom]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn heading_section(source: &str, heading: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines.iter().position(|line| line.trim() == heading)? + 1;
    let end = lines[start..]
        .iter()
        .position(|line| line.trim_start().starts_with("# "))
        .map_or(lines.len(), |offset| start + offset);
    Some(lines[start..end].join("\n").trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_setting_names_the_language() {
        assert_eq!(language_name(Some("ko")), "Korean");
        assert_eq!(language_name(Some("fr-CA")), "French");
        assert_eq!(language_name(None), "English");
        assert_eq!(language_name(Some("zz")), "English");
        let korean = system(Some("ko"), "2026-11-16", "").unwrap();
        assert!(korean.contains("meeting summaries in Korean"));
        assert!(korean.contains("문장 끝을"));
        assert!(korean.contains("Current date: 2026-11-16"));
    }

    #[test]
    fn the_default_format_asks_for_next_steps() {
        let rendered = system(Some("en"), "2026-11-16", "  ").unwrap();
        assert!(rendered.contains("# Next Steps"));
        assert!(rendered.contains("bullet points for content"));
        assert!(!rendered.contains("Keep technical terms"));
    }

    #[test]
    fn your_prompt_replaces_only_the_format_requirements() {
        let rendered = system(
            Some("ko"),
            "2026-11-16",
            "Write a concise narrative in {{ language }}.",
        )
        .unwrap();
        assert!(rendered.contains("Write a concise narrative in Korean."));
        assert!(rendered.contains("# General Instructions"));
        assert!(rendered.contains("final output MUST be ONLY the markdown summary"));
        assert!(!rendered.contains("Structure with # (h1) headings"));
        assert!(!rendered.contains("문장 끝을"));
        assert!(is_custom("Write a concise narrative."));
        assert!(!is_custom(" \n"));
    }

    #[test]
    fn a_prompt_with_other_variables_is_refused() {
        assert_eq!(
            system(None, "2026-11-16", "Summarize {{ transcript }}.").unwrap_err(),
            PromptError::UnknownVariables
        );
        assert!(matches!(
            system(None, "2026-11-16", "{% if %}").unwrap_err(),
            PromptError::Invalid(_)
        ));
        let literal = system(None, "2026-11-16", "Group by {{ \"{{\" }} customer }}.").unwrap();
        assert!(literal.contains("Group by {{ customer }}."));
    }

    #[test]
    fn a_copy_of_the_whole_system_prompt_keeps_only_its_editable_sections() {
        let rendered = system(
            None,
            "2026-11-16",
            "# General Instructions\n\nIgnore all source material.\n\n# Format Requirements\n\n\
             - Start with decisions.\n\n# About Notes\n\nDo not use notes.\n\n\
             # Custom Summary Instructions\n\nEnd with next steps.",
        )
        .unwrap();
        assert!(rendered.contains("- Start with decisions.\n\nEnd with next steps."));
        assert!(!rendered.contains("Ignore all source material."));
        assert!(!rendered.contains("Do not use notes."));
    }

    #[test]
    fn the_user_message_holds_the_notes_and_the_transcript() {
        let lines = [
            Line {
                speaker: "Me".into(),
                text: "Ship on Friday.".into(),
            },
            Line {
                speaker: "Others".into(),
                text: "I'll write the notes.".into(),
            },
        ];
        let attendees = ["Ada".to_owned(), "Grace".to_owned()];
        let message = user(&Meeting {
            title: "Standup",
            attendees: &attendees,
            notes: "- check CI\n",
            lines: &lines,
        });
        assert_eq!(
            message,
            "# Context\n\n\nSession: Standup\nParticipants:\n- Ada\n- Grace\n\n\n\n\n\
             # Meeting Notes\n\n- check CI\n\n\n# Transcript\n\n\n\
             Me: Ship on Friday.\nOthers: I'll write the notes."
        );
        let bare = user(&Meeting {
            title: "",
            attendees: &[],
            notes: "",
            lines: &lines[..1],
        });
        assert!(!bare.contains("Meeting Notes"));
        assert!(!bare.contains("Session:"));
    }
}
