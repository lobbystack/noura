//! Restricted domains on a device (docs/workspace-format/sync-v2.md §8,
//! §9): the journal keeps one [`Frame`] per domain, and the device works in
//! one at a time, so every step of the push and pull runs the same way in
//! each. This module makes a folder restricted, loads the domains the
//! policy lets the device read, records domain roots from the markers, and
//! looks after the marker files themselves.
//!
//! A marker sits outside the scanner's view, so capture never sees it:
//! [`WorkspaceSync::check_markers`] writes one you deleted again, rewrites
//! one an outside edit changed, and follows one that moved with its folder.

use std::collections::{BTreeMap, BTreeSet};

use sync_key_envelope::v2::{EnvelopeHeader, wrap};
use sync_protocol::codec::{Bytes, Nested, Sorted, Text};
use sync_protocol::crypto::{hash, random_nonce};
use sync_protocol::keys::{EpochChain, EpochParams, build_epoch};
use sync_protocol::matching::{
    DomainRoot, HeldRoot, LiveState, MARKER, MarkerView, Placement, RecordedRoot, beneath,
    hold_ends, marker_allowed, marker_present, placement, valid_roots,
};
use sync_protocol::messages::RecordRef;
use sync_protocol::records::{
    Access, Content, EpochBundle, Level, Operation, Payload, PayloadKind, PolicyChange, PolicyKind,
    Signed, SyncPath, marker_content, parse_marker,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use super::super::{RemoteWorkspace, SyncError, SyncService, random};
use super::capture::{Found, Skipped};
use super::state::{Content as Carried, Frame, Local, Object, OpKey, StateInfo};
use super::{WorkspaceSync, core, rebuild, stat};

/// What the device holds in memory for a domain it isn't working in.
#[derive(Default)]
pub(super) struct Runtime {
    pub(super) keys: BTreeMap<u64, SecretKey>,
    pub(super) remote: Option<RemoteWorkspace>,
    pub(super) entry_hint: Option<u64>,
    pub(super) armed: BTreeSet<(bool, u64)>,
}

impl WorkspaceSync {
    /// Starts or stops working in `domain`; calling it twice leaves the
    /// device where it was.
    pub(super) fn swap_domain(&mut self, domain: Id) {
        let runtime = self.parked.entry(domain).or_default();
        std::mem::swap(&mut self.keys, &mut runtime.keys);
        std::mem::swap(&mut self.remote, &mut runtime.remote);
        std::mem::swap(&mut self.entry_hint, &mut runtime.entry_hint);
        std::mem::swap(&mut self.armed, &mut runtime.armed);
        self.state.swap(&domain);
        self.active = match self.active {
            Some(_) => None,
            None => Some(domain),
        };
    }

    /// Runs `work` in `domain`, or where the device is for `None`.
    pub(super) fn in_domain<R>(
        &mut self,
        domain: Option<Id>,
        work: impl FnOnce(&mut Self) -> R,
    ) -> R {
        match domain {
            Some(domain) if self.active.is_none() => {
                self.swap_domain(domain);
                let result = work(self);
                self.swap_domain(domain);
                result
            }
            _ => work(self),
        }
    }

    /// The log that holds `domain`, wherever the device works.
    pub(super) fn log_of(&self, domain: Option<Id>) -> &super::Log {
        match (domain, self.active) {
            (None, None) => &self.state.log,
            (None, Some(active)) => &self.state.restricted[&active].log,
            (Some(domain), Some(active)) if domain == active => &self.state.log,
            (Some(domain), _) => self
                .state
                .restricted
                .get(&domain)
                .map_or(&self.state.log, |frame| &frame.log),
        }
    }

    /// The restricted domains the device has loaded and still reads.
    pub(super) fn loaded_domains(&self) -> Vec<Id> {
        let device = self.credentials.keys.device();
        self.parked
            .iter()
            .filter(|(domain, runtime)| {
                runtime
                    .remote
                    .as_ref()
                    .is_some_and(|r| r.policy.head_snapshot().can_read(&device, domain))
            })
            .map(|(domain, _)| *domain)
            .collect()
    }

    /// The revision of the change that created the domain the device works
    /// in, whose log has no entry for an earlier change; 1 otherwise.
    pub(super) fn first_revision(&self) -> u64 {
        let domain = self.state.domain;
        self.state
            .policy
            .iter()
            .find_map(|change| match &change.body.kind {
                PolicyKind::CreateDomain { domain: d, .. } if *d == domain => {
                    Some(change.body.revision)
                }
                _ => None,
            })
            .unwrap_or(1)
    }

    /// Loads each restricted domain the verified policy lets this device
    /// read and hands the others the latest policy. A domain whose envelope
    /// hasn't arrived waits for a later round (§9).
    pub(super) async fn sync_domains(
        &mut self,
        service: &mut SyncService,
    ) -> Result<(), SyncError> {
        if self.active.is_some() {
            return Ok(());
        }
        let Some(policy) = self.remote.as_ref().map(|r| r.policy.clone()) else {
            return Ok(());
        };
        let device = self.credentials.keys.device();
        let workspace = self.state.workspace;
        let recipient = self.credentials.keys.clone();
        let readable: Vec<Id> = policy
            .head_snapshot()
            .domains
            .iter()
            .filter(|(domain, d)| d.restricted && policy.head_snapshot().can_read(&device, domain))
            .map(|(domain, _)| *domain)
            .collect();
        for runtime in self.parked.values_mut() {
            if let Some(remote) = runtime.remote.as_mut()
                && remote.policy.head().0 < policy.head().0
            {
                remote.policy = policy.clone();
            }
        }
        for domain in readable {
            if self
                .parked
                .get(&domain)
                .is_some_and(|runtime| runtime.remote.is_some())
            {
                continue;
            }
            let known = self.state.restricted.contains_key(&domain);
            let loaded = service
                .load_keys(workspace, domain, &policy, &recipient, known)
                .await;
            let (epochs, keys, envelope) = match loaded {
                Ok(loaded) => loaded,
                Err(SyncError::KeyMissing) => continue,
                Err(error) => return Err(error),
            };
            let remote = RemoteWorkspace::from_parts(
                workspace,
                domain,
                policy.clone(),
                epochs,
                keys,
                envelope,
            )?;
            if !known {
                let next = self.mark.start().get(&domain).copied().unwrap_or(1);
                self.write_mark()?;
                self.state
                    .restricted
                    .insert(domain, Frame::new(domain, next));
            }
            self.swap_domain(domain);
            let adopted = self.adopt(remote);
            let compared = match adopted {
                Ok(()) => self.compare_own_chain(service).await,
                Err(error) => Err(error),
            };
            self.swap_domain(domain);
            compared?;
        }
        self.update_roots()?;
        self.share_domain_keys();
        self.save()
    }

    /// Writes the mark to the credential store, as every raised reservation
    /// does before a counter in it is signed (§14).
    fn write_mark(&self) -> Result<(), SyncError> {
        self.mark.write(
            &*self.secrets,
            &self.credentials.keys.device(),
            &self.state.workspace,
        )
    }

    /// Hands the workspace's verified state the keys of the domains the
    /// device reads, so a change it signs there wraps for new cards (§8).
    fn share_domain_keys(&mut self) {
        let keys: Vec<(Id, u64, SecretKey)> = self
            .parked
            .iter()
            .filter_map(|(domain, runtime)| {
                let remote = runtime.remote.as_ref()?;
                Some((*domain, remote.epoch(), remote.current_key()))
            })
            .collect();
        if let Some(remote) = self.remote.as_mut() {
            remote.set_restricted(keys);
        }
    }

    /// Whether the device still waits for a marker of a domain it reads,
    /// so a new local file can't tell which domain it belongs to (§9).
    pub(super) fn markers_pending(&self) -> bool {
        let Some(remote) = self.remote.as_ref() else {
            return self.state.restricted.values().any(|f| f.bootstrap);
        };
        let device = self.credentials.keys.device();
        let snapshot = remote.policy.head_snapshot();
        snapshot.domains.iter().any(|(domain, d)| {
            d.restricted
                && snapshot.can_read(&device, domain)
                && self
                    .state
                    .restricted
                    .get(domain)
                    .is_none_or(|frame| frame.bootstrap)
        })
    }

    /// The `marker_rid` of each domain this journal's policy ever created.
    fn created_markers(&self) -> BTreeMap<Id, (Id, u64)> {
        self.state
            .policy
            .iter()
            .filter_map(|change| match &change.body.kind {
                PolicyKind::CreateDomain {
                    domain, marker_rid, ..
                } => Some((*domain, (*marker_rid, change.body.revision))),
                _ => None,
            })
            .collect()
    }

    /// The revision of `domain`'s `create_domain`.
    pub(super) fn created_revision(&self, domain: &Id) -> Option<u64> {
        self.created_markers().get(domain).map(|(_, at)| *at)
    }

    /// The markers this device follows, with their objects' `rid`s.
    pub(super) fn marker_objects(&self) -> BTreeSet<Id> {
        self.created_markers()
            .values()
            .map(|(rid, _)| *rid)
            .collect()
    }

    /// The live states of a marker. Before the device has pulled the
    /// marker's entry, what its journal carries stands in, as after the
    /// device created the domain or a bootstrap read the marker first.
    fn marker_states(&self, domain: Id, rid: &Id) -> Vec<LiveState> {
        let Some(object) = self.state.objects.get(rid) else {
            return Vec::new();
        };
        let state = |info: &StateInfo| match &info.content {
            Some(content) => LiveState {
                path: content.path.clone(),
                revision: content.revision,
                delete: content.delete,
            },
            None => LiveState {
                path: String::new(),
                revision: Digest::ZERO,
                delete: false,
            },
        };
        let live = self.log_of(Some(domain)).live.live(rid);
        if live.is_empty() {
            return object.states.values().map(state).collect();
        }
        live.iter()
            .filter_map(|(op, _)| {
                object.states.get(&OpKey {
                    device: op.device,
                    counter: op.counter,
                })
            })
            .map(state)
            .collect()
    }

    /// Takes the domain roots from the markers (§9). A valid marker records
    /// or moves its root, unless the move would change the domain of
    /// existing objects. A root whose marker stopped being valid stays
    /// recorded and holds its files, and a marker valid again ends the
    /// hold, after which the files beneath it are matched.
    pub(super) fn update_roots(&mut self) -> Result<(), SyncError> {
        let Some(snapshot) = self
            .remote
            .as_ref()
            .map(|r| r.policy.head_snapshot().clone())
        else {
            return Ok(());
        };
        // A domain the policy dropped, or that this device no longer
        // reads, holds its files: new ones can't reach it, and they stay
        // out of any other (§9).
        let device = self.credentials.keys.device();
        let listed = |domain: &Id| snapshot.can_read(&device, domain);
        let created = self.created_markers();
        let views: Vec<MarkerView> = created
            .iter()
            .filter(|(domain, _)| self.state.restricted.contains_key(domain))
            .map(|(domain, (rid, created_at))| MarkerView {
                domain: *domain,
                rid: *rid,
                created_at: *created_at,
                listed: listed(domain),
                live: self.marker_states(*domain, rid),
            })
            .collect();
        let mut valid = valid_roots(&views);
        // A root stays recorded after its marker stopped being valid, and
        // it still wins over a later marker inside it.
        let earlier = |root: &RecordedRoot, kept: &RecordedRoot| {
            let revision = |domain: &Id| created.get(domain).map_or(u64::MAX, |(_, at)| *at);
            kept.domain != root.domain
                && revision(&kept.domain) < revision(&root.domain)
                && beneath(&root.folder, &kept.folder)
        };
        valid.retain(|root| !self.state.roots.iter().any(|kept| earlier(root, kept)));
        let mut roots = self.state.roots.clone();
        for root in &valid {
            match roots.iter().position(|r| r.domain == root.domain) {
                None => roots.push(root.clone()),
                Some(index) if roots[index].folder == root.folder => {}
                Some(index) if self.move_changes_domain(root) => {
                    let folder = roots[index].folder.clone();
                    self.hold(folder, root.domain);
                }
                Some(index) => roots[index].folder = root.folder.clone(),
            }
        }
        let stale: Vec<RecordedRoot> = roots
            .iter()
            .filter(|kept| !valid.iter().any(|root| root.domain == kept.domain))
            .cloned()
            .collect();
        for root in stale {
            self.hold(root.folder, root.domain);
        }
        roots.sort_by(|a, b| a.folder.cmp(&b.folder));
        let added: Vec<RecordedRoot> = roots
            .iter()
            .filter(|root| !self.state.roots.iter().any(|r| r.domain == root.domain))
            .cloned()
            .collect();
        self.state.roots = roots;
        for root in &added {
            self.leave_new_root(root);
        }
        // A marker you moved away from its root leaves the synced marker
        // valid, but the hold stands until the file is back (§9).
        let covering: Vec<DomainRoot> = valid
            .iter()
            .filter(|root| !self.marker_moved_away(root))
            .map(|root| DomainRoot {
                folder: root.folder.clone(),
                domain: root.domain,
            })
            .collect();
        // An iCloud placeholder can't name its domain, but at a valid root
        // it counts as that root's marker.
        let placeholders: Vec<String> = covering
            .iter()
            .filter(|root| !self.names_in(&root.folder).iter().any(|n| n == MARKER))
            .map(|root| root.folder.clone())
            .collect();
        let before = self.state.held_roots.len();
        self.state.held_roots.retain(|held| {
            !hold_ends(held, &covering)
                && !(held.domain.is_none() && placeholders.contains(&held.folder))
        });
        self.rematch |= self.state.held_roots.len() != before;
        Ok(())
    }

    /// Takes out of a root this device just learned of each file of
    /// another domain that doesn't wait for its move, as a bootstrap leaves
    /// them when it reads the workspace domain first (§9). A file that
    /// changed on disk stays, and the next capture finds it.
    fn leave_new_root(&mut self, root: &RecordedRoot) {
        let markers = self.marker_objects();
        let synced: Vec<(Id, String)> = self
            .state
            .objects
            .iter()
            .filter(|(rid, object)| {
                object.domain != Some(root.domain)
                    && !markers.contains(rid)
                    && object.base.is_some()
                    && object.states.values().all(|state| state.seq.is_some())
            })
            .filter_map(|(rid, object)| {
                let local = object.local.as_ref()?;
                beneath(&local.path, &root.folder).then(|| (*rid, local.path.clone()))
            })
            .collect();
        for (rid, path) in synced {
            let Placement::Shared(to) = self.placement_with_exception(&rid, &path) else {
                continue;
            };
            if let Err(error) = self.relocate(&rid, &to) {
                tracing::warn!(%error, "a file of another domain stayed under a new root");
            }
        }
    }

    fn hold(&mut self, folder: String, domain: Id) {
        let held = HeldRoot {
            folder,
            domain: Some(domain),
        };
        if !self.state.held_roots.contains(&held) {
            self.state.held_roots.push(held);
        }
    }

    /// Whether a marker moved to `root` would take in a file of another
    /// domain.
    fn move_changes_domain(&self, root: &RecordedRoot) -> bool {
        self.state.objects.iter().any(|(rid, object)| {
            *rid != root.rid
                && object.domain != Some(root.domain)
                && object
                    .local
                    .as_ref()
                    .is_some_and(|l| beneath(&l.path, &root.folder))
        })
    }

    /// Where `rid`'s file sits for the synced path `path` (§9).
    pub(super) fn placement_of(&self, rid: &Id, path: &str) -> Placement {
        let domain = self.state.objects.get(rid).and_then(|o| o.domain);
        placement(path, domain, &self.state.roots)
    }

    /// Records a held root at the folder of an object of a restricted
    /// domain that no recorded root covers, as a bootstrap does when it
    /// meets a moved, deleted or conflicted marker, before the object's
    /// file is written (§9).
    pub(super) fn hold_uncovered(&mut self, rid: &Id, path: &str) {
        let Some(domain) = self.state.objects.get(rid).and_then(|o| o.domain) else {
            return;
        };
        if self.state.roots.iter().any(|root| root.domain == domain) {
            return;
        }
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        self.hold(folder.to_owned(), domain);
    }

    /// Restricts `folder` to its grantees (§8, §9): one push carries the
    /// `create_domain` whose access list names this device's account as a
    /// manager, the domain's epoch 1, an envelope for each of its readers
    /// and the marker. The device then records the root. Returns the new
    /// domain.
    pub async fn restrict_folder(
        &mut self,
        service: &mut SyncService,
        folder: &str,
        grants: &[Access],
    ) -> Result<Id, SyncError> {
        self.refuse_halted()?;
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        if !marker_allowed(folder, &self.state.roots) {
            return Err(SyncError::Invalid(
                "This folder is already inside a restricted folder.".into(),
            ));
        }
        let marker_path = format!("{folder}/{MARKER}");
        let path = SyncPath::new(marker_path.clone())
            .map_err(|e| SyncError::Invalid(format!("path: {}", e.name())))?;
        let mut remote = self.remote.take().ok_or(SyncError::KeyMissing)?;
        let committed = self
            .push_new_domain(service, &mut remote, path, grants)
            .await;
        self.adopt(remote)?;
        let (domain, frame, runtime, object, marker_rid, entry) = committed?;
        self.state.log.domain_entries.extend(entry);
        self.state.restricted.insert(domain, frame);
        self.parked.insert(domain, runtime);
        self.state.objects.insert(marker_rid, object);
        self.state.roots.push(RecordedRoot {
            folder: folder.to_owned(),
            domain,
            rid: marker_rid,
        });
        self.state.roots.sort_by(|a, b| a.folder.cmp(&b.folder));
        let root = RecordedRoot {
            folder: folder.to_owned(),
            domain,
            rid: marker_rid,
        };
        self.restore_marker(&root)?;
        // Each object already under the folder moves in, starting now;
        // rounds push the pairs (§9).
        self.state.move_due.insert(domain, 0);
        self.finish_moves()?;
        self.save()?;
        Ok(domain)
    }

    /// Signs and pushes the records of a new domain, and returns what the
    /// journal keeps of it.
    async fn push_new_domain(
        &mut self,
        service: &mut SyncService,
        remote: &mut RemoteWorkspace,
        path: SyncPath,
        grants: &[Access],
    ) -> Result<(Id, Frame, Runtime, Object, Id, Option<(u64, u64)>), SyncError> {
        service.refresh_policy(remote).await?;
        let chains = service.chains_of(remote).await?;
        let own = service.credentials.account;
        let own_head = remote
            .policy
            .head_snapshot()
            .members
            .get(&own)
            .map(|m| m.head.clone())
            .ok_or(SyncError::Check("sync_policy_unauthorized"))?;
        let mut access: BTreeMap<Id, Level> = grants.iter().map(|a| (a.account, a.level)).collect();
        access.insert(own, Level::Manage);
        let access = Sorted::new(
            access
                .into_iter()
                .map(|(account, level)| Access { account, level })
                .collect(),
        )
        .map_err(|e| SyncError::Invalid(e.name().to_owned()))?;
        let workspace = remote.workspace;
        let domain = random(Id::random())?;
        let marker_rid = random(Id::random())?;
        let keys = service.credentials.keys.clone();
        let device = keys.device();
        let (revision, previous) = remote.policy.head();
        let change: Signed<PolicyChange> = Signed::sign(
            PolicyChange {
                workspace,
                revision: revision + 1,
                previous,
                signer: device,
                heads: Sorted::new(vec![own_head])
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                kind: PolicyKind::CreateDomain {
                    domain,
                    marker_rid,
                    access,
                },
            },
            keys.signing(),
        );
        let next = remote
            .policy
            .check(&change, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        let readers = next.readers(&domain);
        let key = random(SecretKey::random())?;
        let epoch = Signed::sign(
            build_epoch(
                &EpochParams {
                    workspace,
                    domain,
                    epoch: 1,
                    previous: Digest::ZERO,
                    policy_revision: change.body.revision,
                    policy_head: change.digest(),
                    rotating_device: device,
                    recipients: readers.clone(),
                },
                &key,
                None,
                Nonce::ZERO,
            ),
            keys.signing(),
        );
        let mut records = vec![
            RecordRef::Policy(change.clone()),
            RecordRef::EpochBundle(EpochBundle {
                epoch: Nested(epoch.clone()),
                recipients: Sorted::new(readers.iter().copied().collect())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            }),
        ];
        let mut own_envelope = None;
        for reader in &readers {
            let envelope = wrap(
                &EnvelopeHeader {
                    workspace,
                    domain,
                    epoch: 1,
                    wrapping_device: device,
                },
                &key,
                &next.devices[reader].card,
                &keys,
            )
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
            if *reader == device {
                own_envelope = Some(envelope.clone());
            }
            records.push(RecordRef::Envelope(envelope));
        }

        // The marker, the first record of this device's chain there.
        let counter = self.mark.start().get(&domain).copied().unwrap_or(1);
        self.mark.reserve(&domain, counter);
        self.write_mark()?;
        let bytes = marker_content(&domain).into_bytes();
        let revision = hash(&bytes);
        let payload = Payload {
            rid: marker_rid,
            kind: PayloadKind::File {
                object_id: Text::default(),
                path: path.clone(),
                revision,
                content: Content::Inline {
                    bytes: Bytes(bytes),
                },
                origin: None,
            },
        };
        let mut op = Operation {
            workspace,
            domain,
            device,
            counter,
            previous: Digest::ZERO,
            epoch: 1,
            policy_revision: change.body.revision,
            policy_head: change.digest(),
            previous_attempt: Digest::ZERO,
            generation: self.state.generation,
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid: marker_rid,
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        op.seal_payload(&payload, &key, &epoch.digest(), random(random_nonce())?)
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
        let op = Signed::sign(op, keys.signing());
        records.push(RecordRef::Operation(op.clone()));

        let response = service.client().push(workspace, records).await?;
        let seq = response
            .receipts
            .0
            .iter()
            .find(|r| r.record_digest == op.digest())
            .map(|r| r.seq)
            .ok_or_else(|| SyncError::Network("the push left the marker out".into()))?;
        let entry = response
            .receipts
            .0
            .iter()
            .find(|r| r.record_digest == change.digest() && r.domain == self.state.domain)
            .map(|r| (change.body.revision, r.seq));
        remote
            .policy
            .append(&change, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;

        let mut epochs = EpochChain::new(workspace, domain);
        epochs
            .accept(&epoch, &key, &remote.policy)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        let keys_held = BTreeMap::from([(1, key)]);
        let domain_remote = RemoteWorkspace::from_parts(
            workspace,
            domain,
            remote.policy.clone(),
            epochs,
            keys_held.clone(),
            own_envelope.clone(),
        )?;
        let mut frame = Frame::new(domain, counter + 1);
        frame.bootstrap = false;
        frame.epochs = vec![epoch];
        frame.envelope = own_envelope;
        frame.own.head = Some((counter, op.digest()));
        frame.own.acked = Some((counter, op.digest(), seq));
        let key = OpKey { device, counter };
        let object = Object {
            domain: Some(domain),
            own_highest: counter,
            base: Some(key),
            states: BTreeMap::from([(
                key,
                StateInfo {
                    digest: op.digest(),
                    seq: Some(seq),
                    covers: Vec::new(),
                    content: Some(Carried {
                        path: path.as_str().to_owned(),
                        revision,
                        delete: false,
                        object_id: String::new(),
                        kept: None,
                        blob: None,
                        text: None,
                    }),
                    epoch: 1,
                    policy_revision: change.body.revision,
                },
            )]),
            ..Object::default()
        };
        Ok((
            domain,
            frame,
            Runtime {
                keys: keys_held,
                remote: Some(domain_remote),
                ..Runtime::default()
            },
            object,
            marker_rid,
            entry,
        ))
    }

    /// Writes `root`'s marker from what sync knows and records its file.
    pub(super) fn restore_marker(&mut self, root: &RecordedRoot) -> Result<(), SyncError> {
        let path = format!("{}/{MARKER}", root.folder);
        let bytes = marker_content(&root.domain).into_bytes();
        let expected = match self.engine.hash_synced(&path).map_err(core)? {
            Some(there) => local_core::SyncedExpect::Revision(there),
            None => local_core::SyncedExpect::Absent,
        };
        self.engine
            .write_synced(&path, &bytes, &expected)
            .map_err(core)?;
        self.note_marker(root, &path, hash(&bytes))
    }

    fn note_marker(
        &mut self,
        root: &RecordedRoot,
        path: &str,
        revision: Digest,
    ) -> Result<(), SyncError> {
        let (size, modified_ns, file_id) = stat(self.engine.root(), path)
            .ok_or_else(|| SyncError::Invalid("the marker went away".into()))?;
        if let Some(object) = self.state.objects.get_mut(&root.rid) {
            object.local = Some(Local {
                path: path.to_owned(),
                revision,
                size,
                modified_ns,
                file_id,
            });
        }
        Ok(())
    }

    /// The folder another copy of `root`'s marker sits in, when its folder
    /// moved: a local marker naming the domain, away from any other root.
    fn moved_marker(&self, root: &RecordedRoot) -> Option<String> {
        rebuild::local_markers(self.engine.root())
            .into_iter()
            .find(|folder| {
                *folder != root.folder
                    && self
                        .engine
                        .read_synced(&format!("{folder}/{MARKER}"))
                        .ok()
                        .flatten()
                        .is_some_and(|bytes| parse_marker(&bytes) == Some(root.domain))
                    && !self
                        .state
                        .roots
                        .iter()
                        .any(|r| r.domain != root.domain && beneath(folder, &r.folder))
            })
    }

    /// The names in `folder` on disk.
    fn names_in(&self, folder: &str) -> Vec<String> {
        std::fs::read_dir(self.engine.root().join(folder))
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `root`'s marker file left its folder for another one.
    fn marker_moved_away(&self, root: &RecordedRoot) -> bool {
        !marker_present(self.names_in(&root.folder).iter().map(String::as_str))
            && self.moved_marker(root).is_some()
    }

    /// Looks after each recorded root's marker file (§9): one that moved
    /// with its folder is signed at its new path, so the domain moves with
    /// it, unless that takes in another domain's files or the root is held,
    /// which wait for review; one you deleted is written again, since only
    /// `delete_domain` retires a marker; one an outside edit changed is
    /// rewritten; an iCloud placeholder counts as present. Returns how many
    /// records it signed.
    pub(super) fn check_markers(&mut self) -> Result<usize, SyncError> {
        let mut signed = 0;
        for root in self.state.roots.clone() {
            let held = self
                .state
                .held_roots
                .iter()
                .any(|h| h.domain == Some(root.domain));
            let names = self.names_in(&root.folder);
            if !marker_present(names.iter().map(String::as_str)) {
                match self.moved_marker(&root) {
                    Some(moved) if !held => {
                        let target = RecordedRoot {
                            folder: moved.clone(),
                            ..root.clone()
                        };
                        if self.move_changes_domain(&target) {
                            self.hold(root.folder.clone(), root.domain);
                        } else {
                            self.sign_marker_move(&root, &moved)?;
                            signed += 1;
                        }
                    }
                    Some(_) => {}
                    None => self.restore_marker(&root)?,
                }
                continue;
            }
            if names.iter().any(|name| name == MARKER) {
                let path = format!("{}/{MARKER}", root.folder);
                let there = self.engine.read_synced(&path).map_err(core)?;
                if there.as_deref() != Some(marker_content(&root.domain).as_bytes()) {
                    self.restore_marker(&root)?;
                }
            }
        }
        Ok(signed)
    }

    /// Restores `domain`'s marker from synced state (§9): writes its file
    /// at the recorded root and signs a state that covers every live
    /// operation of the marker, which makes it valid again and ends the
    /// hold on the files beneath it.
    pub fn restore_domain_marker(&mut self, domain: Id) -> Result<(), SyncError> {
        let root = self
            .state
            .roots
            .iter()
            .find(|root| root.domain == domain)
            .cloned()
            .ok_or_else(|| SyncError::Invalid("no such restricted folder".into()))?;
        self.restore_marker(&root)?;
        if let Some(object) = self.state.objects.get_mut(&root.rid) {
            object.cover_all = true;
        }
        self.state.refused.retain(|rid| *rid != root.rid);
        self.sign_marker_at(&root, &root.folder)?;
        if let Some(object) = self.state.objects.get_mut(&root.rid) {
            object.cover_all = false;
        }
        self.update_roots()?;
        self.save()
    }

    /// The domain whose marker can end the hold on `folder`: this device
    /// recorded its root and may sign the marker's state (§9).
    pub fn restorable_domain(&self, folder: &str) -> Option<Id> {
        let held = self
            .state
            .held_roots
            .iter()
            .find(|root| root.folder == folder)?;
        let domain = held.domain?;
        let device = self.credentials.keys.device();
        let writes = self
            .remote
            .as_ref()
            .is_some_and(|r| r.policy.head_snapshot().can_write(&device, &domain));
        (writes && self.state.roots.iter().any(|root| root.domain == domain)).then_some(domain)
    }

    /// Signs the marker's state at its new folder and moves the root.
    fn sign_marker_move(&mut self, root: &RecordedRoot, folder: &str) -> Result<(), SyncError> {
        self.sign_marker_at(root, folder)?;
        if let Some(kept) = self.state.roots.iter_mut().find(|r| r.rid == root.rid) {
            kept.folder = folder.to_owned();
        }
        Ok(())
    }

    fn sign_marker_at(&mut self, root: &RecordedRoot, folder: &str) -> Result<(), SyncError> {
        let path = format!("{folder}/{MARKER}");
        let bytes = marker_content(&root.domain).into_bytes();
        let revision = hash(&bytes);
        let (size, modified_ns, file_id) = stat(self.engine.root(), &path)
            .ok_or_else(|| SyncError::Invalid("the marker went away".into()))?;
        let found = Found {
            file: local_core::SyncedFile {
                relative_path: path.clone(),
                size,
                modified_ns,
                file_id,
            },
            revision,
            object_id: String::new(),
            bytes: Some(bytes),
        };
        let rid = root.rid;
        self.in_domain(Some(root.domain), |sync| {
            let covers = sync.edit_covers(&rid);
            sync.sign_file(rid, &found, covers)
        })
    }

    /// Signs a state of `rid` at `path` into its domain's outbox, covering
    /// what this device sees, whatever the client would do on its own, for
    /// tests of records that break the marker rules.
    #[cfg(feature = "test-hooks")]
    pub fn sign_state_at(&mut self, rid: Id, path: &str, bytes: &[u8]) -> Result<(), SyncError> {
        let found = Found {
            file: local_core::SyncedFile {
                relative_path: path.to_owned(),
                size: bytes.len() as u64,
                modified_ns: 0,
                file_id: None,
            },
            revision: hash(bytes),
            object_id: String::new(),
            bytes: Some(bytes.to_vec()),
        };
        let domain = self.object_domain(&rid);
        let kept = self.state.objects.get(&rid).cloned();
        self.in_domain(domain, |sync| {
            let covers = sync.edit_covers(&rid);
            sync.sign_file(rid, &found, covers)
        })?;
        let object = self.state.objects.get_mut(&rid).expect("signed");
        object.local = kept.as_ref().and_then(|o| o.local.clone());
        object.base = kept.and_then(|o| o.base);
        self.save()
    }

    /// Signs a delete of `rid` into its domain's outbox, for the same
    /// tests.
    #[cfg(feature = "test-hooks")]
    pub fn sign_delete_of(&mut self, rid: Id) -> Result<(), SyncError> {
        let domain = self.object_domain(&rid);
        let local = self.state.objects.get(&rid).and_then(|o| o.local.clone());
        self.in_domain(domain, |sync| {
            let covers = sync.edit_covers(&rid);
            sync.sign_delete(rid, covers)
        })?;
        self.state.objects.get_mut(&rid).expect("signed").local = local;
        self.save()
    }

    /// A checkpoint of `domain` at this device's cursor, signed but not
    /// pushed: with no checkpoint accepted there yet, a colluding writer's
    /// checkpoint off the chain, for the tampering tests.
    #[cfg(feature = "test-hooks")]
    pub fn sign_side_checkpoint(
        &mut self,
        domain: Id,
    ) -> Option<sync_protocol::records::CheckpointBundle> {
        let keys = self.credentials.keys.clone();
        self.in_domain(Some(domain), |sync| {
            let remote = sync.remote.as_ref()?;
            sync.state.log.checkpoints.sign(
                sync.state.workspace,
                sync.state.domain,
                sync.state.log.cursor,
                sync.state.log.retention_floor,
                &sync.state.log.live,
                &remote.policy,
                keys.device(),
                keys.signing(),
            )
        })
    }

    /// The latest epoch of `domain` this device accepted.
    pub fn domain_epoch(&self, domain: &Id) -> u64 {
        let epochs = match self.active {
            Some(active) if active == *domain => &self.state.epochs,
            _ => match self.state.restricted.get(domain) {
                Some(frame) => &frame.epochs,
                None => return 0,
            },
        };
        epochs.last().map_or(0, |e| e.body.epoch)
    }

    /// The latest checkpoint of `domain` this device accepted.
    pub fn domain_checkpoint(
        &self,
        domain: &Id,
    ) -> Option<Signed<sync_protocol::records::Checkpoint>> {
        let log = self.log_of(Some(*domain));
        log.checkpoints.accepted.as_ref().map(|a| a.record.clone())
    }

    /// The checkpoints of `domain` that failed their checks here.
    pub fn domain_checkpoint_failures(
        &self,
        domain: &Id,
    ) -> Vec<crate::sync::checkpoints::Failure> {
        self.log_of(Some(*domain)).checkpoints.failures.clone()
    }

    /// The domain roots this device recorded.
    pub fn domain_roots(&self) -> &[RecordedRoot] {
        &self.state.roots
    }

    /// The domain of the object at `rid`, `None` for the workspace domain.
    pub fn object_domain(&self, rid: &Id) -> Option<Id> {
        self.state.objects.get(rid).and_then(|o| o.domain)
    }

    /// How capture reports a file it leaves in place because it crossed a
    /// root, which is a move between domains.
    pub(super) fn skip_crossing(&self, path: &str) -> Skipped {
        Skipped {
            path: path.to_owned(),
            why: super::SkipWhy::CrossesRoot,
        }
    }
}
