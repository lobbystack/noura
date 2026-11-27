use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::{Stream, StreamExt};
use genai::{
    Client, ModelIden, ServiceTarget,
    adapter::AdapterKind,
    chat::{
        ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, ContentPart, MessageContent,
        ReasoningEffort, StopReason, Tool, ToolCall, ToolResponse,
    },
    resolver::{AuthData, Endpoint, ServiceTargetResolver},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use ts_rs::TS;

use crate::{CoreError, ErrorCategory, Result};
use crate::{CredentialStore, SystemCredentials};

pub mod failures;
mod genai_streamer;
mod operations;
mod providers;
mod validation;

use operations::*;
use providers::*;
use validation::*;

pub use genai_streamer::GenAiStreamer;
pub use providers::{AI_PROVIDER_KINDS, RETIRED_MODELS, replacement_model};

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderConfig {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub model: String,
    #[ts(optional = nullable)]
    pub endpoint: Option<String>,
    #[ts(optional = nullable)]
    pub credential_ref: Option<String>,
    pub enabled: bool,
    /// How many tokens the model reads at most, when you set it. Chats
    /// summarize their earlier messages to stay within it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub context_window: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiModelRef {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AiContentPart {
    Text {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        #[ts(type = "unknown")]
        arguments: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        name: Option<String>,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[ts(type = "unknown")]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiToolCall {
    pub call_id: String,
    pub name: String,
    #[ts(type = "unknown")]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiTransportMessage {
    pub role: String,
    pub content: Vec<AiContentPart>,
}

/// How hard the model reasons before it answers, as the provider's
/// request names it. Each provider maps these to its own parameter: an
/// Anthropic model's `output_config.effort` and thinking, OpenAI's
/// `reasoning_effort` and Gemini's thinking budget or level. `Zero` turns
/// reasoning off, and `Minimal` asks for the least a model that can't turn
/// it off allows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum AiReasoning {
    Zero,
    Minimal,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamInput {
    pub operation_id: String,
    /// The provider, and the model to ask: the provider's own, or another
    /// model of the same provider.
    pub model: AiModelRef,
    pub messages: Vec<AiTransportMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tools: Option<Vec<AiToolDefinition>>,
    /// How hard the model reasons, or none for the provider's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reasoning: Option<AiReasoning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiUsage {
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AiStopReason {
    Completed { raw: String },
    MaxTokens { raw: String },
    ToolCall { raw: String },
    ContentFilter { raw: String },
    StopSequence { raw: String },
    Other { raw: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamSummary {
    pub model: AiModelRef,
    pub usage: Option<AiUsage>,
    pub stop_reason: Option<AiStopReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AiStreamEvent {
    Started { model: AiModelRef },
    TextDelta { text: String },
    ToolCall { call: AiToolCall },
    Completed { summary: AiStreamSummary },
    Cancelled,
    Error { error: CoreError },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamFrame {
    pub operation_id: String,
    /// Serialized as a JSON number; a stream never approaches 2^53 frames.
    #[ts(type = "number")]
    pub sequence: u64,
    pub event: AiStreamEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiCancelOutcome {
    pub operation_id: String,
    pub cancelled: bool,
}

pub struct ResolvedAiStreamRequest {
    /// The provider, with the model the request asks in its `model`.
    pub provider: AiProviderConfig,
    pub credential: Option<String>,
    pub messages: Vec<AiTransportMessage>,
    pub tools: Vec<AiToolDefinition>,
    pub reasoning: Option<AiReasoning>,
}

pub type AiProviderEventStream = Pin<Box<dyn Stream<Item = Result<AiProviderStreamEvent>> + Send>>;

#[derive(Debug, Clone)]
pub enum AiProviderStreamEvent {
    TextDelta(String),
    Completed {
        usage: Option<AiUsage>,
        stop_reason: Option<AiStopReason>,
        tool_calls: Vec<AiToolCall>,
    },
}

pub trait AiStreamer: Send + Sync + 'static {
    fn stream(
        &self,
        request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>>;
}

pub struct AiStreamOperation {
    pub receiver: mpsc::Receiver<AiStreamFrame>,
}

pub struct AiFoundation {
    config_path: PathBuf,
    providers: Mutex<Vec<AiProviderConfig>>,
    credentials: Arc<dyn CredentialStore>,
    stream_gate: Mutex<()>,
    operations: Arc<AiOperationRegistry>,
}

impl AiFoundation {
    /// Open the provider settings at `config_path`, on this device, with
    /// keys in the operating system's credential store.
    pub fn open(config_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(config_path, Arc::new(SystemCredentials))
    }

    /// [`AiFoundation::open`] with keys in `credentials`.
    pub fn open_with(
        config_path: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self> {
        let config_path = config_path.as_ref().to_owned();
        let providers: Vec<AiProviderConfig> = if config_path.exists() {
            let bytes = std::fs::read(&config_path)
                .map_err(|error| CoreError::io(error, "ai_provider_load", config_path.to_str()))?;
            serde_json::from_slice(&bytes).map_err(|_| {
                CoreError::new(
                    "provider_config_invalid",
                    ErrorCategory::Parse,
                    "AI provider settings are invalid",
                    "ai_provider_load",
                )
            })?
        } else {
            Vec::new()
        };
        validate_provider_configs(&providers, "ai_provider_load")?;
        let mut providers = providers;
        let today = jiff::Zoned::now().date();
        if retire_models(&mut providers, today)
            && let Err(error) = write_json(&config_path, &providers)
        {
            // The move happens again at the next start; requests use the
            // replacement now.
            tracing::warn!(code = %error.code, "a retired model's replacement wasn't saved");
        }
        Ok(Self {
            config_path,
            providers: Mutex::new(providers),
            credentials,
            stream_gate: Mutex::new(()),
            operations: Arc::new(AiOperationRegistry {
                operations: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn list_providers(&self) -> Result<Vec<AiProviderConfig>> {
        Ok(self
            .providers
            .lock()
            .map_err(|_| provider_lock_error("ai_provider_list"))?
            .clone())
    }

    pub fn save_provider(&self, provider: AiProviderConfig) -> Result<()> {
        validate_provider(&provider, "ai_provider_save")?;
        ensure_secure_endpoint(&provider, "ai_provider_save")?;
        // A stream must use either the provider configuration it started
        // with or the replacement, never a snapshot raced with an update.
        let _gate = self
            .stream_gate
            .lock()
            .map_err(|_| operation_lock_error("ai_provider_save"))?;
        let mut values = self
            .providers
            .lock()
            .map_err(|_| provider_lock_error("ai_provider_save"))?;
        let mut next = values.clone();
        next.retain(|value| value.id != provider.id);
        next.push(provider);
        write_json(&self.config_path, &next)?;
        *values = next;
        Ok(())
    }

    /// Keep `secret` as the key of the provider `provider_id`, in the
    /// credential store. The settings keep only a new reference to it, and
    /// the key it replaces leaves the store.
    pub fn set_credential(&self, provider_id: &str, secret: &str) -> Result<String> {
        if secret.is_empty() {
            return Err(CoreError::validation(
                "credential_empty",
                "A credential value is required",
                "ai_credential_set",
            ));
        }
        let mut values = self
            .providers
            .lock()
            .map_err(|_| credential_error("ai_credential_set"))?;
        let position = values
            .iter()
            .position(|value| value.id == provider_id)
            .ok_or_else(|| {
                CoreError::validation(
                    "provider_not_found",
                    "The AI provider does not exist",
                    "ai_credential_set",
                )
            })?;
        let previous = values[position].credential_ref.clone();
        let reference = format!("credential_{}", uuid::Uuid::new_v4());
        self.credentials.set(&reference, secret)?;
        let mut next = values.clone();
        next[position].credential_ref = Some(reference.clone());
        if let Err(error) = write_json(&self.config_path, &next) {
            let _ = self.credentials.delete(&reference);
            return Err(error);
        }
        *values = next;
        if let Some(previous) = previous {
            let _ = self.credentials.delete(&previous);
        }
        Ok(reference)
    }

    /// Forget the key of the provider `provider_id`: its reference leaves
    /// the settings and the key leaves the credential store.
    pub fn clear_credential(&self, provider_id: &str) -> Result<()> {
        let mut values = self
            .providers
            .lock()
            .map_err(|_| credential_error("ai_credential_delete"))?;
        let Some(position) = values.iter().position(|value| value.id == provider_id) else {
            return Err(CoreError::validation(
                "provider_not_found",
                "The AI provider does not exist",
                "ai_credential_delete",
            ));
        };
        let Some(reference) = values[position].credential_ref.clone() else {
            return Ok(());
        };
        let mut next = values.clone();
        next[position].credential_ref = None;
        write_json(&self.config_path, &next)?;
        *values = next;
        self.credentials.delete(&reference)
    }

    pub fn delete_credential(&self, reference: &str) -> Result<()> {
        self.credentials.delete(reference)
    }

    /// Remove the provider `provider_id` from the settings, with its key.
    /// Its running requests stop.
    pub fn remove_provider(&self, provider_id: &str) -> Result<()> {
        let _gate = self
            .stream_gate
            .lock()
            .map_err(|_| operation_lock_error("ai_provider_remove"))?;
        let mut values = self
            .providers
            .lock()
            .map_err(|_| provider_lock_error("ai_provider_remove"))?;
        let Some(removed) = values.iter().find(|value| value.id == provider_id).cloned() else {
            return Ok(());
        };
        let next: Vec<AiProviderConfig> = values
            .iter()
            .filter(|value| value.id != provider_id)
            .cloned()
            .collect();
        write_json(&self.config_path, &next)?;
        *values = next;
        drop(values);
        self.operations.cancel_provider(provider_id)?;
        if let Some(reference) = removed.credential_ref {
            let _ = self.credentials.delete(&reference);
        }
        Ok(())
    }

    /// Ask the provider `input` names and stream its answer. Choosing the
    /// provider is your say-so: nothing asks before the request goes out.
    pub fn start_stream(&self, input: AiStreamInput) -> Result<AiStreamOperation> {
        self.start_stream_with(input, Arc::new(GenAiStreamer))
    }

    pub fn start_stream_with<S: AiStreamer + ?Sized>(
        &self,
        input: AiStreamInput,
        streamer: Arc<S>,
    ) -> Result<AiStreamOperation> {
        validate_operation_id(&input.operation_id, "ai_stream")?;
        validate_transport_messages(&input.messages, "ai_stream")?;
        validate_tools(input.tools.as_deref().unwrap_or_default(), "ai_stream")?;
        // Keep provider lookup and operation registration in the same
        // critical section as provider updates and removals, so a removed
        // provider can't race a request.
        let _gate = self
            .stream_gate
            .lock()
            .map_err(|_| operation_lock_error("ai_stream"))?;
        let mut provider = self.provider(&input.model.provider_id, "ai_stream")?;
        ensure_secure_endpoint(&provider, "ai_stream")?;
        // A request may ask another model of the provider, as an Orb that
        // names one does; the provider's key and address stay its own.
        if provider.model != input.model.model {
            validate_model(&input.model.model, "ai_stream")?;
            provider.model.clone_from(&input.model.model);
        }
        let credential = provider
            .credential_ref
            .as_ref()
            .map(|reference| self.credentials.get(reference))
            .transpose()?;
        let operation = self
            .operations
            .start(input.operation_id.clone(), provider.id.clone())?;
        let (sender, receiver) = mpsc::channel(STREAM_BUFFER_CAPACITY);
        let model = input.model;
        tokio::spawn(run_stream(
            operation,
            sender,
            model,
            ResolvedAiStreamRequest {
                provider,
                credential,
                messages: input.messages,
                tools: input.tools.unwrap_or_default(),
                reasoning: input.reasoning,
            },
            streamer,
        ));
        Ok(AiStreamOperation { receiver })
    }

    pub fn cancel_stream(&self, operation_id: &str) -> Result<AiCancelOutcome> {
        validate_operation_id(operation_id, "ai_stream_cancel")?;
        Ok(AiCancelOutcome {
            operation_id: operation_id.into(),
            cancelled: self.operations.cancel(operation_id)?,
        })
    }

    fn provider(&self, provider_id: &str, operation: &str) -> Result<AiProviderConfig> {
        self.configured_provider(provider_id)?
            .filter(|value| value.enabled)
            .ok_or_else(|| {
                CoreError::validation(
                    "provider_not_found",
                    "The enabled AI provider does not exist",
                    operation,
                )
            })
    }

    fn configured_provider(&self, provider_id: &str) -> Result<Option<AiProviderConfig>> {
        Ok(self
            .list_providers()?
            .into_iter()
            .find(|value| value.id == provider_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct MockStreamer;
    impl AiStreamer for MockStreamer {
        fn stream(
            &self,
            request: ResolvedAiStreamRequest,
        ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
            Box::pin(async move {
                assert!(request.credential.is_none());
                let stream: AiProviderEventStream = Box::pin(futures::stream::iter(vec![
                    Ok(AiProviderStreamEvent::TextDelta("mock ".into())),
                    Ok(AiProviderStreamEvent::TextDelta("stream".into())),
                    Ok(AiProviderStreamEvent::Completed {
                        usage: Some(AiUsage {
                            prompt_tokens: Some(3),
                            completion_tokens: Some(2),
                            total_tokens: Some(5),
                        }),
                        stop_reason: Some(AiStopReason::Completed { raw: "stop".into() }),
                        tool_calls: vec![AiToolCall {
                            call_id: "call_1".into(),
                            name: "read_workspace".into(),
                            arguments: serde_json::json!({"path": "notes/plan.md"}),
                        }],
                    }),
                ]));
                Ok(stream)
            })
        }
    }

    struct PendingMockStreamer;
    impl AiStreamer for PendingMockStreamer {
        fn stream(
            &self,
            request: ResolvedAiStreamRequest,
        ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
            Box::pin(async move {
                assert!(request.credential.is_none());
                let stream: AiProviderEventStream = Box::pin(futures::stream::pending());
                Ok(stream)
            })
        }
    }

    struct ClosedMockStreamer;
    impl AiStreamer for ClosedMockStreamer {
        fn stream(
            &self,
            _request: ResolvedAiStreamRequest,
        ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
            Box::pin(async {
                let stream: AiProviderEventStream = Box::pin(futures::stream::empty());
                Ok(stream)
            })
        }
    }

    fn enabled_provider() -> AiProviderConfig {
        AiProviderConfig {
            id: "test".into(),
            kind: "openai".into(),
            display_name: "Test".into(),
            model: "test-model".into(),
            endpoint: Some("https://example.test/v1/".into()),
            credential_ref: None,
            enabled: true,
            context_window: None,
        }
    }

    #[test]
    fn every_offered_provider_kind_is_one_genai_connects_to() {
        for (kind, name) in AI_PROVIDER_KINDS {
            assert!(AdapterKind::from_lower_str(kind).is_some(), "{kind}");
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn keys_go_to_the_credential_store_and_settings_keep_a_reference() {
        let root = tempdir().unwrap();
        let credentials = Arc::new(crate::MemoryCredentials::new());
        let ai = AiFoundation::open_with(root.path().join("providers.json"), credentials.clone())
            .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        let first = ai.set_credential("test", "sk-first-secret").unwrap();
        let second = ai.set_credential("test", "sk-second-secret").unwrap();
        assert_ne!(first, second);
        assert_eq!(credentials.secrets(), vec!["sk-second-secret".to_owned()]);
        let settings = std::fs::read_to_string(root.path().join("providers.json")).unwrap();
        assert!(settings.contains(&second));
        assert!(!settings.contains("secret"));

        ai.clear_credential("test").unwrap();
        assert!(credentials.secrets().is_empty());
        assert_eq!(ai.list_providers().unwrap()[0].credential_ref, None);

        ai.set_credential("test", "sk-third-secret").unwrap();
        ai.remove_provider("test").unwrap();
        assert!(credentials.secrets().is_empty());
        assert!(ai.list_providers().unwrap().is_empty());
    }

    #[test]
    fn plain_http_endpoints_are_limited_to_this_computer() {
        for (endpoint, allowed) in [
            ("https://api.example.com/v1/", true),
            ("http://localhost:11434/v1/", true),
            ("http://LOCALHOST:1234/", true),
            ("http://127.0.0.1:1234/", true),
            ("http://127.8.0.1/", true),
            ("http://[::1]:8080/", true),
            ("http://api.example.com/v1/", false),
            ("http://192.168.1.20:11434/", false),
            ("http://localhost.example.com/", false),
            ("ftp://localhost/", false),
            ("file:///tmp/model", false),
        ] {
            let mut provider = enabled_provider();
            provider.endpoint = Some(endpoint.into());
            assert_eq!(
                validate_provider(&provider, "ai_provider_save")
                    .and_then(|()| ensure_secure_endpoint(&provider, "ai_provider_save"))
                    .is_ok(),
                allowed,
                "{endpoint}"
            );
        }
    }

    fn stream_input() -> AiStreamInput {
        AiStreamInput {
            operation_id: uuid::Uuid::new_v4().to_string(),
            model: AiModelRef {
                provider_id: "test".into(),
                model: "test-model".into(),
            },
            messages: vec![AiTransportMessage {
                role: "user".into(),
                content: vec![AiContentPart::Text {
                    text: "Hello".into(),
                }],
            }],
            tools: Some(vec![AiToolDefinition {
                name: "read_workspace".into(),
                description: Some("Read a workspace file".into()),
                input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }]),
            reasoning: None,
        }
    }

    #[tokio::test]
    async fn duplicate_operation_ids_preserve_cancellation_and_cleanup_allows_reuse() {
        let registry = Arc::new(AiOperationRegistry {
            operations: Mutex::new(HashMap::new()),
        });
        let provider = "test".to_owned();
        let mut original = registry
            .start("operation".into(), provider.clone())
            .unwrap();

        let Err(error) = registry.start("operation".into(), provider.clone()) else {
            panic!("a duplicate operation should be rejected");
        };
        assert_eq!(error.code, "ai_operation_in_progress");

        assert!(registry.cancel("operation").unwrap());
        assert!(original.cancellation.changed().await.is_ok());

        drop(original);
        let replacement = registry.start("operation".into(), provider).unwrap();
        drop(replacement);
    }

    #[tokio::test]
    async fn stream_frames_are_ordered_and_include_a_terminal_summary() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        let mut operation = ai
            .start_stream_with(stream_input(), Arc::new(MockStreamer))
            .unwrap();
        let mut frames = Vec::new();
        while let Some(frame) = operation.receiver.recv().await {
            let terminal = matches!(frame.event, AiStreamEvent::Completed { .. });
            frames.push(frame);
            if terminal {
                break;
            }
        }
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert!(matches!(frames[3].event, AiStreamEvent::ToolCall { .. }));
        assert!(matches!(
            frames.last().unwrap().event,
            AiStreamEvent::Completed { .. }
        ));
    }

    #[tokio::test]
    async fn cancellation_stops_an_active_stream() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        let input = stream_input();
        let operation_id = input.operation_id.clone();
        let mut operation = ai
            .start_stream_with(input, Arc::new(PendingMockStreamer))
            .unwrap();
        let first = operation.receiver.recv().await.unwrap();
        assert!(matches!(first.event, AiStreamEvent::Started { .. }));
        assert!(ai.cancel_stream(&operation_id).unwrap().cancelled);
        let cancelled = loop {
            let frame = operation.receiver.recv().await.unwrap();
            if matches!(frame.event, AiStreamEvent::Cancelled) {
                break true;
            }
        };
        assert!(cancelled);
    }

    #[tokio::test]
    async fn a_provider_stream_must_emit_an_explicit_terminal_event() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(enabled_provider()).unwrap();

        let mut operation = ai
            .start_stream_with(stream_input(), Arc::new(ClosedMockStreamer))
            .unwrap();
        assert!(matches!(
            operation.receiver.recv().await.unwrap().event,
            AiStreamEvent::Started { .. }
        ));
        let frame = operation.receiver.recv().await.unwrap();
        let AiStreamEvent::Error { error } = frame.event else {
            panic!("a closed provider stream must fail rather than complete");
        };
        assert_eq!(error.code, "provider_stream_incomplete");
        assert!(error.retryable);
    }

    #[test]
    fn opening_the_settings_moves_a_retired_model_to_its_replacement_on_disk() {
        let root = tempdir().unwrap();
        let path = root.path().join("providers.json");
        let saved = vec![AiProviderConfig {
            id: "deepseek".into(),
            kind: "deepseek".into(),
            display_name: "DeepSeek".into(),
            model: "deepseek-chat".into(),
            endpoint: None,
            credential_ref: None,
            enabled: true,
            context_window: None,
        }];
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let ai = AiFoundation::open(&path).unwrap();
        assert_eq!(ai.list_providers().unwrap()[0].model, "deepseek-flash");
        let on_disk: Vec<AiProviderConfig> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk[0].model, "deepseek-flash");
    }

    #[test]
    fn unknown_provider_kind_is_rejected_instead_of_falling_back() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        let mut provider = enabled_provider();
        provider.kind = "not-a-provider".into();
        let error = ai.save_provider(provider).unwrap_err();
        assert_eq!(error.code, "provider_kind_invalid");
        assert!(ai.list_providers().unwrap().is_empty());
    }

    #[test]
    fn provider_settings_reopen_durably_and_reject_duplicate_ids() {
        let root = tempdir().unwrap();
        let providers_path = root.path().join("providers.json");
        let ai = AiFoundation::open(&providers_path).unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        drop(ai);

        let reopened = AiFoundation::open(&providers_path).unwrap();
        assert_eq!(reopened.list_providers().unwrap(), vec![enabled_provider()]);
        drop(reopened);

        let duplicate = vec![enabled_provider(), enabled_provider()];
        std::fs::write(&providers_path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
        let Err(error) = AiFoundation::open(&providers_path) else {
            panic!("duplicate provider IDs must be rejected");
        };
        assert_eq!(error.code, "provider_config_invalid");
    }

    #[test]
    fn stream_rejects_invalid_tool_definitions_and_history_roles() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(enabled_provider()).unwrap();

        let mut invalid_tool = stream_input();
        invalid_tool.tools.as_mut().unwrap()[0].input_schema = serde_json::json!("not a schema");
        let Err(error) = ai.start_stream_with(invalid_tool, Arc::new(MockStreamer)) else {
            panic!("a non-schema tool definition must be rejected");
        };
        assert_eq!(error.code, "tool_definition_invalid");

        let mut boolean_schema = stream_input();
        boolean_schema.tools.as_mut().unwrap()[0].input_schema = serde_json::json!(true);
        assert!(validate_tools(boolean_schema.tools.as_deref().unwrap(), "ai_test").is_ok());
        boolean_schema.tools.as_mut().unwrap()[0].input_schema = serde_json::json!(false);
        assert!(validate_tools(boolean_schema.tools.as_deref().unwrap(), "ai_test").is_ok());

        let mut invalid_history = stream_input();
        invalid_history.messages[0].role = "tool".into();
        let Err(error) = ai.start_stream_with(invalid_history, Arc::new(MockStreamer)) else {
            panic!("tool history must contain tool results");
        };
        assert_eq!(error.code, "message_content_invalid");
    }

    #[tokio::test]
    async fn a_stream_goes_out_without_asking_and_removing_its_provider_stops_it() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        let input = stream_input();
        let mut operation = ai
            .start_stream_with(input.clone(), Arc::new(PendingMockStreamer))
            .unwrap();
        assert!(matches!(
            operation.receiver.recv().await.unwrap().event,
            AiStreamEvent::Started { .. }
        ));
        let Err(error) = ai.start_stream_with(input, Arc::new(PendingMockStreamer)) else {
            panic!("a duplicate operation should be rejected");
        };
        assert_eq!(error.code, "ai_operation_in_progress");

        ai.remove_provider("test").unwrap();

        assert!(matches!(
            operation.receiver.recv().await.unwrap().event,
            AiStreamEvent::Cancelled
        ));
        let Err(error) = ai.start_stream_with(stream_input(), Arc::new(MockStreamer)) else {
            panic!("a stream to a removed provider should be rejected");
        };
        assert_eq!(error.code, "provider_not_found");
    }

    /// Keeps the model and the reasoning of each request it gets.
    #[derive(Default)]
    struct Recording(Mutex<Vec<(String, Option<AiReasoning>)>>);

    impl AiStreamer for Recording {
        fn stream(
            &self,
            request: ResolvedAiStreamRequest,
        ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
            self.0
                .lock()
                .unwrap()
                .push((request.provider.model.clone(), request.reasoning));
            Box::pin(async {
                let stream: AiProviderEventStream = Box::pin(futures::stream::iter(vec![Ok(
                    AiProviderStreamEvent::Completed {
                        usage: None,
                        stop_reason: None,
                        tool_calls: Vec::new(),
                    },
                )]));
                Ok(stream)
            })
        }
    }

    #[tokio::test]
    async fn a_request_may_ask_another_model_of_its_provider_and_how_hard_it_reasons() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        let recording = Arc::new(Recording::default());
        let mut input = stream_input();
        input.model.model = "other-model".into();
        input.reasoning = Some(AiReasoning::High);
        let mut operation = ai.start_stream_with(input, recording.clone()).unwrap();
        while let Some(frame) = operation.receiver.recv().await {
            if matches!(frame.event, AiStreamEvent::Completed { .. }) {
                break;
            }
        }
        assert_eq!(
            recording.0.lock().unwrap().as_slice(),
            [("other-model".to_owned(), Some(AiReasoning::High))]
        );
        // The provider keeps its own model.
        assert_eq!(ai.list_providers().unwrap()[0].model, "test-model");

        let mut input = stream_input();
        input.model.model = "bad\nmodel".into();
        let Err(error) = ai.start_stream_with(input, recording) else {
            panic!("a model with a line break should be rejected");
        };
        assert_eq!(error.code, "model_invalid");
    }

    #[test]
    fn each_reasoning_reaches_genai_as_its_effort() {
        use genai_streamer::reasoning_effort;
        let keywords: Vec<String> = [
            AiReasoning::Zero,
            AiReasoning::Minimal,
            AiReasoning::Low,
            AiReasoning::Medium,
            AiReasoning::High,
        ]
        .into_iter()
        .map(|reasoning| reasoning_effort(reasoning).to_string())
        .collect();
        assert_eq!(keywords, ["zero", "minimal", "low", "medium", "high"]);
    }

    #[test]
    fn export_bindings() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let bindings = concat!(env!("CARGO_MANIFEST_DIR"), "/bindings");
        AiModelRef::export_all_to(bindings)?;
        AiTransportMessage::export_all_to(bindings)?;
        AiContentPart::export_all_to(bindings)?;
        AiToolDefinition::export_all_to(bindings)?;
        AiToolCall::export_all_to(bindings)?;
        AiStreamInput::export_all_to(bindings)?;
        AiReasoning::export_all_to(bindings)?;
        AiStreamFrame::export_all_to(bindings)?;
        AiStreamEvent::export_all_to(bindings)?;
        AiStreamSummary::export_all_to(bindings)?;
        AiCancelOutcome::export_all_to(bindings)?;
        AiUsage::export_all_to(bindings)?;
        AiStopReason::export_all_to(bindings)?;
        Ok(())
    }
}
