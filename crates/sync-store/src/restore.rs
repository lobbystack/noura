//! What a restore asks of the store (§14, §19): instance leases, the blob
//! rows the check against the blob store walks, and the changes it makes to
//! them, each under the locks the collector takes.

use sync_protocol::codec::List;
use sync_protocol::messages::MissingBlobsResponse;
use sync_protocol::types::{Digest, Id};

use crate::reads::reader;
use crate::retention::UPLOAD_TTL;
use crate::{
    Backend, BlobRow, LockPlan, ReadTxn, Result, Session, Store, WriteTxn, policy, refuse,
};

/// How many rows one page of [`Store::blob_rows`] holds.
pub const BLOB_PAGE: u32 = 1_024;

fn plan(workspace: Id, domain: Id, blob: Id) -> LockPlan {
    LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: false,
        domains: vec![domain],
        blobs: vec![blob],
    }
}

impl<B: Backend> Store<B> {
    /// Starts or extends this instance's lease.
    pub async fn hold_lease(&self, instance: Id, until: u64) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.hold_lease(instance, until).await })
            }))
            .await
    }

    /// Ends this instance's lease.
    pub async fn drop_lease(&self, instance: Id) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.drop_lease(instance).await })
            }))
            .await
    }

    /// Whether the store records the segment at `index` of `blob` with
    /// this hash, on a row the collector hasn't marked.
    pub async fn knows_segment(
        &self,
        workspace: Id,
        blob: Id,
        index: u64,
        hash: Digest,
    ) -> Result<bool> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    if !txn.blob(workspace, blob).await?.is_some_and(|b| !b.marked) {
                        return Ok(false);
                    }
                    let segments = txn.blob_segments(workspace, blob).await?;
                    let index = usize::try_from(index).unwrap_or(usize::MAX);
                    Ok(segments.get(index) == Some(&hash))
                })
            }))
            .await
    }

    /// A page of every blob row, after `after`.
    pub async fn blob_rows(&self, after: Option<(Id, Id)>) -> Result<Vec<(Id, BlobRow)>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.blob_rows(after, BLOB_PAGE).await })
            }))
            .await
    }

    /// The hashes of a blob's stored segments.
    pub async fn stored_segments(&self, workspace: Id, blob: Id) -> Result<Vec<Digest>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.blob_segments(workspace, blob).await })
            }))
            .await
    }

    /// Whether an operation lists the blob.
    pub async fn is_listed(&self, workspace: Id, blob: Id) -> Result<bool> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.blob_listed(workspace, blob).await })
            }))
            .await
    }

    /// Under the collector's locks, gives a completed blob that no
    /// operation lists a fresh 24 hours. Returns whether it did.
    pub async fn refresh_blob(&self, workspace: Id, domain: Id, blob: Id) -> Result<bool> {
        let until = self.clock().now() + UPLOAD_TTL;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, domain, blob)).await?;
                    let row = txn.blob(workspace, blob).await?;
                    if !row.is_some_and(|b| b.complete && !b.marked)
                        || txn.blob_listed(workspace, blob).await?
                    {
                        return Ok(false);
                    }
                    txn.set_blob_delete_after(workspace, blob, Some(until))
                        .await?;
                    Ok(true)
                })
            }))
            .await
    }

    /// Under the collector's locks, returns a blob whose store objects are
    /// gone to an upload at offset 0. A listed blob waits there for a
    /// device to upload it again; any other gets 24 hours. Returns whether
    /// the blob is listed.
    pub async fn lose_blob(&self, workspace: Id, domain: Id, blob: Id) -> Result<bool> {
        let until = self.clock().now() + UPLOAD_TTL;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, domain, blob)).await?;
                    let listed = txn.blob_listed(workspace, blob).await?;
                    let delete_after = (!listed).then_some(until);
                    txn.reset_blob(workspace, blob, delete_after).await?;
                    Ok(listed)
                })
            }))
            .await
    }

    /// `GET .../missing-blobs`: the blobs a restore left missing, for an
    /// active device (§19, §21).
    pub async fn missing_blobs(
        &self,
        session: Session,
        workspace: Id,
    ) -> Result<MissingBlobsResponse> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let (_, policy) = policy::load(txn, &cache, workspace)
                        .await?
                        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
                    if reader(policy.head_snapshot(), session).is_none() {
                        return Err(refuse(
                            "forbidden",
                            "the device isn't active in this workspace",
                        ));
                    }
                    let blobs = txn.missing_blobs(workspace).await?;
                    Ok(MissingBlobsResponse { blobs: List(blobs) })
                })
            }))
            .await
    }
}
