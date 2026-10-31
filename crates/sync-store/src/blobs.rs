//! Blob uploads and downloads (§21): the rows behind the tus routes and the
//! range reads. The server stores each segment's bytes in its blob store
//! before it records the segment here, so a recorded offset always names
//! bytes that reached durable storage.

use std::sync::Arc;

use sync_protocol::blob::{
    MAX_BLOB_FILE, SEGMENT_LEN, blob_ciphertext_len, digest_of_segment_hashes,
    is_valid_blob_ciphertext_len,
};
use sync_protocol::messages::EntryRecord;
use sync_protocol::policy::PolicyChain;
use sync_protocol::types::{Digest, Id};

use crate::reads::reader;
use crate::{
    Backend, BlobRow, LockPlan, ReadTxn, Result, Session, Store, UPLOAD_TTL, WriteTxn, policy,
    refuse,
};

/// What a tus creation names in `Upload-Length` and `Upload-Metadata`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobCreation {
    /// The blob ID.
    pub blob: Id,
    /// The padded ciphertext length.
    pub length: u64,
    /// `ciphertext_digest`.
    pub digest: Digest,
    /// The domain whose operations may list the blob.
    pub domain: Id,
}

/// A blob's row with the hashes of its stored segments, in offset order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobUpload {
    /// The row.
    pub row: BlobRow,
    /// The segment hashes; segment `i` covers bytes from `i` MiB.
    pub segments: Vec<Digest>,
}

/// What recording a segment did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// The offset moved; the row says whether the upload completed.
    Stored(BlobRow),
    /// The upload reached its length with another digest, and the store
    /// dropped its rows. The caller deletes these segments' objects and
    /// answers `blob_digest_mismatch`.
    Discarded(Vec<Digest>),
}

/// What a blob's row adds to usage while no operation lists it (§20).
fn usage_of(row: &BlobRow) -> i64 {
    i64::try_from(row.length).unwrap_or(i64::MAX)
}

/// The segment length as a `u64`.
const SEGMENT: u64 = SEGMENT_LEN as u64;

/// The policy, after checking that the session's device is active.
async fn active(
    txn: &mut impl ReadTxn,
    cache: &policy::PolicyCache,
    session: Session,
    workspace: Id,
) -> Result<(Arc<PolicyChain>, Id)> {
    let (_, policy) = policy::load(txn, cache, workspace)
        .await?
        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
    let Some(device) = reader(policy.head_snapshot(), session) else {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    };
    Ok((policy, device))
}

/// The row of an upload the session's device created and may still write.
async fn own_upload(
    txn: &mut impl ReadTxn,
    cache: &policy::PolicyCache,
    session: Session,
    workspace: Id,
    blob: Id,
) -> Result<BlobRow> {
    let (policy, device) = active(txn, cache, session, workspace).await?;
    let row = txn
        .blob(workspace, blob)
        .await?
        .filter(|row| row.device == device && !row.marked)
        .ok_or_else(|| refuse("not_found", "no such upload"))?;
    if !policy.head_snapshot().can_write(&device, &row.domain) {
        return Err(refuse("forbidden", "the device can't write this domain"));
    }
    Ok(row)
}

