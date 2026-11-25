//! The decoders behind the images in notes. egui_extras decodes PNG, JPEG,
//! BMP, GIF and WebP. Noura adds two of its own:
//!
//! - SVG through resvg, on a thread of its own so a costly drawing never
//!   holds up the UI. Text draws in the system's fonts, and images embedded
//!   as data URLs draw: PNG, JPEG, GIF and WebP up to the `image` crate's
//!   default size limit, and SVG. A reference to any other file draws
//!   nothing, and so does gzip-compressed SVG, which could inflate to any
//!   size. egui_extras' SVG loader keeps usvg's default options, which read
//!   any file an `<image href>` names, inside the workspace or not.
//! - AVIF through avif-decode, which decodes AV1 in Rust with rav1d. The
//!   `image` crate decodes AVIF only through the dav1d C library, so Noura
//!   registers avif-decode with `image` as a decoding hook, and
//!   egui_extras' image loader then decodes AVIF off the UI thread like the
//!   other formats. An AVIF image whose frames could decode past the
//!   `image` crate's default size limit draws nothing.
//!
//! An image shows at its own size as Preview shows it: an SVG at the size
//! it declares, and a raster image at the density its file records, so a
//! Retina screenshot shows at half its pixel size. [`natural_size`] gives
//! that size in points, for notes and the image tab alike.
//!
//! egui uploads a raster image at the size it decodes to, and the GPU
//! draws it smaller by skipping pixels, so fine lines beat into moiré.
//! wgpu also panics on a texture wider or taller than the GPU's largest
//! side, 8192 pixels in the desktop app. `ScaledTextureLoader` uploads
//! each raster image at the pixels it draws at instead, scaled down on
//! another thread, and an SVG draws at those pixels.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::mem::MaybeUninit;
use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, Once, OnceLock};

use egui::load::{
    Bytes, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint, SizedTexture,
    TextureLoadResult, TextureLoader, TexturePoll,
};
use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions, Vec2};
use image::error::{DecodingError, ImageFormatHint};
use image::{ColorType, ImageDecoder, ImageError, ImageFormat, ImageResult};
use rav1d::include::dav1d::headers::{
    DAV1D_ADAPTIVE, DAV1D_FRAME_TYPE_INTRA, DAV1D_FRAME_TYPE_KEY, DAV1D_FRAME_TYPE_SWITCH,
    DAV1D_ON, DAV1D_PIXEL_LAYOUT_I400, Dav1dFrameType, Dav1dSequenceHeader,
};
use rav1d::src::lib::dav1d_parse_sequence_header;
use resvg::tiny_skia;
use resvg::usvg::{self, ImageKind, fontdb};

/// The widest or tallest an SVG rasterizes, in pixels.
const MAX_SVG_SIDE: f32 = 4096.0;

/// The first bytes of gzip data, which usvg inflates as compressed SVG
/// when its `svgz` feature is on, with no limit on the size.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// The most bytes an image may decode to: the `image` crate's default
/// allocation limit, which also bounds the formats it decodes itself.
fn max_decoded_bytes() -> u64 {
    image::Limits::default()
        .max_alloc
        .unwrap_or(512 * 1024 * 1024)
}

/// Install the image decoders on `ctx`. Calling this again does nothing.
pub(crate) fn install(ctx: &Context) {
    register_avif();
    egui_extras::install_image_loaders(ctx);
    let measures = measures(ctx);
    // egui asks the newest loader first, so this one answers for SVG even
    // if something installs egui_extras' SVG loader later.
    if !ctx.is_loader_installed(SvgLoader::ID) {
        ctx.add_image_loader(Arc::new(SvgLoader {
            measures: Arc::clone(&measures),
            ..SvgLoader::default()
        }));
    }
    // egui asks this loader before its own.
    if !ctx.is_loader_installed(ScaledTextureLoader::ID) {
        ctx.add_texture_loader(Arc::new(ScaledTextureLoader {
            measures,
            ..ScaledTextureLoader::default()
        }));
    }
}

/// The fonts SVG text draws in, once listed.
static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

/// The system's fonts, listed on the first call, which waits for the
/// listing. Listing reads every font file's header, so only the threads
/// that draw SVGs ask for it. A system without a serif font, where usvg
/// would draw no text at all, draws SVG text in egui's own fonts.
fn system_fonts() -> Arc<fontdb::Database> {
    Arc::clone(FONTS.get_or_init(|| {
        let mut fonts = fontdb::Database::new();
        fonts.load_system_fonts();
        let serif = fontdb::Query {
            families: &[fontdb::Family::Serif],
            ..fontdb::Query::default()
        };
        if fonts.query(&serif).is_none() {
            add_egui_fonts(&mut fonts);
        }
        Arc::new(fonts)
    }))
}

/// Add egui's own fonts to `fonts` and make them the generic families:
/// its proportional font for serif, sans-serif, cursive and fantasy, and
/// its monospace font for monospace.
fn add_egui_fonts(fonts: &mut fontdb::Database) {
    let definitions = egui::FontDefinitions::default();
    let family = |fonts: &mut fontdb::Database, kind: egui::FontFamily| -> Option<String> {
        let name = definitions.families.get(&kind)?.first()?;
        let data = definitions.font_data.get(name)?;
        let id = *fonts
            .load_font_source(fontdb::Source::Binary(Arc::clone(&data.font)))
            .first()?;
        Some(fonts.face(id)?.families.first()?.0.clone())
    };
    if let Some(name) = family(fonts, egui::FontFamily::Proportional) {
        fonts.set_serif_family(name.clone());
        fonts.set_sans_serif_family(name.clone());
        fonts.set_cursive_family(name.clone());
        fonts.set_fantasy_family(name);
    }
    if let Some(name) = family(fonts, egui::FontFamily::Monospace) {
        fonts.set_monospace_family(name);
    }
}

/// usvg's options for note images, drawing text in `fonts`. usvg's default
/// resolver reads the file that an `<image>` element's `href` names, from
/// anywhere on disk, so this one refuses every reference that isn't a data
/// URL. A data URL draws when it holds an SVG that isn't gzip-compressed,
/// or a PNG, JPEG, GIF or WebP image that [`fits`].
fn svg_options(fonts: Arc<fontdb::Database>) -> usvg::Options<'static> {
    let mut options = usvg::Options {
        fontdb: fonts,
        ..usvg::Options::default()
    };
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    let data_url = usvg::ImageHrefResolver::default_data_resolver();
    options.image_href_resolver.resolve_data = Box::new(move |mime, bytes, options| {
        if bytes.starts_with(&GZIP_MAGIC) {
            return None;
        }
        data_url(mime, bytes, options).filter(fits)
    });
    options
}

/// Whether an image embedded in an SVG decodes to at most
/// [`max_decoded_bytes`]. resvg decodes PNG, JPEG, GIF and WebP with no
/// limit of its own, and an embedded SVG draws at the size of the one
/// around it.
fn fits(kind: &ImageKind) -> bool {
    let (bytes, format) = match kind {
        ImageKind::PNG(bytes) => (bytes, ImageFormat::Png),
        ImageKind::JPEG(bytes) => (bytes, ImageFormat::Jpeg),
        ImageKind::GIF(bytes) => (bytes, ImageFormat::Gif),
        ImageKind::WEBP(bytes) => (bytes, ImageFormat::WebP),
        ImageKind::SVG(_) => return true,
    };
    image::ImageReader::with_format(Cursor::new(bytes.as_slice()), format)
        .into_dimensions()
        .is_ok_and(|(width, height)| {
            u64::from(width) * u64::from(height) * 4 <= max_decoded_bytes()
        })
}

/// Draw the SVG in `bytes` at the size `size_hint` asks for, no larger than
/// [`MAX_SVG_SIDE`] on either side, with its text in `fonts`. Gzip data
/// fails before it inflates, whatever features usvg was built with.
fn rasterize_svg(
    bytes: &[u8],
    size_hint: SizeHint,
    fonts: Arc<fontdb::Database>,
) -> Result<ColorImage, String> {
    if bytes.starts_with(&GZIP_MAGIC) {
        return Err("Noura doesn't draw compressed SVG".to_owned());
    }
    let tree = usvg::Tree::from_data(bytes, &svg_options(fonts)).map_err(|err| err.to_string())?;
    let source = Vec2::new(tree.size().width(), tree.size().height());
    let mut size = match size_hint {
        SizeHint::Scale(scale) => source * scale.into_inner(),
        SizeHint::Width(width) => source * (width as f32 / source.x),
        SizeHint::Height(height) => source * (height as f32 / source.y),
        SizeHint::Size {
            width,
            height,
            maintain_aspect_ratio: true,
        } => source * (width as f32 / source.x).min(height as f32 / source.y),
        SizeHint::Size {
            width,
            height,
            maintain_aspect_ratio: false,
        } => Vec2::new(width as f32, height as f32),
    };
    let largest = size.max_elem();
    if largest > MAX_SVG_SIDE {
        size *= MAX_SVG_SIDE / largest;
    }
    let size = size.round().max(Vec2::splat(1.0));
    let (width, height) = (size.x as u32, size.y as u32);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("can't draw an SVG at {width} by {height} pixels"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(size.x / source.x, size.y / source.y),
        &mut pixmap.as_mut(),
    );
    Ok(
        ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data())
            .with_source_size(source),
    )
}

/// One drawn size of an SVG.
struct Drawn {
    /// The pass that last asked for it.
    last_used: u64,
    result: Result<Arc<ColorImage>, String>,
}

/// The drawings of one SVG address.
struct Svg {
    /// Tells these drawings from those of bytes egui held at the same
    /// address before the address was forgotten.
    epoch: u64,
    /// Each size drawn, by the size hint that asked for it.
    sizes: HashMap<SizeHint, Drawn>,
    /// The size a thread draws now.
    drawing: Option<SizeHint>,
    /// The size to draw next: the newest asked for while another drew.
    next: Option<SizeHint>,
    /// egui's texture cache holds another size's drawing in place of one
    /// still drawing, to replace once that drawing ends.
    stood_in: bool,
}

#[derive(Default)]
struct SvgCache {
    pass: u64,
    /// The epochs handed out so far.
    epochs: u64,
    images: HashMap<String, Svg>,
}

/// Loads images whose address ends in `.svg` from the bytes egui holds for
/// them. A thread per address draws one size at a time through
/// [`rasterize_svg`]. Until a size is drawn, the size of the same SVG used
/// most recently stands in for it, scaled, so a window that resizes keeps
/// its SVGs on screen.
#[derive(Default)]
struct SvgLoader {
    cache: Arc<Mutex<SvgCache>>,
    measures: Measures,
}

