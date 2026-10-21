//! The decoders behind the images in notes. egui_extras decodes PNG, JPEG,
//! BMP, GIF and WebP. Noura adds two of its own:
//!
//! - SVG through resvg, with every reference to another file refused.
//!   egui_extras' SVG loader keeps usvg's default options, which read any
//!   file an `<image href>` names, inside the workspace or not.
//! - AVIF through avif-decode, which decodes AV1 in Rust with rav1d. The
//!   `image` crate decodes AVIF only through the dav1d C library, so Noura
//!   registers avif-decode with `image` as a decoding hook, and
//!   egui_extras' image loader then decodes AVIF off the UI thread like the
//!   other formats.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, Once};

use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::{ColorImage, Context, Vec2};
use image::error::{DecodingError, ImageFormatHint};
use image::{ColorType, ImageDecoder, ImageError, ImageFormat, ImageResult};
use resvg::{tiny_skia, usvg};

/// The widest or tallest an SVG rasterizes, in pixels.
const MAX_SVG_SIDE: f32 = 4096.0;

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

/// usvg's options for note images. usvg's default resolver reads the file
/// that an `<image>` element's `href` names, from anywhere on disk, so this
/// one refuses every reference that isn't a data URL. Images embedded as
/// data URLs still draw.
fn svg_options() -> usvg::Options<'static> {
    let mut options = usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options
}

/// Draw the SVG in `bytes` at the size `size_hint` asks for, no larger than
/// [`MAX_SVG_SIDE`] on either side.
fn rasterize_svg(bytes: &[u8], size_hint: SizeHint) -> Result<ColorImage, String> {
    let tree = usvg::Tree::from_data(bytes, &svg_options()).map_err(|err| err.to_string())?;
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
                let result = rasterize_svg(&bytes, size_hint).map(Arc::new);
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
        // the `avif` brand.
        image::hooks::register_format_detection_hook(
            "avif".into(),
            b"\0\0\0\0ftypavif",
            Some(b"\0\0\0\0"),
        );
        image::hooks::register_decoding_hook(
            "avif".into(),
            Box::new(|mut reader| {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                // A malformed file must not take down the thread that
                // decodes images: avif-parse asserts on some in debug
                // builds.
                let image = std::panic::catch_unwind(|| decode_avif(&bytes))
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

/// A decoded AVIF image, handed to the `image` crate as a decoder.
struct AvifImage {
    width: u32,
    height: u32,
    color: ColorType,
    /// The pixels, row by row, 16-bit samples in native byte order.
    bytes: Vec<u8>,
}

fn decode_avif(bytes: &[u8]) -> Result<AvifImage, String> {
    use avif_decode::Image;

    let image = avif_decode::Decoder::from_avif(bytes)
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

    /// Load `bytes` at `address` the way a note draws an image.
    fn load(bytes: Vec<u8>, address: &str) -> Arc<ColorImage> {
        let ctx = Context::default();
        install(&ctx);
        ctx.include_bytes(address.to_owned(), bytes);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match ctx.try_load_image(address, SizeHint::default()) {
                Ok(ImagePoll::Ready { image }) => return image,
                Ok(ImagePoll::Pending { .. }) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{address} never loaded"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(err) => panic!("{address} didn't load: {err}"),
            }
        }
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
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(2, 2)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        std::fs::write(&png, bytes.into_inner()).unwrap();
        let href = png.to_str().unwrap();

        // usvg's default resolver reads the PNG. Noura's never answers.
        let default = usvg::Options::default();
        assert!((default.image_href_resolver.resolve_string)(href, &default).is_some());
        let options = svg_options();
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

    #[test]
    fn an_svg_still_draws_an_image_it_embeds() {
        let svg = svg_showing(&format!(
            "data:image/svg+xml;base64,{}",
            base64(RED_SVG.as_bytes())
        ));
        let image = load(svg, "bytes://noura-image/Garden/embedded.svg");
        assert_eq!(red_pixels(&image), 64);
    }

    #[test]
    fn an_svg_draws_at_the_size_asked_and_no_larger_than_the_limit() {
        let image = rasterize_svg(RED_SVG.as_bytes(), SizeHint::Scale(2.0.into())).unwrap();
        assert_eq!(image.size, [16, 16]);
        assert_eq!(red_pixels(&image), 256);

        let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="50000"><rect width="100000" height="50000" fill="red"/></svg>"#;
        let image = rasterize_svg(huge.as_bytes(), SizeHint::default()).unwrap();
        assert_eq!(image.size, [4096, 2048]);
    }

    /// A 4 by 4 AVIF image, red in its top two rows and blue in the other
    /// two, made with macOS's `sips`.
    const AVIF: &[u8] = include_bytes!("../tests/fixtures/pic.avif");

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
    fn a_broken_avif_image_fails_to_decode() {
        let mut broken = AVIF.to_vec();
        broken.truncate(broken.len() / 2);
        let ctx = Context::default();
        install(&ctx);
        ctx.include_bytes("bytes://noura-image/broken.avif", broken);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match ctx.try_load_image("bytes://noura-image/broken.avif", SizeHint::default()) {
                Ok(ImagePoll::Pending { .. }) => {
                    assert!(std::time::Instant::now() < deadline, "never failed");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(ImagePoll::Ready { .. }) => panic!("a broken image decoded"),
                Err(_) => break,
            }
        }
    }
}
