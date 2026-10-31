//! Text mode at seam 4 (sync-v2.md §22): two desktops co-edit a note over
//! the real server, and both end with identical bytes on disk.

mod sync_scale_support;
mod sync_support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use sync_protocol::codec::{Bytes, Decode, Encode, ExactBytes, Sorted, Text};
use sync_protocol::messages::{
    ErrorBody, Frame, KeyRecord, KeysResponse, PullResponse, PushFailureDetail,
};
use sync_protocol::types::{Id, IdOrZero};
use sync_store::Backend;
use sync_support::{Pair, Server, approve, pair, pair_of_accounts_on, pair_on};

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

/// A device comes back to a note another device took over (§22): it pulls
/// before it decides, so it repeats no snapshot. The soak found this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_back_from_away_repeats_no_snapshot() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    for _ in 0..12 {
        type_at(&mut a, 0, "x");
        pair.sync_a.text_tick(Instant::now()).expect("a tick");
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round");
    }
    drop(a);
    both_rounds(&mut pair).await;
    let rid = pair.sync_b.text_rid(PLAN).expect("the note");

    // The laptop goes away with its rule still to run, and the desktop
    // takes the note over.
    pair.sync_a.set_pulled_within(Duration::from_millis(1));
    pair.sync_b
        .text_tick(Instant::now() + IDLE_ANY_WRITER + Duration::from_secs(1))
        .expect("the idle rule");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the snapshot goes out");
    tokio::time::sleep(Duration::from_millis(20)).await;

    pair.sync_a
        .text_tick(Instant::now() + IDLE_ANY_WRITER + Duration::from_secs(1))
        .expect("nothing to do yet");
    pair.sync_a.set_pulled_within(Duration::from_secs(60));
    both_rounds(&mut pair).await;

    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());
    let head = sync_scale_support::domain_row(&pair.server.store, workspace, domain)
        .await
        .head_seq;
    let live = pair
        .server
        .store
        .live_at(workspace, rid, head)
        .await
        .expect("the live set");
    let states = live
        .iter()
        .filter(|(_, class)| *class == sync_protocol::records::OpClass::State)
        .count();
    assert_eq!(states, 1, "{live:?}");
}

/// A device partway through a long catch-up can't tell whether someone took
/// the note over, so it waits for the log's head (§22). The soak found this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_still_catching_up_writes_no_snapshot() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    for _ in 0..12 {
        type_at(&mut a, 0, "x");
        pair.sync_a.text_tick(Instant::now()).expect("a tick");
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round");
    }
    drop(a);
    pair.sync_a.close_text(PLAN).expect("closed");
    pair.b.write("One.md", b"# One\n");
    pair.b.write("Two.md", b"# Two\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");

    // Each pull brings one entry of the two.
    pair.proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            let entries = page.entries.to_vec();
            if entries.len() > 1 {
                page.complete_through = entries[0].seq;
                page.entries = Sorted::new(vec![entries[0].clone()]).expect("one entry");
            }
        }
        response.to_bytes()
    }));
    pair.sync_a
        .pull(&mut pair.service, 0)
        .await
        .expect("a partial pull");
    let later = Instant::now() + IDLE_ANY_WRITER + Duration::from_secs(1);
    assert_eq!(pair.sync_a.text_tick(later).expect("a tick"), 0);

    pair.proxy.honest();
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.text_tick(later).expect("a tick"), 1);
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
    let _editor = document.catch_up("# Plan\n", "# Plan typed\n");
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
async fn cursors_ready<B: Backend>(
    pair: &mut Pair<B>,
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
    tokio::time::sleep(Duration::from_millis(250)).await;
    (a, b, channel_a, channel_b)
}

