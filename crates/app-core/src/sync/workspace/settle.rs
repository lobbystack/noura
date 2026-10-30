//! Bringing an object's file in line with its live set
//! (docs/workspace-format/sync-v2.md §16): one live state is written to
//! disk through `local-core`'s checked writes, unless the local file holds
//! edits sync hasn't captured yet, which it never overwrites. Two or more
//! live states are a conflict: the device whose file matches a branch or
//! holds local edits keeps its file, every other branch goes to the review
//! store under `.noura/sync/<device-id>/conflicts/`, and a device that holds
//! neither shows the branch with the lowest operation digest. Identical
//! branches aren't a conflict, a path-only change and a content-only
//! change merge, and an edit against a delete keeps the edit and asks.
//!
//! Two objects collide when their paths match after NFC normalization and
//! simple case folding, or when one path is a folder prefix of the other's.
//! The one whose current state operation has the higher digest yields: its
//! author's device moves it to the first free `name (n).ext`, and other
//! devices show it there without writing until the move arrives.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use local_core::SyncedExpect;
use sync_protocol::types::{Digest, Id};

use super::state::{Content as Carried, Local, OpKey};
use super::{SyncError, WorkspaceSync, core, journal, same_state, stat};

pub(crate) use sync_protocol::matching::fold;
use sync_protocol::matching::{Placement, marker_folder, refuses_marker_path};

/// The bytes of a version sync holds while it applies a page: in memory, as
/// an inline payload carries them, or in a file outside the workspace, such
/// as a download in the journal's `downloads/` or a review store copy, which
/// sync streams so a large attachment never sits in memory. Every write
/// from a file checks the bytes against the version's revision as they
/// stream.
#[derive(Clone, Debug)]
pub(crate) enum Body {
    /// The bytes themselves.
    Bytes(Vec<u8>),
    /// A file whose bytes hashed to the version's revision.
    File(PathBuf),
}

/// The bodies of the files a page carried, by revision.
pub(crate) type Bodies = BTreeMap<Digest, Body>;

impl Body {
    /// The bytes, when they sit in memory.
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Body::Bytes(bytes) => Some(bytes),
            Body::File(_) => None,
        }
    }

    /// How many bytes it holds.
    pub(crate) fn len(&self) -> u64 {
        match self {
            Body::Bytes(bytes) => bytes.len() as u64,
            Body::File(path) => std::fs::metadata(path).map_or(0, |m| m.len()),
        }
    }
}

/// `path` as the file system names it: each component as its folder lists
/// it, which differs in case on a file system that ignores case.
pub(crate) fn actual_path(root: &std::path::Path, path: &str) -> String {
    let mut folder = root.to_owned();
    let mut parts = Vec::new();
    for component in path.split('/') {
        let exact = folder.join(component);
        let name = if std::fs::symlink_metadata(&exact).is_ok() {
            std::fs::read_dir(&folder)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .find(|name| name == component)
                .or_else(|| {
                    std::fs::read_dir(&folder)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter_map(|entry| entry.file_name().into_string().ok())
                        .find(|name| fold(name) == fold(component))
                })
                .unwrap_or_else(|| component.to_owned())
        } else {
            component.to_owned()
        };
        folder = folder.join(&name);
        parts.push(name);
    }
    parts.join("/")
}

/// Whether two paths collide (§16): equal once folded, or one a folder
/// prefix of the other.
pub(crate) fn collide(a: &str, b: &str) -> bool {
    let (a, b) = (fold(a), fold(b));
    a == b || b.starts_with(&format!("{a}/")) || a.starts_with(&format!("{b}/"))
}

