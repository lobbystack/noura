//! A folder that syncs without this device's journal
//! (docs/workspace-format/sync-v2.md §9, §15): after a lost or rolled-back
//! journal, or when you connect a copied folder, the client moves the old
//! journal's review store aside, pulls the domain again and matches local
//! files to objects, managed files by frontmatter ID first, ordinary files
//! by content revision, then by path. A matched file that differs from
//! every live state of its object, in path or in content, waits in the
//! review store as a held local version, and the client uploads nothing for
//! that object until you choose. Files beneath a local `.noura-domain` file
//! wait under a held root until you confirm where they go.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use local_core::{SyncedExpect, WorkspaceEngine};
use serde::{Deserialize, Serialize};
use sync_protocol::crypto::hash;
use sync_protocol::matching::{LiveObject, LiveState, LocalFile, Matched, match_files};
use sync_protocol::types::{Digest, Id};

use super::capture::{Found, object_id_of};
use super::settle::{Bodies, Body, free_name};
use super::state::{Held, OpKey, State};
use super::{SyncError, SyncService, WorkspaceSync, core, profile_of};
use crate::sync::journal::{self, Journal, STATE_FILE};
use crate::sync::registry::{Registry, SyncedFolder};

use sync_protocol::matching::HeldRoot;
pub use sync_protocol::matching::MARKER;
/// The listing of an archived review store.
const ARCHIVE_INDEX: &str = "archive.json";

/// What you choose for a held local version (§15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldChoice {
    /// Keep the live state, which replaces the file.
    KeepLive,
    /// Keep this device's version, which writes a state operation covering
    /// every live operation of the object.
    KeepMine,
    /// Keep both: this device's version becomes a new object at the first
    /// free `name (n).ext`, a managed file's copy with a new frontmatter ID.
    KeepBoth,
}

/// A version an earlier journal's review store kept, which stays listed
/// after you connect the folder again until you resolve it (§15).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archived {
    /// The path the version had.
    pub path: String,
    /// Its copy, relative to the journals' folder.
    pub file: String,
    /// The revision of the copy's bytes.
    pub revision: Digest,
}

/// The local `.noura-domain` files under `root`, by the folder that holds
/// each. The workspace scanner skips dot files, so this walk looks for them.
pub(crate) fn local_markers(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_owned(), String::new())];
    while let Some((dir, relative)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if kind.is_dir() && !(relative.is_empty() && (name == ".noura" || name == ".git")) {
                stack.push((entry.path(), path));
            } else if kind.is_file() && workspace_format::is_domain_marker(&name) {
                found.push(relative.clone());
            }
        }
    }
    found.sort();
    found
}

pub(crate) use sync_protocol::matching::beneath;

/// The server, account and remote workspace that `device`'s journal in
/// the folder at `root` names, as a copied folder's journal does.
pub fn journal_workspace(
    root: &Path,
    device: &Id,
    secrets: &dyn local_core::sync::SyncCredentials,
) -> Option<(String, Id, Id)> {
    let journal = Journal::open(root, device, secrets).ok()?;
    let state: State = journal.read(STATE_FILE).ok()??;
    Some((state.server, state.account, state.workspace))
}

/// The device other than `own` whose journal in the folder at `root` syncs
/// `workspace`, as a card a reset retired left it.
pub fn earlier_journal(
    root: &Path,
    own: &Id,
    workspace: Id,
    secrets: &dyn local_core::sync::SyncCredentials,
) -> Option<Id> {
    std::fs::read_dir(root.join(journal::JOURNALS))
        .ok()?
        .flatten()
        .filter_map(|entry| Id::from_hex(entry.file_name().to_str()?))
        .filter(|device| device != own)
        .find(|device| {
            journal_workspace(root, device, secrets).is_some_and(|(_, _, held)| held == workspace)
        })
}

impl WorkspaceSync {
    /// Syncs the folder `engine` opened again without this device's
    /// journal (§15): the folder the registry maps to a remote workspace,
    /// after a lost or rolled-back journal, or one whose journal holds
    /// another replica ID, which this connects. The old journal's review
    /// store moves to `.noura/sync/archive/<old-replica>/` and stays
    /// listed; the device pulls the domain again, continues its chain from
    /// the server's head above its reserved counters, and matches the local
    /// files before it writes any.
    pub async fn rebuild(
        service: &mut SyncService,
        engine: Arc<WorkspaceEngine>,
        registry: &mut Registry,
    ) -> Result<Self, SyncError> {
        let entry = registry
            .of_folder(engine.root())
            .cloned()
            .ok_or_else(|| SyncError::Invalid("this folder doesn't sync".into()))?;
        let workspace = entry.workspace;
        Self::match_folder(service, engine, registry, entry, workspace).await
    }

