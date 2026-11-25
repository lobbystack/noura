//! The file viewers of a window's tabs: each PDF tab's session, from
//! `pdf`, and each image tab's bytes. A viewer lives as long as its tab,
//! so the page and zoom stay while you look at another tab, and it reads
//! its file again when another app replaces it. Images in the formats a
//! note draws inline open in an image tab; files that open in no tab go
//! to the system's default app.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    time::SystemTime,
};

use local_core::WorkspaceEngine;

use crate::{
    AppError, MAX_IMAGE_BYTES, TabId,
    error::Access,
    links::is_image,
    notes::opens_in_editor,
    pdf::{PdfSession, RenderGate, is_pdf},
};

/// Whether the file at `path` opens in a tab: a note, a text or code
/// file, a PDF or an image. Any other file goes to the system's default
/// app.
pub(crate) fn opens_in_tab(path: &str) -> bool {
    opens_in_editor(path) || is_pdf(path) || is_image(path)
}

/// Extensions the system would run, install or follow elsewhere rather
/// than show. Handing one from a workspace someone else wrote to the
/// default app would run their code. Ported from the legacy Tauri app's
/// file actions, with the macOS location files, Java Web Start,
/// configuration profiles and the Windows app installers added.
const RUNNABLE_EXTENSIONS: &[&str] = &[
    "app",
    "appimage",
    "applescript",
    "application",
    "appx",
    "appxbundle",
    "bash",
    "bat",
    "bin",
    "cmd",
    "com",
    "command",
    "cpl",
    "csh",
    "deb",
    "desktop",
    "dmg",
    "exe",
    "fileloc",
    "fish",
    "gadget",
    "hta",
    "inetloc",
    "inf",
    "ins",
    "jar",
    "jnlp",
    "js",
    "jse",
    "ksh",
    "lnk",
    "mobileconfig",
    "mpkg",
    "msc",
    "msi",
    "msix",
    "msixbundle",
    "msp",
    "pif",
    "pkg",
    "ps1",
    "psm1",
    "py",
    "pyw",
    "rb",
    "reg",
    "rpm",
    "run",
    "scf",
    "scpt",
    "scptd",
    "scr",
    "sh",
    "shortcut",
    "terminal",
    "tool",
    "url",
    "vb",
    "vbe",
    "vbs",
    "webloc",
    "workflow",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "zsh",
];

/// Whether the system's default app could run code from the file at
/// `path`, whose own metadata, not its link's, is `metadata`: an app,
/// script, installer or shortcut by its extension, or, on Unix, a file
/// anyone may execute, which macOS opens in Terminal and runs.
pub(crate) fn could_run_code(path: &std::path::Path, metadata: &std::fs::Metadata) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy());
    // Windows drops the dots and spaces that end a name when it opens the
    // file, so `setup.exe.` runs as `setup.exe`.
    // A name that is all extension, such as `.command`, counts as one.
    let extension = name.as_deref().and_then(|name| {
        let name = name.trim_end_matches(['.', ' ']);
        name.rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
    });
    if extension
        .as_deref()
        .is_some_and(|extension| RUNNABLE_EXTENSIONS.contains(&extension))
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return true;
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    false
}

/// Where an image tab stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    /// The image is being read on another thread.
    Reading,
    /// The image file's bytes, for the client to decode and draw.
    /// `generation` changes whenever the bytes do.
    Shown { bytes: Arc<[u8]>, generation: u64 },
    /// The image couldn't be read, for the reason `message` gives.
    Failed { message: String },
}

/// The zoom levels Zoom In and Zoom Out step an image tab through, as
/// magnifications of the image's own size: the PDF tab's, with room to
/// look at a large photo whole and at a small icon's pixels.
pub const IMAGE_ZOOM_STEPS: [f32; 22] = [
    0.05, 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
    4.0, 6.0, 8.0, 12.0, 16.0,
];

