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
        StopReason, Tool, ToolCall, ToolResponse,
    },
    resolver::{AuthData, Endpoint, ServiceTargetResolver},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use ts_rs::TS;

use crate::{
    AiConsentDataCategory, AiConsentGrant, AiConsentGrantInput, AiConsentReadInput,
    AiConsentRevokeInput, AiConsentRevokeOutcome,
    consent::{AiConsentKey, AiConsentStore, consent_key},
};
use crate::{CoreError, ErrorCategory, Result};

const STREAM_BUFFER_CAPACITY: usize = 32;
const STREAM_BACKPRESSURE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderConfig {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub credential_ref: Option<String>,
    pub enabled: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamInput {
    pub operation_id: String,
    pub model: AiModelRef,
    pub messages: Vec<AiTransportMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tools: Option<Vec<AiToolDefinition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub policy_version: Option<String>,
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
    pub provider: AiProviderConfig,
    pub credential: Option<String>,
    pub messages: Vec<AiTransportMessage>,
    pub tools: Vec<AiToolDefinition>,
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

pub struct GenAiStreamer;
impl AiStreamer for GenAiStreamer {
    fn stream(
        &self,
        request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
        Box::pin(async move {
            let client = provider_client(
                &request.provider,
                request.credential.as_deref(),
                "ai_stream",
            )?;
            let messages = request
                .messages
                .into_iter()
                .map(|message| message_from_parts(message, "ai_stream"))
                .collect::<Result<Vec<_>>>()?;
            let tools: Vec<Tool> = request
                .tools
                .into_iter()
                .map(tool_from_definition)
                .collect();
            let options = ChatOptions::default()
                .with_capture_usage(true)
                .with_capture_tool_calls(true);
            let response = client
                .exec_chat_stream(
                    request.provider.model,
                    ChatRequest::new(messages).with_tools(tools),
                    Some(&options),
                )
                .await
                .map_err(|_| provider_error("ai_stream"))?;
            let stream = response.stream.filter_map(|event| async move {
                match event {
                    Ok(ChatStreamEvent::Chunk(chunk)) => {
                        Some(Ok(AiProviderStreamEvent::TextDelta(chunk.content)))
                    }
                    Ok(ChatStreamEvent::End(end)) => {
                        let tool_calls = end
                            .captured_tool_calls()
                            .unwrap_or_default()
                            .into_iter()
                            .map(tool_call_from_genai)
                            .collect();
                        Some(Ok(AiProviderStreamEvent::Completed {
                            usage: end.captured_usage.map(|usage| AiUsage {
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: usage.total_tokens,
                            }),
                            stop_reason: end.captured_stop_reason.map(stop_reason),
                            tool_calls,
                        }))
                    }
                    Ok(_) => None,
                    Err(_) => Some(Err(provider_error("ai_stream"))),
                }
            });
            let stream: AiProviderEventStream = Box::pin(stream);
            Ok(stream)
        })
    }
}

struct AiOperationRegistry {
    operations: Mutex<HashMap<String, ActiveAiOperation>>,
}

struct ActiveAiOperation {
    cancellation: watch::Sender<()>,
    consent_key: AiConsentKey,
}

impl AiOperationRegistry {
    fn start(
        self: &Arc<Self>,
        operation_id: String,
        consent_key: AiConsentKey,
    ) -> Result<AiOperation> {
        let (cancellation, receiver) = watch::channel(());
        let mut operations = self
            .operations
            .lock()
            .map_err(|_| operation_lock_error("ai_stream"))?;
        match operations.entry(operation_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(ActiveAiOperation {
                    cancellation,
                    consent_key,
                });
            }
            Entry::Occupied(_) => {
                return Err(CoreError::validation(
                    "ai_operation_in_progress",
                    "An AI operation with this identifier is already running",
                    "ai_stream",
                ));
            }
        }
        Ok(AiOperation {
            registry: self.clone(),
            operation_id,
            cancellation: receiver,
        })
    }

    fn cancel(&self, operation_id: &str) -> Result<bool> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| operation_lock_error("ai_stream_cancel"))?;
        Ok(operations
            .get(operation_id)
            .is_some_and(|operation| operation.cancellation.send(()).is_ok()))
    }

    fn cancel_matching(&self, consent_key: &AiConsentKey) -> Result<u32> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| operation_lock_error("ai_consent_revoke"))?;
        Ok(operations
            .values()
            .filter(|operation| &operation.consent_key == consent_key)
            .filter(|operation| operation.cancellation.send(()).is_ok())
            .count() as u32)
    }
}

