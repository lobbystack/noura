//! Moves between key domains (docs/workspace-format/sync-v2.md §9): a file
//! that crosses a root boundary gets a new `rid` in the target domain,
//! whose state operation names the source in `origin`, and a reason-2
//! delete in the source, which names the target. The two records go in one
//! push, so each file sits in exactly one domain throughout. A device never
//! moves a file to a wider audience on its own: such a move waits for you
//! with the number of people who would newly see the file.

use std::collections::{BTreeMap, BTreeSet};

use sync_protocol::codec::{Decode, Encode};
use sync_protocol::matching::{Placement, RecordedRoot, beneath, domain_of};
use sync_protocol::messages::{MAX_BODY, PushFailureDetail, RecordRef};
use sync_protocol::records::{DeleteReason, Operation, Origin, Signed};
use sync_protocol::types::Id;

use super::super::{SyncError, SyncService, random};
use super::capture::Found;
use super::settle::collide;
use super::state::{MovePair, Object};
use super::{PUSH_MARGIN, WorkspaceSync, unix_now};

/// A file that left a restricted folder or would reach more people, which
/// waits until you confirm the move (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveReview {
    /// The object.
    pub rid: Id,
    /// Where the file sits now.
    pub path: String,
    /// The target domain, `None` for the workspace domain.
    pub target: Option<Id>,
    /// How many people who can't read the file now would read it there.
    pub people: usize,
}

/// A file under a held root, with what confirming it would show (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldFile {
    /// Where the file sits.
    pub path: String,
    /// The held folder.
    pub folder: String,
    /// How many people who can't read it now would read it in the domain
    /// it would otherwise go to.
    pub people: usize,
}

/// How long a manager's device leaves the device that restricted a folder
/// to finish its moves, before it moves the rest itself (§9).
pub const MOVE_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

impl WorkspaceSync {
    /// The workspace domain's ID, wherever the device works.
    fn workspace_domain(&self) -> Id {
        match self.active {
            Some(active) => self.state.restricted[&active].domain,
            None => self.state.domain,
        }
    }

    fn domain_id(&self, domain: Option<Id>) -> Id {
        domain.unwrap_or_else(|| self.workspace_domain())
    }

    /// The accounts that read `domain`, at the accepted policy.
    fn reading_accounts(&self, domain: Option<Id>) -> BTreeSet<Id> {
        let id = self.domain_id(domain);
        self.remote.as_ref().map_or_else(BTreeSet::new, |remote| {
            let snapshot = remote.policy.head_snapshot();
            snapshot
                .readers(&id)
                .iter()
                .filter_map(|device| snapshot.account_of(device))
                .collect()
        })
    }

    /// How many people read `to` who don't read `from`.
    pub(super) fn audience_gain(&self, from: Option<Id>, to: Option<Id>) -> usize {
        let before = self.reading_accounts(from);
        self.reading_accounts(to).difference(&before).count()
    }

    /// Whether this device writes both domains of a move.
    pub(super) fn may_move(&self, from: Option<Id>, to: Option<Id>) -> bool {
        let device = self.credentials.keys.device();
        let (from, to) = (self.domain_id(from), self.domain_id(to));
        self.remote.as_ref().is_some_and(|remote| {
            let snapshot = remote.policy.head_snapshot();
            snapshot.can_write(&device, &from) && snapshot.can_write(&device, &to)
        })
    }

    /// Whether the object's latest state operation predates `root`'s
    /// `create_domain`: by position in the workspace domain's log, or, on a
    /// device that bootstrapped past that entry, by the policy revision the
    /// operation was signed at. Anything else is a later write, which a
    /// root never takes in (§9).
    fn sits_before(&self, object: &Object, root: &RecordedRoot) -> bool {
        let Some(created) = self.created_revision(&root.domain) else {
            return false;
        };
        let Some((seq, latest)) = object
            .states
            .values()
            .filter_map(|state| state.seq.map(|seq| (seq, state)))
            .max_by_key(|(seq, _)| *seq)
        else {
            return false;
        };
        match self.log_of(None).domain_entries.get(&created) {
            Some(entry) => seq < *entry,
            None => latest.policy_revision < created,
        }
    }

