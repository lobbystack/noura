//! The server's background work (docs/workspace-format/sync-v2.md §19,
//! §20): retention floors, pruning, reclaiming deleted domains, the quota's
//! claim on history, and the blob collector. Each instance runs it; the
//! store's locks serve one at a time.

use std::sync::Arc;
use std::time::Duration;

use sync_store::{Backend, Maintenance, Store};

use crate::blobs::{SegmentStore, segment_key};

/// How often a pass runs.
pub const PERIOD: Duration = Duration::from_secs(60);

/// Takes each blob that's due through the collector's three steps: mark the
/// row, delete the store objects, delete the row. A crash between steps
/// leaves a marked row that the next pass finishes, and so does a failure
/// on one blob, which costs the others nothing. Returns how many blobs it
/// deleted.
pub async fn collect_blobs<B: Backend>(
    store: &Store<B>,
    blobs: &dyn SegmentStore,
) -> Result<u64, sync_store::Error> {
    let mut collected = 0;
    for target in store.collectable_blobs().await? {
        let segments = match store.mark_blob(target).await {
            Ok(Some(segments)) => segments,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!("can't mark a collected blob: {error}");
                continue;
            }
        };
        let mut gone = true;
        for (index, hash) in segments.iter().enumerate() {
            let key = segment_key(&target.workspace, &target.blob, index as u64, hash);
            if let Err(error) = blobs.delete(&key).await {
                tracing::warn!("can't delete a collected segment: {error}");
                gone = false;
            }
        }
        if gone {
            match store.finish_blob(target).await {
                Ok(()) => collected += 1,
                Err(error) => tracing::warn!("can't finish a collected blob: {error}"),
            }
        }
    }
    Ok(collected)
}

/// One pass: the store's work, then the collector, then the store again so
/// a domain whose last blob just went finishes reclaiming.
pub async fn run_once<B: Backend>(
    store: &Store<B>,
    blobs: &dyn SegmentStore,
) -> Result<Maintenance, sync_store::Error> {
    let report = store.maintain().await?;
    if let Some(failure) = &report.failure {
        tracing::error!(failed = report.failed, cause = %failure, "maintenance skipped some work");
    }
    if collect_blobs(store, blobs).await? > 0 {
        store.maintain().await?;
    }
    if let Err(error) = crate::restore::collect_quarantine(blobs, store.clock().now()).await {
        tracing::warn!("can't collect the blob quarantine: {error}");
    }
    Ok(report)
}

/// Runs a pass every [`PERIOD`].
pub fn spawn<B: Backend>(
    store: Arc<Store<B>>,
    blobs: Arc<dyn SegmentStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(PERIOD);
        loop {
            every.tick().await;
            if let Err(error) = run_once(&store, blobs.as_ref()).await {
                tracing::error!(cause = %error, "a maintenance pass failed");
            }
        }
    })
}