struct AiOperation {
    registry: Arc<AiOperationRegistry>,
    operation_id: String,
    cancellation: watch::Receiver<()>,
}

impl Drop for AiOperation {
    fn drop(&mut self) {
        if let Ok(mut operations) = self.registry.operations.lock() {
            operations.remove(&self.operation_id);
        }
    }
}

pub struct AiStreamOperation {
    pub receiver: mpsc::Receiver<AiStreamFrame>,
}

pub struct AiFoundation {
    config_path: PathBuf,
    providers: Mutex<Vec<AiProviderConfig>>,
    consents: AiConsentStore,
    stream_gate: Mutex<()>,
    operations: Arc<AiOperationRegistry>,
}

impl AiFoundation {
    pub fn open(config_path: impl AsRef<Path>, consent_path: impl AsRef<Path>) -> Result<Self> {
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
        Ok(Self {
            config_path,
            providers: Mutex::new(providers),
            consents: AiConsentStore::open(consent_path)?,
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
        // A stream must use either the provider configuration that consented to
        // it or the replacement configuration, never a snapshot raced with a
        // provider update. The same gate also protects consent revocation.
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
        let entry = keyring::Entry::new("org.noura.ai", &reference)
            .map_err(|_| credential_error("ai_credential_set"))?;
        entry
            .set_password(secret)
            .map_err(|_| credential_error("ai_credential_set"))?;
        let mut next = values.clone();
        next[position].credential_ref = Some(reference.clone());
        if let Err(error) = write_json(&self.config_path, &next) {
            let _ = entry.delete_credential();
            return Err(error);
        }
        *values = next;
        if let Some(previous) = previous {
            let _ = keyring::Entry::new("org.noura.ai", &previous)
                .and_then(|old| old.delete_credential());
        }
        Ok(reference)
    }

    pub fn delete_credential(&self, reference: &str) -> Result<()> {
        let entry = keyring::Entry::new("org.noura.ai", reference)
            .map_err(|_| credential_error("ai_credential_delete"))?;
        entry
            .delete_credential()
            .map_err(|_| credential_error("ai_credential_delete"))
    }

    pub fn read_consent(
        &self,
        workspace_id: &str,
        input: AiConsentReadInput,
    ) -> Result<Option<AiConsentGrant>> {
        let Some(provider) = self.configured_provider(&input.provider_id)? else {
            return Ok(None);
        };
        self.consents.read(&consent_key(
            workspace_id,
            input.provider_id,
            input.policy_version,
            provider_fingerprint(&provider),
        ))
    }

    pub fn grant_consent(
        &self,
        workspace_id: &str,
        input: AiConsentGrantInput,
    ) -> Result<AiConsentGrant> {
        let _gate = self
            .stream_gate
            .lock()
            .map_err(|_| operation_lock_error("ai_consent_grant"))?;
        let provider = self.provider(&input.provider_id, "ai_consent_grant")?;
        let key = consent_key(
            workspace_id,
            input.provider_id.clone(),
            input.policy_version.clone(),
            provider_fingerprint(&provider),
        );
        self.consents.grant(key, input)
    }

    pub fn revoke_consent(
        &self,
        workspace_id: &str,
        input: AiConsentRevokeInput,
    ) -> Result<AiConsentRevokeOutcome> {
        let _gate = self
            .stream_gate
            .lock()
            .map_err(|_| operation_lock_error("ai_consent_revoke"))?;
        let keys = self.consents.revoke_matching(
            workspace_id,
            &input.provider_id,
            &input.policy_version,
        )?;
        let revoked = !keys.is_empty();
        let cancelled_operations = keys
            .iter()
            .map(|key| self.operations.cancel_matching(key))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .sum();
        Ok(AiConsentRevokeOutcome {
            provider_id: input.provider_id,
            policy_version: input.policy_version,
            revoked,
            cancelled_operations,
        })
    }

    pub fn start_stream(
        &self,
        workspace_id: &str,
        input: AiStreamInput,
    ) -> Result<AiStreamOperation> {
        self.start_stream_with(workspace_id, input, Arc::new(GenAiStreamer))
    }

    pub fn start_stream_with<S: AiStreamer>(
        &self,
        workspace_id: &str,
        input: AiStreamInput,
        streamer: Arc<S>,
    ) -> Result<AiStreamOperation> {
        validate_operation_id(&input.operation_id, "ai_stream")?;
        validate_transport_messages(&input.messages, "ai_stream")?;
        validate_tools(input.tools.as_deref().unwrap_or_default(), "ai_stream")?;
        let policy_version = input.policy_version.as_deref().ok_or_else(|| {
            CoreError::validation(
                "ai_consent_required",
                "Explicit consent is required before sending workspace content to an AI provider",
                "ai_stream",
            )
        })?;
        // Keep provider lookup, consent verification, and operation registration
        // in the same critical section as provider updates and revocations.
        // Otherwise a replacement endpoint could race this request after its
        // consent fingerprint had been selected.
        let _gate = self
            .stream_gate
            .lock()
            .map_err(|_| operation_lock_error("ai_stream"))?;
        let provider = self.provider(&input.model.provider_id, "ai_stream")?;
        if provider.model != input.model.model {
            return Err(CoreError::validation(
                "model_not_found",
                "The selected model is not configured for this provider",
                "ai_stream",
            ));
        }
        let consent_key = consent_key(
            workspace_id,
            input.model.provider_id.clone(),
            policy_version.into(),
            provider_fingerprint(&provider),
        );
        let consent = self.consents.read(&consent_key)?;
        if !matches!(
            consent,
            Some(AiConsentGrant {
                data_category: AiConsentDataCategory::WorkspaceContent,
                ..
            })
        ) {
            return Err(CoreError::new(
                "ai_consent_required",
                ErrorCategory::Permission,
                "Explicit consent is required before sending workspace content to an AI provider",
                "ai_stream",
            ));
        }
        let credential = credential_for(&provider, "ai_stream")?;
        let operation = self
            .operations
            .start(input.operation_id.clone(), consent_key)?;
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

async fn run_stream<S: AiStreamer>(
    mut operation: AiOperation,
    sender: mpsc::Sender<AiStreamFrame>,
    model: AiModelRef,
    request: ResolvedAiStreamRequest,
    streamer: Arc<S>,
) {
    let mut sequence = 1;
    if !send_stream_frame(
        &sender,
        &mut operation.cancellation,
        &operation.operation_id,
        &mut sequence,
        AiStreamEvent::Started {
            model: model.clone(),
        },
    )
    .await
    {
        let _ = send_stream_frame(
            &sender,
            &mut operation.cancellation,
            &operation.operation_id,
            &mut sequence,
            AiStreamEvent::Cancelled,
        )
        .await;
        return;
    }
    let stream_result = tokio::select! {
        changed = operation.cancellation.changed() => {
            if changed.is_ok() {
                let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Cancelled).await;
            }
            return;
        }
        result = streamer.stream(request) => result,
    };
    let mut stream = match stream_result {
        Ok(stream) => stream,
        Err(error) => {
            let _ = send_stream_frame(
                &sender,
                &mut operation.cancellation,
                &operation.operation_id,
                &mut sequence,
                AiStreamEvent::Error { error },
            )
            .await;
            return;
        }
    };
    loop {
        tokio::select! {
            changed = operation.cancellation.changed() => {
                if changed.is_ok() {
                    let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Cancelled).await;
                }
                return;
            }
            event = stream.next() => match event {
                Some(Ok(AiProviderStreamEvent::TextDelta(text))) => {
                    if !send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::TextDelta { text }).await {
                        return;
                    }
                }
                Some(Ok(AiProviderStreamEvent::Completed { usage, stop_reason, tool_calls })) => {
                    for call in tool_calls {
                        if !send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::ToolCall { call }).await {
                            return;
                        }
                    }
                    let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Completed {
                        summary: AiStreamSummary { model, usage, stop_reason },
                    }).await;
                    return;
                }
                Some(Err(error)) => {
                    let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Error { error }).await;
                    return;
                }
                None => {
                    let _ = send_stream_frame(
                        &sender,
                        &mut operation.cancellation,
                        &operation.operation_id,
                        &mut sequence,
                        AiStreamEvent::Error {
                            error: incomplete_stream_error(),
                        },
                    )
                    .await;
                    return;
                }
            }
        }
    }
}