impl SvgLoader {
    const ID: &'static str = egui::generate_loader_id!(SvgLoader);
}

fn poll(result: Result<Arc<ColorImage>, String>) -> ImageLoadResult {
    match result {
        Ok(image) => Ok(ImagePoll::Ready { image }),
        Err(err) => Err(LoadError::Loading(err)),
    }
}

/// Draw the SVG in `bytes` at `size_hint` on another thread, then each
/// size asked for meanwhile, into the drawings of `uri` from `epoch`.
fn draw_svg(
    ctx: Context,
    cache: Arc<Mutex<SvgCache>>,
    uri: String,
    epoch: u64,
    bytes: Bytes,
    size_hint: SizeHint,
) {
    std::thread::spawn(move || {
        let mut size_hint = size_hint;
        loop {
            // A malformed SVG must not leave its address drawing forever.
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                rasterize_svg(&bytes, size_hint, system_fonts())
            }))
            .unwrap_or_else(|_| Err("the SVG renderer failed".to_owned()))
            .map(Arc::new);
            let mut cache = cache.lock().unwrap();
            let pass = cache.pass;
            // egui forgot the address while this drew, and may hold other
            // bytes there now.
            let Some(svg) = cache.images.get_mut(&uri).filter(|svg| svg.epoch == epoch) else {
                return;
            };
            svg.sizes.insert(
                size_hint,
                Drawn {
                    last_used: pass,
                    result,
                },
            );
            let stood_in = std::mem::take(&mut svg.stood_in);
            svg.drawing = svg.next.take().filter(|next| !svg.sizes.contains_key(next));
            let next = svg.drawing;
            drop(cache);
            if stood_in {
                // egui's texture cache keeps whatever drawing it got for a
                // size, so it forgets the stand-ins to ask again.
                let loaders = ctx.loaders();
                for loader in loaders.texture.lock().iter() {
                    loader.forget(&uri);
                }
            }
            ctx.request_repaint();
            match next {
                Some(next) => size_hint = next,
                None => return,
            }
        }
    });
}

impl ImageLoader for SvgLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &Context, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        if !egui::load::has_extension(uri, "svg") {
            return Err(LoadError::NotSupported);
        }
        {
            let mut cache = self.cache.lock().unwrap();
            let pass = cache.pass;
            if let Some(drawn) = cache
                .images
                .get_mut(uri)
                .and_then(|svg| svg.sizes.get_mut(&size_hint))
            {
                drawn.last_used = pass;
                return poll(drawn.result.clone());
            }
        }
        let bytes = match ctx.try_load_bytes(uri)? {
            BytesPoll::Pending { size } => return Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => bytes,
        };
        let mut cache = self.cache.lock().unwrap();
        let SvgCache {
            pass,
            epochs,
            images,
        } = &mut *cache;
        let svg = images.entry(uri.to_owned()).or_insert_with(|| {
            *epochs += 1;
            Svg {
                epoch: *epochs,
                sizes: HashMap::new(),
                drawing: None,
                next: None,
                stood_in: false,
            }
        });
        match svg.drawing {
            None => {
                svg.drawing = Some(size_hint);
                draw_svg(
                    ctx.clone(),
                    Arc::clone(&self.cache),
                    uri.to_owned(),
                    svg.epoch,
                    bytes,
                    size_hint,
                );
            }
            Some(drawing) if drawing != size_hint => svg.next = Some(size_hint),
            Some(_) => {}
        }
        let stand_in = svg
            .sizes
            .values_mut()
            .filter(|drawn| drawn.result.is_ok())
            .max_by_key(|drawn| drawn.last_used);
        match stand_in {
            Some(drawn) => {
                drawn.last_used = *pass;
                svg.stood_in = true;
                poll(drawn.result.clone())
            }
            None => Ok(ImagePoll::Pending { size: None }),
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().unwrap().images.remove(uri);
        if egui::load::has_extension(uri, "svg") {
            self.measures.lock().unwrap().remove(uri);
        }
    }

    fn forget_all(&self) {
        self.cache.lock().unwrap().images.clear();
        self.measures
            .lock()
            .unwrap()
            .retain(|uri, _| !egui::load::has_extension(uri, "svg"));
    }

    fn end_pass(&self, pass_index: u64) {
        let mut cache = self.cache.lock().unwrap();
        cache.pass = pass_index;
        // An image drawn at several sizes, as while a window resizes, keeps
        // only the sizes the last two passes used.
        for svg in cache.images.values_mut() {
            if svg.sizes.len() > 1 {
                svg.sizes
                    .retain(|_, drawn| drawn.last_used + 1 >= pass_index);
            }
        }
        cache
            .images
            .retain(|_, svg| !svg.sizes.is_empty() || svg.drawing.is_some());
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .unwrap()
            .images
            .values()
            .flat_map(|svg| svg.sizes.values())
            .map(|drawn| match &drawn.result {
                Ok(image) => image.pixels.len() * size_of::<egui::Color32>(),
                Err(err) => err.len(),
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.cache
            .lock()
            .unwrap()
            .images
            .values()
            .any(|svg| svg.drawing.is_some())
    }
}

/// What the loaders measured of an image: the size it shows at, in
/// points, and for a raster image, its pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Measure {
    points: Vec2,
    pixels: Option<[usize; 2]>,
}

/// The images measured so far, by address. A context keeps one, which
/// [`install`] sets up. An SVG's measure goes when `SvgLoader` forgets
/// its address, a raster image's when `ScaledTextureLoader` does: both
/// happen when egui forgets the image, which it does before it holds new
/// bytes at an address.
type Measures = Arc<Mutex<HashMap<String, Measure>>>;

fn measures(ctx: &Context) -> Measures {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Measures>(egui::Id::unique("noura-image-measures"))
            .clone()
    })
}

/// The size hint that measures an SVG: a drawing one pixel wide costs
/// nothing beyond reading the SVG, and holds its declared size.
const MEASURE_SVG: SizeHint = SizeHint::Size {
    width: 1,
    height: 1,
    maintain_aspect_ratio: true,
};

/// The size the image at `uri` shows at its own size, in points, once
/// it's known: an SVG's declared size, or a raster image's pixels at the
/// density its file records, as Preview shows it. `Ok(None)` while the
/// image decodes. Measuring an SVG reads it on another thread without
/// drawing it, and measuring a raster image decodes it, which drawing it
/// needs anyway.
pub(crate) fn natural_size(ctx: &Context, uri: &str) -> Result<Option<Vec2>, LoadError> {
    Ok(measure(ctx, uri)?.map(|measure| measure.points))
}

fn measure(ctx: &Context, uri: &str) -> Result<Option<Measure>, LoadError> {
    let measures = measures(ctx);
    if let Some(measure) = measures.lock().unwrap().get(uri) {
        return Ok(Some(*measure));
    }
    let svg = egui::load::has_extension(uri, "svg");
    let hint = if svg {
        MEASURE_SVG
    } else {
        SizeHint::default()
    };
    let image = match ctx.try_load_image(uri, hint)? {
        ImagePoll::Pending { .. } => return Ok(None),
        ImagePoll::Ready { image } => image,
    };
    let measure = if svg {
        Measure {
            points: image.source_size,
            pixels: None,
        }
    } else {
        let [x, y] = match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, .. }) => app_core::points_per_pixel(&bytes),
            _ => [1.0, 1.0],
        };
        let [width, height] = image.size;
        Measure {
            points: Vec2::new(width as f32 * x, height as f32 * y),
            pixels: Some(image.size),
        }
    };
    measures.lock().unwrap().insert(uri.to_owned(), measure);
    Ok(Some(measure))
}

/// The most bytes of decoded raster images `ScaledTextureLoader` leaves
/// egui's image loader holding, to scale them again without decoding
/// them. Past this, the images it used longest ago decode again when
/// they draw at a new size.
const DECODED_BUDGET: usize = 256 * 1024 * 1024;

/// Uploads each raster image at the size it draws, in pixels, so it shows
/// sharp and without moiré at any size: the GPU samples each texture at
/// one level, so an image drawn much smaller than its pixels skips some
/// of them. Its pixels scale down on a thread of their own with a
/// Lanczos filter, and an image drawn larger than its pixels uploads
/// them as they are. An image wider or taller than the GPU's largest
/// texture uploads scaled down to fit. The texture keeps the size the
/// image shows at, in points, which [`natural_size`] gives.
///
/// While a new size scales, as when a window resizes or a pinch zooms,
/// the size the image drew at last stands in for it. Each image keeps
/// only the sizes the last two passes drew, and egui's image loader keeps
/// at most [`DECODED_BUDGET`] bytes of decoded images. SVGs go to egui's
/// own texture loader, which `SvgLoader` draws for at each size, and so do
/// the frames of animated GIF and WebP images.
#[derive(Default)]
struct ScaledTextureLoader {
    measures: Measures,
    state: Mutex<Scaling>,
}

impl ScaledTextureLoader {
    const ID: &'static str = egui::generate_loader_id!(ScaledTextureLoader);
}

#[derive(Default)]
struct Scaling {
    pass: u64,
    images: HashMap<(String, TextureOptions), Scaled>,
}

/// A thread's scaling of an image: `None` until it's done, then the image,
/// or `None` if it failed.
type ScaleJob = Arc<Mutex<Option<Option<ColorImage>>>>;

/// The textures of one image.
#[derive(Default)]
struct Scaled {
    /// Each size uploaded, in pixels, with the pass that last drew it.
    textures: HashMap<[usize; 2], (TextureHandle, u64)>,
    /// The size a thread scales the image to now.
    scaling: Option<([usize; 2], ScaleJob)>,
    /// The bytes of the decoded image egui's image loader holds, and the
    /// pass that last scaled from it.
    decoded: Option<(usize, u64)>,
    /// The size that couldn't be scaled to.
    failed: Option<[usize; 2]>,
}

/// The pixels an image of `pixels` draws from at `hint`, given the size
/// it shows at, `points`: as many as it draws at, at most its own, and at
/// most `max_side` on a side.
fn target_pixels(hint: SizeHint, points: Vec2, pixels: [usize; 2], max_side: usize) -> [usize; 2] {
    let want = match hint {
        SizeHint::Scale(scale) => points * scale.into_inner(),
        SizeHint::Width(width) => points * (width as f32 / points.x),
        SizeHint::Height(height) => points * (height as f32 / points.y),
        SizeHint::Size {
            width,
            height,
            maintain_aspect_ratio: true,
        } => points * (width as f32 / points.x).min(height as f32 / points.y),
        SizeHint::Size {
            width,
            height,
            maintain_aspect_ratio: false,
        } => Vec2::new(width as f32, height as f32),
    };
    let full = Vec2::new(pixels[0] as f32, pixels[1] as f32);
    let mut size = if want.is_finite() && want.x < full.x && want.y < full.y {
        want
    } else {
        full
    };
    let largest = size.max_elem();
    if largest > max_side as f32 {
        size *= max_side as f32 / largest;
    }
    [0, 1].map(|axis| (size[axis].round() as usize).clamp(1, pixels[axis].max(1)))
}

