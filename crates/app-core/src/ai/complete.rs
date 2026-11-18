//! One-off completions: a prompt sent to a provider for one answer, as a
//! Transcription's Summary or its list of action items, with no chat
//! written and nothing kept. Choosing the provider is your say-so, so
//! nothing asks before the prompt goes out.

use std::time::{Duration, Instant};

use local_core::{AiModelRef, AiStreamEvent, AiStreamInput};
use plugin_host::{AiCompletion, AiCompletionRequest, AiFailure};

use super::history::{BASE_INSTRUCTIONS, text};
use super::{Ai, failure_of, lock, uuid_v4};

/// How long a completion may take before Noura gives up on it.
pub(crate) const COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// What Noura adds to the instructions when the answer must be JSON.
const JSON_INSTRUCTIONS: &str =
    "Answer with one JSON value and nothing else: no prose and no code fence.";

impl Ai {
    /// Complete `request` with one answer, writing no chat. The provider
    /// is the one it names, or your first provider that is on.
    pub(super) fn complete(&self, request: AiCompletionRequest) -> Result<AiCompletion, AiFailure> {
        let store = self
            .store()
            .map_err(|error| failure("ai_settings_unreadable", &error.message))?;
        let providers = store.list_providers().map_err(|error| failure_of(&error))?;
        let provider = match &request.provider {
            Some(id) => providers
                .into_iter()
                .find(|provider| &provider.id == id && provider.enabled)
                .ok_or_else(|| {
                    failure(
                        "provider_not_found",
                        "The provider isn't set up or turned on anymore.",
                    )
                })?,
            None => providers
                .into_iter()
                .find(|provider| provider.enabled)
                .ok_or_else(|| failure("no_provider", "Add an AI provider in Settings first."))?,
        };
        let mut instructions = BASE_INSTRUCTIONS.to_owned();
        if let Some(more) = request
            .instructions
            .as_deref()
            .map(str::trim)
            .filter(|more| !more.is_empty())
        {
            instructions.push_str("\n\n");
            instructions.push_str(more);
        }
        if request.json {
            instructions.push_str("\n\n");
            instructions.push_str(JSON_INSTRUCTIONS);
        }
        let operation = uuid_v4();
        let input = AiStreamInput {
            operation_id: operation.clone(),
            model: AiModelRef {
                provider_id: provider.id.clone(),
                model: provider.model.clone(),
            },
            messages: vec![text("system", &instructions), text("user", &request.prompt)],
            tools: None,
        };
        let client = lock(&self.client).clone();
        let mut frames = {
            let _entered = self.runtime.enter();
            store
                .start_stream_with(input, client)
                .map_err(|error| failure_of(&error))?
                .receiver
        };
        let started = Instant::now();
        let mut answer = String::new();
        let ended = self.block_on(async {
            loop {
                let left = COMPLETION_TIMEOUT.saturating_sub(started.elapsed());
                let frame = match tokio::time::timeout(left, frames.recv()).await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        break Err(failure(
                            "provider_stream_incomplete",
                            "The provider stopped early. Try again.",
                        ));
                    }
                    Err(_) => {
                        break Err(failure(
                            "provider_timed_out",
                            "The provider took too long to answer. Try again.",
                        ));
                    }
                };
                match frame.event {
                    AiStreamEvent::TextDelta { text } => answer.push_str(&text),
                    AiStreamEvent::Completed { .. } => break Ok(()),
                    AiStreamEvent::Cancelled => {
                        break Err(failure("cancelled", "The request stopped."));
                    }
                    AiStreamEvent::Error { error } => break Err(failure_of(&error)),
                    AiStreamEvent::Started { .. } | AiStreamEvent::ToolCall { .. } => {}
                }
            }
        });
        if ended.is_err() {
            let _ = store.cancel_stream(&operation);
        }
        ended?;
        let text = if request.json {
            json_answer(&answer).ok_or_else(|| {
                failure(
                    "invalid_json",
                    "The provider's answer wasn't the JSON Noura asked for. Try again.",
                )
            })?
        } else {
            answer.trim().to_owned()
        };
        Ok(AiCompletion {
            provider: provider.id,
            model: provider.model,
            text,
        })
    }
}

/// The JSON value `answer` holds, written again without spaces, once any
/// code fence around it is gone. `None` when it holds none.
fn json_answer(answer: &str) -> Option<String> {
    let trimmed = answer.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .unwrap_or(trimmed);
    serde_json::from_str::<serde_json::Value>(unfenced.trim())
        .ok()
        .map(|value| value.to_string())
}

fn failure(code: &str, message: &str) -> AiFailure {
    AiFailure {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::json_answer;

    #[test]
    fn json_answers_may_come_in_a_code_fence() {
        assert_eq!(json_answer("[1, 2]").as_deref(), Some("[1,2]"));
        assert_eq!(
            json_answer("```json\n{\"a\": 1}\n```\n").as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(json_answer("Here you go: [1]"), None);
    }
}