async fn send_selection<B: Backend>(pair: &mut Pair<B>, channel: &mut Channel, at: u32) {
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
    no_presence_stored(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_stores_no_presence() {
    no_presence_stored(Server::start_postgres().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktops_on_two_postgres_instances_co_edit_and_see_cursors() {
    let server = Server::start_postgres().await;
    let second = server.second_instance().await;
    let mut pair = pair_on(server).await;
    pair.desktop.service = pair.desktop.connect_through(&second).await;
    let (mut a, mut b, mut channel_a, mut channel_b) = cursors_ready(&mut pair).await;

    // Each hears of the other's records on its own instance, with no pull.
    type_at(&mut a, 6, " laptop");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    assert_eq!(read(&pair.b, PLAN), "# Plan laptop\n");
    drain(&mut channel_a, &mut pair.sync_a, &mut pair.service).await;
    assert_eq!(b.poll().len(), 1);
    type_at(&mut b, 13, " desktop");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    drain(&mut channel_a, &mut pair.sync_a, &mut pair.service).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan laptop desktop\n");

    send_selection(&mut pair, &mut channel_a, 3).await;
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    let cursors = pair.sync_b.cursors(PLAN, Instant::now());
    assert_eq!(cursors.len(), 1, "the record crossed instances");
    assert_eq!(cursors[0].selections[0].head, 3);
}

async fn no_presence_stored<B: Backend>(server: Server<B>) {
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
    // The digests decide which object yields, so the test retries until
    // the note is the one.
    for _ in 0..12 {
        let mut pair = pair().await;
        let (_a, _b) = open_both(&mut pair).await;
        let rid = pair.sync_a.rid_at(PLAN).unwrap();
        // The note takes a name another object takes at once.
        let root = pair.a.dir.path();
        std::fs::rename(root.join(PLAN), root.join("Shared.md")).unwrap();
        pair.b.write("Shared.md", b"# Other\n");
        both_rounds(&mut pair).await;
        both_rounds(&mut pair).await;
        assert_eq!(pair.a.files(), pair.b.files());
        if pair.sync_a.local_path(&rid).as_deref() == Some("Shared.md") {
            continue;
        }
        let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
        let carried = file
            .records
            .iter()
            .any(|r| !r.snapshot && r.meta.iter().any(|entry| entry.key == "path"));
        assert!(carried, "{:?}", file.records);
        assert!(pair.sync_a.in_text_mode(&rid));
        return;
    }
    panic!("the note never yielded");
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

/// A patch and the snapshot that replaces it can both meet a full workspace.
/// The change waits on this device and reaches the other once space frees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_the_server_has_no_space_for_waits_and_goes_out_as_a_snapshot() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    type_at(&mut a, 6, " waiting");
    pair.sync_a.capture().expect("a capture");
    let detail = PushFailureDetail {
        record_index: 0,
        code: Text("quota_exceeded".into()),
        record_detail: None,
    };
    let bytes = ErrorBody {
        code: Text("quota_exceeded".into()),
        message: Text("the workspace is full".into()),
        retry_after_seconds: None,
        detail: Some(Bytes(detail.to_bytes())),
    }
    .to_bytes();
    let refused = Arc::new(AtomicUsize::new(0));
    let count = refused.clone();
    pair.proxy.forge(Box::new(move |method, path| {
        (method == "POST" && path.ends_with("/ops") && count.fetch_add(1, Ordering::Relaxed) < 2)
            .then(|| (507, bytes.clone()))
    }));
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused record doesn't fail the round");
    assert!(refused.load(Ordering::Relaxed) >= 2);
    assert_eq!(pair.sync_a.status().waiting_for_space, 1);

    // Later edits wait behind the same change, and nothing is lost on the
    // way: the desktop never has to wait on a missing record.
    type_at(&mut a, 14, " still");
    for _ in 0..3 {
        both_rounds(&mut pair).await;
    }
    assert_eq!(pair.sync_a.status().waiting_for_space, 0);
    assert_eq!(read(&pair.b, PLAN), "# Plan waiting still\n");
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    let file = pair.sync_b.text_file(&rid).unwrap().unwrap();
    assert!(!file.stopped);
    assert!(file.pending_since.is_none(), "no update waits on a record");
    let own = pair.sync_a.text_file(&rid).unwrap().unwrap();
    assert!(
        own.records.iter().all(|r| r.seq.is_some()),
        "no refused record stays in the journal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_writes_again_a_devices_records_under_a_skipped_epoch() {
    // A device replaces only an epoch another account rotated (§9).
    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let (_a, mut b) = open_both(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    // The desktop rotates to epoch 2 and writes under it.
    let third = approve(
        &pair.server,
        &mut pair.desktop,
        std::slice::from_mut(pair.sync_b.remote_mut().unwrap()),
    )
    .await;
    both_rounds(&mut pair).await;
    pair.desktop
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
    // The fixture's rotator case: the rotator's record of the skipped epoch
    // is unreadable, and its signer writes it again.
    let keys: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/workspace-format/fixtures/sync-v2-keys.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let case = keys["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "record_of_the_skipped_epochs_rotator")
        .unwrap();
    assert_eq!(case["expect"], "unreadable");
    assert!(
        pair.sync_a
            .unreadable()
            .iter()
            .any(|(_, device, _)| *device == pair.sync_b.device()),
        "the record under the skipped epoch waits for its rewrite"
    );

    // The desktop accepts the replacement. An update that depends on the
    // unreadable record goes out first and stays pending, and a snapshot
    // that covers the record supplies it, under the same lineage.
    type_at(&mut b, 16, " more");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan under two more\n");
    assert!(
        !pair
            .sync_a
            .text_reviews()
            .iter()
            .any(|review| review.kind == TextReviewKind::Pending)
    );
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

/// One item of `client` at `clock`, after the first document's first
/// element, which holds until the clocks before it arrive.
fn item_at(client: u64, clock: u64, text: &str) -> Vec<u8> {
    let mut out = vec![1, 1];
    varint(client, &mut out);
    varint(clock, &mut out);
    out.push(0x84);
    out.push(0);
    out.push(0);
    varint(text.len() as u64, &mut out);
    out.extend_from_slice(text.as_bytes());
    out.push(0);
    out
}

/// A third desktop's copy of the workspace, downloaded now.
async fn third_desktop<B: Backend>(
    pair: &mut Pair<B>,
) -> (sync_support::Desktop, sync_support::Folder, WorkspaceSync) {
    let mut desktop = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    let mut folder = sync_support::Folder::new("Third");
    let mut sync = WorkspaceSync::download(
        &mut desktop.service,
        folder.engine.clone(),
        pair.sync_a.workspace(),
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    for _ in 0..3 {
        sync.round(&mut desktop.service, 0).await.expect("a round");
    }
    (desktop, folder, sync)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_that_disagrees_keeps_the_note_through_a_close_and_a_restart() {
    let mut pair = pair().await;
    let (mut a, b) = open_both(&mut pair).await;
    drop(b);
    pair.sync_b.close_text(PLAN).expect("closed");
    type_at(&mut a, 6, " kept");
    both_rounds(&mut pair).await;
    drop(a);
    pair.sync_a.close_text(PLAN).expect("closed");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan kept\n");

    // The desktop covers the update with a snapshot that lacks its text.
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    pair.sync_b
        .sign_hostile_snapshot(PLAN, "# Plan\n")
        .expect("signed");
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.take_text_reports(), ["sync_text_conflict"]);
    let review = pair
        .sync_a
        .text_reviews()
        .into_iter()
        .find(|review| review.kind == TextReviewKind::Disagrees)
        .expect("a review entry");
    assert_eq!(review.signer, Some(pair.sync_b.device()));
    let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
    assert!(
        file.records
            .iter()
            .any(|r| r.snapshot && r.judgement == Judgement::Disagrees)
    );
    // The canonical rebuild starts from the live snapshot.
    let rebuilt = read(&pair.a, PLAN);
    assert_eq!(rebuilt, "# Plan\n");

    // Nobody edits the note, so nothing keeps its document open: the next
    // load comes from the journal.
    assert_eq!(pair.sync_a.text_body(&rid), None);
    both_rounds(&mut pair).await;
    let (_, document) = pair.sync_a.open_text(PLAN).expect("opens");
    assert!(document.is_some());
    assert_eq!(pair.sync_a.text_body(&rid), Some(rebuilt.as_str()));
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), rebuilt, "the body survived the close");

    drop(document);
    pair.sync_a.close_text(PLAN).expect("closed");
    drop(pair.sync_a);
    let reopened = WorkspaceSync::open(
        pair.a.engine.clone(),
        pair.laptop.credentials.clone(),
        pair.laptop.secrets(),
        &pair.a.registry,
    )
    .expect("the client reopens");
    pair.sync_a = reopened;
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), rebuilt, "and the restart");
    assert!(pair.sync_a.in_text_mode(&rid));
    assert!(
        pair.sync_a
            .text_reviews()
            .iter()
            .any(|review| review.kind == TextReviewKind::Disagrees)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebase_starts_a_new_lineage_and_late_updates_go_to_review() {
    let mut pair = pair().await;
    let (mut a, mut b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    let first = pair.sync_a.text_file(&rid).unwrap().unwrap().lineage;
    pair.sync_a.set_rebase_at(1);
    pair.sync_b.set_rebase_at(1);
    let a_is_lower = pair.sync_a.device() < pair.sync_b.device();
    let edit = |pair: &mut Pair<_>, a: &mut SandboxDocument, b: &mut SandboxDocument| {
        type_at(a, 0, "a");
        type_at(b, 0, "b");
        pair.sync_a.send_text_for_testing(rid).unwrap();
        pair.sync_b.send_text_for_testing(rid).unwrap();
    };

    // Both hold live patches, and a device the volume rule doesn't name
    // keeps the lineage.
    edit(&mut pair, &mut a, &mut b);
    both_rounds(&mut pair).await;
    if a_is_lower {
        pair.sync_b.write_snapshot_for_testing(rid).unwrap();
        assert_eq!(pair.sync_b.text_file(&rid).unwrap().unwrap().lineage, first);
    } else {
        pair.sync_a.write_snapshot_for_testing(rid).unwrap();
        assert_eq!(pair.sync_a.text_file(&rid).unwrap().unwrap().lineage, first);
    }
    both_rounds(&mut pair).await;
    edit(&mut pair, &mut a, &mut b);
    both_rounds(&mut pair).await;

    // The lowest device rebases, and the other device writes an update of
    // the old lineage before it hears of it.
    let writer = if a_is_lower {
        pair.sync_a.write_snapshot_for_testing(rid).unwrap();
        type_at(&mut b, 0, "late ");
        pair.sync_b.send_text_for_testing(rid).unwrap();
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round");
        pair.sync_b.device()
    } else {
        pair.sync_b.write_snapshot_for_testing(rid).unwrap();
        type_at(&mut a, 0, "late ");
        pair.sync_a.send_text_for_testing(rid).unwrap();
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
        pair.sync_a.device()
    };
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;

    let rebased = pair.sync_a.text_file(&rid).unwrap().unwrap().lineage;
    assert_ne!(rebased, first);
    for sync in [&pair.sync_a, &pair.sync_b] {
        let file = sync.text_file(&rid).unwrap().unwrap();
        assert_eq!(file.lineage, rebased);
        let reviews: Vec<_> = file
            .reviews
            .iter()
            .filter(|r| r.kind == TextReviewKind::EndedLineage)
            .collect();
        assert_eq!(reviews.len(), 1, "only the late update: {reviews:?}");
        assert_eq!(reviews[0].signer, Some(writer));
        assert!(
            !file
                .records
                .iter()
                .any(|r| r.judgement == Judgement::Waiting)
        );
    }
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
    assert!(pair.sync_a.conflicts().is_empty());
    assert!(!read(&pair.a, PLAN).contains("late"));
    // Whoever held the old document hears that it moved on.
    let moved = |notes: Vec<app_core::sync::workspace::text::LeftNote>| {
        notes
            .iter()
            .any(|note| note.why == app_core::sync::workspace::text::LeftWhy::Moved)
    };
    assert!(moved(pair.sync_a.take_left_notes()));
    assert!(moved(pair.sync_b.take_left_notes()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lowest_device_snapshots_past_the_volume_rule() {
    let mut pair = pair().await;
    let (mut a, mut b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    for index in 0..257 {
        type_at(&mut a, 6 + index, "a");
        pair.sync_a.send_text_for_testing(rid).unwrap();
        type_at(&mut b, index, "b");
        pair.sync_b.send_text_for_testing(rid).unwrap();
    }
    let lower = pair.sync_a.device() < pair.sync_b.device();
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    let snapshots_by = |file: &app_core::sync::workspace::text::TextFile, device| {
        file.records
            .iter()
            .filter(|r| r.snapshot && r.key.device == device)
            .count()
    };
    let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
    let designated = if lower {
        pair.sync_a.device()
    } else {
        pair.sync_b.device()
    };
    assert_eq!(
        snapshots_by(&file, designated),
        1,
        "the lowest device wrote one"
    );
    let patches = file.records.iter().filter(|r| !r.snapshot).count();
    assert!(patches < 512, "{patches}");
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_note_right_after_typing_writes_its_snapshot() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    for index in 0..12 {
        type_at(&mut a, 6 + index, "x");
        both_rounds(&mut pair).await;
    }
    type_at(&mut a, 18, "y");
    let before = pair.sync_a.text_file(&rid).unwrap().unwrap();
    let snapshots = before.records.iter().filter(|r| r.snapshot).count();
    drop(a);
    pair.sync_a.close_text(PLAN).expect("closed");
    let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
    assert_eq!(
        file.records.iter().filter(|r| r.snapshot).count(),
        snapshots + 1,
        "the close wrote it"
    );
    both_rounds(&mut pair).await;
    for sync in [&pair.sync_a, &pair.sync_b] {
        let file = sync.text_file(&rid).unwrap().unwrap();
        let updates = file.records.iter().filter(|r| !r.snapshot).count();
        assert!(updates <= 8, "{updates}");
    }
    assert_eq!(read(&pair.a, PLAN), "# Planxxxxxxxxxxxxy\n");
    assert_eq!(read(&pair.b, PLAN), "# Planxxxxxxxxxxxxy\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_catches_up_with_an_editor_that_fell_behind_it() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let bridge = TextBridge::default();
    drop(_b);
    pair.sync_b.close_text(PLAN).expect("closed");
    // The desktop's window opens the note, and a remote update merges
    // between the first keystroke and the document's arrival.
    bridge.open(PLAN);
    bridge.serve(&mut pair.sync_b);
    let document = bridge.take_document(PLAN).expect("the document");
    type_at(&mut a, 2, "Monday ");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.b, PLAN), "# Monday Plan\n");

    let mut editor = document.catch_up("# Plan\n", "# Plan!\n");
    pair.sync_b.text_tick(Instant::now()).expect("a tick");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Monday Plan!\n", "both kept");
    assert_eq!(read(&pair.b, PLAN), "# Monday Plan!\n", "both kept");
    // The editor takes the collaborator's text on its first poll.
    let splices = editor.poll();
    assert_eq!(splices.len(), 1, "{splices:?}");
    assert_eq!(splices[0].inserted, "Monday ");
    assert_eq!(splices[0].start, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_refused_before_its_file_operation_is_live_enters_on_a_later_edit() {
    let mut pair = pair().await;
    let bridge = TextBridge::default();
    pair.a.write(PLAN, b"# Plan edited\n");
    bridge.open(PLAN);
    bridge.serve(&mut pair.sync_a);
    assert!(bridge.take_document(PLAN).is_none());
    bridge.open(PLAN);
    assert!(!bridge.has_work(), "too soon to ask again");

    // The save's file operation goes out, and the next edit asks again.
    both_rounds(&mut pair).await;
    bridge.ask_again(PLAN);
    bridge.open(PLAN);
    bridge.serve(&mut pair.sync_a);
    let document = bridge.take_document(PLAN).expect("the note entered");
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    assert!(pair.sync_a.in_text_mode(&rid));
    assert_eq!(document.rid, rid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outside_edit_and_a_remote_update_in_one_round_both_stay() {
    let mut pair = pair().await;
    let (mut a, b) = open_both(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    drop(b);
    pair.sync_b.close_text(PLAN).expect("closed");
    both_rounds(&mut pair).await;
    let mut channel = Channel::open(pair.desktop.service.client(), pair.sync_b.workspace())
        .await
        .expect("the channel");
    drain(&mut channel, &mut pair.sync_b, &mut pair.desktop.service).await;

    // An outside edit sits in the file, not captured, when a collaborator's
    // update arrives on the realtime channel.
    pair.b.write(PLAN, b"# Plan outside\n");
    type_at(&mut a, 0, "Remote ");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    for _ in 0..50 {
        drain(&mut channel, &mut pair.sync_b, &mut pair.desktop.service).await;
        let file = pair.sync_b.text_file(&rid).unwrap().unwrap();
        if file.records.iter().filter(|r| !r.snapshot).count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(read(&pair.b, PLAN), "# Plan outside\n", "the file waits");

    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "Remote # Plan outside\n");
    assert_eq!(read(&pair.b, PLAN), "Remote # Plan outside\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_property_a_collaborator_changed_survives_an_outside_edit_of_the_body() {
    let mut pair = pair().await;
    let (a, b) = managed_note(&mut pair).await;
    drop((a, b));
    pair.sync_a.close_text(PLAN).expect("closed");
    pair.sync_b.close_text(PLAN).expect("closed");
    both_rounds(&mut pair).await;
    let original = read(&pair.a, PLAN);
    let body_at = original.rfind("---\n").unwrap() + 4;
    let mut channel = Channel::open(pair.desktop.service.client(), pair.sync_b.workspace())
        .await
        .expect("the channel");
    drain(&mut channel, &mut pair.sync_b, &mut pair.desktop.service).await;

    let mut edited = original.clone();
    edited.push_str("outside\n");
    pair.b.write(PLAN, edited.as_bytes());
    let renamed = original.replace("- a\n", "- b\n");
    assert_ne!(renamed, original);
    pair.a.write(PLAN, renamed.as_bytes());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    for _ in 0..50 {
        drain(&mut channel, &mut pair.sync_b, &mut pair.desktop.service).await;
        let file = pair.sync_b.text_file(&rid).unwrap().unwrap();
        if file.records.iter().any(|r| !r.snapshot) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    both_rounds(&mut pair).await;
    let text = read(&pair.a, PLAN);
    assert_eq!(text, read(&pair.b, PLAN));
    assert!(text.contains("- b\n"), "{text}");
    assert!(text[body_at..].contains("outside"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_hears_when_its_documents_lineage_stops() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    let huge = "x".repeat(9 << 20);
    pair.sync_b
        .sign_hostile_update(PLAN, |client| item(client, &huge))
        .expect("signed");
    both_rounds(&mut pair).await;
    let left = pair.sync_a.take_left_notes();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].rid, rid);
    assert_eq!(
        left[0].why,
        app_core::sync::workspace::text::LeftWhy::Stopped
    );
    assert_eq!(left[0].body, "# Plan\n");
    let (entered, document) = pair.sync_a.open_text(PLAN).expect("asked");
    assert_eq!(entered, Entered::Never, "the note waits for your choice");
    assert!(document.is_none());

    pair.sync_a.keep_last_good(rid).expect("kept");
    let left = pair.sync_a.take_left_notes();
    assert_eq!(left[0].why, app_core::sync::workspace::text::LeftWhy::Ended);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_hears_when_a_file_operation_ends_its_document() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    pair.a.write(PLAN, &[0xff, 0xfe, 0x00]);
    both_rounds(&mut pair).await;
    for notes in [pair.sync_a.take_left_notes(), pair.sync_b.take_left_notes()] {
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(
            notes[0].why,
            app_core::sync::workspace::text::LeftWhy::Ended
        );
        assert_eq!(notes[0].body, "# Plan\n");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updates_that_wait_on_a_missing_record_go_to_review_and_a_snapshot_supplies_them() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    // Clock 4 of a load whose earlier clocks nobody wrote.
    pair.sync_b
        .sign_hostile_update(PLAN, |client| item_at(client, 4, "gap"))
        .expect("signed");
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.text_status(&rid), Some(LineageStatus::Pending));
    assert!(
        pair.sync_a
            .text_reviews()
            .iter()
            .any(|review| review.kind == TextReviewKind::Pending)
    );
    assert_eq!(read(&pair.a, PLAN), "# Plan\n");
    // The note stays writable.
    let mut a = _a;
    type_at(&mut a, 6, "!");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), "# Plan!\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_who_floods_a_note_every_tick_slows_only_their_own_edits() {
    let mut pair = pair().await;
    let (mut a, _b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    let mut typed = String::new();
    for round in 0..5u32 {
        pair.sync_b
            .sign_hostile_update_at(PLAN, 1_000 + round, |client| keystrokes(client, 180_000))
            .expect("signed");
        type_at(&mut a, typed.len(), "a");
        typed.push('a');
        both_rounds(&mut pair).await;
        assert_eq!(pair.sync_a.text_status(&rid), Some(LineageStatus::Merging));
        assert!(
            read(&pair.a, PLAN).starts_with(&format!("{typed}#")),
            "round {round}"
        );
        assert!(pair.sync_a.in_text_mode(&rid));
    }
    let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
    assert!(
        file.records
            .iter()
            .any(|r| r.judgement == Judgement::Waiting),
        "the writer's last records wait on the allowance"
    );
    assert!(file.reviews.is_empty(), "{:?}", file.reviews);

    // A snapshot leaves them live, as the fixture's case says.
    let live: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/workspace-format/fixtures/sync-v2-live.json"
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(
        live["cases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|case| case["name"] == "a_snapshot_leaves_a_deferred_patch_live")
    );
    let waiting: Vec<_> = file
        .records
        .iter()
        .filter(|r| r.judgement == Judgement::Waiting)
        .map(|r| r.key)
        .collect();
    let lowest = waiting.iter().map(|key| key.counter).min().unwrap();
    pair.sync_a.write_snapshot_for_testing(rid).unwrap();
    both_rounds(&mut pair).await;
    let file = pair.sync_a.text_file(&rid).unwrap().unwrap();
    for key in waiting {
        assert!(file.records.iter().any(|r| r.key == key), "{key:?} stays");
    }
    let snapshot = file
        .records
        .iter()
        .rfind(|r| r.snapshot && r.key.device == pair.sync_a.device())
        .unwrap();
    assert!(
        snapshot
            .covers
            .iter()
            .all(|cover| cover.device != pair.sync_b.device() || cover.counter < lowest),
        "{:?}",
        snapshot.covers
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_that_boots_reaches_the_body_of_one_that_merged_as_records_arrived() {
    let mut pair = pair().await;
    let (mut a, mut b) = open_both(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).unwrap();
    // Two signers type at one place at once, and each integrates the other
    // later, in a different order.
    type_at(&mut a, 2, "one ");
    type_at(&mut b, 2, "two ");
    both_rounds(&mut pair).await;
    type_at(&mut a, 0, "x");
    both_rounds(&mut pair).await;
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
    let arrived = read(&pair.a, PLAN);

    let (mut desktop, folder, mut sync) = third_desktop(&mut pair).await;
    for _ in 0..3 {
        sync.round(&mut desktop.service, 0).await.expect("a round");
    }
    assert_eq!(read(&folder, PLAN), arrived);
    let records = sync.text_file(&rid).unwrap().unwrap().records;
    let order: Vec<u64> = records.iter().filter_map(|r| r.seq).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted, "the journal keeps log order");
    let (_, document) = sync.open_text(PLAN).expect("opens");
    assert!(document.is_some());
    assert_eq!(sync.text_body(&rid), Some(arrived.as_str()));
}

async fn error_frame<B: Backend>(pair: &mut Pair<B>, code: &str) -> Vec<String> {
    let before = pair.proxy.requests().len();
    let frame = Frame::Error {
        rejected_type: 2,
        domain: IdOrZero::ZERO,
        session: IdOrZero::ZERO,
        sequence: 1,
        code: Text::new(code).unwrap(),
        detail: None,
    };
    pair.sync_a
        .on_frame(&mut pair.service, &frame)
        .await
        .expect("the frame");
    pair.proxy.requests()[before..]
        .iter()
        .map(|request| request.path.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_presence_record_fetches_policy_and_keys() {
    let mut pair = pair().await;
    let _ = open_both(&mut pair).await;
    for code in ["epoch_stale", "forbidden", "rotation_pending"] {
        let asked = error_frame(&mut pair, code).await;
        assert!(
            asked.iter().any(|path| path.contains("/policy"))
                && asked.iter().any(|path| path.contains("/keys")),
            "{code}: {asked:?}"
        );
    }
    let asked = error_frame(&mut pair, "too_many").await;
    assert!(asked.is_empty(), "{asked:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_goes_out_on_the_tick_while_frames_keep_arriving() {
    let mut pair = pair().await;
    let (_a, mut b) = open_both(&mut pair).await;
    let wake = tokio::sync::Notify::new();
    let mut live = app_core::sync::realtime::Live::default();
    pair.sync_a
        .set_selections(PLAN, vec![SelectionRange { anchor: 1, head: 1 }], true);
    // The channel comes up, and the first pull passes.
    for _ in 0..2 {
        live.step(&mut pair.sync_a, &mut pair.service, &wake)
            .await
            .expect("a step");
    }
    let mut listener = Channel::open(pair.desktop.service.client(), pair.sync_b.workspace())
        .await
        .expect("the channel");
    pair.sync_b.presence_connected().unwrap();
    for frame in pair.sync_b.presence_frames().unwrap() {
        listener.send(&frame).await.unwrap();
    }

    // Records frames arrive every 100 ms, faster than the tick.
    let started = Instant::now();
    let mut presence = 0;
    let mut steps = 0;
    while started.elapsed() < Duration::from_millis(1_600) {
        type_at(&mut b, 0, "b");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
        let step = tokio::time::timeout(
            Duration::from_millis(100),
            live.step(&mut pair.sync_a, &mut pair.service, &wake),
        )
        .await;
        if let Ok(step) = step {
            step.expect("a step");
            steps += 1;
        }
        while let Ok(frame) = tokio::time::timeout(Duration::from_millis(1), listener.next()).await
        {
            if matches!(frame, Ok(Frame::Presence { .. })) {
                presence += 1;
            }
        }
        pair.sync_a.set_selections(
            PLAN,
            vec![SelectionRange {
                anchor: 1 + steps,
                head: 1 + steps,
            }],
            true,
        );
    }
    assert!(presence >= 1, "the tick ran under {steps} steps");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_persons_two_devices_share_a_cursor_color() {
    let mut pair = pair().await;
    let (_a, _b, mut channel_a, mut channel_b) = cursors_ready(&mut pair).await;
    send_selection(&mut pair, &mut channel_a, 1).await;
    drain(&mut channel_b, &mut pair.sync_b, &mut pair.desktop.service).await;
    let cursors = pair.sync_b.cursors(PLAN, Instant::now());
    assert_eq!(cursors.len(), 1);
    assert!(cursors[0].color < app_core::sync::workspace::presence::CURSOR_COLORS);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_note_gets_its_turn_of_presence_records() {
    let mut pair = pair().await;
    let (_a, _b) = open_both(&mut pair).await;
    pair.a.write("Second.md", b"# Second\n");
    both_rounds(&mut pair).await;
    let (_, second) = pair.sync_a.open_text("Second.md").expect("opens");
    assert!(second.is_some());
    pair.sync_a.presence_connected().unwrap();
    pair.sync_a.presence_frames().unwrap();
    let mut told = std::collections::BTreeSet::new();
    for turn in 0..4 {
        for path in [PLAN, "Second.md"] {
            pair.sync_a.set_selections(
                path,
                vec![SelectionRange {
                    anchor: turn,
                    head: turn,
                }],
                true,
            );
        }
        for frame in pair.sync_a.presence_frames().unwrap() {
            if let Frame::Presence { record } = frame {
                told.insert(record.body.channel);
            }
        }
    }
    assert_eq!(told.len(), 2, "both notes went out");
}

async fn search_for(engine: &local_core::WorkspaceEngine, text: &str) -> usize {
    engine
        .search(&local_core::SearchInput {
            query: text.to_owned(),
            ..Default::default()
        })
        .map_or(0, |results| results.len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_index_gives_way_to_a_text_mode_notes_file() {
    let mut pair = pair().await;
    let (mut a, b) = managed_note(&mut pair).await;
    both_rounds(&mut pair).await;
    let index = pair.b.engine.index_path().to_path_buf();
    let stale = index.with_extension("old");
    std::fs::copy(&index, &stale).unwrap();
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    type_at(&mut a, 0, "zebrafish ");
    both_rounds(&mut pair).await;
    assert!(read(&pair.b, PLAN).contains("zebrafish"));
    drop(b);
    drop(a);

    // An old copy of the index comes back, as a restored backup would.
    pair.sync_b.close_text(PLAN).expect("closed");
    drop(pair.sync_b);
    std::fs::copy(&stale, &index).unwrap();
    pair.b.reopen();
    pair.sync_b = WorkspaceSync::open(
        pair.b.engine.clone(),
        pair.desktop.credentials.clone(),
        pair.desktop.secrets(),
        &pair.b.registry,
    )
    .expect("the client reopens");
    assert!(
        search_for(&pair.b.engine, "zebrafish").await > 0,
        "files win"
    );
    both_rounds(&mut pair).await;
    assert!(pair.sync_b.in_text_mode(&rid));
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_update_that_fails_leaves_a_remote_edit_on_disk() {
    let mut pair = pair().await;
    let (mut a, _b) = managed_note(&mut pair).await;
    both_rounds(&mut pair).await;
    // The index can't take the change.
    let index = pair.b.engine.index_path().to_path_buf();
    let blocker = rusqlite::Connection::open(&index).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();
    type_at(&mut a, 0, "platypus ");
    both_rounds(&mut pair).await;
    assert!(
        read(&pair.b, PLAN).contains("platypus"),
        "the file is canonical"
    );
    blocker.execute_batch("ROLLBACK").unwrap();
    drop(blocker);
    // The interrupted index catches up from the file on its next walk.
    pair.b.engine.reconcile().expect("the index catches up");
    assert!(search_for(&pair.b.engine, "platypus").await > 0);
    assert_eq!(read(&pair.a, PLAN), read(&pair.b, PLAN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_file_with_a_text_mode_notes_id_stays_out_until_you_resolve_it() {
    let mut pair = pair().await;
    let (mut a, _b) = managed_note(&mut pair).await;
    both_rounds(&mut pair).await;
    let rid = pair.sync_b.rid_at(PLAN).unwrap();
    let copy = read(&pair.b, PLAN);
    pair.b.write("Copy.md", copy.as_bytes());
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_b.rid_at("Copy.md"), None);
    assert!(
        pair.sync_b.status().skipped.iter().any(|skipped| {
            skipped.path == "Copy.md"
                && skipped.why == app_core::sync::workspace::SkipWhy::IdentityConflict
        }),
        "{:?}",
        pair.sync_b.status().skipped
    );
    assert!(!pair.a.files().contains_key("Copy.md"));
    // The note keeps merging.
    type_at(&mut a, 0, "still ");
    both_rounds(&mut pair).await;
    assert!(pair.sync_b.in_text_mode(&rid));
    assert!(read(&pair.b, PLAN).contains("still"));
}
