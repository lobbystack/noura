//! Finding local changes (docs/workspace-format/sync-v2.md §9, §15): one
//! walk of the files the workspace scanner reports, compared with the files
//! the journal maps to objects. A file sync can't carry is reported. Within
//! one walk, a deleted file and a created one count as a move: a managed
//! file by its frontmatter ID, an ordinary file by its content revision and,
//! where the platform reports one, its file ID. Two files that share one
//! frontmatter ID don't sync as that object until you resolve the
//! `identity_conflict`.

use std::collections::{BTreeMap, BTreeSet};

use local_core::{ParsedMarkdown, SkipReason, SyncedFile, SyncedListing};
use sync_protocol::records::{MAX_PAYLOAD_LEN, SyncPath};
use sync_protocol::types::{Digest, Id};

use super::state::{Local, Object};

/// Why sync leaves a file out, as Settings lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipWhy {
    /// A symbolic link.
    Symlink,
    /// A name with no UTF-8 form, in this folder.
    UnsupportedName,
    /// A path that breaks the path rules of §13, such as a reserved
    /// Windows name or a trailing space.
    PathRules,
    /// The file can't be read.
    Unreadable,
    /// The file lives only in the cloud for now.
    NotDownloaded,
    /// Another file shares its frontmatter ID (`identity_conflict`).
    IdentityConflict,
    /// The file is above the 2 GiB limit of a blob (§21).
    TooLarge,
    /// The file sits beneath a local `.noura-domain` file found without a
    /// journal, and waits until you confirm where it goes (§9).
    HeldRoot,
    /// The file crossed into or out of a restricted folder, which is a
    /// move between domains (§9).
    CrossesRoot,
    /// This device can't write there, so the change stays in the file
    /// until it can (§14).
    CantWrite,
}

/// A file sync leaves out.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Skipped {
    /// Its path, or the folder that holds it; empty at the top.
    pub path: String,
    /// Why.
    pub why: SkipWhy,
}

/// A file the walk found, with what sync knows of its bytes.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub file: SyncedFile,
    pub revision: Digest,
    pub object_id: String,
    /// The bytes, when the walk read them.
    pub bytes: Option<Vec<u8>>,
}

impl Found {
    pub(crate) fn local(&self) -> Local {
        Local {
            path: self.file.relative_path.clone(),
            revision: self.revision,
            size: self.file.size,
            modified_ns: self.file.modified_ns,
            file_id: self.file.file_id,
        }
    }
}

/// One local change.
#[derive(Clone, Debug)]
pub(crate) enum Change {
    /// A new object.
    Create(Found),
    /// New bytes, a new path, or both, of a known object.
    Update { rid: Id, found: Found },
    /// The object's file is gone.
    Delete { rid: Id },
    /// The file didn't change; the walk only saw new times or a new ID.
    Touch { rid: Id, found: Found },
}

/// The frontmatter ID of a Markdown file's bytes, or empty.
pub(crate) fn object_id_of(path: &str, bytes: &[u8]) -> String {
    if !path.ends_with(".md") {
        return String::new();
    }
    match local_core::parse_markdown(path, bytes) {
        ParsedMarkdown::Managed(object) => object.id,
        _ => String::new(),
    }
}

