//! Durable writes that have not reached disk yet.
//!
//! A service that writes a file on another thread registers the write here
//! first and finishes it once the bytes are durable or the write fails.
//! Restarting to update waits for every registered write and refuses while
//! one has failed, so an update never drops changes that aren't on disk.
//! The desktop app also waits for registered writes before it quits.

use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::Duration,
};

use crate::{AppError, files::remap};

/// A write whose changes didn't reach disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFailure {
    /// The file, as a workspace-relative path.
    pub file: String,
    /// Why the write failed, in words for the person using Noura.
    pub message: String,
}

/// The writes in progress and the files whose last write failed. Clones
/// share the same list.
#[derive(Clone, Default)]
pub struct PendingWrites(Arc<Shared>);

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    finished: Condvar,
}

#[derive(Default)]
struct State {
    next_id: u64,
    running: Vec<(u64, String)>,
    failed: Vec<WriteFailure>,
}

impl PendingWrites {
    /// Register a write to `file` that is about to start. The write counts
    /// as pending until [`PendingWrite::finish`] reports how it went.
    pub fn begin(&self, file: &str) -> PendingWrite {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.running.push((id, file.to_owned()));
        PendingWrite {
            writes: self.clone(),
            id,
            file: file.to_owned(),
            finished: false,
        }
    }

    /// Register background work that writes files and reports its own
    /// failures, such as setting up a workspace. Waits count it as pending
    /// until the returned [`PendingWork`] drops, however the work ends.
    pub fn begin_work(&self, label: &str) -> PendingWork {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.running.push((id, label.to_owned()));
        PendingWork {
            writes: self.clone(),
            id,
        }
    }

    /// Wait until every write in progress has finished. Fails with the
    /// first file whose last write failed, since its changes aren't on disk.
    pub fn flush(&self) -> Result<(), WriteFailure> {
        let mut state = self.lock();
        while !state.running.is_empty() {
            state = self
                .0
                .finished
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        match state.failed.first() {
            Some(failure) => Err(failure.clone()),
            None => Ok(()),
        }
    }

    /// Wait up to `limit` for the writes in progress to finish, as Noura
    /// does before it quits. Returns whether they all finished.
    pub fn wait_for_running(&self, limit: Duration) -> bool {
        let state = self.lock();
        let (state, _) = self
            .0
            .finished
            .wait_timeout_while(state, limit, |state| !state.running.is_empty())
            .unwrap_or_else(|error| error.into_inner());
        state.running.is_empty()
    }

    /// The item at `path` went to the trash, or left the workspace, so the
    /// changes that failed to reach it and the files inside it no longer
    /// matter.
    pub fn forget(&self, path: &str) {
        self.lock()
            .failed
            .retain(|failure| remap(&failure.file, path, path).is_none());
    }

    /// The item at `from` moved to `to`. A failed write to it, or to a file
    /// inside it, now names the new place, and a later save there clears it.
    pub fn moved(&self, from: &str, to: &str) {
        for failure in &mut self.lock().failed {
            if let Some(file) = remap(&failure.file, from, to) {
                failure.file = file;
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn finish(&self, id: u64, file: &str, failure: Option<String>) {
        let mut state = self.lock();
        state.running.retain(|(running, _)| *running != id);
        state.failed.retain(|failed| failed.file != file);
        if let Some(message) = failure {
            state.failed.push(WriteFailure {
                file: file.to_owned(),
                message,
            });
        }
        drop(state);
        self.0.finished.notify_all();
    }

    fn end_work(&self, id: u64) {
        self.lock().running.retain(|(running, _)| *running != id);
        self.0.finished.notify_all();
    }
}

/// Registered background work that reports its own failures. Waits for
/// pending writes include it until it drops.
pub struct PendingWork {
    writes: PendingWrites,
    id: u64,
}

impl Drop for PendingWork {
    fn drop(&mut self) {
        self.writes.end_work(self.id);
    }
}

/// A registered write. Report how it went with [`PendingWrite::finish`].
/// Dropping it unfinished counts as a failure, because nothing says the
/// bytes reached disk.
pub struct PendingWrite {
    writes: PendingWrites,
    id: u64,
    file: String,
    finished: bool,
}

impl PendingWrite {
    /// The write is over: `Ok` once the bytes are durable, or the error
    /// that stopped it. A success clears an earlier failure of the same
    /// file.
    pub fn finish(mut self, result: Result<(), AppError>) {
        self.finished = true;
        let failure = result.err().map(|error| error.message);
        self.writes.finish(self.id, &self.file, failure);
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        if !self.finished {
            self.writes.finish(
                self.id,
                &self.file,
                Some("Noura stopped saving it before the write finished.".to_owned()),
            );
        }
    }
}
