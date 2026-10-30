//! The blob store suite (§21) against local disk, memory and an
//! S3-compatible emulator, and the migration from disk to S3.

use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use object_store::aws::AmazonS3Builder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use sync_server::blobs::{DiskStore, Objects, SegmentStore, conformance};
use tokio::net::TcpListener;

/// An S3-compatible emulator over a temporary directory, serving one empty
/// bucket on a loopback port. It keeps serving until the test ends.
pub async fn emulator() -> (Objects, tempfile::TempDir) {
    let root = tempfile::tempdir().expect("temporary directory");
    std::fs::create_dir(root.path().join("blobs")).expect("bucket");
    let fs = s3s_fs::FileSystem::new(root.path()).expect("emulator");
    let mut builder = S3ServiceBuilder::new(fs);
    builder.set_auth(SimpleAuth::from_single("test-access", "test-secret"));
    let service = builder.build();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listens");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let server = ConnBuilder::new(TokioExecutor::new());
        while let Ok((socket, _)) = listener.accept().await {
            let connection = server
                .serve_connection(TokioIo::new(socket), service.clone())
                .into_owned();
            tokio::spawn(connection);
        }
    });
    let store = AmazonS3Builder::new()
        .with_bucket_name("blobs")
        .with_region("us-east-1")
        .with_access_key_id("test-access")
        .with_secret_access_key("test-secret")
        .with_endpoint(format!("http://{address}"))
        .with_allow_http(true)
        .with_virtual_hosted_style_request(false)
        .build()
        .expect("client");
    (Objects::new(Arc::new(store)), root)
}

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
    let (store, _root) = emulator().await;
    conformance::round_trips(&store).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disk_blobs_migrate_to_s3() {
    let root = tempfile::tempdir().expect("temporary directory");
    let disk = DiskStore::open(root.path()).expect("opens");
    let (s3, _bucket) = emulator().await;
    conformance::migrates(&disk, &s3).await;
}

#[tokio::test]
async fn disk_store_refuses_keys_outside_its_layout() {
    let root = tempfile::tempdir().expect("temporary directory");
    let store = DiskStore::open(root.path()).expect("opens");
    for key in ["../escape/a/b", "a/b", "/abs/olute/key", "a/../b", "a/b/c/d"] {
        assert!(
            store.put(key, bytes::Bytes::from_static(b"x")).await.is_err(),
            "{key}"
        );
    }
}
