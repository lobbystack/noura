//! Text mode at seam 4 (sync-v2.md §22): two desktops co-edit a note over
//! the real server, and both end with identical bytes on disk.

mod sync_support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::sync::SyncService;
use app_core::sync::realtime::Channel;
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::workspace::presence::{CURSOR_LIFETIME, SelectionRange};
use app_core::sync::workspace::text::{Entered, IDLE, IDLE_ANY_WRITER, Judgement, TextReviewKind};
use app_core::text_bridge::TextBridge;
use collab_sandbox::{LineageStatus, SandboxDocument};
use editor_core::text_mode::{CollabDocument, Utf16Splice};
use sync_protocol::codec::{Bytes, Decode, Encode, ExactBytes, Text};
use sync_protocol::messages::{ErrorBody, Frame, KeyRecord, KeysResponse, PushFailureDetail};
use sync_protocol::types::Id;
use sync_store::Backend;
use sync_support::{Pair, Server, approve, pair, pair_on};

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
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("round");
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
    let file = pair
        .sync_b
        .text_file(&rid)
        .unwrap()
        .expect("in the journal");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_windows_get_a_document_that_holds_the_edit_that_asked_for_it() {
    let mut pair = pair().await;
    both_rounds(&mut pair).await;
    let bridge = TextBridge::default();
    bridge.open(PLAN);
    bridge.serve(&mut pair.sync_a);
    let document = bridge.take_document(PLAN).expect("the document");
    let rid = document.rid;
    let _editor = document.catch_up("# Plan typed\n");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), "# Plan typed\n");

    // Closing the note leaves it in text mode with its file in place.
    bridge.close(rid);
    bridge.serve(&mut pair.sync_a);
    assert!(pair.sync_a.in_text_mode(&rid));
    assert_eq!(read(&pair.a, PLAN), "# Plan typed\n");
}

/// Hands every frame that waits on `channel` to `sync`, until it goes quiet.
async fn drain(channel: &mut Channel, sync: &mut WorkspaceSync, service: &mut SyncService) {
    while let Ok(frame) = tokio::time::timeout(Duration::from_millis(300), channel.next()).await {
        sync.on_frame(service, &frame.expect("the channel stays open"))
            .await
            .expect("handled");
    }
}

