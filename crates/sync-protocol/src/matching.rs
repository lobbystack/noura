//! Matching a folder's files to objects when it syncs without this
//! device's journal (§9, §15, §18): after a lost or rolled-back journal,
//! Reset devices, Set up sync again, connecting a copied folder, or a
//! domain that bootstraps over files the journal doesn't map.
//!
//! Every client must reach the same verdict on the same folder, so the
//! rules live here as pure functions: which object each local file
//! matches, whether it is adopted or held for review, which local
//! `.noura-domain` files hold the files beneath them, when such a hold
//! ends, which objects at a marker path a client refuses, and what an old
//! journal's review store keeps when the folder connects again.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::types::{Digest, Id};

/// The name of a restricted domain's marker file (§9).
pub const MARKER: &str = ".noura-domain";

/// A path as collisions and path matches compare it (§16): NFC, then each
/// character's simple case folding where it folds to one character.
pub fn fold(path: &str) -> String {
    path.nfc()
        .map(|c| {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(single), None) => single,
                _ => c,
            }
        })
        .collect()
}

/// One state an object's live set derives (§16): for a text-mode note the
/// merged body, not a state operation's own `revision` field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveState {
    /// The decrypted path.
    pub path: String,
    /// The revision of the derived content.
    pub revision: Digest,
    /// Whether the state is a delete.
    pub delete: bool,
}

/// An object with the states its live set derives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveObject {
    /// The object.
    pub rid: Id,
    /// The frontmatter ID of a managed file, empty for an ordinary file.
    pub object_id: String,
    /// Its live states.
    pub states: Vec<LiveState>,
}

/// A file on disk that the journal doesn't map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFile {
    /// Its path in the workspace.
    pub path: String,
    /// The revision of its bytes.
    pub revision: Digest,
    /// Its frontmatter ID when it is a managed file, else empty.
    pub object_id: String,
}

/// What a local file becomes (§15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", content = "rid", rename_all = "snake_case")]
pub enum Matched {
    /// It equals a live state of the object in path and revision, so the
    /// client takes it as that object's file.
    Adopt(Id),
    /// It matched the object but differs from every live state, in path or
    /// in content, so it waits for review and nothing uploads for the
    /// object until you choose.
    Held(Id),
    /// It matched no object and uploads as a new one.
    New,
}

/// Matches each of `locals` to `objects` (§15): managed files by frontmatter
/// ID first, ordinary files by content revision, then by path after
/// folding. Each pass takes the objects and files the earlier passes left,
/// in the order given. Returns one verdict per local file, in order.
pub fn match_files(objects: &[LiveObject], locals: &[LocalFile]) -> Vec<Matched> {
    let mut verdicts = vec![None; locals.len()];
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    for pass in 0..3 {
        for (o, object) in objects.iter().enumerate() {
            if taken.contains(&o) {
                continue;
            }
            let hit = locals.iter().enumerate().find(|(l, local)| {
                verdicts[*l].is_none()
                    && match pass {
                        0 => !object.object_id.is_empty() && local.object_id == object.object_id,
                        1 => {
                            local.object_id.is_empty()
                                && object.object_id.is_empty()
                                && object
                                    .states
                                    .iter()
                                    .any(|s| !s.delete && s.revision == local.revision)
                        }
                        _ => object
                            .states
                            .iter()
                            .any(|s| !s.delete && fold(&s.path) == fold(&local.path)),
                    }
            });
            if let Some((l, local)) = hit {
                taken.insert(o);
                let same = object
                    .states
                    .iter()
                    .any(|s| !s.delete && s.path == local.path && s.revision == local.revision);
                verdicts[l] = Some(if same {
                    Matched::Adopt(object.rid)
                } else {
                    Matched::Held(object.rid)
                });
            }
        }
    }
    verdicts
        .into_iter()
        .map(|v| v.unwrap_or(Matched::New))
        .collect()
}

/// Whether `path` lies at or beneath the folder `root` (the empty folder is
/// the workspace's root).
pub fn beneath(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.starts_with(&format!("{root}/"))
}

/// A local `.noura-domain` file found when a folder syncs without this
/// device's journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMarker {
    /// The folder that holds it.
    pub folder: String,
    /// The domain its content names, if it parses as a marker.
    pub domain: Option<Id>,
}

/// A held root (§9): every file at or beneath `folder` waits until you
/// confirm a domain or the innermost valid root there belongs to `domain`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldRoot {
    /// The folder.
    pub folder: String,
    /// The domain whose files it holds, if the file named one.
    pub domain: Option<Id>,
}

/// A recorded domain root: a valid marker the client applied (§9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainRoot {
    /// The folder that holds the marker.
    pub folder: String,
    /// Its domain.
    pub domain: Id,
}

/// A restricted domain's marker as a device knows it (§9): the domain's
/// `create_domain` and the live set of its `marker_rid` in the domain's own
/// log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerView {
    /// The domain.
    pub domain: Id,
    /// The `marker_rid` its `create_domain` names.
    pub rid: Id,
    /// The policy revision of that `create_domain`.
    pub created_at: u64,
    /// Whether the policy still lists the domain.
    pub listed: bool,
    /// The marker's live states.
    pub live: Vec<LiveState>,
}

