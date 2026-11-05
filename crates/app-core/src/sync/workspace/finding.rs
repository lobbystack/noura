//! How a device finds local changes (docs/workspace-format/sync-v2.md §28).
//! A walk stats every file, which a large workspace can't afford each
//! round. A device whose engine reports changes follows those reports and
//! lists its files from the journal, with fresh details for the paths the
//! reports named. Whatever the reports can't vouch for takes a walk: a path
//! the journal doesn't know, a folder, lost events, a watcher that stopped,
//! and every [`WALK_EVERY`], in case the platform missed a change.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use local_core::{CoreEvent, SyncedEntry, SyncedFile, SyncedListing};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

use super::{SkipWhy, SyncError, WorkspaceSync, core};

/// How often a following device walks the workspace anyway, the app's own
/// full rescan interval.
const WALK_EVERY: Duration = Duration::from_secs(60);

/// A walk's result and the reports since, for the next capture.
#[derive(Default)]
pub(super) struct Finding {
    /// The engine's reports, once this device follows them.
    events: Option<Receiver<CoreEvent>>,
    /// When the last walk ended, while it still vouches for the files.
    walked: Option<Instant>,
    /// The paths reported since the last capture.
    named: BTreeSet<String>,
    /// How often a following device walks, when a test sets it.
    every: Option<Duration>,
    /// What the last walk left out, which a listing from the journal keeps.
    left_out: Vec<local_core::SkippedEntry>,
    /// Walks so far.
    walks: u64,
}

impl Finding {
    /// Takes the reports waiting. False when a report can't be followed,
    /// so only a walk tells what changed.
    fn drain(&mut self) -> bool {
        let Some(events) = self.events.as_mut() else {
            return false;
        };
        let mut complete = true;
        loop {
            match events.try_recv() {
                Ok(event) => match paths_of(&event) {
                    Some(paths) => self.named.extend(paths),
                    None => complete &= !changes_files(&event),
                },
                Err(TryRecvError::Empty) => return complete,
                Err(TryRecvError::Lagged(_) | TryRecvError::Closed) => complete = false,
            }
        }
    }
}

/// The paths a report names, outside `.noura`, or `None` for a report that
/// names none and for a walk's, which lists only what it touched.
fn paths_of(event: &CoreEvent) -> Option<Vec<String>> {
    let payload = &event.payload;
    let paths: Vec<String> = if event.event_type == "file:changed" {
        if payload.get("rescanned").is_some() {
            return None;
        }
        let paths = payload.get("paths")?.as_array()?;
        paths
            .iter()
            .map(|path| Some(path.as_str()?.to_owned()))
            .collect::<Option<_>>()?
    } else if event.event_type.starts_with("object:") {
        ["path", "previousPath"]
            .iter()
            .filter_map(|key| payload.get(*key)?.as_str().map(str::to_owned))
            .collect()
    } else {
        return None;
    };
    Some(
        paths
            .into_iter()
            .filter(|path| !path.starts_with(".noura/"))
            .collect(),
    )
}

/// Whether a report without paths can stand for a change on disk, or for a
/// change in what the walk leaves out.
fn changes_files(event: &CoreEvent) -> bool {
    event.event_type.starts_with("file:")
        || event.event_type.starts_with("object")
        || event.event_type == "workspace:manifest-updated"
}

impl WorkspaceSync {
    /// Finds local changes from the engine's reports from now on, after one
    /// walk. The app calls this for a folder it watches.
    pub fn follow_changes(&mut self) {
        if self.finding.events.is_none() {
            self.finding.events = Some(self.engine.subscribe());
        }
    }

    /// Sets how often a following device walks the workspace anyway.
    pub fn set_walk_every(&mut self, every: Duration) {
        self.finding.every = Some(every);
    }

    /// How many times this device walked the workspace to find changes.
    pub fn walks(&self) -> u64 {
        self.finding.walks
    }

    /// The files to compare with the journal, and whether they came from
    /// the journal rather than a walk.
    pub(super) fn local_listing(&mut self) -> Result<(SyncedListing, bool), SyncError> {
        let every = self.finding.every.unwrap_or(WALK_EVERY);
        let complete = self.finding.drain();
        let fresh = self
            .finding
            .walked
            .is_some_and(|walked| walked.elapsed() < every);
        // What waits outside the journal's objects only a walk can check.
        let waiting = self.skipped.iter().any(|s| s.why == SkipWhy::WaitsForPage);
        let plain =
            self.state.deleted_away.is_empty() && self.state.held_roots.is_empty() && !waiting;
        if complete && fresh && plain && self.engine.is_watching() {
            let named = std::mem::take(&mut self.finding.named);
            if let Some(listing) = self.listing_from_journal(&named) {
                return Ok((listing, true));
            }
        }
        // Reports that arrive during the walk stay for the next capture.
        self.finding.named.clear();
        self.finding.walks += 1;
        let listing = self.engine.synced_listing().map_err(core)?;
        self.finding.left_out = listing.skipped.clone();
        self.finding.walked = self.finding.events.is_some().then(Instant::now);
        Ok((listing, false))
    }

    /// The files the journal maps to objects, fresh for the `named` paths,
    /// or `None` when one of those paths takes a walk.
    fn listing_from_journal(&self, named: &BTreeSet<String>) -> Option<SyncedListing> {
        let known: BTreeSet<&str> = self
            .state
            .objects
            .values()
            .filter_map(|object| object.local.as_ref())
            .map(|local| local.path.as_str())
            .collect();
        let mut files: Vec<SyncedFile> = self
            .state
            .objects
            .values()
            .filter_map(|object| object.local.as_ref())
            .filter(|local| !named.contains(&local.path))
            .map(|local| SyncedFile {
                relative_path: local.path.clone(),
                size: local.size,
                modified_ns: local.modified_ns,
                file_id: local.file_id,
            })
            .collect();
        for path in named {
            match self.engine.synced_entry(path) {
                SyncedEntry::File(file) if known.contains(path.as_str()) => files.push(file),
                // Ignore rules decide on a new file.
                SyncedEntry::File(_) | SyncedEntry::Other => return None,
                SyncedEntry::Absent if known.contains(path.as_str()) => {}
                // A removed folder takes the files beneath it along.
                SyncedEntry::Absent => {
                    let beneath = format!("{path}/");
                    let mut after = known.range(beneath.as_str()..);
                    if after.next().is_some_and(|next| next.starts_with(&beneath)) {
                        return None;
                    }
                }
            }
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Some(SyncedListing {
            files,
            skipped: self.finding.left_out.clone(),
            complete: true,
        })
    }
}