impl ScaledTextureLoader {
    /// Start making the texture of `uri` at `size` pixels: upload the
    /// decoded image when it has that size, or scale it on another
    /// thread. Does nothing while the image decodes, until egui asks again
    /// once it's decoded.
    fn start(
        &self,
        ctx: &Context,
        uri: &str,
        options: TextureOptions,
        scaled: &mut Scaled,
        pass: u64,
        size: [usize; 2],
    ) -> Result<(), LoadError> {
        let image = match ctx.try_load_image(uri, SizeHint::default()) {
            Ok(ImagePoll::Ready { image }) => image,
            Ok(ImagePoll::Pending { .. }) => return Ok(()),
            Err(_) => return Err(LoadError::NotSupported),
        };
        scaled.decoded = Some((image.pixels.len() * size_of::<Color32>(), pass));
        if image.size == size {
            let texture = ctx.load_texture(texture_name(uri, size), image, options);
            scaled.textures.insert(size, (texture, pass));
            return Ok(());
        }
        let done: ScaleJob = Arc::default();
        let result = Arc::clone(&done);
        let waker = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("noura-image-scale".to_owned())
            .spawn(move || {
                let image =
                    std::panic::catch_unwind(AssertUnwindSafe(|| scale_image(&image, size))).ok();
                *result.lock().unwrap() = Some(image);
                waker.request_repaint();
            });
        if spawned.is_err() {
            scaled.failed = Some(size);
            return Err(LoadError::Loading(SCALE_FAILED.to_owned()));
        }
        scaled.scaling = Some((size, done));
        Ok(())
    }

    /// Let egui's image loader forget the decoded images used longest ago
    /// while those it holds take more than [`DECODED_BUDGET`] bytes.
    fn keep_decoded_within_budget(
        ctx: &Context,
        images: &mut HashMap<(String, TextureOptions), Scaled>,
    ) {
        let mut held: Vec<(u64, usize, String)> = images
            .iter()
            .filter_map(|((uri, _), scaled)| {
                scaled
                    .decoded
                    .map(|(bytes, used)| (used, bytes, uri.clone()))
            })
            .collect();
        let mut total: usize = held.iter().map(|(_, bytes, _)| bytes).sum();
        if total <= DECODED_BUDGET {
            return;
        }
        held.sort();
        let loaders = ctx.loaders();
        for (_, bytes, uri) in held {
            if total <= DECODED_BUDGET {
                break;
            }
            for loader in loaders.image.lock().iter() {
                loader.forget(&uri);
            }
            for ((address, _), scaled) in images.iter_mut() {
                if *address == uri {
                    scaled.decoded = None;
                }
            }
            total -= bytes;
        }
    }
}

fn texture_name(uri: &str, [width, height]: [usize; 2]) -> String {
    format!("{uri} {width}x{height}")
}

impl TextureLoader for ScaledTextureLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(
        &self,
        ctx: &Context,
        uri: &str,
        texture_options: TextureOptions,
        size_hint: SizeHint,
    ) -> TextureLoadResult {
        if egui::load::has_extension(uri, "svg") || egui::decode_animated_image_uri(uri).is_ok() {
            return Err(LoadError::NotSupported);
        }
        // egui's own loader says why an image can't load.
        let measure = match measure(ctx, uri) {
            Ok(Some(measure)) => measure,
            Ok(None) => return Ok(TexturePoll::Pending { size: None }),
            Err(_) => return Err(LoadError::NotSupported),
        };
        let Some(pixels) = measure.pixels else {
            return Err(LoadError::NotSupported);
        };
        if pixels.contains(&0) {
            return Err(LoadError::Loading("The image has no pixels.".to_owned()));
        }
        let max_side = ctx.input(|input| input.max_texture_side).max(1);
        let size = target_pixels(size_hint, measure.points, pixels, max_side);
        let sized = |texture: &TextureHandle| SizedTexture::new(texture.id(), measure.points);

        let mut state = self.state.lock().unwrap();
        let Scaling { pass, images } = &mut *state;
        let pass = *pass;
        let key = (uri.to_owned(), texture_options);
        let scaled = images.entry(key.clone()).or_default();
        // Take in a scaling that finished.
        let finished = scaled
            .scaling
            .as_ref()
            .and_then(|(scaled_to, done)| Some((*scaled_to, done.lock().unwrap().take()?)));
        if let Some((scaled_to, result)) = finished {
            scaled.scaling = None;
            match result {
                Some(image) => {
                    let texture =
                        ctx.load_texture(texture_name(uri, scaled_to), image, texture_options);
                    scaled.textures.insert(scaled_to, (texture, pass));
                }
                None => scaled.failed = Some(scaled_to),
            }
        }
        if let Some((texture, used)) = scaled.textures.get_mut(&size) {
            *used = pass;
            return Ok(TexturePoll::Ready {
                texture: sized(texture),
            });
        }
        if scaled.failed == Some(size) {
            return Err(LoadError::Loading(SCALE_FAILED.to_owned()));
        }
        if scaled.scaling.is_none() {
            self.start(ctx, uri, texture_options, scaled, pass, size)?;
            if let Some((texture, _)) = scaled.textures.get(&size) {
                let texture = sized(texture);
                Self::keep_decoded_within_budget(ctx, images);
                return Ok(TexturePoll::Ready { texture });
            }
        }
        // The size drawn last stands in until this one is ready.
        let stand_in = scaled
            .textures
            .values_mut()
            .max_by_key(|(_, used)| *used)
            .map(|(texture, used)| {
                *used = pass;
                sized(texture)
            });
        Self::keep_decoded_within_budget(ctx, images);
        Ok(match stand_in {
            Some(texture) => TexturePoll::Ready { texture },
            None => TexturePoll::Pending {
                size: Some(measure.points),
            },
        })
    }

    fn forget(&self, uri: &str) {
        self.state
            .lock()
            .unwrap()
            .images
            .retain(|(address, _), _| address != uri);
        // An SVG's measure goes with `SvgLoader`'s drawings, which egui's
        // texture loader forgets alone while a drawing stands in.
        if !egui::load::has_extension(uri, "svg") {
            self.measures.lock().unwrap().remove(uri);
        }
    }

    fn forget_all(&self) {
        self.state.lock().unwrap().images.clear();
        self.measures.lock().unwrap().clear();
    }

    fn end_pass(&self, pass_index: u64) {
        let mut state = self.state.lock().unwrap();
        state.pass = pass_index;
        for scaled in state.images.values_mut() {
            // An image drawn at several sizes, as while a window resizes,
            // keeps only the sizes the last two passes drew.
            if scaled.textures.len() > 1 {
                scaled
                    .textures
                    .retain(|_, (_, used)| *used + 1 >= pass_index);
            }
        }
        state.images.retain(|_, scaled| {
            !scaled.textures.is_empty() || scaled.scaling.is_some() || scaled.decoded.is_some()
        });
    }

    fn byte_size(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .images
            .values()
            .flat_map(|scaled| scaled.textures.values())
            .map(|(texture, _)| texture.byte_size())
            .sum()
    }
}

/// Why an image doesn't draw at a size.
const SCALE_FAILED: &str = "Noura couldn't scale the image.";

/// `image` scaled to `size` pixels with a Lanczos filter, as Core Image's
/// Lanczos scale does, which keeps edges sharp and averages away detail
/// finer than a pixel of the result, so a grid of 1-pixel lines turns an
/// even gray instead of beating. Halving first, while the image is more
/// than four times the size, keeps the filter's work small for a large
/// photo. egui's premultiplied alpha averages without dark fringes. The
/// image keeps the size it shows at.
fn scale_image(image: &ColorImage, size: [usize; 2]) -> ColorImage {
    let mut current: std::borrow::Cow<'_, ColorImage> = std::borrow::Cow::Borrowed(image);
    while current.size[0] >= size[0] * 4 && current.size[1] >= size[1] * 4 {
        current = std::borrow::Cow::Owned(halve(&current));
    }
    let [width, height] = current.size;
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, current.as_raw().to_vec())
        .expect("a ColorImage holds four bytes a pixel");
    let resized = image::imageops::resize(
        &buffer,
        size[0] as u32,
        size[1] as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let mut rgba = resized.into_raw();
    // The filter's negative lobes can leave a color past its alpha, which
    // premultiplied alpha draws brighter than white.
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = pixel[3];
        for channel in &mut pixel[..3] {
            *channel = (*channel).min(alpha);
        }
    }
    ColorImage::from_rgba_premultiplied(size, &rgba).with_source_size(image.source_size)
}

/// `image` at half its size, each pixel the mean of the two by two it
/// covers. An odd last row or column folds into the one before.
fn halve(image: &ColorImage) -> ColorImage {
    let [width, height] = image.size;
    let size = [(width / 2).max(1), (height / 2).max(1)];
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    for y in 0..size[1] {
        let rows = 2 * y..(2 * y + 2 + usize::from(y + 1 == size[1]) * (height % 2)).min(height);
        for x in 0..size[0] {
            let columns =
                2 * x..(2 * x + 2 + usize::from(x + 1 == size[0]) * (width % 2)).min(width);
            let mut sum = [0u32; 4];
            for row in rows.clone() {
                for pixel in &image.pixels[row * width + columns.start..row * width + columns.end] {
                    for (total, channel) in sum.iter_mut().zip(pixel.to_array()) {
                        *total += u32::from(channel);
                    }
                }
            }
            let count = (rows.len() * columns.len()) as u32;
            let [r, g, b, a] = sum.map(|total| ((total + count / 2) / count) as u8);
            pixels.push(Color32::from_rgba_premultiplied(r, g, b, a));
        }
    }
    ColorImage {
        size,
        source_size: image.source_size,
        pixels,
    }
}

