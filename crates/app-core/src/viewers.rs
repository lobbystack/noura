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

/// What an image tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageView {
    path: String,
    state: ImageState,
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
