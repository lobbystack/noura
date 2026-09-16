use super::*;
use crate::sync::{ResolveSyncConflict, SyncConflict, SyncResolutionChoice, SyncSecrets};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolutionIntent {
    input: ResolveSyncConflict,
    operation: EncryptedOperation,
}

impl WorkspaceEngine {
    pub(super) fn sync_mark_reviewed_conflicts(
        &self,
        journal: &mut Journal,
        object: &str,
        resolution: &FileChange,
    ) -> Result<()> {
        let Some(accepted) = &resolution.accepted_revisions else {
            return Ok(());
        };
        for (id, receipt) in journal
            .receipts
            .iter_mut()
            .filter(|(_, receipt)| receipt.outcome == ApplyOutcome::Conflict)
        {
            let Some(expected) = &receipt.change_digest else {
                continue;
            };
            crate::sync::identifier(id)?;
            let operation_bytes =
                read_optional(&self.sync_path(&format!(".noura/sync/inbox/{id}.json"))?)?
                    .ok_or_else(|| invalid("sync_conflict_missing"))?;
            let operation: EncryptedOperation = serde_json::from_slice(&operation_bytes)
                .map_err(|_| invalid("sync_invalid_conflict"))?;
            if operation.object_id != object {
                continue;
            }
            self.sync_check_workspace(&operation)?;
            if operation.operation_id != *id
                || markdown::revision(
                    &serde_json::to_vec(&operation)
                        .map_err(|_| invalid("sync_serialize_failed"))?,
                ) != receipt.digest
            {
                return Err(invalid("sync_invalid_conflict"));
            }
            let bytes =
                read_optional(&self.sync_path(&format!(".noura/sync/conflicts/{id}.json"))?)?
                    .ok_or_else(|| invalid("sync_conflict_missing"))?;
            if markdown::revision(&bytes) != *expected {
                return Err(invalid("sync_invalid_conflict"));
            }
            let change: FileChange =
                serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_change"))?;
            validate_change(&change)?;
            let revision = change
                .content
                .as_ref()
                .map(|value| {
                    STANDARD
                        .decode(value)
                        .map(|bytes| markdown::revision(&bytes))
                        .map_err(|_| invalid("sync_invalid_content"))
                })
                .transpose()?;
            if change.blob.is_none()
                && change.path == resolution.path
                && change.previous_path.is_none()
                && accepted.contains(&revision)
            {
                receipt.outcome = ApplyOutcome::Resolved;
            }
        }
        Ok(())
    }
    pub fn sync_conflicts(&self, secrets: &SyncSecrets) -> Result<Vec<SyncConflict>> {
        let _lock = self.write_lock("sync_conflicts")?;
        let journal = self.sync_journal()?;
        let mut result = Vec::new();
        for (id, receipt) in journal
            .receipts
            .iter()
            .filter(|(_, receipt)| receipt.outcome == ApplyOutcome::Conflict)
            .take(100)
        {
            let (operation, change) = self.sync_read_conflict(id, receipt, secrets)?;
            let source = change.previous_path.as_deref().unwrap_or(&change.path);
            let local = read_optional(&self.sync_file_path(source)?)?;
            let remote = change
                .content
                .as_ref()
                .map(|value| {
                    STANDARD
                        .decode(value)
                        .map_err(|_| invalid("sync_invalid_content"))
                })
                .transpose()?;
            let revision = local.as_ref().map(|bytes| markdown::revision(bytes));
            // Attachment conflicts share the text safety conditions, but a binary
            // resolution always needs existing local bytes to keep or replace.
            let can_resolve = change.previous_path.is_none()
                && (change.blob.is_none() || local.is_some())
                && journal
                    .objects
                    .get(&operation.object_id)
                    .is_some_and(|object| {
                        object.path == change.path && object.revision == revision
                    });
            result.push(SyncConflict {
                operation_id: id.clone(),
                path: change.path,
                current_revision: revision,
                local_preview: local.as_deref().and_then(preview),
                remote_preview: remote.as_deref().and_then(preview),
                local_deleted: local.is_none(),
                remote_deleted: remote.is_none() && change.blob.is_none(),
                can_resolve,
            });
        }
        Ok(result)
    }

