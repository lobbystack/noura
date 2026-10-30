//! The editor's side of a text-mode note: [`SandboxDocument`] carries the
//! edits `editor-core` makes into the note's document in the sandbox, and
//! the document's changes back, through a [`NoteHandle`].

use editor_core::text_mode::{CollabDocument, Utf16Splice};

use crate::op::{Mismatch, Op, Splice};
use crate::session::{HandleError, NoteHandle, SharedNote};

/// A text-mode note's document as `editor-core` sees it: hand it to
/// [`editor_core::MdDoc::enter_text_mode`], and keep [`Self::handle`] for
/// the sync client, which passes remote records in and takes local updates
/// out. None of its calls waits for the sandbox's thread.
pub struct SandboxDocument {
    handle: NoteHandle,
}

impl SandboxDocument {
    /// Wraps a note's handle.
    pub fn new(handle: NoteHandle) -> Self {
        Self { handle }
    }

    /// The sync client's side of the note.
    pub fn handle(&self) -> SharedNote {
        self.handle.shared()
    }

    fn record(&self, result: Result<(), HandleError>) {
        if let Err(error) = result {
            self.handle.record(error);
        }
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
            let splices: Vec<Splice> = splices
                .into_iter()
                .map(to_splice)
                .collect::<Option<_>>()
                .ok_or(HandleError::Mismatch(Mismatch))?;
            let op = Op::from_splices(self.handle.len(), &splices)?;
            self.handle.edit(op, undoable)
        })();
        self.record(result);
    }

    fn undo(&mut self) {
        let result = self.handle.undo();
        self.record(result);
    }

    fn redo(&mut self) {
        let result = self.handle.redo();
        self.record(result);
    }

    fn can_undo(&self) -> bool {
        self.handle.can_undo()
    }

    fn can_redo(&self) -> bool {
        self.handle.can_redo()
    }

    fn poll(&mut self) -> Vec<Utf16Splice> {
        self.handle
            .poll()
            .iter()
            .flat_map(Op::splices)
            .map(from_splice)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use editor_core::MdDoc;
    use serde_json::Value;
    use sync_protocol::types::from_hex;

    use super::*;
    use crate::{Backend, Event, LineageStatus, RemoteRecord, Sandbox};

    fn hex(value: &Value) -> Vec<u8> {
        from_hex(value.as_str().expect("a hex string")).expect("hexadecimal")
    }

    /// Seam 2 for the fixture's verdict trap cases: a stand-in module whose
    /// `verdict` export traps, as a guest that panics would, judges a record.
    /// The record is unreadable and never reaches the editor, and the next
    /// valid records, which the pinned module judges, reach it.
    #[test]
    fn a_trap_in_the_verdict_leaves_the_editor_alone_and_the_next_record_applies() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/workspace-format/fixtures/sync-v2-collaboration.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let cases: Vec<&Value> = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["kind"] == "verdict_trap")
            .collect();
        assert_eq!(cases.len(), 1);
        for backend in [
            Backend::Cranelift,
            #[cfg(feature = "pulley")]
            Backend::Pulley,
        ] {
            let pinned = Sandbox::new(backend).unwrap();
            for case in &cases {
                let name = &case["name"];
                let stand_in =
                    Sandbox::with_module(backend, case["module_wat"].as_str().unwrap().as_bytes())
                        .unwrap();
                let mut load = 0;
                let (handle, text) = NoteHandle::open(
                    &pinned,
                    vec![hex(&case["base"])],
                    Box::new(move || {
                        load += 1;
                        Some((3 << 29) + load)
                    }),
                )
                .unwrap();
                let mut doc = MdDoc::new(&text);
                let document = SandboxDocument::new(handle);
                let note = document.handle();
                doc.enter_text_mode(Box::new(document));
                for step in case["steps"].as_array().unwrap() {
                    let before = doc.text().to_string();
                    let _ = doc.take_changes();
                    let judge = if step["judge"] == "stand_in" {
                        &stand_in
                    } else {
                        &pinned
                    };
                    note.judge_with(judge.clone()).unwrap();
                    let receive = &step["receive"];
                    let id = receive["id"].as_u64().unwrap();
                    note.receive(RemoteRecord {
                        id,
                        writer: receive["card_index"].as_u64().unwrap() as u32,
                        card_count: receive["card_count"].as_u64().unwrap() as u32,
                        payload: hex(&receive["payload"]),
                        blob: None,
                        place: Default::default(),
                    })
                    .unwrap();
                    note.settle().unwrap();
                    doc.sync_text_mode();
                    let events = note.take_events();
                    let outcome = events
                        .iter()
                        .find_map(|event| match event {
                            Event::Verdict { verdict, .. } => Some(verdict.outcome),
                            _ => None,
                        })
                        .expect("a verdict");
                    let merged = events
                        .iter()
                        .any(|event| matches!(event, Event::Merged { .. }));
                    let got = match outcome {
                        Err(reason) => reason.code(),
                        Ok(()) if merged => "merged",
                        Ok(()) => "other",
                    };
                    assert_eq!(got, step["expect"], "{name} record {id}");
                    if outcome.is_err() {
                        assert_eq!(doc.text(), before, "{name} record {id}");
                        assert_eq!(doc.take_changes(), [], "{name} record {id}");
                    }
                }
                assert_eq!(doc.text(), case["text"].as_str().unwrap(), "{name}");
                assert_eq!(note.merged_text().unwrap(), doc.text(), "{name}");
                assert_eq!(note.status(), LineageStatus::Merging, "{name}");
                assert_eq!(note.take_errors(), [], "{name}");
                // Closing the note ends its thread once the requests ahead
                // of it are done, so the sync client's next call reports it.
                drop(doc);
                assert_eq!(note.settle(), Err(HandleError::Closed), "{name}");
            }
        }
    }
}
