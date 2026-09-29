//! AI providers, credentials, consent, and streams.

use local_core::{
    AiCancelOutcome, AiConsentGrant, AiConsentGrantInput, AiConsentReadInput, AiConsentRevokeInput,
    AiConsentRevokeOutcome, AiProviderConfig, AiStreamFrame, AiStreamInput, CoreError,
};
use serde::Deserialize;
use tauri::{State, ipc::Channel};

use crate::{AppState, blocking, current_engine, with_engine};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSetInput {
    provider_id: String,
    secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialDeleteInput {
    credential_ref: String,
}

#[tauri::command(async)]
pub fn ai_provider_list(state: State<AppState>) -> Result<Vec<AiProviderConfig>, CoreError> {
    state.ai("ai_provider_list")?.list_providers()
}

#[tauri::command(async)]
pub fn ai_provider_save(state: State<AppState>, input: AiProviderConfig) -> Result<(), CoreError> {
    state.ai("ai_provider_save")?.save_provider(input)
}

#[tauri::command(async)]
pub fn ai_credential_set(
    state: State<AppState>,
    input: CredentialSetInput,
) -> Result<serde_json::Value, CoreError> {
    let credential_ref = state
        .ai("ai_credential_set")?
        .set_credential(&input.provider_id, &input.secret)?;
    Ok(serde_json::json!({"credentialRef":credential_ref}))
}

#[tauri::command(async)]
pub fn ai_credential_delete(
    state: State<AppState>,
    input: CredentialDeleteInput,
) -> Result<(), CoreError> {
    state
        .ai("ai_credential_delete")?
        .delete_credential(&input.credential_ref)
}

#[tauri::command(async)]
pub fn ai_consent_read(
    state: State<AppState>,
    input: AiConsentReadInput,
) -> Result<Option<AiConsentGrant>, CoreError> {
    with_engine(&state, "ai_consent_read", |engine| {
        state
            .ai("ai_consent_read")?
            .read_consent(&engine.manifest().id, input)
    })
}

#[tauri::command(async)]
pub fn ai_consent_grant(
    state: State<AppState>,
    input: AiConsentGrantInput,
) -> Result<AiConsentGrant, CoreError> {
    with_engine(&state, "ai_consent_grant", |engine| {
        state
            .ai("ai_consent_grant")?
            .grant_consent(&engine.manifest().id, input)
    })
}

#[tauri::command(async)]
pub fn ai_consent_revoke(
    state: State<AppState>,
    input: AiConsentRevokeInput,
) -> Result<AiConsentRevokeOutcome, CoreError> {
    with_engine(&state, "ai_consent_revoke", |engine| {
        state
            .ai("ai_consent_revoke")?
            .revoke_consent(&engine.manifest().id, input)
    })
}

#[tauri::command]
pub async fn ai_stream(
    state: State<'_, AppState>,
    input: AiStreamInput,
    channel: Channel<AiStreamFrame>,
) -> Result<(), CoreError> {
    let operation_id = input.operation_id.clone();
    let workspace_id = current_engine(&state, "ai_stream")?.manifest().id;
    let ai = state.ai("ai_stream")?;
    let starter = ai.clone();
    let mut operation = blocking("ai_stream", move || {
        starter.start_stream(&workspace_id, input)
    })
    .await?;
    while let Some(frame) = operation.receiver.recv().await {
        if channel.send(frame).is_err() {
            if let Err(error) = ai.cancel_stream(&operation_id) {
                log::warn!("could not cancel an abandoned AI stream: {}", error.code);
            }
            break;
        }
    }
    Ok(())
}

#[tauri::command(async)]
pub fn ai_stream_cancel(
    state: State<AppState>,
    operation_id: String,
) -> Result<AiCancelOutcome, CoreError> {
    state.ai("ai_stream_cancel")?.cancel_stream(&operation_id)
}
