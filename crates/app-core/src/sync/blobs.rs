//! Blob transfers (sync-v2.md §21) on [`SyncClient`]: a tus upload in
//! 1 MiB segments that resumes from the server's offset after a dropped
//! connection, and a download in ranges of at most 1 MiB whose segment
//! hashes the client checks against `ciphertext_digest` before it decrypts.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sync_protocol::blob::{SEGMENT_LEN, decrypt_blob_to, digest_of_segment_hashes};
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

    /// Downloads the blob `blob_ref` names in ranges of at most 1 MiB into
    /// a temporary file in `dir`, hashing each segment as it arrives, and
    /// checks the hashes against `ciphertext_digest` before it decrypts.
    /// It then decrypts one chunk at a time into a second temporary file,
    /// and returns that file's path once `plaintext_length` equals the
    /// file's length and, when given, its hash equals `revision`. Memory
    /// stays at about one segment whatever the blob's size. On any failure
    /// both files are removed.
    pub async fn download_blob(
        &mut self,
        workspace: Id,
        blob_ref: &BlobRef,
        revision: Option<&Digest>,
        dir: &Path,
    ) -> Result<PathBuf, SyncError> {
        let name = sync_protocol::types::to_hex(&blob_ref.blob.0);
        let ciphertext = dir.join(format!("{name}.cipher"));
        let plaintext = dir.join(format!("{name}.plain"));
        let result = self
            .download_into(workspace, blob_ref, revision, &ciphertext, &plaintext)
            .await;
        let _ = std::fs::remove_file(&ciphertext);
        if result.is_err() {
            let _ = std::fs::remove_file(&plaintext);
        }
        result.map(|()| plaintext)
    }

    async fn download_into(
        &mut self,
        workspace: Id,
        blob_ref: &BlobRef,
        revision: Option<&Digest>,
        ciphertext: &Path,
        plaintext: &Path,
    ) -> Result<(), SyncError> {
        let disk = |e: std::io::Error| SyncError::Invalid(format!("can't keep the download: {e}"));
        if let Some(parent) = ciphertext.parent() {
            std::fs::create_dir_all(parent).map_err(disk)?;
        }
        let length = blob_ref.ciphertext_length;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(ciphertext)
            .map_err(disk)?;
        let mut received = 0u64;
        let mut hashes = Vec::new();
        let mut failures = 0;
        while received < length {
            let start = received;
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
            let want = end + 1 - start;
            // The segment streams to disk as it arrives and is hashed on
            // the way, so it never sits whole in memory.
            let segment = match self.tus(request).await {
                Ok(mut response) => {
                    let mut hasher = blake3::Hasher::new();
                    let mut got = 0u64;
                    let streamed = async {
                        while let Some(chunk) = response.chunk().await.map_err(network)? {
                            got += chunk.len() as u64;
                            if got > want {
                                return Err(corrupt("came back in the wrong length"));
                            }
                            hasher.update(&chunk);
                            file.write_all(&chunk).map_err(disk)?;
                        }
                        Ok(())
                    }
                    .await;
                    streamed.map(|()| (got, Digest(*hasher.finalize().as_bytes())))
                }
                Err(error) => Err(error),
            };
            match segment {
                Ok((got, segment_hash)) => {
                    if got != want {
                        return Err(corrupt("came back in the wrong length"));
                    }
                    hashes.push(segment_hash);
                    received += got;
                }
                Err(SyncError::Network(message)) => {
                    // Start the segment again where it began.
                    file.set_len(start).map_err(disk)?;
                    file.seek(SeekFrom::Start(start)).map_err(disk)?;
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
        file.seek(SeekFrom::Start(0)).map_err(disk)?;
        let mut out = std::io::BufWriter::with_capacity(
            1 << 16,
            std::fs::File::create(plaintext).map_err(disk)?,
        );
        let file_hash = decrypt_blob_to(
            std::io::BufReader::with_capacity(1 << 16, file),
            blob_ref,
            &mut out,
        )
        .map_err(|e| corrupt(&e.to_string()))?;
        if revision.is_some_and(|revision| file_hash != *revision) {
            return Err(corrupt(
                &sync_protocol::blob::BlobError::Revision.to_string(),
            ));
        }
        out.flush().map_err(disk)?;
        Ok(())
    }
}
