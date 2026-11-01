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
//!
//! The file follows the document, never the editor: the thread publishes
//! each change the document merges, in the order it merges them, and
//! [`SharedNote::take_merged_changes`] hands them over. Your edits join
//! that stream only once they merge after the records ahead of them, so
//! while a slow record decodes the file keeps its last merged state.
//!
//! The editor's side, [`NoteHandle`], never waits for the thread, and
//! dropping it doesn't either; [`NoteHandle::start`] loads the journal
//! without waiting, for the UI thread. The sync client's side,
//! [`SharedNote`], waits for each answer on a channel of its own and holds
//! no lock the editor takes, so a call that waits behind a slow record
//! holds up only the thread that made it.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::Sandbox;
use crate::note::{
    ClientIds, DocumentError, Effects, Event, LineageStatus, LocalUpdate, Note, Opened,
    RemoteRecord,
};
use crate::op::{Mismatch, Op, three_way, transform};

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

/// Where the thread sends the answer to one request.
type Answer<T> = Sender<Result<T, DocumentError>>;

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
    /// Applies an outside edit made from `base`, which moved the file to
    /// `text`, on top of what the document merged since `base`.
    Merge {
        base: String,
        text: String,
        at: u64,
        answer: Answer<()>,
    },
    Balances(Answer<Vec<(u32, i64)>>),
    Record(Box<RemoteRecord>),
    TakeUpdates(Answer<Vec<LocalUpdate>>),
    State(Answer<Vec<u8>>),
    Text(Answer<String>),
    EncodePosition {
        index: u32,
        after: bool,
        answer: Answer<Vec<u8>>,
    },
    DecodePosition {
        bytes: Vec<u8>,
        answer: Answer<u32>,
    },
    Flush(Sender<()>),
    /// Moves the thread's clock forward, as tests do to stand in for the
    /// refills that time brings.
    #[cfg(any(test, feature = "test-support"))]
    Advance(Duration),
    /// Sets a writer's allowance balance, as the fixtures' base states do.
    #[cfg(any(test, feature = "test-support"))]
    SetBalance {
        writer: u32,
        balance: i64,
    },
    /// Makes the next integration trap, as the fixtures' trap cases ask.
    #[cfg(any(test, feature = "test-support"))]
    TrapNext,
    /// Judges the records after it with another module, as the fixtures'
    /// verdict trap cases ask.
    #[cfg(test)]
    Verdicts(Sandbox),
    /// Ends the thread once the requests ahead of it are done.
    Close,
}

/// What the thread sends the editor.
enum Reply {
    Change(ServerMessage),
    Failed(DocumentError),
}

/// Why a call on a [`NoteHandle`] or a [`SharedNote`] failed.
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Pauses the thread before its next request while set, as a test does to
/// stand in for a record that takes long to decode.
#[derive(Default)]
struct Hold {
    held: Mutex<bool>,
    changed: Condvar,
}

