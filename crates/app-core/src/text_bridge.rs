//! What the windows and the sync thread say to each other about text-mode
//! notes (docs/workspace-format/sync-v2.md §22, §23). A window asks for a
//! note's document on its first edit and sends its selection; the sync
//! thread answers with the document and the collaborators' cursors. Neither
//! side waits for the other.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use collab_sandbox::SandboxDocument;
use collab_sandbox::op::difference;
use editor_core::text_mode::{CollabDocument, Utf16Splice};
use sync_protocol::types::Id;

use crate::sync::workspace::WorkspaceSync;
use crate::sync::workspace::presence::{Cursor, SelectionRange};
use crate::sync::workspace::text::Entered;

/// A note's document, on its way to the editor.
pub struct TextDocument {
    /// The note.
    pub rid: Id,
    document: SandboxDocument,
    /// The text the document holds.
    body: String,
}

impl std::fmt::Debug for TextDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextDocument")
            .field("rid", &self.rid)
            .finish_non_exhaustive()
    }
}

impl TextDocument {
    /// The document, caught up with `shown`, the editor's text, which holds
    /// the edit that asked for it.
    pub fn catch_up(mut self, shown: &str) -> Box<dyn CollabDocument> {
        let splices: Vec<Utf16Splice> = difference(&self.body, shown)
            .splices()
            .into_iter()
            .map(|splice| Utf16Splice {
                start: splice.start as usize,
                removed: splice.removed as usize,
                inserted: splice.inserted,
            })
            .collect();
        if !splices.is_empty() {
            self.document.edit(splices, true);
        }
        Box::new(self.document)
    }
}

#[derive(Default)]
struct Waiting {
    /// Notes a window asked for.
    opening: Vec<String>,
    /// Documents that wait for their window, by the path asked for.
    ready: BTreeMap<String, TextDocument>,
    /// Paths whose note stays outside text mode for now.
    refused: BTreeMap<String, Entered>,
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
    pub fn open(&self, path: &str) {
        let mut waiting = self.waiting();
        let asked = waiting.opening.iter().any(|p| p == path)
            || waiting.ready.contains_key(path)
            || waiting.refused.contains_key(path);
        if !asked {
            waiting.opening.push(path.to_owned());
        }
        drop(waiting);
        self.wake();
    }

    /// The document of the note at `path`, once it's ready.
    pub fn take_document(&self, path: &str) -> Option<TextDocument> {
        self.waiting().ready.remove(path)
    }

    /// Whether a window waits for the sync thread.
    pub fn has_work(&self) -> bool {
        let waiting = self.waiting();
        !waiting.opening.is_empty() || !waiting.closing.is_empty()
    }

    /// Lets the next edit ask again for a note that stayed out of text mode.
    pub fn ask_again(&self, path: &str) {
        self.waiting().refused.remove(path);
    }

    /// The window let go of the note.
    pub fn close(&self, rid: Id) {
        let mut waiting = self.waiting();
        waiting.closing.push(rid);
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

    /// The sync thread's turn: opens what the windows asked for, passes on
    /// selections and collects cursors.
    pub fn serve(&self, sync: &mut WorkspaceSync) {
        let (opening, closing, selections) = {
            let mut waiting = self.waiting();
            (
                std::mem::take(&mut waiting.opening),
                std::mem::take(&mut waiting.closing),
                waiting.selections.clone(),
            )
        };
        for path in opening {
            match sync.open_text(&path) {
                Ok((Entered::Yes, Some(document))) => {
                    let Some(rid) = sync.rid_at(&path) else {
                        continue;
                    };
                    let body = sync.text_body(&rid).unwrap_or_default().to_owned();
                    self.waiting().ready.insert(
                        path,
                        TextDocument {
                            rid,
                            document,
                            body,
                        },
                    );
                }
                Ok((entered, _)) => {
                    self.waiting().refused.insert(path, entered);
                }
                Err(error) => {
                    tracing::warn!(kind = ?error.code(), "a note's document didn't open");
                    self.waiting().refused.insert(path, Entered::NotYet);
                }
            }
        }
        for rid in closing {
            if let Err(error) = sync.close_text_note(rid) {
                tracing::warn!(kind = ?error.code(), "a note's document didn't close");
            }
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
}