/// Both desktops hold the note open, each on its realtime channel.
async fn cursors_ready(
    pair: &mut Pair<sync_store_sqlite::SqliteBackend>,
) -> (SandboxDocument, SandboxDocument, Channel, Channel) {
    let (a, b) = open_both(pair).await;
    let mut channel_a = Channel::open(pair.service.client(), pair.sync_a.workspace())
        .await
        .expect("the channel opens");
    let mut channel_b = Channel::open(pair.desktop.service.client(), pair.sync_b.workspace())
        .await
        .expect("the channel opens");
    pair.sync_a.presence_connected().unwrap();
    pair.sync_b.presence_connected().unwrap();
    for frame in pair.sync_a.presence_frames().unwrap() {
        channel_a.send(&frame).await.unwrap();
    }
    for frame in pair.sync_b.presence_frames().unwrap() {
        channel_b.send(&frame).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    (a, b, channel_a, channel_b)
}

async fn send_selection(
    pair: &mut Pair<sync_store_sqlite::SqliteBackend>,
    channel: &mut Channel,
    at: u32,
) {
    pair.sync_a.set_selections(
        PLAN,
        vec![SelectionRange {
            anchor: at,
            head: at,
        }],
        true,
    );
    for frame in pair.sync_a.presence_frames().unwrap() {
        channel.send(&frame).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collaborators_cursor_stays_on_its_character_and_goes_with_them() {
    let mut pair = pair().await;
    let (mut a, _b, mut channel_a, mut channel_b) = cursors_ready(&mut pair).await;
    send_selection(&mut pair, &mut channel_a, 6).await;
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    let cursors = pair.sync_b.cursors(PLAN, Instant::now());
    assert_eq!(cursors.len(), 1, "{cursors:?}");
    assert_eq!(cursors[0].device, pair.sync_a.device());
    assert_eq!(cursors[0].selections[0].head, 6);

    // Text arrives before the cursor, and the cursor moves with its
    // character.
    type_at(&mut a, 0, "New ");
    both_rounds(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    assert!(pair.sync_b.text_body(&rid).is_some());
    let cursors = pair.sync_b.cursors(PLAN, Instant::now());
    assert_eq!(cursors[0].selections[0].head, 10);

    // Thirty seconds after its last record it goes.
    let later = Instant::now() + CURSOR_LIFETIME + Duration::from_secs(1);
    assert!(pair.sync_b.cursors(PLAN, later).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cursor_goes_when_the_server_says_its_session_ended() {
    let mut pair = pair().await;
    let (_a, _b, mut channel_a, mut channel_b) = cursors_ready(&mut pair).await;
    send_selection(&mut pair, &mut channel_a, 2).await;
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    assert_eq!(pair.sync_b.cursors(PLAN, Instant::now()).len(), 1);
    drop(channel_a);
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    assert!(pair.sync_b.cursors(PLAN, Instant::now()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_rejects_an_old_presence_record() {
    let mut pair = pair().await;
    let (_a, _b, mut channel_a, mut channel_b) = cursors_ready(&mut pair).await;
    pair.sync_a
        .set_selections(PLAN, vec![SelectionRange { anchor: 1, head: 1 }], true);
    let frames = pair.sync_a.presence_frames().unwrap();
    let old = frames.last().cloned().expect("a record");
    let Frame::Presence { record } = old.clone() else {
        panic!("a presence record");
    };
    channel_a.send(&old).await.unwrap();
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    let now = Instant::now();
    // The server forwards the same record again.
    assert!(!pair.sync_b.take_presence(&record, now));

    // One from outside the two-minute window.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let mut stale = record.clone();
    stale.body.sequence += 1;
    stale.body.minute -= 3;
    assert!(
        !pair.sync_b.take_presence(&stale, now),
        "signature or clock"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_stores_no_presence() {
    let server = Server::start().await;
    let mut pair = pair_on(server).await;
    let (_a, _b, mut channel_a, mut channel_b) = cursors_ready(&mut pair).await;
    pair.sync_a
        .set_selections(PLAN, vec![SelectionRange { anchor: 1, head: 1 }], true);
    let frames = pair.sync_a.presence_frames().unwrap();
    let Some(Frame::Presence { record }) = frames.last().cloned() else {
        panic!("a presence record");
    };
    channel_a
        .send(&Frame::Presence {
            record: record.clone(),
        })
        .await
        .unwrap();
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    for path in pair.server.stored_files().await {
        let bytes = std::fs::read(path).unwrap_or_default();
        assert!(
            !bytes
                .windows(16)
                .any(|window| window == record.body.channel.0),
            "a stored file holds the channel"
        );
        assert!(
            !bytes
                .windows(32)
                .any(|window| window == &record.body.ciphertext.0[..32]),
            "a stored file holds the ciphertext"
        );
    }
}

fn varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// One item of `client` after the first document's first element.
fn item(client: u64, text: &str) -> Vec<u8> {
    let mut out = vec![1, 1];
    varint(client, &mut out);
    out.push(0);
    out.push(0x84);
    out.push(0);
    out.push(0);
    varint(text.len() as u64, &mut out);
    out.extend_from_slice(text.as_bytes());
    out.push(0);
    out
}

/// `count` items of `client`, one element each, each after the one before.
fn keystrokes(client: u64, count: u64) -> Vec<u8> {
    let mut out = vec![1];
    varint(count, &mut out);
    varint(client, &mut out);
    out.push(0);
    for index in 0..count {
        out.push(0x84);
        if index == 0 {
            out.extend_from_slice(&[0, 0]);
        } else {
            varint(client, &mut out);
            varint(index - 1, &mut out);
        }
        out.push(1);
        out.push(b'k');
    }
    out.push(0);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hostile_update_is_unreadable_and_leaves_the_note_to_the_others() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    pair.sync_b
        .sign_hostile_update(PLAN, |_| vec![1, 2, 3])
        .expect("signed");
    both_rounds(&mut pair).await;
    let reviews = pair.sync_a.text_reviews();
    assert_eq!(reviews.len(), 1, "{reviews:?}");
    assert_eq!(reviews[0].kind, TextReviewKind::Unreadable);
    assert_eq!(reviews[0].signer, Some(pair.sync_b.device()));
    type_at(&mut a, 6, "!");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan!\n");
    assert_eq!(read(&pair.b, PLAN), "# Plan!\n");
    assert!(pair.sync_a.in_text_mode(&rid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_who_floods_a_note_slows_only_their_own_edits() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    // One record overdraws the writer's allowance.
    pair.sync_b
        .sign_hostile_update(PLAN, |client| keystrokes(client, 600_000))
        .expect("signed");
    both_rounds(&mut pair).await;
    assert!(read(&pair.a, PLAN).contains("kkk"));
    // The next records wait.
    pair.sync_b
        .sign_hostile_update(PLAN, |client| item(client + 1, "later"))
        .ok();
    both_rounds(&mut pair).await;
    // The other editor keeps typing, and the note stays writable.
    type_at(&mut a, 0, "still here ");
    both_rounds(&mut pair).await;
    let text = read(&pair.a, PLAN);
    assert!(text.starts_with("still here "), "{}", &text[..40]);
    assert!(!text.contains("later"));
    assert_eq!(pair.sync_a.text_status(&rid), Some(LineageStatus::Merging));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_jumps_past_the_limits_stops_the_lineage_until_you_keep_the_last_good_state()
{
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    let huge = "x".repeat(9 << 20);
    pair.sync_b
        .sign_hostile_update(PLAN, |client| item(client, &huge))
        .expect("signed");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan\n", "the last good state stays");
    assert_eq!(pair.sync_a.stopped_notes(), [(rid, PLAN.to_owned())]);
    assert!(
        pair.sync_a
            .text_reviews()
            .iter()
            .any(|review| review.kind == TextReviewKind::Stopped)
    );

    pair.sync_a.keep_last_good(rid).expect("kept");
    both_rounds(&mut pair).await;
    assert!(!pair.sync_a.in_text_mode(&rid));
    assert!(!pair.sync_b.in_text_mode(&rid));
    assert_eq!(read(&pair.a, PLAN), "# Plan\n");
    assert_eq!(read(&pair.b, PLAN), "# Plan\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_note_travels_in_blobs() {
    let mut pair = pair().await;
    let long = "line of the note\n".repeat(20_000);
    pair.a.write(PLAN, long.as_bytes());
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), long);
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 0, "first ");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), format!("first {long}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn your_edit_past_4_mib_ends_text_mode() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    type_at(&mut a, 0, &"y".repeat((4 << 20) + 1));
    both_rounds(&mut pair).await;
    assert!(!pair.sync_a.in_text_mode(&rid));
    assert!(!pair.sync_b.in_text_mode(&rid));
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_edit_reaches_disk_before_the_round_that_applies_it_ends() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 6, " durable");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("round");
    let root = pair.b.engine.root().to_owned();
    let service = &mut pair.desktop.service;
    let sync = &mut pair.sync_b;
    let (result, synced) = local_core::durability_probe::record(|| {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(sync.round(service, 0))
        })
    });
    result.expect("round");
    assert_eq!(read(&pair.b, PLAN), "# Plan durable\n");
    assert!(synced.contains(&root), "{synced:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_mode_note_that_yields_its_path_moves_by_a_text_update() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    // Another object takes a colliding name: the digests decide who moves.
    pair.b.write("plan.md", b"# Other\n");
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
    assert_eq!(pair.a.files().len(), pair.b.files().len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_journal_file_is_refused_and_a_restart_rebuilds_the_note() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 6, " kept");
    both_rounds(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    let journal = pair
        .b
        .dir
        .path()
        .join(".noura/sync")
        .join(pair.sync_b.device().to_hex())
        .join(format!("text-{}.json", rid.to_hex()));
    let bytes = std::fs::read(&journal).unwrap();
    let mut changed = bytes.clone();
    let last = changed.len() - 2;
    changed[last] ^= 1;
    std::fs::write(&journal, changed).unwrap();
    assert!(pair.sync_b.text_file(&rid).is_err());
    std::fs::write(&journal, bytes).unwrap();
    assert!(pair.sync_b.text_file(&rid).unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_forwards_an_old_presence_record_moves_no_cursor() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    // The laptop's frames pass the proxy; the desktop sends the records.
    let mut laptop = Channel::open(pair.service.client(), pair.sync_a.workspace())
        .await
        .expect("the channel opens");
    let mut desktop = Channel::open(pair.desktop.service.client(), pair.sync_b.workspace())
        .await
        .expect("the channel opens");
    pair.sync_a.presence_connected().unwrap();
    pair.sync_b.presence_connected().unwrap();
    for frame in pair.sync_a.presence_frames().unwrap() {
        laptop.send(&frame).await.unwrap();
    }
    for frame in pair.sync_b.presence_frames().unwrap() {
        desktop.send(&frame).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The relay keeps the first record and sends it again after each later
    // one.
    let kept: Arc<Mutex<Option<Frame>>> = Arc::default();
    let held = kept.clone();
    pair.proxy.tamper_frames(Box::new(move |frame| {
        if !matches!(frame, Frame::Presence { .. }) {
            return vec![frame];
        }
        let mut kept = held.lock().unwrap();
        match kept.clone() {
            Some(old) => vec![frame, old],
            None => {
                *kept = Some(frame.clone());
                vec![frame]
            }
        }
    }));
    for at in [1, 4] {
        pair.sync_b.set_selections(
            PLAN,
            vec![SelectionRange {
                anchor: at,
                head: at,
            }],
            true,
        );
        for frame in pair.sync_b.presence_frames().unwrap() {
            desktop.send(&frame).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    drain(&mut laptop, &mut pair.sync_a, &mut pair.service).await;
    let cursors = pair.sync_a.cursors(PLAN, Instant::now());
    assert_eq!(cursors.len(), 1, "{cursors:?}");
    assert_eq!(cursors[0].selections[0].head, 4, "the old record lost");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_takes_the_place_of_a_patch_the_server_refused() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    type_at(&mut a, 6, " refused");
    pair.sync_a.capture().expect("a capture");
    // The server refuses the next push of the patch once.
    let detail = PushFailureDetail {
        record_index: 0,
        code: Text("snapshot_required".into()),
        record_detail: None,
    };
    let answered = Arc::new(AtomicBool::new(false));
    let once = answered.clone();
    let bytes = ErrorBody {
        code: Text("snapshot_required".into()),
        message: Text("too many patches".into()),
        retry_after_seconds: None,
        detail: Some(Bytes(detail.to_bytes())),
    }
    .to_bytes();
    pair.proxy.forge(Box::new(move |method, path| {
        (method == "POST" && path.ends_with("/ops") && !once.swap(true, Ordering::Relaxed))
            .then(|| (409, bytes.clone()))
    }));
    both_rounds(&mut pair).await;
    assert!(answered.load(Ordering::Relaxed));
    assert_eq!(read(&pair.b, PLAN), "# Plan refused\n");
    let file = pair.sync_b.text_file(&rid).unwrap().unwrap();
    assert!(file.records.iter().any(|record| record.snapshot));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_writes_again_a_devices_records_under_a_skipped_epoch() {
    let mut pair = pair().await;
    let (_a, mut b) = open_both(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    // The desktop rotates to epoch 2 and writes under it.
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    both_rounds(&mut pair).await;
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    pair.desktop
        .service
        .record_heads(pair.sync_b.remote_mut().unwrap(), &[])
        .await
        .expect("the desktop rotates");
    type_at(&mut b, 6, " under two");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop writes under epoch 2");
    // The laptop's envelope of epoch 2 fails, so it replaces the epoch.
    damage_envelopes(&pair.proxy, pair.laptop.id(), pair.desktop.id());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.proxy.honest();
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 3);
    assert_eq!(
        read(&pair.a, PLAN),
        "# Plan\n",
        "epoch 2 is unreadable here"
    );

    // The desktop accepts the replacement and writes a snapshot that
    // covers its record, under the same lineage.
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan under two\n");
    let lineage = pair.sync_b.text_file(&rid).unwrap().unwrap().lineage;
    let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
    assert_eq!(file.lineage, lineage);
    assert!(pair.sync_a.conflicts().is_empty());
}

/// Damages, on their way to `reader`, the envelopes that `wrapper` signed.
fn damage_envelopes(proxy: &sync_support::Proxy, reader: Id, wrapper: Id) {
    proxy.tamper(Box::new(move |path, body| {
        if !(path.ends_with("/keys/current") || path.ends_with("/keys")) {
            return body;
        }
        let mut keys = KeysResponse::from_bytes(&body).unwrap();
        for entry in keys.entries.0.iter_mut() {
            if let KeyRecord::Envelope(envelope) = &mut entry.record
                && envelope.body.recipient == reader
                && envelope.body.wrapping_device == wrapper
            {
                let mut bytes = envelope.body.ciphertext.as_slice().to_vec();
                bytes[0] ^= 1;
                envelope.body.ciphertext = ExactBytes::new(bytes).unwrap();
            }
        }
        keys.to_bytes()
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_devices_that_start_from_different_states_conflict_as_two_branches() {
    let mut pair = pair().await;
    both_rounds(&mut pair).await;
    // The laptop enters text mode while the desktop writes the file.
    let (entered, document) = pair.sync_a.open_text(PLAN).expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    let mut document = document.unwrap();
    type_at(&mut document, 6, " laptop");
    pair.b.write(PLAN, b"# Plan desktop\n");
    pair.sync_b.capture().expect("a capture");
    both_rounds(&mut pair).await;
    let conflicts = pair.sync_a.conflicts();
    assert_eq!(conflicts.len(), 1, "two branches");
    assert_eq!(conflicts[0].branches.len(), 2);

    // Choosing the text-mode branch keeps the merged text and ends the
    // lineage on both desktops.
    let rid = conflicts[0].rid;
    let own = pair.sync_a.device();
    let branch = conflicts[0]
        .branches
        .iter()
        .find(|branch| branch.device == own)
        .unwrap();
    pair.sync_a.choose(rid, branch.key).expect("chosen");
    both_rounds(&mut pair).await;
    assert!(pair.sync_a.conflicts().is_empty());
    assert_eq!(read(&pair.a, PLAN), "# Plan laptop\n");
    assert_eq!(read(&pair.b, PLAN), "# Plan laptop\n");
    assert!(!pair.sync_b.in_text_mode(&rid));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_client_keeps_a_notes_journal_and_rebuilds_from_it() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 6, " kept");
    both_rounds(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    let before = pair.sync_b.text_file(&rid).unwrap().unwrap();
    drop(_b);
    let replica = pair.sync_b.workspace();
    assert_eq!(replica, pair.sync_a.workspace());
    drop(pair.sync_b);
    let reopened = WorkspaceSync::open(
        pair.b.engine.clone(),
        pair.desktop.credentials.clone(),
        pair.desktop.secrets(),
        &pair.b.registry,
    )
    .expect("the client reopens");
    pair.sync_b = reopened;
    let after = pair.sync_b.text_file(&rid).unwrap().unwrap();
    assert_eq!(before.records.len(), after.records.len());
    assert!(pair.sync_b.in_text_mode(&rid));
    both_rounds(&mut pair).await;
    let (entered, document) = pair.sync_b.open_text(PLAN).expect("opens");
    assert_eq!(entered, Entered::Yes);
    assert!(document.is_some());
    assert_eq!(pair.sync_b.text_body(&rid), Some("# Plan kept\n"));
}
