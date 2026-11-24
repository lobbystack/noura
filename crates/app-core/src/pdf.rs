//! PDF files in tabs and in notes. hayro parses a PDF, unlocks it with its
//! password and draws its pages; a device of Noura's own collects the text
//! of each page for search. Each PDF tab has a thread of its own that holds
//! the parsed document, so opening, drawing, searching and unlocking never
//! hold up the thread that draws the window. Bytes come through
//! `local-core`'s checks. Noura logs, saves and shows a password nowhere,
//! and wipes its own copy once hayro has tried it. Copies that hayro makes
//! while it tries one, or that a client's text field leaves behind, are
//! freed without being wiped.
//!
//! A tab lays its pages out one under the other, and its client scrolls
//! through them and says which part shows. The tab draws the pages near
//! that part in tiles of at most [`TILE_WIDTH`] by [`TILE_HEIGHT`] pixels,
//! at the pixels per point of the client's screen, and forgets the tiles
//! far from it. A page of any height draws as sharp as a short one, and a
//! PDF of any length holds about as many tiles as fill a few screens.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::{Range, RangeInclusive},
    panic::AssertUnwindSafe,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    time::SystemTime,
};

use hayro::{
    PixmapSettings, RenderCache, RenderSettings,
    hayro_interpret::{
        BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
        InterpreterCache, InterpreterSettings, SoftMask, TransformExt,
        font::{Glyph, GlyphRun},
        hayro_cmap::BfString,
        hayro_syntax::{DecryptionError, LoadPdfError, Pdf, page::Page},
        interpret_page,
    },
    kurbo::{Affine, BezPath, Point, Rect},
    vello_cpu::{self, Pixmap, RenderContext, color::palette::css::WHITE},
};
use local_core::WorkspaceEngine;
use zeroize::Zeroizing;

use crate::{AppError, error::Access};

/// The largest PDF Noura opens, in bytes. hayro reads the whole file.
pub const MAX_PDF_BYTES: u64 = 512 * 1024 * 1024;

/// The zoom levels Zoom in and Zoom out step through, as scales of the
/// page's own size.
pub const ZOOM_STEPS: [f32; 16] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0,
];

/// How wide a thumbnail draws a page, in points.
pub const THUMBNAIL_WIDTH: f32 = 100.0;

/// How tall a thumbnail draws a page at most, in points. A taller page's
/// thumbnail narrows to keep its shape.
pub const THUMBNAIL_MAX_HEIGHT: f32 = 2.0 * THUMBNAIL_WIDTH;

/// The space above each page of a tab and below the last, in points.
pub const PAGE_GAP: f32 = 12.0;

/// The widest a tile of a page is, in pixels. A page wider than this at
/// its zoom draws in columns of tiles.
pub const TILE_WIDTH: u32 = 2048;

/// The tallest a tile of a page is, in pixels.
pub const TILE_HEIGHT: u32 = 1024;

/// How many tiles, one under the other, a tab's thread draws in one pass,
/// which reads the page once for all of them.
const TILES_PER_PASS: usize = 4;

/// How far past each side of the view a tab draws its pages before they
/// show, in heights and widths of the view.
const DRAW_AHEAD: f32 = 1.0;

/// How far past each side of the view a tab keeps the tiles it drew, in
/// heights and widths of the view.
const KEEP_AHEAD: f32 = 2.0;

/// The most bytes of tiles a tab keeps beside the tiles that show.
pub const MAX_TILE_BYTES: usize = 256 * 1024 * 1024;

/// The most tiles a tab asks its thread for at once.
const MAX_ASKED_TILES: usize = 256;

/// The most thumbnails a tab keeps.
pub const MAX_THUMBNAILS: usize = 240;

/// The widest or tallest Noura draws an embed's page or a thumbnail, in
/// pixels. A page zoomed past it draws at this size and stretches.
const MAX_SIDE: f32 = 4096.0;

/// The most pixels a tab draws a page on a side, at any zoom.
const MAX_PAGE_PIXELS: f64 = 4_194_304.0;

/// The message for a PDF with no pages.
const NO_PAGES: &str = "This PDF has no pages.";

/// How wide an embed in a note draws its page, in pixels.
pub const EMBED_WIDTH: f32 = 1200.0;

/// How a PDF tab stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfState {
    /// The PDF is being read on its own thread.
    Opening,
    /// The PDF asks for its password. `wrong` says the last one you gave
    /// didn't open it.
    Locked {
        wrong: bool,
    },
    /// The password you gave is being tried.
    Unlocking,
    Ready,
    /// The PDF couldn't open, for the reason `message` gives.
    Failed {
        message: String,
    },
}

/// How large a PDF tab draws its pages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PdfZoom {
    /// The widest page as wide as the tab.
    FitWidth,
    /// At this scale of the page's own size, one of [`ZOOM_STEPS`].
    Scale(f32),
}

/// A page, or a part of one, drawn to pixels on a white background.
#[derive(Clone)]
pub struct PdfPicture {
    /// The page's number, from 1.
    pub page: usize,
    pub width: u32,
    pub height: u32,
    /// The pixels, row by row, as premultiplied RGBA. Every pixel is
    /// opaque.
    pub rgba: Arc<[u8]>,
    /// Differs for every picture Noura draws, so a client knows when to
    /// upload one again.
    pub generation: u64,
}

/// Two pictures are the same when they are one drawing.
impl PartialEq for PdfPicture {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

impl std::fmt::Debug for PdfPicture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PdfPicture")
            .field("page", &self.page)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Which tile of a page a drawing is: the page, the pixels per point the
/// page draws at, and the tile's column and row in the page's grid of
/// [`TILE_WIDTH`] by [`TILE_HEIGHT`] tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PdfTileKey {
    /// The page's number, from 1.
    pub page: usize,
    density: u32,
    pub column: u32,
    pub row: u32,
}

impl PdfTileKey {
    fn new(page: usize, density: f32, column: u32, row: u32) -> Self {
        Self {
            page,
            density: density.to_bits(),
            column,
            row,
        }
    }

    /// The pixels per point the tile draws its page at.
    pub fn density(&self) -> f32 {
        f32::from_bits(self.density)
    }

    /// Every key of the page `page`, in key order.
    fn of_page(page: usize) -> RangeInclusive<Self> {
        let first = Self {
            page,
            density: 0,
            column: 0,
            row: 0,
        };
        first..=Self {
            density: u32::MAX,
            column: u32::MAX,
            row: u32::MAX,
            ..first
        }
    }
}

/// A tile of a page, drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfTile {
    pub key: PdfTileKey,
    /// Where the tile's top-left pixel lies in the whole page drawn at the
    /// key's density: pixels across, then down.
    pub x: u32,
    pub y: u32,
    pub picture: PdfPicture,
}

/// The part of a PDF tab's pages that shows, in points from the top-left
/// corner of the pages as [`PdfView::page_rect`] lays them out.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PdfArea {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    /// The [`PdfJump::seq`] of the newest jump the client made, 0 for none.
    pub jump: u64,
}

impl PdfArea {
    /// The area with a finite place and at least a point each way.
    fn sanitized(self) -> Self {
        let finite = |value: f32, otherwise: f32| {
            if value.is_finite() { value } else { otherwise }
        };
        Self {
            left: finite(self.left, 0.0).max(0.0),
            top: finite(self.top, 0.0).max(0.0),
            width: finite(self.width, 1.0).max(1.0),
            height: finite(self.height, 1.0).max(1.0),
            jump: self.jump,
        }
    }
}

/// Where a PDF tab's client scrolls its pages to, once: to a page you
/// picked or a link named, to a match, or to keep what shows in place as
/// the zoom changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfJump {
    /// Differs for each jump. The client hands back the newest it made in
    /// [`PdfArea::jump`].
    pub seq: u64,
    /// How far to scroll down from the top of the pages, in points.
    pub top: f32,
    /// How far to scroll across from their left, in points, or `None` to
    /// leave the scroll across alone.
    pub left: Option<f32>,
}

/// A place in a tab's pages for a jump to bring into view.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Anchor {
    page: usize,
    /// How far down the page the place is, in the page's own points.
    down: f32,
    /// How far below the view's top the place lands, in points.
    at: f32,
    /// Where the middle of the view lands across the pages, as a share of
    /// their width. `None` leaves the scroll across alone.
    across: Option<f32>,
    /// The page's top stays in view when the place is near it.
    keep_top: bool,
}

impl Anchor {
    /// The top of the page `page`, a gap below the view's top.
    fn top_of(page: usize) -> Self {
        Self {
            page,
            down: 0.0,
            at: PAGE_GAP,
            across: None,
            keep_top: true,
        }
    }
}

/// A rectangle of a tab's laid-out pages, in points from their top-left
/// corner.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Zone {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Zone {
    /// `area`, grown by `margin` times its width on each side and its
    /// height above and below.
    fn around(area: &PdfArea, margin: f32) -> Self {
        let (across, down) = (area.width * margin, area.height * margin);
        Self {
            left: area.left - across,
            top: area.top - down,
            right: area.left + area.width + across,
            bottom: area.top + area.height + down,
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    /// How far `other` lies from this zone's middle, in points.
    fn distance_from_middle(&self, other: &Self) -> f32 {
        let x = (self.left + self.right) / 2.0;
        let y = (self.top + self.bottom) / 2.0;
        let across = (other.left - x).max(x - other.right).max(0.0);
        let down = (other.top - y).max(y - other.bottom).max(0.0);
        across.hypot(down)
    }
}

/// One place a search found its text.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfMatch {
    /// The page's number, from 1.
    pub page: usize,
    /// The line the match is on.
    pub snippet: String,
    /// How far down the page that line's text starts, in the page's own
    /// points.
    pub top: f32,
}

