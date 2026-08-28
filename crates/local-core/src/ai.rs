use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Mutex,
};

use genai::{
    Client, ModelIden, ServiceTarget,
    adapter::AdapterKind,
    chat::{ChatMessage, ChatRequest},
    resolver::{AuthData, Endpoint, ServiceTargetResolver},
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CoreError, ErrorCategory, Result};

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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiInvokeInput {
    pub provider_id: String,
    pub messages: Vec<AiMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiResponse {
    pub text: String,
    pub provider_id: String,
    pub model: String,
}

pub struct ResolvedAiRequest {
    pub provider: AiProviderConfig,
    pub credential: Option<String>,
    pub messages: Vec<AiMessage>,
}

pub trait AiInvoker: Send + Sync {
    fn invoke(
        &self,
        request: ResolvedAiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>>;
}

pub struct GenAiInvoker;
impl AiInvoker for GenAiInvoker {
    fn invoke(
        &self,
        request: ResolvedAiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
        Box::pin(async move {
            let adapter = AdapterKind::from_lower_str(&request.provider.kind).ok_or_else(|| {
                CoreError::validation(
                    "provider_kind_invalid",
                    "The AI provider kind is not supported",
                    "ai_invoke",
                )
            })?;
            let endpoint = request.provider.endpoint.clone();
            let credential = request.credential.clone();
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
            let client = Client::builder()
                .with_service_target_resolver(resolver)
                .build();
            let messages = request
                .messages
                .into_iter()
                .map(|message| match message.role.as_str() {
                    "system" => ChatMessage::system(message.content),
                    "assistant" => ChatMessage::assistant(message.content),
                    _ => ChatMessage::user(message.content),
                })
                .collect();
            let response = client
                .exec_chat(
                    request.provider.model.clone(),
                    ChatRequest::new(messages),
                    None,
                )
                .await
                .map_err(|_| {
                    CoreError::new(
                        "provider_request_failed",
                        ErrorCategory::Provider,
                        "The AI provider request failed",
                        "ai_invoke",
                    )
                })?;
            let text = response.first_text().unwrap_or_default().to_owned();
            Ok(AiResponse {
                text,
                provider_id: request.provider.id,
                model: request.provider.model,
            })
        })
    }
}

pub struct AiFoundation {
    config_path: PathBuf,
    providers: Mutex<Vec<AiProviderConfig>>,
}
impl AiFoundation {
    pub fn open(config_path: impl AsRef<Path>) -> Result<Self> {
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
        for provider in &providers {
            validate_provider(provider, "ai_provider_load")?;
        }
        Ok(Self {
            config_path,
            providers: Mutex::new(providers),
        })
    }
    pub fn list_providers(&self) -> Result<Vec<AiProviderConfig>> {
        Ok(self
            .providers
            .lock()
            .map_err(|_| {
                CoreError::new(
                    "provider_lock_unavailable",
                    ErrorCategory::Transient,
                    "AI provider settings are unavailable",
                    "ai_provider_list",
                )
            })?
            .clone())
    }
    pub fn save_provider(&self, provider: AiProviderConfig) -> Result<()> {
        validate_provider(&provider, "ai_provider_save")?;
        let mut values = self.providers.lock().map_err(|_| {
            CoreError::new(
                "provider_lock_unavailable",
                ErrorCategory::Transient,
                "AI provider settings are unavailable",
                "ai_provider_save",
            )
        })?;
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
    pub async fn invoke(&self, input: AiInvokeInput) -> Result<AiResponse> {
        self.invoke_with(input, &GenAiInvoker).await
    }
    pub async fn invoke_with<I: AiInvoker>(
        &self,
        input: AiInvokeInput,
        invoker: &I,
    ) -> Result<AiResponse> {
        let provider = self
            .list_providers()?
            .into_iter()
            .find(|value| value.id == input.provider_id && value.enabled)
            .ok_or_else(|| {
                CoreError::validation(
                    "provider_not_found",
                    "The enabled AI provider does not exist",
                    "ai_invoke",
                )
            })?;
        let credential = provider
            .credential_ref
            .as_ref()
            .map(|reference| {
                keyring::Entry::new("org.noura.ai", reference)
                    .and_then(|entry| entry.get_password())
                    .map_err(|_| credential_error("ai_invoke"))
            })
            .transpose()?;
        invoker
            .invoke(ResolvedAiRequest {
                provider,
                credential,
                messages: input.messages,
            })
            .await
    }
}

fn validate_provider(provider: &AiProviderConfig, operation: &str) -> Result<()> {
    if provider.id.is_empty() || provider.model.is_empty() {
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
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| CoreError::io(error, "ai_provider_save", temporary.to_str()))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| CoreError::io(error, "ai_provider_save", path.to_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    struct MockInvoker;
    impl AiInvoker for MockInvoker {
        fn invoke(
            &self,
            request: ResolvedAiRequest,
        ) -> Pin<Box<dyn Future<Output = Result<AiResponse>> + Send + '_>> {
            Box::pin(async move {
                assert!(request.credential.is_none());
                Ok(AiResponse {
                    text: "mocked".into(),
                    provider_id: request.provider.id,
                    model: request.provider.model,
                })
            })
        }
    }
    #[tokio::test]
    async fn provider_abstraction_can_be_mocked_without_a_credential() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        ai.save_provider(AiProviderConfig {
            id: "test".into(),
            kind: "openai".into(),
            display_name: "Test".into(),
            model: "test-model".into(),
            endpoint: Some("https://example.test/v1/".into()),
            credential_ref: None,
            enabled: true,
        })
        .unwrap();
        let response = ai
            .invoke_with(
                AiInvokeInput {
                    provider_id: "test".into(),
                    messages: vec![AiMessage {
                        role: "user".into(),
                        content: "Hello".into(),
                    }],
                },
                &MockInvoker,
            )
            .await
            .unwrap();
        assert_eq!(response.text, "mocked");
    }

    #[test]
    fn unknown_provider_kind_is_rejected_instead_of_falling_back() {
        let root = tempdir().unwrap();
        let ai = AiFoundation::open(root.path().join("providers.json")).unwrap();
        let error = ai
            .save_provider(AiProviderConfig {
                id: "unknown".into(),
                kind: "not-a-provider".into(),
                display_name: "Unknown".into(),
                model: "model".into(),
                endpoint: None,
                credential_ref: None,
                enabled: true,
            })
            .unwrap_err();
        assert_eq!(error.code, "provider_kind_invalid");
        assert!(ai.list_providers().unwrap().is_empty());
    }
}
