//! The blob store suite (§21) against local disk, memory and an
//! S3-compatible emulator, and the migration from disk to S3.

mod emulator;

use emulator::Emulator;
use sync_server::blobs::{DiskStore, Objects, SegmentStore, conformance};

#[tokio::test]
async fn disk_store_passes_the_suite() {
    let root = tempfile::tempdir().expect("temporary directory");
    let store = DiskStore::open(root.path()).expect("opens");
    conformance::round_trips(&store).await;
}

#[tokio::test]
async fn memory_store_passes_the_suite() {
    conformance::round_trips(&Objects::memory()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s3_emulator_passes_the_suite() {
    let emulator = Emulator::start().await;
    conformance::round_trips(&emulator.store()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disk_blobs_migrate_to_s3() {
    let root = tempfile::tempdir().expect("temporary directory");
    let disk = DiskStore::open(root.path()).expect("opens");
    let emulator = Emulator::start().await;
    conformance::migrates(&disk, &emulator.store()).await;
}

#[tokio::test]
async fn disk_store_refuses_keys_outside_its_layout() {
    let root = tempfile::tempdir().expect("temporary directory");
    let store = DiskStore::open(root.path()).expect("opens");
    for key in [
        "../escape/a/b",
        "a/b",
        "/abs/olute/key",
        "a/../b",
        "a/b/c/d",
    ] {
        assert!(
            store
                .put(key, bytes::Bytes::from_static(b"x"))
                .await
                .is_err(),
            "{key}"
        );
    }
}
