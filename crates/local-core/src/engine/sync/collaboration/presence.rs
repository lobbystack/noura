//! Encrypted presence: who else has a collaborative document open.

use super::*;

impl WorkspaceEngine {
    pub fn collaboration_seal_presence(
        &self,
        input: crate::sync::CollaborationPresenceInput,
        relay_session_id: &str,
        sequence: u64,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<crate::sync::EncryptedPresence> {
        crate::sync::identifier(&input.session_id)?;
        let (object, generation, _) = self
            .collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .get(&input.session_id)
            .cloned()
            .ok_or_else(|| invalid("collaboration_session_closed"))?;
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, &object)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if descriptor.generation != generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        let key = secrets
            .objects
            .get(&(object.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_missing"))?;
        let state = self
            .collaboration_state(&object)?
            .filter(|state| state.generation == generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        TextDocument::restore(&state.update)?
            .validate_relative_positions(&[input.anchor.clone(), input.head.clone()])?;
        crate::sync::EncryptedPresence::seal(
            device,
            key,
            &crate::sync::PresenceContext {
                workspace_id: &journal.workspace_id,
                object_id: &object,
                generation: &generation,
                epoch,
                session_id: relay_session_id,
                sequence,
            },
            &crate::sync::PresenceSelection {
                anchor: input.anchor,
                head: input.head,
            },
        )
    }

    pub fn collaboration_receive_presence(
        &self,
        value: &crate::sync::EncryptedPresence,
        secrets: &SyncSecrets,
    ) -> Result<()> {
        let public_key = secrets
            .trusted_devices
            .get(&value.device_id)
            .ok_or_else(|| invalid("sync_untrusted_device"))?;
        value.verify(public_key)?;
        if value.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, &value.object_id)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if epoch != value.epoch || descriptor.generation != value.generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        let key = secrets
            .objects
            .get(&(value.object_id.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_missing"))?;
        let selection = value.open(key, public_key)?;
        let state = self
            .collaboration_state(&value.object_id)?
            .filter(|state| state.generation == value.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        TextDocument::restore(&state.update)?
            .validate_relative_positions(&[selection.anchor.clone(), selection.head.clone()])?;
        let color_hash = blake3::hash(value.device_id.as_bytes());
        let color = format!(
            "#{:02x}{:02x}{:02x}",
            color_hash.as_bytes()[0],
            color_hash.as_bytes()[1],
            color_hash.as_bytes()[2]
        );
        let mut presence = self
            .collaboration_presence
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?;
        presence.retain(|_, entry| entry.expires_at > std::time::Instant::now());
        if presence.get(&value.device_id).is_some_and(|entry| {
            entry.session_id == value.session_id && entry.sequence >= value.sequence
        }) {
            return Ok(());
        }
        presence.insert(
            value.device_id.clone(),
            super::CollaborationPresenceCacheEntry {
                object_id: value.object_id.clone(),
                generation: value.generation.clone(),
                session_id: value.session_id.clone(),
                sequence: value.sequence,
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(30),
                member: crate::sync::CollaborationPresenceMember {
                    device_id: value.device_id.clone(),
                    name: value.device_id.clone(),
                    color,
                    anchor: selection.anchor,
                    head: selection.head,
                },
            },
        );
        drop(presence);
        self.collaboration_emit_presence(&value.object_id, &value.generation)
    }

    pub fn collaboration_remove_presence(
        &self,
        device_id: &str,
        relay_session_id: &str,
    ) -> Result<()> {
        crate::sync::identifier(device_id)?;
        crate::sync::identifier(relay_session_id)?;
        let affected = {
            let mut presence = self
                .collaboration_presence
                .lock()
                .map_err(|_| invalid("collaboration_unavailable"))?;
            let affected = presence
                .get(device_id)
                .filter(|entry| entry.session_id == relay_session_id)
                .map(|entry| (entry.object_id.clone(), entry.generation.clone()));
            if affected.is_some() {
                presence.remove(device_id);
            }
            affected
        };
        if let Some((object, generation)) = affected {
            self.collaboration_emit_presence(&object, &generation)?;
        }
        Ok(())
    }

    pub fn collaboration_expire_presence(&self) -> Result<()> {
        let affected = {
            let mut presence = self
                .collaboration_presence
                .lock()
                .map_err(|_| invalid("collaboration_unavailable"))?;
            let now = std::time::Instant::now();
            let affected = presence
                .values()
                .filter(|entry| entry.expires_at <= now)
                .map(|entry| (entry.object_id.clone(), entry.generation.clone()))
                .collect::<BTreeSet<_>>();
            presence.retain(|_, entry| entry.expires_at > now);
            affected
        };
        for (object, generation) in affected {
            self.collaboration_emit_presence(&object, &generation)?;
        }
        Ok(())
    }

    pub fn collaboration_clear_presence(&self) -> Result<()> {
        let affected = {
            let mut presence = self
                .collaboration_presence
                .lock()
                .map_err(|_| invalid("collaboration_unavailable"))?;
            let affected = presence
                .values()
                .map(|entry| (entry.object_id.clone(), entry.generation.clone()))
                .collect::<BTreeSet<_>>();
            presence.clear();
            affected
        };
        for (object, generation) in affected {
            self.collaboration_emit_presence(&object, &generation)?;
        }
        Ok(())
    }

    fn collaboration_emit_presence(&self, object: &str, generation: &str) -> Result<()> {
        let mut members = self
            .collaboration_presence
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .values()
            .filter(|entry| entry.object_id == object && entry.generation == generation)
            .map(|entry| entry.member.clone())
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        self.emit(
            "collaboration:presence",
            EventSource::Sync,
            serde_json::to_value(crate::sync::CollaborationPresenceEvent {
                object_id: object.into(),
                generation: generation.into(),
                presence: members,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        Ok(())
    }
}