/// A PDF tab's search.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfSearch {
    /// What you searched for.
    pub query: String,
    /// What the search found, in page order.
    pub matches: Vec<PdfMatch>,
    /// The match that shows, as an index into `matches`.
    pub current: Option<usize>,
    /// The search runs on the PDF's thread.
    pub searching: bool,
}

/// What a PDF tab shows: the PDF's pages laid out one under the other,
/// where you are in them, how large they draw and the tiles drawn so far,
/// its thumbnails and its search.
#[derive(Debug, Clone)]
pub struct PdfView {
    path: String,
    state: PdfState,
    /// Each page's size in points, as it draws, rotation included.
    pages: Vec<[f32; 2]>,
    /// The heights of the pages before each page, added up, and of every
    /// page last.
    heights: Vec<f64>,
    /// The widest page's width, in points.
    widest: f32,
    page: usize,
    zoom: PdfZoom,
    /// How wide the tab draws its pages at most, in points, as its client
    /// said.
    width: Option<f32>,
    /// The pixels per point of the client's screen.
    pixels_per_point: f32,
    /// What the client showed last.
    area: Option<PdfArea>,
    thumbnails_shown: bool,
    tiles: BTreeMap<PdfTileKey, PdfTile>,
    /// How many bytes of pixels `tiles` holds.
    tile_bytes: usize,
    /// The tiles whose newest drawing failed.
    undrawn: BTreeSet<PdfTileKey>,
    thumbnails: HashMap<usize, PdfPicture>,
    search: PdfSearch,
    /// The jump the client hasn't made yet, and its number.
    jump: Option<(u64, Anchor)>,
    /// The view's top where the last jump landed. Its page stays the one
    /// that shows until you scroll away from there, as when the end of
    /// the pages keeps a short last page from reaching the view's top.
    landed: Option<f32>,
}

impl PdfView {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            state: PdfState::Opening,
            pages: Vec::new(),
            heights: vec![0.0],
            widest: 0.0,
            page: 1,
            zoom: PdfZoom::FitWidth,
            width: None,
            pixels_per_point: 1.0,
            area: None,
            thumbnails_shown: false,
            tiles: BTreeMap::new(),
            tile_bytes: 0,
            undrawn: BTreeSet::new(),
            thumbnails: HashMap::new(),
            search: PdfSearch::default(),
            jump: None,
            landed: None,
        }
    }

    fn set_pages(&mut self, pages: Vec<[f32; 2]>) {
        let mut total = 0.0;
        self.heights = std::iter::once(0.0)
            .chain(pages.iter().map(|[_, height]| {
                total += f64::from(height.max(0.0));
                total
            }))
            .collect();
        self.widest = pages.iter().map(|[width, _]| *width).fold(0.0, f32::max);
        self.pages = pages;
    }

    /// The PDF's workspace path.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn state(&self) -> &PdfState {
        &self.state
    }

    /// How many pages the PDF has, once it's open.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page that shows, from 1: the one at the middle of the view, or
    /// the one you last went to until you scroll.
    pub fn page(&self) -> usize {
        self.page
    }

    /// The size of the page `page` in points, across then down, as it
    /// draws.
    pub fn page_size(&self, page: usize) -> Option<[f32; 2]> {
        page.checked_sub(1)
            .and_then(|index| self.pages.get(index))
            .copied()
    }

    pub fn zoom(&self) -> PdfZoom {
        self.zoom
    }

    /// The scale the pages draw at: the zoom's, or for Fit width, the
    /// tab's width over the widest page's.
    pub fn scale(&self) -> f32 {
        match self.zoom {
            PdfZoom::Scale(scale) => scale,
            PdfZoom::FitWidth => match self.width {
                Some(width) if self.widest > 0.0 => {
                    (width / self.widest).clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1])
                }
                _ => 1.0,
            },
        }
    }

    /// The pixels per point of the client's screen, as it said.
    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    /// The pixels per point the tab draws its pages at: the scale times
    /// the screen's pixels per point, so a page draws a pixel for each of
    /// the screen's.
    pub fn density(&self) -> f32 {
        density_of(self.scale(), self.pixels_per_point)
    }

    /// The size of the laid-out pages in points: as wide as the widest
    /// page or the tab, with a gap each side, and as tall as the pages
    /// with a gap above each and below the last.
    pub fn content_size(&self) -> [f32; 2] {
        let gaps = f64::from(PAGE_GAP) * (self.pages.len() as f64 + 1.0);
        let pages = self.heights.last().copied().unwrap_or(0.0) * f64::from(self.scale());
        [self.column_width(), (gaps + pages) as f32]
    }

    fn column_width(&self) -> f32 {
        let pages = self.widest * self.scale() + 2.0 * PAGE_GAP;
        self.width
            .map_or(pages, |width| pages.max(width + 2.0 * PAGE_GAP))
    }

    /// How far down the laid-out pages the page `page` starts, in points.
    fn page_top(&self, page: usize) -> f64 {
        let index = page.saturating_sub(1).min(self.pages.len());
        let above = self.heights.get(index).copied().unwrap_or(0.0);
        f64::from(PAGE_GAP) * (index as f64 + 1.0) + above * f64::from(self.scale())
    }

    fn page_bottom(&self, page: usize) -> f64 {
        let height = self.page_size(page).map_or(0.0, |size| size[1]);
        self.page_top(page) + f64::from(height * self.scale())
    }

    /// Where the page `page` lays out, in points from the top-left corner
    /// of the pages: its left, top, width and height at the zoom. Pages
    /// line up on the middle of the widest.
    pub fn page_rect(&self, page: usize) -> Option<[f32; 4]> {
        let [width, height] = self.page_size(page)?;
        let scale = self.scale();
        Some([
            (self.column_width() - width * scale) / 2.0,
            self.page_top(page) as f32,
            width * scale,
            height * scale,
        ])
    }

    /// The pages that lie wholly or in part between `top` and `bottom`
    /// points down the laid-out pages, by number.
    pub fn pages_between(&self, top: f32, bottom: f32) -> Range<usize> {
        let count = self.pages.len();
        let first = partition(count, |index| self.page_bottom(index + 1) <= f64::from(top));
        let end = partition(count, |index| self.page_top(index + 1) < f64::from(bottom));
        first + 1..end.max(first) + 1
    }

    /// The page at `down` points down the laid-out pages. Each page takes
    /// half the gap above it and half the gap below.
    fn page_at(&self, down: f32) -> usize {
        let half = f64::from(PAGE_GAP / 2.0);
        let index = partition(self.pages.len(), |index| {
            self.page_bottom(index + 1) + half <= f64::from(down)
        });
        (index + 1).min(self.pages.len().max(1))
    }

    /// The tiles of the page `page` the tab keeps, in key order. Those
    /// drawn at another density, for another zoom or screen, stay until
    /// the tiles at the newest one cover the part of the page that shows.
    pub fn tiles(&self, page: usize) -> impl Iterator<Item = &PdfTile> {
        self.tiles
            .range(PdfTileKey::of_page(page))
            .map(|(_, tile)| tile)
    }

    /// The tile `key`, while the tab keeps it.
    pub fn tile(&self, key: &PdfTileKey) -> Option<&PdfTile> {
        self.tiles.get(key)
    }

    /// How many tiles the tab keeps.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// How many bytes of pixels the tab's tiles hold.
    pub fn tile_bytes(&self) -> usize {
        self.tile_bytes
    }

    /// Whether the newest drawing of a tile of the page `page`, at the
    /// newest density, failed.
    pub fn page_failed(&self, page: usize) -> bool {
        let density = self.density().to_bits();
        self.undrawn
            .range(PdfTileKey::of_page(page))
            .any(|key| key.density == density)
    }

    /// Where the client should scroll to, until it says it did.
    pub fn jump(&self) -> Option<PdfJump> {
        let (seq, anchor) = self.jump?;
        let page = anchor.page.clamp(1, self.pages.len().max(1));
        let page_top = self.page_top(page);
        let mut top =
            page_top + f64::from(anchor.down) * f64::from(self.scale()) - f64::from(anchor.at);
        if anchor.keep_top {
            top = top.max(page_top - f64::from(PAGE_GAP));
        }
        let width = self.area.map_or(0.0, |area| area.width);
        let left = anchor
            .across
            .map(|across| (across * self.column_width() - width / 2.0).max(0.0));
        Some(PdfJump {
            seq,
            top: top.max(0.0) as f32,
            left,
        })
    }

    /// The place at the middle of the view, to keep there across a change
    /// of zoom.
    fn middle(&self) -> Option<Anchor> {
        let area = self.area?;
        if self.pages.is_empty() {
            return None;
        }
        let middle = area.top + area.height / 2.0;
        let page = self.page_at(middle);
        let down = (f64::from(middle) - self.page_top(page)) / f64::from(self.scale());
        Some(Anchor {
            page,
            down: down as f32,
            at: area.height / 2.0,
            across: Some((area.left + area.width / 2.0) / self.column_width()),
            keep_top: false,
        })
    }

    pub fn thumbnails_shown(&self) -> bool {
        self.thumbnails_shown
    }

    /// The pages the thumbnails panel lists, one each.
    pub fn thumbnail_pages(&self) -> impl Iterator<Item = usize> + use<> {
        1..=self.pages.len()
    }

    /// The thumbnail of the page `page`, once it's drawn.
    pub fn thumbnail(&self, page: usize) -> Option<&PdfPicture> {
        self.thumbnails.get(&page)
    }

    pub fn search(&self) -> &PdfSearch {
        &self.search
    }

    /// The pixels per point the thumbnail of the page `page` draws at on
    /// a screen with `pixels_per_point`: [`THUMBNAIL_WIDTH`] wide, or
    /// [`THUMBNAIL_MAX_HEIGHT`] tall for a tall page, within [`MAX_SIDE`].
    fn thumbnail_density(&self, page: usize, pixels_per_point: f32) -> f32 {
        let [width, height] = self.page_size(page).unwrap_or([1.0, 1.0]);
        let scale = (THUMBNAIL_WIDTH / width.max(1.0)).min(THUMBNAIL_MAX_HEIGHT / height.max(1.0));
        let largest = MAX_SIDE / width.max(height).max(1.0);
        (scale * pixels_per_point).min(largest).max(0.01)
    }

    /// Where the tile `key` lays out, in points from the top-left corner
    /// of the pages, at the zoom. `None` past its page.
    fn key_zone(&self, key: &PdfTileKey) -> Option<Zone> {
        let size = self.page_size(key.page)?;
        let [left, top, _, _] = self.page_rect(key.page)?;
        let [x, y, width, height] =
            tile_pixels(page_pixels(size, key.density()), key.column, key.row)?;
        let points = self.scale() / key.density();
        Some(Zone {
            left: left + x as f32 * points,
            top: top + y as f32 * points,
            right: left + (x + width) as f32 * points,
            bottom: top + (y + height) as f32 * points,
        })
    }

    /// The tiles of the page `page`, at the newest density, that cover
    /// `zone`, row by row.
    fn tiles_over(&self, page: usize, zone: &Zone) -> Vec<PdfTileKey> {
        let (Some(size), Some([left, top, _, _])) = (self.page_size(page), self.page_rect(page))
        else {
            return Vec::new();
        };
        let density = self.density();
        let pixels = page_pixels(size, density);
        let per_point = density / self.scale();
        let span = |from: f32, to: f32, origin: f32, tile: u32, count: u32| {
            let clamp = |value: f32| {
                if value.is_nan() {
                    0
                } else {
                    value.clamp(0.0, count as f32) as u32
                }
            };
            let first = ((from - origin) * per_point / tile as f32).floor();
            let end = ((to - origin) * per_point / tile as f32).ceil();
            clamp(first)..clamp(end)
        };
        let columns = span(
            zone.left,
            zone.right,
            left,
            TILE_WIDTH,
            pixels[0].div_ceil(TILE_WIDTH),
        );
        let rows = span(
            zone.top,
            zone.bottom,
            top,
            TILE_HEIGHT,
            pixels[1].div_ceil(TILE_HEIGHT),
        );
        rows.flat_map(|row| {
            columns
                .clone()
                .map(move |column| PdfTileKey::new(page, density, column, row))
        })
        .collect()
    }
}

