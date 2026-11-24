//! PDF pages embedded in notes. A note's client lays each embed out from
//! its page's size, then says which part of the page its view covers and
//! how many pixels a point it draws the page at there. Each PDF that notes
//! embed has a thread of its own, the one a PDF tab uses, which reads the
//! PDF once and draws its pages in tiles of at most [`TILE_WIDTH`] by
//! [`TILE_HEIGHT`] pixels: the tiles that show, then those within a view's
//! height of them. So a page of any height draws as sharp as it shows.
//!
//! The tiles more than two views from every part that shows go, and the
//! embeds of every note keep at most [`MAX_EMBED_TILE_BYTES`] of tiles
//! beside those that show. A PDF lets its thread, its copy of the file and
//! its tiles go once no note asked about it for [`PDF_EMBED_RECENT`], or
//! once the last note that showed it closes. It keeps only its pages'
//! sizes, so its embeds lay out the same when they show again.
//!
//! [`TILE_WIDTH`]: crate::TILE_WIDTH
//! [`TILE_HEIGHT`]: crate::TILE_HEIGHT

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

use local_core::WorkspaceEngine;

use crate::pdf::{
    Answer, DRAW_AHEAD, EmbedProblem, KEEP_AHEAD, PdfArea, PdfTile, PdfTileKey, RenderGate,
    Request, TILE_HEIGHT, TILE_WIDTH, Zone, density_of, page_pixels, page_to_open, start_worker,
    tile_pixels,
};

/// The most bytes of tiles the embeds of every note keep beside the tiles
/// that show.
pub const MAX_EMBED_TILE_BYTES: usize = 64 * 1024 * 1024;

/// How long a PDF keeps its thread and its tiles after a note last asked
/// about one of its embeds. An open note asks each frame it draws.
pub(crate) const PDF_EMBED_RECENT: Duration = Duration::from_secs(30);

/// The most tiles the embeds of one PDF ask its thread for at once.
const MAX_ASKED: usize = 64;

/// The most views of one PDF's pages a viewer keeps between two frames,
/// should it never say a frame is done.
const MAX_VIEWS: usize = 64;

/// What an embed of a PDF in a note shows.
#[derive(Debug, Clone, PartialEq)]
pub enum PdfEmbedState {
    /// The PDF opens on another thread.
    Opening,
    /// The page the embed's link names: its number, from 1, and its size
    /// in points as it draws, rotation included.
    Page { number: usize, size: [f32; 2] },
    /// The PDF shows no page, for this reason.
    Problem(EmbedProblem),
}

/// The part of an embed's page a client's view covers, in the page's own
/// points from its top-left corner, and the pixels per point the page
/// draws at. The view may reach past the page, as when the embed is about
/// to scroll in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfEmbedArea {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    /// The screen's pixels per point times the embed's width over the
    /// page's, so each pixel of the page falls on one of the screen's.
    pub density: f32,
}

impl PdfEmbedArea {
    /// The area with a finite place, at least a point each way and a
    /// density within reason.
    fn sanitized(self) -> Self {
        let finite = |value: f32, otherwise: f32| {
            if value.is_finite() { value } else { otherwise }
        };
        Self {
            left: finite(self.left, 0.0),
            top: finite(self.top, 0.0),
            width: finite(self.width, 1.0).max(1.0),
            height: finite(self.height, 1.0).max(1.0),
            density: density_of(self.density, 1.0),
        }
    }

    /// The area, grown by `margin` times its width on each side and its
    /// height above and below.
    fn zone(&self, margin: f32) -> Zone {
        Zone::around(
            &PdfArea {
                left: self.left,
                top: self.top,
                width: self.width,
                height: self.height,
                jump: 0,
            },
            margin,
        )
    }
}

/// One client of the PDF embeds, such as one note's editor in one window:
/// what it shows of each embed, and how to wake it when a tile arrives.
/// Dropping it forgets what it showed.
pub struct PdfEmbedViewer {
    id: u64,
    embeds: Arc<PdfEmbeds>,
}

