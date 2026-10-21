//! The images a note's editor draws: workspace images, read through
//! `app-core`'s containment checks off the UI thread, and remote images,
//! which load only when their Load image button asks. The decoders in
//! `image_decoders` draw both from bytes Noura hands egui. No loader reads
//! a file or the network on its own, and an SVG's references to other
//! files draw nothing. A newer listing of the workspace's files, as after
//! each save of the note, reads again only the images whose file changed,
//! and each keeps drawing until its new read ends.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use app_core::{ImageVersion, NoteLinks, RemoteImageState, is_remote_image};
use editor_egui::resolvers::{EmbedResolver, EmbedState};
use egui::load::{SizeHint, TexturePoll};
use egui::{CornerRadius, Rect, TextureOptions, Ui, Vec2};

/// The box an image takes while it loads, in pixels.
const LOADING_SIZE: Vec2 = Vec2::new(96.0, 96.0);

/// Where a workspace image stands.
#[derive(Clone, PartialEq, Eq)]
enum Local {
    Reading,
    /// Handed to egui under this address.
    Ready(String),
    /// Nothing to draw: no such image, or one Noura may not read.
    Missing,
}

/// What a read on another thread found, for the UI thread to hand egui.
enum Found {
    /// The image's bytes, for egui under this address.
    Image {
        address: String,
        bytes: Vec<u8>,
    },
    Missing,
}

/// One workspace image of the note.
struct Image {
    state: Local,
    /// The file the newest read or check found.
    version: Option<ImageVersion>,
    /// The listing the newest read or check started under, or `None`
    /// before the first read.
    listing: Option<u64>,
    /// A read or check runs on another thread.
    busy: bool,
    /// What a read found that egui doesn't hold yet.
    found: Option<Found>,
}

/// The workspace images of one note, by the address the note names them
/// with.
type Images = HashMap<String, Image>;

/// Loads and paints the images of one note.
pub(crate) struct WorkspaceImages {
    links: NoteLinks,
    ctx: egui::Context,
    local: Arc<Mutex<Images>>,
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
        Self {
            links,
            ctx,
            local: Arc::default(),
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
        let listing = self.links.seq();
        let mut images = self.local.lock().unwrap();
        self.hand_over(&mut images, url);
        let image = images.entry(url.to_owned()).or_insert_with(|| Image {
            state: Local::Reading,
            version: None,
            listing: None,
            busy: false,
            found: None,
        });
        if image.listing != Some(listing) && !image.busy {
            // The first read, or a check after a newer listing.
            let known = image.listing.map(|_| image.version.clone());
            image.listing = Some(listing);
            image.busy = true;
            self.read(url, known);
        }
        match &image.state {
            Local::Ready(address) => Ok(address.clone()),
            Local::Missing => Err(EmbedState::NotFound),
            Local::Reading => Err(EmbedState::Loading(LOADING_SIZE)),
        }
    }

    /// Hand egui what a read of the image at `url` found, in place of what
    /// it held before. egui forgets an address once no image of the note
    /// draws from it.
    fn hand_over(&self, images: &mut Images, url: &str) {
        let Some(found) = images.get_mut(url).and_then(|image| image.found.take()) else {
            return;
        };
        let state = match found {
            Found::Image { address, bytes } => {
                // Another image of the note may draw this version of the
                // same file already.
                let version = &images[url].version;
                let held = images.iter().any(|(other, image)| {
                    other != url
                        && image.state == Local::Ready(address.clone())
                        && image.version == *version
                });
                if !held {
                    self.ctx.forget_image(&address);
                    self.ctx.include_bytes(address.clone(), bytes);
                    self.decoded.lock().unwrap().retain(|done| *done != address);
                }
                Local::Ready(address)
            }
            Found::Missing => Local::Missing,
        };
        let Some(image) = images.get_mut(url) else {
            return;
        };
        let before = std::mem::replace(&mut image.state, state);
        if let Local::Ready(old) = before
            && !images
                .values()
                .any(|image| image.state == Local::Ready(old.clone()))
        {
            self.ctx.forget_image(&old);
            self.decoded.lock().unwrap().retain(|done| *done != old);
        }
    }

    /// Read the workspace image at `url` on another thread. When `known`
    /// holds the version an earlier read found, the thread first checks
    /// the file and reads it only when its version changed.
    fn read(&self, url: &str, known: Option<Option<ImageVersion>>) {
        let links = self.links.clone();
        let local = Arc::clone(&self.local);
        let seq = Arc::clone(&self.seq);
        let ctx = self.ctx.clone();
        let url = url.to_owned();
        std::thread::spawn(move || {
            let version = links.image_version(&url);
            let unchanged = known.is_some_and(|known| known == version);
            let read = if unchanged {
                None
            } else {
                Some(version.as_ref().and_then(|_| links.image(&url)))
            };
            let mut images = local.lock().unwrap();
            let Some(image) = images.get_mut(&url) else {
                return;
            };
            image.busy = false;
            let Some(read) = read else {
                return;
            };
            match read {
                Some(read) => {
                    image.version = Some(read.version);
                    image.found = Some(Found::Image {
                        address: format!("bytes://noura-image/{}", read.path),
                        bytes: read.bytes,
                    });
                }
                None => {
                    image.version = version;
                    image.found = Some(Found::Missing);
                }
            }
            drop(images);
            seq.fetch_add(1, Ordering::SeqCst);
            ctx.request_repaint();
        });
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
