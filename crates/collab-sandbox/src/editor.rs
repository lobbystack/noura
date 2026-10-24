//! The editor's side of a text-mode note: [`SandboxDocument`] carries the
//! edits `editor-core` makes into the note's document in the sandbox, and
//! the document's changes back, through a [`NoteHandle`].

use std::sync::{Arc, Mutex, MutexGuard};

use editor_core::text_mode::{CollabDocument, Utf16Splice};

use crate::op::{Op, Splice};
use crate::session::{HandleError, NoteHandle};

/// A text-mode note's document as `editor-core` sees it: hand it to
/// [`editor_core::MdDoc::enter_text_mode`], and keep [`Self::handle`] for
/// the sync client, which passes remote records in and takes local updates
/// out.
pub struct SandboxDocument {
    handle: Arc<Mutex<NoteHandle>>,
    errors: Arc<Mutex<Vec<HandleError>>>,
}

/// The sync client's side of a [`SandboxDocument`].
#[derive(Clone)]
pub struct SharedNote {
    handle: Arc<Mutex<NoteHandle>>,
    errors: Arc<Mutex<Vec<HandleError>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl SandboxDocument {
    /// Wraps a note's handle.
    pub fn new(handle: NoteHandle) -> Self {
        Self {
            handle: Arc::new(Mutex::new(handle)),
            errors: Arc::default(),
        }
    }

    /// The handle, shared with the editor's side.
    pub fn handle(&self) -> SharedNote {
        SharedNote {
            handle: self.handle.clone(),
            errors: self.errors.clone(),
        }
    }

    fn record(&self, result: Result<(), HandleError>) {
        if let Err(error) = result {
            lock(&self.errors).push(error);
        }
    }
}

impl SharedNote {
    /// Locks the note's handle.
    pub fn lock(&self) -> MutexGuard<'_, NoteHandle> {
        lock(&self.handle)
    }

    /// The errors the editor's calls met since the last call. An edit that
    /// failed is in the editor but not in the document, so the app leaves
    /// text mode.
    pub fn take_errors(&self) -> Vec<HandleError> {
        std::mem::take(&mut *lock(&self.errors))
    }
}

fn to_splice(splice: Utf16Splice) -> Option<Splice> {
    Some(Splice {
        start: u32::try_from(splice.start).ok()?,
        removed: u32::try_from(splice.removed).ok()?,
        inserted: splice.inserted,
    })
}

fn from_splice(splice: Splice) -> Utf16Splice {
    Utf16Splice {
        start: splice.start as usize,
        removed: splice.removed as usize,
        inserted: splice.inserted,
    }
}

impl CollabDocument for SandboxDocument {
    fn edit(&mut self, splices: Vec<Utf16Splice>, undoable: bool) {
        let result = (|| {
            let mut handle = lock(&self.handle);
            let splices: Vec<Splice> = splices
                .into_iter()
                .map(to_splice)
                .collect::<Option<_>>()
                .ok_or(HandleError::Mismatch(crate::op::Mismatch))?;
            let op = Op::from_splices(handle.len(), &splices)?;
            handle.edit(op, undoable)
        })();
        self.record(result);
    }

    fn undo(&mut self) {
        let result = lock(&self.handle).undo();
        self.record(result);
    }

    fn redo(&mut self) {
        let result = lock(&self.handle).redo();
        self.record(result);
    }

    fn poll(&mut self) -> Vec<Utf16Splice> {
        let result = lock(&self.handle).poll();
        match result {
            Ok(ops) => ops.iter().flat_map(Op::splices).map(from_splice).collect(),
            Err(error) => {
                lock(&self.errors).push(error);
                Vec::new()
            }
        }
    }
}