impl Hold {
    #[cfg(any(test, feature = "test-support"))]
    fn set(&self, held: bool) {
        *lock(&self.held) = held;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut held = lock(&self.held);
        while *held {
            held = self
                .changed
                .wait(held)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}

/// What both sides read without waiting for the thread. The thread holds
/// each lock only to copy a value in or out.
#[derive(Default)]
struct Shared {
    /// Where the lineage stands, as of the last request the thread handled.
    status: Mutex<LineageStatus>,
    /// The events the thread reported and nobody took yet.
    events: Mutex<Vec<Event>>,
    /// The changes the document merged and nobody took yet, in the order it
    /// merged them: what the file gets.
    merged: Mutex<Vec<Op>>,
    /// Whether undo and redo have a step of yours, as of the last request
    /// the thread handled.
    undo: Mutex<(bool, bool)>,
    /// The errors the editor's calls met and nobody took yet.
    errors: Mutex<Vec<HandleError>>,
    hold: Hold,
}

/// The editor's side of a text-mode note whose document lives in the
/// sandbox on its own thread. None of its calls waits for the thread.
pub struct NoteHandle {
    shared: SharedNote,
    replies: Receiver<Reply>,
    /// How many messages the editor sent.
    sent: u64,
    /// How many changes the editor applied.
    received: u64,
    /// The editor's messages the thread may not have taken in yet.
    outgoing: VecDeque<(u64, Op)>,
    /// The editor's text length in UTF-16 units.
    len: u32,
    /// Whether a change no longer fits the editor's text, or the thread is
    /// gone: the editor and the document went apart, so no more changes
    /// apply.
    broken: bool,
    /// Whether the thread outlives this side, for the sync client to take
    /// what the editor left in the document.
    outlived: bool,
    started: Instant,
}

impl NoteHandle {
    /// Opens a note's document on a new thread from the records the journal
    /// holds for its lineage, and returns the handle with the note's text.
    /// This call waits until the journal loads, which a journal of costly
    /// records can make take minutes, so the UI thread never makes it: it
    /// calls [`Self::start`] and polls.
    pub fn open(
        sandbox: &Sandbox,
        journal: Vec<Vec<u8>>,
        client_ids: ClientIds,
    ) -> Result<(Self, String), HandleError> {
        Self::start(sandbox, journal, client_ids).wait()
    }

    /// Starts loading a note's document on a new thread from the records
    /// the journal holds for its lineage, without waiting for it. The note
    /// shows its file, which holds the last merged state, until
    /// [`Opening::poll`] hands over the handle and the document's text.
    pub fn start(sandbox: &Sandbox, journal: Vec<Vec<u8>>, client_ids: ClientIds) -> Opening {
        let records = journal
            .into_iter()
            .enumerate()
            .map(|(id, bytes)| crate::note::JournalRecord {
                id: id as u64,
                snapshot: id == 0,
                place: crate::note::Place::default(),
                bytes,
            })
            .collect();
        Self::start_lineage(
            sandbox,
            Opened {
                records,
                ..Opened::default()
            },
            client_ids,
        )
    }

    /// [`Self::start`] from the journal's records with their places, each
    /// writer's balance and where deferred records come back from.
    pub fn start_lineage(sandbox: &Sandbox, source: Opened, client_ids: ClientIds) -> Opening {
        let (requests, inbox) = mpsc::channel();
        let (outbox, replies) = mpsc::channel();
        let (opened_tx, opened) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let sandbox = sandbox.clone();
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("collab-sandbox".into())
            .spawn(move || {
                let mut note = match Note::open_lineage(&sandbox, source, client_ids) {
                    Ok(note) => note,
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                        return;
                    }
                };
                *lock(&thread_shared.status) = note.status();
                let text = note.text().map(|text| (text, note.len()));
                let ok = text.is_ok();
                if opened_tx.send(text).is_err() || !ok {
                    return;
                }
                serve(note, &inbox, &outbox, &thread_shared);
            });
        Opening {
            parts: Some((SharedNote { requests, shared }, replies)),
            opened: spawned.ok().map(|_| opened),
        }
    }

    fn with(shared: SharedNote, replies: Receiver<Reply>, len: u32) -> Self {
        Self {
            shared,
            replies,
            sent: 0,
            received: 0,
            outgoing: VecDeque::new(),
            len,
            broken: false,
            outlived: false,
            started: Instant::now(),
        }
    }

    /// The sync client's side of the note.
    pub fn shared(&self) -> SharedNote {
        self.shared.clone()
    }

    /// Lets the thread run on after this side drops, until the last
    /// [`SharedNote`] goes, so that closing a note's tab leaves the edits
    /// that wait in the document for the sync client to send.
    pub fn outlive(&mut self) {
        self.outlived = true;
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
        self.shared.send(Request::Edit {
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
        self.shared.send(Request::Undo { message, at })
    }

    /// Asks the document to reapply your last undone step.
    pub fn redo(&mut self) -> Result<(), HandleError> {
        let at = self.now();
        let message = self.message(Op::identity(self.len));
        self.shared.send(Request::Redo { message, at })
    }

    /// Ends the current undo step.
    pub fn end_undo_step(&mut self) -> Result<(), HandleError> {
        self.shared.send(Request::EndUndoStep)
    }

    /// Keeps an error for [`SharedNote::take_errors`].
    pub(crate) fn record(&self, error: HandleError) {
        lock(&self.shared.shared.errors).push(error);
    }

    /// Takes in one reply. Returns false once no further reply can apply.
    fn take_in(&mut self, reply: Reply, changes: &mut Vec<Op>) -> bool {
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
                    match transform(&op, pending) {
                        Ok((server, client)) => {
                            op = server;
                            *pending = client;
                        }
                        Err(mismatch) => {
                            self.broken = true;
                            self.record(mismatch.into());
                            return false;
                        }
                    }
                }
                self.received += 1;
                self.len = op.target_len();
                changes.push(op);
            }
            Reply::Failed(error) => self.record(error.into()),
        }
        true
    }