impl PdfEmbedViewer {
    /// The client drew a frame. The embeds it didn't show in that frame
    /// no longer show, and the tiles far from those it did show go.
    pub fn frame_done(&self) {
        self.embeds.frame_done(self.id);
    }
}

impl Drop for PdfEmbedViewer {
    fn drop(&mut self) {
        self.embeds.forget_viewer(self.id);
    }
}

impl std::fmt::Debug for PdfEmbedViewer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PdfEmbedViewer")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// The PDFs that notes embed, shared by every window's notes.
#[derive(Default)]
pub(crate) struct PdfEmbeds {
    pdfs: Mutex<Pdfs>,
    /// How to wake each viewer, by its number.
    wakers: Mutex<HashMap<u64, Wake>>,
    /// Counts the viewers so far.
    viewers: AtomicU64,
    /// Counts the times an embedded PDF opened with other pages, locked or
    /// failed.
    changes: AtomicU64,
    gate: RenderGate,
}

#[derive(Default)]
struct Pdfs {
    /// Each embedded PDF by its workspace path.
    by_path: HashMap<String, EmbeddedPdf>,
    /// How many bytes of pixels the tiles of every PDF hold.
    tile_bytes: usize,
    /// How many frames each viewer said it drew.
    frames: HashMap<u64, u64>,
}

/// What an embedded PDF opened to.
#[derive(Debug, Clone, PartialEq)]
enum Opened {
    Opening,
    /// Each page's size in points, as it draws.
    Pages(Vec<[f32; 2]>),
    Problem(EmbedProblem),
}

/// A part of a page that a viewer showed in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    viewer: u64,
    page: usize,
    frame: u64,
    area: PdfEmbedArea,
}

/// One PDF that notes embed: its thread, what it opened to, and the tiles
/// drawn for the parts of its pages that show.
struct EmbeddedPdf {
    /// The thread's requests and answers, while it runs.
    thread: Option<(Sender<Request>, Receiver<Answer>)>,
    /// The thread stopped or couldn't start. Another starts after a newer
    /// listing of the files.
    stopped: bool,
    opened: Opened,
    /// When a note last asked about one of its embeds.
    asked: Instant,
    /// The listing of the files the newest check of the file started
    /// under.
    listing: u64,
    tiles: BTreeMap<PdfTileKey, PdfTile>,
    /// The tiles whose newest drawing failed.
    undrawn: BTreeSet<PdfTileKey>,
    /// The tiles in the newest list the thread took that it hasn't drawn.
    drawing: HashSet<PdfTileKey>,
    views: Vec<View>,
}

/// Whether `view` showed in its viewer's frame that ended last or in the
/// one it draws now, of `frames`.
fn live(view: &View, frames: &HashMap<u64, u64>) -> bool {
    view.frame + 1 >= frames.get(&view.viewer).copied().unwrap_or(0)
}

/// Where the tile `key` lies in a page `size` points large, in the page's
/// own points. `None` past the page.
fn key_zone(size: [f32; 2], key: &PdfTileKey) -> Option<Zone> {
    let density = key.density();
    let [x, y, width, height] = tile_pixels(page_pixels(size, density), key.column, key.row)?;
    Some(Zone {
        left: x as f32 / density,
        top: y as f32 / density,
        right: (x + width) as f32 / density,
        bottom: (y + height) as f32 / density,
    })
}

