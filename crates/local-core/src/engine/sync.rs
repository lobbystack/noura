use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use super::*;
use crate::sync::{
    ApplyOutcome, DeviceKeys, DocumentDescriptor, DocumentMode, EncryptedOperation, FileChange,
    KeyEnvelope, ObjectKey, OperationKind, SigningIdentity,
};
use crate::sync::{identifier, invalid, validate_file_change as validate_change};

const STATE_PATH: &str = ".noura/sync/state.json";
mod access;
mod apply;
mod blobs;
mod capture;
mod collaboration;
mod config;
mod conflicts;
mod plugin;
pub use plugin::SYNC_PLUGIN_ID;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transition: Option<crate::sync::AccessTransition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transition_phase: Option<TransitionPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activation: Option<crate::sync::ObjectActivation>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    activations: BTreeMap<String, crate::sync::ObjectActivation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransitionPhase {
    Prepare,
    Stage,
    ResolveCommit,
    Install,
    Rebase,
    Complete,
}

impl TransitionPhase {
    fn order(self) -> u8 {
        match self {
            Self::Prepare => 0,
            Self::Stage => 1,
            Self::ResolveCommit => 2,
            Self::Install => 3,
            Self::Rebase => 4,
            Self::Complete => 5,
        }
    }
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
    fn sync_document_descriptor(journal: &Journal, object_id: &str) -> Option<DocumentDescriptor> {
        Self::sync_document(journal, object_id).map(|(_, descriptor)| descriptor)
    }

    fn sync_document(journal: &Journal, object_id: &str) -> Option<(u64, DocumentDescriptor)> {
        journal
            .access_policy
            .as_ref()
            .and_then(|policy| {
                policy
                    .objects
                    .iter()
                    .find(|object| object.object_id == object_id)
                    .and_then(|object| {
                        object
                            .document
                            .clone()
                            .map(|document| (object.epoch, document))
                    })
            })
            .or_else(|| {
                journal.activations.get(object_id).map(|activation| {
                    (
                        activation.checkpoint.payload.epoch,
                        activation.document.clone(),
                    )
                })
            })
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
        if let Some(activation) = &journal.activation {
            if activation.workspace_id != journal.workspace_id {
                return Err(invalid("sync_invalid_journal"));
            }
            activation.digest()?;
        }
        for (object_id, activation) in &journal.activations {
            if activation.workspace_id != journal.workspace_id
                || activation.checkpoint.payload.object_id != *object_id
            {
                return Err(invalid("sync_invalid_journal"));
            }
            activation.digest()?;
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
        for reserved in [".noura", ".git", "node_modules", "target"] {
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
mod tests;
