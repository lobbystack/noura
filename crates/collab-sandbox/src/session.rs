//! Runs a [`Note`] on its own thread, so the note stays editable while a
//! slow record decodes (sync-v2.md §22).
//!
//! The editor applies its own edits at once and sends them to the thread,
//! where they queue behind the records ahead of them. The thread sends back
//! the text changes that records, undo and redo make. Neither side waits for
//! the other, so each rewrites what it receives through what it sent and the
//! other hasn't seen yet, with [`transform`]: the editor and the document
//! end on the same text whatever order the work runs in. This is the
//! two-party Jupiter protocol: each message names how many of the other
//! side's messages its sender had taken in.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::Sandbox;
use crate::note::{ClientIds, DocumentError, Effects, Event, Note, RemoteRecord};
use crate::op::{Mismatch, Op, transform};

/// An edit, undo or redo from the editor.
struct ClientMessage {
    /// How many of the thread's changes the editor had applied.
    seen: u64,
    /// The edit, against the editor's text; the identity for undo and redo.
    op: Op,
}

/// A text change from the thread.
struct ServerMessage {
    /// How many of the editor's messages the thread had taken in.
    seen: u64,
    /// The change, against the document's text.
    op: Op,
}

enum Request {
    Edit {
        message: ClientMessage,
        undoable: bool,
        at: u64,
    },
    Undo {
        message: ClientMessage,
        at: u64,
    },
    Redo {
        message: ClientMessage,
        at: u64,
    },
    EndUndoStep,
    Record(Box<RemoteRecord>),
    TakeUpdate,
    State,
    Text,
    EncodePosition {
        index: u32,
        after: bool,
    },
    DecodePosition(Vec<u8>),
    Flush,
}

enum Reply {
    Opened(Result<(String, u32), DocumentError>),
    Change(ServerMessage),
    Event(Event),
    Failed(DocumentError),
    Update(Result<Option<Vec<u8>>, DocumentError>),
    State(Result<Vec<u8>, DocumentError>),
    Text(Result<String, DocumentError>),
    Position(Result<Vec<u8>, DocumentError>),
    Offset(Result<u32, DocumentError>),
    Flushed,
}