/// How many of the indices below `count` come before the first for which
/// `before` fails, for a `before` that holds for the first few and for
/// none after.
fn partition(count: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        if before(middle) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// The pixels per point that draw a page at `scale` on a screen with
/// `pixels_per_point`, within a sane range.
fn density_of(scale: f32, pixels_per_point: f32) -> f32 {
    let density = scale * pixels_per_point;
    if density.is_finite() && density > 0.0 {
        density.clamp(1.0 / 64.0, 64.0)
    } else {
        1.0
    }
}

/// The size in pixels of a page `size` points large drawn at `density`
/// pixels per point: at least a pixel on each side. A side a hair past a
/// whole number of pixels, from rounding, takes that number.
fn page_pixels(size: [f32; 2], density: f32) -> [u32; 2] {
    size.map(|side| {
        (f64::from(side.max(0.0)) * f64::from(density) - 1e-3)
            .ceil()
            .clamp(1.0, MAX_PAGE_PIXELS) as u32
    })
}

/// The pixels the tile at `column` and `row` covers in a page `pixels`
/// large: its left, top, width and height. `None` past the page.
fn tile_pixels(pixels: [u32; 2], column: u32, row: u32) -> Option<[u32; 4]> {
    let x = column.checked_mul(TILE_WIDTH)?;
    let y = row.checked_mul(TILE_HEIGHT)?;
    (x < pixels[0] && y < pixels[1]).then(|| {
        [
            x,
            y,
            (pixels[0] - x).min(TILE_WIDTH),
            (pixels[1] - y).min(TILE_HEIGHT),
        ]
    })
}

/// The page a link's or an embed's `#page=` fragment names, as written:
/// `None` when it names none or names one that isn't a whole number from 1.
/// A number too large to hold names the largest page there could be, which
/// is past the end of any PDF.
pub(crate) fn page_in_link(target: &str) -> Option<u32> {
    let target = target.split('|').next().unwrap_or(target);
    let (_, fragment) = target.split_once('#')?;
    fragment
        .split('&')
        .find_map(|part| part.strip_prefix("page="))
        .filter(|page| !page.is_empty() && page.bytes().all(|byte| byte.is_ascii_digit()))
        // Only digits are left, so a number that doesn't parse is too large.
        .map(|page| page.parse::<u32>().unwrap_or(u32::MAX))
        .filter(|page| *page > 0)
}

/// The page that opens for a link naming `asked` in a PDF of `count`
/// pages: page 1 for none, the last page for one past the end.
pub(crate) fn page_to_open(asked: Option<u32>, count: usize) -> usize {
    let asked = asked.map_or(1, |page| usize::try_from(page).unwrap_or(usize::MAX));
    asked.clamp(1, count.max(1))
}

/// Whether the workspace path `path` names a PDF.
pub(crate) fn is_pdf(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && extension.eq_ignore_ascii_case("pdf"))
}

/// Holds every PDF thread before it draws, or makes each drawing fail, so
/// tests can watch the window keep drawing while a page renders and see
/// what a page that can't be drawn shows.
#[derive(Clone, Default)]
pub(crate) struct RenderGate(Arc<Gate>);

#[derive(Default)]
pub(crate) struct Gate {
    held: Mutex<bool>,
    changed: Condvar,
    failing: AtomicBool,
}

impl RenderGate {
    fn wait(&self) {
        let mut held = self.0.held.lock().unwrap_or_else(PoisonError::into_inner);
        while *held {
            held = self
                .0
                .changed
                .wait(held)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn failing(&self) -> bool {
        self.0.failing.load(Ordering::SeqCst)
    }

    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub(crate) fn set(&self, hold: bool) {
        *self.0.held.lock().unwrap_or_else(PoisonError::into_inner) = hold;
        self.0.changed.notify_all();
    }

    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]
    pub(crate) fn fail(&self, fail: bool) {
        self.0.failing.store(fail, Ordering::SeqCst);
    }
}

/// Holds every PDF thread before it draws, until dropped.
#[cfg(feature = "test-hooks")]
pub struct PdfRenderHold(pub(crate) RenderGate);

#[cfg(feature = "test-hooks")]
impl Drop for PdfRenderHold {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Makes every drawing of a page or thumbnail fail, until dropped.
#[cfg(feature = "test-hooks")]
pub struct PdfDrawFailure(pub(crate) RenderGate);

#[cfg(feature = "test-hooks")]
impl Drop for PdfDrawFailure {
    fn drop(&mut self) {
        self.0.fail(false);
    }
}

enum Request {
    Unlock(Zeroizing<String>),
    /// Draw these tiles, most wanted first, in place of those asked for
    /// before.
    Tiles(Vec<PdfTileKey>),
    Thumbnail {
        page: usize,
        density: f32,
    },
    Search {
        generation: u64,
        query: String,
    },
    /// The files changed: read the PDF again if it did.
    Check,
    /// Try to read a PDF that couldn't open again.
    Retry,
    /// The PDF moved to this path.
    Moved(String),
}

enum Answer {
    Opened {
        pages: Vec<[f32; 2]>,
    },
    Locked {
        wrong: bool,
    },
    Failed {
        message: String,
    },
    Tile(PdfTile),
    /// The tile couldn't be drawn.
    TileFailed(PdfTileKey),
    Thumbnail(PdfPicture),
    Searched {
        generation: u64,
        matches: Vec<PdfMatch>,
    },
}

/// Counts the pictures drawn so far.
static PICTURES: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    PICTURES.fetch_add(1, Ordering::Relaxed)
}

/// One PDF tab: what it shows, and its thread.
pub(crate) struct PdfSession {
    view: PdfView,
    requests: Sender<Request>,
    answers: Receiver<Answer>,
    /// The tiles in the newest list the thread took that it hasn't drawn.
    asked: HashSet<PdfTileKey>,
    asked_thumbnails: HashSet<usize>,
    /// When the thumbnails panel last wanted each thumbnail, as a count of
    /// wants.
    thumbnail_wants: HashMap<usize, u64>,
    wants: u64,
    search_generation: u64,
    /// Counts the jumps so far.
    jumps: u64,
    /// The page a link asked for before the page count was known.
    pending_page: Option<Option<u32>>,
    /// What another thread needs, should this one stop.
    engine: Arc<WorkspaceEngine>,
    wake: Arc<dyn Fn() + Send + Sync>,
    gate: RenderGate,
}

impl PdfSession {
    /// Open the PDF at `path` on a thread of its own. `wake` runs whenever
    /// the thread has an answer for [`Self::poll`].
    pub(crate) fn open(
        engine: Arc<WorkspaceEngine>,
        path: &str,
        wake: Arc<dyn Fn() + Send + Sync>,
        gate: RenderGate,
    ) -> Self {
        let (requests, answers) = (channel().0, channel().1);
        let mut session = Self {
            view: PdfView::new(path),
            requests,
            answers,
            asked: HashSet::new(),
            asked_thumbnails: HashSet::new(),
            thumbnail_wants: HashMap::new(),
            wants: 0,
            search_generation: 0,
            jumps: 0,
            pending_page: None,
            engine,
            wake,
            gate,
        };
        session.start();
        session
    }

    /// Start the tab's thread, which reads the PDF again. When it can't
    /// start, the tab fails, and Try again starts it again.
    fn start(&mut self) {
        let (requests, receive_requests) = channel();
        let (send_answers, answers) = channel();
        let worker = Worker {
            engine: self.engine.clone(),
            path: self.view.path.clone(),
            requests: receive_requests,
            pending: Vec::new(),
            tiles: VecDeque::new(),
            answers: send_answers,
            wake: self.wake.clone(),
            gate: self.gate.clone(),
        };
        self.requests = requests;
        self.answers = answers;
        self.asked.clear();
        self.asked_thumbnails.clear();
        let spawned = std::thread::Builder::new()
            .name("pdf".into())
            .spawn(move || worker.run());
        if let Err(error) = spawned {
            tracing::error!(%error, "could not start a PDF thread");
            self.view.state = PdfState::Failed {
                message: "Noura couldn't open this PDF.".to_owned(),
            };
        }
    }