async fn send_stream_frame(
    sender: &mpsc::Sender<AiStreamFrame>,
    cancellation: &mut watch::Receiver<()>,
    operation_id: &str,
    sequence: &mut u64,
    event: AiStreamEvent,
) -> bool {
    let frame = AiStreamFrame {
        operation_id: operation_id.into(),
        sequence: *sequence,
        event,
    };
    let sent = if matches!(&frame.event, AiStreamEvent::Cancelled) {
        matches!(
            tokio::time::timeout(STREAM_BACKPRESSURE_TIMEOUT, sender.send(frame)).await,
            Ok(Ok(()))
        )
    } else {
        tokio::select! {
            changed = cancellation.changed() => changed.is_err(),
            result = tokio::time::timeout(STREAM_BACKPRESSURE_TIMEOUT, sender.send(frame)) => matches!(result, Ok(Ok(()))),
        }
    };
    if sent {
        *sequence += 1;
    }
    sent
}

fn provider_client(
    provider: &AiProviderConfig,
    credential: Option<&str>,
    operation: &str,
) -> Result<Client> {
    let adapter = AdapterKind::from_lower_str(&provider.kind).ok_or_else(|| {
        CoreError::validation(
            "provider_kind_invalid",
            "The AI provider kind is not supported",
            operation,
        )
    })?;
    let endpoint = provider.endpoint.clone();
    let credential = credential.map(str::to_owned);
    let resolver = ServiceTargetResolver::from_resolver_fn(move |target: ServiceTarget| {
        let model = ModelIden::new(adapter, target.model.model_name);
        let endpoint = endpoint
            .as_ref()
            .map_or(target.endpoint, |value| Endpoint::from_owned(value.clone()));
        let auth = credential
            .as_ref()
            .map_or(AuthData::None, |value| AuthData::from_single(value.clone()));
        Ok(ServiceTarget {
            endpoint,
            auth,
            model,
        })
    });
    Ok(Client::builder()
        .with_service_target_resolver(resolver)
        .build())
}