/// The lock plan of a write to one blob: the shared policy lock, the
/// domain's head row, then the blob's row (§15).
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
    /// tus creation, `POST .../blobs`: a new upload at offset 0, for a
    /// device that can write the domain. A retry with the same creation
    /// returns the original row; another creation under the same ID gets
    /// `id_conflict`.
    pub async fn create_blob(
        &self,
        session: Session,
        workspace: Id,
        creation: BlobCreation,
    ) -> Result<BlobRow> {
        if creation.length > blob_ciphertext_len(MAX_BLOB_FILE) {
            return Err(refuse(
                "too_large",
                "the blob exceeds 2 GiB of file content",
            ));
        }
        if !is_valid_blob_ciphertext_len(creation.length) {
            return Err(refuse(
                "padding_invalid",
                "Upload-Length isn't a padded blob's ciphertext length",
            ));
        }
        let cache = self.policies.clone();
        let quota = self.retention.quota_bytes;
        let now = self.clock().now();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    // tus creation locks the domain's head row before the
                    // blob's row (§15).
                    txn.lock(&plan(workspace, creation.domain, creation.blob))
                        .await?;
                    let (policy, device) = active(txn, &cache, session, workspace).await?;
                    let snapshot = policy.head_snapshot();
                    if !snapshot.domains.contains_key(&creation.domain)
                        || txn.domain(workspace, creation.domain).await?.is_none()
                    {
                        return Err(refuse("not_found", "no such domain"));
                    }
                    if !snapshot.can_write(&device, &creation.domain) {
                        return Err(refuse("forbidden", "the device can't write this domain"));
                    }
                    let row = BlobRow {
                        blob: creation.blob,
                        domain: creation.domain,
                        device,
                        length: creation.length,
                        digest: creation.digest,
                        offset: 0,
                        complete: false,
                        delete_after: Some(now + UPLOAD_TTL),
                        marked: false,
                        live_listings: 0,
                    };
                    if let Some(stored) = txn.blob(workspace, creation.blob).await? {
                        if stored.marked {
                            return Err(refuse(
                                "blob_incomplete",
                                "the collector is deleting this upload",
                            ));
                        }
                        let same = stored.domain == row.domain
                            && stored.device == row.device
                            && stored.length == row.length
                            && stored.digest == row.digest;
                        if same {
                            return Ok(stored);
                        }
                        return Err(refuse("id_conflict", "another upload holds this blob ID"));
                    }
                    txn.insert_blob(workspace, &row).await?;
                    let length = i64::try_from(row.length).unwrap_or(i64::MAX);
                    txn.add_usage(workspace, row.domain, length, 0).await?;
                    crate::retention::check_quota(txn, workspace, quota, length).await?;
                    crate::retention::note_pressure(txn, workspace, quota).await?;
                    Ok(row)
                })
            }))
            .await
    }

    /// tus `HEAD`: the upload the session's device created, with its
    /// stored segments.
    pub async fn blob_upload(
        &self,
        session: Session,
        workspace: Id,
        blob: Id,
    ) -> Result<BlobUpload> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let row = own_upload(txn, &cache, session, workspace, blob).await?;
                    let segments = txn.blob_segments(workspace, blob).await?;
                    Ok(BlobUpload { row, segments })
                })
            }))
            .await
    }

    /// Checks that a tus `PATCH` of `len` bytes at `offset` would append
    /// the next segment, before the server stores its bytes.
    pub fn check_segment(row: &BlobRow, offset: u64, len: u64) -> Result<()> {
        if row.complete {
            return Err(refuse("blob_offset_conflict", "the upload is complete"));
        }
        if offset != row.offset {
            return Err(refuse(
                "blob_offset_conflict",
                "Upload-Offset isn't the upload's offset",
            ));
        }
        let rest = row.length - row.offset;
        if len != SEGMENT.min(rest) {
            return Err(refuse(
                "bad_request",
                "a PATCH appends exactly 1 MiB, or the rest of the upload",
            ));
        }
        Ok(())
    }

    /// Records a segment whose bytes the blob store already holds durably,
    /// and moves the offset. At `Upload-Length` it checks
    /// `ciphertext_digest` over the stored segment hashes.
    pub async fn record_segment(
        &self,
        session: Session,
        workspace: Id,
        blob: Id,
        offset: u64,
        len: u64,
        hash: Digest,
    ) -> Result<Recorded> {
        let domain = self.blob_upload(session, workspace, blob).await?.row.domain;
        let cache = self.policies.clone();
        let now = self.clock().now();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, domain, blob)).await?;
                    let row = own_upload(txn, &cache, session, workspace, blob).await?;
                    Self::check_segment(&row, offset, len)?;
                    let next = offset + len;
                    let complete = next == row.length;
                    let index = offset / SEGMENT;
                    if complete {
                        let mut segments = txn.blob_segments(workspace, blob).await?;
                        segments.push(hash);
                        if digest_of_segment_hashes(&segments) != row.digest {
                            txn.delete_blob(workspace, blob).await?;
                            txn.add_usage(workspace, domain, -usage_of(&row), 0).await?;
                            return Ok(Recorded::Discarded(segments));
                        }
                    }
                    txn.append_blob_segment(workspace, blob, index, hash, next, complete)
                        .await?;
                    // A completed upload that no push lists goes 24 hours
                    // after completion (§19).
                    let delete_after = if complete {
                        let at = Some(now + UPLOAD_TTL);
                        txn.set_blob_delete_after(workspace, blob, at).await?;
                        at
                    } else {
                        row.delete_after
                    };
                    Ok(Recorded::Stored(BlobRow {
                        offset: next,
                        complete,
                        delete_after,
                        ..row
                    }))
                })
            }))
            .await
    }

    /// tus `DELETE`: abandons an incomplete upload and returns its stored
    /// segments, whose objects the caller then deletes.
    pub async fn abandon_blob(
        &self,
        session: Session,
        workspace: Id,
        blob: Id,
    ) -> Result<Vec<Digest>> {
        let domain = self.blob_upload(session, workspace, blob).await?.row.domain;
        let cache = self.policies.clone();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, domain, blob)).await?;
                    let row = own_upload(txn, &cache, session, workspace, blob).await?;
                    if row.complete {
                        return Err(refuse(
                            "blob_offset_conflict",
                            "a completed upload can't be abandoned",
                        ));
                    }
                    let segments = txn.blob_segments(workspace, blob).await?;
                    txn.delete_blob(workspace, blob).await?;
                    txn.add_usage(workspace, domain, -usage_of(&row), 0).await?;
                    Ok(segments)
                })
            }))
            .await
    }

    /// `GET .../blobs/{id}`: a completed blob, for an active device that
    /// reads its domain once an operation lists it, and for its uploading
    /// device before then.
    pub async fn blob_download(
        &self,
        session: Session,
        workspace: Id,
        blob: Id,
    ) -> Result<BlobUpload> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let (policy, device) = active(txn, &cache, session, workspace).await?;
                    let row = txn
                        .blob(workspace, blob)
                        .await?
                        .filter(|row| !row.marked)
                        .ok_or_else(|| refuse("not_found", "no such blob"))?;
                    let listed = txn.blob_listed(workspace, blob).await?;
                    let serves = if listed {
                        policy.head_snapshot().can_read(&device, &row.domain)
                    } else {
                        row.device == device
                    };
                    if !serves {
                        return Err(refuse("not_found", "no such blob"));
                    }
                    if !row.complete {
                        return Err(refuse("blob_incomplete", "the upload isn't complete"));
                    }
                    if listed && !above_history_floor(txn, session, workspace, blob, &row).await? {
                        return Err(refuse(
                            "before_history_floor",
                            "only operations below the account's history floor list this blob",
                        ));
                    }
                    let segments = txn.blob_segments(workspace, blob).await?;
                    Ok(BlobUpload { row, segments })
                })
            }))
            .await
    }
}

