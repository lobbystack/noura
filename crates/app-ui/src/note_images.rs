//! The images a note's editor draws: workspace images, read through
//! `app-core`'s containment checks off the UI thread, and remote images,
//! which load only when their Load image button asks. The decoders in
//! `image_decoders` draw both from bytes Noura hands egui. No loader reads
//! a file or the network on its own, and an SVG's references to other
//! files draw nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use app_core::{NoteLinks, RemoteImageState, is_remote_image};
use editor_egui::resolvers::{EmbedResolver, EmbedState};
use egui::load::{SizeHint, TexturePoll};
use egui::{CornerRadius, Rect, TextureOptions, Ui, Vec2};

/// The box an image takes while it loads, in pixels.
const LOADING_SIZE: Vec2 = Vec2::new(96.0, 96.0);

/// Where a workspace image stands.
#[derive(Clone)]
enum Local {
    Reading,
    /// Handed to egui under this address.
    Ready(String),
    /// Nothing to draw: no such image, or one Noura may not read.
    Missing,
}

/// Loads and paints the images of one note.
pub(crate) struct WorkspaceImages {
    links: NoteLinks,
    ctx: egui::Context,
    local: Arc<Mutex<HashMap<String, Local>>>,
    /// The listing the local images were read under. A newer listing,
    /// after any file changed, reads them again.
    listing: AtomicU64,
    /// Counts the changes to what this answers.
    seq: Arc<AtomicU64>,
    /// Addresses whose texture finished decoding.
    decoded: Mutex<Vec<String>>,
    /// Remote images whose bytes egui holds.
    included: Mutex<Vec<String>>,
}

/// The extension egui's decoders recognize for a content type.
fn extension(content_type: &str) -> &'static str {
    match content_type {
        "image/svg+xml" => "svg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/jpeg" => "jpg",
        "image/bmp" => "bmp",
        "image/avif" => "avif",
        _ => "png",
    }
}

impl WorkspaceImages {
    pub(crate) fn new(links: NoteLinks, ctx: egui::Context) -> Self {
        crate::image_decoders::install(&ctx);
        let listing = links.seq();
        Self {
            links,
            ctx,
            local: Arc::default(),
            listing: AtomicU64::new(listing),
            seq: Arc::default(),
            decoded: Mutex::default(),
            included: Mutex::default(),
        }
    }

    /// The address egui knows the image at `url` by, once its bytes are in
    /// hand, or the state to report until then.
    fn address(&self, url: &str) -> Result<String, EmbedState> {
        if is_remote_image(url) {
            return match self.links.remote_image(url) {
                RemoteImageState::NotLoaded => Err(EmbedState::NotLoaded),
                RemoteImageState::Loading => Err(EmbedState::Loading(LOADING_SIZE)),
                RemoteImageState::Failed(_) => Err(EmbedState::Failed),
                RemoteImageState::Loaded(image) => {
                    let address = format!(
                        "bytes://noura-remote/{:x}.{}",
                        hash(url),
                        extension(&image.content_type)
                    );
                    let mut included = self.included.lock().unwrap();
                    if !included.contains(&address) {
                        self.ctx.include_bytes(address.clone(), image.bytes.clone());
                        included.push(address.clone());
                    }
                    Ok(address)
                }
            };
        }
        self.forget_old_listing();
        let state = self.local.lock().unwrap().get(url).cloned();
        match state {
            Some(Local::Ready(address)) => Ok(address),
            Some(Local::Missing) => Err(EmbedState::NotFound),
            Some(Local::Reading) => Err(EmbedState::Loading(LOADING_SIZE)),
            None => {
                self.read(url);
                Err(EmbedState::Loading(LOADING_SIZE))
            }
        }
    }

    /// Read the workspace image at `url` on another thread.
    fn read(&self, url: &str) {
        self.local
            .lock()
            .unwrap()
            .insert(url.to_owned(), Local::Reading);
        let links = self.links.clone();
        let local = Arc::clone(&self.local);
        let seq = Arc::clone(&self.seq);
        let ctx = self.ctx.clone();
        let url = url.to_owned();
        std::thread::spawn(move || {
            let state = match links.image(&url) {
                Some(image) => {
                    let address = format!("bytes://noura-image/{}", image.path);
                    ctx.include_bytes(address.clone(), image.bytes);
                    Local::Ready(address)
                }
                None => Local::Missing,
            };
            local.lock().unwrap().insert(url, state);
            seq.fetch_add(1, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    /// After any file changed, read the images again.
    fn forget_old_listing(&self) {
        let listing = self.links.seq();
        if self.listing.swap(listing, Ordering::SeqCst) == listing {
            return;
        }
        let mut local = self.local.lock().unwrap();
        for state in local.values() {
            if let Local::Ready(address) = state {
                self.ctx.forget_image(address);
            }
        }
        local.clear();
        self.decoded.lock().unwrap().clear();
        self.seq.fetch_add(1, Ordering::SeqCst);
    }
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl EmbedResolver for WorkspaceImages {
    fn state(&self, url: &str) -> EmbedState {
        let address = match self.address(url) {
            Ok(address) => address,
            Err(state) => return state,
        };
        match self
            .ctx
            .try_load_texture(&address, TextureOptions::LINEAR, SizeHint::default())
        {
            Ok(TexturePoll::Ready { texture }) => {
                let mut decoded = self.decoded.lock().unwrap();
                if !decoded.contains(&address) {
                    decoded.push(address);
                    self.seq.fetch_add(1, Ordering::SeqCst);
                }
                EmbedState::Loaded(texture.size)
            }
            Ok(TexturePoll::Pending { size }) => EmbedState::Loading(size.unwrap_or(LOADING_SIZE)),
            Err(_) => EmbedState::NotFound,
        }
    }

    fn show(&self, ui: &mut Ui, url: &str, rect: Rect, rounding: CornerRadius) {
        if let Ok(address) = self.address(url) {
            egui::Image::new(address)
                .corner_radius(rounding)
                .paint_at(ui, rect);
        }
    }

    fn prefetch(&self, url: &str) {
        if !is_remote_image(url) {
            let _ = self.address(url);
        }
    }

    fn load(&self, url: &str) {
        let ctx = self.ctx.clone();
        self.links
            .load_remote_image(url, move || ctx.request_repaint());
        self.seq.fetch_add(1, Ordering::SeqCst);
    }

    fn seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
            ^ self.links.remote_seq().rotate_left(16)
            ^ self.links.seq().rotate_left(40)
    }
}