    /// The text changes to apply to the editor, in order, each against the
    /// editor's text as the ones before it leave it. An error a reply
    /// brings goes to [`SharedNote::take_errors`] and keeps the changes
    /// that came before it.
    pub fn poll(&mut self) -> Vec<Op> {
        let mut changes = Vec::new();
        while !self.broken {
            match self.replies.try_recv() {
                Ok(reply) => {
                    if !self.take_in(reply, &mut changes) {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.broken = true;
                    self.record(HandleError::Closed);
                }
            }
        }
        changes
    }

    /// Waits up to `timeout` for a change, then returns the changes to
    /// apply as [`Self::poll`] does.
    pub fn wait(&mut self, timeout: Duration) -> Vec<Op> {
        let mut changes = Vec::new();
        if self.broken {
            return changes;
        }
        match self.replies.recv_timeout(timeout) {
            Ok(reply) => {
                if !self.take_in(reply, &mut changes) {
                    return changes;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                self.broken = true;
                self.record(HandleError::Closed);
                return changes;
            }
        }
        changes.extend(self.poll());
        changes
    }

    /// Where the note's lineage stands, as of the last request the thread
    /// handled.
    pub fn status(&self) -> LineageStatus {
        self.shared.status()
    }

    /// Whether undo has a step of yours to take back, as of the last
    /// request the thread handled: an edit still in its queue doesn't count
    /// yet.
    pub fn can_undo(&self) -> bool {
        lock(&self.shared.shared.undo).0
    }

    /// Whether redo has a step of yours to reapply, as of the last request
    /// the thread handled.
    pub fn can_redo(&self) -> bool {
        lock(&self.shared.shared.undo).1
    }
}

impl Drop for NoteHandle {
    /// Ends the thread once it finishes the work ahead of it, without
    /// waiting for that work.
    fn drop(&mut self) {
        if !self.outlived {
            let _ = self.shared.send(Request::Close);
        }
    }
}

/// A note whose document is loading on its own thread: see
/// [`NoteHandle::start`].
pub struct Opening {
    /// The sides of the note, until the load ends.
    parts: Option<(SharedNote, Receiver<Reply>)>,
    /// Where the thread sends the note's text once the journal loads; none
    /// when the thread didn't start.
    opened: Option<Receiver<Result<(String, u32), DocumentError>>>,
}

type OpenedNote = Result<(NoteHandle, String), HandleError>;

impl Opening {
    /// The handle and the document's text once the journal loaded, without
    /// waiting: `None` while it still loads, and an error when it failed.
    /// After it returns `Some`, it returns [`HandleError::Closed`].
    pub fn poll(&mut self) -> Option<OpenedNote> {
        let result = match &self.opened {
            None => Err(HandleError::Closed),
            Some(opened) => match opened.try_recv() {
                Ok(result) => result.map_err(HandleError::from),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err(HandleError::Closed),
            },
        };
        Some(self.finish(result))
    }

    /// Waits until the journal loaded, then returns the handle and the
    /// document's text.
    pub fn wait(mut self) -> OpenedNote {
        let result = match &self.opened {
            None => Err(HandleError::Closed),
            Some(opened) => match opened.recv() {
                Ok(result) => result.map_err(HandleError::from),
                Err(_) => Err(HandleError::Closed),
            },
        };
        self.finish(result)
    }

    fn finish(&mut self, result: Result<(String, u32), HandleError>) -> OpenedNote {
        self.opened = None;
        let (text, len) = result?;
        let (shared, replies) = self.parts.take().ok_or(HandleError::Closed)?;
        Ok((NoteHandle::with(shared, replies, len), text))
    }
}

impl Drop for Opening {
    /// Ends the thread once the journal loads, when nobody took the note.
    fn drop(&mut self) {
        if let Some((shared, _)) = &self.parts {
            let _ = shared.send(Request::Close);
        }
    }
}

/// The sync client's side of a text-mode note: it passes remote records in,
/// takes local updates out and encodes positions for presence. A call that
/// needs an answer waits for the thread on a channel of its own, so it
/// holds up only the thread that made it, never the editor.
#[derive(Clone)]
pub struct SharedNote {
    requests: Sender<Request>,
    shared: Arc<Shared>,
}

impl SharedNote {
    fn send(&self, request: Request) -> Result<(), HandleError> {
        self.requests.send(request).map_err(|_| HandleError::Closed)
    }

    fn ask<T>(&self, request: impl FnOnce(Answer<T>) -> Request) -> Result<T, HandleError> {
        let (answer, answered) = mpsc::channel();
        self.send(request(answer))?;
        Ok(answered.recv().map_err(|_| HandleError::Closed)??)
    }

    /// Whether the note's thread still runs.
    pub fn is_alive(&self) -> bool {
        let (flushed, _) = mpsc::channel();
        self.requests.send(Request::Flush(flushed)).is_ok()
    }

    /// Hands the thread a remote record to judge and merge.
    pub fn receive(&self, record: RemoteRecord) -> Result<(), HandleError> {
        self.send(Request::Record(Box::new(record)))
    }

    /// Applies an outside edit of the file: the change from `base`, what
    /// the file held, to `text`, what it holds now, on top of what the
    /// document merged since `base`, which stays. Undo never takes it back,
    /// and it reaches the editor like a collaborator's. Waits for the
    /// requests ahead of it.
    pub fn merge_text(&self, base: String, text: String, at: u64) -> Result<(), HandleError> {
        self.ask(|answer| Request::Merge {
            base,
            text,
            at,
            answer,
        })
    }

    /// Each writer's balance, which the journal keeps.
    pub fn balances(&self) -> Result<Vec<(u32, i64)>, HandleError> {
        self.ask(Request::Balances)
    }

    /// Your changes since the last call, one update per load, for the
    /// `text_update`s that carry them. Waits for the requests ahead of it.
    pub fn take_updates(&self) -> Result<Vec<LocalUpdate>, HandleError> {
        self.ask(Request::TakeUpdates)
    }

    /// The document encoded as a snapshot holds it.
    pub fn state(&self) -> Result<Vec<u8>, HandleError> {
        self.ask(Request::State)
    }

    /// The document's merged text, once the thread handled everything sent
    /// before this call, which it waits for. The file follows
    /// [`Self::take_merged_changes`] instead, which never waits.
    pub fn merged_text(&self) -> Result<String, HandleError> {
        self.ask(Request::Text)
    }

    /// The changes the note's document merged since the last call, in the
    /// order it merged them, each against the text the one before it left:
    /// collaborators' records, undo and redo, and your edits once they merge
    /// after the records ahead of them. They are what the file gets:
    /// applied in turn to the body the note opened with, they give the
    /// document's merged text, so while a slow record decodes the file
    /// keeps its last merged state even as you type. The editor's own
    /// change events carry your edits before they merge, so they never
    /// feed a text-mode note's file. Once a record stops the lineage, no
    /// change follows and the file keeps its last good state.
    pub fn take_merged_changes(&self) -> Vec<Op> {
        std::mem::take(&mut *lock(&self.shared.merged))
    }

    /// The relative position of the document's UTF-16 offset `index`.
    pub fn encode_position(&self, index: u32, after: bool) -> Result<Vec<u8>, HandleError> {
        self.ask(|answer| Request::EncodePosition {
            index,
            after,
            answer,
        })
    }

    /// The document's UTF-16 offset a collaborator's relative position
    /// points at.
    pub fn decode_position(&self, bytes: Vec<u8>) -> Result<u32, HandleError> {
        self.ask(|answer| Request::DecodePosition { bytes, answer })
    }

    /// Waits until the thread has handled everything sent so far; the
    /// changes it made wait for the editor's next poll.
    pub fn settle(&self) -> Result<(), HandleError> {
        let (flushed, done) = mpsc::channel();
        self.send(Request::Flush(flushed))?;
        done.recv().map_err(|_| HandleError::Closed)
    }

    /// Moves the thread's clock forward by `by`, running the deferred
    /// records whose writers' allowances refill by then, as tests do in
    /// place of waiting.
    #[cfg(any(test, feature = "test-support"))]
    pub fn advance_clock(&self, by: Duration) -> Result<(), HandleError> {
        self.send(Request::Advance(by))
    }

    /// Sets a writer's allowance balance, as the fixtures' base states do.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_balance(&self, writer: u32, balance: i64) -> Result<(), HandleError> {
        self.send(Request::SetBalance { writer, balance })
    }

    /// Makes the instance trap while it integrates the next record that
    /// passes the verdict, as the fixtures' trap cases ask.
    #[cfg(any(test, feature = "test-support"))]
    pub fn trap_next_integration(&self) -> Result<(), HandleError> {
        self.send(Request::TrapNext)
    }

    /// Judges the records sent after this call with `sandbox`'s module.
    #[cfg(test)]
    pub(crate) fn judge_with(&self, sandbox: Sandbox) -> Result<(), HandleError> {
        self.send(Request::Verdicts(sandbox))
    }

    /// Pauses the thread before its next request while `held`, as tests do
    /// to stand in for a record that takes long to decode.
    #[cfg(any(test, feature = "test-support"))]
    pub fn hold(&self, held: bool) {
        self.shared.hold.set(held);
    }

    /// Where the note's lineage stands, as of the last request the thread
    /// handled.
    pub fn status(&self) -> LineageStatus {
        *lock(&self.shared.status)
    }

    /// The events the thread reported since the last call.
    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *lock(&self.shared.events))
    }

    /// The errors the editor's side met since the last call. An edit that
    /// failed is in the editor but not in the document, so the app leaves
    /// text mode.
    pub fn take_errors(&self) -> Vec<HandleError> {
        std::mem::take(&mut *lock(&self.shared.errors))
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

/// Sends a request's answer, once the thread published the changes, events
/// and status the request left.
type Respond = Box<dyn FnOnce()>;

fn answer<T: 'static>(answer: Answer<T>, result: Result<T, DocumentError>) -> Option<Respond> {
    Some(Box::new(move || {
        // The caller may have stopped waiting.
        let _ = answer.send(result);
    }))
}