/// How large an image tab draws its image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageZoom {
    /// Small enough to show whole in the tab, and no larger than its own
    /// size, as Preview, Safari and VS Code open an image.
    Fit,
    /// At this magnification of the image's own size on the screen.
    Scale(f32),
}

/// A scroll an image tab's client makes once: the point of its content
/// to show at the top left of the view, in points. Each scroll has a
/// number of its own, which the client reports back once it made it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageScroll {
    pub seq: u64,
    pub offset: [f32; 2],
}

/// What an image tab shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageView {
    path: String,
    state: ImageState,
    zoom: ImageZoom,
    /// The image's own size in the screen's points, as its client
    /// measured it.
    size: Option<[f32; 2]>,
    /// The size of the view the tab shows the image in, in the client's
    /// points.
    area: Option<[f32; 2]>,
    /// The client's interface zoom: how many of the screen's points each
    /// of its points covers.
    interface_zoom: f32,
    /// The point of the content at the top left of the view, as the
    /// client last reported it.
    offset: [f32; 2],
    /// A scroll the client hasn't made yet.
    scroll: Option<ImageScroll>,
    /// The scrolls asked for so far.
    scrolls: u64,
}

impl ImageView {
    /// The image's workspace path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The image's file name, which labels it.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn state(&self) -> &ImageState {
        &self.state
    }

    pub fn zoom(&self) -> ImageZoom {
        self.zoom
    }

    /// How large the image shows on the screen against its own size: the
    /// zoom's, or at Fit, the largest that shows it whole, up to 1.
    pub fn magnification(&self) -> f32 {
        match self.zoom {
            ImageZoom::Scale(magnification) => magnification,
            ImageZoom::Fit => self.fit_magnification().unwrap_or(1.0),
        }
    }

    /// The client's points per point of the image: the magnification,
    /// less the interface zoom the client's points carry, so the interface
    /// zoom leaves the image's size on the screen alone, as it does a PDF
    /// page's.
    pub fn scale(&self) -> f32 {
        self.magnification() / self.interface_zoom
    }

    /// The image's size as it draws, in the client's points, once the
    /// client measured it.
    pub fn drawn_size(&self) -> Option<[f32; 2]> {
        let scale = self.scale();
        self.size
            .map(|[width, height]| [width * scale, height * scale])
    }

    /// Where the image's top left draws in the view's content: centered
    /// on each side where it's smaller than the view.
    pub fn image_origin(&self) -> [f32; 2] {
        let (Some(drawn), Some(area)) = (self.drawn_size(), self.area) else {
            return [0.0, 0.0];
        };
        [0, 1].map(|axis| ((area[axis] - drawn[axis]) / 2.0).max(0.0))
    }

    /// The scroll the client should make, until it reports it made it.
    pub fn scroll(&self) -> Option<ImageScroll> {
        self.scroll
    }

    /// The magnification that shows the image whole, up to 1.
    fn fit_magnification(&self) -> Option<f32> {
        let (size, area) = (self.size?, self.area?);
        let zoom = self.interface_zoom;
        let fit = (area[0] * zoom / size[0])
            .min(area[1] * zoom / size[1])
            .min(1.0);
        (fit.is_finite() && fit > 0.0).then_some(fit)
    }

    /// The image measures `size` of the screen's points, and its view
    /// `area` of the client's points at the interface zoom
    /// `interface_zoom`. The client says so each frame, before it draws.
    /// When the image's scale in the client's points changes, as at Fit
    /// for a new view or for a new interface zoom, what shows at the
    /// middle of the view stays there.
    pub(crate) fn measure(&mut self, size: [f32; 2], area: [f32; 2], interface_zoom: f32) {
        let measured = self.size.is_some() && self.area.is_some();
        let before = self.scale();
        let origin = self.image_origin();
        let middle = self.area.map(|area| [area[0] / 2.0, area[1] / 2.0]);
        let usable = |value: [f32; 2]| value.iter().all(|side| side.is_finite() && *side > 0.0);
        if usable(size) {
            self.size = Some(size);
        }
        if usable(area) {
            self.area = Some(area);
        }
        if interface_zoom.is_finite() && interface_zoom > 0.0 {
            self.interface_zoom = interface_zoom;
        }
        if measured
            && (self.scale() - before).abs() > 1e-5
            && let (Some(middle), Some(area)) = (middle, self.area)
        {
            self.keep_in_place(before, origin, middle, [area[0] / 2.0, area[1] / 2.0]);
        }
    }

    /// The client shows `offset` at the view's top left, having made the
    /// scroll `seq`. Until it makes the scroll asked for last, what it
    /// shows is about to change.
    pub(crate) fn scrolled(&mut self, offset: [f32; 2], seq: u64) {
        if let Some(scroll) = self.scroll {
            if scroll.seq != seq {
                return;
            }
            self.scroll = None;
        }
        if offset.iter().all(|side| side.is_finite()) {
            self.offset = offset;
        }
    }

    /// Zoom one step in or out from the magnification the image shows
    /// at. A step across the magnification that fits the image stops at
    /// Fit.
    pub(crate) fn zoom_step(&mut self, zoom_in: bool) -> bool {
        let current = self.magnification();
        let next = if zoom_in {
            IMAGE_ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > current + 0.001)
                .unwrap_or(IMAGE_ZOOM_STEPS[IMAGE_ZOOM_STEPS.len() - 1])
        } else {
            IMAGE_ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current - 0.001)
                .unwrap_or(IMAGE_ZOOM_STEPS[0])
        };
        let crosses_fit = self.fit_magnification().is_some_and(|fit| {
            self.zoom != ImageZoom::Fit
                && (current - fit).abs() > 0.001
                && (next - fit).abs() > 0.001
                && (current < fit) != (next < fit)
        });
        let zoom = if crosses_fit {
            ImageZoom::Fit
        } else {
            ImageZoom::Scale(next)
        };
        self.set_zoom(zoom, None)
    }

    /// Show the image at its own size.
    pub(crate) fn actual_size(&mut self) -> bool {
        self.set_zoom(ImageZoom::Scale(1.0), None)
    }

    /// Magnify the image by `factor`, as a pinch does, keeping the point
    /// at `anchor` in the view, from its top left, where it is.
    pub(crate) fn zoom_by(&mut self, factor: f32, anchor: [f32; 2]) -> bool {
        if !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        let first = IMAGE_ZOOM_STEPS[0];
        let last = IMAGE_ZOOM_STEPS[IMAGE_ZOOM_STEPS.len() - 1];
        let magnification = (self.magnification() * factor).clamp(first, last);
        self.set_zoom(ImageZoom::Scale(magnification), Some(anchor))
    }

    /// Show the image at `zoom`, keeping the point of the image at
    /// `anchor` in the view where it is, or the one at the view's middle.
    fn set_zoom(&mut self, zoom: ImageZoom, anchor: Option<[f32; 2]>) -> bool {
        if zoom == self.zoom {
            return false;
        }
        let before = self.scale();
        let origin = self.image_origin();
        self.zoom = zoom;
        if let Some(area) = self.area {
            let anchor = anchor.unwrap_or([area[0] / 2.0, area[1] / 2.0]);
            self.keep_in_place(before, origin, anchor, anchor);
        }
        true
    }

    /// The image drew at `before` of the client's points per point, from
    /// `origin` in the view's content, and draws at its scale now: scroll
    /// so the point of the image that showed at `from` in the view shows
    /// at `to`. The two differ when the view's size changed.
    fn keep_in_place(&mut self, before: f32, origin: [f32; 2], from: [f32; 2], to: [f32; 2]) {
        let (Some(size), Some(area)) = (self.size, self.area) else {
            return;
        };
        let offset = self.scroll.map_or(self.offset, |scroll| scroll.offset);
        let after = self.scale();
        let new_origin = self.image_origin();
        let target = [0, 1].map(|axis| {
            // The point of the image, in its own points, under the anchor.
            let point =
                ((from[axis] - origin[axis] + offset[axis]) / before).clamp(0.0, size[axis]);
            let reach = (size[axis] * after - area[axis]).max(0.0);
            (new_origin[axis] + point * after - to[axis]).clamp(0.0, reach)
        });
        self.scrolls += 1;
        self.scroll = Some(ImageScroll {
            seq: self.scrolls,
            offset: target,
        });
    }
}