/// Teach the `image` crate to recognize and decode AVIF through
/// avif-decode.
fn register_avif() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // An AVIF file starts with a box of any size, then its `ftyp` and
        // its major brand: `avif` for an image, `avis` for an image
        // sequence, or the generic HEIF brands `mif1` and `msf1`, which
        // [`mark_avif`] checks.
        for signature in [
            b"\0\0\0\0ftypavif",
            b"\0\0\0\0ftypavis",
            b"\0\0\0\0ftypmif1",
            b"\0\0\0\0ftypmsf1",
        ] {
            image::hooks::register_format_detection_hook(
                "avif".into(),
                signature,
                Some(b"\0\0\0\0"),
            );
        }
        image::hooks::register_decoding_hook(
            "avif".into(),
            Box::new(|mut reader| {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                // A malformed file must not take down the thread that
                // decodes images: avif-parse asserts on some in debug
                // builds.
                let image = std::panic::catch_unwind(move || decode_avif(bytes))
                    .unwrap_or_else(|_| Err("the AVIF decoder failed".to_owned()))
                    .map_err(|err| {
                        ImageError::Decoding(DecodingError::new(
                            ImageFormatHint::Exact(ImageFormat::Avif),
                            err,
                        ))
                    })?;
                Ok(Box::new(image))
            }),
        );
    });
}

/// Mark the file in `bytes` as AVIF for avif-parse, which reads only files
/// whose major brand is `avif`. A file whose `ftyp` box lists `avif` among
/// its other brands holds an AVIF image as its primary item whatever its
/// major brand, as an image sequence does. Anything else, such as a HEIC
/// image, fails.
fn mark_avif(bytes: &mut [u8]) -> Result<(), String> {
    let not_avif = || "the file holds no AVIF image".to_owned();
    let size = bytes
        .first_chunk::<4>()
        .map(|size| u32::from_be_bytes(*size) as usize)
        .ok_or_else(not_avif)?;
    let ftyp = bytes
        .get(..size)
        .filter(|ftyp| ftyp.len() >= 16 && &ftyp[4..8] == b"ftyp")
        .ok_or_else(not_avif)?;
    if &ftyp[8..12] == b"avif" {
        return Ok(());
    }
    if !ftyp[16..].as_chunks::<4>().0.contains(b"avif") {
        return Err(not_avif());
    }
    bytes[8..12].copy_from_slice(b"avif");
    Ok(())
}

/// The type of the OBU that holds an AV1 sequence header.
const OBU_SEQUENCE_HEADER: u8 = 1;
/// The types of the OBUs that start with a frame header.
const OBU_FRAME_HEADER: u8 = 3;
const OBU_FRAME: u8 = 6;
const OBU_REDUNDANT_FRAME_HEADER: u8 = 7;

/// One OBU of some AV1 data.
struct Obu<'a> {
    kind: u8,
    /// The whole OBU, header included.
    whole: &'a [u8],
    /// What follows the OBU's header and size.
    payload: &'a [u8],
}

/// The OBU at the start of the AV1 data `av1`, or `None` when the data
/// ends too soon.
fn next_obu(av1: &[u8]) -> Option<Obu<'_>> {
    let header = *av1.first()?;
    let kind = (header >> 3) & 0b1111;
    let mut start = 1 + usize::from(header & 0b100 != 0);
    // An OBU without a size runs to the end of the data.
    if header & 0b10 == 0 {
        return Some(Obu {
            kind,
            whole: av1,
            payload: av1.get(start..)?,
        });
    }
    let mut size = 0u64;
    for index in 0..8 {
        let byte = *av1.get(start)?;
        start += 1;
        size |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            let end = start.checked_add(usize::try_from(size).ok()?)?;
            let whole = av1.get(..end)?;
            return Some(Obu {
                kind,
                whole,
                payload: &whole[start..],
            });
        }
    }
    None
}

/// The sequence header in the OBU `obu`, as rav1d reads it.
fn sequence_header(obu: &[u8]) -> Option<Dav1dSequenceHeader> {
    let mut header = MaybeUninit::<Dav1dSequenceHeader>::uninit();
    // SAFETY: rav1d only reads the `obu.len()` bytes `obu` holds, and it
    // writes a whole header to `header` when it succeeds.
    let result = unsafe {
        dav1d_parse_sequence_header(
            Some(NonNull::from(&mut header).cast()),
            NonNull::new(obu.as_ptr().cast_mut()),
            obu.len(),
        )
    };
    // SAFETY: a result of 0 means rav1d wrote the header.
    (result.0 == 0).then(|| unsafe { header.assume_init() })
}

/// Reads the fields of an AV1 header, most significant bit first.
struct BitReader<'a> {
    bytes: &'a [u8],
    read: usize,
}

impl BitReader<'_> {
    fn bits(&mut self, count: u8) -> Option<u64> {
        let mut value = 0;
        for _ in 0..count {
            let byte = self.bytes.get(self.read / 8)?;
            value = value << 1 | u64::from(byte >> (7 - self.read % 8) & 1);
            self.read += 1;
        }
        Some(value)
    }

    fn bit(&mut self) -> Option<bool> {
        Some(self.bits(1)? == 1)
    }
}

/// How large one frame can be, from its header.
#[derive(Debug, PartialEq, Eq)]
enum FrameSize {
    /// The largest size its sequence header gives.
    Largest,
    /// This many pixels.
    Exact(u64),
    /// Any size the sequence header's bits can write.
    Any,
}

/// How large the frame whose header starts `payload` can be under the
/// sequence header `sequence`, read as rav1d reads it, or `None` when the
/// header ends too soon. A frame takes the largest size unless its header
/// overrides the size. Noura reads the size an intra frame overrides with,
/// and lets any other frame that overrides take any size, since it may
/// take a reference frame's. A frame that shows one decoded before
/// decodes nothing new.
fn frame_size(sequence: &Dav1dSequenceHeader, payload: &[u8]) -> Option<FrameSize> {
    if sequence.reduced_still_picture_header != 0 {
        return Some(FrameSize::Largest);
    }
    let mut bits = BitReader {
        bytes: payload,
        read: 0,
    };
    // show_existing_frame
    if bits.bit()? {
        return Some(FrameSize::Exact(0));
    }
    let frame_type = bits.bits(2)? as Dav1dFrameType;
    let show_frame = bits.bit()?;
    if !show_frame {
        // showable_frame
        bits.bit()?;
    } else if sequence.decoder_model_info_present != 0 && sequence.equal_picture_interval == 0 {
        // frame_presentation_time
        bits.bits(sequence.frame_presentation_delay_length)?;
    }
    let key = frame_type == DAV1D_FRAME_TYPE_KEY;
    let intra = key || frame_type == DAV1D_FRAME_TYPE_INTRA;
    let switch = frame_type == DAV1D_FRAME_TYPE_SWITCH;
    let error_resilient = (key && show_frame) || switch || bits.bit()?;
    // disable_cdf_update
    bits.bit()?;
    let screen_content = match sequence.screen_content_tools {
        DAV1D_ADAPTIVE => bits.bit()?,
        DAV1D_ON => true,
        _ => false,
    };
    if screen_content && sequence.force_integer_mv == DAV1D_ADAPTIVE {
        // force_integer_mv
        bits.bit()?;
    }
    if sequence.frame_id_numbers_present != 0 {
        // current_frame_id
        bits.bits(sequence.frame_id_n_bits)?;
    }
    let frame_size_override = switch || bits.bit()?;
    if !frame_size_override {
        return Some(FrameSize::Largest);
    }
    // Inter frames, and the timing fields of a decoder model, aren't worth
    // reading for a still image.
    if !intra || sequence.decoder_model_info_present != 0 {
        return Some(FrameSize::Any);
    }
    if sequence.order_hint != 0 {
        // order_hint
        bits.bits(sequence.order_hint_n_bits)?;
    }
    let refresh_frame_flags = if key && show_frame {
        0xff
    } else {
        bits.bits(8)?
    };
    if refresh_frame_flags != 0xff && error_resilient && sequence.order_hint != 0 {
        for _ in 0..8 {
            // ref_order_hint
            bits.bits(sequence.order_hint_n_bits)?;
        }
    }
    let width = bits.bits(sequence.width_n_bits)? + 1;
    let height = bits.bits(sequence.height_n_bits)? + 1;
    Some(FrameSize::Exact(width * height))
}

/// How large the frames in some AV1 data can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameBound {
    pixels: u64,
    /// A frame may hold more than 8 bits a sample.
    high_bit_depth: bool,
    /// Every frame is grayscale.
    monochrome: bool,
}

/// How large the frames in the AV1 data `av1` can be, or `None` when it
/// holds no frame, or data rav1d wouldn't read. `sequence` holds the
/// sequence header in force, which rav1d keeps from one piece of data to
/// the next it decodes, and the newest one here once this returns.
fn frame_bound(av1: &[u8], sequence: &mut Option<Dav1dSequenceHeader>) -> Option<FrameBound> {
    let mut bound: Option<FrameBound> = None;
    let mut rest = av1;
    while !rest.is_empty() {
        let obu = next_obu(rest)?;
        match obu.kind {
            OBU_SEQUENCE_HEADER => *sequence = Some(sequence_header(obu.whole)?),
            OBU_FRAME_HEADER | OBU_FRAME | OBU_REDUNDANT_FRAME_HEADER => {
                // rav1d refuses a frame before any sequence header.
                let header = sequence.as_ref()?;
                let largest = || -> Option<u64> {
                    Some(
                        u64::try_from(header.max_width).ok()?
                            * u64::try_from(header.max_height).ok()?,
                    )
                };
                let pixels = match frame_size(header, obu.payload) {
                    Some(FrameSize::Largest) => largest()?,
                    Some(FrameSize::Exact(pixels)) => pixels,
                    Some(FrameSize::Any) | None => {
                        (1u64 << header.width_n_bits) * (1u64 << header.height_n_bits)
                    }
                };
                let this = FrameBound {
                    pixels,
                    high_bit_depth: header.hbd != 0,
                    monochrome: header.layout == DAV1D_PIXEL_LAYOUT_I400,
                };
                bound = Some(match bound {
                    None => this,
                    Some(bound) => FrameBound {
                        pixels: bound.pixels.max(this.pixels),
                        high_bit_depth: bound.high_bit_depth || this.high_bit_depth,
                        monochrome: bound.monochrome && this.monochrome,
                    },
                });
            }
            _ => {}
        }
        rest = &rest[obu.whole.len()..];
    }
    bound
}