    /// Connects the folder a member synced before to `workspace`, which an
    /// owner set up after closing the folder's old workspace (§14). Its
    /// files match as after a lost journal: the device holds every file
    /// under a marker of the old workspace's restricted domains and every
    /// file that differs from the new workspace's live state. A folder of
    /// a workspace that's still open stays with it.
    pub async fn rejoin(
        service: &mut SyncService,
        engine: Arc<WorkspaceEngine>,
        workspace: Id,
        registry: &mut Registry,
    ) -> Result<Self, SyncError> {
        let entry = registry
            .of_folder(engine.root())
            .cloned()
            .ok_or_else(|| SyncError::Invalid("this folder doesn't sync".into()))?;
        match service.client().policy(entry.workspace, 0).await {
            Err(SyncError::Refused { code, .. }) if code == "not_found" => {}
            Err(error) => return Err(error),
            Ok(_) => {
                return Err(SyncError::Invalid(
                    "This folder syncs a workspace that's still open. Choose an empty folder."
                        .into(),
                ));
            }
        }
        // The new workspace verifies before the old journal moves aside.
        service.open_workspace(workspace).await?;
        let old = entry.workspace;
        let sync = Self::match_folder(service, engine, registry, entry, workspace).await?;
        service.kept_heads_mut().forget_workspace(old)?;
        Ok(sync)
    }

    /// Syncs the folder of `entry` with `workspace` without this device's
    /// journal, whose review store moves aside.
    async fn match_folder(
        service: &mut SyncService,
        engine: Arc<WorkspaceEngine>,
        registry: &mut Registry,
        entry: SyncedFolder,
        workspace: Id,
    ) -> Result<Self, SyncError> {
        let device = service.credentials().keys.device();
        let secrets = service.secrets();
        let journal = Journal::open(engine.root(), &device, &*secrets)?;
        let old: Option<State> = journal.read(STATE_FILE).ok().flatten();
        let old_replica = old.as_ref().map_or(entry.replica, |s| s.replica);
        let mut archived = Self::archive(&journal, old.as_ref(), &old_replica)?;
        drop(journal);
        let server = service.client().base().to_owned();
        let replica = registry.connect(&server, entry.account, workspace, engine.root())?;
        if entry.paused {
            registry.update(&server, &entry.account, &workspace, |e| e.paused = true)?;
        }
        let mut remote = service.open_workspace(workspace).await?;
        let (name, profile) = profile_of(service, &mut remote).await?;
        let generation = service.client().restore_generation();
        let mut sync = Self::start(
            service, engine, remote, server, replica, generation, &name, profile,
        )?;
        sync.paused = entry.paused;
        sync.hold_local_markers();
        if let Some(old) = old {
            sync.hold_old_roots(&old);
            archived.extend(old.archived);
        }
        sync.state.archived = archived;
        sync.compare_own_chain(service).await?;
        // The client bootstraps again and matches its files (§15).
        sync.state.bootstrap = true;
        sync.matching = true;
        let caught_up = async {
            for _ in 0..10_000 {
                let (_, waiting) = sync.pull(service, 0).await?;
                let behind = sync.state.log.cursor < sync.state.log.head
                    || sync
                        .state
                        .restricted
                        .values()
                        .any(|f| f.log.cursor < f.log.head);
                if waiting || !behind {
                    return Ok(());
                }
            }
            Err(SyncError::Network("the domain's log didn't end".into()))
        }
        .await;
        sync.matching = false;
        caught_up?;
        sync.match_local()?;
        sync.save()?;
        Ok(sync)
    }

    /// Holds the files beneath every local marker until a valid marker of
    /// the domain it names covers them (§9).
    pub(super) fn hold_local_markers(&mut self) {
        self.state.held_roots = local_markers(self.engine.root())
            .into_iter()
            .map(|folder| {
                let named = self
                    .engine
                    .read_synced(&format!("{folder}/{MARKER}"))
                    .ok()
                    .flatten()
                    .and_then(|bytes| sync_protocol::records::parse_marker(&bytes));
                HeldRoot {
                    folder,
                    domain: named,
                }
            })
            .collect();
    }