/// The tiles of the page `page`, `size` points large, drawn at `density`,
/// that cover `zone` of it, row by row.
fn tiles_over(page: usize, size: [f32; 2], zone: &Zone, density: f32) -> Vec<PdfTileKey> {
    let pixels = page_pixels(size, density);
    let span = |from: f32, to: f32, tile: u32, count: u32| {
        let clamp = |value: f32| {
            if value.is_nan() {
                0
            } else {
                value.clamp(0.0, count as f32) as u32
            }
        };
        clamp((from * density / tile as f32).floor())..clamp((to * density / tile as f32).ceil())
    };
    let columns = span(
        zone.left,
        zone.right,
        TILE_WIDTH,
        pixels[0].div_ceil(TILE_WIDTH),
    );
    let rows = span(
        zone.top,
        zone.bottom,
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

impl EmbeddedPdf {
    fn new(listing: u64, now: Instant) -> Self {
        Self {
            thread: None,
            stopped: false,
            opened: Opened::Opening,
            asked: now,
            listing,
            tiles: BTreeMap::new(),
            undrawn: BTreeSet::new(),
            drawing: HashSet::new(),
            views: Vec::new(),
        }
    }

    fn page_size(&self, page: usize) -> Option<[f32; 2]> {
        let Opened::Pages(pages) = &self.opened else {
            return None;
        };
        pages.get(page.checked_sub(1)?).copied()
    }

    fn live_views(&self, frames: &HashMap<u64, u64>) -> Vec<View> {
        self.views
            .iter()
            .filter(|view| live(view, frames))
            .copied()
            .collect()
    }

    fn forget_tile(&mut self, key: &PdfTileKey, tile_bytes: &mut usize) {
        if let Some(tile) = self.tiles.remove(key) {
            *tile_bytes -= tile.picture.rgba.len();
        }
    }

    /// Forget every tile, as when the file changes, locks or the thread
    /// stops.
    fn forget_tiles(&mut self, tile_bytes: &mut usize) {
        for tile in self.tiles.values() {
            *tile_bytes -= tile.picture.rgba.len();
        }
        self.tiles.clear();
        self.undrawn.clear();
        self.drawing.clear();
    }

    /// Stop the thread, which drops its copy of the PDF, and forget the
    /// tiles. The pages' sizes stay.
    fn stop(&mut self, tile_bytes: &mut usize) {
        self.thread = None;
        self.forget_tiles(tile_bytes);
    }

    fn set_opened(&mut self, opened: Opened, tile_bytes: &mut usize) -> bool {
        // A new file, or one that locked or failed, draws again from
        // scratch.
        self.forget_tiles(tile_bytes);
        let changed = self.opened != opened;
        self.opened = opened;
        changed
    }

    /// Take in what the thread answered. Returns whether what the embeds
    /// show changed: the pages, or a lock or failure.
    fn poll(&mut self, tile_bytes: &mut usize, frames: &HashMap<u64, u64>) -> bool {
        let Some(thread) = self.thread.take() else {
            return false;
        };
        let mut changed = false;
        let mut running = true;
        loop {
            match thread.1.try_recv() {
                Ok(Answer::Opened { pages }) => {
                    changed |= self.set_opened(Opened::Pages(pages), tile_bytes);
                }
                Ok(Answer::Locked { .. }) => {
                    changed |= self.set_opened(Opened::Problem(EmbedProblem::Locked), tile_bytes);
                }
                Ok(Answer::Failed { .. }) => {
                    changed |=
                        self.set_opened(Opened::Problem(EmbedProblem::Unreadable), tile_bytes);
                }
                Ok(Answer::Tile(tile)) => {
                    self.drawing.remove(&tile.key);
                    // A tile for a density no view draws at any more, or
                    // for a place every view scrolled far from, is too
                    // late to help.
                    if self.wanted_near_a_view(&tile.key, frames) {
                        self.undrawn.remove(&tile.key);
                        *tile_bytes += tile.picture.rgba.len();
                        if let Some(old) = self.tiles.insert(tile.key, tile) {
                            *tile_bytes -= old.picture.rgba.len();
                        }
                    }
                }
                Ok(Answer::TileFailed(key)) => {
                    self.drawing.remove(&key);
                    self.undrawn.insert(key);
                }
                Ok(Answer::Thumbnail(_) | Answer::Searched { .. }) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if running {
            self.thread = Some(thread);
        } else {
            tracing::error!("a PDF embed's thread stopped");
            self.stopped = true;
            self.stop(tile_bytes);
            changed |= self.set_opened(Opened::Problem(EmbedProblem::Unreadable), tile_bytes);
        }
        changed
    }

    /// Whether a view that shows draws at the tile `key`'s density and
    /// keeps the part of the page it covers.
    fn wanted_near_a_view(&self, key: &PdfTileKey, frames: &HashMap<u64, u64>) -> bool {
        let Some(zone) = self
            .page_size(key.page)
            .and_then(|size| key_zone(size, key))
        else {
            return false;
        };
        self.views.iter().any(|view| {
            live(view, frames)
                && view.page == key.page
                && view.area.density.to_bits() == key.density().to_bits()
                && view.area.zone(KEEP_AHEAD).overlaps(&zone)
        })
    }

    /// Forget the tiles no view that shows keeps: those more than two
    /// views from it, and those drawn at another density once the view's
    /// own density covers what shows. A tile whose drawing failed is
    /// tried again once it's out of reach or no view draws at its density.
    fn trim(&mut self, tile_bytes: &mut usize, frames: &HashMap<u64, u64>) {
        let views = self.live_views(frames);
        let covered: Vec<bool> = views
            .iter()
            .map(|view| {
                self.page_size(view.page).is_none_or(|size| {
                    tiles_over(view.page, size, &view.area.zone(0.0), view.area.density)
                        .iter()
                        .all(|key| self.tiles.contains_key(key))
                })
            })
            .collect();
        let keeps = |key: &PdfTileKey, newest_only: bool| {
            let Some(zone) = self
                .page_size(key.page)
                .and_then(|size| key_zone(size, key))
            else {
                return false;
            };
            views.iter().zip(&covered).any(|(view, covered)| {
                let same = view.area.density.to_bits() == key.density().to_bits();
                view.page == key.page
                    && view.area.zone(KEEP_AHEAD).overlaps(&zone)
                    && (same || (!newest_only && !covered && view.area.zone(0.0).overlaps(&zone)))
            })
        };
        let forget: Vec<PdfTileKey> = self
            .tiles
            .keys()
            .filter(|key| !keeps(key, false))
            .copied()
            .collect();
        let retry: Vec<PdfTileKey> = self
            .undrawn
            .iter()
            .filter(|key| !keeps(key, true))
            .copied()
            .collect();
        for key in &forget {
            self.forget_tile(key, tile_bytes);
        }
        for key in &retry {
            self.undrawn.remove(key);
        }
    }

    /// Ask the thread for the tiles the views that show want: those that
    /// show first, from the middle of each view out, then those within a
    /// view's height of them.
    fn ask_tiles(&mut self, frames: &HashMap<u64, u64>) {
        let Some((requests, _)) = &self.thread else {
            return;
        };
        let mut wanted: Vec<(bool, f32, PdfTileKey)> = Vec::new();
        for view in self.views.iter().filter(|view| live(view, frames)) {
            let Some(size) = self.page_size(view.page) else {
                continue;
            };
            let shown = view.area.zone(0.0);
            for key in tiles_over(
                view.page,
                size,
                &view.area.zone(DRAW_AHEAD),
                view.area.density,
            ) {
                if self.tiles.contains_key(&key) || self.undrawn.contains(&key) {
                    continue;
                }
                if let Some(zone) = key_zone(size, &key) {
                    wanted.push((
                        !shown.overlaps(&zone),
                        shown.distance_from_middle(&zone),
                        key,
                    ));
                }
            }
        }
        wanted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut keys = HashSet::new();
        wanted.retain(|(_, _, key)| keys.insert(*key));
        wanted.truncate(MAX_ASKED);
        let keys: HashSet<PdfTileKey> = wanted.iter().map(|(_, _, key)| *key).collect();
        if keys != self.drawing {
            // A thread that stopped has answered why, which the next poll
            // takes in.
            let _ = requests.send(Request::Tiles(
                wanted.into_iter().map(|(_, _, key)| key).collect(),
            ));
            self.drawing = keys;
        }
    }
}

impl PdfEmbeds {
    fn lock(&self) -> MutexGuard<'_, Pdfs> {
        self.pdfs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new viewer, which `wake` wakes whenever a tile arrives.
    pub(crate) fn viewer(self: &Arc<Self>, wake: Wake) -> PdfEmbedViewer {
        let id = self.viewers.fetch_add(1, Ordering::SeqCst) + 1;
        self.wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, wake);
        PdfEmbedViewer {
            id,
            embeds: Arc::clone(self),
        }
    }

    /// Changes whenever an embedded PDF opens with other pages, locks or
    /// fails.
    pub(crate) fn seq(&self) -> u64 {
        self.changes.load(Ordering::SeqCst)
    }

    /// Wake every viewer.
    fn wake(&self) {
        let wakers: Vec<Wake> = self
            .wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for wake in wakers {
            wake();
        }
    }

    /// Keep only the PDFs whose paths `paths` holds, as a listing of the
    /// files shows them.
    pub(crate) fn retain(&self, paths: &HashSet<String>) {
        let mut pdfs = self.lock();
        let Pdfs {
            by_path,
            tile_bytes,
            ..
        } = &mut *pdfs;
        by_path.retain(|path, pdf| {
            let keep = paths.contains(path);
            if !keep {
                pdf.stop(tile_bytes);
            }
            keep
        });
    }

    /// The embedded PDF at `path`, asked about now under the listing
    /// `listing`: its thread starts if it isn't running, checks the file
    /// after a newer listing, and its answers come in.
    fn ask<'a>(
        self: &Arc<Self>,
        pdfs: &'a mut Pdfs,
        engine: &Arc<WorkspaceEngine>,
        path: &str,
        listing: u64,
    ) -> &'a mut EmbeddedPdf {
        let now = Instant::now();
        let Pdfs {
            by_path,
            tile_bytes,
            frames,
        } = pdfs;
        stop_idle(by_path, tile_bytes, now);
        if !by_path.contains_key(path) {
            by_path.insert(path.to_owned(), EmbeddedPdf::new(listing, now));
        }
        let pdf = by_path.get_mut(path).expect("inserted above");
        pdf.asked = now;
        if pdf.listing != listing {
            pdf.listing = listing;
            pdf.stopped = false;
            if let Some((requests, _)) = &pdf.thread {
                let _ = requests.send(Request::Check);
            }
        }
        if pdf.thread.is_none() && !pdf.stopped {
            let embeds = Arc::downgrade(self);
            let wake: Wake = Arc::new(move || {
                if let Some(embeds) = embeds.upgrade() {
                    embeds.wake();
                }
            });
            match start_worker(engine, path, &wake, &self.gate) {
                Ok(thread) => pdf.thread = Some(thread),
                Err(error) => {
                    // The next listing tries again. The pages' sizes stay
                    // meanwhile, or the embed shows the missing-image
                    // marker.
                    tracing::warn!(%error, "a PDF embed's thread didn't start");
                    pdf.stopped = true;
                    if pdf.opened == Opened::Opening {
                        pdf.opened = Opened::Problem(EmbedProblem::Unreadable);
                        self.changes.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
        if pdf.poll(tile_bytes, frames) {
            self.changes.fetch_add(1, Ordering::SeqCst);
        }
        pdf
    }

    /// What the embed of the page `asked` names in the PDF at `path`
    /// shows, under the files' listing `listing`.
    pub(crate) fn state(
        self: &Arc<Self>,
        engine: &Arc<WorkspaceEngine>,
        path: &str,
        asked: Option<u32>,
        listing: u64,
    ) -> PdfEmbedState {
        let mut pdfs = self.lock();
        let pdf = self.ask(&mut pdfs, engine, path, listing);
        match &pdf.opened {
            Opened::Opening => PdfEmbedState::Opening,
            Opened::Problem(problem) => PdfEmbedState::Problem(*problem),
            Opened::Pages(pages) => {
                let number = page_to_open(asked, pages.len());
                match pages.get(number - 1) {
                    Some(size) => PdfEmbedState::Page {
                        number,
                        size: *size,
                    },
                    None => PdfEmbedState::Problem(EmbedProblem::Unreadable),
                }
            }
        }
    }

    /// The viewer `viewer` shows `area` of the page `asked` names in the
    /// PDF at `path`: draw the tiles there and near there. Returns every
    /// tile kept of the page, those at `area`'s density last, so a client
    /// that draws them in order puts the sharpest on top.
    pub(crate) fn tiles(
        self: &Arc<Self>,
        engine: &Arc<WorkspaceEngine>,
        path: &str,
        asked: Option<u32>,
        listing: u64,
        viewer: &PdfEmbedViewer,
        area: PdfEmbedArea,
    ) -> Vec<PdfTile> {
        if !Arc::ptr_eq(&viewer.embeds, self) {
            return Vec::new();
        }
        let area = area.sanitized();
        let mut pdfs = self.lock();
        let frame = pdfs.frames.get(&viewer.id).copied().unwrap_or(0);
        let pdf = self.ask(&mut pdfs, engine, path, listing);
        let Opened::Pages(pages) = &pdf.opened else {
            return Vec::new();
        };
        let page = page_to_open(asked, pages.len());
        let view = View {
            viewer: viewer.id,
            page,
            frame,
            area,
        };
        if !pdf.views.contains(&view) {
            pdf.views.push(view);
            let mine = pdf
                .views
                .iter()
                .filter(|view| view.viewer == viewer.id)
                .count();
            if mine > MAX_VIEWS
                && let Some(oldest) = pdf.views.iter().position(|view| view.viewer == viewer.id)
            {
                pdf.views.remove(oldest);
            }
        }
        let Pdfs {
            by_path,
            tile_bytes,
            frames,
        } = &mut *pdfs;
        let pdf = by_path.get_mut(path).expect("asked above");
        pdf.ask_tiles(frames);
        cap(by_path, tile_bytes, frames, MAX_EMBED_TILE_BYTES);
        let pdf = by_path.get(path).expect("asked above");
        let density = area.density.to_bits();
        let mut tiles: Vec<PdfTile> = pdf
            .tiles
            .range(PdfTileKey::of_page(page))
            .map(|(_, tile)| tile.clone())
            .collect();
        tiles.sort_by_key(|tile| tile.key.density().to_bits() == density);
        tiles
    }

    /// The viewer `viewer` drew a frame: what it didn't show in it no
    /// longer shows.
    fn frame_done(&self, viewer: u64) {
        let mut pdfs = self.lock();
        let Pdfs {
            by_path,
            tile_bytes,
            frames,
        } = &mut *pdfs;
        let frame = frames.entry(viewer).or_insert(0);
        let done = *frame;
        *frame += 1;
        for pdf in by_path.values_mut() {
            pdf.views
                .retain(|view| view.viewer != viewer || view.frame >= done);
            pdf.trim(tile_bytes, frames);
            pdf.ask_tiles(frames);
        }
        cap(by_path, tile_bytes, frames, MAX_EMBED_TILE_BYTES);
    }

    /// The viewer `viewer` went, as when its note closed: forget what it
    /// showed. A PDF no viewer shows lets its thread and tiles go.
    fn forget_viewer(&self, viewer: u64) {
        self.wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&viewer);
        let mut pdfs = self.lock();
        let Pdfs {
            by_path,
            tile_bytes,
            frames,
        } = &mut *pdfs;
        frames.remove(&viewer);
        for pdf in by_path.values_mut() {
            pdf.views.retain(|view| view.viewer != viewer);
            if pdf.views.is_empty() {
                pdf.stop(tile_bytes);
            } else {
                pdf.trim(tile_bytes, frames);
                pdf.ask_tiles(frames);
            }
        }
    }

    /// How many bytes of pixels the tiles of every embedded PDF hold.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn tile_bytes(&self) -> usize {
        self.lock().tile_bytes
    }

    /// How many embedded PDFs have a thread running.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn threads(&self) -> usize {
        self.lock()
            .by_path
            .values()
            .filter(|pdf| pdf.thread.is_some())
            .count()
    }
}

/// The PDFs no note asked about for [`PDF_EMBED_RECENT`] before `now`
/// let their threads and tiles go, and forget what showed of them.
fn stop_idle(by_path: &mut HashMap<String, EmbeddedPdf>, tile_bytes: &mut usize, now: Instant) {
    for pdf in by_path.values_mut() {
        if pdf.thread.is_some() && now.saturating_duration_since(pdf.asked) >= PDF_EMBED_RECENT {
            pdf.stop(tile_bytes);
            pdf.views.clear();
        }
    }
}

/// Past `most` bytes, forget the tiles that don't show, farthest from a
/// view that shows first, and those of a PDF no view shows before them.
fn cap(
    by_path: &mut HashMap<String, EmbeddedPdf>,
    tile_bytes: &mut usize,
    frames: &HashMap<u64, u64>,
    most: usize,
) {
    if *tile_bytes <= most {
        return;
    }
    let mut far: Vec<(f32, String, PdfTileKey)> = Vec::new();
    for (path, pdf) in by_path.iter() {
        let views = pdf.live_views(frames);
        for key in pdf.tiles.keys() {
            let Some(zone) = pdf.page_size(key.page).and_then(|size| key_zone(size, key)) else {
                far.push((f32::INFINITY, path.clone(), *key));
                continue;
            };
            let mut shows = false;
            let mut distance = f32::INFINITY;
            for view in views.iter().filter(|view| view.page == key.page) {
                let shown = view.area.zone(0.0);
                shows |= shown.overlaps(&zone);
                distance = distance.min(shown.distance_from_middle(&zone));
            }
            if !shows {
                far.push((distance, path.clone(), *key));
            }
        }
    }
    far.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, path, key) in far {
        if *tile_bytes <= most {
            break;
        }
        if let Some(pdf) = by_path.get_mut(&path) {
            pdf.forget_tile(&key, tile_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, mpsc::channel},
        time::Instant,
    };

    use super::{
        EmbeddedPdf, Opened, PDF_EMBED_RECENT, PdfEmbedArea, View, cap, key_zone, stop_idle,
        tiles_over,
    };
    use crate::pdf::{PdfPicture, PdfTile, PdfTileKey, TILE_HEIGHT, page_pixels, tile_pixels};

    /// A PDF of one page 10 by 14,000 points, 14 rows of tiles at one
    /// pixel a point, with every row drawn and a view of its top 700
    /// points, which only row 0 covers, in frame 0 of viewer 1.
    fn drawn_pdf(tile_bytes: &mut usize) -> EmbeddedPdf {
        let size = [10.0, 14000.0];
        let mut pdf = EmbeddedPdf::new(0, Instant::now());
        pdf.opened = Opened::Pages(vec![size]);
        pdf.views.push(View {
            viewer: 1,
            page: 1,
            frame: 0,
            area: PdfEmbedArea {
                left: 0.0,
                top: 0.0,
                width: 10.0,
                height: 700.0,
                density: 1.0,
            },
        });
        for row in 0..14 {
            let key = PdfTileKey::new(1, 1.0, 0, row);
            let [x, y, width, height] = tile_pixels(page_pixels(size, 1.0), 0, row).unwrap();
            let bytes = (width * height * 4) as usize;
            *tile_bytes += bytes;
            pdf.tiles.insert(
                key,
                PdfTile {
                    key,
                    x,
                    y,
                    picture: PdfPicture {
                        page: 1,
                        width,
                        height,
                        rgba: Arc::from(vec![255; bytes]),
                        generation: 0,
                    },
                },
            );
        }
        pdf
    }

    #[test]
    fn past_the_most_bytes_the_tiles_farthest_from_what_shows_go_first() {
        let mut tile_bytes = 0;
        let mut by_path = HashMap::new();
        by_path.insert("shown.pdf".to_owned(), drawn_pdf(&mut tile_bytes));
        // A PDF no view shows any more.
        let mut gone = drawn_pdf(&mut tile_bytes);
        gone.views.clear();
        by_path.insert("gone.pdf".to_owned(), gone);
        let tile = 10 * TILE_HEIGHT as usize * 4;
        let frames = HashMap::from([(1, 0)]);

        cap(&mut by_path, &mut tile_bytes, &frames, 4 * tile);

        assert!(by_path["gone.pdf"].tiles.is_empty());
        let rows: Vec<u32> = by_path["shown.pdf"]
            .tiles
            .keys()
            .map(|key| key.row)
            .collect();
        assert_eq!(rows, [0, 1, 2, 3]);
        assert_eq!(tile_bytes, 4 * tile);

        // The tiles that show stay past the most bytes.
        cap(&mut by_path, &mut tile_bytes, &frames, 0);
        let rows: Vec<u32> = by_path["shown.pdf"]
            .tiles
            .keys()
            .map(|key| key.row)
            .collect();
        assert_eq!(rows, [0]);
    }

    #[test]
    fn a_pdf_no_note_asked_about_lately_lets_its_thread_and_tiles_go() {
        let mut tile_bytes = 0;
        let mut pdf = drawn_pdf(&mut tile_bytes);
        let (requests, _) = channel();
        let (_, answers) = channel();
        pdf.thread = Some((requests, answers));
        let asked = pdf.asked;
        let mut by_path = HashMap::from([("tall.pdf".to_owned(), pdf)]);

        stop_idle(&mut by_path, &mut tile_bytes, asked + PDF_EMBED_RECENT / 2);
        assert!(by_path["tall.pdf"].thread.is_some());

        stop_idle(&mut by_path, &mut tile_bytes, asked + PDF_EMBED_RECENT);
        let pdf = &by_path["tall.pdf"];
        assert!(pdf.thread.is_none() && pdf.tiles.is_empty() && pdf.views.is_empty());
        assert_eq!(tile_bytes, 0);
        // Its embeds lay out as before, and a new ask starts a thread.
        assert_eq!(pdf.opened, Opened::Pages(vec![[10.0, 14000.0]]));
        assert!(!pdf.stopped);
    }

    #[test]
    fn the_tiles_over_a_part_of_a_page_draw_it_at_the_views_density() {
        // A page 612 by 14,000 points that shows 840 points wide on a 2x
        // screen draws at 2.745 pixels a point: 1,680 pixels wide.
        let size = [612.0, 14000.0];
        let density = 2.0 * 840.0 / 612.0;
        let area = PdfEmbedArea {
            left: 0.0,
            top: 5000.0,
            width: 612.0,
            height: 300.0,
            density,
        };
        let keys = tiles_over(1, size, &area.zone(0.0), density);
        let rows: Vec<u32> = keys.iter().map(|key| key.row).collect();
        // Points 5,000 to 5,300 are pixels 13,725 to 14,549: rows 13 and 14.
        assert_eq!(rows, [13, 14]);
        assert!(keys.iter().all(|key| key.column == 0));
        let zone = key_zone(size, &keys[0]).unwrap();
        assert!((zone.top - 13.0 * TILE_HEIGHT as f32 / density).abs() < 1e-3);
        assert!((zone.right - 612.0).abs() < 1e-3);

        // A view above the page or below it covers none of it, and one
        // 100 points above it comes within a view of its top.
        let above = PdfEmbedArea {
            top: -400.0,
            ..area
        };
        assert!(tiles_over(1, size, &above.zone(0.0), density).is_empty());
        let near: Vec<u32> = tiles_over(1, size, &above.zone(1.0), density)
            .iter()
            .map(|key| key.row)
            .collect();
        assert_eq!(near, [0]);
        let below = PdfEmbedArea {
            top: 15000.0,
            ..area
        };
        assert!(tiles_over(1, size, &below.zone(0.0), density).is_empty());
        assert!(key_zone(size, &PdfTileKey::new(1, density, 0, 999)).is_none());
    }
}