/// A domain root a device records: the folder of a valid marker (§9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedRoot {
    /// The folder that holds the marker.
    pub folder: String,
    /// Its domain.
    pub domain: Id,
    /// The marker's `rid`.
    pub rid: Id,
}

/// Where an object's file sits on a device (§9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// At its synced path.
    At(String),
    /// Under `<root> (shared)`, since its path lies beneath the root of
    /// another domain.
    Shared(String),
    /// Nowhere new: the path left the object's own root (given here), so
    /// the file stays at its last path inside and the change goes to
    /// review.
    Outside(String),
}

/// The folder a marker's path names, when the path ends in
/// `/.noura-domain`.
pub fn marker_folder(path: &str) -> Option<&str> {
    path.strip_suffix(MARKER)?
        .strip_suffix('/')
        .filter(|folder| !folder.is_empty())
}

/// The roots that `markers` give a device (§9), sorted by folder. A marker
/// is valid when its live set holds exactly one operation, a state whose
/// path ends in `/.noura-domain`, and the policy lists the domain. A valid
/// marker at or beneath the root of a domain created earlier, or at its
/// path, gives way, so someone outside a domain can't redirect its files.
pub fn valid_roots(markers: &[MarkerView]) -> Vec<RecordedRoot> {
    let mut by_age: Vec<&MarkerView> = markers.iter().collect();
    by_age.sort_by_key(|m| m.created_at);
    let mut roots: Vec<RecordedRoot> = Vec::new();
    for marker in by_age {
        let [state] = marker.live.as_slice() else {
            continue;
        };
        if !marker.listed || state.delete {
            continue;
        }
        let Some(folder) = marker_folder(&state.path) else {
            continue;
        };
        if roots.iter().any(|root| beneath(folder, &root.folder)) {
            continue;
        }
        roots.push(RecordedRoot {
            folder: folder.to_owned(),
            domain: marker.domain,
            rid: marker.rid,
        });
    }
    roots.sort_by(|a, b| a.folder.cmp(&b.folder));
    roots
}

fn innermost<'a>(path: &str, roots: &'a [RecordedRoot]) -> Option<&'a RecordedRoot> {
    roots
        .iter()
        .filter(|root| beneath(path, &root.folder))
        .max_by_key(|root| root.folder.len())
}

/// The domain a new file at `path` goes to (§9): that of the innermost
/// recorded root that holds it, or `None` for the workspace domain.
pub fn domain_of(path: &str, roots: &[RecordedRoot]) -> Option<Id> {
    innermost(path, roots).map(|root| root.domain)
}

/// Whether a device may create a marker in `folder` (§9): the workspace's
/// root takes none, and neither does a folder at or beneath a recorded
/// root.
pub fn marker_allowed(folder: &str, roots: &[RecordedRoot]) -> bool {
    !folder.is_empty() && !roots.iter().any(|root| beneath(folder, &root.folder))
}

/// Where the file of an object of `domain` (`None` for the workspace
/// domain) with synced path `path` sits on a device that records `roots`
/// (§9).
pub fn placement(path: &str, domain: Option<Id>, roots: &[RecordedRoot]) -> Placement {
    if let Some(own) = roots.iter().find(|r| Some(r.domain) == domain)
        && !beneath(path, &own.folder)
    {
        return Placement::Outside(own.folder.clone());
    }
    match innermost(path, roots) {
        Some(root) if Some(root.domain) != domain => {
            let rest = path.strip_prefix(&root.folder).unwrap_or_default();
            Placement::Shared(format!("{} (shared){rest}", root.folder))
        }
        _ => Placement::At(path.to_owned()),
    }
}

/// The synced path of a file at `local` (§9): a file under `<root> (shared)`
/// keeps the path beneath the root it was relocated from.
pub fn synced_path(local: &str, roots: &[RecordedRoot]) -> String {
    roots
        .iter()
        .find_map(|root| {
            let shared = format!("{} (shared)", root.folder);
            beneath(local, &shared).then(|| {
                let rest = local.strip_prefix(&shared).unwrap_or_default();
                format!("{}{rest}", root.folder)
            })
        })
        .unwrap_or_else(|| local.to_owned())
}

/// The held roots a bootstrap or journal rebuild records for `domain` when
/// its marker is moved, deleted or in conflict (§9): one at each folder
/// that directly holds a live object of the domain.
pub fn bootstrap_held_roots(domain: Id, paths: &[String]) -> Vec<HeldRoot> {
    let mut folders: Vec<&str> = paths
        .iter()
        .map(|path| path.rsplit_once('/').map_or("", |(folder, _)| folder))
        .collect();
    folders.sort_unstable();
    folders.dedup();
    folders
        .into_iter()
        .map(|folder| HeldRoot {
            folder: folder.to_owned(),
            domain: Some(domain),
        })
        .collect()
}

