//! The images a note's editor draws: workspace images, read through
//! `app-core`'s containment checks off the UI thread, and remote images,
//! which load only when their Load image button asks. The decoders in
//! `image_decoders` draw both from bytes Noura hands egui. No loader reads
//! a file or the network on its own, and an SVG's references to other
//! files draw nothing. A newer listing of the workspace's files, as after
//! each save of the note, reads again only the images whose file changed,
//! and each keeps drawing until its new read ends. Every note open in a
//! window shares the bytes egui holds for a workspace image, which egui
//! forgets once no open note draws from them. An embed of a PDF draws the
//! page its link names, which `app-core` draws on another thread, with an
//! Open in tab button over its corner.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use app_core::{
    EMBED_WIDTH, ImageVersion, NoteLinks, PdfEmbedState, RemoteImageState, is_remote_image,
};
use editor_egui::resolvers::{EmbedResolver, EmbedState};
use egui::load::{SizeHint, SizedTexture, TexturePoll};
use egui::{Button, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions, Ui, Vec2, vec2};

/// The PDFs whose embed's Open in tab button was pressed, with the page
/// each one's link names, for the note's view to open after it draws.
pub(crate) type PdfOpens = Arc<Mutex<Vec<(String, Option<u32>)>>>;

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
        version: ImageVersion,
    },
    Missing {
        version: Option<ImageVersion>,
    },
}

/// One workspace image of the note.
struct Image {
    state: Local,
    /// The file the newest read or check that the UI thread took in found.
    version: Option<ImageVersion>,
    /// The listing the newest read or check started under, or `None`
    /// before the first read.
    listing: Option<u64>,
    /// A read or check runs on another thread.
    busy: bool,
    /// What a read found that the UI thread hasn't taken in yet.
    found: Option<Found>,
}

/// The workspace images of one note, by the address the note names them
/// with.
type Images = HashMap<String, Image>;

/// The bytes egui holds at one workspace image address.
struct Held {
    /// The version of the file they came from.
    version: ImageVersion,
    /// How many images of open notes draw from them.
    users: usize,
    /// Changes each time egui gets new bytes at the address.
    generation: u64,
}

/// The workspace image bytes egui holds, shared by every note that a
/// context draws.
#[derive(Default)]
struct HeldImages {
    /// The generations handed out so far.
    generations: u64,
    addresses: HashMap<String, Held>,
}

type SharedHeld = Arc<Mutex<HeldImages>>;

/// The workspace image bytes `ctx` holds.
fn held_images(ctx: &egui::Context) -> SharedHeld {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<SharedHeld>(egui::Id::unique("noura-workspace-images"))
            .clone()
    })
}

impl HeldImages {
    /// One image stopped drawing from `address`. egui forgets the bytes
    /// there once no image of an open note draws from them.
    fn release(&mut self, ctx: &egui::Context, address: &str) {
        let Some(held) = self.addresses.get_mut(address) else {
            return;
        };
        held.users = held.users.saturating_sub(1);
        if held.users == 0 {
            self.addresses.remove(address);
            ctx.forget_image(address);
        }
    }
}

/// Loads and paints the images of one note.
pub(crate) struct WorkspaceImages {
    links: NoteLinks,
    ctx: egui::Context,
    local: Arc<Mutex<Images>>,
    held: SharedHeld,
    /// Counts the changes to what this answers.
    seq: Arc<AtomicU64>,
    /// The addresses whose texture finished decoding, with the
    /// generation of the workspace image bytes it decoded.
    decoded: Mutex<HashMap<String, Option<u64>>>,
    /// Remote images whose bytes egui holds.
    included: Mutex<Vec<String>>,
    /// The pages the note's PDF embeds drew, as uploaded, by the address
    /// the note names each embed with.
    pdf_pages: Mutex<HashMap<String, (u64, TextureHandle)>>,
    /// Where an Open in tab button puts its PDF.
    opens: PdfOpens,
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
    pub(crate) fn new(links: NoteLinks, ctx: egui::Context, opens: PdfOpens) -> Self {
        crate::image_decoders::install(&ctx);
        Self {
            links,
            held: held_images(&ctx),
            ctx,
            local: Arc::default(),
            seq: Arc::default(),
            decoded: Mutex::default(),
            included: Mutex::default(),
            pdf_pages: Mutex::default(),
            opens,
        }
    }