    /// Holds what the old journal held or recorded as a root too: a folder
    /// whose marker you deleted meanwhile still keeps its files out of the
    /// new workspace domain until you decide (§14).
    pub(super) fn hold_old_roots(&mut self, old: &State) {
        let recorded = old.roots.iter().map(|root| HeldRoot {
            folder: root.folder.clone(),
            domain: Some(root.domain),
        });
        for root in old.held_roots.iter().cloned().chain(recorded) {
            if !self
                .state
                .held_roots
                .iter()
                .any(|held| held.folder == root.folder)
            {
                self.state.held_roots.push(root);
            }
        }
    }

    /// Moves an old journal's review store aside and lists what it kept.
    pub(super) fn archive(
        journal: &Journal,
        old: Option<&State>,
        replica: &Id,
    ) -> Result<Vec<Archived>, SyncError> {
        let mut kept = Vec::new();
        if let Some(old) = old {
            for (rid, object) in &old.objects {
                for (key, info) in &object.states {
                    let Some(content) = &info.content else {
                        continue;
                    };
                    if let Some(file) = &content.kept
                        && !content.delete
                        && object.base != Some(*key)
                    {
                        kept.push(Archived {
                            path: content.path.clone(),
                            file: format!("{}/{}/{file}", journal::ARCHIVE, replica.to_hex()),
                            revision: content.revision,
                        });
                    }
                }
                let _ = rid;
            }
        }
        let folder = journal.archive(replica)?;
        if !kept.is_empty() {
            let bytes =
                serde_json::to_vec_pretty(&kept).map_err(|e| SyncError::Invalid(e.to_string()))?;
            journal::durable_write(&folder.join(ARCHIVE_INDEX), &bytes)?;
        }
        Ok(kept)
    }

    /// Matches local files to the objects the pull found (§15): managed
    /// files by frontmatter ID first, ordinary files by content revision,
    /// then by path. A match that equals a live state is adopted as it is;
    /// one that differs is held; objects nothing matched are written; the
    /// files nothing matched upload as new objects in the next capture.
    pub(super) fn match_local(&mut self) -> Result<(), SyncError> {
        let cache = std::mem::take(&mut self.cache);
        let listing = self.engine.synced_listing().map_err(core)?;
        let mut locals: Vec<Found> = Vec::new();
        for file in listing.files {
            if self
                .state
                .held_roots
                .iter()
                .any(|root| beneath(&file.relative_path, &root.folder))
            {
                continue;
            }
            let Some((revision, bytes)) =
                super::read_for_sync(&self.engine, &file.relative_path, file.size)
            else {
                continue;
            };
            locals.push(Found {
                revision,
                object_id: bytes
                    .as_deref()
                    .map(|bytes| object_id_of(&file.relative_path, bytes))
                    .unwrap_or_default(),
                file,
                bytes: None,
            });
        }
        // A rewind writes the objects that lag again from these files.
        let lagging: Vec<String> = self
            .state
            .restore
            .rewrites
            .values()
            .filter_map(|r| r.object.local.as_ref().map(|l| l.path.clone()))
            .collect();
        locals.retain(|l| !lagging.contains(&l.file.relative_path));
        let rids: Vec<Id> = self
            .state
            .objects
            .keys()
            .filter(|rid| !self.lagging.contains(rid))
            .copied()
            .collect();
        let objects: Vec<LiveObject> = rids
            .iter()
            .map(|rid| LiveObject {
                rid: *rid,
                object_id: self.state.objects[rid].object_id.clone(),
                states: self
                    .live_contents(rid)
                    .into_iter()
                    .map(|(_, c)| LiveState {
                        path: c.path,
                        revision: c.revision,
                        delete: c.delete,
                    })
                    .collect(),
            })
            .collect();
        let files: Vec<LocalFile> = locals
            .iter()
            .map(|l| LocalFile {
                path: l.file.relative_path.clone(),
                revision: l.revision,
                object_id: l.object_id.clone(),
            })
            .collect();
        let mut matched: BTreeMap<Id, Found> = BTreeMap::new();
        for (verdict, local) in match_files(&objects, &files).into_iter().zip(locals) {
            if let Matched::Adopt(rid) | Matched::Held(rid) = verdict {
                matched.insert(rid, local);
            }
        }
        for rid in rids {
            match matched.remove(&rid) {
                Some(found) => self.adopt_match(rid, found, &cache)?,
                None => self.settle(rid, &cache)?,
            }
        }
        Ok(())
    }

    /// The live states of `rid` that opened, with their positions.
    fn live_contents(&self, rid: &Id) -> Vec<(OpKey, super::state::Content)> {
        let object = &self.state.objects[rid];
        self.live_states(rid)
            .into_iter()
            .filter_map(|key| {
                let content = object.states.get(&key)?.content.clone()?;
                Some((key, content))
            })
            .collect()
    }