    pub(crate) fn view(&self) -> &PdfView {
        &self.view
    }

    fn send(&self, request: Request) {
        // A thread that stopped has answered why, or the session fails
        // in `poll`.
        let _ = self.requests.send(request);
    }

    /// Take in what the thread answered. Returns whether the view changed.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.answers.try_recv() {
                Ok(answer) => {
                    self.apply(answer);
                    changed = true;
                }
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    if !matches!(self.view.state, PdfState::Failed { .. }) {
                        tracing::error!("a PDF thread stopped");
                        self.view.state = PdfState::Failed { message: damaged() };
                        changed = true;
                    }
                    return changed;
                }
            }
        }
    }

    /// Forget every drawing, as when the file changes or locks.
    fn forget_drawings(&mut self) {
        self.view.tiles.clear();
        self.view.tile_bytes = 0;
        self.view.undrawn.clear();
        self.view.thumbnails.clear();
        self.asked.clear();
        self.asked_thumbnails.clear();
        self.thumbnail_wants.clear();
    }

    fn apply(&mut self, answer: Answer) {
        match answer {
            Answer::Opened { pages } => {
                let count = pages.len();
                self.view.set_pages(pages);
                self.view.state = PdfState::Ready;
                // A new file draws again from scratch.
                self.forget_drawings();
                self.view.landed = None;
                match self.pending_page.take() {
                    Some(asked) => {
                        self.go_to(page_to_open(asked, count));
                    }
                    None => self.view.page = self.view.page.clamp(1, count.max(1)),
                }
                let query = std::mem::take(&mut self.view.search.query);
                self.search(&query);
            }
            Answer::Locked { wrong } => {
                self.view.state = PdfState::Locked { wrong };
                self.forget_drawings();
            }
            Answer::Failed { message } => {
                self.view.state = PdfState::Failed { message };
                self.forget_drawings();
            }
            Answer::Tile(tile) => {
                self.asked.remove(&tile.key);
                // A tile drawn for an older zoom, or for a place you
                // scrolled far from since, is too late to help.
                let near = self.view.area.is_none_or(|area| {
                    self.view
                        .key_zone(&tile.key)
                        .is_some_and(|zone| Zone::around(&area, KEEP_AHEAD).overlaps(&zone))
                });
                if near && tile.key.density == self.view.density().to_bits() {
                    self.view.undrawn.remove(&tile.key);
                    self.view.tile_bytes += tile.picture.rgba.len();
                    if let Some(old) = self.view.tiles.insert(tile.key, tile) {
                        self.view.tile_bytes -= old.picture.rgba.len();
                    }
                }
            }
            Answer::TileFailed(key) => {
                self.asked.remove(&key);
                if key.density == self.view.density().to_bits() {
                    self.view.undrawn.insert(key);
                }
            }
            Answer::Thumbnail(picture) => {
                self.view.thumbnails.insert(picture.page, picture);
                self.trim_thumbnails();
            }
            Answer::Searched {
                generation,
                matches,
            } if generation == self.search_generation => {
                self.view.search.searching = false;
                self.view.search.current = (!matches.is_empty()).then_some(0);
                if let Some(first) = matches.first() {
                    self.show_match(&first.clone());
                }
                self.view.search.matches = matches;
            }
            Answer::Searched { .. } => {}
        }
    }

    /// Have the client scroll so `anchor` shows.
    fn jump_to(&mut self, anchor: Anchor) {
        self.jumps += 1;
        self.view.jump = Some((self.jumps, anchor));
        self.view.landed = None;
    }

    /// Scroll to the top of the page `page`, or of the nearest page there
    /// is.
    pub(crate) fn go_to(&mut self, page: usize) -> bool {
        if self.view.state != PdfState::Ready {
            return false;
        }
        let page = page.clamp(1, self.view.page_count().max(1));
        self.view.page = page;
        self.jump_to(Anchor::top_of(page));
        true
    }

    /// Scroll to the page a link named, once the page count is known.
    pub(crate) fn go_to_link_page(&mut self, asked: Option<u32>) {
        if self.view.state == PdfState::Ready {
            self.go_to(page_to_open(asked, self.view.page_count()));
        } else {
            self.pending_page = Some(asked);
        }
    }

    /// Scroll to the line with `found`, a third of the way down the view,
    /// or to its page's top when the line is near it.
    fn show_match(&mut self, found: &PdfMatch) {
        let at = self
            .view
            .area
            .map_or(PAGE_GAP, |area| (area.height / 3.0).max(PAGE_GAP));
        self.view.page = found.page;
        self.jump_to(Anchor {
            page: found.page,
            down: found.top,
            at,
            across: None,
            keep_top: true,
        });
    }

    /// Change the zoom to `zoom`, keeping what shows at the middle of the
    /// view there.
    fn set_zoom(&mut self, zoom: PdfZoom) -> bool {
        if zoom == self.view.zoom {
            return false;
        }
        // A jump the client hasn't made yet keeps its place instead.
        let anchor = self
            .view
            .jump
            .map(|(_, anchor)| anchor)
            .or_else(|| self.view.middle());
        self.view.zoom = zoom;
        if let Some(anchor) = anchor {
            self.view.page = anchor.page;
            self.jump_to(anchor);
        }
        true
    }

    /// Zoom one step in or out from the scale the pages draw at.
    pub(crate) fn zoom(&mut self, zoom_in: bool) -> bool {
        let current = self.view.scale();
        let next = if zoom_in {
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > current + 0.001)
                .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current - 0.001)
                .unwrap_or(ZOOM_STEPS[0])
        };
        self.set_zoom(PdfZoom::Scale(next))
    }

    pub(crate) fn fit_width(&mut self) -> bool {
        self.set_zoom(PdfZoom::FitWidth)
    }

    /// The tab draws its pages `width` points wide at most, on a screen
    /// with `pixels_per_point`. At Fit width, a new width scales the
    /// pages, and what shows at the middle of the view stays there.
    pub(crate) fn size(&mut self, width: f32, pixels_per_point: f32) {
        if width.is_finite() && width > 0.0 && self.view.width != Some(width) {
            let anchor = if self.view.zoom == PdfZoom::FitWidth {
                self.view
                    .jump
                    .map(|(_, anchor)| anchor)
                    .or_else(|| self.view.middle())
            } else {
                None
            };
            self.view.width = Some(width);
            if let Some(anchor) = anchor {
                self.jump_to(anchor);
            }
        }
        if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
            self.view.pixels_per_point = pixels_per_point;
        }
    }

    /// The client shows `area` of the pages: follow the page that shows,
    /// draw the tiles that show and those near them, and forget those far
    /// from them.
    pub(crate) fn show_area(&mut self, area: PdfArea) {
        if self.view.state != PdfState::Ready || self.view.pages.is_empty() {
            return;
        }
        let area = area.sanitized();
        self.view.area = Some(area);
        if let Some((seq, anchor)) = self.view.jump {
            // Until the client makes the jump, what it shows is about to
            // change.
            if area.jump != seq {
                return;
            }
            self.view.page = anchor.page.clamp(1, self.view.page_count());
            self.view.jump = None;
            self.view.landed = Some(area.top);
        } else if !self
            .view
            .landed
            .is_some_and(|top| (top - area.top).abs() < 0.5)
        {
            self.view.landed = None;
            self.view.page = self.view.page_at(area.top + area.height / 2.0);
        }
        self.update_tiles(&area);
    }

    fn forget_tile(&mut self, key: &PdfTileKey) {
        if let Some(tile) = self.view.tiles.remove(key) {
            self.view.tile_bytes -= tile.picture.rgba.len();
        }
    }

    /// Forget the tiles far from `area`, and ask the thread for those that
    /// show or soon will, from the middle of the view out.
    fn update_tiles(&mut self, area: &PdfArea) {
        let density = self.view.density().to_bits();
        let shown = Zone::around(area, 0.0);
        let ahead = Zone::around(area, DRAW_AHEAD);
        let keep = Zone::around(area, KEEP_AHEAD);
        let view = &self.view;

        // A tile drawn at another density stays while its page shows and
        // the newest density's tiles don't cover what shows of it yet.
        let covered: HashSet<usize> = view
            .pages_between(shown.top, shown.bottom)
            .filter(|page| {
                view.tiles_over(*page, &shown)
                    .iter()
                    .all(|key| view.tiles.contains_key(key))
            })
            .collect();
        let forget: Vec<PdfTileKey> = view
            .tiles
            .keys()
            .filter(|key| {
                let Some(zone) = view.key_zone(key) else {
                    return true;
                };
                !keep.overlaps(&zone)
                    || (key.density != density
                        && (!shown.overlaps(&zone) || covered.contains(&key.page)))
            })
            .copied()
            .collect();
        // A tile whose drawing failed is tried again once it went out of
        // reach or the density changed.
        let retry: Vec<PdfTileKey> = view
            .undrawn
            .iter()
            .filter(|key| {
                key.density != density
                    || !view.key_zone(key).is_some_and(|zone| keep.overlaps(&zone))
            })
            .copied()
            .collect();
        for key in &forget {
            self.forget_tile(key);
        }
        for key in &retry {
            self.view.undrawn.remove(key);
        }

        // Past the most bytes, the tiles that don't show go, farthest
        // first.
        if self.view.tile_bytes > MAX_TILE_BYTES {
            let mut far: Vec<(f32, PdfTileKey)> = self
                .view
                .tiles
                .keys()
                .filter_map(|key| {
                    let zone = self.view.key_zone(key)?;
                    (!shown.overlaps(&zone)).then(|| (shown.distance_from_middle(&zone), *key))
                })
                .collect();
            far.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, key) in far {
                if self.view.tile_bytes <= MAX_TILE_BYTES {
                    break;
                }
                self.forget_tile(&key);
            }
        }

        let view = &self.view;
        let mut wanted: Vec<(bool, f32, PdfTileKey)> = Vec::new();
        for page in view.pages_between(ahead.top, ahead.bottom) {
            for key in view.tiles_over(page, &ahead) {
                if view.tiles.contains_key(&key) || view.undrawn.contains(&key) {
                    continue;
                }
                if let Some(zone) = view.key_zone(&key) {
                    wanted.push((
                        !shown.overlaps(&zone),
                        shown.distance_from_middle(&zone),
                        key,
                    ));
                }
            }
            // Thousands of pages a pixel tall can't all draw at once.
            if wanted.len() >= 4 * MAX_ASKED_TILES {
                break;
            }
        }
        wanted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        wanted.truncate(MAX_ASKED_TILES);
        let keys: HashSet<PdfTileKey> = wanted.iter().map(|(_, _, key)| *key).collect();
        if keys != self.asked {
            self.send(Request::Tiles(
                wanted.into_iter().map(|(_, _, key)| key).collect(),
            ));
            self.asked = keys;
        }
    }

    /// Draw the thumbnail of the page `page`, for a screen with
    /// `pixels_per_point`, unless it's drawn or asked for already. Past
    /// [`MAX_THUMBNAILS`], the thumbnails wanted longest ago go.
    pub(crate) fn want_thumbnail(&mut self, page: usize, pixels_per_point: f32) {
        if self.view.state != PdfState::Ready || self.view.page_size(page).is_none() {
            return;
        }
        self.wants += 1;
        self.thumbnail_wants.insert(page, self.wants);
        if self.asked_thumbnails.insert(page) {
            let density = self.view.thumbnail_density(page, pixels_per_point);
            self.send(Request::Thumbnail { page, density });
        }
        self.trim_thumbnails();
    }

    /// Past [`MAX_THUMBNAILS`], forget the thumbnails wanted longest ago.
    fn trim_thumbnails(&mut self) {
        while self.view.thumbnails.len() > MAX_THUMBNAILS {
            let wants = &self.thumbnail_wants;
            let Some(oldest) = self
                .view
                .thumbnails
                .keys()
                .min_by_key(|page| wants.get(page).copied().unwrap_or(0))
                .copied()
            else {
                break;
            };
            self.view.thumbnails.remove(&oldest);
            self.asked_thumbnails.remove(&oldest);
            self.thumbnail_wants.remove(&oldest);
        }
    }

    pub(crate) fn toggle_thumbnails(&mut self) {
        self.view.thumbnails_shown = !self.view.thumbnails_shown;
    }

    pub(crate) fn unlock(&mut self, password: String) {
        if !matches!(self.view.state, PdfState::Locked { .. }) {
            drop(Zeroizing::new(password));
            return;
        }
        self.view.state = PdfState::Unlocking;
        self.send(Request::Unlock(Zeroizing::new(password)));
    }

    /// Search the PDF's text for `query`, ignoring case and how spaces and
    /// line breaks fall. An empty query ends the search.
    pub(crate) fn search(&mut self, query: &str) {
        self.search_generation += 1;
        let query = query.trim();
        self.view.search = PdfSearch {
            query: query.to_owned(),
            matches: Vec::new(),
            current: None,
            searching: !query.is_empty() && self.view.state == PdfState::Ready,
        };
        if self.view.search.searching {
            self.send(Request::Search {
                generation: self.search_generation,
                query: query.to_owned(),
            });
        }
    }

    /// Show the next match, or the one before it, wrapping around.
    pub(crate) fn step_match(&mut self, forward: bool) -> bool {
        let count = self.view.search.matches.len();
        if count == 0 {
            return false;
        }
        let current = match (self.view.search.current, forward) {
            (None, _) => 0,
            (Some(current), true) => (current + 1) % count,
            (Some(current), false) => (current + count - 1) % count,
        };
        self.view.search.current = Some(current);
        let found = self.view.search.matches[current].clone();
        self.show_match(&found);
        true
    }

    /// The files changed: the thread reads the PDF again if it did.
    pub(crate) fn check(&self) {
        self.send(Request::Check);
    }

    pub(crate) fn retry(&mut self) {
        if !matches!(self.view.state, PdfState::Failed { .. }) {
            return;
        }
        self.view.state = PdfState::Opening;
        // A thread that stopped takes no more requests: start another.
        if self.requests.send(Request::Retry).is_err() {
            self.start();
        }
    }

    pub(crate) fn moved_to(&mut self, path: &str) {
        path.clone_into(&mut self.view.path);
        self.send(Request::Moved(path.to_owned()));
    }
}