    /// The root whose files `path` lies among, when it belongs to a
    /// workspace-domain object that stays there until its move arrives
    /// (§9).
    fn awaiting_root(&self, rid: &Id, path: &str) -> Option<&RecordedRoot> {
        let object = self.state.objects.get(rid)?;
        if object.domain.is_some() {
            return None;
        }
        self.state
            .roots
            .iter()
            .find(|root| beneath(path, &root.folder) && self.sits_before(object, root))
    }

    /// Where a workspace-domain object that predates a root sits: at its
    /// path, which the §9 exception keeps until its move lands.
    pub(super) fn stays_for_move(&self, rid: &Id, path: &str) -> bool {
        self.awaiting_root(rid, path).is_some()
    }

    /// Whether this device never pulled `domain`'s `create_domain` entry,
    /// because it bootstrapped past it.
    #[doc(hidden)]
    pub fn bootstrapped_past(&self, domain: &Id) -> bool {
        self.created_revision(domain).is_some_and(|revision| {
            !self.log_of(None).domain_entries.contains_key(&revision)
        })
    }

    /// Whether `rid` waits for its move into the root that now holds it,
    /// so a grantee's edits to it wait too (§9).
    pub(super) fn awaits_move(&self, rid: &Id) -> bool {
        self.synced_path_of(rid)
            .is_some_and(|path| self.stays_for_move(rid, &path))
    }

    fn synced_path_of(&self, rid: &Id) -> Option<String> {
        let object = self.state.objects.get(rid)?;
        object
            .states
            .values()
            .filter(|state| state.seq.is_some() || object.base.is_some())
            .max_by_key(|state| state.seq.unwrap_or(u64::MAX))
            .and_then(|state| state.content.as_ref())
            .filter(|content| !content.delete)
            .map(|content| content.path.clone())
    }

    /// Signs both halves of `rid`'s move to `target` into their domains'
    /// outboxes: the new object's state, with a new `rid` that names the
    /// source in `origin`, and a reason-2 delete of the source that names
    /// the new `rid`. An attachment gets a new blob under a new key, since
    /// the new `rid` has none to lend (§9).
    pub(super) fn sign_move(
        &mut self,
        rid: Id,
        found: &Found,
        target: Option<Id>,
    ) -> Result<Id, SyncError> {
        let source = self.object_domain(&rid);
        let new_rid = random(Id::random())?;
        let pair = MovePair {
            source: rid,
            target: new_rid,
        };
        let origin = Origin {
            domain: self.domain_id(source),
            rid,
        };
        let moved_to = Origin {
            domain: self.domain_id(target),
            rid: new_rid,
        };
        self.state.objects.insert(
            new_rid,
            Object {
                object_id: found.object_id.clone(),
                domain: target,
                ..Object::default()
            },
        );
        self.in_domain(target, |sync| {
            sync.sign_file_from(new_rid, found, Vec::new(), Some(origin))?;
            sync.mark_pair(pair);
            Ok::<_, SyncError>(())
        })?;
        self.in_domain(source, |sync| {
            let covers = sync.edit_covers(&rid);
            sync.sign_removal(rid, covers, DeleteReason::Moved, Some(moved_to))?;
            sync.mark_pair(pair);
            Ok::<_, SyncError>(())
        })?;
        Ok(new_rid)
    }

    fn mark_pair(&mut self, pair: MovePair) {
        if let Some(outgoing) = self.state.outbox.last_mut() {
            outgoing.pair = Some(pair);
        }
    }