fn message_from_parts(message: AiTransportMessage, operation: &str) -> Result<ChatMessage> {
    let parts = message
        .content
        .into_iter()
        .map(|part| match part {
            AiContentPart::Text { text } => ContentPart::Text(text),
            AiContentPart::ToolCall {
                call_id,
                name,
                arguments,
            } => ContentPart::ToolCall(ToolCall {
                call_id,
                fn_name: name,
                fn_arguments: arguments,
                thought_signatures: None,
            }),
            AiContentPart::ToolResult {
                call_id,
                name,
                content,
            } => ContentPart::ToolResponse(ToolResponse {
                call_id,
                fn_name: name,
                content,
            }),
        })
        .collect::<Vec<_>>();
    let content = MessageContent::from_parts(parts);
    match message.role.as_str() {
        "system" => Ok(ChatMessage::system(content)),
        "assistant" => Ok(ChatMessage::assistant(content)),
        "user" => Ok(ChatMessage::user(content)),
        "tool" => Ok(ChatMessage::tool(content)),
        _ => Err(CoreError::validation(
            "message_role_invalid",
            "The AI message role is not supported",
            operation,
        )),
    }
}

fn validate_transport_messages(messages: &[AiTransportMessage], operation: &str) -> Result<()> {
    if messages.is_empty() {
        return Err(CoreError::validation(
            "messages_empty",
            "At least one AI message is required",
            operation,
        ));
    }
    for message in messages {
        if !matches!(
            message.role.as_str(),
            "system" | "user" | "assistant" | "tool"
        ) {
            return Err(CoreError::validation(
                "message_role_invalid",
                "The AI message role is not supported",
                operation,
            ));
        }
        if message.content.is_empty() {
            return Err(CoreError::validation(
                "message_content_empty",
                "AI messages require content",
                operation,
            ));
        }
        for part in &message.content {
            match part {
                AiContentPart::Text { text } if !text.is_empty() => {
                    if message.role == "tool" {
                        return Err(invalid_message_content(operation));
                    }
                }
                AiContentPart::ToolCall { call_id, name, .. } if message.role == "assistant" => {
                    validate_tool_identifier(call_id, "tool call IDs", operation)?;
                    validate_tool_identifier(name, "tool names", operation)?;
                }
                AiContentPart::ToolResult {
                    call_id,
                    name,
                    content,
                } if message.role == "tool" && !content.is_empty() => {
                    validate_tool_identifier(call_id, "tool call IDs", operation)?;
                    if let Some(name) = name {
                        validate_tool_identifier(name, "tool names", operation)?;
                    }
                }
                _ => return Err(invalid_message_content(operation)),
            }
        }
    }
    Ok(())
}

