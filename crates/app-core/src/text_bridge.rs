//! What the windows and the sync thread say to each other about text-mode
//! notes (docs/workspace-format/sync-v2.md §22, §23). A window asks for a
//! note's document on its first edit and sends its selection; the sync
//! thread answers with the document and the collaborators' cursors. Neither
//! side waits for the other.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use collab_sandbox::SandboxDocument;
use collab_sandbox::op::{Op, three_way};
use editor_core::text_mode::{CollabDocument, Utf16Splice};
use sync_protocol::types::Id;

use crate::sync::workspace::WorkspaceSync;
use crate::sync::workspace::presence::{Cursor, SelectionRange};
use crate::sync::workspace::text::{Entered, LeftNote};

/// How long a note that couldn't enter text mode waits before an edit asks
/// again.
const ASK_AGAIN: Duration = Duration::from_secs(2);

/// The sync client's side of a test's document.
#[cfg(feature = "test-hooks")]
pub type TestNote = collab_sandbox::SharedNote;

/// A note's document, on its way to the editor.
pub struct TextDocument {
    /// The note.
    pub rid: Id,
    document: SandboxDocument,
    /// The text the document's changes apply to.
    text: String,
}

impl std::fmt::Debug for TextDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextDocument")
            .field("rid", &self.rid)
            .finish_non_exhaustive()
    }
}

impl TextDocument {
    /// The document, caught up with the editor. `base` is the text the
    /// editor last loaded and `shown` what it holds now, so your edits are
    /// the difference between them. They go to the document on top of what
    /// it merged since `base`, and the editor takes what the document holds
    /// beyond `base` on its first poll.
    pub fn catch_up(mut self, base: &str, shown: &str) -> Box<dyn CollabDocument> {
        for change in self.document.take_changes() {
            match change.apply(&self.text) {
                Ok(text) => self.text = text,
                // The editor's side went apart from the document, so the
                // next turn of the sync client takes it out of text mode.
                Err(_) => break,
            }
        }
        match three_way(base, shown, &self.text) {
            Ok((mine, theirs)) => {
                let mine = splices(&mine);
                if !mine.is_empty() {
                    self.document.edit(mine, true);
                }
                Box::new(Caught {
                    document: self.document,
                    first: splices(&theirs),
                })
            }
            Err(_) => Box::new(self.document),
        }
    }
}

fn splices(op: &Op) -> Vec<Utf16Splice> {
    op.splices()
        .into_iter()
        .map(|splice| Utf16Splice {
            start: splice.start as usize,
            removed: splice.removed as usize,
            inserted: splice.inserted,
        })
        .collect()
}

/// A document whose first poll carries what the editor hasn't shown yet.
struct Caught {
    document: SandboxDocument,
    first: Vec<Utf16Splice>,
}

impl CollabDocument for Caught {
    fn edit(&mut self, splices: Vec<Utf16Splice>, undoable: bool) {
        self.document.edit(splices, undoable);
    }

    fn undo(&mut self) {
        self.document.undo();
    }

    fn redo(&mut self) {
        self.document.redo();
    }

    fn can_undo(&self) -> bool {
        self.document.can_undo()
    }

    fn can_redo(&self) -> bool {
        self.document.can_redo()
    }

    fn poll(&mut self) -> Vec<Utf16Splice> {
        let mut changes = std::mem::take(&mut self.first);
        changes.extend(self.document.poll());
        changes
    }
}

#[derive(Default)]
struct Waiting {
    /// Notes a window asked for, which enter text mode.
    opening: Vec<String>,
    /// Notes a window shows, whose documents it takes if they have one.
    attaching: Vec<String>,
    /// Documents that wait for their window, by the path asked for.
    ready: BTreeMap<String, TextDocument>,
    /// Paths whose note stays outside text mode for now, and since when.
    refused: BTreeMap<String, (Entered, Instant)>,
    /// Documents that stopped taking the windows' edits.
    left: BTreeMap<Id, LeftNote>,
    /// Notes the windows let go of.
    closing: Vec<Id>,
    /// Each open note's selections, and whether you're typing.
    selections: BTreeMap<Id, (Vec<SelectionRange>, bool)>,
    /// Each open note's collaborators, as of the last turn.
    cursors: BTreeMap<Id, Vec<Cursor>>,
}

