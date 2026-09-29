//! Access policy transitions and object activations. Each one is written to
//! the sync journal before the relay sees it.

use super::*;

impl WorkspaceEngine {
    /// Persist exact signed transition bytes before any relay submission. Outbox edits are retained.
    pub fn sync_prepare_transition(
        &self,
        transition: &crate::sync::AccessTransition,
        public_key: &str,
    ) -> Result<()> {
        transition.verify(public_key)?;
        if transition.policy.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let _lock = self.write_lock("sync_transition")?;
        let mut journal = self.sync_journal()?;
        if let Some(existing) = &journal.transition {
            if existing.digest()? != transition.digest()? {
                return Err(invalid("sync_transition_pending"));
            }
            if journal.transition_phase.is_none() {
                journal.transition_phase = Some(TransitionPhase::Prepare);
                self.sync_write(STATE_PATH, &journal)?;
            }
            return Ok(());
        }
        if sync_cursor(&transition.covered_sequence)? != sync_cursor(&journal.cursor)?
            || sync_cursor(&transition.policy.revision)?
                != sync_cursor(&journal.access_revision)? + 1
        {
            return Err(invalid("sync_transition_stale"));
        }
        journal.transition = Some(transition.clone());
        journal.transition_phase = Some(TransitionPhase::Prepare);
        self.sync_write(STATE_PATH, &journal)
    }

    pub fn sync_pending_transition(&self) -> Result<Option<crate::sync::AccessTransition>> {
        let _lock = self.write_lock("sync_transition")?;
        let mut journal = self.sync_journal()?;
        if journal.transition_phase == Some(TransitionPhase::Complete) {
            journal.transition = None;
            journal.transition_phase = None;
            self.sync_write(STATE_PATH, &journal)?;
        }
        Ok(journal.transition)
    }

    /// Persist an exact unpublished activation before contacting the relay.
    pub fn sync_prepare_activation(
        &self,
        activation: &crate::sync::ObjectActivation,
        public_key: &str,
    ) -> Result<()> {
        activation.verify(public_key)?;
        if activation.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let _lock = self.write_lock("sync_activation")?;
        let mut journal = self.sync_journal()?;
        if let Some(existing) = &journal.activation {
            if existing.digest()? != activation.digest()? {
                return Err(invalid("sync_activation_pending"));
            }
            return Ok(());
        }
        if activation.policy_revision != journal.access_revision
            || activation.covered_sequence != journal.cursor
        {
            return Err(invalid("sync_activation_stale"));
        }
        journal.activation = Some(activation.clone());
        self.sync_write(STATE_PATH, &journal)
    }

    pub fn sync_pending_activation(&self) -> Result<Option<crate::sync::ObjectActivation>> {
        let _lock = self.write_lock("sync_activation")?;
        Ok(self.sync_journal()?.activation)
    }

    /// Record a committed activation before installing its canonical checkpoint.
    pub fn sync_accept_activation(
        &self,
        activation: &crate::sync::ObjectActivation,
        public_key: &str,
    ) -> Result<()> {
        activation.verify(public_key)?;
        if activation.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let object_id = &activation.checkpoint.payload.object_id;
        let _lock = self.write_lock("sync_activation")?;
        let mut journal = self.sync_journal()?;
        if sync_cursor(&activation.policy_revision)? > sync_cursor(&journal.access_revision)? {
            return Err(invalid("sync_activation_stale"));
        }
        if let Some(existing) = journal.activations.get(object_id) {
            if existing.digest()? != activation.digest()? {
                return Err(invalid("sync_activation_changed"));
            }
            return Ok(());
        }
        journal
            .activations
            .insert(object_id.clone(), activation.clone());
        self.sync_write(STATE_PATH, &journal)
    }

    pub fn sync_finish_activation(&self, activation_id: &str) -> Result<()> {
        identifier(activation_id)?;
        let _lock = self.write_lock("sync_activation")?;
        let mut journal = self.sync_journal()?;
        let Some(activation) = journal.activation.as_ref() else {
            return Ok(());
        };
        if activation.activation_id != activation_id {
            return Err(invalid("sync_activation_pending"));
        }
        if !self.collaboration_generation_is_installed(
            &activation.checkpoint.payload.object_id,
            &activation.document.generation,
        )? {
            return Err(invalid("sync_activation_install_incomplete"));
        }
        journal.activation = None;
        self.sync_write(STATE_PATH, &journal)
    }

