//! What the client does when the server refuses one of its records (seam 4,
//! sync-v2.md §14): it re-seals the change at the same position, naming the
//! refused attempt in `previous_attempt`, up to 3 times when no newer epoch
//! or policy change forces it, then reports `sync_reseal_limit`, signs no
//! more there and retries the last attempt's bytes. A record refused for
//! its object gives its position to the next record, and one whose write
//! access is gone is dropped while the file keeps the change.

mod sync_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sync_protocol::codec::{Bytes, Encode, Text};
use sync_protocol::messages::{EntryRecord, ErrorBody, PushFailureDetail};
use sync_support::{Pair, Proxy, pair};

/// Refuses the next `times` pushes through `proxy` with `code` for the
/// push's first record, and counts them.
fn refuse_pushes(proxy: &Proxy, code: &'static str, status: u16, times: usize) -> Arc<AtomicUsize> {
    let refused = Arc::new(AtomicUsize::new(0));
    let count = refused.clone();
    proxy.forge(Box::new(move |method, path| {
        if method != "POST" || !path.ends_with("/ops") {
            return None;
        }
        if count.fetch_add(1, Ordering::SeqCst) >= times {
            return None;
        }
        let detail = PushFailureDetail {
            record_index: 0,
            code: Text(code.to_owned()),
            record_detail: None,
        };
        let body = ErrorBody {
            code: Text(code.to_owned()),
            message: Text("refused".to_owned()),
            retry_after_seconds: None,
            detail: Some(Bytes(detail.to_bytes())),
        };
        Some((status, body.to_bytes()))
    }));
    refused
}

/// The laptop's record at `counter`, as the server holds it.
async fn stored(
    pair: &mut Pair<sync_store_sqlite::SqliteBackend>,
    counter: u64,
) -> sync_protocol::records::Signed<sync_protocol::records::Operation> {
    let workspace = pair.sync_a.workspace();
    let domain = pair.sync_a.remote_mut().unwrap().domain;
    let device = pair.laptop.id();
    let entry = pair
        .service
        .client()
        .chain_position(workspace, domain, device, counter)
        .await
        .unwrap()
        .expect("a record there");
    let EntryRecord::Operation(op) = entry.record else {
        panic!("an operation");
    };
    op
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_attempt_is_named_by_the_next_one() {
    let mut pair = pair().await;
    pair.a.write("Plan.md", b"# Plan, edited\n");
    assert_eq!(pair.sync_a.capture().unwrap(), 1);
    let first = pair.sync_a.outbox()[0];
    refuse_pushes(&pair.proxy, "policy_regressed", 422, 1);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the re-sealed record goes through");
    let counter = {
        let workspace = pair.sync_a.workspace();
        let domain = pair.sync_a.remote_mut().unwrap().domain;
        pair.service
            .client()
            .chain_head(workspace, domain, pair.laptop.id())
            .await
            .unwrap()
            .unwrap()
            .counter
    };
    let op = stored(&mut pair, counter).await;
    assert_ne!(op.digest(), first);
    assert_eq!(op.body.previous_attempt, first);
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    assert_eq!(pair.b.files(), pair.a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_unforced_reseals_then_the_last_bytes_wait() {
    let mut pair = pair().await;
    pair.a.write("Plan.md", b"# Plan, edited\n");
    let refused = refuse_pushes(&pair.proxy, "policy_regressed", 422, 100);
    let result = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        result
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_reseal_limit")
    );
    assert_eq!(
        refused.load(Ordering::SeqCst),
        4,
        "the first attempt and 3 re-seals"
    );
    assert_eq!(pair.sync_a.stopped(), Some("sync_reseal_limit"));
    let last = pair.sync_a.outbox();
    // The device signs nothing more while it waits; the file keeps the
    // edit.
    pair.a.write("Other.md", b"# Other\n");
    assert_eq!(pair.sync_a.capture().unwrap(), 0);
    assert!(pair.sync_a.round(&mut pair.service, 0).await.is_err());
    assert_eq!(
        pair.sync_a.outbox(),
        last,
        "no other attempt at that position"
    );
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan, edited\n");

    // Once the server takes the last attempt's bytes, writing resumes.
    pair.proxy.honest();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.sync_a.stopped(), None);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    assert_eq!(pair.b.files(), pair.a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_refused_for_its_object_gives_its_place_to_the_next() {
    let mut pair = pair().await;
    pair.a.write("Plan.md", b"# Plan, too many branches\n");
    pair.a.write("Notes.md", b"# Notes\n");
    assert_eq!(pair.sync_a.capture().unwrap(), 2);
    let rejected = pair.sync_a.outbox()[0];
    assert_eq!(pair.sync_a.outbox().len(), 2);
    refuse_pushes(&pair.proxy, "conflict_limit", 409, 1);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    // The next record took the refused record's position and names it.
    let workspace = pair.sync_a.workspace();
    let domain = pair.sync_a.remote_mut().unwrap().domain;
    let device = pair.laptop.id();
    let head = pair
        .service
        .client()
        .chain_head(workspace, domain, device)
        .await
        .unwrap()
        .unwrap();
    let mut counter = 1;
    let mut named = None;
    while counter <= head.counter {
        let op = stored(&mut pair, counter).await;
        if op.body.previous_attempt == rejected {
            named = Some(op.clone());
        }
        counter = op.body.counter + 1;
    }
    assert!(named.is_some(), "the replacement names the refused record");
    // The refused change stays in its file and goes out again.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    assert_eq!(pair.b.files(), pair.a.files());
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan, too many branches\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_whose_write_access_is_gone_is_dropped() {
    let mut pair = pair().await;
    pair.a.write("Plan.md", b"# Plan, after access ended\n");
    refuse_pushes(&pair.proxy, "forbidden", 403, 1);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert!(pair.sync_a.outbox().is_empty(), "the record is dropped");
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan, after access ended\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan\n");
}