/// A file's size and modification time, which change when another app
/// replaces it.
type Stamp = Option<(u64, Option<SystemTime>)>;

/// Counts the image reads so far.
static IMAGE_READS: AtomicU64 = AtomicU64::new(0);

/// What a read of an image found: nothing new, or the file's version and
/// its bytes or why they couldn't be read.
enum ImageRead {
    Unchanged,
    Read {
        stamp: Stamp,
        bytes: Result<Vec<u8>, String>,
    },
}

/// One image tab and its reads.
pub(crate) struct ImageSession {
    view: ImageView,
    /// The version of the file the shown bytes came from.
    stamp: Stamp,
    /// A read or check runs on another thread.
    busy: bool,
    /// Another check waits for the running one.
    check_again: bool,
    engine: Arc<WorkspaceEngine>,
    wake: Arc<dyn Fn() + Send + Sync>,
    sender: Sender<ImageRead>,
    answers: Receiver<ImageRead>,
}

impl ImageSession {
    fn open(engine: Arc<WorkspaceEngine>, path: &str, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (sender, answers) = channel();
        let mut session = Self {
            view: ImageView {
                path: path.to_owned(),
                state: ImageState::Reading,
                zoom: ImageZoom::Fit,
                size: None,
                area: None,
                interface_zoom: 1.0,
                offset: [0.0, 0.0],
                scroll: None,
                scrolls: 0,
            },
            stamp: None,
            busy: false,
            check_again: false,
            engine,
            wake,
            sender,
            answers,
        };
        session.read(None);
        session
    }

