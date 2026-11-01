//! The blob store (§21): each 1 MiB segment of a blob's ciphertext as its
//! own object, named by its workspace, blob, index and BLAKE3 hash, on local
//! disk or, through object_store, on S3-compatible storage. A `put` returns
//! only once the segment is durable, so the offset the store then records
//! always names bytes any instance can read.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures::TryStreamExt;
use futures::future::BoxFuture;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use sync_protocol::types::{Digest, Id, to_hex};

/// The directory under the data directory that a disk store uses.
pub const BLOB_DIR: &str = "blobs";

/// The object key of segment `index` of `blob`, whose hash is `hash`.
pub fn segment_key(workspace: &Id, blob: &Id, index: u64, hash: &Digest) -> String {
    format!(
        "{}/{}/{index:08}-{}",
        to_hex(&workspace.0),
        to_hex(&blob.0),
        to_hex(&hash.0)
    )
}

/// The BLAKE3 hash a segment key names, if the key has the form
/// [`segment_key`] writes.
pub fn key_hash(key: &str) -> Option<Digest> {
    let name = key.rsplit('/').next()?;
    let (_, hex) = name.split_once('-')?;
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    Some(Digest(bytes.try_into().ok()?))
}

/// Where blob segments live.
pub trait SegmentStore: Send + Sync + 'static {
    /// Stores `bytes` under `key` and returns once they're durable.
    fn put(&self, key: &str, bytes: Bytes) -> BoxFuture<'_, io::Result<()>>;
    /// Reads `range` of the object under `key`.
    fn get(&self, key: &str, range: Range<u64>) -> BoxFuture<'_, io::Result<Bytes>>;
    /// The length of the object under `key`.
    fn size(&self, key: &str) -> BoxFuture<'_, io::Result<u64>>;
    /// Deletes the object under `key`; a missing one is no error.
    fn delete(&self, key: &str) -> BoxFuture<'_, io::Result<()>>;
    /// Every key the store holds.
    fn list(&self) -> BoxFuture<'_, io::Result<Vec<String>>>;
    /// Every key under the directory-like `prefix`.
    fn list_under(&self, prefix: &str) -> BoxFuture<'_, io::Result<Vec<String>>>;
}

/// Segments as files under a directory. Each `put` writes a temporary file,
/// flushes it, renames it into place and flushes the directory.
pub struct DiskStore {
    root: PathBuf,
}

impl DiskStore {
    /// A store under `root`, which it creates.
    pub fn open(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root.join(".partial"))?;
        Ok(Self {
            root: root.to_owned(),
        })
    }

    fn path(&self, key: &str) -> io::Result<PathBuf> {
        // Keys come from segment_key alone, or sit under the quarantine
        // prefix with a date; refuse anything else.
        let parts = if key.starts_with("quarantine/") { 5 } else { 3 };
        let valid = key.split('/').count() == parts
            && key.split('/').all(|part| {
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            });
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a segment key",
            ));
        }
        Ok(self.root.join(key))
    }
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> BoxFuture<'static, io::Result<T>> {
    Box::pin(async move {
        tokio::task::spawn_blocking(job)
            .await
            .map_err(io::Error::other)?
    })
}

impl SegmentStore for DiskStore {
    fn put(&self, key: &str, bytes: Bytes) -> BoxFuture<'_, io::Result<()>> {
        let path = self.path(key);
        let partial = self.root.join(".partial");
        blocking(move || {
            use std::io::Write;
            let path = path?;
            let parent = path.parent().expect("a segment key has a parent");
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(&partial)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(&path).map_err(|e| e.error)?;
            sync_dir(parent)?;
            sync_dir(parent.parent().expect("a workspace directory"))?;
            Ok(())
        })
    }

    fn get(&self, key: &str, range: Range<u64>) -> BoxFuture<'_, io::Result<Bytes>> {
        let path = self.path(key);
        blocking(move || {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(path?)?;
            file.seek(SeekFrom::Start(range.start))?;
            let mut bytes =
                vec![0; usize::try_from(range.end - range.start).map_err(io::Error::other)?];
            file.read_exact(&mut bytes)?;
            Ok(Bytes::from(bytes))
        })
    }

    fn size(&self, key: &str) -> BoxFuture<'_, io::Result<u64>> {
        let path = self.path(key);
        blocking(move || Ok(std::fs::metadata(path?)?.len()))
    }

    fn delete(&self, key: &str) -> BoxFuture<'_, io::Result<()>> {
        let path = self.path(key);
        blocking(move || {
            let path = path?;
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
            // Drop the blob's directory once it's empty.
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            Ok(())
        })
    }

    fn list(&self) -> BoxFuture<'_, io::Result<Vec<String>>> {
        self.list_under("")
    }

    fn list_under(&self, prefix: &str) -> BoxFuture<'_, io::Result<Vec<String>>> {
        let root = self.root.clone();
        let prefix = prefix.to_owned();
        blocking(move || {
            let mut keys = Vec::new();
            let mut stack = vec![root.join(&prefix)];
            while let Some(dir) = stack.pop() {
                let entries = match std::fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                for entry in entries {
                    let entry = entry?;
                    if entry.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    if entry.file_type()?.is_dir() {
                        stack.push(entry.path());
                    } else if let Ok(key) = entry.path().strip_prefix(&root) {
                        let parts: Vec<_> = key
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy())
                            .collect();
                        keys.push(parts.join("/"));
                    }
                }
            }
            keys.sort();
            Ok(keys)
        })
    }
}