    /// Handles a file capture found across a root from its object: moves
    /// it, or lists it for review. Returns how many records it signed.
    pub(super) fn crossing(
        &mut self,
        rid: Id,
        found: &Found,
        source: Option<Id>,
        target: Option<Id>,
    ) -> Result<usize, SyncError> {
        let path = found.file.relative_path.clone();
        if self.awaits_move(&rid) {
            self.skipped_now.push(self.skip_crossing(&path));
            return Ok(0);
        }
        let people = self.audience_gain(source, target);
        if people > 0 && !self.confirmed_moves.contains(&rid) {
            self.reviews.push(MoveReview {
                rid,
                path: path.clone(),
                target,
                people,
            });
            self.skipped_now.push(self.skip_crossing(&path));
            return Ok(0);
        }
        if !self.may_move(source, target) {
            self.skipped_now.push(self.skip_crossing(&path));
            return Ok(0);
        }
        self.sign_move(rid, found, target)?;
        self.confirmed_moves.remove(&rid);
        if let Some(domain) = target
            && !self.moved_in.iter().any(|(_, p)| *p == path)
        {
            self.moved_in.push((domain, path));
        }
        Ok(2)
    }

    /// The moves that wait for you, each with how many people it would
    /// show the file to.
    pub fn moves_for_review(&self) -> &[MoveReview] {
        &self.reviews
    }

    /// The files you moved into a restricted folder, whose earlier versions
    /// stay in the source domain's log (§9), each with its domain. The app
    /// shows them until you dismiss them.
    pub fn moved_in(&self) -> &[(Id, String)] {
        &self.moved_in
    }

    /// Dismisses the notice about files moved into a restricted folder.
    pub fn dismiss_moved_in(&mut self) {
        self.moved_in.clear();
    }

    /// Confirms a move that waited for review; the next round signs it.
    pub fn confirm_move(&mut self, rid: Id) {
        self.confirmed_moves.insert(rid);
    }

    /// The files under held roots, each with how many people it would
    /// reach in the domain it would otherwise go to (§9).
    pub fn held_files(&self) -> Vec<HeldFile> {
        let Ok(listing) = self.engine.synced_listing() else {
            return Vec::new();
        };
        let mut held = Vec::new();
        for file in listing.files {
            let path = file.relative_path;
            let Some(root) = self
                .state
                .held_roots
                .iter()
                .find(|root| beneath(&path, &root.folder))
            else {
                continue;
            };
            let others: Vec<RecordedRoot> = self
                .state
                .roots
                .iter()
                .filter(|r| Some(r.domain) != root.domain)
                .cloned()
                .collect();
            let people = self.audience_gain(root.domain, domain_of(&path, &others));
            held.push(HeldFile {
                folder: root.folder.clone(),
                path,
                people,
            });
        }
        held
    }

    /// Confirms where the files of a held `folder` go (§9): each goes to
    /// the domain its path falls in without the held root. Objects of a
    /// domain the device can still write move there. Those of a deleted
    /// domain, or one the device no longer reads, can't be signed out of
    /// it, so the next round uploads their files as new ones.
    pub fn confirm_held(&mut self, folder: &str) -> Result<(), SyncError> {
        let held: Vec<Option<Id>> = self
            .state
            .held_roots
            .iter()
            .filter(|root| root.folder == folder)
            .map(|root| root.domain)
            .collect();
        if held.is_empty() {
            return Err(SyncError::Invalid("no such held folder".into()));
        }
        self.state.held_roots.retain(|root| root.folder != folder);
        for domain in held.into_iter().flatten() {
            let device = self.credentials.keys.device();
            let writes = self
                .remote
                .as_ref()
                .is_some_and(|remote| remote.policy.head_snapshot().can_write(&device, &domain));
            let recorded = self
                .state
                .roots
                .iter()
                .position(|root| root.domain == domain && root.folder == folder);
            if let Some(index) = recorded {
                let root = self.state.roots.remove(index);
                if !writes {
                    self.forget_marker(&root)?;
                }
            }
            let rids: Vec<Id> = self
                .state
                .objects
                .iter()
                .filter(|(_, object)| {
                    object.domain == Some(domain)
                        && object
                            .local
                            .as_ref()
                            .is_some_and(|local| beneath(&local.path, folder))
                })
                .map(|(rid, _)| *rid)
                .collect();
            for rid in rids {
                if writes {
                    self.confirmed_moves.insert(rid);
                } else {
                    self.state.objects.remove(&rid);
                }
            }
        }
        self.save()
    }

