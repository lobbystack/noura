//! An S3-compatible emulator for the blob store tests: s3s-fs over a
//! temporary directory, on a loopback port, with test credentials.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use object_store::aws::AmazonS3Builder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use sync_server::blobs::Objects;
use tokio::net::TcpListener;

/// The emulator's bucket.
pub const BUCKET: &str = "blobs";
/// The emulator's test access key.
pub const ACCESS_KEY: &str = "test-access";
/// The emulator's test secret key.
pub const SECRET_KEY: &str = "test-secret";

/// A running emulator: its address and the directory it serves.
pub struct Emulator {
    pub address: SocketAddr,
    pub root: tempfile::TempDir,
}

impl Emulator {
    /// Starts an emulator with one empty bucket. It serves until the test
    /// ends.
    pub async fn start() -> Self {
        let root = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir(root.path().join(BUCKET)).expect("bucket");
        let fs = s3s_fs::FileSystem::new(root.path()).expect("emulator");
        let mut builder = S3ServiceBuilder::new(fs);
        builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
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
        Self { address, root }
    }

    /// The endpoint URL.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    /// A client for the bucket.
    pub fn store(&self) -> Objects {
        let store = AmazonS3Builder::new()
            .with_bucket_name(BUCKET)
            .with_region("us-east-1")
            .with_access_key_id(ACCESS_KEY)
            .with_secret_access_key(SECRET_KEY)
            .with_endpoint(self.endpoint())
            .with_allow_http(true)
            .with_virtual_hosted_style_request(false)
            .build()
            .expect("client");
        Objects::new(Arc::new(store))
    }
}