/// Why a call on a [`NoteHandle`] failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandleError {
    /// The note's thread is gone.
    #[error("the note's sandbox thread stopped")]
    Closed,
    /// The document refused the call.
    #[error(transparent)]
    Document(#[from] DocumentError),
    /// An edit doesn't fit the editor's text.
    #[error(transparent)]
    Mismatch(#[from] Mismatch),
}

/// Pauses the thread before its next request while set, as a test does to
/// stand in for a record that takes long to decode.
#[derive(Default)]
struct Hold {
    held: Mutex<bool>,
    changed: Condvar,
}

impl Hold {
    fn set(&self, held: bool) {
        *self
            .held
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = held;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        while *held {
            held = self
                .changed
                .wait(held)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}

/// The editor's side of a text-mode note whose document lives in the
/// sandbox on its own thread.
pub struct NoteHandle {
    requests: Option<Sender<Request>>,
    replies: Receiver<Reply>,
    /// Replies that arrived while a call waited for its own.
    stash: VecDeque<Reply>,
    events: Vec<Event>,
    /// How many messages the editor sent.
    sent: u64,
    /// How many changes the editor applied.
    received: u64,
    /// The editor's messages the thread may not have taken in yet.
    outgoing: VecDeque<(u64, Op)>,
    /// The editor's text length in UTF-16 units.
    len: u32,
    started: Instant,
    hold: Arc<Hold>,
    thread: Option<JoinHandle<()>>,
}

impl NoteHandle {
    /// Opens a note's document on a new thread from the records the journal
    /// holds for its lineage, and returns the handle with the note's text.
    pub fn open(
        sandbox: &Sandbox,
        journal: Vec<Vec<u8>>,
        client_ids: ClientIds,
    ) -> Result<(Self, String), HandleError> {
        let (requests, inbox) = mpsc::channel();
        let (outbox, replies) = mpsc::channel();
        let hold = Arc::new(Hold::default());
        let sandbox = sandbox.clone();
        let thread_hold = hold.clone();
        let thread = std::thread::Builder::new()
            .name("collab-sandbox".into())
            .spawn(move || {
                let mut note = match Note::open(&sandbox, journal, client_ids) {
                    Ok(note) => note,
                    Err(error) => {
                        let _ = outbox.send(Reply::Opened(Err(error)));
                        return;
                    }
                };
                let opened = note.text().map(|text| (text, note.len()));
                let ok = opened.is_ok();
                if outbox.send(Reply::Opened(opened)).is_err() || !ok {
                    return;
                }
                serve(note, &inbox, &outbox, &thread_hold);
            })
            .map_err(|_| HandleError::Closed)?;
        let (text, len) = match replies.recv() {
            Ok(Reply::Opened(opened)) => opened?,
            _ => return Err(HandleError::Closed),
        };
        Ok((
            Self {
                requests: Some(requests),
                replies,
                stash: VecDeque::new(),
                events: Vec::new(),
                sent: 0,
                received: 0,
                outgoing: VecDeque::new(),
                len,
                started: Instant::now(),
                hold,
                thread: Some(thread),
            },
            text,
        ))
    }

    fn send(&self, request: Request) -> Result<(), HandleError> {
        self.requests
            .as_ref()
            .ok_or(HandleError::Closed)?
            .send(request)
            .map_err(|_| HandleError::Closed)
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The editor's text length in UTF-16 units.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the editor's text is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn message(&mut self, op: Op) -> ClientMessage {
        let message = ClientMessage {
            seen: self.received,
            op: op.clone(),
        };
        self.len = op.target_len();
        self.outgoing.push_back((self.sent, op));
        self.sent += 1;
        message
    }

    /// Sends an edit the editor already made to its text. Undo takes it
    /// back only when `undoable`; an outside edit of the file isn't.
    pub fn edit(&mut self, op: Op, undoable: bool) -> Result<(), HandleError> {
        if op.base_len() != self.len {
            return Err(Mismatch.into());
        }
        let at = self.now();
        let message = self.message(op);
        self.send(Request::Edit {
            message,
            undoable,
            at,
        })
    }

    /// Asks the document to take back your last undo step. Its change
    /// arrives through [`Self::poll`].
    pub fn undo(&mut self) -> Result<(), HandleError> {
        let at = self.now();
        let message = self.message(Op::identity(self.len));
        self.send(Request::Undo { message, at })
    }

    /// Asks the document to reapply your last undone step.
    pub fn redo(&mut self) -> Result<(), HandleError> {
        let at = self.now();
        let message = self.message(Op::identity(self.len));
        self.send(Request::Redo { message, at })
    }

    /// Ends the current undo step.
    pub fn end_undo_step(&mut self) -> Result<(), HandleError> {
        self.send(Request::EndUndoStep)
    }

    /// Hands the thread a remote record to judge and merge.
    pub fn receive(&mut self, record: RemoteRecord) -> Result<(), HandleError> {
        self.send(Request::Record(Box::new(record)))
    }

    fn take_in(&mut self, reply: Reply, changes: &mut Vec<Op>) -> Result<(), HandleError> {
        match reply {
            Reply::Change(message) => {
                while self
                    .outgoing
                    .front()
                    .is_some_and(|(index, _)| *index < message.seen)
                {
                    self.outgoing.pop_front();
                }
                let mut op = message.op;
                for (_, pending) in &mut self.outgoing {
                    let (server, client) = transform(&op, pending)?;
                    op = server;
                    *pending = client;
                }
                self.received += 1;
                self.len = op.target_len();
                changes.push(op);
            }
            Reply::Event(event) => self.events.push(event),
            Reply::Failed(error) => return Err(error.into()),
            _ => {}
        }
        Ok(())
    }

    /// The text changes to apply to the editor, in order, each against the
    /// editor's text as the ones before it leave it.
    pub fn poll(&mut self) -> Result<Vec<Op>, HandleError> {
        let mut changes = Vec::new();
        while let Some(reply) = self.stash.pop_front() {
            self.take_in(reply, &mut changes)?;
        }
        while let Ok(reply) = self.replies.try_recv() {
            self.take_in(reply, &mut changes)?;
        }
        Ok(changes)
    }

    /// Waits until the thread has handled everything sent so far, then
    /// returns the changes to apply, as [`Self::poll`] does.
    pub fn flush(&mut self) -> Result<Vec<Op>, HandleError> {
        self.send(Request::Flush)?;
        self.wait_for(|reply| matches!(reply, Reply::Flushed))?;
        self.poll()
    }

    /// Waits up to `timeout` for a change or an event.
    pub fn wait(&mut self, timeout: Duration) -> Result<Vec<Op>, HandleError> {
        if self.stash.is_empty() {
            match self.replies.recv_timeout(timeout) {
                Ok(reply) => self.stash.push_back(reply),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(HandleError::Closed),
            }
        }
        self.poll()
    }

    /// The events the thread reported since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn wait_for(&mut self, mine: impl Fn(&Reply) -> bool) -> Result<Reply, HandleError> {
        loop {
            let reply = self.replies.recv().map_err(|_| HandleError::Closed)?;
            if mine(&reply) {
                return Ok(reply);
            }
            self.stash.push_back(reply);
        }
    }

    /// The update holding your changes since the last call, for a
    /// `text_update`.
    pub fn take_update(&mut self) -> Result<Option<Vec<u8>>, HandleError> {
        self.send(Request::TakeUpdate)?;
        match self.wait_for(|reply| matches!(reply, Reply::Update(_)))? {
            Reply::Update(update) => Ok(update?),
            _ => unreachable!(),
        }
    }

    /// The document encoded as a snapshot holds it.
    pub fn state(&mut self) -> Result<Vec<u8>, HandleError> {
        self.send(Request::State)?;
        match self.wait_for(|reply| matches!(reply, Reply::State(_)))? {
            Reply::State(state) => Ok(state?),
            _ => unreachable!(),
        }
    }

    /// The document's merged text: what the file holds once the thread
    /// catches up.
    pub fn merged_text(&mut self) -> Result<String, HandleError> {
        self.send(Request::Text)?;
        match self.wait_for(|reply| matches!(reply, Reply::Text(_)))? {
            Reply::Text(text) => Ok(text?),
            _ => unreachable!(),
        }
    }

    /// The relative position of the document's UTF-16 offset `index`.
    pub fn encode_position(&mut self, index: u32, after: bool) -> Result<Vec<u8>, HandleError> {
        self.send(Request::EncodePosition { index, after })?;
        match self.wait_for(|reply| matches!(reply, Reply::Position(_)))? {
            Reply::Position(position) => Ok(position?),
            _ => unreachable!(),
        }
    }

    /// The document's UTF-16 offset a collaborator's relative position
    /// points at.
    pub fn decode_position(&mut self, bytes: Vec<u8>) -> Result<u32, HandleError> {
        self.send(Request::DecodePosition(bytes))?;
        match self.wait_for(|reply| matches!(reply, Reply::Offset(_)))? {
            Reply::Offset(offset) => Ok(offset?),
            _ => unreachable!(),
        }
    }

    /// Pauses the thread before its next request while `held`, as tests do
    /// to stand in for a record that takes long to decode.
    pub fn hold(&self, held: bool) {
        self.hold.set(held);
    }
}

impl Drop for NoteHandle {
    fn drop(&mut self) {
        self.hold.set(false);
        drop(self.requests.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread's side: how many changes it sent, how many editor messages
/// it took in, and the changes the editor may not have applied yet.
#[derive(Default)]
struct Server {
    sent: u64,
    received: u64,
    outgoing: VecDeque<(u64, Op)>,
}

impl Server {
    fn take_in(&mut self, message: ClientMessage) -> Result<Op, Mismatch> {
        while self
            .outgoing
            .front()
            .is_some_and(|(index, _)| *index < message.seen)
        {
            self.outgoing.pop_front();
        }
        let mut op = message.op;
        for (_, pending) in &mut self.outgoing {
            let (server, client) = transform(pending, &op)?;
            *pending = server;
            op = client;
        }
        self.received += 1;
        Ok(op)
    }

    fn send_out(&mut self, op: Op) -> ServerMessage {
        let message = ServerMessage {
            seen: self.received,
            op: op.clone(),
        };
        self.outgoing.push_back((self.sent, op));
        self.sent += 1;
        message
    }
}

fn serve(mut note: Note, inbox: &Receiver<Request>, outbox: &Sender<Reply>, hold: &Hold) {
    let mut server = Server::default();
    loop {
        let request = match note.next_wake() {
            Some(at) => match inbox.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(request) => Some(request),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match inbox.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            },
        };
        hold.wait();
        let now = Instant::now();
        let mut effects = Effects::default();
        let mut reply = None;
        match request {
            None => effects = note.wake(now),
            Some(Request::Record(record)) => effects = note.receive(*record, now),
            Some(Request::Edit {
                message,
                undoable,
                at,
            }) => {
                let result = server
                    .take_in(message)
                    .map_err(DocumentError::from)
                    .and_then(|op| note.local(&op, undoable, at));
                if let Err(error) = result {
                    reply = Some(Reply::Failed(error));
                }
            }
            Some(Request::Undo { message, at }) => {
                let _ = server.take_in(message);
                match note.undo(at) {
                    Ok(op) => effects.changes.push(op),
                    Err(error) => reply = Some(Reply::Failed(error)),
                }
            }
            Some(Request::Redo { message, at }) => {
                let _ = server.take_in(message);
                match note.redo(at) {
                    Ok(op) => effects.changes.push(op),
                    Err(error) => reply = Some(Reply::Failed(error)),
                }
            }
            Some(Request::EndUndoStep) => {
                if let Err(error) = note.end_undo_step() {
                    reply = Some(Reply::Failed(error));
                }
            }
            Some(Request::TakeUpdate) => reply = Some(Reply::Update(note.take_update())),
            Some(Request::State) => reply = Some(Reply::State(note.state())),
            Some(Request::Text) => reply = Some(Reply::Text(note.text())),
            Some(Request::EncodePosition { index, after }) => {
                reply = Some(Reply::Position(note.encode_position(index, after)))
            }
            Some(Request::DecodePosition(bytes)) => {
                reply = Some(Reply::Offset(note.decode_position(&bytes)))
            }
            Some(Request::Flush) => reply = Some(Reply::Flushed),
        }
        for op in effects.changes {
            if !op.is_identity() && outbox.send(Reply::Change(server.send_out(op))).is_err() {
                return;
            }
        }
        for event in effects.events {
            if outbox.send(Reply::Event(event)).is_err() {
                return;
            }
        }
        if let Some(reply) = reply
            && outbox.send(reply).is_err()
        {
            return;
        }
    }
}