    /// Removes the marker file of a root that no longer stands.
    fn forget_marker(&mut self, root: &RecordedRoot) -> Result<(), SyncError> {
        let path = format!("{}/{}", root.folder, sync_protocol::matching::MARKER);
        if let Some(revision) = self.engine.hash_synced(&path).map_err(super::core)? {
            self.engine
                .delete_synced(&path, &revision)
                .map_err(super::core)?;
        }
        self.state.objects.remove(&root.rid);
        Ok(())
    }

    /// Signs the moves this device owes: each workspace-domain object that
    /// predates a domain it manages, once that domain's moves fall due
    /// here. The device that restricted a folder owes them at once; any
    /// other manager's device leaves it `move_wait` first (§9). Returns how
    /// many records it signed.
    pub(super) fn finish_moves(&mut self) -> Result<usize, SyncError> {
        let Some(remote) = self.remote.as_ref() else {
            return Ok(0);
        };
        let snapshot = remote.policy.head_snapshot();
        let own = self.credentials.account;
        let managed: Vec<RecordedRoot> = self
            .state
            .roots
            .iter()
            .filter(|root| snapshot.manages(&own, &root.domain))
            .filter(|root| {
                !self
                    .state
                    .held_roots
                    .iter()
                    .any(|held| held.domain == Some(root.domain))
            })
            .cloned()
            .collect();
        let mut signed = 0;
        for root in managed {
            let rids: Vec<Id> = self
                .state
                .objects
                .iter()
                .filter(|(rid, object)| {
                    object.domain.is_none()
                        && !self.state.held.contains_key(rid)
                        && object
                            .local
                            .as_ref()
                            .is_some_and(|local| beneath(&local.path, &root.folder))
                        && self.awaits_move(rid)
                })
                .map(|(rid, _)| *rid)
                .collect();
            if rids.is_empty() {
                self.state.move_due.remove(&root.domain);
                continue;
            }
            let wait = self.move_wait.as_secs();
            let due = *self
                .state
                .move_due
                .entry(root.domain)
                .or_insert_with(|| unix_now() + wait);
            if unix_now() < due || !self.may_move(None, Some(root.domain)) {
                continue;
            }
            for rid in rids {
                let Some(path) = self.state.objects[&rid]
                    .local
                    .as_ref()
                    .map(|l| l.path.clone())
                else {
                    continue;
                };
                let Ok(found) = self.found_on_disk(&path) else {
                    continue;
                };
                self.sign_move(rid, &found, Some(root.domain))?;
                signed += 2;
            }
        }
        Ok(signed)
    }

    /// The local file of an object whose move this page brings is the new
    /// object's now, when the move's source is what the device holds: the
    /// same bytes, or the same frontmatter ID, so a member of the target
    /// domain can't take another object's file by naming it (§9).
    pub(super) fn arrive(&mut self, moved: Id, origin: Origin) {
        let Some(source) = self.state.objects.get(&origin.rid) else {
            return;
        };
        let same_domain = self.domain_id(source.domain) == origin.domain;
        let new = &self.state.objects[&moved];
        let state = new
            .states
            .iter()
            .find_map(|(key, state)| state.content.as_ref().map(|c| (*key, c)));
        let same = source.local.as_ref().is_some_and(|local| {
            state.is_some_and(|(_, c)| {
                c.revision == local.revision
                    || (!c.object_id.is_empty() && c.object_id == source.object_id)
            })
        });
        // The same file at the same path is the state the device's edits
        // build on.
        let base = source.local.as_ref().and_then(|local| {
            state
                .filter(|(_, c)| c.revision == local.revision && c.path == local.path)
                .map(|(key, _)| key)
        });
        if same_domain && same && new.local.is_none() {
            let local = self
                .state
                .objects
                .get_mut(&origin.rid)
                .and_then(|o| o.local.take());
            if let Some(object) = self.state.objects.get_mut(&moved) {
                object.local = local;
                object.base = base;
            }
        }
        if self.state.deferred_moves.get(&origin.rid) == Some(&moved) {
            self.state.deferred_moves.remove(&origin.rid);
            self.state.moved_away.push(origin.rid);
        }
    }