/// Segments in an object_store store, such as S3 or memory. An S3 `PUT`
/// that succeeds is durable.
pub struct Objects {
    store: Arc<dyn ObjectStore>,
}

impl Objects {
    /// Segments in `store`.
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    /// Segments in memory, for tests.
    pub fn memory() -> Self {
        Self::new(Arc::new(object_store::memory::InMemory::new()))
    }

    /// Segments in the S3 bucket `bucket`, configured from the standard
    /// `AWS_*` variables, at `endpoint` when one is given, such as an
    /// S3-compatible service.
    pub fn s3(bucket: &str, endpoint: Option<&str>) -> io::Result<Self> {
        let mut builder = object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);
        if let Some(endpoint) = endpoint {
            builder = builder
                .with_endpoint(endpoint)
                .with_allow_http(endpoint.starts_with("http://"))
                .with_virtual_hosted_style_request(false);
        }
        let store = builder.build().map_err(io::Error::other)?;
        Ok(Self::new(Arc::new(store)))
    }
}

fn object_error(error: object_store::Error) -> io::Error {
    match error {
        object_store::Error::NotFound { .. } => {
            io::Error::new(io::ErrorKind::NotFound, "no such object")
        }
        other => io::Error::other(other),
    }
}

impl SegmentStore for Objects {
    fn put(&self, key: &str, bytes: Bytes) -> BoxFuture<'_, io::Result<()>> {
        let path = ObjectPath::from(key);
        Box::pin(async move {
            self.store
                .put(&path, bytes.into())
                .await
                .map_err(object_error)?;
            Ok(())
        })
    }

    fn get(&self, key: &str, range: Range<u64>) -> BoxFuture<'_, io::Result<Bytes>> {
        let path = ObjectPath::from(key);
        Box::pin(async move {
            if range.is_empty() {
                return Ok(Bytes::new());
            }
            self.store
                .get_range(&path, range)
                .await
                .map_err(object_error)
        })
    }

    fn size(&self, key: &str) -> BoxFuture<'_, io::Result<u64>> {
        let path = ObjectPath::from(key);
        Box::pin(async move {
            let meta = self.store.head(&path).await.map_err(object_error)?;
            Ok(meta.size)
        })
    }

    fn delete(&self, key: &str) -> BoxFuture<'_, io::Result<()>> {
        let path = ObjectPath::from(key);
        Box::pin(async move {
            match self.store.delete(&path).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
                Err(e) => Err(io::Error::other(e)),
            }
        })
    }

    fn list(&self) -> BoxFuture<'_, io::Result<Vec<String>>> {
        self.list_under("")
    }

    fn list_under(&self, prefix: &str) -> BoxFuture<'_, io::Result<Vec<String>>> {
        let prefix = (!prefix.is_empty()).then(|| ObjectPath::from(prefix));
        Box::pin(async move {
            let mut keys: Vec<String> = self
                .store
                .list(prefix.as_ref())
                .map_ok(|meta| meta.location.to_string())
                .try_collect()
                .await
                .map_err(object_error)?;
            keys.sort();
            Ok(keys)
        })
    }
}

/// What a migration did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Migration {
    /// Segments copied to the target and deleted from the source.
    pub moved: usize,
}

