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
            let can_resolve = change.blob.is_none()
                && change.previous_path.is_none()
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
        if remote.blob.is_some() {
            return Err(invalid("sync_attachment_resolution_required"));
        }
        if remote.previous_path.is_some() || object.path != remote.path {
            return Err(invalid("sync_move_resolution_required"));
        }
        let current = read_optional(&self.sync_file_path(&remote.path)?)?;
        let revision = current.as_ref().map(|bytes| markdown::revision(bytes));
        let operation = if let Some(intent) = intent {
            intent.operation
        } else {
            if revision != input.current_revision {
                return Err(invalid("sync_file_changed"));
            }
            if revision != object.revision {
                return Err(invalid("sync_capture_required"));
            }
            let incoming = remote
                .content
                .as_ref()
                .map(|content| {
                    STANDARD
                        .decode(content)
                        .map_err(|_| invalid("sync_invalid_content"))
                })
                .transpose()?;
            let remote_revision = incoming.as_ref().map(|bytes| markdown::revision(bytes));
            let mut accepted = vec![revision.clone()];
            if remote_revision != revision {
                accepted.push(remote_revision);
            }
            let chosen = match input.choice {
                SyncResolutionChoice::Local => current.as_ref().map(|bytes| STANDARD.encode(bytes)),
                SyncResolutionChoice::Remote => remote.content.clone(),
            };
            let change = FileChange {
                version: 2,
                path: remote.path.clone(),
                previous_path: None,
                base_revision: revision.clone(),
                content: chosen,
                accepted_revisions: Some(accepted),
                blob: None,
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
        if change.version != 2
            || change.path != remote.path
            || change.base_revision != input.current_revision
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
        // Replay may observe the already committed choice, but never an unrelated later external edit.
        if revision != input.current_revision && current != chosen {
            return Err(invalid("sync_file_changed"));
        }
        if self.sync_apply_change(&operation, &change)? != ApplyOutcome::Applied {
            return Err(invalid("sync_resolution_conflict"));
        }
        journal.objects.insert(
            operation.object_id.clone(),
            ObjectState {
                path: change.path.clone(),
                revision: chosen.as_ref().map(|bytes| markdown::revision(bytes)),
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
}