/// What reading a PDF found.
enum Read {
    Open(Pdf),
    Locked,
    Failed(String),
}

/// A file's size and modification time, which change when another app
/// replaces it.
type Stamp = Option<(u64, Option<SystemTime>)>;

/// One PDF tab's thread.
struct Worker {
    engine: Arc<WorkspaceEngine>,
    path: String,
    requests: Receiver<Request>,
    /// Requests taken from the channel and not handled yet, but tiles.
    pending: Vec<Request>,
    /// The tiles to draw, most wanted first.
    tiles: VecDeque<PdfTileKey>,
    answers: Sender<Answer>,
    wake: Arc<dyn Fn() + Send + Sync>,
    gate: RenderGate,
}

/// What a thread does next.
enum Job {
    Request(Request),
    /// Draw the most wanted tile.
    Tiles,
}

/// What a thread does after it served an open PDF or waited on one that
/// couldn't open.
enum Next {
    Read(Option<Zeroizing<String>>),
    Stop,
}

impl Worker {
    fn answer(&self, answer: Answer) {
        if self.answers.send(answer).is_ok() {
            (self.wake)();
        }
    }

    fn run(mut self) {
        let mut password: Option<Zeroizing<String>> = None;
        loop {
            let stamp = self.stamp();
            let tried = password.is_some();
            let read = read_pdf(&self.engine, &self.path, password.take());
            let next = match read {
                Read::Open(pdf) if pdf.pages().is_empty() => {
                    self.answer(Answer::Failed {
                        message: NO_PAGES.to_owned(),
                    });
                    self.wait(&stamp)
                }
                Read::Open(pdf) => {
                    let pages = pdf
                        .pages()
                        .iter()
                        .map(|page| {
                            let (width, height) = page.render_dimensions();
                            [width, height]
                        })
                        .collect();
                    self.answer(Answer::Opened { pages });
                    self.serve(&pdf, &stamp)
                }
                Read::Locked => {
                    self.answer(Answer::Locked { wrong: tried });
                    self.wait(&stamp)
                }
                Read::Failed(message) => {
                    self.answer(Answer::Failed { message });
                    self.wait(&stamp)
                }
            };
            match next {
                Next::Read(next) => password = next,
                Next::Stop => return,
            }
        }
    }

    fn stamp(&self) -> Stamp {
        self.engine.file_version(&self.path).ok()
    }

    /// Take `request` in: a list of tiles replaces the one before it.
    fn receive(&mut self, request: Request) {
        match request {
            Request::Tiles(tiles) => self.tiles = tiles.into(),
            request => self.pending.push(request),
        }
    }

