use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

use super::*;
use crate::sync::EncryptedBlob;

const CHUNK: u64 = 1024 * 1024;

impl HttpSyncTransport {
    /// Resume a previously sealed ciphertext file. Never regenerate encryption on retry.
    pub async fn upload_blob(
        &self,
        workspace: &str,
        object: &str,
        epoch: u64,
        descriptor: &EncryptedBlob,
        file: &mut File,
    ) -> Result<()> {
        let path = blob_path(workspace, object, descriptor)?;
        if epoch == 0 || epoch > 9_007_199_254_740_991 {
            return Err(invalid("sync_stale_epoch"));
        }
        file.rewind()
            .map_err(|_| invalid("sync_blob_read_failed"))?;
        descriptor.verify(&mut *file)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reservation {
            id: String,
            offset: u64,
            complete: bool,
            failed: bool,
        }
        let reservation: Reservation = self
            .request(
                Method::POST,
                &format!("/v1/workspaces/{workspace}/objects/{object}/blobs"),
                Some(&serde_json::json!({"id":descriptor.id,"size":descriptor.size,"epoch":epoch})),
            )
            .await?;
        if reservation.id != descriptor.id
            || reservation.offset > descriptor.size
            || reservation.failed
            || (reservation.complete && reservation.offset != descriptor.size)
        {
            return Err(invalid("sync_invalid_blob_response"));
        }
        if reservation.complete {
            return Ok(());
        }
        // HEAD also completes verification if the last PATCH was committed before a disconnect.
        let head = self
            .client
            .head(format!("{}{path}", self.origin))
            .bearer_auth(self.token.as_str())
            .header("Tus-Resumable", "1.0.0")
            .send()
            .await
            .map_err(|_| network_error())?;
        blob_status(&head)?;
        if header_number(&head, "Upload-Length")? != descriptor.size {
            return Err(invalid("sync_invalid_blob_response"));
        }
        let mut offset = header_number(&head, "Upload-Offset")?;
        if offset > descriptor.size {
            return Err(invalid("sync_invalid_blob_response"));
        }
        if offset == descriptor.size {
            return complete(&head);
        }
        let mut buffer = vec![0u8; CHUNK as usize];
        while offset < descriptor.size {
            let length = CHUNK.min(descriptor.size - offset) as usize;
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(&mut buffer[..length]))
                .map_err(|_| invalid("sync_blob_read_failed"))?;
            let response = self
                .client
                .patch(format!("{}{path}", self.origin))
                .bearer_auth(self.token.as_str())
                .header("Tus-Resumable", "1.0.0")
                .header("Content-Type", "application/offset+octet-stream")
                .header("Upload-Offset", offset.to_string())
                .body(buffer[..length].to_vec())
                .send()
                .await
                .map_err(|_| network_error())?;
            blob_status(&response)?;
            offset += length as u64;
            if header_number(&response, "Upload-Offset")? != offset {
                return Err(invalid("sync_invalid_blob_response"));
            }
            if offset == descriptor.size {
                complete(&response)?;
            }
        }
        Ok(())
    }

    /// Resume into a private ciphertext staging file, flushing each range before requesting the next.
    /// The caller publishes the cache entry only after this function authenticates the complete digest.
    pub async fn download_blob(
        &self,
        workspace: &str,
        object: &str,
        descriptor: &EncryptedBlob,
        file: &mut File,
    ) -> Result<()> {
        let path = blob_path(workspace, object, descriptor)?;
        let mut offset = file
            .metadata()
            .map_err(|_| invalid("sync_blob_read_failed"))?
            .len();
        if offset > descriptor.size {
            return Err(invalid("sync_blob_length_mismatch"));
        }
        while offset < descriptor.size {
            let end = (offset + CHUNK).min(descriptor.size) - 1;
            let mut response = self
                .client
                .get(format!("{}{path}/content", self.origin))
                .bearer_auth(self.token.as_str())
                .header("Range", format!("bytes={offset}-{end}"))
                .send()
                .await
                .map_err(|_| network_error())?;
            blob_status(&response)?;
            if response.status() != StatusCode::PARTIAL_CONTENT
                || response
                    .headers()
                    .get("Content-Range")
                    .and_then(|v| v.to_str().ok())
                    != Some(format!("bytes {offset}-{end}/{}", descriptor.size).as_str())
                || header_number(&response, "Content-Length")? != end - offset + 1
            {
                return Err(invalid("sync_invalid_blob_response"));
            }
            // A broken response cannot leave a partial range marked as durable progress.
            let mut bytes = Vec::with_capacity((end - offset + 1) as usize);
            while let Some(chunk) = response.chunk().await.map_err(|_| network_error())? {
                if bytes.len() + chunk.len() > (end - offset + 1) as usize {
                    return Err(invalid("sync_invalid_blob_response"));
                }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.len() != (end - offset + 1) as usize {
                return Err(invalid("sync_invalid_blob_response"));
            }
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.write_all(&bytes))
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            offset = end + 1;
        }
        file.rewind()
            .map_err(|_| invalid("sync_blob_read_failed"))?;
        if let Err(error) = descriptor.verify(&mut *file) {
            // A corrupt resumed prefix must not make every later retry reuse the same bad bytes.
            file.set_len(0)
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            return Err(error);
        }
        file.rewind()
            .map_err(|_| invalid("sync_blob_read_failed"))?;
        Ok(())
    }
}

fn blob_path(workspace: &str, object: &str, descriptor: &EncryptedBlob) -> Result<String> {
    identifier(workspace)?;
    identifier(object)?;
    descriptor.validate()?;
    Ok(format!(
        "/v1/workspaces/{workspace}/objects/{object}/blobs/{}",
        descriptor.id
    ))
}
fn header_number(response: &reqwest::Response, name: &str) -> Result<u64> {
    let value = response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| invalid("sync_invalid_blob_response"))?;
    parse_cursor(value)
}
fn complete(response: &reqwest::Response) -> Result<()> {
    if response
        .headers()
        .get("Noura-Blob-Complete")
        .and_then(|value| value.to_str().ok())
        != Some("true")
    {
        return Err(invalid("sync_blob_incomplete"));
    }
    Ok(())
}
fn blob_status(response: &reqwest::Response) -> Result<()> {
    if response.status().is_success() {
        return Ok(());
    }
    let (code, category) = match response.status() {
        StatusCode::UNAUTHORIZED => ("sync_sign_in_required", ErrorCategory::Credential),
        StatusCode::FORBIDDEN => ("sync_access_denied", ErrorCategory::Permission),
        StatusCode::CONFLICT => ("sync_server_conflict", ErrorCategory::Conflict),
        StatusCode::PAYLOAD_TOO_LARGE => ("sync_quota_or_payload_limit", ErrorCategory::Validation),
        StatusCode::TOO_MANY_REQUESTS => ("sync_rate_limited", ErrorCategory::Transient),
        _ => ("sync_server_unavailable", ErrorCategory::Transient),
    };
    Err(CoreError::new(
        code,
        category,
        "The encrypted attachment request could not complete",
        "sync",
    ))
}