fn serve(mut note: Note, inbox: &Receiver<Request>, outbox: &Sender<Reply>, shared: &Shared) {
    let mut server = Server::default();
    // How far tests moved the clock forward.
    #[cfg_attr(not(any(test, feature = "test-support")), allow(unused_mut))]
    let mut skew = Duration::ZERO;
    loop {
        let request = match note.next_wake() {
            Some(at) => {
                match inbox.recv_timeout(at.saturating_duration_since(Instant::now() + skew)) {
                    Ok(request) => Some(request),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            None => match inbox.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            },
        };
        shared.hold.wait();
        #[cfg(any(test, feature = "test-support"))]
        if let Some(Request::Advance(by)) = &request {
            skew += *by;
        }
        let now = Instant::now() + skew;
        // A stopped lineage's file keeps its last good state.
        let stopped = note.is_stopped();
        let mut effects = Effects::default();
        // Your edit, as the document merged it.
        let mut merged_edit = None;
        let mut failed = None;
        let mut reply: Option<Respond> = None;
        match request {
            None => effects = note.wake(now),
            Some(Request::Close) => return,
            Some(Request::Record(record)) => effects = note.receive(*record, now),
            Some(Request::Edit {
                message,
                undoable,
                at,
            }) => {
                let result = server
                    .take_in(message)
                    .map_err(DocumentError::from)
                    .and_then(|op| note.local(&op, undoable, at).map(|()| op));
                match result {
                    Ok(op) => merged_edit = Some(op),
                    Err(error) => failed = Some(error),
                }
            }
            Some(Request::Undo { message, at }) => {
                let _ = server.take_in(message);
                match note.undo(at) {
                    Ok(op) => effects.changes.push(op),
                    Err(error) => failed = Some(error),
                }
            }
            Some(Request::Redo { message, at }) => {
                let _ = server.take_in(message);
                match note.redo(at) {
                    Ok(op) => effects.changes.push(op),
                    Err(error) => failed = Some(error),
                }
            }
            Some(Request::EndUndoStep) => failed = note.end_undo_step().err(),
            Some(Request::Merge {
                base,
                text,
                at,
                answer: to,
            }) => {
                let result = note.text().and_then(|current| {
                    let (op, _) = three_way(&base, &text, &current)?;
                    note.local(&op, false, at).map(|()| op)
                });
                reply = match result {
                    Ok(op) => {
                        effects.changes.push(op);
                        answer(to, Ok(()))
                    }
                    Err(error) => answer(to, Err(error)),
                };
            }
            Some(Request::Balances(to)) => reply = answer(to, Ok(note.balances(now))),
            Some(Request::TakeUpdates(to)) => reply = answer(to, note.take_updates()),
            Some(Request::State(to)) => reply = answer(to, note.state()),
            Some(Request::Text(to)) => reply = answer(to, note.text()),
            Some(Request::EncodePosition {
                index,
                after,
                answer: to,
            }) => reply = answer(to, note.encode_position(index, after)),
            Some(Request::DecodePosition { bytes, answer: to }) => {
                reply = answer(to, note.decode_position(&bytes))
            }
            Some(Request::Flush(to)) => {
                reply = Some(Box::new(move || {
                    let _ = to.send(());
                }))
            }
            #[cfg(any(test, feature = "test-support"))]
            Some(Request::Advance(_)) => effects = note.wake(now),
            #[cfg(any(test, feature = "test-support"))]
            Some(Request::SetBalance { writer, balance }) => note.set_balance(writer, balance, now),
            #[cfg(any(test, feature = "test-support"))]
            Some(Request::TrapNext) => note.trap_next_integration(),
            #[cfg(test)]
            Some(Request::Verdicts(sandbox)) => note.judge_with(sandbox),
        }
        if !stopped {
            let merged = merged_edit.iter().chain(&effects.changes);
            let merged: Vec<Op> = merged.filter(|op| !op.is_identity()).cloned().collect();
            if !merged.is_empty() {
                lock(&shared.merged).extend(merged);
            }
        }
        for op in effects.changes {
            if !op.is_identity() && outbox.send(Reply::Change(server.send_out(op))).is_err() {
                return;
            }
        }
        if let Some(error) = failed
            && outbox.send(Reply::Failed(error)).is_err()
        {
            return;
        }
        if !effects.events.is_empty() {
            lock(&shared.events).extend(effects.events);
        }
        // The status goes before the answer, so a caller that waits for its
        // answer sees the status the call left.
        *lock(&shared.status) = note.status();
        if let Ok(undo) = note.undo_state() {
            *lock(&shared.undo) = undo;
        }
        if let Some(reply) = reply {
            reply();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::Splice;

    /// A handle whose replies come from the test instead of a thread, with
    /// the channels standing in for the thread's.
    fn detached(len: u32) -> (NoteHandle, Sender<Reply>, Receiver<Request>) {
        let (requests, inbox) = mpsc::channel();
        let (outbox, replies) = mpsc::channel();
        let handle = NoteHandle {
            shared: SharedNote {
                requests,
                shared: Arc::default(),
            },
            replies,
            sent: 0,
            received: 0,
            outgoing: VecDeque::new(),
            len,
            broken: false,
            outlived: false,
            started: Instant::now(),
        };
        (handle, outbox, inbox)
    }

    fn insert(len: u32, at: u32, text: &str) -> Op {
        Op::from_splices(
            len,
            &[Splice {
                start: at,
                removed: 0,
                inserted: text.into(),
            }],
        )
        .unwrap()
    }

    fn change(seen: u64, op: Op) -> Reply {
        Reply::Change(ServerMessage { seen, op })
    }

    #[test]
    fn a_failure_amid_changes_keeps_the_changes_on_both_sides_of_it() {
        let (mut handle, thread, _inbox) = detached(2);
        thread.send(change(0, insert(2, 0, "a"))).unwrap();
        thread.send(Reply::Failed(DocumentError::Position)).unwrap();
        thread.send(change(0, insert(3, 3, "b"))).unwrap();
        let changes = handle.poll();
        assert_eq!(changes, [insert(2, 0, "a"), insert(3, 3, "b")]);
        assert_eq!(handle.len(), 4);
        assert_eq!(
            handle.shared().take_errors(),
            [HandleError::Document(DocumentError::Position)]
        );
    }

    #[test]
    fn a_change_that_no_longer_fits_keeps_the_changes_before_it() {
        let (mut handle, thread, _inbox) = detached(2);
        // You type at the end while the thread sends three changes made
        // before it saw your edit.
        handle.edit(insert(2, 2, "e"), true).unwrap();
        thread.send(change(0, insert(2, 0, "a"))).unwrap();
        // One against a text of another length than the thread's.
        thread.send(change(0, insert(9, 0, "x"))).unwrap();
        thread.send(change(0, insert(4, 0, "c"))).unwrap();
        let changes = handle.poll();
        assert_eq!(changes, [insert(3, 0, "a")]);
        assert_eq!(
            handle.shared().take_errors(),
            [HandleError::Mismatch(Mismatch)]
        );
        // Nothing more applies once the two sides went apart.
        thread.send(change(0, insert(4, 0, "d"))).unwrap();
        assert_eq!(handle.poll(), []);
    }

    #[test]
    fn a_gone_thread_is_reported_once() {
        let (mut handle, thread, _inbox) = detached(0);
        thread.send(change(0, insert(0, 0, "a"))).unwrap();
        drop(thread);
        assert_eq!(handle.poll(), [insert(0, 0, "a")]);
        assert_eq!(handle.poll(), []);
        assert_eq!(handle.shared().take_errors(), [HandleError::Closed]);
    }
}
