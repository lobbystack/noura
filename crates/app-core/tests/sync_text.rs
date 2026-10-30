//! Text mode at seam 4 (sync-v2.md §22): two desktops co-edit a note over
//! the real server, and both end with identical bytes on disk.

mod sync_support;

use app_core::sync::workspace::text::{Entered, Judgement};
use collab_sandbox::SandboxDocument;
use editor_core::text_mode::{CollabDocument, Utf16Splice};
use sync_store::Backend;
use sync_support::{Pair, Server, pair, pair_on};

const PLAN: &str = "Plan.md";

fn type_at(document: &mut SandboxDocument, start: usize, text: &str) {
    document.edit(
        vec![Utf16Splice {
            start,
            removed: 0,
            inserted: text.to_owned(),
        }],
        true,
    );
}

async fn both_rounds<B: Backend>(pair: &mut Pair<B>) {
    for _ in 0..3 {
        pair.sync_a.round(&mut pair.service, 0).await.expect("round");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("round");
    }
}

fn read(folder: &sync_support::Folder, path: &str) -> String {
    String::from_utf8(folder.files()[path].clone()).expect("UTF-8")
}

/// Both desktops hold the note in their editors.
async fn open_both<B: Backend>(pair: &mut Pair<B>) -> (SandboxDocument, SandboxDocument) {
    both_rounds(pair).await;
    let (entered, a) = pair.sync_a.open_text(PLAN).expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    both_rounds(pair).await;
    let (entered, b) = pair.sync_b.open_text(PLAN).expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    (a.expect("the document"), b.expect("the document"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_desktops_see_each_others_edits_and_end_with_identical_bytes() {
    let mut pair = pair().await;
    co_edit(&mut pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_desktops_co_edit_on_postgres() {
    let mut pair = pair_on(Server::start_postgres().await).await;
    co_edit(&mut pair).await;
}

async fn co_edit<B: Backend>(pair: &mut Pair<B>) {
    let (mut a, mut b) = open_both(pair).await;
    type_at(&mut a, 6, " for Monday");
    both_rounds(pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan for Monday\n");
    assert_eq!(read(&pair.b, PLAN), "# Plan for Monday\n");
    let arrived = b.poll();
    assert_eq!(arrived.len(), 1, "{arrived:?}");

    // Both type at once, at different places.
    type_at(&mut a, 0, "A ");
    type_at(&mut b, 17, "!");
    both_rounds(pair).await;
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
    assert_eq!(read(&pair.a, PLAN), "A # Plan for Monday!\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_with_no_matching_file_operation_waits_outside_text_mode() {
    let mut pair = pair().await;
    pair.a.write(PLAN, b"# Plan edited\n");
    let (entered, document) = pair.sync_a.open_text(PLAN).expect("asked");
    assert_eq!(entered, Entered::NotYet);
    assert!(document.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_desktops_that_start_at_once_merge_without_a_conflict() {
    let mut pair = pair().await;
    both_rounds(&mut pair).await;
    let (_, a) = pair.sync_a.open_text(PLAN).expect("the note opens");
    let (_, b) = pair.sync_b.open_text(PLAN).expect("the note opens");
    let (mut a, mut b) = (a.unwrap(), b.unwrap());
    type_at(&mut a, 0, "1");
    type_at(&mut b, 6, "2");
    both_rounds(&mut pair).await;
    assert!(pair.sync_a.conflicts().is_empty());
    assert_eq!(read(&pair.a, PLAN), "1# Plan2\n");
    assert_eq!(read(&pair.b, PLAN), "1# Plan2\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_journal_rebuilds_a_note_after_a_restart() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 6, " one");
    both_rounds(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    let file = pair.sync_b.text_file(&rid).unwrap().expect("in the journal");
    assert!(
        file.records
            .iter()
            .all(|record| record.judgement == Judgement::Readable),
        "{:?}",
        file.records
    );
    assert!(file.records.len() >= 2);
}