    /// Clear only an activation proven uncommitted by the relay. Captured local bytes and
    /// legacy outbox ciphertext remain available to rebuild against the new policy boundary.
    pub fn sync_abandon_activation(&self, activation_id: &str) -> Result<()> {
        identifier(activation_id)?;
        let _lock = self.write_lock("sync_activation")?;
        let mut journal = self.sync_journal()?;
        let Some(activation) = journal.activation.as_ref() else {
            return Ok(());
        };
        if activation.activation_id != activation_id {
            return Err(invalid("sync_activation_pending"));
        }
        journal.activation = None;
        self.sync_write(STATE_PATH, &journal)
    }

    pub(crate) fn sync_advance_transition(&self, transition_id: &str, phase: &str) -> Result<()> {
        crate::sync::identifier(transition_id)?;
        let next = match phase {
            "stage" => TransitionPhase::Stage,
            "resolve_commit" => TransitionPhase::ResolveCommit,
            "install" => TransitionPhase::Install,
            "rebase" => TransitionPhase::Rebase,
            "complete" => TransitionPhase::Complete,
            _ => return Err(invalid("sync_invalid_transition_phase")),
        };
        let _lock = self.write_lock("sync_transition_phase")?;
        let mut journal = self.sync_journal()?;
        let transition = journal
            .transition
            .as_ref()
            .ok_or_else(|| invalid("sync_transition_missing"))?;
        if transition.transition_id != transition_id {
            return Err(invalid("sync_transition_changed"));
        }
        let current = journal.transition_phase.unwrap_or(TransitionPhase::Prepare);
        // Retrying an exact durable transition replays the orchestration from the start.
        // Phases already crossed are successful no-ops; skipping a phase still fails closed.
        if next.order() <= current.order() {
            return Ok(());
        }
        if next.order() > current.order() + 1 {
            return Err(invalid("sync_invalid_transition_phase"));
        }
        journal.transition_phase = Some(next);
        self.sync_write(STATE_PATH, &journal)?;
        Ok(())
    }

    pub(crate) fn sync_finish_transition_install(&self, transition_id: &str) -> Result<()> {
        let _lock = self.write_lock("sync_transition_finish")?;
        let mut journal = self.sync_journal()?;
        let transition = journal
            .transition
            .as_ref()
            .filter(|transition| transition.transition_id == transition_id)
            .ok_or_else(|| invalid("sync_transition_changed"))?;
        if journal.access_revision != transition.policy.revision {
            return Err(invalid("sync_transition_install_incomplete"));
        }
        for checkpoint in &transition.checkpoints {
            if !self.collaboration_generation_is_installed(
                &checkpoint.payload.object_id,
                &checkpoint.generation,
            )? {
                return Err(invalid("sync_transition_install_incomplete"));
            }
        }
        let current = journal.transition_phase.unwrap_or(TransitionPhase::Prepare);
        if current == TransitionPhase::Install {
            journal.transition_phase = Some(TransitionPhase::Rebase);
            self.sync_write(STATE_PATH, &journal)?;
            journal = self.sync_journal()?;
        }
        if journal.transition_phase != Some(TransitionPhase::Rebase) {
            return Err(invalid("sync_invalid_transition_phase"));
        }
        journal.transition_phase = Some(TransitionPhase::Complete);
        self.sync_write(STATE_PATH, &journal)
    }

    /// Resolve a checkpoint's policy through the hash chain anchored by the accepted head.
    /// Missing policy history fails closed; an untrusted relay cannot invent an earlier authority.
    pub fn sync_checkpoint_policy(&self, revision: &str) -> Result<crate::sync::AccessPolicy> {
        let target = sync_cursor(revision)?;
        let _lock = self.write_lock("sync_checkpoint_policy")?;
        let mut policy = self
            .sync_journal()?
            .access_policy
            .ok_or_else(|| invalid("sync_policy_chain_changed"))?;
        for _ in 0..10_000 {
            let current = sync_cursor(&policy.revision)?;
            if current == target {
                return Ok(policy);
            }
            if current < target || current == 0 {
                break;
            }
            let bytes = read_optional(
                &self.sync_path(&format!(".noura/sync/policies/{}.json", current - 1))?,
            )?
            .ok_or_else(|| invalid("sync_policy_history_required"))?;
            let prior: crate::sync::AccessPolicy =
                serde_json::from_slice(&bytes).map_err(|_| invalid("sync_policy_chain_changed"))?;
            if sync_cursor(&prior.revision)? != current - 1
                || policy.previous_policy_digest.as_ref() != Some(&prior.digest()?)
            {
                return Err(invalid("sync_policy_chain_changed"));
            }
            policy = prior;
        }
        Err(invalid("sync_policy_history_required"))
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
        if let Some(prior) = &journal.access_policy {
            self.sync_write_once(
                &format!(".noura/sync/policies/{}.json", prior.revision),
                prior,
            )?;
        }
        self.sync_write_once(
            &format!(".noura/sync/policies/{}.json", policy.revision),
            policy,
        )?;
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
}
