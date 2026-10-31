//! A server restored from a backup (seam 4, sync-v2.md §14): a device that
//! sees a new generation stops, compares what it accepted with what the
//! server holds, and either stops with `sync_restore_lost` or rewinds and
//! writes the lost content again from its files. A database put back
//! without a new generation is caught as a rollback.

mod sync_support;

use app_core::sync::workspace::restore::Lost;
use sync_store_sqlite::SqliteBackend;
use sync_support::{Pair, approve, pair};

const NO_WAIT: std::time::Duration = std::time::Duration::ZERO;

/// Both devices through the proxy, which now reaches `base`.
async fn reconnect(pair: &mut Pair<SqliteBackend>, base: &str) {
    pair.proxy.retarget(base);
    pair.service = pair.laptop.connect_through(&pair.proxy.base).await;
    pair.desktop.service = pair.desktop.connect_through(&pair.proxy.base).await;
    pair.sync_a.set_restore_wait(NO_WAIT);
    pair.sync_b.set_restore_wait(NO_WAIT);
}

async fn rounds(pair: &mut Pair<SqliteBackend>) {
    for _ in 0..3 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("desktop");
    }
}

/// A desktop that wrote behind the backup: the laptop writes, both apply.
async fn write_after_backup(pair: &mut Pair<SqliteBackend>) {
    pair.a.write("Lost.md", b"# Lost\n");
    rounds(pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_that_lost_only_content_writes_it_again() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    let base = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &base).await;
    rounds(&mut pair).await;

    assert_eq!(pair.a.files(), files, "no file changed on the laptop");
    assert_eq!(pair.b.files(), files, "no file changed on the desktop");
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(pair.sync_a.restore_alert().is_some(), "the app alerts you");
    assert!(pair.sync_a.status().held == 0 && pair.sync_b.status().held == 0);
    let written = [pair.sync_a.written_again(), pair.sync_b.written_again()].concat();
    assert!(written.iter().any(|path| path == "Lost.md"));

    // The lost file is back on the server: a new device finds it, and the
    // devices keep syncing.
    pair.b.write("After.md", b"# After\n");
    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.a.files().contains_key("After.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_copy_of_the_database_is_a_rollback_until_the_generation_changes() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    let base = pair.server.put_back(&copy, false).await;
    reconnect(&mut pair, &base).await;
    pair.a.write("Held.md", b"# Held\n");
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        round
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
    assert_eq!(pair.b.files(), files, "the desktop's files stay");
    let again = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(again.is_err(), "the device stays held");

    // `mark-restored` raises the generation, and the devices compare.
    let base = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.rollback(), None);
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.a.files().contains_key("Lost.md"));
    assert!(pair.a.files().contains_key("Held.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lower_generation_is_a_rollback() {
    let mut pair = pair().await;
    let first = pair.server.backup("first.sqlite3").await;
    let base = pair.server.put_back(&first, true).await;
    reconnect(&mut pair, &base).await;
    rounds(&mut pair).await;
    assert!(pair.sync_a.restore_alert().is_some());

    // The original server, at the generation before.
    let old = pair.server.base.clone();
    reconnect(&mut pair, &old).await;
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        round
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_link_of_your_own_chain_stops_the_workspace() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    // A third device: a link of the account chain the backup lacks.
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let _ = third.service.client();
    rounds(&mut pair).await;
    pair.a.write("Kept.md", b"# Kept\n");
    let before = pair.a.files();

    let base = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &base).await;
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_ok(), "{round:?}");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::OwnAccount));
    assert_eq!(
        pair.a.files(),
        before,
        "the stopped workspace keeps its files"
    );
    pair.a.write("Later.md", b"# Later\n");
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.pushed, 0, "it writes nothing");
}