    /// What the PDF embed of `url` shows, with its page uploaded once it
    /// drew.
    fn pdf_state(&self, url: &str) -> EmbedState {
        let ctx = self.ctx.clone();
        match self.links.pdf_embed(url, move || ctx.request_repaint()) {
            None | Some(PdfEmbedState::Problem(_)) => EmbedState::NotFound,
            // A Letter page's shape, until the page draws.
            Some(PdfEmbedState::Drawing) => {
                EmbedState::Loading(vec2(EMBED_WIDTH, EMBED_WIDTH * 11.0 / 8.5))
            }
            Some(PdfEmbedState::Drawn(picture)) => {
                let mut pages = self.pdf_pages.lock().unwrap();
                let fresh = pages
                    .get(url)
                    .is_none_or(|(generation, _)| *generation != picture.generation);
                if fresh {
                    let size = [picture.width as usize, picture.height as usize];
                    let image = ColorImage::from_rgba_premultiplied(size, &picture.rgba);
                    let texture = self.ctx.load_texture(
                        format!("pdf-embed-{url}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    pages.insert(url.to_owned(), (picture.generation, texture));
                    self.seq.fetch_add(1, Ordering::SeqCst);
                }
                EmbedState::Loaded(vec2(picture.width as f32, picture.height as f32))
            }
        }
    }

    /// Paint the PDF embed of `url` in `rect`, with its Open in tab button.
    fn show_pdf(&self, ui: &mut Ui, url: &str, rect: Rect, rounding: CornerRadius) {
        let Some((path, page)) = self.links.pdf_link(url) else {
            return;
        };
        if let Some((_, texture)) = self.pdf_pages.lock().unwrap().get(url) {
            egui::Image::from_texture(SizedTexture::new(texture.id(), rect.size()))
                .corner_radius(rounding)
                .paint_at(ui, rect);
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let size = vec2(96.0, 24.0);
        let button = Rect::from_min_size(rect.right_top() + vec2(-size.x - 8.0, 8.0), size);
        let response = ui
            .push_id(url, |ui| ui.put(button, Button::new("Open in tab")))
            .inner
            .accessible_name(format!("Open {name} in a tab"));
        if response.clicked() {
            self.opens.lock().unwrap().push((path, page));
            self.ctx.request_repaint();
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

    /// Take in what a read of the image at `url` found. egui gets the
    /// bytes unless it holds that version of the file already, for this
    /// note or another.
    fn hand_over(&self, images: &mut Images, url: &str) {
        let Some(image) = images.get_mut(url) else {
            return;
        };
        let Some(found) = image.found.take() else {
            return;
        };
        let mut held = self.held.lock().unwrap();
        let state = match found {
            Found::Image {
                address,
                bytes,
                version,
            } => {
                let same = held
                    .addresses
                    .get(&address)
                    .is_some_and(|held| held.version == version);
                if !same {
                    // Notes that draw an older version of the file draw
                    // this one now, and read it again after their next
                    // listing.
                    self.ctx.forget_image(&address);
                    self.ctx.include_bytes(address.clone(), bytes);
                    held.generations += 1;
                    let generation = held.generations;
                    let users = held.addresses.get(&address).map_or(0, |held| held.users);
                    held.addresses.insert(
                        address.clone(),
                        Held {
                            version: version.clone(),
                            users,
                            generation,
                        },
                    );
                }
                if let Some(held) = held.addresses.get_mut(&address) {
                    held.users += 1;
                }
                image.version = Some(version);
                Local::Ready(address)
            }
            Found::Missing { version } => {
                image.version = version;
                Local::Missing
            }
        };
        if let Local::Ready(old) = std::mem::replace(&mut image.state, state) {
            held.release(&self.ctx, &old);
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
            let mut images = local.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(image) = images.get_mut(&url) else {
                return;
            };
            image.busy = false;
            let Some(read) = read else {
                return;
            };
            image.found = Some(match read {
                Some(read) => Found::Image {
                    address: format!("bytes://noura-image/{}", read.path),
                    bytes: read.bytes,
                    version: read.version,
                },
                None => Found::Missing { version },
            });
            drop(images);
            seq.fetch_add(1, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }
}

impl Drop for WorkspaceImages {
    /// A closed note stops drawing its images.
    fn drop(&mut self) {
        let images = self.local.lock().unwrap_or_else(PoisonError::into_inner);
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        for image in images.values() {
            if let Local::Ready(address) = &image.state {
                held.release(&self.ctx, address);
            }
        }
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
        if self.links.pdf_link(url).is_some() {
            return self.pdf_state(url);
        }
        let address = match self.address(url) {
            Ok(address) => address,
            Err(state) => return state,
        };
        match self
            .ctx
            .try_load_texture(&address, TextureOptions::LINEAR, SizeHint::default())
        {
            Ok(TexturePoll::Ready { texture }) => {
                // Another note may have handed egui new bytes here.
                let generation = self
                    .held
                    .lock()
                    .unwrap()
                    .addresses
                    .get(&address)
                    .map(|held| held.generation);
                let mut decoded = self.decoded.lock().unwrap();
                if decoded.get(&address) != Some(&generation) {
                    decoded.insert(address, generation);
                    self.seq.fetch_add(1, Ordering::SeqCst);
                }
                EmbedState::Loaded(texture.size)
            }
            Ok(TexturePoll::Pending { size }) => EmbedState::Loading(size.unwrap_or(LOADING_SIZE)),
            Err(_) => EmbedState::NotFound,
        }
    }

    fn show(&self, ui: &mut Ui, url: &str, rect: Rect, rounding: CornerRadius) {
        if self.links.pdf_link(url).is_some() {
            self.show_pdf(ui, url, rect, rounding);
            return;
        }
        if let Ok(address) = self.address(url) {
            egui::Image::new(address)
                .corner_radius(rounding)
                .paint_at(ui, rect);
        }
    }

    fn prefetch(&self, url: &str) {
        if self.links.pdf_link(url).is_some() {
            let _ = self.pdf_state(url);
        } else if !is_remote_image(url) {
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
            ^ self.links.pdf_embeds_seq().rotate_left(28)
            ^ self.links.seq().rotate_left(40)
    }
}
