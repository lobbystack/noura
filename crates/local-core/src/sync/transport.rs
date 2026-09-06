use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use super::{ApplyOutcome, EncryptedOperation, ObjectKey, identifier, invalid};
use crate::{CoreError, ErrorCategory, Result, WorkspaceEngine};

mod blobs;

/// Native-only key material obtained from approved device/recovery grants.
#[derive(Default)]
pub struct SyncSecrets {
    pub objects: BTreeMap<(String, u64), ObjectKey>,
    pub trusted_devices: BTreeMap<String, String>,
    /// Devices authorized to write every object by a locally verified access policy.
    pub authorized_workspace_writers: BTreeSet<String>,
    /// Object-scoped writers authorized by that same policy.
    pub authorized_object_writers: BTreeSet<(String, String)>,
    /// Durable policy-revision checkpoints used to authorize replayed history.
    pub historical_workspace_writers: BTreeMap<String, BTreeSet<String>>,
    pub historical_object_writers: BTreeSet<(String, String, String)>,
}

impl SyncSecrets {
    fn writer_is_authorized(&self, revision: &str, object: &str, device: &str) -> bool {
        self.historical_workspace_writers
            .get(revision)
            .is_some_and(|writers| writers.contains(device))
            || self.historical_object_writers.contains(&(
                revision.into(),
                object.into(),
                device.into(),
            ))
    }
}

/// A bounded pass can be resumed; the durable journal is the source of progress.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPass {
    pub uploaded: usize,
    pub downloaded: usize,
    pub conflicts: usize,
    pub has_more: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Page {
    access_revision: String,
    operations: Vec<SequencedOperation>,
    cursor: String,
    has_more: bool,
}

// Flatten is deliberately limited to this wire DTO. Validate the embedded envelope explicitly.
#[derive(Deserialize)]
struct SequencedOperation {
    #[serde(flatten)]
    operation: EncryptedOperation,
    sequence: String,
}

#[derive(Deserialize)]
struct ServerErrorEnvelope {
    error: ServerErrorBody,
}

#[derive(Deserialize)]
struct ServerErrorBody {
    code: String,
}

/// HTTP transport with redirect refusal, finite timeouts, and native-only bearer credentials.
pub struct HttpSyncTransport {
    client: Client,
    origin: String,
    token: Zeroizing<String>,
}