    /// Takes a matched local file as `rid`'s: adopted when it equals a live
    /// state in path and revision, else held for review.
    fn adopt_match(&mut self, rid: Id, found: Found, cache: &Bodies) -> Result<(), SyncError> {
        let live = self.live_contents(&rid);
        let local = found.local();
        let same = live
            .iter()
            .find(|(_, c)| !c.delete && c.path == local.path && c.revision == local.revision);
        let object = self.state.objects.get_mut(&rid).expect("listed");
        object.local = Some(local.clone());
        if let Some((key, _)) = same {
            object.base = Some(*key);
            return Ok(());
        }
        object.base = None;
        // The file stays the version you hold, and the review store keeps
        // a copy of it. A file that changed meanwhile keeps the hold.
        let copy = held_copy(&rid, &local.revision);
        let source = self.engine.root().join(&local.path);
        if let Err(error) = self.journal.keep_file(&copy, &source, &local.revision) {
            tracing::warn!(%error, "a held version stays in its file alone");
        }
        self.state.held.insert(
            rid,
            Held {
                path: local.path,
                revision: local.revision,
            },
        );
        for (key, _) in live {
            self.keep_branch(&rid, &key, cache)?;
        }
        Ok(())
    }

    /// Resolves a held local version (§15).
    pub fn resolve_held(&mut self, rid: Id, choice: HeldChoice) -> Result<(), SyncError> {
        let held = self
            .state
            .held
            .get(&rid)
            .cloned()
            .ok_or_else(|| SyncError::Invalid("no such held version".into()))?;
        let live = self.live_contents(&rid);
        // A delete that waits holds no file, so "both" keeps the live state.
        let deleted = held.revision == Digest::ZERO;
        match choice {
            HeldChoice::KeepMine if deleted => {
                let view: BTreeSet<OpKey> = self.live_states(&rid).into_iter().collect();
                let covers = self.covers_of(&rid, &view);
                self.state.held.remove(&rid);
                let domain = self.object_domain(&rid);
                self.in_domain(domain, |sync| sync.sign_delete(rid, covers))?;
            }
            HeldChoice::KeepBoth | HeldChoice::KeepLive if deleted => {
                self.state.held.remove(&rid);
                self.adopt_live(rid, &live)?;
            }
            HeldChoice::KeepMine => {
                let view: BTreeSet<OpKey> = self.live_states(&rid).into_iter().collect();
                let covers = self.covers_of(&rid, &view);
                let found = self.found_on_disk(&held.path)?;
                self.state.held.remove(&rid);
                self.sign_file(rid, &found, covers)?;
                self.journal.forget(&held_copy(&rid, &held.revision));
            }
            HeldChoice::KeepLive => {
                self.state.held.remove(&rid);
                self.adopt_live(rid, &live)?;
                self.journal.forget(&held_copy(&rid, &held.revision));
            }
            HeldChoice::KeepBoth => {
                let root = self.engine.root().to_owned();
                let taken = |candidate: &str| root.join(candidate).exists();
                let copy = free_name(&held.path, &taken);
                if copy.ends_with(".md") {
                    let bytes = self
                        .engine
                        .read_synced(&held.path)
                        .map_err(core)?
                        .ok_or_else(|| SyncError::Invalid("the file went away".into()))?;
                    let copied = new_identity(&copy, &bytes)?;
                    self.engine
                        .write_synced(&copy, &copied, &SyncedExpect::Absent)
                        .map_err(core)?;
                } else {
                    // Any other file keeps its bytes, which stream across.
                    let revision = self
                        .engine
                        .hash_synced(&held.path)
                        .map_err(core)?
                        .ok_or_else(|| SyncError::Invalid("the file went away".into()))?;
                    self.engine
                        .write_synced_from(
                            &copy,
                            &root.join(&held.path),
                            &revision,
                            &SyncedExpect::Absent,
                        )
                        .map_err(core)?;
                }
                self.state.held.remove(&rid);
                self.adopt_live(rid, &live)?;
                self.journal.forget(&held_copy(&rid, &held.revision));
            }
        }
        self.save()
    }

