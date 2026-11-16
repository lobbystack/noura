//! Speech in Noura's kernel (ADR 0006): recording the microphone and the
//! sound other apps play, speech-to-text models on this device, the model
//! catalog and its downloads, pasting into other apps, and this device's
//! speech choices. Plugins reach it only through the `speech` and `paste`
//! interfaces, which `app-core` serves from [`Speech`].
//!
//! Nothing here touches the workspace. Models, recordings and choices live
//! in the app data folder and never sync.

pub mod audio;
pub mod catalog;
pub mod chunker;
#[cfg(feature = "download")]
pub mod download;
pub mod engine;
pub mod paste;
pub mod recording;
pub mod state;

mod hub;

pub use hub::{Event, ModelRow, Speech, SpeechConfig};

/// Why a speech call failed: a code for logs and tests, and a message in
/// words for you.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SpeechError {
    pub code: String,
    pub message: String,
}

impl SpeechError {
    pub fn failed(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
        }
    }

    pub fn not_found(what: &str) -> Self {
        Self::failed("not_found", what)
    }

    pub fn cancelled() -> Self {
        Self::failed("cancelled", "The recording was cancelled.")
    }

    pub(crate) fn io(error: &std::io::Error, what: &str) -> Self {
        tracing::warn!(%error, what, "a speech file operation failed");
        Self::failed(
            "io",
            "Noura couldn't write its speech files. Check the disk.",
        )
    }
}

pub type Result<T> = std::result::Result<T, SpeechError>;
