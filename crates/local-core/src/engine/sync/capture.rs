//! Outgoing changes: capturing local files as encrypted operations and
//! keeping them in the outbox until the server acknowledges them.

use super::*;

impl WorkspaceEngine {
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
        if self.collaboration_capture_external(path, device, secrets)? {
            return Ok(());
        }
        if self.sync_capture_attachment(path, device, secrets)? {
            return Ok(());
        }
        self.sync_capture_with(
            path,
            |workspace, object, is_new, policy_revision, document, plaintext| {
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
                match document {
                    Some(document) if document.mode == DocumentMode::Attachment => {
                        device.signer().seal_for_document(
                            key,
                            workspace,
                            object,
                            device.device_id(),
                            (
                                epoch,
                                policy_revision,
                                &document.generation,
                                OperationKind::File,
                            ),
                            plaintext,
                        )
                    }
                    Some(_) => Err(invalid("collaboration_transaction_required")),
                    None => device.signer().seal_at_revision(
                        key,
                        workspace,
                        object,
                        device.device_id(),
                        (epoch, policy_revision),
                        plaintext,
                    ),
                }
            },
        )?;
        Ok(())
    }

    pub(crate) fn sync_capture_path(
        &self,
        path: &str,
        device: &DeviceKeys,
        secrets: &mut crate::sync::SyncSecrets,
    ) -> Result<()> {
        self.sync_capture_device_path(path, device, secrets)
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
        self.sync_capture_with(
            path,
            |workspace, object, _, policy_revision, document, plaintext| match document {
                Some(document) if document.mode == DocumentMode::Attachment => signer
                    .seal_for_document(
                        key,
                        workspace,
                        object,
                        device,
                        (
                            epoch,
                            policy_revision,
                            &document.generation,
                            OperationKind::File,
                        ),
                        plaintext,
                    ),
                Some(_) => Err(invalid("collaboration_transaction_required")),
                None => signer.seal_at_revision(
                    key,
                    workspace,
                    object,
                    device,
                    (epoch, policy_revision),
                    plaintext,
                ),
            },
        )
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
            |workspace, object, is_new, policy_revision, document, plaintext| {
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
                match document {
                    Some(document) if document.mode == DocumentMode::Attachment => {
                        device.signer().seal_for_document(
                            &key,
                            workspace,
                            object,
                            device.device_id(),
                            (
                                epoch,
                                policy_revision,
                                &document.generation,
                                OperationKind::File,
                            ),
                            plaintext,
                        )
                    }
                    Some(_) => Err(invalid("collaboration_transaction_required")),
                    None => device.signer().seal_at_revision(
                        &key,
                        workspace,
                        object,
                        device.device_id(),
                        (epoch, policy_revision),
                        plaintext,
                    ),
                }
            },
        )
    }

    fn sync_capture_with(
        &self,
        path: &str,
        seal: impl FnOnce(
            &str,
            &str,
            bool,
            &str,
            Option<DocumentDescriptor>,
            &[u8],
        ) -> Result<EncryptedOperation>,
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
        self.collaboration_guard_sync_change(path)?;
        if let Some(previous) = previous {
            self.collaboration_guard_sync_change(&previous.path)?;
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
        let document = Self::sync_document_descriptor(&journal, &object_id);
        let op = seal(
            &journal.workspace_id,
            &object_id,
            previous.is_none(),
            &journal.access_revision,
            document,
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
        self.collaboration_acknowledge(op, sequence)?;
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
        self.sync_write(STATE_PATH, &journal)?;
        if let Some(generation) = &op.generation
            && !journal
                .outbox
                .iter()
                .any(|pending| pending.object_id == op.object_id)
        {
            self.emit(
                "collaboration:status",
                EventSource::Sync,
                serde_json::to_value(crate::sync::collaboration::CollaborationStatusEvent {
                    object_id: op.object_id.clone(),
                    generation: generation.clone(),
                    status: crate::sync::collaboration::CollaborationStatus::Synced,
                })
                .map_err(|_| invalid("sync_serialize_failed"))?,
            );
        }
        Ok(())
    }
}