    /// Take the requests waiting in the channel. `false` once the session
    /// dropped its end, as when the tab closes.
    fn take_waiting(&mut self) -> bool {
        loop {
            match self.requests.try_recv() {
                Ok(request) => self.receive(request),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    /// The next job: unlocking, checking the file and following a move
    /// first, then the tiles, the newest search, and the thumbnails.
    /// `None` once the tab closed.
    fn next(&mut self) -> Option<Job> {
        if !self.take_waiting() {
            return None;
        }
        while self.pending.is_empty() && self.tiles.is_empty() {
            let request = self.requests.recv().ok()?;
            self.receive(request);
            if !self.take_waiting() {
                return None;
            }
        }
        // Only the newest search counts.
        if let Some(newest) = self
            .pending
            .iter()
            .rposition(|request| matches!(request, Request::Search { .. }))
        {
            let mut index = 0;
            self.pending.retain(|request| {
                let keep = index == newest || !matches!(request, Request::Search { .. });
                index += 1;
                keep
            });
        }
        let rank = |request: &Request| match request {
            Request::Unlock(_) | Request::Check | Request::Retry | Request::Moved(_) => 0,
            Request::Tiles(_) => 1,
            Request::Search { .. } => 2,
            Request::Thumbnail { .. } => 3,
        };
        let first = (0..self.pending.len()).min_by_key(|index| rank(&self.pending[*index]));
        match first {
            Some(index) if rank(&self.pending[index]) == 0 || self.tiles.is_empty() => {
                Some(Job::Request(self.pending.remove(index)))
            }
            _ => Some(Job::Tiles),
        }
    }

    /// Draw the most wanted tile, with the wanted tiles above and below it
    /// in its column, up to [`TILES_PER_PASS`], in one pass over the page,
    /// and answer with each tile or that it couldn't be drawn.
    fn draw_tiles<'a>(&mut self, pdf: &'a Pdf, cache: &RenderCache<'a>) {
        let Some(first) = self.tiles.pop_front() else {
            return;
        };
        let mut rows = first.row..first.row + 1;
        while rows.len() < TILES_PER_PASS {
            let next = self.tiles.iter().position(|key| {
                key.page == first.page
                    && key.density == first.density
                    && key.column == first.column
                    && (key.row == rows.end || key.row + 1 == rows.start)
            });
            let Some(key) = next.and_then(|index| self.tiles.remove(index)) else {
                break;
            };
            if key.row == rows.end {
                rows.end += 1;
            } else {
                rows.start -= 1;
            }
        }
        let keys: Vec<PdfTileKey> = rows.map(|row| PdfTileKey { row, ..first }).collect();
        let page = first
            .page
            .checked_sub(1)
            .and_then(|index| pdf.pages().get(index));
        self.gate.wait();
        let drawn = page
            .filter(|_| !self.gate.failing())
            .and_then(|page| draw_rows(page, cache, &keys));
        match drawn {
            Some(tiles) => {
                for tile in tiles {
                    self.answer(Answer::Tile(tile));
                }
            }
            None => {
                tracing::warn!("a PDF page couldn't be drawn");
                for key in keys {
                    self.answer(Answer::TileFailed(key));
                }
            }
        }
    }

    /// Draw the thumbnail of page `page` of `pdf` at `density` pixels per
    /// point. A thumbnail that can't be drawn stays a blank page.
    fn draw_thumbnail<'a>(&self, pdf: &'a Pdf, cache: &RenderCache<'a>, page: usize, density: f32) {
        let shown = page.checked_sub(1).and_then(|index| pdf.pages().get(index));
        self.gate.wait();
        match shown
            .filter(|_| !self.gate.failing())
            .and_then(|shown| draw(shown, cache, page, density))
        {
            Some(picture) => self.answer(Answer::Thumbnail(picture)),
            None => tracing::warn!("a PDF thumbnail couldn't be drawn"),
        }
    }

    /// Whether the file changed since `stamp`.
    fn changed(&self, stamp: &Stamp) -> bool {
        self.stamp() != *stamp
    }

    /// Wait on a PDF that couldn't open: read it again with a password,
    /// when you retry, or when the file changed.
    fn wait(&mut self, stamp: &Stamp) -> Next {
        loop {
            match self.next() {
                None => return Next::Stop,
                Some(Job::Tiles) => self.tiles.clear(),
                Some(Job::Request(Request::Unlock(password))) => return Next::Read(Some(password)),
                Some(Job::Request(Request::Retry)) => return Next::Read(None),
                Some(Job::Request(Request::Check)) if self.changed(stamp) => {
                    return Next::Read(None);
                }
                Some(Job::Request(Request::Moved(path))) => self.path = path,
                Some(Job::Request(_)) => {}
            }
        }
    }

    /// Draw and search the open `pdf` until the tab closes or the file
    /// changes.
    fn serve(&mut self, pdf: &Pdf, stamp: &Stamp) -> Next {
        let cache = RenderCache::new();
        let mut texts: Vec<Option<Vec<Line>>> = vec![None; pdf.pages().len()];
        loop {
            match self.next() {
                None => return Next::Stop,
                Some(Job::Tiles) => self.draw_tiles(pdf, &cache),
                Some(Job::Request(Request::Thumbnail { page, density })) => {
                    self.draw_thumbnail(pdf, &cache, page, density);
                }
                Some(Job::Request(Request::Search { generation, query })) => {
                    let mut matches = Vec::new();
                    let needle = fold(&query);
                    for (index, page) in pdf.pages().iter().enumerate() {
                        // The tab closed, so nobody waits for the search.
                        if !self.take_waiting() {
                            return Next::Stop;
                        }
                        // A newer search replaces this one.
                        if self
                            .pending
                            .iter()
                            .any(|request| matches!(request, Request::Search { .. }))
                        {
                            break;
                        }
                        // The pages in view draw before the search goes on.
                        while !self.tiles.is_empty() {
                            self.draw_tiles(pdf, &cache);
                            if !self.take_waiting() {
                                return Next::Stop;
                            }
                        }
                        let lines = texts[index].get_or_insert_with(|| page_text(page));
                        matches.extend(find(lines, &needle, index + 1));
                    }
                    self.answer(Answer::Searched {
                        generation,
                        matches,
                    });
                }
                Some(Job::Request(Request::Check | Request::Retry)) if self.changed(stamp) => {
                    return Next::Read(None);
                }
                Some(Job::Request(Request::Moved(path))) => self.path = path,
                Some(Job::Request(_)) => {}
            }
        }
    }
}

/// Read the PDF at `path` through `local-core` and parse it, with
/// `password` when there is one.
fn read_pdf(engine: &WorkspaceEngine, path: &str, password: Option<Zeroizing<String>>) -> Read {
    let bytes = match engine.read_file(path, MAX_PDF_BYTES) {
        Ok((bytes, _)) => bytes,
        Err(error) if error.code == "file_too_large" => {
            return Read::Failed(
                "This PDF is larger than 512 MB, the most Noura opens.".to_owned(),
            );
        }
        Err(error) => {
            return Read::Failed(AppError::from_core(error, path, Access::ReadFile).message);
        }
    };
    let password = password.unwrap_or_default();
    let parsed = std::panic::catch_unwind(AssertUnwindSafe(|| {
        Pdf::new_with_password(bytes, password.as_str())
    }));
    drop(password);
    match parsed {
        Ok(Ok(pdf)) => Read::Open(pdf),
        Ok(Err(LoadPdfError::Decryption(DecryptionError::PasswordProtected))) => Read::Locked,
        Ok(Err(LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm))) => {
            Read::Failed("This PDF is locked in a way Noura can't unlock yet.".to_owned())
        }
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("a PDF couldn't be parsed");
            Read::Failed(damaged())
        }
    }
}

fn damaged() -> String {
    "Noura couldn't read this PDF. It may be damaged.".to_owned()
}

/// The pixels per point that draw a side `side` points long at `density`,
/// kept within [`MAX_SIDE`] pixels and at least one pixel: hayro rounds a
/// page's size in pixels down, and no texture can be 0 pixels on a side.
fn side_scale(side: f32, density: f32) -> f32 {
    let side = side.max(1.0);
    let density = if density.is_finite() && density > 0.0 {
        density
    } else {
        1.0
    };
    if side * density < 1.0 {
        // One pixel and a half rounds down to one, whatever the rounding
        // error.
        1.5 / side
    } else {
        density.min(MAX_SIDE / side)
    }
}

/// Draw `page`, number `number`, whole at `density` pixels per point, or
/// on a side that would draw less than a pixel, one pixel, within
/// [`MAX_SIDE`]. `None` when hayro fails.
fn draw<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    number: usize,
    density: f32,
) -> Option<PdfPicture> {
    let (width, height) = page.render_dimensions();
    let drawn = std::panic::catch_unwind(AssertUnwindSafe(|| {
        hayro::render(
            page,
            cache,
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: side_scale(width, density),
                y_scale: side_scale(height, density),
                bg_color: WHITE,
            },
        )
    }))
    .ok()?;
    let pixels = usize::from(drawn.width()) * usize::from(drawn.height()) * 4;
    if pixels == 0 || drawn.data_as_u8_slice().len() != pixels {
        return None;
    }
    Some(PdfPicture {
        page: number,
        width: u32::from(drawn.width()),
        height: u32::from(drawn.height()),
        rgba: Arc::from(drawn.data_as_u8_slice()),
        generation: next_generation(),
    })
}

/// Draw the tiles `keys`, one column of a page's tiles from top to bottom,
/// in one pass over `page`. `None` when hayro fails or a key lies past the
/// page.
fn draw_rows<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    keys: &[PdfTileKey],
) -> Option<Vec<PdfTile>> {
    let first = keys.first()?;
    let density = first.density();
    let (width, height) = page.render_dimensions();
    let pixels = page_pixels([width, height], density);
    let rects = keys
        .iter()
        .map(|key| tile_pixels(pixels, key.column, key.row))
        .collect::<Option<Vec<_>>>()?;
    let [x, y, width, _] = rects[0];
    let height = rects.iter().map(|rect| rect[3]).sum();
    let pixmap = std::panic::catch_unwind(AssertUnwindSafe(|| {
        draw_region(page, cache, density, [x, y, width, height])
    }))
    .ok()??;
    let bytes = pixmap.data_as_u8_slice();
    let row = width as usize * 4;
    if bytes.len() != row * height as usize {
        return None;
    }
    let mut start = 0;
    Some(
        keys.iter()
            .zip(&rects)
            .map(|(key, [x, y, width, height])| {
                let end = start + row * *height as usize;
                let rgba = Arc::from(&bytes[start..end]);
                start = end;
                PdfTile {
                    key: *key,
                    x: *x,
                    y: *y,
                    picture: PdfPicture {
                        page: key.page,
                        width: *width,
                        height: *height,
                        rgba,
                        generation: next_generation(),
                    },
                }
            })
            .collect(),
    )
}

/// Draw the pixels `[x, y, width, height]` of `page` drawn whole at
/// `density` pixels per point, on white. `None` for a region no pixmap can
/// hold.
fn draw_region<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    density: f32,
    [x, y, width, height]: [u32; 4],
) -> Option<Pixmap> {
    let (Ok(width), Ok(height)) = (u16::try_from(width), u16::try_from(height)) else {
        return None;
    };
    if width == 0 || height == 0 {
        return None;
    }
    let mut context = RenderContext::new(width, height);
    let transform = Affine::translate((-f64::from(x), -f64::from(y)))
        * Affine::scale(f64::from(density))
        * page.initial_transform(true).to_kurbo();
    hayro::render_into(
        page,
        cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &mut context,
        transform,
    );
    context.flush();
    let mut pixmap = Pixmap::new(width, height);
    context.render_with(
        &mut pixmap,
        &mut vello_cpu::Resources::default(),
        vello_cpu::RasterizerSettings {
            target_init: vello_cpu::TargetInit::Clear(WHITE),
            ..Default::default()
        },
    );
    Some(pixmap)
}