/// The two sides' shared state. Every lock lasts only as long as a copy.
#[derive(Default)]
pub struct TextBridge {
    waiting: Mutex<Waiting>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl std::fmt::Debug for TextBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextBridge")
    }
}

impl TextBridge {
    fn waiting(&self) -> MutexGuard<'_, Waiting> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Calls `wake` when a window needs the sync thread.
    pub fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(wake);
    }

    fn wake(&self) {
        let wake = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Asks for the document of the note at `path`, which you just edited.
    /// A note that stayed out of text mode isn't asked for again until
    /// [`Self::ask_again`], or a moment later when it may have changed.
    pub fn open(&self, path: &str) {
        let mut waiting = self.waiting();
        if waiting
            .refused
            .get(path)
            .is_some_and(|(entered, at)| *entered != Entered::Never && at.elapsed() >= ASK_AGAIN)
        {
            waiting.refused.remove(path);
        }
        let asked = waiting.opening.iter().any(|p| p == path)
            || waiting.ready.contains_key(path)
            || waiting.refused.contains_key(path);
        if !asked {
            waiting.opening.push(path.to_owned());
        }
        drop(waiting);
        self.wake();
    }

    /// Asks for the document of the note at `path` if the note is in text
    /// mode: a window shows it, and cursors and edits reach it from there.
    pub fn attach(&self, path: &str) {
        let mut waiting = self.waiting();
        if !waiting.attaching.iter().any(|p| p == path) {
            waiting.attaching.push(path.to_owned());
        }
        drop(waiting);
        self.wake();
    }

    /// What ended the window's hold on the document of `rid`, when it did.
    pub fn take_left(&self, rid: &Id) -> Option<LeftNote> {
        self.waiting().left.remove(rid)
    }

    /// The document of the note at `path`, once it's ready.
    pub fn take_document(&self, path: &str) -> Option<TextDocument> {
        self.waiting().ready.remove(path)
    }

    /// Whether a window waits for the sync thread.
    pub fn has_work(&self) -> bool {
        let waiting = self.waiting();
        !waiting.opening.is_empty() || !waiting.attaching.is_empty() || !waiting.closing.is_empty()
    }

    /// Lets the next edit ask again for a note that stayed out of text
    /// mode, once its file changed.
    pub fn ask_again(&self, path: &str) {
        self.waiting().refused.remove(path);
    }

    /// The window let go of the note.
    pub fn close(&self, rid: Id) {
        let mut waiting = self.waiting();
        waiting.closing.push(rid);
        waiting.left.remove(&rid);
        waiting.selections.remove(&rid);
        waiting.cursors.remove(&rid);
        drop(waiting);
        self.wake();
    }

    /// Your selections in the note, in UTF-16 offsets.
    pub fn set_selections(&self, rid: Id, ranges: Vec<SelectionRange>, typing: bool) {
        let mut waiting = self.waiting();
        let selection = (ranges, typing);
        if waiting.selections.get(&rid) != Some(&selection) {
            waiting.selections.insert(rid, selection);
        }
    }

    /// The collaborators' cursors in the note.
    pub fn cursors(&self, rid: &Id) -> Vec<Cursor> {
        self.waiting().cursors.get(rid).cloned().unwrap_or_default()
    }

    /// Hands the note's document to the window that asked, or remembers
    /// why it stays out of text mode.
    fn open_note(&self, sync: &mut WorkspaceSync, path: String) {
        match sync.open_text_at(&path) {
            Ok((Entered::Yes, Some((document, text)))) => {
                let Some(rid) = sync.rid_at(&path) else {
                    return;
                };
                self.waiting().ready.insert(
                    path,
                    TextDocument {
                        rid,
                        document,
                        text,
                    },
                );
            }
            // Another window holds the document: its edits reach the note,
            // and this one saves its own.
            Ok((Entered::Yes, None)) => {}
            Ok((entered, _)) => {
                self.waiting()
                    .refused
                    .insert(path, (entered, Instant::now()));
            }
            Err(error) => {
                tracing::warn!(kind = ?error.code(), "a note's document didn't open");
                self.waiting()
                    .refused
                    .insert(path, (Entered::NotYet, Instant::now()));
            }
        }
    }

    /// The sync thread's turn: lets go of the notes the windows closed,
    /// opens what they asked for, tells them of documents that ended,
    /// passes on selections and collects cursors.
    pub fn serve(&self, sync: &mut WorkspaceSync) {
        let (attaching, opening, closing, selections) = {
            let mut waiting = self.waiting();
            (
                std::mem::take(&mut waiting.attaching),
                std::mem::take(&mut waiting.opening),
                std::mem::take(&mut waiting.closing),
                waiting.selections.clone(),
            )
        };
        for rid in closing {
            if let Err(error) = sync.close_text_note(rid) {
                tracing::warn!(kind = ?error.code(), "a note's document didn't close");
            }
        }
        for path in attaching {
            if sync
                .rid_at(&path)
                .is_some_and(|rid| sync.in_text_mode(&rid))
            {
                self.open_note(sync, path);
            }
        }
        for path in opening {
            self.open_note(sync, path);
        }
        let left = sync.take_left_notes();
        if !left.is_empty() {
            let mut waiting = self.waiting();
            for note in left {
                // A window that hasn't taken the document never needs to
                // leave it.
                let unclaimed = waiting
                    .ready
                    .iter()
                    .find(|(_, document)| document.rid == note.rid)
                    .map(|(path, _)| path.clone());
                match unclaimed {
                    Some(path) => {
                        waiting.ready.remove(&path);
                    }
                    None => {
                        waiting.left.insert(note.rid, note);
                    }
                }
            }
            drop(waiting);
            self.wake();
        }
        let now = Instant::now();
        let mut cursors = BTreeMap::new();
        for (rid, (ranges, typing)) in selections {
            sync.set_note_selections(rid, ranges, typing);
            cursors.insert(rid, sync.note_cursors(rid, now));
        }
        let changed = {
            let mut waiting = self.waiting();
            let changed = waiting.cursors != cursors;
            waiting.cursors = cursors;
            changed
        };
        if changed {
            self.wake();
        }
    }

    /// A document for a note, as the sync thread would hand a window one,
    /// with the sync client's side of it.
    #[cfg(feature = "test-hooks")]
    pub fn document_for_testing(rid: Id, text: &str) -> (TextDocument, collab_sandbox::SharedNote) {
        use collab_sandbox::{Backend, NoteHandle, Sandbox};
        let sandbox = Sandbox::new(Backend::Cranelift).expect("the sandbox compiles");
        let first = sandbox.first_document(text).expect("a first document");
        let mut load = 0;
        let ids: collab_sandbox::ClientIds = Box::new(move || {
            load += 1;
            Some((1 << 29) + load)
        });
        let (handle, text) = NoteHandle::open(&sandbox, vec![first], ids).expect("the note opens");
        let shared = handle.shared();
        (
            TextDocument {
                rid,
                document: SandboxDocument::new(handle),
                text,
            },
            shared,
        )
    }

    /// Hands the window that asked for the note at `path` its document.
    #[cfg(feature = "test-hooks")]
    pub fn deliver_for_testing(&self, path: &str, document: TextDocument) {
        self.waiting().ready.insert(path.to_owned(), document);
    }

    /// The notes windows asked to open since the last call.
    #[cfg(feature = "test-hooks")]
    pub fn take_opening_for_testing(&self) -> Vec<String> {
        let mut waiting = self.waiting();
        let mut paths = std::mem::take(&mut waiting.opening);
        paths.append(&mut waiting.attaching);
        paths
    }

    /// Ends a window's hold on a document, as the sync thread would.
    #[cfg(feature = "test-hooks")]
    pub fn leave_for_testing(&self, note: LeftNote) {
        self.waiting().left.insert(note.rid, note);
    }

    /// Sets the collaborators' cursors a window shows.
    #[cfg(feature = "test-hooks")]
    pub fn set_cursors_for_testing(&self, rid: Id, cursors: Vec<Cursor>) {
        self.waiting().cursors.insert(rid, cursors);
    }
}