    /// Writes the live state over a held file, which the device read as
    /// `rid`'s.
    fn adopt_live(
        &mut self,
        rid: Id,
        live: &[(OpKey, super::state::Content)],
    ) -> Result<(), SyncError> {
        let Some((key, content)) = live.first().cloned() else {
            return Ok(());
        };
        if content.delete {
            if let Some(local) = self.state.objects[&rid].local.clone() {
                self.engine
                    .delete_synced(&local.path, &local.revision.to_hex())
                    .map_err(core)?;
            }
            let object = self.state.objects.get_mut(&rid).expect("listed");
            object.local = None;
            object.base = Some(key);
            return Ok(());
        }
        let body = self.content_body(&rid, &key, &BTreeMap::new())?;
        self.place(&rid, &content.path, &body, content.revision)?;
        self.state.objects.get_mut(&rid).expect("listed").base = Some(key);
        Ok(())
    }

    /// Keeps a live branch's bytes in the review store, for a held object.
    fn keep_branch(&mut self, rid: &Id, key: &OpKey, cache: &Bodies) -> Result<(), SyncError> {
        let object = &self.state.objects[rid];
        let Some(content) = object.states.get(key).and_then(|s| s.content.clone()) else {
            return Ok(());
        };
        if content.delete || content.kept.is_some() {
            return Ok(());
        }
        let Ok(body) = self.content_body(rid, key, cache) else {
            return Ok(());
        };
        let relative = format!(
            "{}/{}/{}",
            journal::HELD,
            rid.to_hex(),
            object.states[key].digest.to_hex()
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

    /// Ends the hold on the files beneath `folder`, which upload to the
    /// workspace domain from the next capture on (§9).
    pub fn confirm_held_root(&mut self, folder: &str) -> Result<(), SyncError> {
        self.state.held_roots.retain(|root| root.folder != folder);
        self.save()
    }

    /// The held roots, each with the files beneath it and how many people
    /// would see each in the workspace domain (§9).
    pub fn held_roots(&self) -> Vec<(String, Vec<String>, usize)> {
        let people = self
            .remote
            .as_ref()
            .map_or(0, |r| r.policy.head_snapshot().members.len());
        let listing = self
            .engine
            .synced_listing()
            .map(|l| l.files)
            .unwrap_or_default();
        self.state
            .held_roots
            .iter()
            .map(|root| {
                let files = listing
                    .iter()
                    .filter(|f| beneath(&f.relative_path, &root.folder))
                    .map(|f| f.relative_path.clone())
                    .collect();
                (root.folder.clone(), files, people)
            })
            .collect()
    }

    /// The versions an earlier journal's review store kept.
    pub fn archived(&self) -> &[Archived] {
        &self.state.archived
    }

    /// Restores an archived version as a new file at the first free name,
    /// or discards it, and stops listing it.
    pub fn resolve_archived(&mut self, index: usize, restore: bool) -> Result<(), SyncError> {
        if index >= self.state.archived.len() {
            return Err(SyncError::Invalid("no such version".into()));
        }
        let item = self.state.archived.remove(index);
        let journals = self.engine.root().join(journal::JOURNALS);
        if restore {
            let bytes = std::fs::read(journals.join(&item.file))
                .map_err(|e| crate::sync::local_failure("Noura can't read the kept version.", e))?;
            if hash(&bytes) != item.revision {
                return Err(SyncError::Invalid("the kept version changed".into()));
            }
            let root = self.engine.root().to_owned();
            let taken = |candidate: &str| root.join(candidate).exists();
            let path = if item.path.is_empty() {
                "Restored"
            } else {
                &item.path
            };
            let copy = free_name(path, &taken);
            let copied = new_identity(&copy, &bytes)?;
            self.engine
                .write_synced(&copy, &copied, &SyncedExpect::Absent)
                .map_err(core)?;
        }
        let _ = std::fs::remove_file(journals.join(&item.file));
        self.save()
    }

    /// The held local versions, by object, with their paths.
    pub fn held(&self) -> Vec<(Id, String)> {
        self.state
            .held
            .iter()
            .map(|(rid, held)| (*rid, held.path.clone()))
            .collect()
    }
}

/// Where the review store keeps a copy of `rid`'s held version.
fn held_copy(rid: &Id, revision: &Digest) -> String {
    format!("{}/{}/{}", journal::HELD, rid.to_hex(), revision.to_hex())
}

/// `bytes` with a new frontmatter ID when they hold a managed file, as a
/// copy that becomes its own object.
fn new_identity(path: &str, bytes: &[u8]) -> Result<Vec<u8>, SyncError> {
    match local_core::parse_markdown(path, bytes) {
        local_core::ParsedMarkdown::Managed(mut object) => {
            object.id = local_core::new_object_id(&object.object_type);
            local_core::serialize_object_over(bytes, &object).map_err(core)
        }
        _ => Ok(bytes.to_vec()),
    }
}
