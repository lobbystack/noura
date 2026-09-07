use std::io::{Read, Seek};

use super::*;
use crate::sync::{EncryptedBlob, SyncSecrets};

impl WorkspaceEngine {
    pub(crate) fn sync_blob_file(&self, blob: &EncryptedBlob, create: bool) -> Result<File> {
        blob.validate()?;
        let relative = format!(".noura/sync/blobs/{}", blob.id);
        self.sync_prepare_parent(&relative)?;
        let path = self.sync_path(&relative)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(create).create(create);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|_| invalid("sync_blob_read_failed"))?;
        if create {
            sync_parent(&path, "sync_blob")?;
        }
        Ok(file)
    }

    pub(super) fn sync_capture_attachment(
        &self,
        path: &str,
        device: &DeviceKeys,
        secrets: &mut SyncSecrets,
    ) -> Result<bool> {
        validate_change(&FileChange {
            version: 1,
            path: path.into(),
            previous_path: None,
            base_revision: None,
            content: None,
            accepted_revisions: None,
            blob: None,
        })?;
        let source = self.sync_file_path(path)?;
        let metadata = match source.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(invalid("sync_file_read_failed")),
        };
        if metadata.len() <= 700 * 1024 {
            return Ok(false);
        }
        if metadata.len() >= crate::sync::blobs::MAX_BLOB_BYTES {
            return Err(invalid("sync_blob_too_large"));
        }
        let _lock = self.write_lock("sync_capture_attachment")?;
        let revision = file_revision(&source)?.ok_or_else(|| invalid("sync_file_changed"))?;
        let managed = managed_identity(path, &source)?;
        if let Some(id) = &managed
            && self.sync_duplicate_identity(id, &[path])?
        {
            return Err(invalid("sync_duplicate_identity"));
        }
        let mut journal = self.sync_journal()?;
        let known = journal.objects.iter().find(|(_, state)| state.path == path);
        if let (Some(id), Some((known, _))) = (&managed, known)
            && id != known
        {
            return Err(invalid("sync_identity_changed"));
        }
        let mut moved = None;
        if managed.is_none() && known.is_none() {
            for (id, state) in &journal.objects {
                if state.revision.as_ref() == Some(&revision)
                    && !self.sync_file_path(&state.path)?.exists()
                {
                    if moved.is_some() {
                        return Err(invalid("sync_ambiguous_move"));
                    }
                    moved = Some(id.clone());
                }
            }
        }
        let object = managed
            .or_else(|| known.map(|(id, _)| id.clone()))
            .or(moved)
            .unwrap_or_else(|| format!("file_{}", ulid::Ulid::new().to_string().to_lowercase()));
        let previous = journal.objects.get(&object);
        if previous
            .is_some_and(|state| state.path == path && state.revision.as_ref() == Some(&revision))
        {
            return Ok(true);
        }
        self.collaboration_guard_file_mutation(path)?;
        if let Some(previous) = previous {
            self.collaboration_guard_file_mutation(&previous.path)?;
        }
        if let Some(previous) = previous
            && previous.path != path
            && self.sync_file_path(&previous.path)?.exists()
        {
            return Err(invalid("sync_duplicate_identity"));
        }
        let epoch = secrets
            .objects
            .keys()
            .filter(|(id, _)| id == &object)
            .map(|(_, epoch)| *epoch)
            .max();
        let epoch = match epoch {
            Some(epoch) => epoch,
            None if previous.is_none() => {
                let key = ObjectKey::generate();
                let envelope = device.wrap_key(
                    &journal.workspace_id,
                    &object,
                    1,
                    device.device_id(),
                    &device.recipient(),
                    &key,
                )?;
                self.sync_write_once(&key_path(&object, 1, device.device_id())?, &envelope)?;
                secrets.objects.insert((object.clone(), 1), key);
                1
            }
            None => return Err(invalid("sync_key_required")),
        };
        let key = secrets
            .objects
            .get(&(object.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let temporary = format!(".noura/sync/blobs/pending_{}", uuid::Uuid::new_v4());
        self.sync_prepare_parent(&temporary)?;
        let temporary_path = self.sync_path(&temporary)?;
        let mut output = atomic_write_file::AtomicWriteFile::open(&temporary_path)
            .map_err(|_| invalid("sync_blob_write_failed"))?;
        let mut input = open_for_durable_read(&self.sync_file_path(path)?)
            .map_err(|_| invalid("sync_file_read_failed"))?;
        input
            .sync_all()
            .map_err(|_| invalid("sync_file_read_failed"))?;
        let blob = EncryptedBlob::encrypt(key, &mut input, &mut output)?;
        if blob.revision != revision
            || file_revision(&self.sync_file_path(path)?)?.as_ref() != Some(&revision)
        {
            return Err(invalid("sync_file_changed"));
        }
        output
            .sync_all()
            .map_err(|_| invalid("sync_blob_write_failed"))?;
        output
            .commit()
            .map_err(|_| invalid("sync_blob_write_failed"))?;
        let final_path = self.sync_path(&format!(".noura/sync/blobs/{}", blob.id))?;
        std::fs::rename(&temporary_path, &final_path)
            .map_err(|_| invalid("sync_blob_write_failed"))?;
        sync_parent(&final_path, "sync_blob")?;
        let change = FileChange {
            version: 3,
            path: path.into(),
            previous_path: previous
                .filter(|state| state.path != path)
                .map(|state| state.path.clone()),
            base_revision: previous.and_then(|state| state.revision.clone()),
            content: None,
            accepted_revisions: None,
            blob: Some(blob),
        };
        validate_change(&change)?;
        let plaintext = zeroize::Zeroizing::new(
            serde_json::to_vec(&change).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        let operation = device.signer().seal_at_revision(
            key,
            &journal.workspace_id,
            &object,
            device.device_id(),
            (epoch, &journal.access_revision),
            &plaintext,
        )?;
        journal.outbox.push(operation);
        journal.objects.insert(
            object,
            ObjectState {
                path: path.into(),
                revision: Some(revision),
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        Ok(true)
    }

    pub(super) fn sync_apply_attachment(
        &self,
        op: &EncryptedOperation,
        change: &FileChange,
        key: &ObjectKey,
    ) -> Result<ApplyOutcome> {
        let blob = change
            .blob
            .as_ref()
            .ok_or_else(|| invalid("sync_invalid_blob"))?;
        let mut input = self.sync_blob_file(blob, false)?;
        blob.verify(&mut input)?;
        input
            .rewind()
            .map_err(|_| invalid("sync_blob_read_failed"))?;
        let source_path = change.previous_path.as_deref().unwrap_or(&change.path);
        let source = self.sync_file_path(source_path)?;
        let destination = self.sync_file_path(&change.path)?;
        let current = file_revision(&source)?;
        let target = if source == destination {
            current.clone()
        } else {
            file_revision(&destination)?
        };
        if self.sync_duplicate_identity(&op.object_id, &[source_path, &change.path])?
            || managed_identity(source_path, &source)?.is_some_and(|id| id != op.object_id)
        {
            return self.sync_conflict(op, change);
        }
        let already_written = target.as_ref() == Some(&blob.revision);
        if already_written
            && managed_identity(&change.path, &destination)?.is_some_and(|id| id != op.object_id)
        {
            return self.sync_conflict(op, change);
        }
        if (current != change.base_revision
            && !(already_written && (source == destination || current.is_none())))
            || (source != destination && target.is_some() && !already_written)
        {
            return self.sync_conflict(op, change);
        }
        if !already_written {
            self.sync_prepare_parent(&change.path)?;
            let mut output = atomic_write_file::AtomicWriteFile::options()
                .read(true)
                .open(&destination)
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            blob.decrypt(key, &mut input, &mut output)?;
            output
                .sync_all()
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            // The native canonical parser remains authoritative for managed Markdown identity.
            if managed_reader(&change.path, &mut output)?.is_some_and(|id| id != op.object_id) {
                return self.sync_conflict(op, change);
            }
            if file_revision(&self.sync_file_path(&change.path)?)? != target {
                return self.sync_conflict(op, change);
            }
            output
                .commit()
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            sync_parent(&destination, "sync_apply")?;
        }
        if source != destination && current.is_some() {
            if file_revision(&self.sync_file_path(source_path)?)? != current {
                return self.sync_conflict(op, change);
            }
            std::fs::remove_file(&source).map_err(|_| invalid("sync_blob_write_failed"))?;
            sync_parent(&source, "sync_apply")?;
        }
        Ok(ApplyOutcome::Applied)
    }
}

pub(super) fn file_revision(path: &Path) -> Result<Option<String>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid("sync_file_read_failed")),
    };
    let mut hash = blake3::Hasher::new();
    hash.update_reader(&mut file)
        .map_err(|_| invalid("sync_file_read_failed"))?;
    Ok(Some(hash.finalize().to_hex().to_string()))
}
fn managed_identity(path: &str, source: &Path) -> Result<Option<String>> {
    let mut file = match File::open(source) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid("sync_file_read_failed")),
    };
    managed_reader(path, &mut file)
}
fn managed_reader(path: &str, reader: &mut (impl Read + Seek)) -> Result<Option<String>> {
    reader
        .rewind()
        .map_err(|_| invalid("sync_blob_read_failed"))?;
    let mut prefix = Vec::with_capacity(5);
    reader
        .take(5)
        .read_to_end(&mut prefix)
        .map_err(|_| invalid("sync_blob_read_failed"))?;
    if !(prefix.starts_with(b"---\n") || prefix.starts_with(b"---\r\n")) {
        return Ok(None);
    }
    reader
        .rewind()
        .map_err(|_| invalid("sync_blob_read_failed"))?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    reader
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("sync_blob_read_failed"))?;
    Ok(match markdown::parse_markdown(path, &bytes) {
        ParsedMarkdown::Managed(object) => Some(object.id),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncCredentials;
    use std::cell::RefCell;
    use zeroize::Zeroizing;

    #[test]
    fn managed_attachment_identity_survives_short_reads() {
        struct ShortReader(std::io::Cursor<Vec<u8>>);
        impl Read for ShortReader {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let length = bytes.len().min(1);
                self.0.read(&mut bytes[..length])
            }
        }
        impl Seek for ShortReader {
            fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
                self.0.seek(position)
            }
        }
        for newline in ["\n", "\r\n"] {
            let content = format!(
                "---{newline}id: note_01h00000000000000000000000{newline}type: note{newline}---{newline}{newline}# Attachment{newline}"
            );
            let mut reader = ShortReader(std::io::Cursor::new(content.into_bytes()));
            assert_eq!(
                managed_reader("large.md", &mut reader).unwrap().as_deref(),
                Some("note_01h00000000000000000000000")
            );
        }
        let mut empty = ShortReader(std::io::Cursor::new(Vec::new()));
        assert_eq!(managed_reader("empty.md", &mut empty).unwrap(), None);
    }
    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, name: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(name)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, name: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(name.into(), value.into());
            Ok(())
        }
    }
    #[test]
    fn attachments_survive_restart_move_and_index_rebuild_and_preserve_external_edits() {
        for name in ["picture.bin", "large.md"] {
            let directory = tempfile::TempDir::new().unwrap();
            let root = directory.path().join("first");
            let app = directory.path().join("app");
            let first = WorkspaceEngine::create_with_app_data(&root, "First", &app).unwrap();
            let workspace = first.manifest().id;
            let peer = WorkspaceEngine::create_sync_replica(
                directory.path().join("peer"),
                "Peer",
                &workspace,
                directory.path().join("peer_app"),
            )
            .unwrap();
            let credentials = Memory::default();
            let device = DeviceKeys::create(&credentials).unwrap();
            let mut secrets = SyncSecrets::default();
            secrets
                .trusted_devices
                .insert(device.device_id().into(), device.signer().public_key());
            let mut bytes =
                b"---\nid: note_01h00000000000000000000000\ntype: note\n---\n\n# Attachment\n"
                    .to_vec();
            bytes.extend(vec![b'a'; 1024 * 1024 + 123]);
            std::fs::write(root.join(name), &bytes).unwrap();
            first.sync_capture_workspace(&device, &mut secrets).unwrap();
            let operation = first.sync_outbox().unwrap().pop().unwrap();
            let key = secrets
                .objects
                .get(&(operation.object_id.clone(), 1))
                .unwrap();
            let plaintext = operation.open(key, &device.signer().public_key()).unwrap();
            let change: FileChange = serde_json::from_slice(&plaintext).unwrap();
            assert_eq!(change.version, 3);
            let blob = change.blob.as_ref().unwrap();
            let mut source = first.sync_blob_file(blob, false).unwrap();
            let mut target = peer.sync_blob_file(blob, true).unwrap();
            std::io::copy(&mut source, &mut target).unwrap();
            target.sync_all().unwrap();
            drop(target);
            let peer_before = peer.sync_journal().unwrap();
            assert_eq!(
                peer.sync_apply_file(&operation, key, &device.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            assert_eq!(std::fs::read(peer.root().join(name)).unwrap(), bytes);
            // Crash after the canonical file commit, before its receipt.
            peer.sync_write(STATE_PATH, &peer_before).unwrap();
            assert_eq!(
                peer.sync_apply_file(&operation, key, &device.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            peer.rebuild_index().unwrap();
            drop(first);
            let first = WorkspaceEngine::open_with_app_data(&root, &app).unwrap();
            let mut restored = first
                .sync_restore_secrets(&device, &secrets.trusted_devices)
                .unwrap();
            first
                .sync_capture_workspace(&device, &mut restored)
                .unwrap();
            assert_eq!(first.sync_outbox().unwrap().len(), 1);
            let renamed = format!("moved-{name}");
            std::fs::rename(root.join(name), root.join(&renamed)).unwrap();
            first
                .sync_capture_workspace(&device, &mut restored)
                .unwrap();
            let moved = first.sync_outbox().unwrap().pop().unwrap();
            assert_eq!(operation.object_id, moved.object_id);
            let moved_change: FileChange =
                serde_json::from_slice(&moved.open(key, &device.signer().public_key()).unwrap())
                    .unwrap();
            let moved_blob = moved_change.blob.as_ref().unwrap();
            let mut source = first.sync_blob_file(moved_blob, false).unwrap();
            let mut target = peer.sync_blob_file(moved_blob, true).unwrap();
            std::io::copy(&mut source, &mut target).unwrap();
            target.sync_all().unwrap();
            drop(target);
            std::fs::write(peer.root().join(name), b"unreviewed external edit").unwrap();
            assert_eq!(
                peer.sync_apply_file(&moved, key, &device.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Conflict
            );
            assert_eq!(
                std::fs::read(peer.root().join(name)).unwrap(),
                b"unreviewed external edit"
            );
            assert!(!peer.root().join(&renamed).exists());
            // Duplicate delivery cannot erase a later edit even when its ciphertext remains available.
            assert_eq!(
                peer.sync_apply_file(&operation, key, &device.signer().public_key())
                    .unwrap(),
                ApplyOutcome::Applied
            );
            assert_eq!(
                std::fs::read(peer.root().join(name)).unwrap(),
                b"unreviewed external edit"
            );
        }
    }
}