/// Whether a folder whose entries bear `names` holds its marker: an iCloud
/// placeholder counts as present (§9).
pub fn marker_present<'a>(names: impl IntoIterator<Item = &'a str>) -> bool {
    names.into_iter().any(|name| {
        matches!(
            name,
            MARKER | ".noura-domain.icloud" | "..noura-domain.icloud"
        )
    })
}

/// The held roots a folder that syncs without its journal records (§9):
/// one at the folder of each local `.noura-domain` file other than a valid
/// marker it applied, for the domain that file names.
pub fn held_roots(locals: &[LocalMarker], applied: &[DomainRoot]) -> Vec<HeldRoot> {
    let mut held: Vec<HeldRoot> = locals
        .iter()
        .filter(|m| {
            !applied
                .iter()
                .any(|root| root.folder == m.folder && Some(root.domain) == m.domain)
        })
        .map(|m| HeldRoot {
            folder: m.folder.clone(),
            domain: m.domain,
        })
        .collect();
    held.sort_by(|a, b| a.folder.cmp(&b.folder));
    held.dedup();
    held
}

/// Whether a hold ends without your confirmation (§9): once the innermost
/// valid root that covers its folder belongs to the held domain. A valid
/// marker of any other domain never ends it.
pub fn hold_ends(held: &HeldRoot, roots: &[DomainRoot]) -> bool {
    let innermost = roots
        .iter()
        .filter(|root| beneath(&held.folder, &root.folder))
        .max_by_key(|root| root.folder.len());
    innermost.is_some_and(|root| held.domain == Some(root.domain))
}

/// Whether a client refuses, and lists for review instead of writing, the
/// object `rid` at `path` (§9): its last path component is `.noura-domain`
/// and no domain the device reads names it as its `marker_rid`.
pub fn refuses_marker_path(rid: &Id, path: &str, marker_rids: &[Id]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == MARKER && !marker_rids.contains(rid)
}

/// One branch an old journal's review store kept for an object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptBranch {
    /// The branch's operation: its device and counter.
    pub device: Id,
    /// Its chain counter.
    pub counter: u64,
    /// Whether the branch is a delete, which keeps no copy.
    pub delete: bool,
    /// Whether the review store holds a copy of it.
    pub kept: bool,
}

/// An object as an old journal recorded it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalObject {
    /// The object.
    pub rid: Id,
    /// The branch the file on disk matched, if any.
    pub base: Option<(Id, u64)>,
    /// The branches the review store kept.
    pub branches: Vec<KeptBranch>,
}

/// What connecting a folder moves to `.noura/sync/archive/<old-replica>/`
/// and keeps listing for review (§15): every kept copy of a branch other
/// than the one the file on disk matched, whether or not its author device
/// is still there. Returns `(rid, device, counter)` in order.
pub fn archived(objects: &[JournalObject]) -> Vec<(Id, Id, u64)> {
    let mut out = Vec::new();
    for object in objects {
        for branch in &object.branches {
            if branch.kept && !branch.delete && object.base != Some((branch.device, branch.counter))
            {
                out.push((object.rid, branch.device, branch.counter));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(n: u8) -> Digest {
        Digest([n; 32])
    }

    fn state(path: &str, revision: u8) -> LiveState {
        LiveState {
            path: path.into(),
            revision: d(revision),
            delete: false,
        }
    }

    fn local(path: &str, revision: u8, object_id: &str) -> LocalFile {
        LocalFile {
            path: path.into(),
            revision: d(revision),
            object_id: object_id.into(),
        }
    }

    #[test]
    fn a_managed_file_matches_by_id_before_a_path_match() {
        let note = LiveObject {
            rid: Id([1; 16]),
            object_id: "note_1".into(),
            states: vec![state("Plan.md", 1)],
        };
        let other = LiveObject {
            rid: Id([2; 16]),
            object_id: String::new(),
            states: vec![state("Moved.md", 9)],
        };
        let verdicts = match_files(
            &[note, other],
            &[local("Moved.md", 1, "note_1"), local("Plan.md", 1, "")],
        );
        assert_eq!(verdicts, vec![Matched::Held(Id([1; 16])), Matched::New]);
    }

    #[test]
    fn folding_matches_case_and_normalization() {
        assert_eq!(fold("Caf\u{0065}\u{0301}.md"), fold("CAFÉ.md"));
    }

    #[test]
    fn a_hold_ends_only_at_the_held_domains_innermost_root() {
        let held = HeldRoot {
            folder: "Team/Pay".into(),
            domain: Some(Id([5; 16])),
        };
        let other = DomainRoot {
            folder: "Team".into(),
            domain: Id([6; 16]),
        };
        assert!(!hold_ends(&held, std::slice::from_ref(&other)));
        let own = DomainRoot {
            folder: "Team/Pay".into(),
            domain: Id([5; 16]),
        };
        assert!(hold_ends(&held, &[other, own]));
    }
}