/// What a walk found, against the journal's `objects`. `read` returns a
/// file's revision, and its bytes when it read them, which it does for
/// Markdown files and files that fit inline; `markers` names the domain
/// markers, which the scanner leaves out and [`super::domains`] looks after;
/// `held` names objects that take no change for now, such as one under
/// review.
pub(crate) fn changes(
    listing: &SyncedListing,
    objects: &BTreeMap<Id, Object>,
    markers: &BTreeSet<Id>,
    held: &BTreeSet<Id>,
    waiting: &BTreeMap<String, Digest>,
    held_root: impl Fn(&str) -> bool,
    mut read: impl FnMut(&SyncedFile) -> Option<(Digest, Option<Vec<u8>>)>,
) -> (Vec<Change>, Vec<Skipped>) {
    let mut skipped: Vec<Skipped> = listing
        .skipped
        .iter()
        .map(|s| Skipped {
            path: s.relative_path.clone().unwrap_or_default(),
            why: match s.reason {
                SkipReason::Symlink => SkipWhy::Symlink,
                SkipReason::UnsupportedName => SkipWhy::UnsupportedName,
                SkipReason::Unreadable => SkipWhy::Unreadable,
                SkipReason::NotDownloaded => SkipWhy::NotDownloaded,
            },
        })
        .collect();
    // Paths whose file may still exist, so their objects aren't deleted.
    let mut present: BTreeSet<String> = skipped.iter().map(|s| s.path.clone()).collect();
    let by_path: BTreeMap<&str, Id> = objects
        .iter()
        .filter(|(rid, _)| !markers.contains(rid))
        .filter_map(|(rid, o)| o.local.as_ref().map(|l| (l.path.as_str(), *rid)))
        .collect();

    let mut found: Vec<(Option<Id>, Found)> = Vec::new();
    for file in &listing.files {
        let path = file.relative_path.as_str();
        if SyncPath::new(path).is_err() {
            skipped.push(Skipped {
                path: path.to_owned(),
                why: SkipWhy::PathRules,
            });
            present.insert(path.to_owned());
            continue;
        }
        if held_root(path) {
            skipped.push(Skipped {
                path: path.to_owned(),
                why: SkipWhy::HeldRoot,
            });
            present.insert(path.to_owned());
            continue;
        }
        let known = by_path.get(path).copied();
        if known.is_none() && waiting.contains_key(path) {
            let same = read(file).is_some_and(|(revision, _)| waiting.get(path) == Some(&revision));
            if same {
                present.insert(path.to_owned());
                continue;
            }
        }
        let unchanged = known
            .and_then(|rid| objects[&rid].local.as_ref().map(|l| (rid, l)))
            .filter(|(_, l)| {
                l.size == file.size
                    && l.modified_ns == file.modified_ns
                    && l.file_id == file.file_id
            });
        if let Some((rid, local)) = unchanged {
            found.push((
                Some(rid),
                Found {
                    file: file.clone(),
                    revision: local.revision,
                    object_id: objects[&rid].object_id.clone(),
                    bytes: None,
                },
            ));
            continue;
        }
        let Some((revision, bytes)) = read(file) else {
            skipped.push(Skipped {
                path: path.to_owned(),
                why: SkipWhy::Unreadable,
            });
            present.insert(path.to_owned());
            continue;
        };
        found.push((
            known,
            Found {
                file: file.clone(),
                revision,
                object_id: bytes
                    .as_deref()
                    .map(|bytes| object_id_of(path, bytes))
                    .unwrap_or_default(),
                bytes,
            },
        ));
    }

    // Two files that share one frontmatter ID wait for you (v1.md).
    let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, f) in &found {
        if !f.object_id.is_empty() {
            *by_id.entry(f.object_id.as_str()).or_default() += 1;
        }
    }
    let conflicted: BTreeSet<String> = by_id
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(id, _)| (*id).to_owned())
        .collect();
    let mut kept = Vec::new();
    for (rid, f) in found {
        if conflicted.contains(&f.object_id) {
            present.insert(f.file.relative_path.clone());
            skipped.push(Skipped {
                path: f.file.relative_path.clone(),
                why: SkipWhy::IdentityConflict,
            });
        } else {
            kept.push((rid, f));
        }
    }

    let seen: BTreeSet<&str> = kept
        .iter()
        .map(|(_, f)| f.file.relative_path.as_str())
        .collect();
    let mut gone: Vec<Id> = if listing.complete {
        objects
            .iter()
            .filter(|(rid, o)| {
                o.local
                    .as_ref()
                    .is_some_and(|l| !seen.contains(l.path.as_str()) && !present.contains(&l.path))
                    && !held.contains(rid)
                    && !markers.contains(rid)
            })
            .map(|(rid, _)| *rid)
            .collect()
    } else {
        Vec::new()
    };

    let mut out = Vec::new();
    let mut created = Vec::new();
    for (rid, f) in kept {
        match rid {
            Some(rid) if held.contains(&rid) => {}
            Some(rid) => {
                let object = &objects[&rid];
                let local = object.local.as_ref().expect("mapped by path");
                // A file whose frontmatter ID changed to another object's is
                // that object now, and the old one went away.
                let replaced = !object.object_id.is_empty()
                    && !f.object_id.is_empty()
                    && object.object_id != f.object_id;
                if replaced {
                    gone.push(rid);
                    created.push(f);
                } else if local.revision == f.revision {
                    if local.size != f.file.size
                        || local.modified_ns != f.file.modified_ns
                        || local.file_id != f.file.file_id
                    {
                        out.push(Change::Touch { rid, found: f });
                    }
                } else {
                    out.push(Change::Update { rid, found: f });
                }
            }
            None => created.push(f),
        }
    }

    // Moves: a managed file by its ID, an ordinary file by its revision and
    // file ID.
    for f in created {
        let matched = gone.iter().position(|rid| {
            let object = &objects[rid];
            let local = object.local.as_ref().expect("gone files had one");
            if !f.object_id.is_empty() || !object.object_id.is_empty() {
                object.object_id == f.object_id
            } else {
                local.revision == f.revision
                    && match (local.file_id, f.file.file_id) {
                        (Some(a), Some(b)) => a == b,
                        _ => true,
                    }
            }
        });
        match matched {
            Some(index) => {
                let rid = gone.remove(index);
                out.push(Change::Update { rid, found: f });
            }
            None => out.push(Change::Create(f)),
        }
    }
    out.extend(gone.into_iter().map(|rid| Change::Delete { rid }));
    skipped.sort();
    skipped.dedup();
    (out, skipped)
}

