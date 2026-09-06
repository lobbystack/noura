use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use super::*;
use crate::sync::{
    ApplyOutcome, DeviceKeys, EncryptedOperation, FileChange, KeyEnvelope, ObjectKey,
    SigningIdentity,
};
use crate::sync::{invalid, validate_file_change as validate_change};

const STATE_PATH: &str = ".noura/sync/state.json";
mod blobs;
mod conflicts;

// Windows FlushFileBuffers requires a handle opened with GENERIC_WRITE, even
// when we only read the existing bytes before committing their sync descriptor.
fn open_for_durable_read(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(cfg!(windows))
        .open(path)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u8,
    workspace_id: String,
    cursor: String,
    access_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_policy: Option<crate::sync::AccessPolicy>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    access_authorizations: BTreeMap<String, PolicyAuthorization>,
    receipts: BTreeMap<String, Receipt>,
    objects: BTreeMap<String, ObjectState>,
    outbox: Vec<EncryptedOperation>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObjectState {
    path: String,
    revision: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyAuthorization {
    workspace_writers: Vec<String>,
    object_writers: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    digest: String,
    outcome: ApplyOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    change_digest: Option<String>,
}

impl WorkspaceEngine {
    /// Create a new empty local replica. Existing workspace identities are never changed.
    pub fn create_sync_replica(
        root: impl AsRef<Path>,
        name: &str,
        workspace_id: &str,
        app_data: impl AsRef<Path>,
    ) -> Result<Self> {
        crate::sync::identifier(workspace_id)?;
        let root = root.as_ref();
        if root
            .try_exists()
            .map_err(|_| invalid("sync_replica_path_unavailable"))?
            && std::fs::read_dir(root)
                .map_err(|_| invalid("sync_replica_path_unavailable"))?
                .next()
                .is_some()
        {
            return Err(invalid("sync_replica_directory_not_empty"));
        }
        Self::create_with_identity(root, name, app_data, Some(workspace_id))
    }
    pub fn sync_configuration(&self) -> Result<Option<crate::sync::WorkspaceSyncConfig>> {
        let _lock = self.write_lock("sync_configuration")?;
        let value = read_optional(&self.sync_path(".noura/sync/config.json")?)?
            .map(|bytes| {
                serde_json::from_slice::<crate::sync::WorkspaceSyncConfig>(&bytes)
                    .map_err(|_| invalid("sync_invalid_configuration"))
            })
            .transpose()?;
        if let Some(config) = &value {
            config.validate(&self.manifest().id)?;
        }
        Ok(value)
    }

    pub fn sync_save_configuration(&self, config: &crate::sync::WorkspaceSyncConfig) -> Result<()> {
        config.validate(&self.manifest().id)?;
        let _lock = self.write_lock("sync_configuration")?;
        self.sync_write(".noura/sync/config.json", config)
    }

    pub fn sync_pause(&self, paused: bool) -> Result<()> {
        let mut config = self
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        config.enabled = !paused;
        self.sync_save_configuration(&config)
    }

    pub fn sync_status(&self) -> Result<crate::sync::WorkspaceSyncStatus> {
        use crate::sync::{WorkspaceSyncPhase, WorkspaceSyncStatus};
        let config = self.sync_configuration()?;
        let _lock = self.write_lock("sync_status")?;
        let journal = self.sync_journal()?;
        Ok(WorkspaceSyncStatus {
            enabled: config.as_ref().is_some_and(|value| value.enabled),
            phase: match config {
                None => WorkspaceSyncPhase::Disabled,
                Some(value) if !value.enabled => WorkspaceSyncPhase::Paused,
                Some(_) => WorkspaceSyncPhase::Idle,
            },
            pending: journal.outbox.len().try_into().unwrap_or(u32::MAX),
            conflicts: journal
                .receipts
                .values()
                .filter(|receipt| receipt.outcome == ApplyOutcome::Conflict)
                .count()
                .try_into()
                .unwrap_or(u32::MAX),
            error_code: None,
            last_success: None,
        })
    }

    /// Restore encrypted local keys without consulting a derived database or trusting server metadata.
    pub fn sync_restore_secrets(
        &self,
        device: &DeviceKeys,
        trusted: &BTreeMap<String, String>,
    ) -> Result<crate::sync::SyncSecrets> {
        let _lock = self.write_lock("sync_restore_keys")?;
        let mut result = crate::sync::SyncSecrets {
            objects: BTreeMap::new(),
            trusted_devices: trusted.clone(),
            authorized_workspace_writers: Default::default(),
            authorized_object_writers: Default::default(),
            ..Default::default()
        };
        let root = self.sync_path(".noura/sync/keys")?;
        if !root.exists() {
            return Ok(result);
        }
        for object in std::fs::read_dir(&root).map_err(|_| invalid("sync_key_read_failed"))? {
            let object = object.map_err(|_| invalid("sync_key_read_failed"))?;
            let name = object
                .file_name()
                .into_string()
                .map_err(|_| invalid("sync_unsupported_path"))?;
            crate::sync::identifier(&name)?;
            let directory = self.sync_path(&format!(".noura/sync/keys/{name}"))?;
            for epoch in
                std::fs::read_dir(directory).map_err(|_| invalid("sync_key_read_failed"))?
            {
                let epoch = epoch
                    .map_err(|_| invalid("sync_key_read_failed"))?
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("sync_unsupported_path"))?;
                let number = sync_cursor(&epoch)?;
                let path = key_path(&name, number, device.device_id())?;
                let Some(bytes) = read_optional(&self.sync_path(&path)?)? else {
                    continue;
                };
                let envelope: KeyEnvelope = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("sync_invalid_wrapped_key"))?;
                if envelope.workspace_id != self.manifest().id
                    || envelope.object_id != name
                    || envelope.epoch != number
                {
                    return Err(invalid("sync_invalid_wrapped_key"));
                }
                let signer = trusted
                    .get(&envelope.signing_device)
                    .ok_or_else(|| invalid("sync_untrusted_device"))?;
                result.objects.insert(
                    (name.clone(), number),
                    device.unwrap_key(&envelope, signer)?,
                );
            }
        }
        Ok(result)
    }

    pub fn sync_key_envelope(
        &self,
        object: &str,
        epoch: u64,
        device: &str,
    ) -> Result<Option<KeyEnvelope>> {
        let _lock = self.write_lock("sync_read_key")?;
        read_optional(&self.sync_path(&key_path(object, epoch, device)?)?)?
            .map(|bytes| {
                serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_wrapped_key"))
            })
            .transpose()
    }

    /// Scan canonical files first, then missing catalog paths, so moves precede tombstones.
    /// A failed scan never interprets unreadable or ignored files as deletions.
    pub fn sync_capture_workspace(
        &self,
        device: &DeviceKeys,
        secrets: &mut crate::sync::SyncSecrets,
    ) -> Result<()> {
        let mut paths = Vec::new();
        for entry in workspace_walker(&self.root, &self.current_ignore())? {
            let entry = entry.map_err(|_| invalid("sync_scan_failed"))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&self.root)
                .ok()
                .and_then(Path::to_str)
                .ok_or_else(|| invalid("sync_unsupported_path"))?;
            #[cfg(windows)]
            let portable = relative.replace('\\', "/");
            #[cfg(windows)]
            let relative = portable.as_str();
            if relative.eq_ignore_ascii_case("workspace.yaml") {
                continue;
            }
            self.sync_file_path(relative)?;
            paths.push(relative.to_string());
        }
        paths.sort();
        for path in paths {
            self.sync_capture_device_path(&path, device, secrets)?;
        }
        let missing: Vec<String> = {
            let _lock = self.write_lock("sync_scan_deleted")?;
            self.sync_journal()?
                .objects
                .values()
                .filter(|object| object.revision.is_some())
                .map(|object| object.path.clone())
                .collect()
        };
        for path in missing {
            if !self
                .sync_file_path(&path)?
                .try_exists()
                .map_err(|_| invalid("sync_scan_failed"))?
            {
                self.sync_capture_device_path(&path, device, secrets)?;
            }
        }
        Ok(())
    }

    fn sync_capture_device_path(
        &self,
        path: &str,
        device: &DeviceKeys,
        secrets: &mut crate::sync::SyncSecrets,
    ) -> Result<()> {
        if self.sync_capture_attachment(path, device, secrets)? {
            return Ok(());
        }
        self.sync_capture_with(
            path,
            |workspace, object, is_new, policy_revision, plaintext| {
                let epoch = secrets
                    .objects
                    .keys()
                    .filter(|(id, _)| id == object)
                    .map(|(_, epoch)| *epoch)
                    .max();
                let epoch = match epoch {
                    Some(epoch) => epoch,
                    None if is_new => {
                        let key = ObjectKey::generate();
                        let envelope = device.wrap_key(
                            workspace,
                            object,
                            1,
                            device.device_id(),
                            &device.recipient(),
                            &key,
                        )?;
                        self.sync_write_once(&key_path(object, 1, device.device_id())?, &envelope)?;
                        secrets.objects.insert((object.into(), 1), key);
                        1
                    }
                    None => return Err(invalid("sync_key_required")),
                };
                let key = secrets
                    .objects
                    .get(&(object.into(), epoch))
                    .ok_or_else(|| invalid("sync_key_required"))?;
                device.signer().seal_at_revision(
                    key,
                    workspace,
                    object,
                    device.device_id(),
                    (epoch, policy_revision),
                    plaintext,
                )
            },
        )?;
        Ok(())
    }

    /// Persist only an authenticated, recipient-encrypted key. Plain object keys never enter files.
    /// The caller must obtain `trusted_signer` from a prior device approval, not the response being stored.
    pub fn sync_store_key(
        &self,
        envelope: &KeyEnvelope,
        device: &DeviceKeys,
        trusted_signer: &str,
    ) -> Result<()> {
        if envelope.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let _key = device.unwrap_key(envelope, trusted_signer)?;
        let _lock = self.write_lock("sync_store_key")?;
        let path = key_path(&envelope.object_id, envelope.epoch, device.device_id())?;
        self.sync_write_once(&path, envelope)
    }

    /// Restore a content key from its durable encrypted envelope after an index rebuild or restart.
    pub fn sync_load_key(
        &self,
        object: &str,
        epoch: u64,
        device: &DeviceKeys,
        trusted_signer: &str,
    ) -> Result<ObjectKey> {
        let _lock = self.write_lock("sync_load_key")?;
        let path = key_path(object, epoch, device.device_id())?;
        let bytes =
            read_optional(&self.sync_path(&path)?)?.ok_or_else(|| invalid("sync_key_required"))?;
        let envelope: KeyEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_wrapped_key"))?;
        if envelope.workspace_id != self.manifest().id
            || envelope.object_id != object
            || envelope.epoch != epoch
        {
            return Err(invalid("sync_invalid_wrapped_key"));
        }
        device.unwrap_key(&envelope, trusted_signer)
    }

    /// Capture durable current bytes, including changes made outside Noura.
    /// Ordinary files receive stable sidecar IDs; managed Markdown keeps its frontmatter ID.
    pub fn sync_capture_file(
        &self,
        path: &str,
        key: &ObjectKey,
        signer: &SigningIdentity,
        device: &str,
        epoch: u64,
    ) -> Result<Option<EncryptedOperation>> {
        self.sync_capture_with(path, |workspace, object, _, policy_revision, plaintext| {
            signer.seal_at_revision(
                key,
                workspace,
                object,
                device,
                (epoch, policy_revision),
                plaintext,
            )
        })
    }

    /// Capture using an object-specific key encrypted to this native device.
    /// A new object's wrapped key reaches disk before its operation enters the durable outbox.
    /// Existing objects with missing keys fail closed instead of silently generating replacement keys.
    pub fn sync_capture_owned_file(
        &self,
        path: &str,
        device: &DeviceKeys,
        epoch: u64,
    ) -> Result<Option<EncryptedOperation>> {
        self.sync_capture_with(
            path,
            |workspace, object, is_new, policy_revision, plaintext| {
                let path = key_path(object, epoch, device.device_id())?;
                let key = match read_optional(&self.sync_path(&path)?)? {
                    Some(bytes) => {
                        let envelope: KeyEnvelope = serde_json::from_slice(&bytes)
                            .map_err(|_| invalid("sync_invalid_wrapped_key"))?;
                        if envelope.workspace_id != workspace
                            || envelope.object_id != object
                            || envelope.epoch != epoch
                        {
                            return Err(invalid("sync_invalid_wrapped_key"));
                        }
                        device.unwrap_key(&envelope, &device.signer().public_key())?
                    }
                    None if is_new => {
                        let key = ObjectKey::generate();
                        let envelope = device.wrap_key(
                            workspace,
                            object,
                            epoch,
                            device.device_id(),
                            &device.recipient(),
                            &key,
                        )?;
                        self.sync_write_once(&path, &envelope)?;
                        key
                    }
                    None => return Err(invalid("sync_key_required")),
                };
                device.signer().seal_at_revision(
                    &key,
                    workspace,
                    object,
                    device.device_id(),
                    (epoch, policy_revision),
                    plaintext,
                )
            },
        )
    }

    fn sync_capture_with(
        &self,
        path: &str,
        seal: impl FnOnce(&str, &str, bool, &str, &[u8]) -> Result<EncryptedOperation>,
    ) -> Result<Option<EncryptedOperation>> {
        validate_change(&FileChange {
            version: 1,
            path: path.into(),
            previous_path: None,
            base_revision: None,
            content: None,
            accepted_revisions: None,
            blob: None,
        })?;
        let _lock = self.write_lock("sync_capture")?;
        let source = self.sync_file_path(path)?;
        if source
            .try_exists()
            .map_err(|_| invalid("sync_file_read_failed"))?
            && source
                .metadata()
                .map_err(|_| invalid("sync_file_read_failed"))?
                .len()
                > 700 * 1024
        {
            return Err(invalid("sync_attachment_required"));
        }
        let current = read_optional(&self.sync_file_path(path)?)?;
        if current.is_some() {
            open_for_durable_read(&self.sync_file_path(path)?)
                .and_then(|file| file.sync_all())
                .map_err(|error| CoreError::io(error, "sync_capture", Some(path)))?;
            if read_optional(&self.sync_file_path(path)?)? != current {
                return Err(invalid("sync_file_changed"));
            }
        }
        let mut journal = self.sync_journal()?;
        let known = journal.objects.iter().find(|(_, value)| value.path == path);
        let managed_id =
            current
                .as_ref()
                .and_then(|bytes| match markdown::parse_markdown(path, bytes) {
                    ParsedMarkdown::Managed(object) => Some(object.id),
                    _ => None,
                });
        if let (Some(id), Some((known_id, _))) = (&managed_id, known)
            && id != known_id
        {
            return Err(invalid("sync_identity_changed"));
        }
        if let Some(id) = &managed_id
            && self.sync_duplicate_identity(id, &[path])?
        {
            return Err(invalid("sync_duplicate_identity"));
        }
        let revision = current.as_ref().map(|bytes| markdown::revision(bytes));
        let mut moved_id = None;
        if known.is_none() && managed_id.is_none() && current.is_some() {
            for (id, state) in &journal.objects {
                if state.revision == revision
                    && read_optional(&self.sync_file_path(&state.path)?)?.is_none()
                {
                    if moved_id.is_some() {
                        return Err(invalid("sync_ambiguous_move"));
                    }
                    moved_id = Some(id.clone());
                }
            }
        }
        let object_id = managed_id
            .or_else(|| known.map(|(id, _)| id.clone()))
            .or(moved_id)
            .unwrap_or_else(|| format!("file_{}", ulid::Ulid::new().to_string().to_lowercase()));
        let previous = journal.objects.get(&object_id);
        if previous.is_some_and(|value| value.path == path && value.revision == revision)
            || (previous.is_none() && current.is_none())
        {
            return Ok(None);
        }
        if previous.is_some_and(|value| value.path != path)
            && let Some(old) = previous
            && self.sync_file_path(&old.path)?.exists()
        {
            return Err(invalid("sync_duplicate_identity"));
        }
        let change = FileChange {
            version: 1,
            path: path.into(),
            previous_path: previous
                .filter(|value| value.path != path)
                .map(|value| value.path.clone()),
            base_revision: previous.and_then(|value| value.revision.clone()),
            content: current.as_ref().map(|bytes| STANDARD.encode(bytes)),
            accepted_revisions: None,
            blob: None,
        };
        let plaintext = zeroize::Zeroizing::new(
            serde_json::to_vec(&change).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        let op = seal(
            &journal.workspace_id,
            &object_id,
            previous.is_none(),
            &journal.access_revision,
            &plaintext,
        )?;
        journal.outbox.push(op.clone());
        journal.objects.insert(
            object_id,
            ObjectState {
                path: path.into(),
                revision,
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        Ok(Some(op))
    }

    /// Persist a sealed operation before attempting any network request.
    pub fn sync_enqueue(&self, op: &EncryptedOperation, trusted_key: &str) -> Result<()> {
        op.verify(trusted_key)?;
        self.sync_check_workspace(op)?;
        let _lock = self.write_lock("sync_enqueue")?;
        let mut journal = self.sync_journal()?;
        if let Some(existing) = journal
            .outbox
            .iter()
            .find(|existing| existing.operation_id == op.operation_id)
        {
            if existing != op {
                return Err(invalid("sync_operation_id_reused"));
            }
            return Ok(());
        }
        journal.outbox.push(op.clone());
        self.sync_write(STATE_PATH, &journal)
    }

    /// Reload exact ciphertext in capture order after process restart.
    pub fn sync_outbox(&self) -> Result<Vec<EncryptedOperation>> {
        let _lock = self.write_lock("sync_outbox")?;
        Ok(self.sync_journal()?.outbox)
    }

    /// Record a server receipt durably before removing a pending upload.
    pub fn sync_acknowledge(&self, op: &EncryptedOperation, sequence: &str) -> Result<()> {
        sync_cursor(sequence)?;
        self.sync_check_workspace(op)?;
        op.validate()?;
        let _lock = self.write_lock("sync_acknowledge")?;
        let mut journal = self.sync_journal()?;
        let Some(index) = journal
            .outbox
            .iter()
            .position(|existing| existing.operation_id == op.operation_id)
        else {
            return Ok(());
        };
        if &journal.outbox[index] != op {
            return Err(invalid("sync_operation_id_reused"));
        }
        self.sync_write_once(
            &format!(".noura/sync/sent/{}.json", op.operation_id),
            &serde_json::json!({"version":1,"sequence":sequence,"operation":op}),
        )?;
        journal.outbox.remove(index);
        journal.receipts.insert(
            op.operation_id.clone(),
            Receipt {
                digest: markdown::revision(
                    &serde_json::to_vec(op).map_err(|_| invalid("sync_serialize_failed"))?,
                ),
                outcome: ApplyOutcome::Applied,
                change_digest: None,
            },
        );
        self.sync_write(STATE_PATH, &journal)
    }

    /// Persist incoming ciphertext, authenticate it, then apply or preserve a conflict.
    /// The operation receipt is committed only after canonical bytes or conflict bytes.
    pub fn sync_apply_file(
        &self,
        op: &EncryptedOperation,
        key: &ObjectKey,
        trusted_key: &str,
    ) -> Result<ApplyOutcome> {
        self.sync_check_workspace(op)?;
        let plaintext = op.open(key, trusted_key)?;
        let change: FileChange =
            serde_json::from_slice(&plaintext).map_err(|_| invalid("sync_invalid_change"))?;
        validate_change(&change)?;
        let _lock = self.write_lock("sync_apply")?;
        let mut journal = self.sync_journal()?;
        let digest = markdown::revision(
            &serde_json::to_vec(op).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        if let Some(receipt) = journal.receipts.get(&op.operation_id) {
            if receipt.digest != digest {
                return Err(invalid("sync_operation_id_reused"));
            }
            return Ok(receipt.outcome.clone());
        }
        self.sync_write_once(&format!(".noura/sync/inbox/{}.json", op.operation_id), op)?;
        let occupied = journal.objects.iter().any(|(id, state)| {
            id != &op.object_id && state.path == change.path && state.revision.is_some()
        });
        let wrong_source = journal.objects.get(&op.object_id).is_some_and(|state| {
            state.path != change.previous_path.as_deref().unwrap_or(&change.path)
        });
        let unbound_update =
            !journal.objects.contains_key(&op.object_id) && change.base_revision.is_some();
        let outcome = if occupied || wrong_source || unbound_update {
            self.sync_conflict(op, &change)?
        } else if change.blob.is_some() {
            self.sync_apply_attachment(op, &change, key)?
        } else {
            self.sync_apply_change(op, &change)?
        };
        if outcome == ApplyOutcome::Applied {
            self.sync_mark_reviewed_conflicts(&mut journal, &op.object_id, &change)?;
            let revision = change
                .content
                .as_ref()
                .map(|content| {
                    STANDARD
                        .decode(content)
                        .map(|bytes| markdown::revision(&bytes))
                        .map_err(|_| invalid("sync_invalid_content"))
                })
                .transpose()?
                .or_else(|| change.blob.as_ref().map(|blob| blob.revision.clone()));
            journal.objects.insert(
                op.object_id.clone(),
                ObjectState {
                    path: change.path.clone(),
                    revision,
                },
            );
        }
        journal.receipts.insert(
            op.operation_id.clone(),
            Receipt {
                digest,
                outcome: outcome.clone(),
                change_digest: Some(markdown::revision(
                    &serde_json::to_vec(&change).map_err(|_| invalid("sync_serialize_failed"))?,
                )),
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        drop(_lock);
        // Index repair cannot turn a successful canonical commit into a failed mutation.
        let _ =
            self.index_outcome(self.reconcile_forced_with_source(
                &std::collections::HashSet::from([change.path]),
                "sync",
            ));
        Ok(outcome)
    }

    /// Advance a pull checkpoint only after every envelope in that page has a durable receipt.
    pub fn sync_checkpoint(
        &self,
        previous: &str,
        next: &str,
        ops: &[EncryptedOperation],
    ) -> Result<()> {
        let previous_number = sync_cursor(previous)?;
        let next_number = sync_cursor(next)?;
        if next_number < previous_number {
            return Err(invalid("sync_cursor_regressed"));
        }
        let _lock = self.write_lock("sync_checkpoint")?;
        let mut journal = self.sync_journal()?;
        if journal.cursor != previous {
            return Err(invalid("sync_cursor_changed"));
        }
        for op in ops {
            self.sync_check_workspace(op)?;
            let digest = markdown::revision(
                &serde_json::to_vec(op).map_err(|_| invalid("sync_serialize_failed"))?,
            );
            if journal
                .receipts
                .get(&op.operation_id)
                .is_none_or(|receipt| receipt.digest != digest)
            {
                return Err(invalid("sync_unapplied_operation"));
            }
        }
        journal.cursor = next.into();
        self.sync_write(STATE_PATH, &journal)
    }

    pub fn sync_cursor(&self) -> Result<String> {
        let _lock = self.write_lock("sync_cursor")?;
        Ok(self.sync_journal()?.cursor)
    }

    pub fn sync_access_revision(&self) -> Result<String> {
        let _lock = self.write_lock("sync_access_revision")?;
        Ok(self.sync_journal()?.access_revision)
    }

    pub fn sync_access_policy(&self) -> Result<Option<crate::sync::AccessPolicy>> {
        let _lock = self.write_lock("sync_access_policy")?;
        Ok(self.sync_journal()?.access_policy)
    }

    /// Persist one verified policy transition and revisit history exposed by its grants.
    pub fn sync_accept_access_policy(
        &self,
        previous_revision: &str,
        previous_digest: Option<&str>,
        policy: &crate::sync::AccessPolicy,
    ) -> Result<()> {
        sync_cursor(previous_revision)?;
        let revision = sync_cursor(&policy.revision)?;
        let expected = sync_cursor(previous_revision)?
            .checked_add(1)
            .ok_or_else(|| invalid("sync_invalid_policy"))?;
        if revision != expected || policy.workspace_id != self.manifest().id {
            return Err(invalid("sync_policy_chain_changed"));
        }
        let _lock = self.write_lock("sync_access_policy")?;
        let mut journal = self.sync_journal()?;
        let current_digest = journal
            .access_policy
            .as_ref()
            .map(crate::sync::AccessPolicy::digest)
            .transpose()?;
        if journal.access_revision != previous_revision
            || current_digest.as_deref() != previous_digest
            || policy.previous_policy_digest.as_deref() != previous_digest
        {
            return Err(invalid("sync_policy_chain_changed"));
        }
        journal.access_revision = policy.revision.clone();
        journal.access_policy = Some(policy.clone());
        journal.cursor = "0".into();
        self.sync_write(STATE_PATH, &journal)
    }

    pub(crate) fn sync_record_access_authorization(
        &self,
        revision: &str,
        workspace_writers: &BTreeSet<String>,
        object_writers: &BTreeSet<(String, String)>,
    ) -> Result<()> {
        let revision_number = sync_cursor(revision)?;
        let mut objects = BTreeMap::<String, Vec<String>>::new();
        for device in workspace_writers {
            crate::sync::identifier(device)?;
        }
        for (object, device) in object_writers {
            crate::sync::identifier(object)?;
            crate::sync::identifier(device)?;
            objects
                .entry(object.clone())
                .or_default()
                .push(device.clone());
        }
        let authorization = PolicyAuthorization {
            workspace_writers: workspace_writers.iter().cloned().collect(),
            object_writers: objects,
        };
        let _lock = self.write_lock("sync_access_authorization")?;
        let mut journal = self.sync_journal()?;
        if revision_number > sync_cursor(&journal.access_revision)? {
            return Err(invalid("sync_policy_chain_changed"));
        }
        if let Some(existing) = journal.access_authorizations.get(revision)
            && existing == &authorization
        {
            return Ok(());
        }
        journal
            .access_authorizations
            .insert(revision.into(), authorization);
        self.sync_write(STATE_PATH, &journal)
    }

    pub(crate) fn sync_load_access_authorizations(
        &self,
        secrets: &mut crate::sync::SyncSecrets,
    ) -> Result<()> {
        let _lock = self.write_lock("sync_access_authorization")?;
        let journal = self.sync_journal()?;
        secrets.historical_workspace_writers.clear();
        secrets.historical_object_writers.clear();
        for (revision, authorization) in journal.access_authorizations {
            secrets.historical_workspace_writers.insert(
                revision.clone(),
                authorization.workspace_writers.into_iter().collect(),
            );
            for (object, devices) in authorization.object_writers {
                for device in devices {
                    secrets.historical_object_writers.insert((
                        revision.clone(),
                        object.clone(),
                        device,
                    ));
                }
            }
        }
        Ok(())
    }

    /// New grants can expose history before the current cursor. Revisit it, retaining receipts.
    pub fn sync_reset_for_access(&self, revision: &str) -> Result<()> {
        sync_cursor(revision)?;
        let _lock = self.write_lock("sync_access_revision")?;
        let mut journal = self.sync_journal()?;
        if journal.access_revision != revision {
            journal.access_revision = revision.into();
            journal.cursor = "0".into();
            self.sync_write(STATE_PATH, &journal)?;
        }
        Ok(())
    }

    fn sync_apply_change(
        &self,
        op: &EncryptedOperation,
        change: &FileChange,
    ) -> Result<ApplyOutcome> {
        let source_path = change.previous_path.as_deref().unwrap_or(&change.path);
        if self.sync_duplicate_identity(&op.object_id, &[source_path, &change.path])? {
            return self.sync_conflict(op, change);
        }
        self.sync_apply_unique_change(op, change)
    }

    fn sync_duplicate_identity(&self, id: &str, excluded: &[&str]) -> Result<bool> {
        for entry in workspace_walker(&self.root, &self.current_ignore())? {
            let entry = entry.map_err(|_| invalid("sync_scan_failed"))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file())
                || entry.path().extension().and_then(|ext| ext.to_str()) != Some("md")
            {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&self.root)
                .ok()
                .and_then(Path::to_str)
                .ok_or_else(|| invalid("sync_unsupported_path"))?;
            #[cfg(windows)]
            let portable = relative.replace('\\', "/");
            #[cfg(windows)]
            let relative = portable.as_str();
            #[cfg(not(windows))]
            if relative.contains('\\') {
                return Err(invalid("sync_unsupported_path"));
            }
            if excluded.contains(&relative) {
                continue;
            }
            let bytes = std::fs::read(self.sync_file_path(relative)?)
                .map_err(|error| CoreError::io(error, "sync", Some(relative)))?;
            if let ParsedMarkdown::Managed(object) = markdown::parse_markdown(relative, &bytes)
                && object.id == id
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn sync_apply_unique_change(
        &self,
        op: &EncryptedOperation,
        change: &FileChange,
    ) -> Result<ApplyOutcome> {
        let destination = self.sync_file_path(&change.path)?;
        let source_path = change.previous_path.as_deref().unwrap_or(&change.path);
        let source = self.sync_file_path(source_path)?;
        let current = read_optional(&source)?;
        let current_revision = current.as_ref().map(|bytes| markdown::revision(bytes));
        let target = if source == destination {
            current.clone()
        } else {
            read_optional(&destination)?
        };
        let incoming = change
            .content
            .as_ref()
            .map(|content| {
                STANDARD
                    .decode(content)
                    .map_err(|_| invalid("sync_invalid_content"))
            })
            .transpose()?;

        // A managed file's durable ID, not a supplied path, determines identity.
        for (path, bytes) in [
            (source_path, current.as_deref()),
            (change.path.as_str(), incoming.as_deref()),
        ] {
            if let Some(bytes) = bytes
                && let ParsedMarkdown::Managed(object) = markdown::parse_markdown(path, bytes)
                && object.id != op.object_id
            {
                return self.sync_conflict(op, change);
            }
        }
        let base_matches = match (&current, &change.base_revision) {
            (None, None) => true,
            (Some(bytes), Some(base)) => markdown::revision(bytes) == *base,
            _ => false,
        } || change.accepted_revisions.as_ref().is_some_and(|revisions| {
            revisions.contains(&current.as_ref().map(|bytes| markdown::revision(bytes)))
        });
        let already_written = target == incoming;
        if !base_matches && !(already_written && (source == destination || current.is_none())) {
            return self.sync_conflict(op, change);
        }
        if source != destination && target.is_some() && !already_written {
            return self.sync_conflict(op, change);
        }
        if let Some(bytes) = &incoming {
            if !already_written {
                self.sync_prepare_parent(&change.path)?;
                self.sync_write_file_checked(
                    &change.path,
                    bytes,
                    if source == destination {
                        current_revision.as_deref()
                    } else {
                        None
                    },
                )?;
            }
        } else if current.is_some() {
            // Keep deleted bytes recoverable even if a crash follows the unlink.
            self.sync_write(&format!(".noura/sync/deleted/{}.json", op.operation_id),
                &serde_json::json!({"path":source_path,"content":current.as_ref().map(|v| STANDARD.encode(v))}))?;
            if read_optional(&source)? != current {
                return self.sync_conflict(op, change);
            }
            std::fs::remove_file(&source)
                .map_err(|e| CoreError::io(e, "sync_apply", Some(source_path)))?;
            sync_parent(&source, "sync_apply")?;
        }
        if source != destination && current.is_some() {
            // Recheck immediately before removing the original of a move.
            if read_optional(&source)? != current {
                return self.sync_conflict(op, change);
            }
            std::fs::remove_file(&source)
                .map_err(|e| CoreError::io(e, "sync_apply", Some(source_path)))?;
            sync_parent(&source, "sync_apply")?;
        }
        Ok(ApplyOutcome::Applied)
    }

    fn sync_conflict(&self, op: &EncryptedOperation, change: &FileChange) -> Result<ApplyOutcome> {
        self.sync_write_once(
            &format!(".noura/sync/conflicts/{}.json", op.operation_id),
            change,
        )?;
        Ok(ApplyOutcome::Conflict)
    }

    fn sync_check_workspace(&self, op: &EncryptedOperation) -> Result<()> {
        if op.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        Ok(())
    }

    fn sync_journal(&self) -> Result<Journal> {
        let path = self.sync_path(STATE_PATH)?;
        let Some(bytes) = read_optional(&path)? else {
            return Ok(Journal {
                version: 1,
                workspace_id: self.manifest().id,
                cursor: "0".into(),
                access_revision: "0".into(),
                access_policy: None,
                ..Journal::default()
            });
        };
        let journal: Journal =
            serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_journal"))?;
        if journal.version != 1 || journal.workspace_id != self.manifest().id {
            return Err(invalid("sync_invalid_journal"));
        }
        sync_cursor(&journal.cursor)?;
        sync_cursor(&journal.access_revision)?;
        match &journal.access_policy {
            Some(policy)
                if policy.workspace_id == self.manifest().id
                    && policy.revision == journal.access_revision =>
            {
                policy.digest()?;
            }
            None if journal.access_revision == "0" => {}
            _ => return Err(invalid("sync_invalid_journal")),
        }
        let access_revision = sync_cursor(&journal.access_revision)?;
        for (revision, authorization) in &journal.access_authorizations {
            if sync_cursor(revision)? > access_revision
                || !strictly_ordered(&authorization.workspace_writers)
            {
                return Err(invalid("sync_invalid_journal"));
            }
            for device in &authorization.workspace_writers {
                crate::sync::identifier(device)?;
            }
            for (object, devices) in &authorization.object_writers {
                crate::sync::identifier(object)?;
                if !strictly_ordered(devices) {
                    return Err(invalid("sync_invalid_journal"));
                }
                for device in devices {
                    crate::sync::identifier(device)?;
                }
            }
        }
        let mut ids = std::collections::HashSet::new();
        for op in &journal.outbox {
            op.validate()?;
            self.sync_check_workspace(op)?;
            if !ids.insert(&op.operation_id) {
                return Err(invalid("sync_invalid_journal"));
            }
        }
        for (id, object) in &journal.objects {
            crate::sync::identifier(id)?;
            validate_change(&FileChange {
                version: 1,
                path: object.path.clone(),
                previous_path: None,
                base_revision: object.revision.clone(),
                content: None,
                accepted_revisions: None,
                blob: None,
            })?;
        }
        Ok(journal)
    }

    fn sync_file_path(&self, relative: &str) -> Result<PathBuf> {
        let path = self.sync_path(relative)?;
        let existing = path
            .ancestors()
            .find(|parent| parent.exists())
            .ok_or_else(|| invalid("sync_unsafe_path"))?;
        let canonical = existing
            .canonicalize()
            .map_err(|error| CoreError::io(error, "sync", Some(relative)))?;
        // Resolve filesystem aliases (including Windows short names), not only textual prefixes.
        for reserved in [".noura", ".git", "node_modules", "target", "workspace.yaml"] {
            let protected = self.root.join(reserved);
            if protected.exists() {
                let protected = protected
                    .canonicalize()
                    .map_err(|error| CoreError::io(error, "sync", Some(relative)))?;
                if canonical.starts_with(protected) {
                    return Err(invalid("sync_unsafe_path"));
                }
            }
        }
        Ok(path)
    }

    fn sync_path(&self, relative: &str) -> Result<PathBuf> {
        // symlink_metadata also sees dangling links, unlike Path::exists.
        let path = crate::path::validate_relative(relative, "sync")?;
        let mut current = self.root.clone();
        for component in path.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(invalid("sync_symlink")),
                Ok(meta) if !meta.is_file() && !meta.is_dir() => {
                    return Err(invalid("sync_unsupported_path"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(CoreError::io(error, "sync", Some(relative))),
            }
        }
        let existing = current
            .ancestors()
            .find(|parent| parent.exists())
            .ok_or_else(|| invalid("sync_unsafe_path"))?;
        let root = self
            .root
            .canonicalize()
            .map_err(|error| CoreError::io(error, "sync", None))?;
        let canonical = existing
            .canonicalize()
            .map_err(|error| CoreError::io(error, "sync", Some(relative)))?;
        if !canonical.starts_with(root) {
            return Err(invalid("sync_symlink"));
        }
        Ok(current)
    }

    fn sync_write(&self, path: &str, value: &impl Serialize) -> Result<()> {
        self.sync_path(path)?;
        self.sync_prepare_parent(path)?;
        let bytes = serde_json::to_vec(value).map_err(|_| invalid("sync_serialize_failed"))?;
        atomic_write(&self.root, Path::new(path), &bytes, "sync")
    }

    fn sync_write_file_checked(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<&str>,
    ) -> Result<()> {
        self.sync_prepare_parent(path)?;
        let destination = self.sync_file_path(path)?;
        let mut file = atomic_write_file::AtomicWriteFile::open(&destination)
            .map_err(|error| CoreError::io(error, "sync_apply", Some(path)))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| CoreError::io(error, "sync_apply", Some(path)))?;
        let revision = read_optional(&self.sync_file_path(path)?)?
            .as_ref()
            .map(|bytes| markdown::revision(bytes));
        if revision.as_deref() != expected {
            return Err(invalid("sync_file_changed"));
        }
        file.commit()
            .map_err(|error| CoreError::io(error, "sync_apply", Some(path)))?;
        sync_parent(&destination, "sync_apply")
    }

    fn sync_write_once(&self, path: &str, value: &impl Serialize) -> Result<()> {
        let destination = self.sync_path(path)?;
        let bytes = serde_json::to_vec(value).map_err(|_| invalid("sync_serialize_failed"))?;
        if let Some(existing) = read_optional(&destination)? {
            if existing != bytes {
                return Err(invalid("sync_operation_id_reused"));
            }
            return Ok(());
        }
        self.sync_prepare_parent(path)?;
        atomic_write(&self.root, Path::new(path), &bytes, "sync")
    }

    fn sync_prepare_parent(&self, relative: &str) -> Result<()> {
        self.sync_path(relative)?;
        let Some(parent) = Path::new(relative).parent() else {
            return Ok(());
        };
        let mut path = self.root.clone();
        for part in parent.components() {
            path.push(part);
            if !path.exists() {
                std::fs::create_dir(&path)
                    .map_err(|error| CoreError::io(error, "sync", Some(relative)))?;
                sync_parent(&path, "sync")?;
            }
        }
        Ok(())
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CoreError::io(error, "sync", path.to_str())),
    }
}

fn key_path(object: &str, epoch: u64, device: &str) -> Result<String> {
    crate::sync::identifier(object)?;
    crate::sync::identifier(device)?;
    if epoch == 0 || epoch > 9_007_199_254_740_991 {
        return Err(invalid("sync_invalid_epoch"));
    }
    Ok(format!(".noura/sync/keys/{object}/{epoch}/{device}.json"))
}

fn sync_cursor(value: &str) -> Result<u64> {
    let number: u64 = value.parse().map_err(|_| invalid("sync_invalid_cursor"))?;
    if number > i64::MAX as u64 || number.to_string() != value {
        return Err(invalid("sync_invalid_cursor"));
    }
    Ok(number)
}

fn strictly_ordered(values: &[String]) -> bool {
    values
        .windows(2)
        .all(|pair| pair[0].as_str() < pair[1].as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn wrapped_keys_survive_index_rebuild_and_refuse_replacement_or_wrong_context() {
        use crate::sync::SyncCredentials;
        use std::cell::RefCell;
        use zeroize::Zeroizing;
        #[derive(Default)]
        struct Memory(RefCell<BTreeMap<String, String>>);
        impl SyncCredentials for Memory {
            fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
                self.0
                    .borrow()
                    .get(reference)
                    .cloned()
                    .map(Zeroizing::new)
                    .ok_or_else(|| invalid("missing"))
            }
            fn write(&self, reference: &str, value: &str) -> Result<()> {
                self.0.borrow_mut().insert(reference.into(), value.into());
                Ok(())
            }
        }
        let (_dir, engine) = engine();
        let store = Memory::default();
        let device = DeviceKeys::create(&store).unwrap();
        let key = ObjectKey::generate();
        let public = device.signer().public_key();
        let envelope = device
            .wrap_key(
                &engine.manifest().id,
                "object",
                1,
                device.device_id(),
                &device.recipient(),
                &key,
            )
            .unwrap();
        engine.sync_store_key(&envelope, &device, &public).unwrap();
        engine.sync_store_key(&envelope, &device, &public).unwrap();
        engine.rebuild_index().unwrap();
        let restored = engine.sync_load_key("object", 1, &device, &public).unwrap();
        let operation = device
            .signer()
            .seal(
                &key,
                &engine.manifest().id,
                "object",
                device.device_id(),
                1,
                b"secret",
            )
            .unwrap();
        assert_eq!(&**operation.open(&restored, &public).unwrap(), b"secret");
        let other = device
            .wrap_key(
                &engine.manifest().id,
                "object",
                1,
                device.device_id(),
                &device.recipient(),
                &ObjectKey::generate(),
            )
            .unwrap();
        assert!(engine.sync_store_key(&other, &device, &public).is_err());
        assert!(engine.sync_load_key("object", 2, &device, &public).is_err());
        assert!(
            engine
                .sync_load_key("../escape", 1, &device, &public)
                .is_err()
        );
        let bytes = std::fs::read(
            engine
                .root
                .join(key_path("object", 1, device.device_id()).unwrap()),
        )
        .unwrap();
        assert!(
            !String::from_utf8(bytes)
                .unwrap()
                .contains(&STANDARD.encode(key.secret()))
        );
        std::fs::write(engine.root.join("owned.md"), b"first owned file").unwrap();
        let captured = engine
            .sync_capture_owned_file("owned.md", &device, 1)
            .unwrap()
            .unwrap();
        let captured_key = engine
            .sync_load_key(&captured.object_id, 1, &device, &public)
            .unwrap();
        assert!(captured.open(&captured_key, &public).is_ok());
        let own_path = engine
            .root
            .join(key_path(&captured.object_id, 1, device.device_id()).unwrap());
        let original_key_bytes = std::fs::read(&own_path).unwrap();
        std::fs::write(engine.root.join("owned.md"), b"second owned file").unwrap();
        let second = engine
            .sync_capture_owned_file("owned.md", &device, 1)
            .unwrap()
            .unwrap();
        assert_eq!(captured.object_id, second.object_id);
        assert_eq!(std::fs::read(&own_path).unwrap(), original_key_bytes);
        assert!(second.open(&captured_key, &public).is_ok());
        std::fs::remove_file(&own_path).unwrap();
        std::fs::write(engine.root.join("owned.md"), b"third owned file").unwrap();
        assert_eq!(
            engine
                .sync_capture_owned_file("owned.md", &device, 1)
                .unwrap_err()
                .code,
            "sync_key_required"
        );
        assert!(!own_path.exists());
    }

    fn engine() -> (TempDir, WorkspaceEngine) {
        let dir = TempDir::new().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            dir.path().join("workspace"),
            "Sync test",
            dir.path().join("app"),
        )
        .unwrap();
        (dir, engine)
    }

    fn seal(
        engine: &WorkspaceEngine,
        signer: &SigningIdentity,
        key: &ObjectKey,
        change: &FileChange,
    ) -> EncryptedOperation {
        signer
            .seal(
                key,
                &engine.manifest().id,
                "file_a",
                "device_a",
                1,
                &serde_json::to_vec(change).unwrap(),
            )
            .unwrap()
    }

    fn change(path: &str, content: Option<&[u8]>, base: Option<&[u8]>) -> FileChange {
        FileChange {
            version: 1,
            path: path.into(),
            previous_path: None,
            content: content.map(|v| STANDARD.encode(v)),
            base_revision: base.map(markdown::revision),
            accepted_revisions: None,
            blob: None,
        }
    }

    #[test]
    fn duplicate_canonical_ids_block_capture_even_before_cataloging() {
        let (_dir, engine) = engine();
        let note = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Original".into(),
                body: "canonical".into(),
                relative_path: Some("original.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap()
            .value;
        let bytes = std::fs::read(engine.root.join("original.md")).unwrap();
        std::fs::write(engine.root.join("duplicate.md"), &bytes).unwrap();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        assert_eq!(
            engine
                .sync_capture_file("original.md", &key, &signer, "device", 1)
                .unwrap_err()
                .code,
            "sync_duplicate_identity"
        );
        assert!(engine.sync_outbox().unwrap().is_empty());
        let incoming = signer
            .seal(
                &key,
                &engine.manifest().id,
                &note.id,
                "device",
                1,
                &serde_json::to_vec(&change("remote.md", Some(&bytes), None)).unwrap(),
            )
            .unwrap();
        assert_eq!(
            engine
                .sync_apply_file(&incoming, &key, &signer.public_key())
                .unwrap(),
            ApplyOutcome::Conflict
        );
        assert!(!engine.root.join("remote.md").exists());
        assert_eq!(
            std::fs::read(engine.root.join("original.md")).unwrap(),
            bytes
        );
    }

    #[cfg(unix)]
    #[test]
    fn identity_scan_does_not_reinterpret_literal_backslashes_as_directories() {
        let (_dir, engine) = engine();
        std::fs::write(engine.root.join("folder\\note.md"), b"literal filename").unwrap();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        let operation = seal(
            &engine,
            &signer,
            &key,
            &change("incoming.md", Some(b"new"), None),
        );
        assert_eq!(
            engine
                .sync_apply_file(&operation, &key, &signer.public_key())
                .unwrap_err()
                .code,
            "sync_unsupported_path"
        );
        assert!(!engine.root.join("incoming.md").exists());
        assert_eq!(engine.sync_cursor().unwrap(), "0");
    }

    #[cfg(unix)]
    #[test]
    fn sync_paths_reject_special_files_and_canonical_internal_targets() {
        let (_dir, engine) = engine();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(engine.root.join("pipe.md"))
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            engine.sync_path("pipe.md").unwrap_err().code,
            "sync_unsupported_path"
        );
        assert_eq!(
            engine
                .sync_file_path(".noura/sync/state.json")
                .unwrap_err()
                .code,
            "sync_unsafe_path"
        );
        assert_eq!(
            engine.sync_file_path("workspace.yaml").unwrap_err().code,
            "sync_unsafe_path"
        );
    }

    #[test]
    fn restart_retains_ordered_ciphertext_and_external_edits() {
        let (dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        std::fs::write(engine.root.join("note.md"), b"first").unwrap();
        let first = engine
            .sync_capture_file("note.md", &key, &signer, "device", 1)
            .unwrap()
            .unwrap();
        std::fs::write(engine.root.join("note.md"), b"external edit").unwrap();
        let second = engine
            .sync_capture_file("note.md", &key, &signer, "device", 1)
            .unwrap()
            .unwrap();
        assert_eq!(first.object_id, second.object_id);
        let root = engine.root.clone();
        drop(engine);
        let reopened = WorkspaceEngine::open_with_app_data(root, dir.path().join("app")).unwrap();
        assert_eq!(
            reopened.sync_outbox().unwrap(),
            vec![first.clone(), second.clone()]
        );
        reopened.sync_acknowledge(&first, "1").unwrap();
        assert_eq!(reopened.sync_outbox().unwrap(), vec![second]);
        assert!(
            reopened
                .sync_capture_file("note.md", &key, &signer, "device", 1)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cursor_requires_durable_apply_and_replay_does_not_overwrite_external_edit() {
        let (_dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        let op = seal(
            &engine,
            &signer,
            &key,
            &change("note.md", Some(b"remote"), None),
        );
        assert!(
            engine
                .sync_checkpoint("0", "1", std::slice::from_ref(&op))
                .is_err()
        );
        assert_eq!(engine.sync_cursor().unwrap(), "0");
        assert_eq!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .unwrap(),
            ApplyOutcome::Applied
        );
        std::fs::write(engine.root.join("note.md"), b"external edit").unwrap();
        assert_eq!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(
            std::fs::read(engine.root.join("note.md")).unwrap(),
            b"external edit"
        );
        engine.sync_checkpoint("0", "1", &[op]).unwrap();
        assert_eq!(engine.sync_cursor().unwrap(), "1");
        assert!(engine.sync_checkpoint("0", "2", &[]).is_err());
    }

    #[test]
    fn delete_versus_external_edit_preserves_both_versions_and_completes_page() {
        let (_dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        let first = seal(
            &engine,
            &signer,
            &key,
            &change("note.md", Some(b"base"), None),
        );
        engine
            .sync_apply_file(&first, &key, &signer.public_key())
            .unwrap();
        std::fs::write(engine.root.join("note.md"), b"edited locally").unwrap();
        let deletion = seal(
            &engine,
            &signer,
            &key,
            &change("note.md", None, Some(b"base")),
        );
        assert_eq!(
            engine
                .sync_apply_file(&deletion, &key, &signer.public_key())
                .unwrap(),
            ApplyOutcome::Conflict
        );
        assert_eq!(
            std::fs::read(engine.root.join("note.md")).unwrap(),
            b"edited locally"
        );
        assert!(
            engine
                .root
                .join(format!(
                    ".noura/sync/conflicts/{}.json",
                    deletion.operation_id
                ))
                .is_file()
        );
        engine
            .sync_checkpoint("0", "2", &[first, deletion])
            .unwrap();
    }

    #[test]
    fn interrupted_move_replays_after_destination_write_and_after_source_removal() {
        for remove_source in [false, true] {
            let (_dir, engine) = engine();
            let key = ObjectKey::generate();
            let signer = SigningIdentity::generate();
            let first = seal(
                &engine,
                &signer,
                &key,
                &change("old.md", Some(b"base"), None),
            );
            engine
                .sync_apply_file(&first, &key, &signer.public_key())
                .unwrap();
            let mut moved = change("new.md", Some(b"base"), Some(b"base"));
            moved.previous_path = Some("old.md".into());
            let op = seal(&engine, &signer, &key, &moved);
            // Simulate a process interruption after canonical write, before receipt.
            engine
                .sync_write_once(&format!(".noura/sync/inbox/{}.json", op.operation_id), &op)
                .unwrap();
            std::fs::write(engine.root.join("new.md"), b"base").unwrap();
            if remove_source {
                std::fs::remove_file(engine.root.join("old.md")).unwrap();
            }
            assert_eq!(
                engine
                    .sync_apply_file(&op, &key, &signer.public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            assert!(!engine.root.join("old.md").exists());
            assert_eq!(std::fs::read(engine.root.join("new.md")).unwrap(), b"base");
        }
    }

    #[test]
    fn unsafe_paths_and_unbound_updates_cannot_modify_workspace_files() {
        let (_dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        for path in [
            "../escape.md",
            "/absolute.md",
            ".noura/keys",
            "a\\b.md",
            "C:/file",
            "a/../b",
            "a//b",
            "workspace.yaml",
        ] {
            let op = seal(&engine, &signer, &key, &change(path, Some(b"bad"), None));
            assert!(
                engine
                    .sync_apply_file(&op, &key, &signer.public_key())
                    .is_err(),
                "{path}"
            );
        }
        std::fs::write(engine.root.join("private.txt"), b"private").unwrap();
        let op = seal(
            &engine,
            &signer,
            &key,
            &change("private.txt", None, Some(b"private")),
        );
        assert_eq!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .unwrap(),
            ApplyOutcome::Conflict
        );
        assert_eq!(
            std::fs::read(engine.root.join("private.txt")).unwrap(),
            b"private"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_and_parent_symlinks_block_incoming_writes() {
        let (dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        std::os::unix::fs::symlink(dir.path().join("missing"), engine.root.join("link.md"))
            .unwrap();
        std::os::unix::fs::symlink(dir.path(), engine.root.join("outside")).unwrap();
        for path in ["link.md", "outside/escape.md"] {
            let op = seal(&engine, &signer, &key, &change(path, Some(b"bad"), None));
            assert!(
                engine
                    .sync_apply_file(&op, &key, &signer.public_key())
                    .is_err()
            );
        }
        assert!(!dir.path().join("escape.md").exists());
    }

    #[test]
    fn failed_canonical_write_leaves_inbox_and_cursor_without_receipt() {
        let (_dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        std::fs::write(engine.root.join("parent"), b"not a directory").unwrap();
        let op = seal(
            &engine,
            &signer,
            &key,
            &change("parent/note.md", Some(b"remote"), None),
        );
        assert!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .is_err()
        );
        assert!(
            engine
                .root
                .join(format!(".noura/sync/inbox/{}.json", op.operation_id))
                .exists()
        );
        assert!(engine.sync_checkpoint("0", "1", &[op]).is_err());
        assert_eq!(engine.sync_cursor().unwrap(), "0");
    }

    #[test]
    fn malformed_journal_is_rejected_instead_of_resetting_sync_state() {
        let (_dir, engine) = engine();
        std::fs::create_dir_all(engine.root.join(".noura/sync")).unwrap();
        std::fs::write(engine.root.join(STATE_PATH), b"{broken").unwrap();
        assert!(engine.sync_cursor().is_err());
    }
}