/// Refuse an AVIF image whose frames could decode to more than
/// [`max_decoded_bytes`]. avif-decode runs rav1d with no frame size limit,
/// so a small file could otherwise make it allocate gigabytes, which no
/// panic handler can catch.
fn check_avif_size(avif: &avif_parse::AvifData) -> Result<(), String> {
    let unreadable = || "the AVIF image holds no AV1 frame Noura can read".to_owned();
    // avif-decode decodes the color and alpha images with one decoder.
    let mut sequence = None;
    let color = frame_bound(&avif.primary_item, &mut sequence).ok_or_else(unreadable)?;
    let alpha = match &avif.alpha_item {
        Some(alpha) => Some(frame_bound(alpha, &mut sequence).ok_or_else(unreadable)?),
        None => None,
    };
    // avif-decode hands over RGB, RGBA or gray pixels, with 8 or 16 bits a
    // sample.
    let channels = match (alpha, color.monochrome) {
        (Some(_), _) => 4,
        (None, true) => 1,
        (None, false) => 3,
    };
    let sample = if color.high_bit_depth { 2 } else { 1 };
    let pixels = color.pixels.max(alpha.map_or(0, |alpha| alpha.pixels));
    if pixels.saturating_mul(channels * sample) > max_decoded_bytes() {
        return Err("the AVIF image is too large to draw".to_owned());
    }
    Ok(())
}

/// A decoded AVIF image, handed to the `image` crate as a decoder.
struct AvifImage {
    width: u32,
    height: u32,
    color: ColorType,
    /// The pixels, row by row, 16-bit samples in native byte order.
    bytes: Vec<u8>,
}

fn decode_avif(mut bytes: Vec<u8>) -> Result<AvifImage, String> {
    use avif_decode::Image;

    mark_avif(&mut bytes)?;
    let avif = avif_parse::read_avif(&mut bytes.as_slice()).map_err(|err| err.to_string())?;
    check_avif_size(&avif)?;
    drop(avif);
    let image = avif_decode::Decoder::from_avif(&bytes)
        .and_then(avif_decode::Decoder::to_image)
        .map_err(|err| err.to_string())?;
    let (width, height) = match &image {
        Image::Rgb8(img) => (img.width(), img.height()),
        Image::Rgb16(img) => (img.width(), img.height()),
        Image::Rgba8(img) => (img.width(), img.height()),
        Image::Rgba16(img) => (img.width(), img.height()),
        Image::Gray8(img) => (img.width(), img.height()),
        Image::Gray16(img) => (img.width(), img.height()),
    };
    let (color, bytes) = match image {
        Image::Rgb8(img) => (
            ColorType::Rgb8,
            img.pixels().flat_map(|p| [p.r, p.g, p.b]).collect(),
        ),
        Image::Rgba8(img) => (
            ColorType::Rgba8,
            img.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect(),
        ),
        Image::Gray8(img) => (ColorType::L8, img.pixels().map(|p| p.value()).collect()),
        Image::Rgb16(img) => (
            ColorType::Rgb16,
            img.pixels()
                .flat_map(|p| [p.r, p.g, p.b])
                .flat_map(u16::to_ne_bytes)
                .collect(),
        ),
        Image::Rgba16(img) => (
            ColorType::Rgba16,
            img.pixels()
                .flat_map(|p| [p.r, p.g, p.b, p.a])
                .flat_map(u16::to_ne_bytes)
                .collect(),
        ),
        Image::Gray16(img) => (
            ColorType::L16,
            img.pixels().flat_map(|p| p.value().to_ne_bytes()).collect(),
        ),
    };
    Ok(AvifImage {
        width: width as u32,
        height: height as u32,
        color,
        bytes,
    })
}

impl ImageDecoder for AvifImage {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn color_type(&self) -> ColorType {
        self.color
    }