    /// Whether a reason-2 delete that names `target` waits for the target
    /// domain: the device reads it and hasn't seen the new object (§9).
    pub(super) fn defers_delete(&self, target: &Origin) -> bool {
        let device = self.credentials.keys.device();
        let reads = self
            .remote
            .as_ref()
            .is_some_and(|r| r.policy.head_snapshot().can_read(&device, &target.domain));
        reads && !self.state.objects.contains_key(&target.rid)
    }

    /// Settles the deletes whose moves arrived.
    pub(super) fn settle_moved_away(&mut self) -> Result<(), SyncError> {
        for rid in std::mem::take(&mut self.state.moved_away) {
            let domain = self.object_domain(&rid);
            self.in_domain(domain, |sync| sync.settle(rid, &BTreeMap::new()))?;
        }
        Ok(())
    }

    /// The frame that holds `domain`'s records, `None` for the workspace
    /// domain.
    fn frame_of(&self, domain: Id) -> Option<Id> {
        (domain != self.workspace_domain()).then_some(domain)
    }

    fn outbox_of(&self, frame: Option<Id>) -> &[super::Outgoing] {
        match frame {
            None => &self.state.outbox,
            Some(domain) => self
                .state
                .restricted
                .get(&domain)
                .map_or(&[], |f| f.outbox.as_slice()),
        }
    }

    fn frames(&self) -> Vec<Option<Id>> {
        std::iter::once(None)
            .chain(self.state.restricted.keys().copied().map(Some))
            .collect()
    }

    /// The pairs at the outboxes' heads, target first, up to a push's
    /// size: a move's two records leave together or not at all.
    fn pair_batch(&self) -> Vec<Signed<Operation>> {
        let mut at: BTreeMap<Option<Id>, usize> = BTreeMap::new();
        let mut batch = Vec::new();
        let mut size = 0;
        loop {
            let heads: Vec<(Option<Id>, &super::Outgoing)> = self
                .frames()
                .into_iter()
                .filter_map(|frame| {
                    let index = at.get(&frame).copied().unwrap_or(0);
                    let box_ref = self.outbox_of(frame);
                    box_ref.get(index).map(|o| (frame, o))
                })
                .collect();
            let found = heads.iter().find_map(|(frame, head)| {
                let pair = head.pair?;
                heads
                    .iter()
                    .find(|(other, o)| other != frame && o.pair == Some(pair))
                    .map(|(other, o)| ((*frame, *head), (*other, *o), pair))
            });
            let Some(((a_frame, a), (b_frame, b), pair)) = found else {
                return batch;
            };
            let (target, source) = if a.record.body.rid == pair.target {
                ((a_frame, a), (b_frame, b))
            } else {
                ((b_frame, b), (a_frame, a))
            };
            let len = RecordRef::Operation(target.1.record.clone())
                .to_bytes()
                .len()
                + RecordRef::Operation(source.1.record.clone())
                    .to_bytes()
                    .len();
            if !batch.is_empty() && size + len > MAX_BODY - PUSH_MARGIN {
                return batch;
            }
            size += len;
            batch.push(target.1.record.clone());
            batch.push(source.1.record.clone());
            *at.entry(target.0).or_default() += 1;
            *at.entry(source.0).or_default() += 1;
        }
    }

