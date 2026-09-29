//! Incoming changes: applying remote operations to workspace files and
//! advancing the pull cursor once each one has a durable receipt.

use super::*;

impl WorkspaceEngine {
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
        let document = Self::sync_document_descriptor(&journal, &op.object_id);
        match document {
            Some(document) if document.mode == DocumentMode::Text => {
                return Err(invalid("collaboration_transaction_required"));
            }
            Some(document)
                if op.version != 2
                    || op.kind != Some(OperationKind::File)
                    || op.generation.as_deref() != Some(&document.generation) =>
            {
                return Err(invalid("collaboration_stale_generation"));
            }
            None if op.version == 2 || op.generation.is_some() || op.kind.is_some() => {
                return Err(invalid("collaboration_document_required"));
            }
            _ => {}
        }
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
        let _ = self.index_outcome(self.reconcile_forced_with_source(
            &std::collections::HashSet::from([change.path]),
            EventSource::Sync,
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

    pub(super) fn sync_apply_change(
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

    pub(super) fn sync_duplicate_identity(&self, id: &str, excluded: &[&str]) -> Result<bool> {
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

    pub(super) fn sync_conflict(
        &self,
        op: &EncryptedOperation,
        change: &FileChange,
    ) -> Result<ApplyOutcome> {
        self.sync_write_once(
            &format!(".noura/sync/conflicts/{}.json", op.operation_id),
            change,
        )?;
        Ok(ApplyOutcome::Conflict)
    }
}
