//! PDF files in tabs and in notes. hayro parses a PDF, unlocks it with its
//! password and draws its pages; a device of Noura's own collects the text
//! of each page for search. Each PDF tab has a thread of its own that holds
//! the parsed document, so opening, drawing, searching and unlocking never
//! hold up the thread that draws the window. Bytes come through
//! `local-core`'s checks. Noura logs, saves and shows a password nowhere,
//! and wipes its own copy once hayro has tried it. Copies that hayro makes
//! while it tries one, or that a client's text field leaves behind, are
//! freed without being wiped.

use std::{
    collections::{HashMap, HashSet},
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
    vello_cpu::color::palette::css::WHITE,
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

/// The widest or tallest Noura draws a page, in pixels. A page zoomed past
/// it draws at this size and stretches.
const MAX_SIDE: f32 = 4096.0;

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
    /// As wide as the tab.
    FitWidth,
    /// At this scale of the page's own size, one of [`ZOOM_STEPS`].
    Scale(f32),
}

/// A page drawn to pixels, on a white background.
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

/// One place a search found its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfMatch {
    /// The page's number, from 1.
    pub page: usize,
    /// The line the match is on.
    pub snippet: String,
}

/// A PDF tab's search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

/// What a PDF tab shows: the PDF's pages, where you are in them and how
/// large they draw, its thumbnails and its search.
#[derive(Debug, Clone)]
pub struct PdfView {
    path: String,
    state: PdfState,
    /// Each page's size in points, as it draws, rotation included.
    pages: Vec<[f32; 2]>,
    page: usize,
    zoom: PdfZoom,
    /// How wide the tab draws the page, in points, as its client said.
    width: Option<f32>,
    thumbnails_shown: bool,
    picture: Option<PdfPicture>,
    thumbnails: HashMap<usize, PdfPicture>,
    search: PdfSearch,
    /// The page whose newest drawing failed.
    undrawn: Option<usize>,
}