    /// Pushes the moves at the outboxes' heads, each pair in one push. A
    /// refused record goes through the same handling as any record of its
    /// domain, and a pair that loses a half comes undone: the file goes
    /// back to its object, and the next round tries again. Returns how many
    /// records the server acknowledged.
    pub(super) async fn push_pairs(
        &mut self,
        service: &mut SyncService,
    ) -> Result<usize, SyncError> {
        let mut acknowledged = 0;
        let mut attempts = 0;
        loop {
            let batch = self.pair_batch();
            if batch.is_empty() {
                return Ok(acknowledged);
            }
            attempts += 1;
            if attempts > 64 {
                return Err(SyncError::Network(
                    "the server keeps refusing the push".into(),
                ));
            }
            let records: Vec<RecordRef> = batch.iter().cloned().map(RecordRef::Operation).collect();
            match service.client().push(self.state.workspace, records).await {
                Ok(response) => {
                    for (record, receipt) in batch.iter().zip(response.receipts.0.iter()) {
                        let frame = self.frame_of(record.body.domain);
                        self.in_domain(frame, |sync| {
                            let outgoing = sync.state.outbox.remove(0);
                            sync.acknowledge(outgoing, receipt)
                        })?;
                    }
                    acknowledged += batch.len();
                    self.save()?;
                    let accepted = self.state.policy.last().map_or(0, |c| c.body.revision);
                    if response.policy_revision > accepted {
                        self.refresh(service).await?;
                    }
                }
                Err(SyncError::Refused { code, detail, .. }) => {
                    let failure = detail
                        .as_deref()
                        .and_then(|d| PushFailureDetail::from_bytes(d).ok());
                    let index = failure.as_ref().map_or(0, |f| f.record_index as usize);
                    let Some(record) = batch.get(index) else {
                        return Err(SyncError::Refused {
                            code,
                            status: 0,
                            detail: None,
                        });
                    };
                    let domain = record.body.domain;
                    let frame = self.frame_of(domain);
                    let position = batch[..index]
                        .iter()
                        .filter(|r| r.body.domain == domain)
                        .count();
                    let record_code = failure.as_ref().map_or(code.clone(), |f| f.code.0.clone());
                    let record_detail = failure
                        .as_ref()
                        .and_then(|f| f.record_detail.as_ref().map(|d| d.0.clone()));
                    let before = self.open_pairs();
                    if let Some(frame) = frame {
                        self.swap_domain(frame);
                    }
                    let handled = self
                        .refused_at(service, position, &record_code, record_detail)
                        .await;
                    if let Some(frame) = frame {
                        self.swap_domain(frame);
                    }
                    self.undo_broken_pairs(&before)?;
                    self.save()?;
                    handled?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// How many records of each pair wait, across the outboxes.
    fn open_pairs(&self) -> BTreeMap<MovePair, usize> {
        let mut pairs = BTreeMap::new();
        for frame in self.frames() {
            for outgoing in self.outbox_of(frame) {
                if let Some(pair) = outgoing.pair {
                    *pairs.entry(pair).or_default() += 1;
                }
            }
        }
        pairs
    }

    /// Undoes each pair of `before` that lost a record: drops its other
    /// half, and hands the file back to the source object.
    fn undo_broken_pairs(&mut self, before: &BTreeMap<MovePair, usize>) -> Result<(), SyncError> {
        let now = self.open_pairs();
        for pair in before.keys() {
            if now.get(pair) == Some(&2) {
                continue;
            }
            for frame in self.frames() {
                self.in_domain(frame, |sync| {
                    if let Some(index) =
                        sync.state.outbox.iter().position(|o| o.pair == Some(*pair))
                    {
                        let rejected = sync.state.outbox.remove(index);
                        sync.forget_own(&rejected.record);
                        sync.reseal_replacing_or_end(index, &rejected.record)?;
                    }
                    Ok::<_, SyncError>(())
                })?;
            }
            let local = self
                .state
                .objects
                .remove(&pair.target)
                .and_then(|object| object.local);
            if let Some(object) = self.state.objects.get_mut(&pair.source) {
                object.local = local.map(|mut local| {
                    local.revision = sync_protocol::types::Digest::ZERO;
                    local.size = u64::MAX;
                    local
                });
            }
        }
        Ok(())
    }

    /// Where the placement rules put `rid`'s file, with the §9 exception
    /// for an object that waits for its move. The exception gives way to
    /// the root's own objects: a waiting object never takes a path from one.
    pub(super) fn placement_with_exception(&self, rid: &Id, path: &str) -> Placement {
        let keeps_path = self.awaiting_root(rid, path).is_some_and(|root| {
            !self.state.objects.iter().any(|(other, object)| {
                other != rid
                    && object.domain == Some(root.domain)
                    && object
                        .local
                        .as_ref()
                        .is_some_and(|local| collide(path, &local.path))
            })
        });
        if keeps_path {
            return Placement::At(path.to_owned());
        }
        self.placement_of(rid, path)
    }
}