    pub(crate) fn view(&self) -> &ImageView {
        &self.view
    }

    pub(crate) fn view_mut(&mut self) -> &mut ImageView {
        &mut self.view
    }

    /// Read the image on another thread, unless its version is still
    /// `known`.
    fn read(&mut self, known: Option<Stamp>) {
        self.busy = true;
        let engine = self.engine.clone();
        let path = self.view.path.clone();
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        let spawned = std::thread::Builder::new()
            .name("image-read".into())
            .spawn(move || {
                // The version comes first, so a write during the read
                // makes the next check differ.
                let stamp = engine.file_version(&path).ok();
                let read = if known.is_some_and(|known| known == stamp) {
                    ImageRead::Unchanged
                } else {
                    let bytes = match engine.read_file(&path, MAX_IMAGE_BYTES as u64) {
                        Ok((bytes, _)) => Ok(bytes),
                        Err(error) if error.code == "file_too_large" => {
                            Err("This image is larger than 20 MB, the most Noura shows.".to_owned())
                        }
                        Err(error) => {
                            Err(AppError::from_core(error, &path, Access::ReadFile).message)
                        }
                    };
                    ImageRead::Read { stamp, bytes }
                };
                if sender.send(read).is_ok() {
                    wake();
                }
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "could not start an image read");
            self.busy = false;
            self.view.state = ImageState::Failed {
                message: "Noura couldn't open this image.".to_owned(),
            };
        }
    }