fn tool_from_definition(tool: AiToolDefinition) -> Tool {
    Tool {
        name: tool.name.into(),
        description: tool.description,
        schema: Some(tool.input_schema),
        strict: None,
        config: None,
    }
}

fn tool_call_from_genai(call: &ToolCall) -> AiToolCall {
    AiToolCall {
        call_id: call.call_id.clone(),
        name: call.fn_name.clone(),
        arguments: call.fn_arguments.clone(),
    }
}

fn validate_tools(tools: &[AiToolDefinition], operation: &str) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        validate_tool_identifier(&tool.name, "tool names", operation)?;
        if !names.insert(&tool.name)
            || !(tool.input_schema.is_object() || tool.input_schema.is_boolean())
        {
            return Err(CoreError::validation(
                "tool_definition_invalid",
                "AI tool definitions must have unique names and a JSON Schema input schema",
                operation,
            ));
        }
        if let Some(description) = &tool.description
            && (description.is_empty()
                || description.len() > 8_192
                || description.chars().any(char::is_control))
        {
            return Err(CoreError::validation(
                "tool_definition_invalid",
                "AI tool descriptions must be plain text",
                operation,
            ));
        }
    }
    Ok(())
}

fn validate_tool_identifier(value: &str, field: &str, operation: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "tool_content_invalid",
            format!("AI {field} must be non-empty plain text"),
            operation,
        ));
    }
    Ok(())
}

fn invalid_message_content(operation: &str) -> CoreError {
    CoreError::validation(
        "message_content_invalid",
        "AI message content does not match its role",
        operation,
    )
}

fn provider_fingerprint(provider: &AiProviderConfig) -> String {
    let mut fingerprint = blake3::Hasher::new();
    for value in [
        &provider.kind,
        provider.endpoint.as_deref().unwrap_or(""),
        &provider.model,
    ] {
        fingerprint.update(&(value.len() as u64).to_le_bytes());
        fingerprint.update(value.as_bytes());
    }
    fingerprint.finalize().to_hex().to_string()
}

