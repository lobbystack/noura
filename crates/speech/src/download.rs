//! Downloading a model file from Hugging Face with `hf-hub`, into the
//! models folder under the app data folder. The bytes land in a `.part`
//! file beside the model, so a download that stops resumes where it left
//! off. A download that gets no bytes for a while stops and tries again
//! from there, a few times. The file becomes the model only once its
//! SHA-256 matches the catalog's.
//!
//! Noura downloads only from Hugging Face. Handy's mirror publishes no
//! terms that let another app use it, so Noura doesn't fall back to it.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use sha2::{Digest, Sha256};

use crate::catalog::ModelFile;
use crate::{Result, SpeechError};

/// Where model files come from.
pub const HUGGING_FACE: &str = "https://huggingface.co";

/// How a download goes.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// The Hub's address, which tests point at a local server.
    pub endpoint: String,
    /// No bytes for this long counts as a stall.
    pub stall: Duration,
    /// How many times a stalled or broken download starts again from
    /// where it stopped.
    pub retries: u32,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            endpoint: HUGGING_FACE.to_owned(),
            stall: Duration::from_secs(30),
            retries: 5,
        }
    }
}

/// How far a download is, shared with whoever shows it.
#[derive(Debug, Default)]
pub struct Progress {
    pub received: AtomicU64,
    pub total: AtomicU64,
    pub cancelled: AtomicBool,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// The `.part` file a download of `destination` writes.
pub fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_owned();
    name.push(".part");
    destination.with_file_name(name)
}

/// Download `file` to `destination`, resuming a `.part` file there.
/// Blocks until the file is complete and checked, fails, or `progress` is
/// cancelled, which keeps the `.part` file for the next try.
pub fn download(
    file: &ModelFile,
    destination: &Path,
    options: &DownloadOptions,
    progress: &Arc<Progress>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| SpeechError::io(&error, "download runtime"))?;
    runtime.block_on(download_async(file, destination, options, progress))
}

fn network_failed() -> SpeechError {
    SpeechError::failed(
        "download_failed",
        "The download stopped. Check your connection and try again.",
    )
}

async fn download_async(
    file: &ModelFile,
    destination: &Path,
    options: &DownloadOptions,
    progress: &Arc<Progress>,
) -> Result<()> {
    let Some((owner, name)) = file.repository.split_once('/') else {
        return Err(SpeechError::failed(
            "download_failed",
            "The catalog names no repository.",
        ));
    };
    if let Some(folder) = destination.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| SpeechError::io(&error, "models folder"))?;
    }
    let client = hf_hub::HFClient::builder()
        .endpoint(options.endpoint.trim_end_matches('/'))
        .cache_enabled(false)
        .retry_max_attempts(1)
        .build()
        .map_err(|error| {
            tracing::warn!(%error, "the Hugging Face client didn't start");
            network_failed()
        })?;
    let repository = client.model(owner, name);
    let part = part_path(destination);
    progress.total.store(file.size, Ordering::SeqCst);
    let mut failures = 0;
    loop {
        if progress.is_cancelled() {
            return Err(SpeechError::cancelled());
        }
        let mut have = std::fs::metadata(&part).map_or(0, |metadata| metadata.len());
        if have > file.size {
            std::fs::remove_file(&part).map_err(|error| SpeechError::io(&error, "part reset"))?;
            have = 0;
        }
        progress.received.store(have, Ordering::SeqCst);
        if have == file.size {
            break;
        }
        match fetch(&repository, file, &part, have, options, progress).await {
            Ok(()) => {}
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(error) => {
                failures += 1;
                tracing::warn!(code = %error.code, failures, "a model download broke off");
                if failures > options.retries {
                    return Err(error);
                }
                tokio::time::sleep(Duration::from_millis(200 * u64::from(failures))).await;
            }
        }
    }
    if !matches_hash(&part, &file.sha256)? {
        let _ = std::fs::remove_file(&part);
        return Err(SpeechError::failed(
            "checksum_mismatch",
            "The downloaded model was damaged. Download it again.",
        ));
    }
    std::fs::rename(&part, destination).map_err(|error| SpeechError::io(&error, "model rename"))?;
    if let Some(folder) = destination.parent()
        && let Ok(folder) = std::fs::File::open(folder)
    {
        let _ = folder.sync_all();
    }
    Ok(())
}

/// Append the bytes after `have` to `part`, until the file is whole, the
/// stream stalls or ends early.
async fn fetch(
    repository: &hf_hub::HFRepository<hf_hub::RepoTypeModel>,
    file: &ModelFile,
    part: &Path,
    have: u64,
    options: &DownloadOptions,
    progress: &Arc<Progress>,
) -> Result<()> {
    let request = repository
        .download_file_stream()
        .filename(file.filename.clone())
        .revision(file.revision.clone())
        .range(have..file.size)
        .send();
    let (_, mut stream) = tokio::time::timeout(options.stall, request)
        .await
        .map_err(|_| stalled())?
        .map_err(|error| {
            tracing::warn!(%error, "the model request failed");
            network_failed()
        })?;
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .map_err(|error| SpeechError::io(&error, "part open"))?;
    let mut received = have;
    loop {
        if progress.is_cancelled() {
            out.sync_all()
                .map_err(|error| SpeechError::io(&error, "part sync"))?;
            return Err(SpeechError::cancelled());
        }
        let next = tokio::time::timeout(options.stall, stream.next())
            .await
            .map_err(|_| stalled())?;
        match next {
            Some(Ok(bytes)) => {
                let room = file.size - received;
                let take = (bytes.len() as u64).min(room) as usize;
                out.write_all(&bytes[..take])
                    .map_err(|error| SpeechError::io(&error, "part write"))?;
                received += take as u64;
                progress.received.store(received, Ordering::SeqCst);
            }
            Some(Err(error)) => {
                tracing::warn!(%error, "the model stream failed");
                out.sync_all()
                    .map_err(|error| SpeechError::io(&error, "part sync"))?;
                return Err(network_failed());
            }
            None => break,
        }
        if received >= file.size {
            break;
        }
    }
    out.sync_all()
        .map_err(|error| SpeechError::io(&error, "part sync"))?;
    if received < file.size {
        return Err(network_failed());
    }
    Ok(())
}

fn stalled() -> SpeechError {
    SpeechError::failed(
        "download_stalled",
        "The download stopped getting data. Check your connection and try again.",
    )
}

/// Whether the file at `path` hashes to `expected`, lowercase hex.
pub fn matches_hash(path: &Path, expected: &str) -> Result<bool> {
    let mut file =
        std::fs::File::open(path).map_err(|error| SpeechError::io(&error, "hash open"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| SpeechError::io(&error, "hash read"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(hex == expected.to_ascii_lowercase())
}