/// Whether a file's payload would pass the inline limit: its bytes plus the
/// payload's other fields stay at or below 262,144 bytes (§13).
pub(crate) fn fits_inline(path: &str, object_id: &str, size: u64) -> bool {
    // tag, rid, kind, object_id, path, revision, form, length, origin.
    let overhead = 21 + 16 + 1 + 2 + object_id.len() + 4 + path.len() + 32 + 1 + 4 + 1;
    size as usize + overhead <= MAX_PAYLOAD_LEN
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_core::SkippedEntry;

    fn file(path: &str, size: u64, id: u64) -> SyncedFile {
        SyncedFile {
            relative_path: path.into(),
            size,
            modified_ns: 1,
            file_id: Some(id),
        }
    }

    fn object(path: &str, bytes: &[u8], id: u64, object_id: &str) -> Object {
        Object {
            object_id: object_id.into(),
            local: Some(Local {
                path: path.into(),
                revision: sync_protocol::crypto::hash(bytes),
                size: bytes.len() as u64,
                modified_ns: 1,
                file_id: Some(id),
            }),
            ..Object::default()
        }
    }

    const NOTE: &[u8] = b"---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\n---\n# Plan\n";
    const EDITED: &[u8] = b"---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\n---\n# Plan 2\n";

    fn run(
        listing: SyncedListing,
        objects: &BTreeMap<Id, Object>,
        files: &[(&str, &[u8])],
    ) -> (Vec<Change>, Vec<Skipped>) {
        let files: BTreeMap<String, Vec<u8>> = files
            .iter()
            .map(|(p, b)| ((*p).to_owned(), b.to_vec()))
            .collect();
        changes(
            &listing,
            objects,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            |_| false,
            |f| {
                files
                    .get(&f.relative_path)
                    .map(|b| (sync_protocol::crypto::hash(b), Some(b.clone())))
            },
        )
    }

    #[test]
    fn an_ordinary_file_renamed_within_one_walk_is_a_move() {
        let rid = Id([1; 16]);
        let objects = BTreeMap::from([(rid, object("a.png", b"png", 5, ""))]);
        let listing = SyncedListing {
            files: vec![file("b.png", 3, 5)],
            skipped: vec![],
            complete: true,
        };
        let (changes, _) = run(listing, &objects, &[("b.png", b"png")]);
        assert!(matches!(
            changes.as_slice(),
            [Change::Update { rid: r, found }] if *r == rid && found.file.relative_path == "b.png"
        ));
    }

    #[test]
    fn same_bytes_under_another_file_id_is_a_delete_and_a_create() {
        let rid = Id([1; 16]);
        let objects = BTreeMap::from([(rid, object("a.png", b"png", 5, ""))]);
        let listing = SyncedListing {
            files: vec![file("b.png", 3, 6)],
            skipped: vec![],
            complete: true,
        };
        let (changes, _) = run(listing, &objects, &[("b.png", b"png")]);
        assert!(matches!(
            changes.as_slice(),
            [Change::Create(_), Change::Delete { .. }]
        ));
    }

    #[test]
    fn a_managed_file_moved_and_edited_keeps_its_object() {
        let rid = Id([1; 16]);
        let objects = BTreeMap::from([(
            rid,
            object("Old.md", NOTE, 5, "note_01j9z3y7q8r2s4t6v8w0x2y4z6"),
        )]);
        let listing = SyncedListing {
            files: vec![file("New/Plan.md", EDITED.len() as u64, 9)],
            skipped: vec![],
            complete: true,
        };
        let (changes, _) = run(listing, &objects, &[("New/Plan.md", EDITED)]);
        assert!(matches!(
            changes.as_slice(),
            [Change::Update { rid: r, found }] if *r == rid && found.bytes.as_deref() == Some(EDITED)
        ));
    }

    #[test]
    fn two_files_with_one_frontmatter_id_wait() {
        let rid = Id([1; 16]);
        let objects = BTreeMap::from([(
            rid,
            object("Plan.md", NOTE, 5, "note_01j9z3y7q8r2s4t6v8w0x2y4z6"),
        )]);
        let listing = SyncedListing {
            files: vec![
                file("Copy.md", NOTE.len() as u64, 6),
                file("Plan.md", NOTE.len() as u64, 7),
            ],
            skipped: vec![],
            complete: true,
        };
        let (changes, skipped) = run(listing, &objects, &[("Copy.md", NOTE), ("Plan.md", NOTE)]);
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(skipped.len(), 2);
        assert!(skipped.iter().all(|s| s.why == SkipWhy::IdentityConflict));
    }

    #[test]
    fn files_that_break_the_path_rules_and_skipped_entries_are_reported() {
        let rid = Id([1; 16]);
        let objects = BTreeMap::from([(rid, object("Cloud.md", b"x", 5, ""))]);
        let listing = SyncedListing {
            files: vec![file("Notes/CON", 1, 1), file("Plan.md ", 1, 2)],
            skipped: vec![SkippedEntry {
                relative_path: Some("Cloud.md".into()),
                reason: SkipReason::NotDownloaded,
            }],
            complete: true,
        };
        let (changes, skipped) = run(listing, &objects, &[]);
        assert!(changes.is_empty(), "a file in the cloud isn't deleted");
        let whys: Vec<(&str, SkipWhy)> = skipped.iter().map(|s| (s.path.as_str(), s.why)).collect();
        assert!(whys.contains(&("Notes/CON", SkipWhy::PathRules)));
        assert!(whys.contains(&("Plan.md ", SkipWhy::PathRules)));
        assert!(whys.contains(&("Cloud.md", SkipWhy::NotDownloaded)));
    }

    #[test]
    fn an_incomplete_walk_deletes_nothing() {
        let rid = Id([1; 16]);
        let objects = BTreeMap::from([(rid, object("a.png", b"png", 5, ""))]);
        let listing = SyncedListing {
            files: vec![],
            skipped: vec![],
            complete: false,
        };
        let (changes, _) = run(listing, &objects, &[]);
        assert!(changes.is_empty());
    }

    #[test]
    fn the_inline_limit_counts_the_payload_fields() {
        assert!(fits_inline("a.md", "", 200_000));
        assert!(!fits_inline("a.md", "", 262_144));
    }
}
