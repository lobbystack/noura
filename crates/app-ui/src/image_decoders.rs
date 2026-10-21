//! The decoders behind the images in notes. egui_extras decodes PNG, JPEG,
//! BMP, GIF and WebP. Noura adds two of its own:
//!
//! - SVG through resvg. Text draws in the system's fonts, and images
//!   embedded as data URLs draw: PNG, JPEG, GIF and WebP up to the `image`
//!   crate's default size limit, and SVG. A reference to any other file
//!   draws nothing. egui_extras' SVG loader keeps usvg's default options,
//!   which read any file an `<image href>` names, inside the workspace or
//!   not.
//! - AVIF through avif-decode, which decodes AV1 in Rust with rav1d. The
//!   `image` crate decodes AVIF only through the dav1d C library, so Noura
//!   registers avif-decode with `image` as a decoding hook, and
//!   egui_extras' image loader then decodes AVIF off the UI thread like the
//!   other formats. An AVIF image whose frames could decode past the
//!   `image` crate's default size limit draws nothing.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::{ColorImage, Context, Vec2};
use image::error::{DecodingError, ImageFormatHint};
use image::{ColorType, ImageDecoder, ImageError, ImageFormat, ImageResult};
use rav1d::include::dav1d::headers::{DAV1D_PIXEL_LAYOUT_I400, Dav1dSequenceHeader};
use rav1d::src::lib::dav1d_parse_sequence_header;
use resvg::tiny_skia;
use resvg::usvg::{self, ImageKind, fontdb};

/// The widest or tallest an SVG rasterizes, in pixels.
const MAX_SVG_SIDE: f32 = 4096.0;

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
    // egui asks the newest loader first, so this one answers for SVG even
    // if something installs egui_extras' SVG loader later.
    if !ctx.is_loader_installed(SvgLoader::ID) {
        ctx.add_image_loader(Arc::new(SvgLoader::default()));
    }
}

/// The fonts SVG text draws in, once listed.
static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

/// The system's fonts, listed on the first call, which waits for the
/// listing. Listing reads every font file's header, so the SVG loader asks
/// for it off the UI thread. A system without a serif font, where usvg
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
/// URL. A data URL draws when it holds an SVG, or a PNG, JPEG, GIF or WebP
/// image that [`fits`].
fn svg_options(fonts: Arc<fontdb::Database>) -> usvg::Options<'static> {
    let mut options = usvg::Options {
        fontdb: fonts,
        ..usvg::Options::default()
    };
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    let data_url = usvg::ImageHrefResolver::default_data_resolver();
    options.image_href_resolver.resolve_data =
        Box::new(move |mime, bytes, options| data_url(mime, bytes, options).filter(fits));
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
/// [`MAX_SVG_SIDE`] on either side, with its text in `fonts`.
fn rasterize_svg(
    bytes: &[u8],
    size_hint: SizeHint,
    fonts: Arc<fontdb::Database>,
) -> Result<ColorImage, String> {
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

#[derive(Default)]
struct SvgCache {
    pass: u64,
    images: HashMap<String, HashMap<SizeHint, Drawn>>,
}

/// Loads images whose address ends in `.svg` from the bytes egui holds for
/// them, through [`rasterize_svg`].
#[derive(Default)]
struct SvgLoader {
    cache: Mutex<SvgCache>,
    /// A thread waits for the system's fonts, to ask for another frame
    /// once they're listed.
    awaiting_fonts: AtomicBool,
}

impl SvgLoader {
    const ID: &'static str = egui::generate_loader_id!(SvgLoader);

    /// The fonts SVG text draws in, or `None` while another thread lists
    /// them. That thread asks `ctx` for another frame when they're ready.
    fn fonts(&self, ctx: &Context) -> Option<Arc<fontdb::Database>> {
        if let Some(fonts) = FONTS.get() {
            return Some(Arc::clone(fonts));
        }
        if !self.awaiting_fonts.swap(true, Ordering::SeqCst) {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                system_fonts();
                ctx.request_repaint();
            });
        }
        None
    }
}