fn validate_operation_id(operation_id: &str, operation: &str) -> Result<()> {
    uuid::Uuid::parse_str(operation_id).map_err(|_| {
        CoreError::validation(
            "ai_operation_invalid",
            "The AI operation identifier is invalid",
            operation,
        )
    })?;
    Ok(())
}

fn stop_reason(reason: StopReason) -> AiStopReason {
    match reason {
        StopReason::Completed(raw) => AiStopReason::Completed { raw },
        StopReason::MaxTokens(raw) => AiStopReason::MaxTokens { raw },
        StopReason::ToolCall(raw) => AiStopReason::ToolCall { raw },
        StopReason::ContentFilter(raw) => AiStopReason::ContentFilter { raw },
        StopReason::StopSequence(raw) => AiStopReason::StopSequence { raw },
        StopReason::Other(raw) => AiStopReason::Other { raw },
    }
}

fn credential_for(provider: &AiProviderConfig, operation: &str) -> Result<Option<String>> {
    provider
        .credential_ref
        .as_ref()
        .map(|reference| {
            keyring::Entry::new("org.noura.ai", reference)
                .and_then(|entry| entry.get_password())
                .map_err(|_| credential_error(operation))
        })
        .transpose()
}

fn validate_provider(provider: &AiProviderConfig, operation: &str) -> Result<()> {
    if provider.id.is_empty()
        || provider.id.len() > 128
        || provider.id.chars().any(char::is_control)
        || provider.model.trim().is_empty()
        || provider.model.len() > 256
        || provider.model.chars().any(char::is_control)
    {
        return Err(CoreError::validation(
            "provider_invalid",
            "Provider ID and model are required",
            operation,
        ));
    }
    if AdapterKind::from_lower_str(&provider.kind).is_none() {
        return Err(CoreError::validation(
            "provider_kind_invalid",
            "The AI provider kind is not supported",
            operation,
        ));
    }
    if let Some(endpoint) = &provider.endpoint {
        let url = url::Url::parse(endpoint).map_err(|_| {
            CoreError::validation(
                "endpoint_invalid",
                "The provider endpoint must be a valid URL",
                operation,
            )
        })?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err(CoreError::validation(
                "endpoint_invalid",
                "Provider endpoints use HTTP or HTTPS",
                operation,
            ));
        }
    }
    Ok(())
}

fn validate_provider_configs(providers: &[AiProviderConfig], operation: &str) -> Result<()> {
    let mut ids = std::collections::HashSet::with_capacity(providers.len());
    for provider in providers {
        validate_provider(provider, operation)?;
        if !ids.insert(&provider.id) {
            return Err(CoreError::new(
                "provider_config_invalid",
                ErrorCategory::Parse,
                "AI provider settings contain duplicate provider IDs",
                operation,
            ));
        }
    }
    Ok(())
}

fn provider_lock_error(operation: &str) -> CoreError {
    CoreError::new(
        "provider_lock_unavailable",
        ErrorCategory::Transient,
        "AI provider settings are unavailable",
        operation,
    )
}

fn operation_lock_error(operation: &str) -> CoreError {
    CoreError::new(
        "ai_operation_lock_unavailable",
        ErrorCategory::Transient,
        "The AI operation state is unavailable",
        operation,
    )
}

fn provider_error(operation: &str) -> CoreError {
    CoreError::new(
        "provider_request_failed",
        ErrorCategory::Provider,
        "The AI provider request failed",
        operation,
    )
}

fn incomplete_stream_error() -> CoreError {
    let mut error = CoreError::new(
        "provider_stream_incomplete",
        ErrorCategory::Transient,
        "The AI provider stream ended without a terminal response",
        "ai_stream",
    );
    error.retryable = true;
    error
}