/// The first free `name (n).ext`, with n counting up from 2, among
/// `taken` and the files on disk under `root`.
pub(crate) fn free_name(path: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let (folder, file) = match path.rsplit_once('/') {
        Some((folder, file)) => (Some(folder), file),
        None => (None, path),
    };
    let (stem, extension) = match file.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (file, None),
    };
    for n in 2.. {
        let name = match extension {
            Some(extension) => format!("{stem} ({n}).{extension}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = match folder {
            Some(folder) => format!("{folder}/{name}"),
            None => name,
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("some name is free")
}

impl WorkspaceSync {
    /// The `marker_rid` of each domain in the verified policy.
    fn marker_rids(&self) -> Vec<Id> {
        self.remote.as_ref().map_or_else(Vec::new, |r| {
            r.policy
                .head_snapshot()
                .domains
                .values()
                .filter_map(|d| d.marker_rid)
                .collect()
        })
    }

    /// Brings `rid`'s file in line with its live set. `contents` holds the
    /// bytes of the files the page carried, by revision.
    pub(super) fn settle(&mut self, rid: Id, contents: &Bodies) -> Result<(), SyncError> {
        let Some(object) = self.state.objects.get(&rid).cloned() else {
            return Ok(());
        };
        let branches = self.branches(&rid, &object);
        if branches.is_empty() {
            return Ok(());
        }
        let edited = self.local_edit(&rid);
        if branches.len() == 1 {
            // A text-mode note's file follows its document (§22).
            let text = object
                .states
                .get(&branches[0].key)
                .and_then(|state| state.content.as_ref())
                .is_some_and(|content| content.text.is_some());
            if text {
                return Ok(());
            }
            return self.adopt_state(rid, branches[0].key, edited, contents);
        }
        if let Some((path_branch, content_branch)) = self.disjoint(&rid, &branches) {
            return self.merge(rid, path_branch, content_branch, edited, contents);
        }
        let deletes: Vec<_> = branches.iter().filter(|b| b.path.is_none()).collect();
        let edit_against_delete = deletes.len() == 1 && branches.len() == 2;
        let on_disk = branches.iter().any(|b| b.on_disk && b.path.is_some());
        if !edited && !on_disk {
            // The device holds neither: it shows the lowest digest, or the
            // edit when the other branch deletes.
            let show = if edit_against_delete {
                branches
                    .iter()
                    .find(|b| b.path.is_some())
                    .expect("one edit")
            } else {
                &branches[0]
            };
            let show = show.key;
            self.adopt_state(rid, show, false, contents)?;
        }
        for branch in &branches {
            self.keep_for_review(&rid, &branch.key, contents)?;
        }
        Ok(())
    }

    /// Whether `rid`'s file holds edits sync hasn't captured: its bytes
    /// differ from the ones the journal last saw, or it went away.
    pub(super) fn local_edit(&self, rid: &Id) -> bool {
        let Some(local) = self.state.objects.get(rid).and_then(|o| o.local.as_ref()) else {
            return false;
        };
        match stat(self.engine.root(), &local.path) {
            None => true,
            Some((size, modified, file_id))
                if size == local.size
                    && modified == local.modified_ns
                    && file_id == local.file_id =>
            {
                false
            }
            Some(_) => match self.engine.hash_synced(&local.path) {
                Ok(Some(revision)) => revision != local.revision.to_hex(),
                _ => true,
            },
        }
    }

    /// Writes the state at `target` to disk when the file doesn't hold
    /// edits, else keeps it for review.
    fn adopt_state(
        &mut self,
        rid: Id,
        target: OpKey,
        edited: bool,
        contents: &Bodies,
    ) -> Result<(), SyncError> {
        let own = self.credentials.keys.device();
        let object = &self.state.objects[&rid];
        // This device's own state is on disk already, unless a lost
        // journal left the object with no file, as when the file went too.
        if object.base == Some(target) || (target.device == own && object.local.is_some()) {
            return Ok(());
        }
        let Some(content) = object.states.get(&target).and_then(|s| s.content.clone()) else {
            return Ok(());
        };
        // Only `delete_domain` retires a marker: a delete of one, or a move
        // off a marker path, keeps the file and the recorded root, and the
        // files beneath it wait (§9).
        if self.marker_objects().contains(&rid)
            && (content.delete || marker_folder(&content.path).is_none())
        {
            if !self.state.refused.contains(&rid) {
                self.state.refused.push(rid);
            }
            return self.keep_for_review(&rid, &target, contents);
        }
        // An object at a `.noura-domain` path that no domain names as its
        // marker would hold files after a sync without the journal: the
        // device writes none and lists it for review (§9).
        if !content.delete && refuses_marker_path(&rid, &content.path, &self.marker_rids()) {
            if !self.state.refused.contains(&rid) {
                self.state.refused.push(rid);
            }
            return self.keep_for_review(&rid, &target, contents);
        }
        if edited {
            return self.keep_for_review(&rid, &target, contents);
        }
        if content.delete {
            if let Some(local) = object.local.clone() {
                match self
                    .engine
                    .delete_synced(&local.path, &local.revision.to_hex())
                {
                    Ok(()) => {}
                    Err(error) if error.code == "revision_conflict" => {
                        return self.keep_for_review(&rid, &target, contents);
                    }
                    Err(error) => return Err(core(error)),
                }
            }
            let object = self.state.objects.get_mut(&rid).expect("present");
            object.local = None;
            object.base = Some(target);
            return Ok(());
        }
        self.hold_uncovered(&rid, &content.path);
        let synced = match self.placement_with_exception(&rid, &content.path) {
            Placement::At(path) | Placement::Shared(path) => path,
            // A path that left the object's root: the file stays, and the
            // change goes to review (§9).
            Placement::Outside(_) => return self.keep_for_review(&rid, &target, contents),
        };
        let body = self.content_body(&rid, &target, contents)?;
        let (path, displaced) = self.destination(&rid, &target, &synced);
        for other in displaced {
            if let Err(error) = self.move_aside(&other) {
                tracing::warn!(%error, "a file that yields its path couldn't move aside");
                return self.keep_for_review(&rid, &target, contents);
            }
        }
        match self.place(&rid, &path, &body, content.revision) {
            Ok(()) => {
                self.state.objects.get_mut(&rid).expect("present").base = Some(target);
                Ok(())
            }
            Err(SyncError::Invalid(message)) if message.starts_with("revision_conflict") => {
                self.keep_for_review(&rid, &target, contents)
            }
            Err(error) => Err(error),
        }
    }

    /// Puts `body`, whose bytes hash to `revision`, at `path` as `rid`'s
    /// file: moves its current file there when it has one, writes the bytes
    /// when they differ, and records the file in the journal. A file that
    /// changed since the journal saw it, or another file at `path`, fails
    /// with `revision_conflict`.
    pub(super) fn place(
        &mut self,
        rid: &Id,
        path: &str,
        body: &Body,
        revision: Digest,
    ) -> Result<(), SyncError> {
        let local = self.state.objects.get(rid).and_then(|o| o.local.clone());
        let write = |expected: &SyncedExpect| {
            match body {
                Body::Bytes(bytes) => self.engine.write_synced(path, bytes, expected),
                Body::File(source) => {
                    self.engine
                        .write_synced_from(path, source, &revision.to_hex(), expected)
                }
            }
            .map_err(Box::new)
        };
        match local {
            Some(local) => {
                if local.path != path {
                    self.engine
                        .move_synced(&local.path, path, &local.revision.to_hex())
                        .map_err(core)?;
                }
                if local.revision != revision {
                    write(&SyncedExpect::Revision(local.revision.to_hex()))
                        .map_err(|error| core(*error))?;
                }
            }
            None => match write(&SyncedExpect::Absent) {
                Ok(()) => {}
                Err(error) if error.code == "revision_conflict" => {
                    // Already there, as after a restart mid-page.
                    let there = self.engine.hash_synced(path).ok().flatten();
                    if there != Some(revision.to_hex()) {
                        return Err(core(*error));
                    }
                }
                Err(error) => return Err(core(*error)),
            },
        }
        // A folder that differs only in case on a file system that ignores
        // case keeps its own name: the journal records the file where the
        // walk will find it.
        let path = actual_path(self.engine.root(), path);
        let (size, modified_ns, file_id) = stat(self.engine.root(), &path)
            .ok_or_else(|| SyncError::Invalid("the written file went away".into()))?;
        self.state.objects.get_mut(rid).expect("present").local = Some(Local {
            path,
            revision,
            size,
            modified_ns,
            file_id,
        });
        Ok(())
    }

    /// The bytes of the state at `key`: from the page, the review store, or
    /// a file on disk that holds them, its own or another object's, which
    /// it hashes again first.
    pub(super) fn content_body(
        &self,
        rid: &Id,
        key: &OpKey,
        contents: &Bodies,
    ) -> Result<Body, SyncError> {
        let object = &self.state.objects[rid];
        let content = object
            .states
            .get(key)
            .and_then(|s| s.content.as_ref())
            .ok_or_else(|| SyncError::Invalid("that version didn't open".into()))?;
        if let Some(body) = contents.get(&content.revision) {
            return Ok(body.clone());
        }
        if let Some(kept) = &content.kept
            && let Ok(path) = self.journal.kept_path(kept, &content.revision)
        {
            return Ok(Body::File(path));
        }
        let own = object.local.iter();
        let others = self
            .state
            .objects
            .iter()
            .filter(|(other, _)| *other != rid)
            .filter_map(|(_, o)| o.local.as_ref());
        for local in own.chain(others) {
            if local.revision == content.revision
                && self.engine.hash_synced(&local.path).ok().flatten()
                    == Some(content.revision.to_hex())
            {
                return Ok(Body::File(self.engine.root().join(&local.path)));
            }
        }
        Err(SyncError::Invalid(
            "the bytes of that version are gone".into(),
        ))
    }

    /// Keeps the bytes of the state at `key` in the review store, unless
    /// the file on disk holds them or the store already does.
    fn keep_for_review(
        &mut self,
        rid: &Id,
        key: &OpKey,
        contents: &Bodies,
    ) -> Result<(), SyncError> {
        let object = &self.state.objects[rid];
        let Some(content) = object.states.get(key).and_then(|s| s.content.clone()) else {
            return Ok(());
        };
        if content.delete || content.kept.is_some() {
            return Ok(());
        }
        let on_disk = object
            .local
            .as_ref()
            .is_some_and(|l| l.path == content.path && l.revision == content.revision);
        if on_disk {
            return Ok(());
        }
        let Ok(body) = self.content_body(rid, key, contents) else {
            return Ok(());
        };
        let digest = object.states[key].digest;
        let relative = format!(
            "{}/{}/{}",
            journal::CONFLICTS,
            rid.to_hex(),
            digest.to_hex()
        );
        match &body {
            Body::Bytes(bytes) => self.journal.keep(&relative, bytes)?,
            Body::File(source) => self
                .journal
                .keep_file(&relative, source, &content.revision)?,
        }
        if let Some(content) = self
            .state
            .objects
            .get_mut(rid)
            .and_then(|o| o.states.get_mut(key))
            .and_then(|s| s.content.as_mut())
        {
            content.kept = Some(relative);
        }
        Ok(())
    }

    /// Two branches that changed only the path and only the content,
    /// relative to the one base operation their covers name (§16): the
    /// path branch first.
    fn disjoint(&self, rid: &Id, branches: &[super::Branch]) -> Option<(OpKey, OpKey)> {
        let [a, b] = branches else {
            return None;
        };
        let object = &self.state.objects[rid];
        let content = |key: &OpKey| object.states.get(key).and_then(|s| s.content.clone());
        let (ca, cb) = (content(&a.key)?, content(&b.key)?);
        if ca.delete || cb.delete {
            return None;
        }
        let base = self.base_of(rid, &a.key)?;
        if self.base_of(rid, &b.key)? != base {
            return None;
        }
        let cbase = content(&base)?;
        if cbase.delete {
            return None;
        }
        let path_only = |c: &Carried| c.path != cbase.path && c.revision == cbase.revision;
        let content_only = |c: &Carried| c.path == cbase.path && c.revision != cbase.revision;
        if path_only(&ca) && content_only(&cb) {
            Some((a.key, b.key))
        } else if path_only(&cb) && content_only(&ca) {
            Some((b.key, a.key))
        } else {
            None
        }
    }

    /// The state operation `key` started from: the latest one in the
    /// journal that its covers name.
    fn base_of(&self, rid: &Id, key: &OpKey) -> Option<OpKey> {
        let object = &self.state.objects[rid];
        let covers = &object.states.get(key)?.covers;
        object
            .states
            .iter()
            .filter(|(other, _)| *other != key)
            .filter(|(other, _)| {
                covers
                    .iter()
                    .any(|c| c.device == other.device && other.counter <= c.counter)
            })
            .max_by_key(|(_, info)| info.seq.unwrap_or(0))
            .map(|(other, _)| *other)
    }

    /// Materializes the merge of a path-only and a content-only branch,
    /// and, on a device that may write the domain, signs the merge
    /// operation that covers both (§16).
    fn merge(
        &mut self,
        rid: Id,
        path_branch: OpKey,
        content_branch: OpKey,
        edited: bool,
        contents: &Bodies,
    ) -> Result<(), SyncError> {
        if edited {
            self.keep_for_review(&rid, &path_branch, contents)?;
            return self.keep_for_review(&rid, &content_branch, contents);
        }
        let object = &self.state.objects[&rid];
        let path = object.states[&path_branch]
            .content
            .as_ref()
            .map(|c| c.path.clone())
            .expect("a file");
        let revision = object.states[&content_branch]
            .content
            .as_ref()
            .map(|c| c.revision)
            .expect("a file");
        let body = self.content_body(&rid, &content_branch, contents)?;
        match self.place(&rid, &path, &body, revision) {
            Ok(()) => {}
            Err(SyncError::Invalid(message)) if message.starts_with("revision_conflict") => {
                self.keep_for_review(&rid, &path_branch, contents)?;
                return self.keep_for_review(&rid, &content_branch, contents);
            }
            Err(error) => return Err(error),
        }
        let device = self.credentials.keys.device();
        let writes = self.remote.as_ref().is_some_and(|r| {
            r.policy
                .head_snapshot()
                .can_write(&device, &self.state.domain)
        });
        if writes && self.stopped.is_none() {
            let view = BTreeSet::from([path_branch, content_branch]);
            let covers = self.covers_of(&rid, &view);
            let found = self.found_on_disk(&path)?;
            self.sign_file(rid, &found, covers)?;
        } else {
            self.state.objects.get_mut(&rid).expect("present").base = Some(content_branch);
        }
        Ok(())
    }

    /// Where `rid`'s state at `key` goes on this device: its path, or,
    /// when it collides with another object's and yields, the first free
    /// `name (n).ext`. The yielding object's author moves it with an
    /// operation in its next round; other devices only show it there (§16).
    /// Also returns the objects at colliding paths that yield to this one,
    /// which move aside first.
    pub(super) fn destination(&self, rid: &Id, key: &OpKey, path: &str) -> (String, Vec<Id>) {
        let digest = self.state.objects[rid].states[key].digest;
        let others = self.current_paths(rid);
        let colliding: Vec<&(Id, String, Digest)> = others
            .iter()
            .filter(|(_, other, _)| collide(path, other))
            .collect();
        // An object of a restricted domain outranks a workspace-domain
        // object that only waits for its move into that root (§9).
        let restricted = self.state.objects[rid].domain.is_some();
        let yields = colliding.iter().any(|(other, _, current)| {
            digest > *current && !(restricted && self.awaits_move(other))
        });
        if yields {
            return (self.free_for(rid, path), Vec::new());
        }
        let displaced = colliding.iter().map(|(other, _, _)| *other).collect();
        (path.to_owned(), displaced)
    }

    /// The other objects' local paths with the digests of their current
    /// state operations.
    fn current_paths(&self, rid: &Id) -> Vec<(Id, String, Digest)> {
        self.state
            .objects
            .iter()
            .filter(|(other, _)| *other != rid)
            .filter_map(|(other, o)| {
                let local = o.local.as_ref()?;
                let current = o.base.and_then(|b| o.states.get(&b)).map(|s| s.digest)?;
                Some((*other, local.path.clone(), current))
            })
            .collect()
    }

    /// The first free `name (n).ext` for `rid` at `path`.
    fn free_for(&self, rid: &Id, path: &str) -> String {
        let root = self.engine.root().to_owned();
        let others = self.current_paths(rid);
        let taken = |candidate: &str| {
            others.iter().any(|(_, other, _)| collide(candidate, other))
                || root.join(candidate).exists()
        };
        free_name(path, &taken)
    }

    /// Moves `rid`'s file aside to the first free name, as an object that
    /// yields its path does; when this device wrote its current state, it
    /// signs the move in its next capture (§16).
    pub(super) fn move_aside(&mut self, rid: &Id) -> Result<(), SyncError> {
        let Some(local) = self.state.objects.get(rid).and_then(|o| o.local.clone()) else {
            return Ok(());
        };
        // An object that only waits for its move leaves the root (§9).
        if self.awaits_move(rid)
            && let Placement::Shared(path) = self.placement_of(rid, &local.path)
        {
            return self.relocate(rid, &path);
        }
        let free = self.free_for(rid, &local.path);
        self.relocate(rid, &free)
    }

    /// Moves `rid`'s file to `path`, or to the first free name beside it
    /// when something holds `path`. The object's synced path stays.
    pub(super) fn relocate(&mut self, rid: &Id, path: &str) -> Result<(), SyncError> {
        let Some(local) = self.state.objects.get(rid).and_then(|o| o.local.clone()) else {
            return Ok(());
        };
        let taken = self
            .current_paths(rid)
            .iter()
            .any(|(_, other, _)| collide(path, other))
            || self.engine.root().join(path).exists();
        let free = if taken {
            self.free_for(rid, path)
        } else {
            path.to_owned()
        };
        self.engine
            .move_synced(&local.path, &free, &local.revision.to_hex())
            .map_err(core)?;
        let free = actual_path(self.engine.root(), &free);
        let (size, modified_ns, file_id) = stat(self.engine.root(), &free)
            .ok_or_else(|| SyncError::Invalid("the moved file went away".into()))?;
        self.state.objects.get_mut(rid).expect("present").local = Some(Local {
            path: free,
            revision: local.revision,
            size,
            modified_ns,
            file_id,
        });
        Ok(())
    }

    /// Objects whose live state this device yields to another object's
    /// path and wrote itself: it moves each to the name it shows, with an
    /// ordinary operation, in its next capture (§16).
    pub(super) fn yielded_moves(&self) -> Vec<Id> {
        let own = self.credentials.keys.device();
        self.state
            .objects
            .iter()
            .filter_map(|(rid, object)| {
                let base = object.base?;
                let local = object.local.as_ref()?;
                let content = object.states.get(&base)?.content.as_ref()?;
                let shown = match self.placement_with_exception(rid, &content.path) {
                    Placement::At(path) | Placement::Shared(path) => path,
                    Placement::Outside(_) => return None,
                };
                (base.device == own && !content.delete && fold(&shown) != fold(&local.path))
                    .then_some(*rid)
            })
            .collect()
    }

    /// Objects with a readable live state that this device hasn't written
    /// yet, such as one whose path another file held: it tries them again.
    pub(super) fn unsettled(&self) -> Vec<Id> {
        self.state
            .objects
            .iter()
            .filter_map(|(rid, object)| {
                let live = self.live_states(rid);
                let [single] = live.as_slice() else {
                    return None;
                };
                let content = object.states.get(single)?.content.as_ref()?;
                let behind = object.base != Some(*single)
                    && object.local.is_none()
                    && !content.delete
                    && !object
                        .base
                        .and_then(|b| object.states.get(&b))
                        .and_then(|s| s.content.as_ref())
                        .is_some_and(|c| same_state(c, content));
                behind.then_some(*rid)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_collide_after_folding_or_as_a_folder_prefix() {
        assert!(collide("Notes/Plan.md", "notes/plan.md"));
        assert!(collide("Notes/Été.md", "Notes/E\u{301}te\u{301}.md"));
        assert!(collide("Notes/ÉTÉ.md", "Notes/été.md"));
        assert!(collide("Projects", "Projects/Plan.md"));
        assert!(!collide("Projects", "Projects2/Plan.md"));
        assert!(!collide("Plan.md", "Plan 2.md"));
    }

    #[test]
    fn the_first_free_name_counts_up_from_two() {
        let taken = |p: &str| ["Notes/Plan (2).md"].contains(&p);
        assert_eq!(free_name("Notes/Plan.md", &taken), "Notes/Plan (3).md");
        assert_eq!(free_name("README", &|_| false), "README (2)");
        assert_eq!(free_name(".hidden", &|_| false), ".hidden (2)");
    }
}