/// Whether the session's account may read a listed blob under its history
/// floor (§9): an operation that lists it sits above the floor, or is live
/// at a checkpoint the device can read, the floor's or the domain's latest.
async fn above_history_floor(
    txn: &mut impl ReadTxn,
    session: Session,
    workspace: Id,
    blob: Id,
    row: &BlobRow,
) -> Result<bool> {
    let floor = txn
        .history_floor(workspace, session.account, row.domain)
        .await?;
    if floor == 0 {
        return Ok(true);
    }
    let seqs = txn.blob_listing_seqs(workspace, blob).await?;
    if seqs.iter().any(|seq| *seq > floor) {
        return Ok(true);
    }
    let latest = txn
        .domain(workspace, row.domain)
        .await?
        .map_or(floor, |d| d.checkpoint_seq.max(floor));
    for seq in seqs {
        let Some(raw) = txn
            .entries_after(workspace, row.domain, seq.saturating_sub(1), 1)
            .await?
            .into_iter()
            .next()
            .filter(|e| e.seq == seq)
        else {
            continue;
        };
        let EntryRecord::Operation(op) = crate::pull::entry(raw)?.record else {
            continue;
        };
        for at in [floor, latest] {
            let live = txn.live_ops(workspace, op.body.rid, at).await?;
            if live.iter().any(|(op, _)| op.seq == seq) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The push checks of §21 for the blobs an operation lists, under the
/// blob rows' locks: each is a completed upload created for `domain`.
pub(crate) async fn check_listed(
    txn: &mut impl ReadTxn,
    workspace: Id,
    domain: Id,
    blobs: &[Id],
) -> Result<()> {
    for blob in blobs {
        let row = txn.blob(workspace, *blob).await?;
        // A push never lists a marked blob (§19).
        let Some(row) = row.filter(|row| row.complete && !row.marked) else {
            return Err(refuse(
                "blob_incomplete",
                "the workspace holds no completed upload with that ID",
            ));
        };
        if row.domain != domain {
            return Err(refuse(
                "blob_domain_mismatch",
                "the blob was created for another domain",
            ));
        }
    }
    Ok(())
}

/// Records each listed blob against the operation at `seq`.
pub(crate) async fn list(
    txn: &mut impl WriteTxn,
    workspace: Id,
    domain: Id,
    seq: u64,
    blobs: &[Id],
) -> Result<()> {
    for blob in blobs {
        txn.insert_blob_listing(workspace, *blob, domain, seq)
            .await?;
    }
    Ok(())
}
