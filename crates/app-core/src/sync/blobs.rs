//! Blob transfers (sync-v2.md §21) on [`SyncClient`]: a tus upload in
//! 1 MiB segments that resumes from the server's offset after a dropped
//! connection, and a download in ranges of at most 1 MiB whose segment
//! hashes the client checks against `ciphertext_digest` before it decrypts.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sync_protocol::blob::{SEGMENT_LEN, decrypt_blob, digest_of_segment_hashes};
use sync_protocol::crypto::hash;
use sync_protocol::records::BlobRef;
use sync_protocol::types::{Digest, Id};

use super::{SyncClient, SyncError, network};

/// How many times a transfer starts again after a dropped connection.
const RETRIES: u32 = 5;

const SEGMENT: u64 = SEGMENT_LEN as u64;

fn corrupt(what: &str) -> SyncError {
    SyncError::Invalid(format!("the downloaded blob {what}"))
}

impl SyncClient {
    fn blob_path(workspace: &Id, blob: &Id) -> String {
        format!(
            "/v2/workspaces/{}/blobs/{}",
            workspace.to_base64url(),
            blob.to_base64url()
        )
    }

    /// Sends a tus request and returns its response, or the §26 refusal.
    async fn tus(
        &mut self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, SyncError> {
        let response = request
            .bearer_auth(&self.token)
            .header("Tus-Resumable", "1.0.0")
            .send()
            .await
            .map_err(network)?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(network)?;
        let error =
            <sync_protocol::messages::ErrorBody as sync_protocol::codec::Decode>::from_bytes(
                &bytes,
            )
            .map_err(network)?;
        Err(SyncError::Refused {
            code: error.code.0,
            status,
            detail: error.detail.map(|d| d.0),
        })
    }

    /// The server's `Upload-Offset` for the upload of `blob`.
    async fn blob_offset(&mut self, workspace: &Id, blob: &Id) -> Result<u64, SyncError> {
        let url = format!("{}{}", self.base, Self::blob_path(workspace, blob));
        let response = self.tus(self.http.head(url)).await?;
        response
            .headers()
            .get("Upload-Offset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| sync_protocol::types::from_decimal(v).ok())
            .ok_or_else(|| SyncError::Network("HEAD returned no Upload-Offset".into()))
    }

    /// Uploads the ciphertext in `file` as the blob `blob_ref` names, for
    /// `domain`. A retried creation gets the original upload, so a device
    /// that crashed midway resumes from the server's offset; a dropped
    /// connection resumes the same way, up to five times.
    pub async fn upload_blob(
        &mut self,
        workspace: Id,
        domain: Id,
        blob_ref: &BlobRef,
        file: &Path,
    ) -> Result<(), SyncError> {
        let metadata = format!(
            "blob {},digest {},domain {}",
            STANDARD.encode(blob_ref.blob.0),
            STANDARD.encode(blob_ref.ciphertext_digest.0),
            STANDARD.encode(domain.0)
        );
        let url = format!(
            "{}/v2/workspaces/{}/blobs",
            self.base,
            workspace.to_base64url()
        );
        let request = self
            .http
            .post(url)
            .header("Upload-Length", blob_ref.ciphertext_length)
            .header("Upload-Metadata", metadata);
        self.tus(request).await?;
        let mut source = std::fs::File::open(file)
            .map_err(|e| SyncError::Invalid(format!("can't read the blob to upload: {e}")))?;
        let mut failures = 0;
        loop {
            let offset = self.blob_offset(&workspace, &blob_ref.blob).await?;
            if offset >= blob_ref.ciphertext_length {
                return Ok(());
            }
            let len = SEGMENT.min(blob_ref.ciphertext_length - offset);
            let mut segment = vec![0; len as usize];
            source
                .seek(SeekFrom::Start(offset))
                .and_then(|_| source.read_exact(&mut segment))
                .map_err(|e| SyncError::Invalid(format!("can't read the blob to upload: {e}")))?;
            let url = format!(
                "{}{}",
                self.base,
                Self::blob_path(&workspace, &blob_ref.blob)
            );
            let request = self
                .http
                .patch(url)
                .header("Upload-Offset", offset)
                .header("Content-Type", "application/offset+octet-stream")
                .body(segment);
            match self.tus(request).await {
                Ok(_) => {}
                // Another attempt moved the offset; read it again.
                Err(SyncError::Refused { code, .. }) if code == "blob_offset_conflict" => {}
                Err(SyncError::Network(message)) => {
                    failures += 1;
                    if failures > RETRIES {
                        return Err(SyncError::Network(message));
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Downloads the blob `blob_ref` names in ranges of at most 1 MiB,
    /// checks each segment's hash against `ciphertext_digest` before it
    /// decrypts, and returns the file's bytes once `plaintext_length` equals
    /// their length and, when given, their hash equals `revision`.
    pub async fn download_blob(
        &mut self,
        workspace: Id,
        blob_ref: &BlobRef,
        revision: Option<&Digest>,
    ) -> Result<Vec<u8>, SyncError> {
        let length = blob_ref.ciphertext_length;
        let mut ciphertext = Vec::with_capacity(usize::try_from(length).map_err(network)?);
        let mut hashes = Vec::new();
        let mut failures = 0;
        while (ciphertext.len() as u64) < length {
            let start = ciphertext.len() as u64;
            let end = (start + SEGMENT).min(length) - 1;
            let url = format!(
                "{}{}",
                self.base,
                Self::blob_path(&workspace, &blob_ref.blob)
            );
            let request = self
                .http
                .get(url)
                .header("Range", format!("bytes={start}-{end}"));
            let segment = match self.tus(request).await {
                Ok(response) => response.bytes().await.map_err(network),
                Err(error) => Err(error),
            };
            match segment {
                Ok(segment) => {
                    if segment.len() as u64 != end + 1 - start {
                        return Err(corrupt("came back in the wrong length"));
                    }
                    hashes.push(hash(&segment));
                    ciphertext.extend_from_slice(&segment);
                }
                Err(SyncError::Network(message)) => {
                    failures += 1;
                    if failures > RETRIES {
                        return Err(SyncError::Network(message));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        if digest_of_segment_hashes(&hashes) != blob_ref.ciphertext_digest {
            return Err(corrupt("doesn't match its digest"));
        }
        decrypt_blob(&ciphertext, blob_ref, revision).map_err(|e| corrupt(&e.to_string()))
    }
}