/// `blobs migrate-to-s3`: copies each segment from `from` to `to`, reads it
/// back and checks its BLAKE3 hash against the hash its key names, and only
/// then deletes the source copy. A segment already on the target is checked
/// the same way, so a rerun after an interruption finishes the job.
pub async fn migrate(from: &dyn SegmentStore, to: &dyn SegmentStore) -> io::Result<Migration> {
    let mut migration = Migration::default();
    for key in from.list().await? {
        let expected = key_hash(&key).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{key} isn't a segment"))
        })?;
        let bytes = from.get(&key, 0..from.size(&key).await?).await?;
        if digest_of(&bytes) != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{key} doesn't match its hash on the source"),
            ));
        }
        to.put(&key, bytes.clone()).await?;
        let copied = to.get(&key, 0..bytes.len() as u64).await?;
        if digest_of(&copied) != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{key} doesn't match its hash on the target"),
            ));
        }
        from.delete(&key).await?;
        migration.moved += 1;
    }
    Ok(migration)
}

fn digest_of(bytes: &[u8]) -> Digest {
    sync_protocol::crypto::hash(bytes)
}

/// The blob store suite: every store passes it. `fresh` is an empty store.
pub mod conformance {
    use super::*;

    fn key(index: u64, bytes: &[u8]) -> (String, Digest) {
        let hash = sync_protocol::crypto::hash(bytes);
        let workspace = Id([1; 16]);
        let blob = Id([2; 16]);
        (segment_key(&workspace, &blob, index, &hash), hash)
    }

    /// Puts, ranged gets, lists and deletes round-trip, a missing object
    /// reads as an error and deletes quietly, and keys name their hashes.
    pub async fn round_trips(store: &dyn SegmentStore) {
        let first: Vec<u8> = (0..1_048_576u32).map(|i| (i % 253) as u8).collect();
        let last = b"the last segment".to_vec();
        let (k0, h0) = key(0, &first);
        let (k1, _) = key(1, &last);
        assert_eq!(key_hash(&k0), Some(h0));
        store
            .put(&k0, Bytes::from(first.clone()))
            .await
            .expect("put");
        store
            .put(&k1, Bytes::from(last.clone()))
            .await
            .expect("put");
        assert_eq!(
            store.get(&k0, 1000..2000).await.expect("range")[..],
            first[1000..2000]
        );
        assert_eq!(
            store.get(&k1, 0..last.len() as u64).await.expect("get")[..],
            last[..]
        );
        let mut keys = vec![k0.clone(), k1.clone()];
        keys.sort();
        assert_eq!(store.list().await.expect("list"), keys);
        // A second put of the same key replaces it whole.
        store
            .put(&k1, Bytes::from(last.clone()))
            .await
            .expect("again");
        store.delete(&k1).await.expect("delete");
        store
            .delete(&k1)
            .await
            .expect("a missing object deletes quietly");
        assert!(store.get(&k1, 0..1).await.is_err());
        assert_eq!(store.list().await.expect("list"), vec![k0.clone()]);
        assert_eq!(store.size(&k0).await.expect("length"), first.len() as u64);
        store.delete(&k0).await.expect("delete");
        assert!(store.list().await.expect("list").is_empty());
    }

    /// A migration moves every segment, checks it and deletes the source;
    /// a rerun after an interruption finishes the job.
    pub async fn migrates(from: &dyn SegmentStore, to: &dyn SegmentStore) {
        let segments: Vec<Vec<u8>> = (0..3u8).map(|i| vec![i; 4096 + usize::from(i)]).collect();
        let keys: Vec<String> = segments
            .iter()
            .enumerate()
            .map(|(i, s)| key(i as u64, s).0)
            .collect();
        for (k, s) in keys.iter().zip(&segments) {
            from.put(k, Bytes::from(s.clone())).await.expect("put");
        }
        // An interrupted run copied the first segment and stopped before
        // deleting it.
        to.put(&keys[0], Bytes::from(segments[0].clone()))
            .await
            .expect("put");
        let migration = migrate(from, to).await.expect("migrates");
        assert_eq!(migration.moved, 3);
        assert!(from.list().await.expect("list").is_empty());
        assert_eq!(to.list().await.expect("list"), keys);
        for (k, s) in keys.iter().zip(&segments) {
            assert_eq!(to.get(k, 0..s.len() as u64).await.expect("get")[..], s[..]);
        }
        assert_eq!(migrate(from, to).await.expect("a rerun").moved, 0);

        // A corrupt source segment stops the migration and stays put.
        let (bad, _) = key(9, b"expected");
        from.put(&bad, Bytes::from_static(b"tampered"))
            .await
            .expect("put");
        assert!(migrate(from, to).await.is_err());
        assert_eq!(from.list().await.expect("list"), vec![bad]);
    }
}