    /// Take in what the reads found. Returns whether the view changed.
    fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(read) = self.answers.try_recv() {
            self.busy = false;
            if let ImageRead::Read { stamp, bytes } = read {
                self.stamp = stamp;
                self.view.state = match bytes {
                    Ok(bytes) => ImageState::Shown {
                        bytes: Arc::from(bytes),
                        generation: IMAGE_READS.fetch_add(1, Ordering::Relaxed),
                    },
                    Err(message) => ImageState::Failed { message },
                };
                changed = true;
            }
        }
        if !self.busy && std::mem::take(&mut self.check_again) {
            self.check();
        }
        changed
    }

    /// The files changed: read the image again if it did.
    fn check(&mut self) {
        if self.busy {
            self.check_again = true;
        } else {
            self.read(Some(self.stamp));
        }
    }

    fn moved_to(&mut self, path: &str) {
        path.clone_into(&mut self.view.path);
    }
}

/// The viewers of one window's tabs.
#[derive(Default)]
pub(crate) struct Viewers {
    pub(crate) pdfs: HashMap<TabId, PdfSession>,
    pub(crate) images: HashMap<TabId, ImageSession>,
    /// Holds the PDF threads before they draw, for tests.
    pub(crate) gate: RenderGate,
}

impl Viewers {
    /// Bring the viewers in line with `files`, each file tab's ID and
    /// path: open a viewer for each new PDF or image tab, follow tabs
    /// whose file moved, and drop the viewers of closed tabs.
    pub(crate) fn sync(
        &mut self,
        files: &[(TabId, String)],
        engine: &Arc<WorkspaceEngine>,
        wake: &Arc<dyn Fn() + Send + Sync>,
    ) {
        let shows = |tab: &TabId, kind: fn(&str) -> bool| {
            files.iter().any(|(id, path)| id == tab && kind(path))
        };
        self.pdfs.retain(|tab, _| shows(tab, is_pdf));
        self.images.retain(|tab, _| shows(tab, is_image));
        for (tab, path) in files {
            if is_pdf(path) {
                match self.pdfs.get_mut(tab) {
                    Some(session) if session.view().path() != path => session.moved_to(path),
                    Some(_) => {}
                    None => {
                        let session =
                            PdfSession::open(engine.clone(), path, wake.clone(), self.gate.clone());
                        self.pdfs.insert(*tab, session);
                    }
                }
            } else if is_image(path) {
                match self.images.get_mut(tab) {
                    Some(session) if session.view().path() != path => session.moved_to(path),
                    Some(_) => {}
                    None => {
                        let session = ImageSession::open(engine.clone(), path, wake.clone());
                        self.images.insert(*tab, session);
                    }
                }
            }
        }
    }

    /// Take in what the viewers' threads found. Returns whether any view
    /// changed.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        for session in self.pdfs.values_mut() {
            changed |= session.poll();
        }
        for session in self.images.values_mut() {
            changed |= session.poll();
        }
        changed
    }

    /// The files changed: each viewer reads its file again if it did.
    pub(crate) fn check(&mut self) {
        for session in self.pdfs.values() {
            session.check();
        }
        for session in self.images.values_mut() {
            session.check();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pdfs.clear();
        self.images.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::could_run_code;

    fn runs(directory: &std::path::Path, name: &str) -> bool {
        let path = directory.join(name);
        std::fs::write(&path, "x").unwrap();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        could_run_code(&path, &metadata)
    }

    #[test]
    fn apps_scripts_installers_and_shortcuts_could_run_code() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "setup.EXE",
            "run.bat",
            "install.command",
            "tool.jar",
            "Tool.app.zip.js",
            "Profile.mobileconfig",
            "home.fileloc",
            ".command",
            "setup.exe.",
            "setup.exe . .",
        ] {
            assert!(runs(directory.path(), name), "{name}");
        }
        for name in ["report.pdf", "photo.JPG", "data.csv", "archive.zip", "song"] {
            assert!(!runs(directory.path(), name), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_file_anyone_may_execute_could_run_code() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("script");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        for mode in [0o700, 0o710, 0o701] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(could_run_code(&path, &metadata), "{mode:o}");
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(!could_run_code(&path, &metadata));
    }
}