impl HttpSyncTransport {
    pub async fn workspaces(&self) -> Result<Vec<super::RemoteSyncWorkspace>> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct List {
            workspaces: Vec<super::RemoteSyncWorkspace>,
        }
        let list: List = self.request(Method::GET, "/v1/workspaces", None).await?;
        if list.workspaces.len() > 1000 {
            return Err(invalid("sync_workspace_limit"));
        }
        for workspace in &list.workspaces {
            identifier(&workspace.id)?;
            if !matches!(
                workspace.role.as_str(),
                "owner" | "admin" | "editor" | "viewer"
            ) {
                return Err(invalid("sync_invalid_response"));
            }
        }
        Ok(list.workspaces)
    }
    pub(crate) async fn prepare_objects(&self, engine: &WorkspaceEngine) -> Result<()> {
        for operation in engine.sync_outbox()?.into_iter().take(25) {
            #[derive(Deserialize)]
            struct Object {
                epoch: u64,
            }
            let object: Object = self
                .request(
                    Method::POST,
                    &format!("/v1/workspaces/{}/objects", operation.workspace_id),
                    Some(&serde_json::json!({"id":operation.object_id})),
                )
                .await?;
            if object.epoch != operation.epoch {
                return Err(invalid("sync_stale_epoch"));
            }
        }
        Ok(())
    }
    pub(crate) async fn access_state(
        &self,
        workspace: &str,
    ) -> Result<super::approvals::AccessState> {
        identifier(workspace)?;
        self.request(
            Method::GET,
            &format!("/v1/workspaces/{workspace}/access-state"),
            None,
        )
        .await
    }

    /// Verify every signed transition from the durable local checkpoint to the relay's head.
    pub(crate) async fn refresh_access_policies(
        &self,
        engine: &WorkspaceEngine,
        config: &super::WorkspaceSyncConfig,
    ) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Policies {
            revision: String,
            policies: Vec<super::AccessPolicy>,
        }
        let workspace = engine.manifest().id;
        let mut previous = engine.sync_access_policy()?;
        let mut after = engine.sync_access_revision()?;
        for _ in 0..101 {
            let page: Policies = self
                .request(
                    Method::GET,
                    &format!("/v1/workspaces/{workspace}/access?after={after}"),
                    None,
                )
                .await?;
            let head = parse_cursor(&page.revision)?;
            let current = parse_cursor(&after)?;
            if head < current || page.policies.len() > 10 {
                return Err(invalid("sync_invalid_policy_page"));
            }
            if page.policies.is_empty() {
                return if head == current {
                    Ok(())
                } else {
                    Err(invalid("sync_invalid_policy_page"))
                };
            }
            for policy in page.policies {
                let expected = parse_cursor(&after)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("sync_invalid_policy"))?;
                if parse_cursor(&policy.revision)? != expected || policy.workspace_id != workspace {
                    return Err(invalid("sync_policy_chain_changed"));
                }
                let previous_digest = previous
                    .as_ref()
                    .map(super::AccessPolicy::digest)
                    .transpose()?;
                if policy.previous_policy_digest != previous_digest {
                    return Err(invalid("sync_policy_chain_changed"));
                }
                let signer = config
                    .trusted_devices
                    .get(&policy.device_id)
                    .ok_or_else(|| invalid("sync_untrusted_device"))?;
                policy.verify(signer)?;
                let signer_account = config
                    .approved_accounts
                    .get(&policy.device_id)
                    .ok_or_else(|| invalid("sync_device_approval_required"))?;
                let authorized = previous.as_ref().map_or_else(
                    || {
                        policy.members.iter().any(|member| {
                            member.account_id == *signer_account
                                && member.role == super::WorkspaceRole::Owner
                        })
                    },
                    |prior| {
                        prior.members.iter().any(|member| {
                            member.account_id == *signer_account
                                && matches!(
                                    member.role,
                                    super::WorkspaceRole::Owner | super::WorkspaceRole::Admin
                                )
                        })
                    },
                );
                if !authorized {
                    return Err(invalid("sync_policy_signer_not_authorized"));
                }
                if previous.is_none() {
                    let bootstrap_writers = config
                        .approved_accounts
                        .iter()
                        .filter(|(device, account)| {
                            *account == signer_account
                                && config.trusted_devices.contains_key(*device)
                        })
                        .map(|(device, _)| device.clone())
                        .collect();
                    engine.sync_record_access_authorization(
                        "0",
                        &bootstrap_writers,
                        &BTreeSet::new(),
                    )?;
                }
                engine.sync_accept_access_policy(&after, previous_digest.as_deref(), &policy)?;
                let (workspace_writers, object_writers) =
                    super::approvals::policy_authorizations(&policy, config);
                engine.sync_record_access_authorization(
                    &policy.revision,
                    &workspace_writers,
                    &object_writers,
                )?;
                after = policy.revision.clone();
                previous = Some(policy);
            }
            if parse_cursor(&after)? == head {
                return Ok(());
            }
        }
        Err(invalid("sync_policy_page_limit"))
    }
    /// Download authenticated recipient envelopes without learning trust from server-supplied keys.
    /// Accepted envelopes are committed to the file journal before their plaintext keys enter the pass.
    pub async fn receive_keys(
        &self,
        engine: &WorkspaceEngine,
        device: &super::DeviceKeys,
        secrets: &mut SyncSecrets,
    ) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Row {
            object_id: String,
            epoch: String,
            device_id: String,
            wrapped_key: String,
            signing_device: String,
            signature: String,
            signing_public_key: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Keys {
            envelopes: Vec<Row>,
            has_more: bool,
        }
        let workspace = engine.manifest().id;
        identifier(&workspace)?;
        let mut after_object = String::new();
        let mut after_epoch = 0;
        for _ in 0..10 {
            let page: Keys = self
                .request(
                    Method::GET,
                    &format!(
                        "/v1/workspaces/{workspace}/keys?device={}&afterEpoch={after_epoch}{after}",
                        device.device_id(),
                        after = if after_object.is_empty() {
                            String::new()
                        } else {
                            format!("&afterObject={after_object}")
                        }
                    ),
                    None,
                )
                .await?;
            if page.envelopes.len() > 100 || (page.has_more && page.envelopes.is_empty()) {
                return Err(invalid("sync_invalid_key_page"));
            }
            for row in page.envelopes {
                identifier(&row.object_id)?;
                let epoch = parse_cursor(&row.epoch)?;
                if (row.object_id.as_str(), epoch) <= (after_object.as_str(), after_epoch) {
                    return Err(invalid("sync_invalid_key_page"));
                }
                let trusted = secrets
                    .trusted_devices
                    .get(&row.signing_device)
                    .ok_or_else(|| invalid("sync_untrusted_device"))?;
                if trusted != &row.signing_public_key {
                    return Err(invalid("sync_untrusted_device"));
                }
                let envelope = super::KeyEnvelope {
                    workspace_id: workspace.clone(),
                    object_id: row.object_id.clone(),
                    epoch,
                    device_id: row.device_id,
                    wrapped_key: row.wrapped_key,
                    signing_device: row.signing_device,
                    signature: row.signature,
                };
                let key = device.unwrap_key(&envelope, trusted)?;
                if let Some(existing) = secrets.objects.get(&(row.object_id.clone(), epoch)) {
                    if existing.secret() != key.secret() {
                        return Err(invalid("sync_object_key_changed"));
                    }
                } else {
                    engine.sync_store_key(&envelope, device, trusted)?;
                }
                after_object = row.object_id.clone();
                after_epoch = epoch;
                secrets.objects.insert((row.object_id, epoch), key);
            }
            if !page.has_more {
                return Ok(());
            }
        }
        Err(invalid("sync_key_page_limit"))
    }
    pub(crate) async fn share_keys(&self, envelopes: &[super::KeyEnvelope]) -> Result<()> {
        let _: serde_json::Value = self
            .request(
                Method::PUT,
                "/v1/keys/share",
                Some(
                    &serde_json::to_value(envelopes)
                        .map_err(|_| invalid("sync_serialize_failed"))?,
                ),
            )
            .await?;
        Ok(())
    }
    pub async fn set_access(
        &self,
        policy: &super::AccessPolicy,
        trusted_signer: &str,
    ) -> Result<()> {
        policy.verify(trusted_signer)?;
        #[derive(Deserialize)]
        struct Updated {
            revision: String,
        }
        let result: Updated = self
            .request(
                Method::PUT,
                &format!("/v1/workspaces/{}/access", policy.workspace_id),
                Some(&serde_json::to_value(policy).map_err(|_| invalid("sync_serialize_failed"))?),
            )
            .await?;
        if result.revision != policy.revision {
            return Err(invalid("sync_invalid_response"));
        }
        Ok(())
    }
    pub fn new(origin: &str, token: String) -> Result<Self> {
        validate_origin(origin)?;
        if token.len() != 43
            || !token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(invalid("sync_invalid_connection"));
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| network_error())?;
        Ok(Self {
            client,
            origin: origin.into(),
            token: Zeroizing::new(token),
        })
    }

    pub(crate) async fn create_invitation(
        &self,
        workspace: &str,
        role: super::SyncInvitationRole,
    ) -> Result<super::SyncInvitationLink> {
        identifier(workspace)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Created {
            id: String,
            workspace_id: String,
            role: super::SyncInvitationRole,
            expires_at: String,
            token: String,
        }
        let created: Created = self
            .request(
                Method::POST,
                &format!("/v1/workspaces/{workspace}/invitations"),
                Some(&serde_json::json!({ "role": role })),
            )
            .await?;
        identifier(&created.id)?;
        if created.workspace_id != workspace
            || created.role != role
            || !valid_invitation_token(&created.token)
            || !valid_server_time(&created.expires_at)
        {
            return Err(invalid("sync_invalid_invitation"));
        }
        Ok(super::SyncInvitationLink {
            id: created.id,
            role,
            invite_url: format!("{}/invite/{}", self.origin, created.token),
            expires_at: created.expires_at,
        })
    }

    pub(crate) async fn invitations(
        &self,
        workspace: &str,
    ) -> Result<Vec<super::approvals::RemoteInvitation>> {
        identifier(workspace)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Invitations {
            invitations: Vec<super::approvals::RemoteInvitation>,
        }
        let result: Invitations = self
            .request(
                Method::GET,
                &format!("/v1/workspaces/{workspace}/invitations"),
                None,
            )
            .await?;
        if result.invitations.len() > 100 {
            return Err(invalid("sync_invitation_limit"));
        }
        for invitation in &result.invitations {
            identifier(&invitation.id)?;
            if !valid_server_time(&invitation.expires_at) || invitation.devices.len() > 1000 {
                return Err(invalid("sync_invalid_invitation"));
            }
            if let Some(account) = &invitation.account_id {
                identifier(account)?;
            }
            for device in &invitation.devices {
                identifier(&device.device_id)?;
                identifier(&device.account_id)?;
                super::crypto::decode(&device.public_key, 32, 32)?;
                let recipient = device
                    .encryption_recipient
                    .as_ref()
                    .ok_or_else(|| invalid("sync_device_upgrade_required"))?;
                recipient
                    .parse::<age::x25519::Recipient>()
                    .map_err(|_| invalid("sync_invalid_recipient"))?;
                if invitation.account_id.as_ref() != Some(&device.account_id) {
                    return Err(invalid("sync_invalid_invitation"));
                }
            }
        }
        Ok(result.invitations)
    }

    pub(crate) async fn revoke_invitation(&self, workspace: &str, invitation: &str) -> Result<()> {
        identifier(workspace)?;
        identifier(invitation)?;
        let _: serde_json::Value = self
            .request(
                Method::DELETE,
                &format!("/v1/workspaces/{workspace}/invitations/{invitation}"),
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn create_workspace(&self, workspace: &str) -> Result<()> {
        identifier(workspace)?;
        let _: serde_json::Value = self
            .request(
                Method::POST,
                "/v1/workspaces",
                Some(&serde_json::json!({"id":workspace})),
            )
            .await?;
        Ok(())
    }

    pub async fn revoke_device(&self, device: &str) -> Result<()> {
        identifier(device)?;
        let _: serde_json::Value = self
            .request(Method::DELETE, &format!("/v1/devices/{device}"), None)
            .await?;
        Ok(())
    }

    /// Upload pending operations in order, then pull and durably apply up to ten pages.
    /// Missing keys or untrusted senders stop the pass without advancing its cursor.
    pub async fn synchronize(
        &self,
        engine: &WorkspaceEngine,
        secrets: &SyncSecrets,
    ) -> Result<SyncPass> {
        self.synchronize_inner(engine, secrets, false, true).await
    }

    /// Idle coordinators wait for a PostgreSQL commit notification, bounded to 25 seconds.
    /// Dropping this future cancels the HTTP request; durable progress still lives in the journal.
    pub async fn synchronize_wait(
        &self,
        engine: &WorkspaceEngine,
        secrets: &SyncSecrets,
    ) -> Result<SyncPass> {
        self.synchronize_inner(engine, secrets, true, true).await
    }

    pub(crate) async fn synchronize_readonly(
        &self,
        engine: &WorkspaceEngine,
        secrets: &SyncSecrets,
        wait: bool,
    ) -> Result<SyncPass> {
        self.synchronize_inner(engine, secrets, wait, false).await
    }
    async fn synchronize_inner(
        &self,
        engine: &WorkspaceEngine,
        secrets: &SyncSecrets,
        wait: bool,
        upload: bool,
    ) -> Result<SyncPass> {
        let workspace = engine.manifest().id;
        identifier(&workspace)?;
        let mut result = SyncPass::default();
        for op in engine
            .sync_outbox()?
            .into_iter()
            .take(if upload { 25 } else { 0 })
        {
            if !secrets.writer_is_authorized(&op.policy_revision, &op.object_id, &op.device_id) {
                return Err(invalid("sync_writer_not_authorized"));
            }
            let public_key = secrets
                .trusted_devices
                .get(&op.device_id)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            op.verify(public_key)?;
            let _: serde_json::Value = self
                .request(
                    Method::POST,
                    &format!("/v1/workspaces/{workspace}/objects"),
                    Some(&serde_json::json!({"id":op.object_id})),
                )
                .await?;
            if let Some(envelope) =
                engine.sync_key_envelope(&op.object_id, op.epoch, &op.device_id)?
                && envelope.signing_device == op.device_id
            {
                envelope.verify(public_key)?;
                let _: serde_json::Value = self
                    .request(
                        Method::PUT,
                        "/v1/keys/self",
                        Some(
                            &serde_json::to_value(envelope)
                                .map_err(|_| invalid("sync_serialize_failed"))?,
                        ),
                    )
                    .await?;
            }
            let key = secrets
                .objects
                .get(&(op.object_id.clone(), op.epoch))
                .ok_or_else(|| invalid("sync_key_required"))?;
            let plaintext = op.open(key, public_key)?;
            let change: super::FileChange =
                serde_json::from_slice(&plaintext).map_err(|_| invalid("sync_invalid_change"))?;
            super::validate_file_change(&change)?;
            if let Some(blob) = &change.blob {
                let mut file = engine.sync_blob_file(blob, false)?;
                self.upload_blob(&workspace, &op.object_id, op.epoch, blob, &mut file)
                    .await?;
            }
            #[derive(Deserialize)]
            struct Push {
                sequences: Vec<String>,
            }
            let response: Push = self
                .request(
                    Method::POST,
                    &format!("/v1/workspaces/{workspace}/operations"),
                    Some(&serde_json::json!({"operations":[op]})),
                )
                .await?;
            if response.sequences.len() != 1 || parse_cursor(&response.sequences[0])? == 0 {
                return Err(invalid("sync_invalid_response"));
            }
            engine.sync_acknowledge(&op, &response.sequences[0])?;
            result.uploaded += 1;
        }
        for index in 0..10 {
            let previous = engine.sync_cursor()?;
            let waiting = if wait && index == 0 && result.uploaded == 0 {
                format!("&wait=25&accessRevision={}", engine.sync_access_revision()?)
            } else {
                String::new()
            };
            let page: Page = self
                .request(
                    Method::GET,
                    &format!("/v1/workspaces/{workspace}/operations?after={previous}{waiting}"),
                    None,
                )
                .await?;
            parse_cursor(&page.access_revision)?;
            if engine.sync_access_revision()? != page.access_revision {
                return Err(invalid("sync_access_changed"));
            }
            let end = parse_cursor(&page.cursor)?;
            let mut last = parse_cursor(&previous)?;
            if end < last
                || page.operations.len() > 100
                || (page.has_more && page.operations.is_empty())
            {
                return Err(invalid("sync_invalid_page"));
            }
            for entry in &page.operations {
                let sequence = parse_cursor(&entry.sequence)?;
                if sequence <= last || sequence > end || entry.operation.workspace_id != workspace {
                    return Err(invalid("sync_invalid_page"));
                }
                entry.operation.validate()?;
                last = sequence;
            }
            let mut applied = Vec::with_capacity(page.operations.len());
            for entry in page.operations {
                let op = entry.operation;
                if !secrets.writer_is_authorized(&op.policy_revision, &op.object_id, &op.device_id)
                {
                    return Err(invalid("sync_writer_not_authorized"));
                }
                let public_key = secrets
                    .trusted_devices
                    .get(&op.device_id)
                    .ok_or_else(|| invalid("sync_untrusted_device"))?;
                let key = secrets
                    .objects
                    .get(&(op.object_id.clone(), op.epoch))
                    .ok_or_else(|| invalid("sync_key_required"))?;
                let plaintext = op.open(key, public_key)?;
                let change: super::FileChange = serde_json::from_slice(&plaintext)
                    .map_err(|_| invalid("sync_invalid_change"))?;
                super::validate_file_change(&change)?;
                if let Some(blob) = &change.blob {
                    let mut file = engine.sync_blob_file(blob, true)?;
                    self.download_blob(&workspace, &op.object_id, blob, &mut file)
                        .await?;
                }
                if engine.sync_apply_file(&op, key, public_key)? == ApplyOutcome::Conflict {
                    result.conflicts += 1;
                }
                result.downloaded += 1;
                applied.push(op);
            }
            engine.sync_checkpoint(&previous, &page.cursor, &applied)?;
            result.has_more = page.has_more;
            if !page.has_more {
                break;
            }
        }
        result.has_more |= upload && !engine.sync_outbox()?.is_empty();
        Ok(result)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.origin))
            .bearer_auth(self.token.as_str());
        if let Some(body) = body {
            request = request.json(body);
        }
        let mut response = request.send().await.map_err(|_| network_error())?;
        let status = response.status();
        if !status.is_success() {
            let (fallback_code, category) = match status {
                StatusCode::UNAUTHORIZED => ("sync_sign_in_required", ErrorCategory::Credential),
                StatusCode::FORBIDDEN => ("sync_access_denied", ErrorCategory::Permission),
                StatusCode::CONFLICT => ("sync_server_conflict", ErrorCategory::Conflict),
                StatusCode::PAYLOAD_TOO_LARGE => {
                    ("sync_quota_or_payload_limit", ErrorCategory::Validation)
                }
                StatusCode::TOO_MANY_REQUESTS => ("sync_rate_limited", ErrorCategory::Transient),
                _ => ("sync_server_unavailable", ErrorCategory::Transient),
            };
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| network_error())? {
                if bytes.len() + chunk.len() > 16 * 1024 {
                    bytes.clear();
                    break;
                }
                bytes.extend_from_slice(&chunk);
            }
            let code = if status == StatusCode::UNAUTHORIZED {
                // Preserve the native credential contract used to request sign-in.
                fallback_code.into()
            } else {
                public_server_error_code(&bytes).unwrap_or_else(|| fallback_code.into())
            };
            let mut error = CoreError::new(
                &code,
                category,
                "The sync request could not complete",
                "sync",
            );
            error.retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            return Err(error);
        }
        if response.status() == StatusCode::NO_CONTENT {
            return serde_json::from_value(serde_json::Value::Null)
                .map_err(|_| invalid("sync_invalid_response"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| network_error())? {
            if bytes.len() + chunk.len() > 145 * 1024 * 1024 {
                return Err(invalid("sync_response_too_large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_response"))
    }
}

fn public_server_error_code(bytes: &[u8]) -> Option<String> {
    let envelope: ServerErrorEnvelope = serde_json::from_slice(bytes).ok()?;
    let suffix = envelope.error.code.strip_prefix("sync.")?;
    if suffix.is_empty()
        || envelope.error.code.len() > 64
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    Some(format!("sync_{suffix}"))
}

fn valid_invitation_token(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_server_time(value: &str) -> bool {
    value.len() <= 64
        && value.len() >= 20
        && value.bytes().all(|byte| byte.is_ascii_graphic())
        && value.contains('T')
}

pub(crate) fn parse_cursor(value: &str) -> Result<u64> {
    let number: u64 = value.parse().map_err(|_| invalid("sync_invalid_cursor"))?;
    if number > i64::MAX as u64 || number.to_string() != value {
        return Err(invalid("sync_invalid_cursor"));
    }
    Ok(number)
}

fn network_error() -> CoreError {
    let mut error = CoreError::new(
        "sync_network_unavailable",
        ErrorCategory::Transient,
        "The sync server could not be reached",
        "sync",
    );
    error.retryable = true;
    error
}

pub(crate) fn validate_origin(origin: &str) -> Result<()> {
    let parsed = url::Url::parse(origin).map_err(|_| invalid("sync_invalid_origin"))?;
    if parsed.origin().ascii_serialization() != origin
        || !(parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    {
        return Err(invalid("sync_invalid_origin"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_only_go_to_exact_secure_origins() {
        for origin in [
            "http://example.com",
            "https://example.com/path",
            "https://user:pass@example.com",
            "ftp://localhost",
            "https://example.com/",
        ] {
            assert!(
                HttpSyncTransport::new(origin, "a".repeat(43)).is_err(),
                "{origin}"
            );
        }
        assert!(HttpSyncTransport::new("http://127.0.0.1:1900", "a".repeat(43)).is_ok());
    }

    #[test]
    fn sequenced_envelopes_deserialize_without_losing_wire_fields() {
        let signer = super::super::SigningIdentity::generate();
        let op = signer
            .seal(
                &ObjectKey::generate(),
                "workspace",
                "object",
                "device",
                1,
                b"data",
            )
            .unwrap();
        let mut value = serde_json::to_value(&op).unwrap();
        value["sequence"] = serde_json::json!("1");
        let entry: SequencedOperation = serde_json::from_value(value).unwrap();
        assert_eq!(entry.operation, op);
    }

    #[test]
    fn public_server_error_codes_are_bounded_and_normalized() {
        assert_eq!(
            public_server_error_code(br#"{"error":{"code":"sync.key_rotation_required"}}"#),
            Some("sync_key_rotation_required".into())
        );
        assert_eq!(
            public_server_error_code(br#"{"error":{"code":"server.internal"}}"#),
            None
        );
        assert_eq!(
            public_server_error_code(br#"{"error":{"code":"sync.Bad-Code"}}"#),
            None
        );
    }
}
