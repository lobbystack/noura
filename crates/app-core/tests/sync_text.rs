//! Text mode at seam 4 (sync-v2.md §22): two desktops co-edit a note over
//! the real server, and both end with identical bytes on disk.

mod sync_support;

use std::time::{Duration, Instant};

use app_core::sync::workspace::text::{Entered, IDLE, IDLE_ANY_WRITER, Judgement};
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

const MANAGED: &str =
    "---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\ntags: [a]\n---\nBody\n";

/// Pushes the managed note to both desktops and opens it on both.
async fn managed_note<B: Backend>(pair: &mut Pair<B>) -> (SandboxDocument, SandboxDocument) {
    pair.a.write(PLAN, MANAGED.as_bytes());
    both_rounds(pair).await;
    open_both(pair).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renames_and_property_edits_merge_per_key() {
    let mut pair = pair().await;
    let (_a, _b) = managed_note(&mut pair).await;

    // One desktop adds a property outside Noura, the other renames the note.
    let edited = MANAGED.replace("tags: [a]\n", "tags: [a]\nstage: draft\n");
    pair.a.write(PLAN, edited.as_bytes());
    let path = pair.b.dir.path();
    std::fs::rename(path.join(PLAN), path.join("Renamed.md")).unwrap();
    both_rounds(&mut pair).await;

    for folder in [&pair.a, &pair.b] {
        let files = folder.files();
        let text = String::from_utf8(files["Renamed.md"].clone()).unwrap();
        assert!(text.contains("stage: draft"), "{text}");
        assert!(!files.contains_key(PLAN));
    }
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outside_edit_becomes_an_update_computed_from_the_difference() {
    let mut pair = pair().await;
    let (_a, mut b) = open_both(&mut pair).await;
    pair.a.write(PLAN, "# Plan 😀😁\n".as_bytes());
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), "# Plan 😀😁\n");
    assert!(!b.poll().is_empty(), "the editor hears of the outside edit");
    // Between two characters that share a high surrogate.
    pair.a.write(PLAN, "# Plan 😀x😁\n".as_bytes());
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), "# Plan 😀x😁\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_that_only_reformats_the_frontmatter_sends_nothing_and_keeps_your_bytes() {
    let mut pair = pair().await;
    let (_a, _b) = managed_note(&mut pair).await;
    let before = read(&pair.b, PLAN);
    let pending = pair.sync_a.outbox().len();
    let reformatted = MANAGED.replace("tags: [a]", "tags:   [ a ]");
    pair.a.write(PLAN, reformatted.as_bytes());
    pair.sync_a.capture().expect("a capture");
    assert_eq!(pair.sync_a.outbox().len(), pending);
    assert_eq!(read(&pair.a, PLAN), reformatted);
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_that_breaks_the_frontmatter_ends_text_mode_with_a_file_operation() {
    let mut pair = pair().await;
    let (_a, _b) = managed_note(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    let broken = MANAGED.replace("tags: [a]", "tags: [a");
    pair.a.write(PLAN, broken.as_bytes());
    both_rounds(&mut pair).await;
    assert!(!pair.sync_a.in_text_mode(&rid));
    assert!(!pair.sync_b.in_text_mode(&rid));
    assert_eq!(read(&pair.b, PLAN), broken);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_that_turns_binary_ends_text_mode() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    pair.a.write(PLAN, &[0xff, 0xfe, 0x00]);
    both_rounds(&mut pair).await;
    assert!(!pair.sync_b.in_text_mode(&rid));
    assert_eq!(pair.b.files()[PLAN], [0xff, 0xfe, 0x00]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_keeps_its_line_endings_and_byte_order_mark_through_remote_edits() {
    let mut pair = pair().await;
    let note = "\u{feff}One\r\nTwo";
    pair.a.write(PLAN, note.as_bytes());
    both_rounds(&mut pair).await;
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 4, "1");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), "\u{feff}One1\r\nTwo");
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_left_idle_ends_with_one_snapshot_and_few_updates() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    for index in 0..12 {
        type_at(&mut a, 6 + index, "x");
        both_rounds(&mut pair).await;
    }
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    pair.sync_a
        .text_tick(Instant::now() + IDLE + Duration::from_secs(1))
        .expect("a tick");
    both_rounds(&mut pair).await;
    for file in [
        pair.sync_a.text_file(&rid).unwrap().unwrap(),
        pair.sync_b.text_file(&rid).unwrap().unwrap(),
    ] {
        let snapshots = file.records.iter().filter(|r| r.snapshot).count();
        let updates = file.records.len() - snapshots;
        assert_eq!(snapshots, 1, "{:?}", file.records);
        assert!(updates <= 8, "{updates}");
    }
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_the_last_editor_goes_away_another_writer_snapshots_after_five_minutes() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    for index in 0..12 {
        type_at(&mut a, 6 + index, "x");
        both_rounds(&mut pair).await;
    }
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    // The writer that didn't type waits out the five minutes.
    pair.sync_b
        .text_tick(Instant::now() + IDLE + Duration::from_secs(1))
        .expect("a tick");
    let live = pair.sync_b.text_file(&rid).unwrap().unwrap().records.len();
    assert!(live > 8, "{live}");
    pair.sync_b
        .text_tick(Instant::now() + IDLE_ANY_WRITER + Duration::from_secs(1))
        .expect("a tick");
    both_rounds(&mut pair).await;
    let file = pair.sync_b.text_file(&rid).unwrap().unwrap();
    assert!(file.records.len() <= 9, "{}", file.records.len());
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edits_made_offline_merge_on_reconnect() {
    let mut pair = pair().await;
    let (mut a, mut b) = open_both(&mut pair).await;
    // Neither pushes, then both reconnect.
    type_at(&mut a, 0, "offline A ");
    type_at(&mut b, 6, " offline B");
    pair.sync_a.capture().expect("a capture");
    pair.sync_b.capture().expect("a capture");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "offline A # Plan offline B\n");
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_device_rebuilds_the_document_from_its_journal() {
    let mut pair = pair().await;
    let (mut a, b) = open_both(&mut pair).await;
    type_at(&mut a, 6, " one");
    both_rounds(&mut pair).await;
    pair.sync_b.close_text(PLAN).expect("closes");
    drop(b);
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    let (entered, document) = pair.sync_b.open_text(PLAN).expect("opens again");
    assert_eq!(entered, Entered::Yes);
    assert!(document.is_some());
    assert_eq!(pair.sync_b.text_body(&rid), Some("# Plan one\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_text_mode_note_ends_it_on_both_desktops() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    std::fs::remove_file(pair.a.dir.path().join(PLAN)).unwrap();
    both_rounds(&mut pair).await;
    assert!(!pair.b.files().contains_key(PLAN));
    assert!(!pair.sync_b.in_text_mode(&rid));
}
