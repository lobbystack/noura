//! Remote images in notes load only when you ask, so opening a note never
//! tells a website you read it. Each address loads once through
//! `local-core`'s fetcher, and a failed load can be tried again. Every
//! window shares what loaded.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use local_core::RemoteImageFetcher;

/// Where a remote image stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteImageState {
    /// Nobody asked for it yet, so Noura sent no request.
    NotLoaded,
    Loading,
    /// Its bytes, as the server sent them.
    Loaded(Arc<Vec<u8>>),
    /// Why the load failed, in plain words. Loading again tries again.
    Failed(String),
}

/// The remote images of the open workspace's notes.
#[derive(Default)]
pub(crate) struct RemoteImages {
    fetcher: RwLock<RemoteImageFetcher>,
    states: Mutex<HashMap<String, RemoteImageState>>,
    /// Counts the loads that ended.
    seq: AtomicU64,
}

impl RemoteImages {
    pub(crate) fn state(&self, url: &str) -> RemoteImageState {
        self.states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(url)
            .cloned()
            .unwrap_or(RemoteImageState::NotLoaded)
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// Start loading `url` unless it's loading or loaded, and call `done`
    /// once the load ends.
    pub(crate) fn load(self: &Arc<Self>, url: &str, done: impl FnOnce() + Send + 'static) {
        {
            let mut states = self
                .states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if matches!(
                states.get(url),
                Some(RemoteImageState::Loading | RemoteImageState::Loaded(_))
            ) {
                return;
            }
            states.insert(url.to_owned(), RemoteImageState::Loading);
        }
        let fetcher = self
            .fetcher
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let images = Arc::clone(self);
        let url = url.to_owned();
        std::thread::spawn(move || {
            let state = match fetcher.fetch(&url) {
                Ok(image) => RemoteImageState::Loaded(Arc::new(image.bytes)),
                Err(error) => {
                    tracing::info!(code = %error.code, "a remote image didn't load");
                    RemoteImageState::Failed(failure(&error.code))
                }
            };
            images
                .states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(url, state);
            images.seq.fetch_add(1, Ordering::SeqCst);
            done();
        });
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn allow_loopback(&self) {
        *self
            .fetcher
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            RemoteImageFetcher::allowing_loopback();
    }
}

/// Why a load failed, as a note shows it.
fn failure(code: &str) -> String {
    match code {
        "not_an_image" => "That address isn\u{2019}t an image.",
        "image_too_large" => "The image is larger than 20 MB.",
        "image_timeout" => "The image took too long to load.",
        "image_address_invalid" => "Noura loads images only from web addresses.",
        "image_address_private" => {
            "Noura doesn\u{2019}t load images from this computer or its local network."
        }
        _ => "Noura couldn\u{2019}t load the image.",
    }
    .to_owned()
}