    /// Commit the reviewed canonical choice before publishing a signed resolution in the outbox.
    /// The retained intent resumes the same ciphertext after an interrupted file/journal commit.
    pub fn sync_resolve_conflict(
        &self,
        input: &ResolveSyncConflict,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<()> {
        crate::sync::identifier(&input.operation_id)?;
        let _lock = self.write_lock("sync_resolve_conflict")?;
        let mut journal = self.sync_journal()?;
        let receipt = journal
            .receipts
            .get(&input.operation_id)
            .ok_or_else(|| invalid("sync_conflict_missing"))?;
        let intent_path = format!(".noura/sync/resolutions/{}.json", input.operation_id);
        let intent = read_optional(&self.sync_path(&intent_path)?)?
            .map(|bytes| {
                serde_json::from_slice::<ResolutionIntent>(&bytes)
                    .map_err(|_| invalid("sync_invalid_resolution"))
            })
            .transpose()?;
        if let Some(intent) = &intent {
            if intent.input.choice != input.choice
                || intent.input.operation_id != input.operation_id
                || intent.input.current_revision != input.current_revision
            {
                return Err(invalid("sync_resolution_in_progress"));
            }
            if receipt.outcome == ApplyOutcome::Resolved {
                return Ok(());
            }
        }
        if receipt.outcome != ApplyOutcome::Conflict {
            return Err(invalid("sync_conflict_missing"));
        }
        let (original, remote) = self.sync_read_conflict(&input.operation_id, receipt, secrets)?;
        let object = journal
            .objects
            .get(&original.object_id)
            .ok_or_else(|| invalid("sync_identity_resolution_required"))?;
        if remote.previous_path.is_some() || object.path != remote.path {
            return Err(invalid("sync_move_resolution_required"));
        }
        let current = read_optional(&self.sync_file_path(&remote.path)?)?;
        let revision = current.as_ref().map(|bytes| markdown::revision(bytes));
        let remote_revision = match &remote.blob {
            Some(blob) => Some(blob.revision.clone()),
            None => remote
                .content
                .as_ref()
                .map(|content| {
                    STANDARD
                        .decode(content)
                        .map(|bytes| markdown::revision(&bytes))
                        .map_err(|_| invalid("sync_invalid_content"))
                })
                .transpose()?,
        };
        // A remote attachment resolution installs exact ciphertext. Fail before persisting
        // an intent (or touching canonical bytes) when that ciphertext is not available.
        if input.choice == SyncResolutionChoice::Remote
            && let Some(blob) = &remote.blob
        {
            self.sync_blob_file(blob, false)
                .map_err(|_| invalid("sync_blob_unavailable"))?;
        }
        let operation = if let Some(intent) = intent {
            intent.operation
        } else {
            if revision != input.current_revision {
                return Err(invalid("sync_file_changed"));
            }
            if revision != object.revision {
                return Err(invalid("sync_capture_required"));
            }
            if remote.blob.is_some() && revision.is_none() {
                return Err(invalid("sync_capture_required"));
            }
            let mut accepted = vec![revision.clone()];
            if remote_revision != revision {
                accepted.push(remote_revision);
            }
            let change = if remote.blob.is_some() {
                match input.choice {
                    SyncResolutionChoice::Remote => FileChange {
                        version: 3,
                        path: remote.path.clone(),
                        previous_path: None,
                        base_revision: revision.clone(),
                        content: None,
                        accepted_revisions: None,
                        blob: remote.blob.clone(),
                    },
                    SyncResolutionChoice::Local => FileChange {
                        version: 2,
                        path: remote.path.clone(),
                        previous_path: None,
                        base_revision: revision.clone(),
                        content: current.as_ref().map(|bytes| STANDARD.encode(bytes)),
                        accepted_revisions: Some(accepted),
                        blob: None,
                    },
                }
            } else {
                let chosen = match input.choice {
                    SyncResolutionChoice::Local => {
                        current.as_ref().map(|bytes| STANDARD.encode(bytes))
                    }
                    SyncResolutionChoice::Remote => remote.content.clone(),
                };
                FileChange {
                    version: 2,
                    path: remote.path.clone(),
                    previous_path: None,
                    base_revision: revision.clone(),
                    content: chosen,
                    accepted_revisions: Some(accepted),
                    blob: None,
                }
            };
            validate_change(&change)?;
            let ((_, epoch), key) = secrets
                .objects
                .iter()
                .filter(|((id, _), _)| id == &original.object_id)
                .max_by_key(|((_, epoch), _)| *epoch)
                .ok_or_else(|| invalid("sync_key_required"))?;
            let plaintext = zeroize::Zeroizing::new(
                serde_json::to_vec(&change).map_err(|_| invalid("sync_serialize_failed"))?,
            );
            let operation = device.signer().seal_at_revision(
                key,
                &journal.workspace_id,
                &original.object_id,
                device.device_id(),
                (*epoch, &journal.access_revision),
                &plaintext,
            )?;
            self.sync_write_once(
                &intent_path,
                &ResolutionIntent {
                    input: input.clone(),
                    operation: operation.clone(),
                },
            )?;
            operation
        };
        if operation.device_id != device.device_id()
            || operation.object_id != original.object_id
            || operation.workspace_id != journal.workspace_id
        {
            return Err(invalid("sync_invalid_resolution"));
        }
        let key = secrets
            .objects
            .get(&(operation.object_id.clone(), operation.epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let plaintext = operation.open(key, &device.signer().public_key())?;
        let change: FileChange =
            serde_json::from_slice(&plaintext).map_err(|_| invalid("sync_invalid_resolution"))?;
        validate_change(&change)?;
        // A resumed intent must still describe the reviewed branch: the same attachment
        // manifest for a remote binary choice, and text for every other choice.
        let expected_attachment =
            input.choice == SyncResolutionChoice::Remote && remote.blob.is_some();
        if change.version != if expected_attachment { 3 } else { 2 }
            || change.path != remote.path
            || change.base_revision != input.current_revision
            || (expected_attachment && change.blob.as_ref() != remote.blob.as_ref())
            || (!expected_attachment && change.blob.is_some())
        {
            return Err(invalid("sync_invalid_resolution"));
        }
        let chosen = change
            .content
            .as_ref()
            .map(|content| {
                STANDARD
                    .decode(content)
                    .map_err(|_| invalid("sync_invalid_content"))
            })
            .transpose()?;
        let chosen_revision = match &change.blob {
            Some(blob) => Some(blob.revision.clone()),
            None => chosen.as_ref().map(|bytes| markdown::revision(bytes)),
        };
        // Replay may observe the already materialized attachment or committed choice,
        // but never an unrelated later external edit.
        if revision != input.current_revision
            && chosen_revision
                .as_ref()
                .is_none_or(|chosen| revision.as_ref() != Some(chosen))
            && current != chosen
        {
            return Err(invalid("sync_file_changed"));
        }
        let outcome = if change.blob.is_some() {
            self.sync_apply_attachment(&operation, &change, key)?
        } else {
            self.sync_apply_change(&operation, &change)?
        };
        if outcome != ApplyOutcome::Applied {
            return Err(invalid("sync_resolution_conflict"));
        }
        journal.objects.insert(
            operation.object_id.clone(),
            ObjectState {
                path: change.path.clone(),
                revision: chosen_revision,
            },
        );
        if !journal
            .outbox
            .iter()
            .any(|pending| pending.operation_id == operation.operation_id)
        {
            journal.outbox.push(operation);
        }
        journal
            .receipts
            .get_mut(&input.operation_id)
            .ok_or_else(|| invalid("sync_conflict_missing"))?
            .outcome = ApplyOutcome::Resolved;
        self.sync_mark_reviewed_conflicts(&mut journal, &original.object_id, &change)?;
        self.sync_write(STATE_PATH, &journal)?;
        drop(_lock);
        let _ =
            self.index_outcome(self.reconcile_forced_with_source(
                &std::collections::HashSet::from([change.path]),
                "sync",
            ));
        Ok(())
    }

    fn sync_read_conflict(
        &self,
        id: &str,
        receipt: &Receipt,
        secrets: &SyncSecrets,
    ) -> Result<(EncryptedOperation, FileChange)> {
        crate::sync::identifier(id)?;
        let bytes = read_optional(&self.sync_path(&format!(".noura/sync/inbox/{id}.json"))?)?
            .ok_or_else(|| invalid("sync_conflict_missing"))?;
        let operation: EncryptedOperation =
            serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_conflict"))?;
        if operation.operation_id != id
            || markdown::revision(
                &serde_json::to_vec(&operation).map_err(|_| invalid("sync_serialize_failed"))?,
            ) != receipt.digest
        {
            return Err(invalid("sync_invalid_conflict"));
        }
        self.sync_check_workspace(&operation)?;
        let key = secrets
            .objects
            .get(&(operation.object_id.clone(), operation.epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let signer = secrets
            .trusted_devices
            .get(&operation.device_id)
            .ok_or_else(|| invalid("sync_untrusted_device"))?;
        let plaintext = operation.open(key, signer)?;
        let change: FileChange =
            serde_json::from_slice(&plaintext).map_err(|_| invalid("sync_invalid_change"))?;
        validate_change(&change)?;
        Ok((operation, change))
    }
}

fn preview(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.len() <= 65_536 {
        return Some(text.into());
    }
    let mut end = 65_536;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}\n[Preview truncated]", &text[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
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
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(reference.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn reviewed_resolution_survives_interrupted_journal_commit_and_preserves_later_edits() {
        for choice in [SyncResolutionChoice::Local, SyncResolutionChoice::Remote] {
            let chosen: &[u8] = match choice {
                SyncResolutionChoice::Local => b"local",
                SyncResolutionChoice::Remote => b"remote",
            };
            let directory = tempfile::TempDir::new().unwrap();
            let left = WorkspaceEngine::create_with_app_data(
                directory.path().join("left"),
                "Left",
                directory.path().join("app_left"),
            )
            .unwrap();
            let workspace = left.manifest().id;
            let right = WorkspaceEngine::create_sync_replica(
                directory.path().join("right"),
                "Right",
                &workspace,
                directory.path().join("app_right"),
            )
            .unwrap();
            let later = WorkspaceEngine::create_sync_replica(
                directory.path().join("later"),
                "Later",
                &workspace,
                directory.path().join("app_later"),
            )
            .unwrap();
            let credentials = Memory::default();
            let local = DeviceKeys::create(&credentials).unwrap();
            let remote = DeviceKeys::create(&credentials).unwrap();
            let key = ObjectKey::from_bytes([42; 32]);
            let seal = |bytes: &[u8], base: Option<&[u8]>| {
                remote
                    .signer()
                    .seal(
                        &key,
                        &workspace,
                        "object",
                        remote.device_id(),
                        1,
                        &serde_json::to_vec(&FileChange {
                            version: 1,
                            path: "file.txt".into(),
                            previous_path: None,
                            base_revision: base.map(markdown::revision),
                            content: Some(STANDARD.encode(bytes)),
                            accepted_revisions: None,
                            blob: None,
                        })
                        .unwrap(),
                    )
                    .unwrap()
            };
            let base = seal(b"base", None);
            for engine in [&left, &right, &later] {
                assert_eq!(
                    engine
                        .sync_apply_file(&base, &key, &remote.signer().public_key())
                        .unwrap(),
                    ApplyOutcome::Applied
                );
            }
            std::fs::write(left.root().join("file.txt"), b"local").unwrap();
            left.sync_capture_file("file.txt", &key, local.signer(), local.device_id(), 1)
                .unwrap();
            let incoming = seal(b"remote", Some(b"base"));
            assert_eq!(
                left.sync_apply_file(&incoming, &key, &remote.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Conflict
            );
            assert_eq!(
                right
                    .sync_apply_file(&incoming, &key, &remote.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            let secrets = SyncSecrets {
                objects: BTreeMap::from([(("object".into(), 1), ObjectKey::from_bytes([42; 32]))]),
                trusted_devices: BTreeMap::from([
                    (local.device_id().into(), local.signer().public_key()),
                    (remote.device_id().into(), remote.signer().public_key()),
                ]),
                authorized_workspace_writers: Default::default(),
                authorized_object_writers: Default::default(),
                ..Default::default()
            };
            let conflicts = left.sync_conflicts(&secrets).unwrap();
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].local_preview.as_deref(), Some("local"));
            assert!(conflicts[0].can_resolve);
            let input = ResolveSyncConflict {
                operation_id: incoming.operation_id.clone(),
                current_revision: conflicts[0].current_revision.clone(),
                choice,
            };
            std::fs::write(left.root().join("file.txt"), b"new external edit").unwrap();
            assert_eq!(
                left.sync_resolve_conflict(&input, &local, &secrets)
                    .unwrap_err()
                    .code,
                "sync_file_changed"
            );
            assert_eq!(
                std::fs::read(left.root().join("file.txt")).unwrap(),
                b"new external edit"
            );
            std::fs::write(left.root().join("file.txt"), b"local").unwrap();
            let journal_before = std::fs::read(left.root().join(STATE_PATH)).unwrap();
            left.sync_resolve_conflict(&input, &local, &secrets)
                .unwrap();
            assert_eq!(std::fs::read(left.root().join("file.txt")).unwrap(), chosen);
            let outbox = left.sync_outbox().unwrap();
            let resolution = outbox.last().unwrap();
            assert_eq!(outbox.len(), 2);
            // Model process exit after the canonical write but before publishing its journal update.
            std::fs::write(left.root().join(STATE_PATH), journal_before).unwrap();
            left.sync_resolve_conflict(&input, &local, &secrets)
                .unwrap();
            assert_eq!(
                left.sync_outbox().unwrap().last().unwrap().operation_id,
                resolution.operation_id
            );
            assert_eq!(left.sync_status().unwrap().conflicts, 0);
            assert_eq!(
                right
                    .sync_apply_file(resolution, &key, &local.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            assert_eq!(
                std::fs::read(right.root().join("file.txt")).unwrap(),
                chosen
            );
            std::fs::write(later.root().join("file.txt"), b"unreviewed later edit").unwrap();
            assert_eq!(
                later
                    .sync_apply_file(resolution, &key, &local.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Conflict
            );
            assert_eq!(
                std::fs::read(later.root().join("file.txt")).unwrap(),
                b"unreviewed later edit"
            );
            std::fs::write(left.root().join("file.txt"), b"edit after resolution").unwrap();
            left.sync_resolve_conflict(&input, &local, &secrets)
                .unwrap();
            assert_eq!(
                std::fs::read(left.root().join("file.txt")).unwrap(),
                b"edit after resolution"
            );
            assert_eq!(
                left.sync_apply_file(&incoming, &key, &remote.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Resolved
            );
        }
    }

    struct AttachmentConflict {
        _directory: tempfile::TempDir,
        engine: WorkspaceEngine,
        peer: WorkspaceEngine,
        local: DeviceKeys,
        secrets: SyncSecrets,
        key: ObjectKey,
        operation: EncryptedOperation,
        blob: crate::sync::EncryptedBlob,
    }

    /// Build a replica whose local attachment diverged from an incoming version-3 blob.
    fn attachment_conflict(
        path: &str,
        object: &str,
        previous_path: Option<&str>,
    ) -> AttachmentConflict {
        let directory = tempfile::TempDir::new().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("left"),
            "Left",
            directory.path().join("app_left"),
        )
        .unwrap();
        let workspace = engine.manifest().id;
        let peer = WorkspaceEngine::create_sync_replica(
            directory.path().join("right"),
            "Right",
            &workspace,
            directory.path().join("app_right"),
        )
        .unwrap();
        let credentials = Memory::default();
        let local = DeviceKeys::create(&credentials).unwrap();
        let remote = DeviceKeys::create(&credentials).unwrap();
        let key = ObjectKey::from_bytes([7; 32]);
        let base = remote
            .signer()
            .seal(
                &key,
                &workspace,
                object,
                remote.device_id(),
                1,
                &serde_json::to_vec(&FileChange {
                    version: 1,
                    path: path.into(),
                    previous_path: None,
                    base_revision: None,
                    content: Some(STANDARD.encode(b"base")),
                    accepted_revisions: None,
                    blob: None,
                })
                .unwrap(),
            )
            .unwrap();
        for replica in [&engine, &peer] {
            assert_eq!(
                replica
                    .sync_apply_file(&base, &key, &remote.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
        }
        std::fs::write(engine.root().join(path), b"local").unwrap();
        engine
            .sync_capture_file(path, &key, local.signer(), local.device_id(), 1)
            .unwrap();
        let mut ciphertext = Vec::new();
        let blob = crate::sync::EncryptedBlob::encrypt(
            &key,
            b"remote attachment".as_slice(),
            &mut ciphertext,
        )
        .unwrap();
        for replica in [&engine, &peer] {
            let mut file = replica.sync_blob_file(&blob, true).unwrap();
            std::io::Write::write_all(&mut file, &ciphertext).unwrap();
            file.sync_all().unwrap();
        }
        let operation = remote
            .signer()
            .seal(
                &key,
                &workspace,
                object,
                remote.device_id(),
                1,
                &serde_json::to_vec(&FileChange {
                    version: 3,
                    path: path.into(),
                    previous_path: previous_path.map(str::to_owned),
                    base_revision: Some(markdown::revision(b"base")),
                    content: None,
                    accepted_revisions: None,
                    blob: Some(blob.clone()),
                })
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            engine
                .sync_apply_file(&operation, &key, &remote.signer().public_key())
                .unwrap(),
            ApplyOutcome::Conflict
        );
        if previous_path.is_none() {
            assert_eq!(
                peer.sync_apply_file(&operation, &key, &remote.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
        }
        let secrets = SyncSecrets {
            objects: BTreeMap::from([((object.to_string(), 1), ObjectKey::from_bytes([7; 32]))]),
            trusted_devices: BTreeMap::from([
                (local.device_id().into(), local.signer().public_key()),
                (remote.device_id().into(), remote.signer().public_key()),
            ]),
            ..Default::default()
        };
        AttachmentConflict {
            _directory: directory,
            engine,
            peer,
            local,
            secrets,
            key,
            operation,
            blob,
        }
    }

    #[test]
    fn attachment_resolution_keeps_or_replaces_bytes_and_survives_interruption() {
        for choice in [SyncResolutionChoice::Local, SyncResolutionChoice::Remote] {
            let fixture = attachment_conflict("attachment.bin", "object", None);
            let AttachmentConflict {
                engine: left,
                peer,
                local,
                secrets,
                key,
                operation,
                blob,
                ..
            } = fixture;
            let expected: &[u8] = match choice {
                SyncResolutionChoice::Local => b"local",
                SyncResolutionChoice::Remote => b"remote attachment",
            };
            let conflicts = left.sync_conflicts(&secrets).unwrap();
            assert_eq!(conflicts.len(), 1);
            assert!(conflicts[0].can_resolve);
            assert_eq!(conflicts[0].local_preview.as_deref(), Some("local"));
            assert!(conflicts[0].remote_preview.is_none());
            assert_eq!(
                conflicts[0].current_revision.as_deref(),
                Some(markdown::revision(b"local").as_str())
            );
            let input = ResolveSyncConflict {
                operation_id: operation.operation_id.clone(),
                current_revision: conflicts[0].current_revision.clone(),
                choice,
            };
            let journal_before = std::fs::read(left.root().join(STATE_PATH)).unwrap();
            left.sync_resolve_conflict(&input, &local, &secrets)
                .unwrap();
            assert_eq!(
                std::fs::read(left.root().join("attachment.bin")).unwrap(),
                expected
            );
            let outbox = left.sync_outbox().unwrap();
            assert_eq!(outbox.len(), 2);
            let resolution = outbox.last().unwrap().clone();
            let change: FileChange = serde_json::from_slice(
                &resolution.open(&key, &local.signer().public_key()).unwrap(),
            )
            .unwrap();
            match choice {
                SyncResolutionChoice::Local => {
                    assert_eq!(change.version, 2);
                    assert!(change.blob.is_none());
                    assert_eq!(
                        change.content.as_deref(),
                        Some(STANDARD.encode(b"local").as_str())
                    );
                }
                SyncResolutionChoice::Remote => {
                    assert_eq!(change.version, 3);
                    assert!(change.content.is_none());
                    assert_eq!(change.blob.as_ref(), Some(&blob));
                }
            }
            assert_eq!(left.sync_status().unwrap().conflicts, 0);
            // Model process exit after the canonical commit but before its journal update.
            std::fs::write(left.root().join(STATE_PATH), journal_before).unwrap();
            left.sync_resolve_conflict(&input, &local, &secrets)
                .unwrap();
            assert_eq!(
                left.sync_outbox().unwrap().last().unwrap().operation_id,
                resolution.operation_id
            );
            assert_eq!(left.sync_status().unwrap().conflicts, 0);
            assert_eq!(
                std::fs::read(left.root().join("attachment.bin")).unwrap(),
                expected
            );
            // A peer that already held the remote blob converges to the reviewed branch.
            assert_eq!(
                peer.sync_apply_file(&resolution, &key, &local.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            assert_eq!(
                std::fs::read(peer.root().join("attachment.bin")).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn attachment_resolution_without_local_ciphertext_errors_without_mutation() {
        let fixture = attachment_conflict("attachment.bin", "object", None);
        let AttachmentConflict {
            engine: left,
            local,
            secrets,
            operation,
            blob,
            ..
        } = fixture;
        let conflicts = left.sync_conflicts(&secrets).unwrap();
        let input = ResolveSyncConflict {
            operation_id: operation.operation_id.clone(),
            current_revision: conflicts[0].current_revision.clone(),
            choice: SyncResolutionChoice::Remote,
        };
        std::fs::remove_file(
            left.sync_path(&format!(".noura/sync/blobs/{}", blob.id))
                .unwrap(),
        )
        .unwrap();
        let before = std::fs::read(left.root().join("attachment.bin")).unwrap();
        let outbox_before = left.sync_outbox().unwrap().len();
        let error = left
            .sync_resolve_conflict(&input, &local, &secrets)
            .unwrap_err();
        assert_eq!(error.code, "sync_blob_unavailable");
        assert_eq!(
            std::fs::read(left.root().join("attachment.bin")).unwrap(),
            before
        );
        assert_eq!(left.sync_outbox().unwrap().len(), outbox_before);
        assert_eq!(left.sync_status().unwrap().conflicts, 1);
        assert!(
            !left
                .sync_path(&format!(
                    ".noura/sync/resolutions/{}.json",
                    operation.operation_id
                ))
                .unwrap()
                .exists()
        );
    }

    struct UnknownConflict {
        _directory: tempfile::TempDir,
        engine: WorkspaceEngine,
        secrets: SyncSecrets,
    }

    /// A version-3 update for an object this replica has never cataloged.
    fn unknown_attachment_conflict() -> UnknownConflict {
        let directory = tempfile::TempDir::new().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("left"),
            "Left",
            directory.path().join("app_left"),
        )
        .unwrap();
        let workspace = engine.manifest().id;
        let credentials = Memory::default();
        let local = DeviceKeys::create(&credentials).unwrap();
        let remote = DeviceKeys::create(&credentials).unwrap();
        let key = ObjectKey::from_bytes([9; 32]);
        std::fs::write(engine.root().join("unknown.bin"), b"local").unwrap();
        let mut ciphertext = Vec::new();
        let blob = crate::sync::EncryptedBlob::encrypt(&key, b"remote".as_slice(), &mut ciphertext)
            .unwrap();
        {
            let mut file = engine.sync_blob_file(&blob, true).unwrap();
            std::io::Write::write_all(&mut file, &ciphertext).unwrap();
            file.sync_all().unwrap();
        }
        let operation = remote
            .signer()
            .seal(
                &key,
                &workspace,
                "stranger",
                remote.device_id(),
                1,
                &serde_json::to_vec(&FileChange {
                    version: 3,
                    path: "unknown.bin".into(),
                    previous_path: None,
                    base_revision: Some(markdown::revision(b"base")),
                    content: None,
                    accepted_revisions: None,
                    blob: Some(blob),
                })
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            engine
                .sync_apply_file(&operation, &key, &remote.signer().public_key())
                .unwrap(),
            ApplyOutcome::Conflict
        );
        let secrets = SyncSecrets {
            objects: BTreeMap::from([(
                ("stranger".to_string(), 1),
                ObjectKey::from_bytes([9; 32]),
            )]),
            trusted_devices: BTreeMap::from([
                (local.device_id().into(), local.signer().public_key()),
                (remote.device_id().into(), remote.signer().public_key()),
            ]),
            ..Default::default()
        };
        UnknownConflict {
            _directory: directory,
            engine,
            secrets,
        }
    }

    #[test]
    fn attachment_conflict_can_resolve_only_for_supported_shapes() {
        let supported = attachment_conflict("attachment.bin", "object", None);
        let supported_id = supported.operation.operation_id.clone();
        let conflicts = supported.engine.sync_conflicts(&supported.secrets).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].operation_id, supported_id);
        assert!(conflicts[0].can_resolve);

        let missing = attachment_conflict("attachment.bin", "object", None);
        let missing_id = missing.operation.operation_id.clone();
        std::fs::remove_file(missing.engine.root().join("attachment.bin")).unwrap();
        let conflicts = missing.engine.sync_conflicts(&missing.secrets).unwrap();
        let conflict = conflicts
            .iter()
            .find(|conflict| conflict.operation_id == missing_id)
            .unwrap();
        assert!(!conflict.can_resolve);

        let moved = attachment_conflict("attachment.bin", "object", Some("other.bin"));
        let moved_id = moved.operation.operation_id.clone();
        let conflicts = moved.engine.sync_conflicts(&moved.secrets).unwrap();
        let conflict = conflicts
            .iter()
            .find(|conflict| conflict.operation_id == moved_id)
            .unwrap();
        assert!(!conflict.can_resolve);

        let unknown = unknown_attachment_conflict();
        let conflicts = unknown.engine.sync_conflicts(&unknown.secrets).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert!(!conflicts[0].can_resolve);
    }
}