fn credential_error(operation: &str) -> CoreError {
    CoreError::new(
        "credential_store_error",
        ErrorCategory::Credential,
        "The operating system credential store operation failed",
        operation,
    )
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, "ai_provider_save", parent.to_str()))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| {
        CoreError::new(
            "provider_serialize_failed",
            ErrorCategory::Parse,
            "AI provider settings could not be serialized",
            "ai_provider_save",
        )
    })?;
    crate::durable_settings::write(path, &bytes)
        .map_err(|error| CoreError::io(error, "ai_provider_save", path.to_str()))?;
    Ok(())
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
            policy_version: Some("2026-09".into()),
        }
    }

    const WORKSPACE_ID: &str = "workspace_01j00000000000000000000000";

    fn grant_workspace_content(ai: &AiFoundation) {
        ai.grant_consent(
            WORKSPACE_ID,
            AiConsentGrantInput {
                provider_id: "test".into(),
                policy_version: "2026-09".into(),
                data_category: AiConsentDataCategory::WorkspaceContent,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn duplicate_operation_ids_preserve_cancellation_and_cleanup_allows_reuse() {
        let registry = Arc::new(AiOperationRegistry {
            operations: Mutex::new(HashMap::new()),
        });
        let consent = AiConsentKey {
            workspace_id: WORKSPACE_ID.into(),
            provider_id: "test".into(),
            policy_version: "2026-09".into(),
            provider_fingerprint: "a".repeat(64),
        };
        let mut original = registry.start("operation".into(), consent.clone()).unwrap();

        let Err(error) = registry.start("operation".into(), consent.clone()) else {
            panic!("a duplicate operation should be rejected");
        };
        assert_eq!(error.code, "ai_operation_in_progress");

        assert!(registry.cancel("operation").unwrap());
        assert!(original.cancellation.changed().await.is_ok());

        drop(original);
        let replacement = registry.start("operation".into(), consent).unwrap();
        drop(replacement);
    }

    #[tokio::test]
    async fn stream_frames_are_ordered_and_include_a_terminal_summary() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        grant_workspace_content(&ai);
        let mut operation = ai
            .start_stream_with(WORKSPACE_ID, stream_input(), Arc::new(MockStreamer))
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
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        grant_workspace_content(&ai);
        let input = stream_input();
        let operation_id = input.operation_id.clone();
        let mut operation = ai
            .start_stream_with(WORKSPACE_ID, input, Arc::new(PendingMockStreamer))
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
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        grant_workspace_content(&ai);

        let mut operation = ai
            .start_stream_with(WORKSPACE_ID, stream_input(), Arc::new(ClosedMockStreamer))
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
    fn unknown_provider_kind_is_rejected_instead_of_falling_back() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
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
        let consents_path = root.path().join("consents.json");
        let ai = AiFoundation::open(&providers_path, &consents_path).unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        drop(ai);

        let reopened = AiFoundation::open(&providers_path, &consents_path).unwrap();
        assert_eq!(reopened.list_providers().unwrap(), vec![enabled_provider()]);
        drop(reopened);

        let duplicate = vec![enabled_provider(), enabled_provider()];
        std::fs::write(&providers_path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
        let Err(error) = AiFoundation::open(&providers_path, &consents_path) else {
            panic!("duplicate provider IDs must be rejected");
        };
        assert_eq!(error.code, "provider_config_invalid");
    }

    #[test]
    fn stream_requires_matching_workspace_provider_and_policy_consent() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();

        let Err(error) = ai.start_stream_with(WORKSPACE_ID, stream_input(), Arc::new(MockStreamer))
        else {
            panic!("a stream without consent should be rejected");
        };

        assert_eq!(error.code, "ai_consent_required");
    }

    #[test]
    fn provider_configuration_changes_make_existing_consent_ineffective() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        grant_workspace_content(&ai);

        let mut changed = enabled_provider();
        changed.endpoint = Some("https://replacement.example.test/v1/".into());
        ai.save_provider(changed).unwrap();

        assert!(
            ai.read_consent(
                WORKSPACE_ID,
                AiConsentReadInput {
                    provider_id: "test".into(),
                    policy_version: "2026-09".into(),
                },
            )
            .unwrap()
            .is_none()
        );
        let Err(error) = ai.start_stream_with(WORKSPACE_ID, stream_input(), Arc::new(MockStreamer))
        else {
            panic!("a consent for a different provider target must not be effective");
        };
        assert_eq!(error.code, "ai_consent_required");
    }

    #[test]
    fn provider_consent_fingerprint_includes_kind_endpoint_and_model() {
        let provider = enabled_provider();
        let fingerprint = provider_fingerprint(&provider);

        let mut changed_kind = provider.clone();
        changed_kind.kind = "anthropic".into();
        assert_ne!(fingerprint, provider_fingerprint(&changed_kind));

        let mut changed_endpoint = provider.clone();
        changed_endpoint.endpoint = Some("https://other.example.test/v1/".into());
        assert_ne!(fingerprint, provider_fingerprint(&changed_endpoint));

        let mut changed_model = provider;
        changed_model.model = "other-model".into();
        assert_ne!(fingerprint, provider_fingerprint(&changed_model));
    }

    #[test]
    fn stream_rejects_invalid_tool_definitions_and_history_roles() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        grant_workspace_content(&ai);

        let mut invalid_tool = stream_input();
        invalid_tool.tools.as_mut().unwrap()[0].input_schema = serde_json::json!("not a schema");
        let Err(error) = ai.start_stream_with(WORKSPACE_ID, invalid_tool, Arc::new(MockStreamer))
        else {
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
        let Err(error) =
            ai.start_stream_with(WORKSPACE_ID, invalid_history, Arc::new(MockStreamer))
        else {
            panic!("tool history must contain tool results");
        };
        assert_eq!(error.code, "message_content_invalid");
    }

    #[tokio::test]
    async fn revoking_consent_cancels_matching_active_streams_and_blocks_new_ones() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(
            root.path().join("providers.json"),
            root.path().join("consents.json"),
        )
        .unwrap();
        ai.save_provider(enabled_provider()).unwrap();
        grant_workspace_content(&ai);
        let input = stream_input();
        let mut operation = ai
            .start_stream_with(WORKSPACE_ID, input.clone(), Arc::new(PendingMockStreamer))
            .unwrap();
        assert!(matches!(
            operation.receiver.recv().await.unwrap().event,
            AiStreamEvent::Started { .. }
        ));
        let Err(error) = ai.start_stream_with(WORKSPACE_ID, input, Arc::new(PendingMockStreamer))
        else {
            panic!("a duplicate operation should be rejected");
        };
        assert_eq!(error.code, "ai_operation_in_progress");

        let outcome = ai
            .revoke_consent(
                WORKSPACE_ID,
                AiConsentRevokeInput {
                    provider_id: "test".into(),
                    policy_version: "2026-09".into(),
                },
            )
            .unwrap();

        assert_eq!(outcome.cancelled_operations, 1);
        assert!(matches!(
            operation.receiver.recv().await.unwrap().event,
            AiStreamEvent::Cancelled
        ));
        let Err(error) = ai.start_stream_with(WORKSPACE_ID, stream_input(), Arc::new(MockStreamer))
        else {
            panic!("a stream after revocation should be rejected");
        };
        assert_eq!(error.code, "ai_consent_required");
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
        AiStreamFrame::export_all_to(bindings)?;
        AiStreamEvent::export_all_to(bindings)?;
        AiStreamSummary::export_all_to(bindings)?;
        AiCancelOutcome::export_all_to(bindings)?;
        AiUsage::export_all_to(bindings)?;
        AiStopReason::export_all_to(bindings)?;
        AiConsentDataCategory::export_all_to(bindings)?;
        AiConsentGrantInput::export_all_to(bindings)?;
        AiConsentReadInput::export_all_to(bindings)?;
        AiConsentRevokeInput::export_all_to(bindings)?;
        AiConsentGrant::export_all_to(bindings)?;
        AiConsentRevokeOutcome::export_all_to(bindings)?;
        Ok(())
    }
}