/// Read the PDF at `path` and draw the page a link or embed names in
/// `asked`, `width` pixels wide: page 1 for none, the last page for one
/// past the end. `None` when the PDF can't open without a password, or
/// can't open at all.
pub(crate) fn draw_page_once(
    engine: &WorkspaceEngine,
    path: &str,
    asked: Option<u32>,
    width: f32,
) -> Result<PdfPicture, EmbedProblem> {
    let pdf = match read_pdf(engine, path, None) {
        Read::Open(pdf) => pdf,
        Read::Locked => return Err(EmbedProblem::Locked),
        Read::Failed(_) => return Err(EmbedProblem::Unreadable),
    };
    let number = page_to_open(asked, pdf.pages().len());
    let page = pdf
        .pages()
        .get(number - 1)
        .ok_or(EmbedProblem::Unreadable)?;
    let (page_width, page_height) = page.render_dimensions();
    // hayro rounds the page's size in pixels down, so half a pixel more
    // keeps it `width` wide.
    let density = ((width + 0.5) / page_width.max(1.0)).min(MAX_SIDE / page_height.max(1.0));
    let cache = RenderCache::new();
    draw(page, &cache, number, density).ok_or(EmbedProblem::Unreadable)
}

/// Why an embed of a PDF shows no page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedProblem {
    /// The PDF needs its password, which only its tab asks for.
    Locked,
    /// The PDF couldn't be read.
    Unreadable,
}

/// One line of a page's text.
#[derive(Debug, Clone, Default)]
struct Line {
    text: String,
    /// How far down the page the line's first glyph reaches, in the
    /// page's own points.
    top: f32,
}

/// Text folded for search: lowercase, one character for each of the
/// text's, with every run of spaces and line breaks one space.
fn fold(text: &str) -> Vec<char> {
    let mut folded = Vec::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_whitespace() {
            if folded.last().is_some_and(|last: &char| *last != ' ') {
                folded.push(' ');
            }
        } else {
            folded.push(ch.to_lowercase().next().unwrap_or(ch));
        }
    }
    while folded.last() == Some(&' ') {
        folded.pop();
    }
    folded
}

/// The places on page `page`, whose text is `lines`, where `needle` is,
/// each with the line it starts on.
fn find(lines: &[Line], needle: &[char], page: usize) -> Vec<PdfMatch> {
    if needle.is_empty() {
        return Vec::new();
    }
    // The page's text folded, and the line each folded character is on.
    let mut folded = Vec::new();
    let mut line_of = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let text = fold(&line.text);
        if text.is_empty() {
            continue;
        }
        if !folded.is_empty() {
            folded.push(' ');
            line_of.push(index);
        }
        line_of.extend(std::iter::repeat_n(index, text.len()));
        folded.extend(text);
    }
    let mut found = Vec::new();
    let mut start = 0;
    while start + needle.len() <= folded.len() {
        if folded[start..start + needle.len()] == *needle {
            let line = &lines[line_of[start]];
            found.push(PdfMatch {
                page,
                snippet: line.text.trim().to_owned(),
                top: line.top,
            });
            start += needle.len();
        } else {
            start += 1;
        }
    }
    found
}

/// The text on `page`, line by line.
fn page_text(page: &Page<'_>) -> Vec<Line> {
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let cache = InterpreterCache::new();
        let (width, height) = page.render_dimensions();
        let mut context = Context::new(
            page.initial_transform(true).to_kurbo(),
            Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            &cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        let mut device = TextDevice::default();
        interpret_page(page, &mut context, &mut device);
        device.lines
    }))
    .unwrap_or_else(|_| {
        tracing::warn!("a PDF page's text couldn't be read");
        Vec::new()
    })
}

/// Collects the text a page draws, line by line, from each glyph's Unicode
/// value and where it lands. Glyphs that draw on another baseline start a
/// new line, and a gap wider than a quarter of the text's size reads as a
/// space.
#[derive(Default)]
struct TextDevice {
    lines: Vec<Line>,
    /// Where the last glyph ended, and the size of its text.
    last: Option<(Point, f64)>,
    /// The last run's first glyph and length, to skip text drawn twice to
    /// fill and then stroke it.
    last_run: Option<(Point, usize)>,
}