fn poll(result: Result<Arc<ColorImage>, String>) -> ImageLoadResult {
    match result {
        Ok(image) => Ok(ImagePoll::Ready { image }),
        Err(err) => Err(LoadError::Loading(err)),
    }
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
                .and_then(|sizes| sizes.get_mut(&size_hint))
            {
                drawn.last_used = pass;
                return poll(drawn.result.clone());
            }
        }
        match ctx.try_load_bytes(uri)? {
            BytesPoll::Pending { size } => Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => {
                let Some(fonts) = self.fonts(ctx) else {
                    return Ok(ImagePoll::Pending { size: None });
                };
                let result = rasterize_svg(&bytes, size_hint, fonts).map(Arc::new);
                let mut cache = self.cache.lock().unwrap();
                let last_used = cache.pass;
                cache.images.entry(uri.to_owned()).or_default().insert(
                    size_hint,
                    Drawn {
                        last_used,
                        result: result.clone(),
                    },
                );
                poll(result)
            }
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().unwrap().images.remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().unwrap().images.clear();
    }

    fn end_pass(&self, pass_index: u64) {
        let mut cache = self.cache.lock().unwrap();
        cache.pass = pass_index;
        // An image drawn at several sizes, as while a window resizes, keeps
        // only the sizes the last two passes used.
        for sizes in cache.images.values_mut() {
            if sizes.len() > 1 {
                sizes.retain(|_, drawn| drawn.last_used + 1 >= pass_index);
            }
        }
        cache.images.retain(|_, sizes| !sizes.is_empty());
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .unwrap()
            .images
            .values()
            .flat_map(HashMap::values)
            .map(|drawn| match &drawn.result {
                Ok(image) => image.pixels.len() * size_of::<egui::Color32>(),
                Err(err) => err.len(),
            })
            .sum()
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

/// The type of the OBU at the start of the AV1 data `av1` and its length in
/// bytes, header included, or `None` when the data ends too soon.
fn next_obu(av1: &[u8]) -> Option<(u8, usize)> {
    let header = *av1.first()?;
    let kind = (header >> 3) & 0b1111;
    let mut length = 1 + usize::from(header & 0b100 != 0);
    // An OBU without a size runs to the end of the data.
    if header & 0b10 == 0 {
        return Some((kind, av1.len()));
    }
    let mut size = 0u64;
    for index in 0..8 {
        let byte = *av1.get(length)?;
        length += 1;
        size |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            let length = length.checked_add(usize::try_from(size).ok()?)?;
            return (length <= av1.len()).then_some((kind, length));
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

/// How large the frames in some AV1 data can be, from its sequence
/// headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameBound {
    pixels: u64,
    /// A frame may hold more than 8 bits a sample.
    high_bit_depth: bool,
    /// Every frame is grayscale.
    monochrome: bool,
}

/// How large the frames in the AV1 data `av1` can be, or `None` when it
/// holds no sequence header rav1d reads. A frame under a reduced still
/// picture header takes the sequence's largest size. Any other frame may
/// name its own size, in as many bits a side as the sequence header gives,
/// which rav1d doesn't check against the largest size.
fn frame_bound(av1: &[u8]) -> Option<FrameBound> {
    let mut bound: Option<FrameBound> = None;
    let mut rest = av1;
    while !rest.is_empty() {
        let (kind, length) = next_obu(rest)?;
        if kind == OBU_SEQUENCE_HEADER {
            let header = sequence_header(&rest[..length])?;
            let pixels = if header.reduced_still_picture_header != 0 {
                u64::try_from(header.max_width).ok()? * u64::try_from(header.max_height).ok()?
            } else {
                (1u64 << header.width_n_bits) * (1u64 << header.height_n_bits)
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
        rest = &rest[length..];
    }
    bound
}

/// Refuse an AVIF image whose frames could decode to more than
/// [`max_decoded_bytes`]. avif-decode runs rav1d with no frame size limit,
/// so a small file could otherwise make it allocate gigabytes, which no
/// panic handler can catch.
fn check_avif_size(avif: &avif_parse::AvifData) -> Result<(), String> {
    let no_header = || "the AVIF image has no AV1 sequence header".to_owned();
    let color = frame_bound(&avif.primary_item).ok_or_else(no_header)?;
    let alpha = match &avif.alpha_item {
        Some(alpha) => Some(frame_bound(alpha).ok_or_else(no_header)?),
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

    #[test]
    fn an_av1_frame_is_as_large_as_its_sequence_headers_allow() {
        let pixels = |av1: &[u8]| frame_bound(av1).map(|bound| bound.pixels);

        // A reduced header's frames all take its largest size.
        assert_eq!(pixels(&sequence_header_obu(true, 16, 1)), Some(1));
        assert_eq!(pixels(&sequence_header_obu(true, 3, 5)), Some(25));
        // A full header lets a frame name any size its bits can write.
        assert_eq!(pixels(&sequence_header_obu(false, 16, 1)), Some(1 << 32));
        // The largest header counts, and padding counts for nothing.
        let mut two = sequence_header_obu(true, 3, 5);
        two.extend(obu(15, &[0; 4]));
        two.extend(sequence_header_obu(true, 16, 40_000));
        assert_eq!(pixels(&two), Some(40_000 * 40_000));
        // Data without a sequence header, or one cut short, has no bound.
        assert_eq!(pixels(&obu(15, &[0; 4])), None);
        let header = sequence_header_obu(true, 3, 5);
        assert_eq!(pixels(&header[..header.len() - 1]), None);
    }

    #[test]
    fn an_avif_image_too_large_to_decode_fails_before_it_decodes() {
        // The fixture, with its AV1 data swapped for the sequence header of
        // a 65,536 by 65,536 image, padded to the same length: 12 GB of
        // pixels once decoded.
        let item = avif_parse::read_avif(&mut &AVIF[..])
            .unwrap()
            .primary_item
            .to_vec();
        let at = AVIF
            .windows(item.len())
            .position(|window| window == item.as_slice())
            .unwrap();
        let mut av1 = sequence_header_obu(true, 16, 65_536);
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
