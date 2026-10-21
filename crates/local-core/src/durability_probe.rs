//! A test probe that records which folders a mutation syncs, so tests can
//! check that every directory entry a change creates reaches the disk
//! before the change returns. Only test builds have it: `local-core`'s own
//! tests and crates that turn on the `durability-probe` feature for their
//! tests.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

thread_local! {
    static SYNCED: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
}

/// Run `work` and return its result with every folder this thread synced
/// meanwhile, in order. Syncing a folder makes the entries in it durable.
/// On platforms where Noura can't sync a folder, the list holds the
/// folders it would have synced.
pub fn record<T>(work: impl FnOnce() -> T) -> (T, Vec<PathBuf>) {
    let outer = SYNCED.with(|synced| synced.borrow_mut().replace(Vec::new()));
    let result = work();
    let recorded = SYNCED.with(|synced| {
        let mut synced = synced.borrow_mut();
        let recorded = synced.take().unwrap_or_default();
        if let Some(mut outer) = outer {
            outer.extend(recorded.iter().cloned());
            *synced = Some(outer);
        }
        recorded
    });
    (result, recorded)
}

/// Note that `folder` was synced, when a recording is running on this
/// thread.
pub(crate) fn synced(folder: &Path) {
    SYNCED.with(|synced| {
        if let Some(synced) = synced.borrow_mut().as_mut() {
            synced.push(folder.to_owned());
        }
    });
}