impl TextDevice {
    fn push(&mut self, glyph: &Glyph<'_>, transform: Affine) {
        let origin = transform * Point::ORIGIN;
        // Glyph outlines use 1000 units to the em.
        let size = (transform * Point::new(0.0, 1000.0) - origin).hypot();
        let advance = match glyph {
            Glyph::Outline(outline) => outline.advance_width().unwrap_or(500.0),
            Glyph::Type3(_) => 500.0,
        };
        let end = transform * Point::new(f64::from(advance), 0.0);
        let text = match glyph.as_unicode() {
            Some(BfString::Char(ch)) => ch.to_string(),
            Some(BfString::String(text)) => text,
            None => return,
        };
        match self.last {
            Some((last, last_size)) if (origin.y - last.y).abs() <= 0.5 * last_size.max(size) => {
                let gap = origin.x - last.x;
                let line = self.lines.last_mut().expect("a line holds the last glyph");
                if gap > 0.25 * size && !line.text.ends_with(char::is_whitespace) {
                    line.text.push(' ');
                }
            }
            _ => self.lines.push(Line {
                text: String::new(),
                // The page's own space runs down from its top, and a
                // glyph reaches its size above its baseline.
                top: (origin.y - size).max(0.0) as f32,
            }),
        }
        if let Some(line) = self.lines.last_mut() {
            line.text.push_str(&text);
        }
        self.last = Some((end, size));
    }
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}

    fn push_clip_path(&mut self, _: &ClipPath) {}

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        let glyphs = run.glyphs();
        let first = glyphs
            .first()
            .map(|glyph| props.transform * glyph.transform() * Point::ORIGIN);
        let key = first.map(|first| (first, glyphs.len()));
        if matches!(mode, DrawMode::Stroke(_)) && key.is_some() && key == self.last_run {
            return;
        }
        self.last_run = key;
        for glyph in glyphs {
            self.push(glyph, props.transform * glyph.transform());
        }
    }

    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}

    fn pop_clip(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc::channel},
        time::{Duration, Instant},
    };

    use local_core::WorkspaceEngine;

    use super::{
        Line, MAX_SIDE, PAGE_GAP, PdfArea, PdfSession, PdfState, PdfView, RenderGate, TILE_HEIGHT,
        Zone, damaged, find, fold, is_pdf, page_in_link, page_pixels, page_to_open, side_scale,
        tile_pixels,
    };

    #[test]
    fn a_link_names_a_page_with_page_and_a_whole_number() {
        assert_eq!(page_in_link("Doc.pdf#page=7"), Some(7));
        assert_eq!(page_in_link("Doc.pdf#zoom=2&page=3"), Some(3));
        assert_eq!(page_in_link("Doc.pdf#page=2|Second"), Some(2));
        assert_eq!(page_in_link("Doc.pdf#page=0003"), Some(3));
        for target in [
            "Doc.pdf",
            "Doc.pdf#page=",
            "Doc.pdf#page=0",
            "Doc.pdf#page=000",
            "Doc.pdf#page=-1",
            "Doc.pdf#page=1.5",
            "Doc.pdf#page=bad",
            "Doc.pdf#page=-99999999999",
            "Doc.pdf#Heading",
        ] {
            assert_eq!(page_in_link(target), None, "{target}");
        }
    }

    #[test]
    fn a_page_number_too_large_to_hold_is_past_the_end() {
        for target in [
            "Doc.pdf#page=4294967296",
            "Doc.pdf#page=99999999999",
            "Doc.pdf#page=999999999999999999999999999999",
        ] {
            assert_eq!(page_in_link(target), Some(u32::MAX), "{target}");
            assert_eq!(page_to_open(page_in_link(target), 3), 3, "{target}");
        }
    }

    #[test]
    fn a_page_past_the_end_opens_the_last_and_none_opens_the_first() {
        assert_eq!(page_to_open(Some(2), 3), 2);
        assert_eq!(page_to_open(Some(200), 20), 20);
        assert_eq!(page_to_open(None, 20), 1);
        assert_eq!(page_to_open(Some(4), 0), 1);
    }

    #[test]
    fn every_side_draws_at_least_one_pixel_and_at_most_the_largest() {
        // hayro rounds a side's pixels down.
        let pixels = |side: f32, density: f32| (side * side_scale(side, density)) as u16;
        assert_eq!(pixels(612.0, 2.0), 1224);
        assert_eq!(pixels(1.0, 0.25), 1);
        assert_eq!(pixels(1.0, 0.284), 1);
        assert_eq!(pixels(3.0, 0.01), 1);
        assert_eq!(pixels(14400.0, 2.0), MAX_SIDE as u16);
        assert_eq!(pixels(14400.0, 0.0001), 1);
        for density in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!((1..=MAX_SIDE as u16).contains(&pixels(10.0, density)));
        }
    }

    /// A session of the fixture PDF `name`, open in a new workspace, and
    /// the folders that hold the workspace and its app data.
    fn session(name: &str, bytes: &[u8]) -> (PdfSession, [tempfile::TempDir; 2]) {
        let app_data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join(name), bytes).unwrap();
        let engine =
            WorkspaceEngine::open_or_initialize_with_app_data(folder.path(), None, app_data.path())
                .unwrap();
        let session = PdfSession::open(
            Arc::new(engine),
            name,
            Arc::new(|| {}),
            RenderGate::default(),
        );
        (session, [app_data, folder])
    }

    fn wait_until(session: &mut PdfSession, what: &str, done: impl Fn(&PdfView) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            session.poll();
            if done(session.view()) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The 400 dense pages of the large fixture, which take the search a
    /// while.
    const LARGE: &[u8] = include_bytes!("../tests/fixtures/pdf/large.pdf");

    const TEXT: &[u8] = include_bytes!("../tests/fixtures/pdf/text.pdf");

    /// Show `height` points of the session's pages from `top` down, in a
    /// view as wide as the tab, after the jump the view asks for, if any.
    fn show(session: &mut PdfSession, top: f32, height: f32) {
        let jump = session.view().jump();
        let width = session.view().content_size()[0];
        session.show_area(PdfArea {
            left: 0.0,
            top: jump.map_or(top, |jump| jump.top),
            width,
            height,
            jump: jump.map_or(0, |jump| jump.seq),
        });
    }

    /// A view of pages `sizes` laid out in a tab with room for pages
    /// `width` points wide.
    fn laid_out(sizes: &[[f32; 2]], width: f32) -> PdfView {
        let mut view = PdfView::new("doc.pdf");
        view.set_pages(sizes.to_vec());
        view.state = PdfState::Ready;
        view.width = Some(width);
        view
    }

    #[test]
    fn pages_lay_out_one_under_the_other_lined_up_on_their_middles() {
        let view = laid_out(&[[612.0, 792.0], [792.0, 612.0], [612.0, 14000.0]], 396.0);
        // Fit width fits the widest page, 792 points, in 396.
        assert_eq!(view.scale(), 0.5);
        let column = 396.0 + 2.0 * PAGE_GAP;
        let middle = (column - 306.0) / 2.0;
        assert_eq!(view.page_rect(1), Some([middle, PAGE_GAP, 306.0, 396.0]));
        assert_eq!(
            view.page_rect(2),
            Some([PAGE_GAP, 2.0 * PAGE_GAP + 396.0, 396.0, 306.0])
        );
        assert_eq!(
            view.page_rect(3),
            Some([middle, 3.0 * PAGE_GAP + 702.0, 306.0, 7000.0])
        );
        assert_eq!(view.page_rect(4), None);
        assert_eq!(view.content_size(), [column, 4.0 * PAGE_GAP + 7702.0]);
    }

    #[test]
    fn the_pages_between_two_heights_are_those_any_part_of_which_lies_there() {
        // At full size, page n spans 12 + 112 (n - 1) to 100 points below.
        let view = laid_out(&[[100.0, 100.0]; 5], 100.0);
        assert_eq!(view.scale(), 1.0);
        assert_eq!(view.pages_between(0.0, 12.0), 1..1, "the gap above page 1");
        assert_eq!(view.pages_between(0.0, 13.0), 1..2);
        assert_eq!(view.pages_between(112.0, 124.5), 2..3);
        assert_eq!(view.pages_between(50.0, 600.0), 1..6);
        assert_eq!(view.pages_between(1000.0, 2000.0), 6..6);
        // The page at a height takes half of each gap beside it.
        assert_eq!(view.page_at(0.0), 1);
        assert_eq!(view.page_at(117.9), 1);
        assert_eq!(view.page_at(118.0), 2);
        assert_eq!(view.page_at(10_000.0), 5);
    }

    #[test]
    fn a_page_draws_in_tiles_at_most_the_largest_and_at_least_a_pixel() {
        assert_eq!(page_pixels([612.0, 14000.0], 2.0), [1224, 28000]);
        assert_eq!(page_pixels([612.0, 792.0], 2.745), [1680, 2175]);
        // 612 points at 2 × 840 / 612 pixels a point fill 840 points of a
        // 2x screen, which f32 rounds to a hair over 1,680 pixels.
        assert_eq!(page_pixels([612.0, 14000.0], 2.0 * 840.0 / 612.0)[0], 1680);
        assert_eq!(page_pixels([1.0, 1.0], 0.25), [1, 1]);
        assert_eq!(page_pixels([14400.0, 0.0], 64.0), [921_600, 1]);
        let tall = [1224, 28000];
        assert_eq!(tile_pixels(tall, 0, 0), Some([0, 0, 1224, TILE_HEIGHT]));
        assert_eq!(tile_pixels(tall, 0, 27), Some([0, 27_648, 1224, 352]));
        assert_eq!(tile_pixels(tall, 0, 28), None);
        assert_eq!(tile_pixels(tall, 1, 0), None);
        assert_eq!(tile_pixels([5000, 10], 2, 0), Some([4096, 0, 904, 10]));
        assert_eq!(tile_pixels([5000, 10], u32::MAX, 0), None);
    }

    #[test]
    fn the_tiles_over_a_zone_draw_a_pixel_for_each_of_the_screens() {
        let mut view = laid_out(&[[612.0, 14000.0]], 612.0);
        view.pixels_per_point = 2.0;
        assert_eq!(view.density(), 2.0);
        // 700 points from 5,000 points down the page are pixels 10,000 to
        // 11,400: rows 9 to 11.
        let area = PdfArea {
            left: 0.0,
            top: PAGE_GAP + 5000.0,
            width: 636.0,
            height: 700.0,
            jump: 0,
        };
        let keys = view.tiles_over(1, &Zone::around(&area, 0.0));
        let places: Vec<(u32, u32)> = keys.iter().map(|key| (key.column, key.row)).collect();
        assert_eq!(places, [(0, 9), (0, 10), (0, 11)]);
        assert_eq!(
            view.key_zone(&keys[0]),
            Some(Zone {
                left: PAGE_GAP,
                top: PAGE_GAP + 9.0 * 512.0,
                right: PAGE_GAP + 612.0,
                bottom: PAGE_GAP + 10.0 * 512.0,
            })
        );
        assert!(
            view.tiles_over(1, &Zone::around(&area, 0.0))
                .iter()
                .all(|key| key.density() == 2.0)
        );
    }

    #[test]
    fn the_page_draws_while_a_long_search_runs() {
        let (mut session, _folders) = session("large.pdf", LARGE);
        wait_until(&mut session, "the PDF opens", |view| {
            view.state() == &PdfState::Ready
        });
        session.search("no such words");
        // The search is under way when the page is asked for.
        std::thread::sleep(Duration::from_millis(200));

        session.go_to(5);
        session.size(612.0, 1.0);
        show(&mut session, 0.0, 700.0);

        wait_until(&mut session, "page 5 draws", |view| {
            view.tiles(5).next().is_some()
        });
        assert!(session.view().search().searching, "the search ended first");
    }
    #[test]
    fn a_search_stops_when_its_tab_closes() {
        let (mut session, _folders) = session("large.pdf", LARGE);
        let engine = Arc::clone(&session.engine);
        wait_until(&mut session, "the PDF opens", |view| {
            view.state() == &PdfState::Ready
        });
        session.search("no such words");
        std::thread::sleep(Duration::from_millis(200));

        drop(session);

        // The thread lets go of the workspace when it stops.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Arc::strong_count(&engine) > 1 {
            assert!(Instant::now() < deadline, "the thread kept searching");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn try_again_starts_a_new_thread_when_the_tabs_thread_stopped() {
        let (mut session, _folders) = session("text.pdf", TEXT);
        wait_until(&mut session, "the PDF opens", |view| {
            view.state() == &PdfState::Ready
        });
        session.go_to(3);

        // The thread stops, which drops its ends of both channels.
        session.requests = channel().0;
        session.answers = channel().1;
        assert!(session.poll());
        assert_eq!(
            session.view().state(),
            &PdfState::Failed { message: damaged() }
        );

        session.retry();
        assert_eq!(session.view().state(), &PdfState::Opening);
        wait_until(&mut session, "the PDF opens again", |view| {
            view.state() == &PdfState::Ready
        });
        assert_eq!(session.view().page_count(), 3);
        assert_eq!(session.view().page(), 3, "the page you were on stays");
        session.size(612.0, 1.0);
        show(&mut session, 0.0, 700.0);
        assert_eq!(session.view().page(), 3);
        wait_until(&mut session, "the page draws", |view| {
            view.tiles(3).next().is_some()
        });
    }

    #[test]
    fn a_page_that_cant_be_drawn_says_so_and_draws_again_at_another_zoom() {
        let (mut session, _folders) = session("text.pdf", TEXT);
        wait_until(&mut session, "the PDF opens", |view| {
            view.state() == &PdfState::Ready
        });
        session.gate.fail(true);
        session.size(612.0, 1.0);
        show(&mut session, 0.0, 700.0);
        assert!(!session.view().page_failed(1));
        wait_until(&mut session, "the drawing fails", |view| {
            view.page_failed(1)
        });
        assert!(session.view().tiles(1).next().is_none());

        // Showing the same place again asks for nothing, so a page that
        // can't be drawn doesn't keep the thread busy.
        session.gate.fail(false);
        show(&mut session, 0.0, 700.0);
        assert!(session.asked.is_empty());
        assert!(session.view().page_failed(1));

        session.zoom(true);
        assert!(!session.view().page_failed(1), "a new zoom tries again");
        show(&mut session, 0.0, 700.0);
        wait_until(&mut session, "page 1 draws", |view| {
            view.tiles(1).next().is_some()
        });
        assert!(!session.view().page_failed(1));
    }

    #[test]
    fn only_files_ending_in_pdf_are_pdfs() {
        assert!(is_pdf("Docs/Scan.PDF"));
        assert!(is_pdf("a.pdf"));
        assert!(!is_pdf(".pdf"));
        assert!(!is_pdf("pdf"));
        assert!(!is_pdf("Docs.pdf/notes.md"));
    }

    #[test]
    fn a_search_ignores_case_and_how_lines_and_spaces_fall() {
        let lines = [
            Line {
                text: "The quick  brown".to_owned(),
                top: 60.0,
            },
            Line {
                text: "Fox. QUICK fox".to_owned(),
                top: 76.0,
            },
        ];
        let found = find(&lines, &fold("brown fox"), 4);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].page, found[0].snippet.as_str(), found[0].top),
            (4, "The quick  brown", 60.0)
        );
        let found = find(&lines, &fold("Quick"), 4);
        let snippets: Vec<(&str, f32)> = found
            .iter()
            .map(|found| (found.snippet.as_str(), found.top))
            .collect();
        assert_eq!(
            snippets,
            [("The quick  brown", 60.0), ("Fox. QUICK fox", 76.0)]
        );
        assert!(find(&lines, &fold("   "), 4).is_empty());
    }
}