impl PdfView {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            state: PdfState::Opening,
            pages: Vec::new(),
            page: 1,
            zoom: PdfZoom::FitWidth,
            width: None,
            thumbnails_shown: false,
            picture: None,
            thumbnails: HashMap::new(),
            search: PdfSearch::default(),
            undrawn: None,
        }
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

    /// The page that shows, from 1.
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

    /// The scale the page draws at: the zoom's, or for Fit width, the
    /// tab's width over the page's.
    pub fn scale(&self) -> f32 {
        match self.zoom {
            PdfZoom::Scale(scale) => scale,
            PdfZoom::FitWidth => {
                let page_width = self.page_size(self.page).map_or(0.0, |size| size[0]);
                match self.width {
                    Some(width) if page_width > 0.0 => {
                        (width / page_width).clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1])
                    }
                    _ => 1.0,
                }
            }
        }
    }

    /// The newest drawing of a page this tab asked for. It may show
    /// another page or another size than the tab shows now, until the
    /// drawing it asked for since arrives.
    pub fn picture(&self) -> Option<&PdfPicture> {
        self.picture.as_ref()
    }

    /// Whether the newest drawing of the page that shows failed, so no
    /// picture of it is coming.
    pub fn page_failed(&self) -> bool {
        self.undrawn == Some(self.page)
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

    /// The pixels per point the page that shows draws at, for a screen
    /// with `pixels_per_point`, kept within [`MAX_SIDE`].
    fn density(&self, page: usize, scale: f32, pixels_per_point: f32) -> f32 {
        let [width, height] = self.page_size(page).unwrap_or([1.0, 1.0]);
        let wanted = scale * pixels_per_point;
        let largest = MAX_SIDE / width.max(height).max(1.0);
        wanted.min(largest).max(0.01)
    }
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

/// Holds every PDF thread before it draws a page, or makes each drawing
/// fail, so tests can watch the window keep drawing while a page renders
/// and see what a page that can't be drawn shows.
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

/// Holds every PDF thread before it draws a page, until dropped.
#[cfg(feature = "test-hooks")]
pub struct PdfRenderHold(pub(crate) RenderGate);

#[cfg(feature = "test-hooks")]
impl Drop for PdfRenderHold {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Makes every drawing of a page fail, until dropped.
#[cfg(feature = "test-hooks")]
pub struct PdfDrawFailure(pub(crate) RenderGate);

#[cfg(feature = "test-hooks")]
impl Drop for PdfDrawFailure {
    fn drop(&mut self) {
        self.0.fail(false);
    }
}

/// Which drawing of a page a thread makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Page,
    Thumbnail,
}

enum Request {
    Unlock(Zeroizing<String>),
    Render {
        kind: Kind,
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
    Rendered {
        kind: Kind,
        picture: PdfPicture,
    },
    /// The page `page` couldn't be drawn.
    NotDrawn {
        kind: Kind,
        page: usize,
    },
    Searched {
        generation: u64,
        matches: Vec<PdfMatch>,
    },
}

/// Counts the pictures drawn so far.
static PICTURES: AtomicU64 = AtomicU64::new(0);

/// One PDF tab: what it shows, and its thread.
pub(crate) struct PdfSession {
    view: PdfView,
    requests: Sender<Request>,
    answers: Receiver<Answer>,
    /// The drawing of the page last asked for: the page and the density.
    asked: Option<(usize, f32)>,
    asked_thumbnails: HashSet<usize>,
    search_generation: u64,
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
            asked: None,
            asked_thumbnails: HashSet::new(),
            search_generation: 0,
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
            answers: send_answers,
            wake: self.wake.clone(),
            gate: self.gate.clone(),
        };
        self.requests = requests;
        self.answers = answers;
        self.asked = None;
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

    fn apply(&mut self, answer: Answer) {
        match answer {
            Answer::Opened { pages } => {
                let count = pages.len();
                self.view.pages = pages;
                self.view.state = PdfState::Ready;
                self.view.page = match self.pending_page.take() {
                    Some(asked) => page_to_open(asked, count),
                    None => self.view.page.clamp(1, count.max(1)),
                };
                // A new file draws again from scratch.
                self.view.picture = None;
                self.view.thumbnails.clear();
                self.view.undrawn = None;
                self.asked = None;
                self.asked_thumbnails.clear();
                let query = std::mem::take(&mut self.view.search.query);
                self.search(&query);
            }
            Answer::Locked { wrong } => {
                self.view.state = PdfState::Locked { wrong };
                self.view.picture = None;
                self.view.thumbnails.clear();
            }
            Answer::Failed { message } => {
                self.view.state = PdfState::Failed { message };
                self.view.picture = None;
                self.view.thumbnails.clear();
            }
            Answer::Rendered { kind, picture } => match kind {
                Kind::Page => {
                    self.view.undrawn = None;
                    self.view.picture = Some(picture);
                }
                Kind::Thumbnail => {
                    self.view.thumbnails.insert(picture.page, picture);
                }
            },
            // A thumbnail that can't be drawn stays a blank page.
            Answer::NotDrawn { kind, page } => {
                if kind == Kind::Page {
                    self.view.undrawn = Some(page);
                }
            }
            Answer::Searched {
                generation,
                matches,
            } if generation == self.search_generation => {
                self.view.search.searching = false;
                self.view.search.current = (!matches.is_empty()).then_some(0);
                if let Some(first) = matches.first() {
                    self.view.page = first.page;
                }
                self.view.search.matches = matches;
            }
            Answer::Searched { .. } => {}
        }
    }

    /// Show the page `page`, or the nearest page there is.
    pub(crate) fn go_to(&mut self, page: usize) -> bool {
        if self.view.state != PdfState::Ready {
            return false;
        }
        let page = page.clamp(1, self.view.page_count().max(1));
        let changed = page != self.view.page;
        self.view.page = page;
        changed
    }

    /// Show the page a link named, once the page count is known.
    pub(crate) fn go_to_link_page(&mut self, asked: Option<u32>) {
        if self.view.state == PdfState::Ready {
            self.view.page = page_to_open(asked, self.view.page_count());
        } else {
            self.pending_page = Some(asked);
        }
    }

    pub(crate) fn step_page(&mut self, forward: bool) -> bool {
        let page = if forward {
            self.view.page.saturating_add(1)
        } else {
            self.view.page.saturating_sub(1)
        };
        self.go_to(page)
    }

    /// Zoom one step in or out from the scale the page draws at.
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
        let zoom = PdfZoom::Scale(next);
        let changed = zoom != self.view.zoom;
        self.view.zoom = zoom;
        changed
    }

    pub(crate) fn fit_width(&mut self) -> bool {
        let changed = self.view.zoom != PdfZoom::FitWidth;
        self.view.zoom = PdfZoom::FitWidth;
        changed
    }

    /// The tab draws the page `width` points wide at most, on a screen
    /// with `pixels_per_point`: ask for the drawing it needs.
    pub(crate) fn size(&mut self, width: f32, pixels_per_point: f32) {
        if width.is_finite() && width > 0.0 {
            self.view.width = Some(width);
        }
        if self.view.state != PdfState::Ready {
            return;
        }
        let page = self.view.page;
        let density = self.view.density(page, self.view.scale(), pixels_per_point);
        let same = self
            .asked
            .is_some_and(|(asked, at)| asked == page && (at - density).abs() < 0.01 * at);
        if !same {
            self.asked = Some((page, density));
            self.view.undrawn = None;
            self.send(Request::Render {
                kind: Kind::Page,
                page,
                density,
            });
        }
    }

    /// Draw the thumbnail of the page `page`, for a screen with
    /// `pixels_per_point`, unless it's drawn or asked for already.
    pub(crate) fn want_thumbnail(&mut self, page: usize, pixels_per_point: f32) {
        if self.view.state != PdfState::Ready
            || self.view.page_size(page).is_none()
            || !self.asked_thumbnails.insert(page)
        {
            return;
        }
        let width = self.view.page_size(page).map_or(1.0, |size| size[0]);
        let density = self
            .view
            .density(page, THUMBNAIL_WIDTH / width.max(1.0), pixels_per_point);
        self.send(Request::Render {
            kind: Kind::Thumbnail,
            page,
            density,
        });
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
        self.view.page = self.view.search.matches[current].page;
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
    /// Requests taken from the channel and not handled yet.
    pending: Vec<Request>,
    answers: Sender<Answer>,
    wake: Arc<dyn Fn() + Send + Sync>,
    gate: RenderGate,
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

    /// The next request: a waiting one first, by priority, or the next to
    /// arrive. `None` once the tab closed.
    fn next(&mut self) -> Option<Request> {
        self.pending.extend(self.requests.try_iter());
        if self.pending.is_empty() {
            let request = self.requests.recv().ok()?;
            self.pending.push(request);
            self.pending.extend(self.requests.try_iter());
        }
        // Only the newest drawing of the page and the newest search count.
        let newest_page = self.pending.iter().rposition(|request| {
            matches!(
                request,
                Request::Render {
                    kind: Kind::Page,
                    ..
                }
            )
        });
        let newest_search = self
            .pending
            .iter()
            .rposition(|request| matches!(request, Request::Search { .. }));
        let mut index = 0;
        self.pending.retain(|request| {
            let keep = match request {
                Request::Render {
                    kind: Kind::Page, ..
                } => Some(index) == newest_page,
                Request::Search { .. } => Some(index) == newest_search,
                _ => true,
            };
            index += 1;
            keep
        });
        let rank = |request: &Request| match request {
            Request::Unlock(_) | Request::Check | Request::Retry | Request::Moved(_) => 0,
            Request::Render {
                kind: Kind::Page, ..
            } => 1,
            Request::Search { .. } => 2,
            Request::Render {
                kind: Kind::Thumbnail,
                ..
            } => 3,
        };
        let first = (0..self.pending.len()).min_by_key(|index| rank(&self.pending[*index]))?;
        Some(self.pending.remove(first))
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
                Some(Request::Unlock(password)) => return Next::Read(Some(password)),
                Some(Request::Retry) => return Next::Read(None),
                Some(Request::Check) if self.changed(stamp) => return Next::Read(None),
                Some(Request::Moved(path)) => self.path = path,
                Some(_) => {}
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
                Some(Request::Render {
                    kind,
                    page,
                    density,
                }) => {
                    let shown = page.checked_sub(1).and_then(|index| pdf.pages().get(index));
                    self.gate.wait();
                    let drawn = shown
                        .filter(|_| !self.gate.failing())
                        .and_then(|shown| draw(shown, &cache, page, density));
                    match drawn {
                        Some(picture) => self.answer(Answer::Rendered { kind, picture }),
                        None => {
                            tracing::warn!("a PDF page couldn't be drawn");
                            self.answer(Answer::NotDrawn { kind, page });
                        }
                    }
                }
                Some(Request::Search { generation, query }) => {
                    let mut matches = Vec::new();
                    let needle = fold(&query);
                    for (index, page) in pdf.pages().iter().enumerate() {
                        // A newer search replaces this one.
                        self.pending.extend(self.requests.try_iter());
                        if self
                            .pending
                            .iter()
                            .any(|request| matches!(request, Request::Search { .. }))
                        {
                            break;
                        }
                        let lines = texts[index].get_or_insert_with(|| page_text(page));
                        matches.extend(find(lines, &needle, index + 1));
                    }
                    self.answer(Answer::Searched {
                        generation,
                        matches,
                    });
                }
                Some(Request::Check | Request::Retry) if self.changed(stamp) => {
                    return Next::Read(None);
                }
                Some(Request::Moved(path)) => self.path = path,
                Some(_) => {}
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

/// Draw `page`, number `number`, at `density` pixels per point, or on a
/// side that would draw less than a pixel, one pixel. `None` when hayro
/// fails.
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
        generation: PICTURES.fetch_add(1, Ordering::Relaxed),
    })
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
            let snippet = lines[line_of[start]].text.trim().to_owned();
            found.push(PdfMatch { page, snippet });
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
            _ => self.lines.push(Line::default()),
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
        Line, MAX_SIDE, PdfSession, PdfState, PdfView, RenderGate, damaged, find, fold, is_pdf,
        page_in_link, page_to_open, side_scale,
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
        let engine = WorkspaceEngine::open_or_initialize_with_app_data(
            folder.path(),
            "Test",
            None,
            app_data.path(),
        )
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

    #[test]
    fn try_again_starts_a_new_thread_when_the_tabs_thread_stopped() {
        let (mut session, _folders) =
            session("text.pdf", include_bytes!("../tests/fixtures/pdf/text.pdf"));
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
        wait_until(&mut session, "the page draws", |view| {
            view.picture().is_some()
        });
    }

    #[test]
    fn a_page_that_cant_be_drawn_says_so_and_draws_when_asked_again() {
        let (mut session, _folders) =
            session("text.pdf", include_bytes!("../tests/fixtures/pdf/text.pdf"));
        wait_until(&mut session, "the PDF opens", |view| {
            view.state() == &PdfState::Ready
        });
        session.gate.fail(true);
        session.size(612.0, 1.0);
        assert!(!session.view().page_failed());
        wait_until(&mut session, "the drawing fails", PdfView::page_failed);
        assert!(session.view().picture().is_none());

        session.gate.fail(false);
        session.step_page(true);
        assert!(!session.view().page_failed(), "page 2 hasn't failed");
        session.size(612.0, 1.0);
        wait_until(&mut session, "page 2 draws", |view| {
            view.picture().is_some_and(|picture| picture.page == 2)
        });
        session.step_page(false);
        session.size(612.0, 1.0);
        assert!(!session.view().page_failed(), "page 1 draws again");
        wait_until(&mut session, "page 1 draws", |view| {
            view.picture().is_some_and(|picture| picture.page == 1)
        });
        assert!(!session.view().page_failed());
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
            },
            Line {
                text: "Fox. QUICK fox".to_owned(),
            },
        ];
        let found = find(&lines, &fold("brown fox"), 4);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].page, found[0].snippet.as_str()),
            (4, "The quick  brown")
        );
        let found = find(&lines, &fold("Quick"), 4);
        let snippets: Vec<&str> = found.iter().map(|found| found.snippet.as_str()).collect();
        assert_eq!(snippets, ["The quick  brown", "Fox. QUICK fox"]);
        assert!(find(&lines, &fold("   "), 4).is_empty());
    }
}