    fn read_image(self, buf: &mut [u8]) -> ImageResult<()> {
        if buf.len() != self.bytes.len() {
            return Err(ImageError::Decoding(DecodingError::new(
                ImageFormatHint::Exact(ImageFormat::Avif),
                "the decoded image has the wrong size",
            )));
        }
        buf.copy_from_slice(&self.bytes);
        Ok(())
    }

    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buf)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rav1d::include::dav1d::headers::DAV1D_FRAME_TYPE_INTER;

    use super::*;

    /// A red square, 8 pixels on a side.
    const RED_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#;

    /// "Hi" in black, in the sans-serif font.
    const TEXT_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><text x="2" y="26" font-family="sans-serif" font-size="24" fill="black">Hi</text></svg>"#;

    /// An SVG, 8 pixels on a side, whose only content is the image at
    /// `href`.
    fn svg_showing(href: &str) -> Vec<u8> {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image href="{href}" xlink:href="{href}" width="8" height="8"/></svg>"#
        )
        .into_bytes()
    }

    fn red_pixels(image: &ColorImage) -> usize {
        image
            .pixels
            .iter()
            .filter(|pixel| pixel.r() > 200 && pixel.g() < 50 && pixel.a() > 200)
            .count()
    }

    fn opaque_pixels(image: &ColorImage) -> usize {
        image.pixels.iter().filter(|pixel| pixel.a() > 0).count()
    }

    /// What loading `bytes` at `address` gives, the way a note draws an
    /// image, once it stops waiting.
    fn try_load(bytes: Vec<u8>, address: &str) -> Result<Arc<ColorImage>, LoadError> {
        let ctx = Context::default();
        install(&ctx);
        ctx.include_bytes(address.to_owned(), bytes);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match ctx.try_load_image(address, SizeHint::default()) {
                Ok(ImagePoll::Ready { image }) => return Ok(image),
                Ok(ImagePoll::Pending { .. }) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{address} never loaded"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Load `bytes` at `address` the way a note draws an image.
    fn load(bytes: Vec<u8>, address: &str) -> Arc<ColorImage> {
        try_load(bytes, address).unwrap_or_else(|err| panic!("{address} didn't load: {err}"))
    }

    #[test]
    fn an_svg_draws_nothing_from_a_file_it_names() {
        let outside = tempfile::tempdir().unwrap();
        let red_svg = outside.path().join("red.svg");
        std::fs::write(&red_svg, RED_SVG).unwrap();
        let svg = svg_showing(red_svg.to_str().unwrap());

        // usvg's default options read the file and draw it red.
        let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).unwrap();
        let mut pixmap = tiny_skia::Pixmap::new(8, 8).unwrap();
        resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
        assert!(pixmap.data().chunks(4).any(|pixel| pixel[0] > 200));

        // A workspace image and a loaded remote image draw it as nothing.
        for address in [
            "bytes://noura-image/Garden/pic.svg",
            "bytes://noura-remote/1234abcd.svg",
        ] {
            let image = load(svg.clone(), address);
            assert_eq!(image.size, [8, 8], "{address}");
            assert!(
                image.pixels.iter().all(|pixel| pixel.a() == 0),
                "{address} drew the file it names"
            );
        }
    }

    #[test]
    fn an_svg_never_reads_a_png_it_names() {
        let outside = tempfile::tempdir().unwrap();
        let png = outside.path().join("private.png");
        std::fs::write(&png, png_bytes(ImageFormat::Png, 2, 2)).unwrap();
        let href = png.to_str().unwrap();

        // usvg's default resolver reads the PNG. Noura's never answers.
        let default = usvg::Options::default();
        assert!((default.image_href_resolver.resolve_string)(href, &default).is_some());
        let options = svg_options(Arc::default());
        assert!((options.image_href_resolver.resolve_string)(href, &options).is_none());
        let relative = "../private.png";
        assert!((options.image_href_resolver.resolve_string)(relative, &options).is_none());
    }

    /// `bytes` in base64.
    fn base64(bytes: &[u8]) -> String {
        const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &byte)| n | u32::from(byte) << (16 - 8 * i));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A red image, `width` by `height` pixels, in `format`.
    fn png_bytes(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(width, height, image::Rgb([255, 0, 0]))
            .write_to(&mut bytes, format)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn an_svg_still_draws_an_svg_it_embeds() {
        let svg = svg_showing(&format!(
            "data:image/svg+xml;base64,{}",
            base64(RED_SVG.as_bytes())
        ));
        let image = load(svg, "bytes://noura-image/Garden/embedded.svg");
        assert_eq!(red_pixels(&image), 64);
    }

    #[test]
    fn an_svg_draws_png_jpeg_gif_and_webp_images_it_embeds() {
        for (format, mime) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            let svg = svg_showing(&format!(
                "data:{mime};base64,{}",
                base64(&png_bytes(format, 2, 2))
            ));
            let image = load(svg, &format!("bytes://noura-image/Garden/{mime}.svg"));
            assert_eq!(red_pixels(&image), 64, "{mime}");
        }
    }

    /// The 4-byte CRC that ends a PNG chunk.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn an_svg_draws_nothing_from_an_embedded_image_too_large_to_decode() {
        // A small PNG whose header claims 20,000 pixels a side: 1.6 GB of
        // pixels once decoded.
        let mut huge = png_bytes(ImageFormat::Png, 2, 2);
        huge[16..20].copy_from_slice(&20_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&20_000u32.to_be_bytes());
        let crc = crc32(&huge[12..29]);
        huge[29..33].copy_from_slice(&crc.to_be_bytes());

        let options = svg_options(Arc::default());
        let resolve = |png: Vec<u8>| {
            (options.image_href_resolver.resolve_data)("image/png", Arc::new(png), &options)
        };
        assert!(resolve(png_bytes(ImageFormat::Png, 2, 2)).is_some());
        assert!(resolve(huge.clone()).is_none());
        let svg = svg_showing(&format!("data:image/png;base64,{}", base64(&huge)));
        let image = load(svg, "bytes://noura-image/Garden/huge.svg");
        assert_eq!(opaque_pixels(&image), 0);
    }

    #[test]
    fn an_svg_draws_its_text_in_the_systems_fonts() {
        let image = load(
            TEXT_SVG.as_bytes().to_vec(),
            "bytes://noura-image/Garden/text.svg",
        );
        assert_eq!(image.size, [64, 32]);
        assert!(opaque_pixels(&image) > 50, "the text drew nothing");
    }

    #[test]
    fn an_svg_draws_its_text_in_eguis_fonts_where_the_system_has_none() {
        let none = rasterize_svg(TEXT_SVG.as_bytes(), SizeHint::default(), Arc::default());
        assert_eq!(opaque_pixels(&none.unwrap()), 0);

        let mut fonts = fontdb::Database::new();
        add_egui_fonts(&mut fonts);
        let image = rasterize_svg(TEXT_SVG.as_bytes(), SizeHint::default(), Arc::new(fonts));
        assert!(opaque_pixels(&image.unwrap()) > 50, "the text drew nothing");
    }

    #[test]
    fn an_svg_draws_at_the_size_asked_and_no_larger_than_the_limit() {
        let fonts = Arc::<fontdb::Database>::default;
        let image =
            rasterize_svg(RED_SVG.as_bytes(), SizeHint::Scale(2.0.into()), fonts()).unwrap();
        assert_eq!(image.size, [16, 16]);
        assert_eq!(red_pixels(&image), 256);

        let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="50000"><rect width="100000" height="50000" fill="red"/></svg>"#;
        let image = rasterize_svg(huge.as_bytes(), SizeHint::default(), fonts()).unwrap();
        assert_eq!(image.size, [4096, 2048]);
    }

    /// `bytes` in the gzip format, in stored deflate blocks.
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        let blocks: Vec<&[u8]> = bytes.chunks(0xffff).collect();
        for (index, block) in blocks.iter().enumerate() {
            // BFINAL on the last block, and BTYPE 00, stored.
            out.push(u8::from(index + 1 == blocks.len()));
            let len = block.len() as u16;
            out.extend(len.to_le_bytes());
            out.extend((!len).to_le_bytes());
            out.extend_from_slice(block);
        }
        out.extend(crc32(bytes).to_le_bytes());
        out.extend((bytes.len() as u32).to_le_bytes());
        out
    }

    /// Bits written least significant first, as deflate packs them.
    #[derive(Default)]
    struct Deflate {
        bytes: Vec<u8>,
        pending: u64,
        count: u32,
    }

    impl Deflate {
        fn put(&mut self, value: u64, count: u32) {
            self.pending |= value << self.count;
            self.count += count;
            while self.count >= 8 {
                self.bytes.push(self.pending as u8);
                self.pending >>= 8;
                self.count -= 8;
            }
        }

        /// A Huffman code, which deflate packs most significant bit first.
        fn code(&mut self, code: u32, count: u32) {
            self.put(u64::from(code.reverse_bits() >> (32 - count)), count);
        }
    }

    /// Gzip data that inflates to `size` bytes of `a`, at 258 bytes for
    /// every 13 bits: one literal `a`, then copies of the longest length
    /// from one byte back, in deflate's fixed codes. Its checksum is wrong,
    /// which a decoder finds only after it inflates everything.
    fn gzip_bomb(size: usize) -> Vec<u8> {
        let mut deflate = Deflate::default();
        deflate.put(1, 1); // BFINAL
        deflate.put(1, 2); // BTYPE 01, fixed codes
        deflate.code(0x30 + u32::from(b'a'), 8);
        for _ in 0..(size - 1) / 258 {
            deflate.code(0b1100_0101, 8); // length 258
            deflate.code(0, 5); // distance 1
        }
        deflate.code(0, 7); // end of block
        deflate.put(0, 7);
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        out.extend(deflate.bytes);
        out.extend(0u32.to_le_bytes());
        out.extend((size as u32).to_le_bytes());
        out
    }

    #[test]
    fn an_svg_draws_nothing_from_gzip_data() {
        // usvg's svgz feature would draw this red square.
        let compressed = gzip(RED_SVG.as_bytes());
        assert!(rasterize_svg(&compressed, SizeHint::default(), Arc::default()).is_err());
        assert!(try_load(compressed.clone(), "bytes://noura-image/Garden/red.svg").is_err());

        // Embedded in another SVG, it draws nothing either.
        let svg = svg_showing(&format!(
            "data:image/svg+xml;base64,{}",
            base64(&compressed)
        ));
        let image = load(svg, "bytes://noura-image/Garden/embeds-gzip.svg");
        assert_eq!(opaque_pixels(&image), 0);
    }

    #[test]
    fn a_gzip_bomb_named_svg_fails_without_inflating() {
        // 6.5 MB that inflate to 1 GiB.
        let bomb = gzip_bomb(1 << 30);
        assert!(bomb.len() < 7_000_000);
        let started = Instant::now();
        assert!(rasterize_svg(&bomb, SizeHint::default(), Arc::default()).is_err());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the bomb took {:?} to refuse",
            started.elapsed()
        );
        assert!(try_load(bomb, "bytes://noura-image/Garden/bomb.svg").is_err());
    }

    /// What egui's image loaders give for `address` at `size_hint`, once
    /// `done` holds for it.
    fn wait_for(
        ctx: &Context,
        address: &str,
        size_hint: SizeHint,
        done: impl Fn(&ColorImage) -> bool,
    ) -> Arc<ColorImage> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(ImagePoll::Ready { image }) = ctx.try_load_image(address, size_hint)
                && done(&image)
            {
                return image;
            }
            assert!(Instant::now() < deadline, "{address} never drew");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn an_svg_draws_on_another_thread_and_its_last_size_stands_in_for_a_new_one() {
        let ctx = Context::default();
        install(&ctx);
        let address = "bytes://noura-image/Garden/red.svg";
        ctx.include_bytes(address, RED_SVG.as_bytes());

        // The UI thread only starts the drawing.
        assert!(matches!(
            ctx.try_load_image(address, SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));
        wait_for(&ctx, address, SizeHint::default(), |_| true);

        // While twice the size draws, the drawing at the first size stands
        // in for it, and egui's texture holds it.
        let twice = SizeHint::Scale(2.0.into());
        let texture_size =
            |ctx: &Context| match ctx.try_load_texture(address, TextureOptions::LINEAR, twice) {
                Ok(egui::load::TexturePoll::Ready { texture }) => ctx
                    .tex_manager()
                    .read()
                    .meta(texture.id)
                    .map(|meta| meta.size),
                _ => None,
            };
        assert_eq!(texture_size(&ctx), Some([8, 8]));
        let image = wait_for(&ctx, address, twice, |image| image.size != [8, 8]);
        assert_eq!(image.size, [16, 16]);
        // Once it's drawn, egui's texture holds the new drawing.
        assert_eq!(texture_size(&ctx), Some([16, 16]));
    }

    /// The texture egui gives for `address` once it's ready, and the size
    /// of the pixels it uploaded.
    fn texture_for(ctx: &Context, address: &str) -> (SizedTexture, [usize; 2]) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match ctx.try_load_texture(address, TextureOptions::LINEAR, SizeHint::default()) {
                Ok(TexturePoll::Ready { texture }) => {
                    let uploaded = ctx.tex_manager().read().meta(texture.id).unwrap().size;
                    return (texture, uploaded);
                }
                Ok(TexturePoll::Pending { .. }) => {}
                Err(err) => panic!("{address} didn't load: {err}"),
            }
            assert!(Instant::now() < deadline, "{address} never loaded");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn an_image_too_large_for_a_texture_uploads_scaled_down_at_its_own_size() {
        let ctx = Context::default();
        install(&ctx);
        let max_side = ctx.input(|input| input.max_texture_side);
        for (name, width, height, uploaded) in [
            ("wide", 3000, 10, [max_side, 7]),
            ("tall", 10, 3000, [7, max_side]),
            ("sliver", 5000, 1, [max_side, 1]),
        ] {
            let address = format!("bytes://noura-image/Garden/{name}.png");
            ctx.include_bytes(address.clone(), png_bytes(ImageFormat::Png, width, height));

            let (texture, size) = texture_for(&ctx, &address);

            assert_eq!(size, uploaded, "{name}");
            assert_eq!(
                texture.size,
                Vec2::new(width as f32, height as f32),
                "{name}"
            );
        }
        let textures = ctx.tex_manager();
        for (_, meta) in textures.read().allocated() {
            assert!(meta.size.iter().all(|side| *side <= max_side), "{meta:?}");
        }
    }

    #[test]
    fn an_image_that_fits_a_texture_uploads_at_its_own_size() {
        let ctx = Context::default();
        install(&ctx);
        let address = "bytes://noura-image/Garden/small.png";
        ctx.include_bytes(address, png_bytes(ImageFormat::Png, 40, 30));

        let (texture, size) = texture_for(&ctx, address);

        assert_eq!(size, [40, 30]);
        assert_eq!(texture.size, Vec2::new(40.0, 30.0));
    }

    #[test]
    fn halving_makes_each_pixel_the_mean_of_the_two_by_two_it_covers() {
        let [red, blue, clear] = [
            Color32::from_rgb(255, 0, 0),
            Color32::from_rgb(0, 0, 255),
            Color32::TRANSPARENT,
        ];
        let image = ColorImage::new([4, 2], vec![red, red, blue, clear, red, red, blue, clear]);

        let halved = halve(&image);

        assert_eq!(halved.size, [2, 1]);
        assert_eq!(halved.source_size, Vec2::new(4.0, 2.0));
        assert_eq!(
            halved.pixels,
            [red, Color32::from_rgba_premultiplied(0, 0, 128, 128)]
        );
    }

    /// Black and white columns a pixel wide, `width` by `height`.
    fn grating(width: usize, height: usize) -> ColorImage {
        let pixels = (0..width * height)
            .map(|index| {
                if (index % width).is_multiple_of(2) {
                    Color32::BLACK
                } else {
                    Color32::WHITE
                }
            })
            .collect();
        ColorImage::new([width, height], pixels)
    }

    /// How far apart the darkest and lightest columns of `image` are, in
    /// levels of gray, away from its edges, which the filter reaches past.
    fn column_spread(image: &ColorImage) -> u32 {
        let [width, height] = image.size;
        let columns: Vec<u32> = (4..width - 4)
            .map(|x| {
                let sum: u32 = (0..height)
                    .map(|y| u32::from(image.pixels[y * width + x].r()))
                    .sum();
                sum / height as u32
            })
            .collect();
        columns.iter().max().unwrap() - columns.iter().min().unwrap()
    }

    /// What the GPU draws from `image` at `width` pixels wide: the two
    /// texels nearest each pixel's center, blended.
    fn sampled(image: &ColorImage, width: usize) -> ColorImage {
        let [from, height] = image.size;
        let pixels =
            (0..width * height)
                .map(|index| {
                    let (x, y) = (index % width, index / width);
                    let at = ((x as f32 + 0.5) * from as f32 / width as f32 - 0.5).max(0.0);
                    let (left, part) = (at.floor() as usize, at.fract());
                    let gray = |x: usize| f32::from(image.pixels[y * from + x.min(from - 1)].r());
                    Color32::from_gray(
                        (gray(left) * (1.0 - part) + gray(left + 1) * part).round() as u8
                    )
                })
                .collect();
        ColorImage::new([width, height], pixels)
    }

    #[test]
    fn one_pixel_lines_scale_down_to_an_even_gray_without_moire() {
        let lines = grating(1200, 8);
        // From 1.5 to 4 times smaller: the GPU, sampling the whole image,
        // beats between black and white; scaled on the CPU, each column
        // is the same gray.
        for width in [800, 700, 599, 401, 299] {
            let gpu = column_spread(&sampled(&lines, width));
            assert!(
                gpu > 100,
                "the GPU drew {width} wide with columns {gpu} apart"
            );
            let scaled = scale_image(&lines, [width, 8]);
            assert_eq!(scaled.size, [width, 8]);
            assert_eq!(scaled.source_size, Vec2::new(1200.0, 8.0));
            let spread = column_spread(&scaled);
            assert!(spread <= 8, "{width} wide: columns {spread} apart");
        }
        // Halving first, from far larger, too.
        let scaled = scale_image(&grating(4800, 32), [700, 4]);
        assert!(column_spread(&scaled) <= 8, "{}", column_spread(&scaled));
    }

    #[test]
    fn scaling_keeps_each_color_within_its_alpha() {
        let half_red = Color32::from_rgba_premultiplied(128, 0, 0, 128);
        let mut pixels = vec![Color32::TRANSPARENT; 64 * 4];
        for row in pixels.chunks_mut(64) {
            row[30..34].fill(Color32::WHITE);
            row[40..44].fill(half_red);
        }
        let scaled = scale_image(&ColorImage::new([64, 4], pixels), [24, 2]);
        for pixel in &scaled.pixels {
            assert!(
                pixel.r() <= pixel.a() && pixel.g() <= pixel.a() && pixel.b() <= pixel.a(),
                "{pixel:?}"
            );
        }
    }

    /// `png` with a `pHYs` chunk of `per_meter` pixels a meter on each
    /// side, after its header.
    fn with_density(png: &[u8], per_meter: u32) -> Vec<u8> {
        let mut chunk = b"pHYs".to_vec();
        chunk.extend(per_meter.to_be_bytes());
        chunk.extend(per_meter.to_be_bytes());
        chunk.push(1);
        let mut out = png[..33].to_vec();
        out.extend(9u32.to_be_bytes());
        out.extend(&chunk);
        out.extend(crc32(&chunk).to_be_bytes());
        out.extend(&png[33..]);
        out
    }

    /// Run a pass of `ctx` that does `work`, as a window's frame does.
    fn pass(ctx: &Context, work: impl FnMut(&Context)) {
        let mut work = work;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| work(ui.ctx()));
        // No renderer takes the textures in.
        output.textures_delta.clear();
    }

    /// Run passes of `ctx` until `done` gives something.
    fn run_until<T>(ctx: &Context, what: &str, mut done: impl FnMut(&Context) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let mut found = None;
            pass(ctx, |ctx| found = done(ctx));
            if let Some(found) = found {
                return found;
            }
            assert!(Instant::now() < deadline, "{what} never happened");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The texture of `address` at `width` by `height` pixels once it's
    /// ready at that size, and what it shows at, in points.
    fn texture_at(
        ctx: &Context,
        address: &str,
        width: u32,
        height: u32,
    ) -> (SizedTexture, [usize; 2]) {
        let hint = SizeHint::Size {
            width,
            height,
            maintain_aspect_ratio: false,
        };
        run_until(ctx, "the texture at its size", |ctx| {
            match ctx.try_load_texture(address, TextureOptions::LINEAR, hint) {
                Ok(TexturePoll::Ready { texture }) => {
                    let uploaded = ctx.tex_manager().read().meta(texture.id).unwrap().size;
                    let wanted = [width as usize, height as usize];
                    (uploaded == wanted || uploaded[0] < wanted[0]).then_some((texture, uploaded))
                }
                _ => None,
            }
        })
    }

    #[test]
    fn a_raster_image_shows_at_its_density_and_uploads_the_pixels_it_draws_at() {
        let ctx = Context::default();
        install(&ctx);
        // 400 by 300 pixels saved at 144 dpi, as a Retina screenshot is.
        let address = "bytes://noura-image/Garden/shot.png";
        ctx.include_bytes(
            address,
            with_density(&png_bytes(ImageFormat::Png, 400, 300), 5669),
        );
        let size = run_until(&ctx, "the image measures", |ctx| {
            natural_size(ctx, address).unwrap()
        });
        assert_eq!(size, Vec2::new(200.0, 150.0));

        // Drawn 100 by 75 pixels, it uploads 100 by 75.
        let (texture, uploaded) = texture_at(&ctx, address, 100, 75);
        assert_eq!(uploaded, [100, 75]);
        assert_eq!(texture.size, Vec2::new(200.0, 150.0));
        // Drawn larger than its pixels, it uploads them as they are.
        let (_, uploaded) = texture_at(&ctx, address, 800, 600);
        assert_eq!(uploaded, [400, 300]);

        // Every size it drew at but the last two passes' goes.
        for width in [120, 160, 240] {
            texture_at(&ctx, address, width, width * 3 / 4);
        }
        let textures = ctx.tex_manager();
        let held: Vec<_> = textures
            .read()
            .allocated()
            .filter(|(_, meta)| meta.name.starts_with(address))
            .map(|(_, meta)| meta.size)
            .collect();
        assert!(held.len() <= 2, "{held:?}");
    }

    #[test]
    fn while_a_new_size_scales_the_last_one_stands_in() {
        let ctx = Context::default();
        install(&ctx);
        let address = "bytes://noura-image/Garden/photo.png";
        ctx.include_bytes(address, png_bytes(ImageFormat::Png, 300, 200));
        let (first, _) = texture_at(&ctx, address, 150, 100);

        let hint = SizeHint::Size {
            width: 90,
            height: 60,
            maintain_aspect_ratio: false,
        };
        // The first ask for the new size starts its scaling and gets the
        // old size's texture meanwhile.
        let mut asked = None;
        pass(&ctx, |ctx| {
            asked = Some(ctx.try_load_texture(address, TextureOptions::LINEAR, hint));
        });
        match asked.unwrap() {
            Ok(TexturePoll::Ready { texture }) => {
                assert_eq!(texture.id, first.id);
                assert_eq!(texture.size, Vec2::new(300.0, 200.0));
            }
            Ok(TexturePoll::Pending { .. }) => panic!("nothing stood in"),
            Err(err) => panic!("the image didn't load: {err}"),
        }
        let (_, uploaded) = texture_at(&ctx, address, 90, 60);
        assert_eq!(uploaded, [90, 60]);
    }

    #[test]
    fn an_svg_measures_its_declared_size_without_drawing_it() {
        let ctx = Context::default();
        install(&ctx);
        let address = "bytes://noura-image/Garden/wide.svg";
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300"><rect width="400" height="300" fill="red"/></svg>"#;
        ctx.include_bytes(address, svg.as_bytes());
        let size = run_until(&ctx, "the SVG measures", |ctx| {
            natural_size(ctx, address).unwrap()
        });
        assert_eq!(size, Vec2::new(400.0, 300.0));
        let drawn: usize = ctx
            .loaders()
            .image
            .lock()
            .iter()
            .map(|loader| loader.byte_size())
            .sum();
        assert!(drawn < 64, "measuring drew {drawn} bytes");

        // New bytes at the address measure again.
        ctx.forget_image(address);
        ctx.include_bytes(address, svg.replace("400", "40").into_bytes());
        let size = run_until(&ctx, "the new SVG measures", |ctx| {
            natural_size(ctx, address).unwrap()
        });
        assert_eq!(size, Vec2::new(40.0, 300.0));
    }

    #[test]
    fn an_svg_forgotten_while_it_draws_draws_the_bytes_egui_holds_now() {
        let ctx = Context::default();
        install(&ctx);
        let address = "bytes://noura-image/Garden/square.svg";
        ctx.include_bytes(address, RED_SVG.as_bytes());
        assert!(matches!(
            ctx.try_load_image(address, SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));

        // The file changed to a blue square while the red one drew.
        ctx.forget_image(address);
        ctx.include_bytes(address, RED_SVG.replace("red", "blue").into_bytes());
        let image = wait_for(&ctx, address, SizeHint::default(), |_| true);
        assert_eq!(red_pixels(&image), 0);
        assert!(image.pixels.iter().all(|pixel| pixel.b() > 200));
    }

    /// A 4 by 4 AVIF image, red in its top two rows and blue in the other
    /// two, made with macOS's `sips`. Its major brand is `avif`.
    const AVIF: &[u8] = include_bytes!("../tests/fixtures/pic.avif");

    /// A red 16 by 16 AVIF image made with libheif's `heif-enc`, with its
    /// major brand changed from `avif` to the generic HEIF `mif1`. `avif`
    /// stays among its other brands.
    const MIF1_AVIF: &[u8] = include_bytes!("../tests/fixtures/pic-mif1.avif");

    /// A blue 16 by 16 AVIF image sequence of two frames, made with
    /// FFmpeg and SVT-AV1. Its major brand is `avis`, and its first frame is
    /// also its primary image.
    const AVIS_AVIF: &[u8] = include_bytes!("../tests/fixtures/pic-avis.avif");

    /// A red 16 by 16 AVIF image whose AV1 data libaom's `aomenc` wrote
    /// with a full sequence header, `--full-still-picture-hdr`, that allows
    /// frames up to 16,384 pixels a side, `--forced_max_frame_width` and
    /// `--forced_max_frame_height`, written in 14 bits a side. Its frame
    /// names its own size. The data stands in for the image in
    /// `pic-mif1.avif`, whose major brand it keeps.
    const WIDE_HEADER_AVIF: &[u8] = include_bytes!("../tests/fixtures/pic-wide-header.avif");

    #[test]
    fn an_avif_image_decodes() {
        let image = load(AVIF.to_vec(), "bytes://noura-image/Garden/pic.avif");
        assert_eq!(image.size, [4, 4]);
        let top = image.pixels[0];
        assert!(top.r() > 200 && top.b() < 60, "top-left is {top:?}");
        let bottom = image.pixels[3 * 4];
        assert!(
            bottom.b() > 200 && bottom.r() < 60,
            "bottom-left is {bottom:?}"
        );
    }

    #[test]
    fn avif_images_with_the_mif1_or_avis_major_brand_decode() {
        let mif1 = load(MIF1_AVIF.to_vec(), "bytes://noura-image/Garden/mif1.avif");
        assert_eq!(mif1.size, [16, 16]);
        let red = mif1.pixels[0];
        assert!(red.r() > 200 && red.b() < 60, "mif1 is {red:?}");

        let avis = load(AVIS_AVIF.to_vec(), "bytes://noura-image/Garden/avis.avif");
        assert_eq!(avis.size, [16, 16]);
        let blue = avis.pixels[0];
        assert!(blue.b() > 200 && blue.r() < 60, "avis is {blue:?}");
    }

    #[test]
    fn a_heif_image_without_the_avif_brand_fails_to_decode() {
        let mut heic = MIF1_AVIF.to_vec();
        let brand = heic[16..28]
            .as_chunks::<4>()
            .0
            .iter()
            .position(|brand| brand == b"avif")
            .unwrap();
        heic[16 + 4 * brand..20 + 4 * brand].copy_from_slice(b"heic");

        assert_eq!(
            decode_avif(heic.clone()).err().as_deref(),
            Some("the file holds no AVIF image")
        );
        assert!(try_load(heic, "bytes://noura-image/Garden/photo.avif").is_err());
    }

    #[test]
    fn a_broken_avif_image_fails_to_decode() {
        let mut broken = AVIF.to_vec();
        broken.truncate(broken.len() / 2);
        assert!(try_load(broken, "bytes://noura-image/broken.avif").is_err());
    }

    /// Bits written most significant first, as AV1 headers hold them.
    #[derive(Default)]
    struct Bits {
        bytes: Vec<u8>,
        written: usize,
    }

    impl Bits {
        fn put(&mut self, value: u64, count: u32) {
            for index in (0..count).rev() {
                if self.written.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                if value >> index & 1 == 1 {
                    *self.bytes.last_mut().unwrap() |= 0x80 >> (self.written % 8);
                }
                self.written += 1;
            }
        }
    }

    /// An OBU of `kind` holding `payload`.
    fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut obu = vec![kind << 3 | 0b10];
        let mut size = payload.len();
        loop {
            let byte = (size & 0x7f) as u8;
            size >>= 7;
            if size == 0 {
                obu.push(byte);
                break;
            }
            obu.push(byte | 0x80);
        }
        obu.extend_from_slice(payload);
        obu
    }

    /// The sequence header OBU of an 8-bit 4:2:0 AV1 still image whose
    /// largest frame is `largest` pixels a side, written in `bits` bits.
    /// A reduced header makes every frame that size, and a full one lets
    /// each frame name its own.
    fn sequence_header_obu(reduced: bool, bits: u32, largest: u64) -> Vec<u8> {
        let mut header = Bits::default();
        header.put(0, 3); // seq_profile
        header.put(1, 1); // still_picture
        header.put(u64::from(reduced), 1); // reduced_still_picture_header
        if reduced {
            header.put(0, 5); // seq_level_idx
        } else {
            header.put(0, 1); // timing_info_present_flag
            header.put(0, 1); // initial_display_delay_present_flag
            header.put(0, 5); // operating_points_cnt_minus_1
            header.put(0, 12); // operating_point_idc
            header.put(0, 5); // seq_level_idx
        }
        header.put(u64::from(bits - 1), 4); // frame_width_bits_minus_1
        header.put(u64::from(bits - 1), 4); // frame_height_bits_minus_1
        header.put(largest - 1, bits); // max_frame_width_minus_1
        header.put(largest - 1, bits); // max_frame_height_minus_1
        if !reduced {
            header.put(0, 1); // frame_id_numbers_present_flag
        }
        header.put(0, 3); // 128x128 superblocks, filter intra, intra edge filter
        if !reduced {
            header.put(0, 5); // interintra, masked compound, warped motion, dual filter, order hint
            header.put(1, 1); // seq_choose_screen_content_tools
            header.put(1, 1); // seq_choose_integer_mv
        }
        header.put(0, 3); // superres, CDEF, loop restoration
        header.put(0, 1); // high_bitdepth
        header.put(0, 1); // mono_chrome
        header.put(0, 1); // color_description_present_flag
        header.put(0, 1); // color_range
        header.put(0, 2); // chroma_sample_position
        header.put(0, 1); // separate_uv_delta_q
        header.put(0, 1); // film_grain_params_present
        header.put(1, 1); // trailing_one_bit
        obu(OBU_SEQUENCE_HEADER, &header.bytes)
    }

    /// The frame header OBU of a frame of `frame_type` under a full
    /// sequence header from [`sequence_header_obu`], whose sizes take
    /// `bits` bits. With `size`, the frame names its own size.
    fn frame_header_obu(
        frame_type: Dav1dFrameType,
        size: Option<(u64, u64)>,
        bits: u32,
    ) -> Vec<u8> {
        let mut header = Bits::default();
        header.put(0, 1); // show_existing_frame
        header.put(u64::from(frame_type), 2); // frame_type
        header.put(1, 1); // show_frame
        if frame_type != DAV1D_FRAME_TYPE_KEY {
            header.put(0, 1); // error_resilient_mode
        }
        header.put(0, 1); // disable_cdf_update
        header.put(0, 1); // allow_screen_content_tools
        header.put(u64::from(size.is_some()), 1); // frame_size_override_flag
        if let Some((width, height)) = size {
            if frame_type == DAV1D_FRAME_TYPE_INTRA {
                header.put(1, 8); // refresh_frame_flags
            }
            header.put(width - 1, bits); // frame_width_minus_1
            header.put(height - 1, bits); // frame_height_minus_1
        }
        header.put(1, 1); // the rest of the header, which Noura doesn't read
        obu(OBU_FRAME_HEADER, &header.bytes)
    }

    /// The AV1 data of `obus` one after another.
    fn av1(obus: &[Vec<u8>]) -> Vec<u8> {
        obus.concat()
    }

    #[test]
    fn an_av1_frame_is_as_large_as_its_headers_allow() {
        let pixels = |av1: &[u8]| frame_bound(av1, &mut None).map(|bound| bound.pixels);
        let key = |size| frame_header_obu(DAV1D_FRAME_TYPE_KEY, size, 16);

        // A reduced header's frames all take its largest size.
        assert_eq!(
            pixels(&av1(&[sequence_header_obu(true, 16, 1), key(None)])),
            Some(1)
        );
        assert_eq!(
            pixels(&av1(&[sequence_header_obu(true, 3, 5), key(None)])),
            Some(25)
        );
        // Under a full header, a frame takes the largest size unless it
        // names its own. An intra frame's own size counts, however many
        // bits the header writes sizes in.
        let full = sequence_header_obu(false, 16, 5);
        assert_eq!(pixels(&av1(&[full.clone(), key(None)])), Some(25));
        assert_eq!(
            pixels(&av1(&[full.clone(), key(Some((40, 30)))])),
            Some(1200)
        );
        let intra = frame_header_obu(DAV1D_FRAME_TYPE_INTRA, Some((40, 30)), 16);
        assert_eq!(pixels(&av1(&[full.clone(), intra])), Some(1200));
        // An inter frame that names its size, or a frame header cut short,
        // may take any size the header's bits can write.
        let inter = frame_header_obu(DAV1D_FRAME_TYPE_INTER, Some((1, 1)), 16);
        assert_eq!(
            pixels(&av1(&[full.clone(), key(None), inter.clone()])),
            Some(1 << 32)
        );
        let cut = obu(OBU_FRAME_HEADER, &key(Some((40, 30)))[2..3]);
        assert_eq!(pixels(&av1(&[full.clone(), cut])), Some(1 << 32));
        // The largest frame counts, and padding counts for nothing.
        let two = av1(&[
            sequence_header_obu(true, 3, 5),
            key(None),
            obu(15, &[0; 4]),
            sequence_header_obu(true, 16, 40_000),
            key(None),
        ]);
        assert_eq!(pixels(&two), Some(40_000 * 40_000));
        // A frame takes the sequence header the decoder read last, from
        // data it decoded before.
        let mut sequence = None;
        assert!(frame_bound(&av1(&[full.clone(), key(None)]), &mut sequence).is_some());
        assert_eq!(
            frame_bound(&inter, &mut sequence).map(|bound| bound.pixels),
            Some(1 << 32)
        );
        // Data without a frame, a frame without a sequence header, and a
        // sequence header cut short have no bound.
        assert_eq!(pixels(&full), None);
        assert_eq!(pixels(&obu(15, &[0; 4])), None);
        assert_eq!(pixels(&key(None)), None);
        let header = sequence_header_obu(true, 3, 5);
        assert_eq!(
            pixels(&av1(&[header[..header.len() - 1].to_vec(), key(None)])),
            None
        );
    }

    #[test]
    fn an_avif_image_whose_frame_names_its_size_decodes() {
        // The sequence header allows 16,384 by 16,384 frames, 768 MB of
        // pixels once decoded, and lets a frame name any size up to that.
        // The one frame names 16 by 16.
        let mut bytes = WIDE_HEADER_AVIF.to_vec();
        mark_avif(&mut bytes).unwrap();
        let avif = avif_parse::read_avif(&mut bytes.as_slice()).unwrap();
        let mut sequence = None;
        let bound = frame_bound(&avif.primary_item, &mut sequence).unwrap();
        let sequence = sequence.unwrap();
        assert_eq!(sequence.reduced_still_picture_header, 0);
        assert_eq!((sequence.width_n_bits, sequence.height_n_bits), (14, 14));
        assert_eq!((sequence.max_width, sequence.max_height), (16_384, 16_384));
        assert_eq!(bound.pixels, 16 * 16);

        let image = load(
            WIDE_HEADER_AVIF.to_vec(),
            "bytes://noura-image/Garden/wide.avif",
        );
        assert_eq!(image.size, [16, 16]);
        let red = image.pixels[0];
        assert!(red.r() > 200 && red.b() < 60, "the image is {red:?}");
    }

    #[test]
    fn an_avif_image_too_large_to_decode_fails_before_it_decodes() {
        // The fixture, with its AV1 data swapped for the sequence header of
        // a 65,536 by 65,536 image and a frame of that size, padded to the
        // same length: 12 GB of pixels once decoded.
        let item = avif_parse::read_avif(&mut &AVIF[..])
            .unwrap()
            .primary_item
            .to_vec();
        let at = AVIF
            .windows(item.len())
            .position(|window| window == item.as_slice())
            .unwrap();
        let mut av1 = av1(&[
            sequence_header_obu(true, 16, 65_536),
            frame_header_obu(DAV1D_FRAME_TYPE_KEY, None, 16),
        ]);
        let padding = item.len() - av1.len() - 2;
        assert!(padding < 128, "the padding needs a longer size");
        av1.extend(obu(15, &vec![0; padding]));
        let mut huge = AVIF.to_vec();
        huge[at..at + item.len()].copy_from_slice(&av1);

        assert!(decode_avif(AVIF.to_vec()).is_ok());
        assert_eq!(
            decode_avif(huge.clone()).err().as_deref(),
            Some("the AVIF image is too large to draw")
        );
        assert!(try_load(huge, "bytes://noura-image/Garden/huge.avif").is_err());
    }
}
