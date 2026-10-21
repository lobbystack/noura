//! Full parses of large notes on a thread of their own.
//!
//! An edit to a large note reparses only the blocks it reaches
//! ([`crate::Ast::update`]), so typing never waits for a parse of the whole
//! note. That tree differs from a full parse when the edit reaches past
//! those blocks, as an unclosed code fence does, so after each edit the
//! note's text also goes to a background thread for a full parse and the
//! bounds built from it. A result for text that has changed since is
//! dropped; the next one replaces it.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ast::Ast;
use crate::bounds::Bounds;
use crate::doc::MdDoc;

/// Something to call once a background parse finishes, such as asking the
/// window for a frame.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A full parse of one text version and the bounds built from it.
pub(crate) struct Parsed {
    pub(crate) ast: Ast,
    pub(crate) bounds: Bounds,
}

/// What a background parse needs: the text and the settings bounds read.
struct Request {
    text_seq: u64,
    text: String,
    disable_images: bool,
    plaintext: bool,
}

/// The thread that parses a note's text versions, newest first.
pub(crate) struct BackgroundParser {
    requests: Option<Sender<Request>>,
    results: Receiver<Parsed>,
    /// A result taken off the channel and not yet used.
    ready: Option<Parsed>,
    /// Holds parses back while set, for tests.
    hold: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    /// Results installed, and results dropped because the text had
    /// changed since.
    pub(crate) installed: usize,
    pub(crate) dropped: usize,
}

impl BackgroundParser {
    pub(crate) fn new(wake: Option<Wake>) -> Self {
        let (requests, inbox) = channel::<Request>();
        let (outbox, results) = channel();
        let hold = Arc::new((Mutex::new(false), Condvar::new()));
        let held = Arc::clone(&hold);
        let thread = std::thread::Builder::new()
            .name("note parser".into())
            .spawn(move || {
                while let Ok(mut request) = inbox.recv() {
                    // Only the newest text matters.
                    while let Ok(newer) = inbox.try_recv() {
                        request = newer;
                    }
                    {
                        let (held, resume) = &*held;
                        let mut held = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        while *held {
                            held = resume
                                .wait(held)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                    }
                    while let Ok(newer) = inbox.try_recv() {
                        request = newer;
                    }
                    if outbox.send(parse(request)).is_err() {
                        return;
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            })
            .ok();
        Self {
            requests: thread.is_some().then_some(requests),
            results,
            ready: None,
            hold,
            thread,
            installed: 0,
            dropped: 0,
        }
    }

    /// Asks for a full parse of `doc`'s current text.
    pub(crate) fn request(&self, doc: &MdDoc) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(Request {
                text_seq: doc.text_seq,
                text: doc.buffer.current.text_with("\n"),
                disable_images: doc.disable_images,
                plaintext: doc.plaintext,
            });
        }
    }

    /// The newest finished parse, if any.
    pub(crate) fn take(&mut self) -> Option<Parsed> {
        let mut newest = self.ready.take();
        while let Ok(parsed) = self.results.try_recv() {
            if newest.is_some() {
                self.dropped += 1;
            }
            newest = Some(parsed);
        }
        newest
    }

    /// Waits up to `timeout` for a parse to finish, without using it.
    pub(crate) fn wait(&mut self, timeout: Duration) -> bool {
        if self.ready.is_some() {
            return true;
        }
        match self.results.recv_timeout(timeout) {
            Ok(parsed) => {
                self.ready = Some(parsed);
                true
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => false,
        }
    }

    pub(crate) fn hold(&self, hold: bool) {
        let (held, resume) = &*self.hold;
        *held.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = hold;
        resume.notify_all();
    }
}

impl Drop for BackgroundParser {
    fn drop(&mut self) {
        // Closing the channel ends the thread once it finishes its parse.
        self.requests = None;
        self.hold(false);
        drop(self.thread.take());
    }
}

/// A full parse of `request`'s text and its bounds, made the way the
/// editor makes them.
fn parse(request: Request) -> Parsed {
    let mut text = request.text;
    text.pop();
    let mut doc = MdDoc::new(&text);
    doc.disable_images = request.disable_images;
    doc.plaintext = request.plaintext;
    doc.text_seq = request.text_seq;
    let mut ast = doc.take_full_ast();
    ast.set_text_seq(request.text_seq);
    Parsed {
        ast,
        bounds: std::mem::take(&mut doc.bounds),
    }
}
