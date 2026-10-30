//! Share links end to end (seam 4, sync-v2.md §24): the real client
//! creates, lists, updates and revokes links against the server on SQLite
//! and on PostgreSQL, and a browser's view of a link opens with the keys in
//! its fragment. No request carries the link key or the verify key, and the
//! server stores neither.

mod sync_support;

use app_core::sync::shares::{DEFAULT_SHARE_DAYS, NewShare, ShareBook, snapshot_of};
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::{SyncError, SyncService};
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::ShareBody;
use sync_protocol::records::{SharePayload, open_share, parse_share_fragment};
use sync_protocol::types::{Id, to_base64url, to_hex};
use sync_store::{Backend, ShareRow, WriteTxn};
use sync_support::{Desktop, Folder, Server, first_desktop, pair};

const NOTE: &str =
    "---\nid: note_01k0000000000000000000000p\ntype: note\n---\n\n# Plan\n\nTomatoes in May.\n";

/// What a browser with no account reads at the link: the snapshot opened
/// with the keys in the fragment, or the HTTP status when it doesn't open.
async fn view(url: &str) -> Result<(u64, SharePayload), u16> {
    let (address, fragment) = url.split_once('#').expect("a fragment");
    let (origin, token) = address.split_once("/s/").expect("a share path");
    let response = reqwest::get(format!("{origin}/v2/shares/{token}"))
        .await
        .expect("the server answers");
    if !response.status().is_success() {
        return Err(response.status().as_u16());
    }
    let page = reqwest::get(address).await.expect("the page");
    assert!(page.status().is_success());
    let body = ShareBody::from_bytes(&response.bytes().await.unwrap()).expect("a share body");
    let (link_key, verify_key) = parse_share_fragment(fragment).expect("the fragment");
    let (share, payload) =
        open_share(&body.share.0.to_bytes(), &link_key, &verify_key).expect("it opens");
    Ok((share.body.snapshot_revision, payload))
}

/// Every form in which a request could carry `secret`.
fn forms(secret: &[u8]) -> Vec<Vec<u8>> {
    vec![
        secret.to_vec(),
        to_base64url(secret).into_bytes(),
        to_hex(secret).into_bytes(),
    ]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Creates a link to `Plan.md` in `sync`'s workspace through `service`.
async fn share_plan(
    service: &mut SyncService,
    book: &mut ShareBook,
    sync: &WorkspaceSync,
    days: u8,
) -> app_core::sync::shares::CreatedShare {
    let (title, markdown) = snapshot_of("Plan", NOTE);
    service
        .create_share(
            book,
            NewShare {
                workspace: sync.workspace(),
                domain: sync.domain(),
                note: sync.rid_at("Plan.md").expect("Plan.md syncs"),
                title: &title,
                markdown: markdown.as_bytes(),
                days,
            },
        )
        .await
        .expect("the link is created")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_shows_its_snapshot_until_someone_revokes_it() {
    let mut pair = pair().await;
    pair.a.write("Plan.md", NOTE.as_bytes());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let mut book = ShareBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();

    // A response the client can't read makes it send the same bytes
    // again, and the server returns the original link.
    let mangled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let once = mangled.clone();
    pair.proxy.tamper(Box::new(move |path, bytes| {
        if path.ends_with("/shares") && !once.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return vec![0xff];
        }
        bytes
    }));
    let created = share_plan(
        &mut pair.service,
        &mut book,
        &pair.sync_a,
        DEFAULT_SHARE_DAYS,
    )
    .await;
    pair.proxy.honest();
    assert!(mangled.load(std::sync::atomic::Ordering::SeqCst));
    let posts: Vec<_> = pair
        .proxy
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/shares"))
        .collect();
    assert_eq!(posts.len(), 2, "the creation went out twice");
    assert_eq!(posts[0].body, posts[1].body, "with identical bytes");
    let now = pair.server.store.clock().now();
    let expires_at = created.link.expires_at;
    assert!((now + 30 * 86_400 - 60..=now + 30 * 86_400).contains(&expires_at));

    // A browser with no account reads the snapshot.
    let url = created.url.to_string();
    assert!(url.starts_with(&format!("{}/s/", pair.proxy.base)));
    let (revision, payload) = view(&url).await.expect("the link opens");
    assert_eq!(revision, 1);
    assert_eq!(payload.title.0, "Plan");
    assert_eq!(payload.markdown.0, b"# Plan\n\nTomatoes in May.\n");

    // The creating device lists the note's link, and updates it.
    let rid = pair.sync_a.rid_at("Plan.md").unwrap();
    let mine = book.note_links(pair.sync_a.workspace(), rid);
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].token_hash, created.link.token_hash);
    pair.service
        .update_share(&mut book, created.link.token_hash, "Plan", b"Peppers too.")
        .await
        .expect("updated");
    let (revision, payload) = view(&url).await.expect("the link opens");
    assert_eq!(revision, 2, "a reload shows the new version");
    assert_eq!(payload.markdown.0, b"Peppers too.");
    assert_eq!(
        pair.service
            .share_url(&book, &created.link.token_hash)
            .unwrap(),
        url,
        "the creating device copies the same link again"
    );

    // Another owner device lists the workspace's links by token hash,
    // domain, creating device and expiry, can't update one and revokes it.
    let snapshot = pair
        .sync_b
        .remote_mut()
        .unwrap()
        .policy()
        .head_snapshot()
        .clone();
    let listed = pair
        .desktop
        .service
        .workspace_shares(pair.sync_b.workspace(), &snapshot)
        .await
        .expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].token_hash, created.link.token_hash);
    assert_eq!(listed[0].domain, pair.sync_a.domain());
    assert_eq!(listed[0].device, pair.laptop.id());
    assert_eq!(listed[0].expires_at, expires_at);
    assert!(!listed[0].creator_removed);
    let mut other_book = ShareBook::open(pair.desktop.state.path(), pair.desktop.id()).unwrap();
    let refused = pair
        .desktop
        .service
        .update_share(&mut other_book, created.link.token_hash, "Plan", b"Mine")
        .await;
    assert!(matches!(refused, Err(SyncError::Invalid(_))), "{refused:?}");
    pair.desktop
        .service
        .revoke_share(
            &mut other_book,
            pair.sync_b.workspace(),
            created.link.token_hash,
            expires_at,
        )
        .await
        .expect("revoked");
    assert_eq!(
        view(&url).await.err(),
        Some(404),
        "the link shows not found"
    );
    let page = reqwest::get(url.split_once('#').unwrap().0).await.unwrap();
    assert_eq!(page.status().as_u16(), 404);
    let kept = other_book.revoked();
    assert_eq!(kept.len(), 1, "the revoking device keeps the token hash");
    assert_eq!(kept[0].token_hash, created.link.token_hash);
    assert_eq!(kept[0].expires_at, expires_at);
    let reopened = ShareBook::open(pair.desktop.state.path(), pair.desktop.id()).unwrap();
    assert_eq!(reopened.revoked(), kept, "across relaunch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_request_carries_and_no_row_holds_the_link_keys() {
    let mut pair = pair().await;
    let mut book = ShareBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();
    let created = share_plan(&mut pair.service, &mut book, &pair.sync_a, 7).await;
    pair.service
        .update_share(&mut book, created.link.token_hash, "Plan", b"Again")
        .await
        .expect("updated");
    view(&created.url).await.expect("the link opens");
    let (_, fragment) = created.url.split_once('#').unwrap();
    let (link_key, verify_key) = parse_share_fragment(fragment).unwrap();
    let mut secrets = forms(link_key.expose());
    secrets.extend(forms(&verify_key.0));
    secrets.push(fragment.as_bytes().to_vec());

    let requests = pair.proxy.requests();
    assert!(requests.iter().any(|r| r.path.contains("/shares")));
    for request in &requests {
        for secret in &secrets {
            assert!(
                !contains(request.path.as_bytes(), secret),
                "{}",
                request.path
            );
            assert!(
                !contains(&request.body, secret),
                "{} {}",
                request.method,
                request.path
            );
            for (name, value) in &request.headers {
                assert!(!contains(value, secret), "header {name}");
            }
        }
    }

    // The server's rows hold the token hash, workspace, domain, creating
    // device, expiry and signed snapshot, and no key.
    let hash = created.link.token_hash;
    let row = pair
        .server
        .store
        .backend()
        .read(Box::new(
            move |txn: &mut <sync_store_sqlite::SqliteBackend as Backend>::Read| {
                Box::pin(async move { sync_store::ReadTxn::share(txn, hash).await })
            },
        ))
        .await
        .unwrap()
        .expect("the row");
    assert_eq!(
        (row.workspace, row.domain, row.device, row.expires_at),
        (
            pair.sync_a.workspace(),
            pair.sync_a.domain(),
            pair.laptop.id(),
            created.link.expires_at
        )
    );
    let data = pair.server.dir.path().join("data");
    let mut stored = Vec::new();
    for entry in std::fs::read_dir(&data).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            stored.extend(std::fs::read(&path).unwrap());
        }
    }
    assert!(
        contains(&stored, &hash.0),
        "the database holds the token hash"
    );
    for secret in &secrets {
        assert!(!contains(&stored, secret), "the database holds a key");
    }
    let database = rusqlite::Connection::open_with_flags(
        data.join(sync_support::DATABASE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("the database opens");
    let columns: Vec<String> = database
        .prepare("SELECT name FROM pragma_table_info('shares') ORDER BY cid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        columns,
        [
            "token_hash",
            "workspace",
            "domain",
            "device",
            "expires_at",
            "share"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_devices_flag_a_link_whose_creator_left() {
    let mut pair = pair().await;
    let gone = Id::random().unwrap();
    let row = ShareRow {
        token_hash: sync_protocol::types::Digest([3; 32]),
        workspace: pair.sync_a.workspace(),
        domain: pair.sync_a.domain(),
        device: gone,
        expires_at: pair.server.store.clock().now() + 86_400,
        share: vec![0; 300],
    };
    pair.server
        .store
        .backend()
        .write(Box::new(
            move |txn: &mut <sync_store_sqlite::SqliteBackend as Backend>::Write| {
                Box::pin(async move { txn.insert_share(&row).await })
            },
        ))
        .await
        .unwrap();
    let mut book = ShareBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();
    share_plan(&mut pair.service, &mut book, &pair.sync_a, 1).await;
    let snapshot = pair
        .sync_b
        .remote_mut()
        .unwrap()
        .policy()
        .head_snapshot()
        .clone();
    let mut listed = pair
        .desktop
        .service
        .workspace_shares(pair.sync_b.workspace(), &snapshot)
        .await
        .unwrap();
    listed.sort_by_key(|s| s.creator_removed);
    assert_eq!(listed.len(), 2);
    assert!(!listed[0].creator_removed && listed[0].device == pair.laptop.id());
    assert!(listed[1].creator_removed && listed[1].device == gone);
}

async fn end_to_end<B: Backend>(server: Server<B>) {
    let mut desktop: Desktop = first_desktop(&server, "ana@example.com").await;
    let mut folder = Folder::new("Field notes");
    folder.write("Plan.md", NOTE.as_bytes());
    let mut sync = WorkspaceSync::turn_on(
        &mut desktop.service,
        folder.engine.clone(),
        "Field notes",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.round(&mut desktop.service, 0).await.expect("a round");
    let mut book = ShareBook::open(desktop.state.path(), desktop.id()).unwrap();
    let created = share_plan(&mut desktop.service, &mut book, &sync, 30).await;
    let (_, payload) = view(&created.url).await.expect("the link opens");
    assert_eq!(payload.markdown.0, b"# Plan\n\nTomatoes in May.\n");
    desktop
        .service
        .update_share(&mut book, created.link.token_hash, "Plan", b"Later")
        .await
        .unwrap();
    assert_eq!(view(&created.url).await.unwrap().1.markdown.0, b"Later");
    let snapshot = sync.remote_mut().unwrap().policy().head_snapshot().clone();
    let listed = desktop
        .service
        .workspace_shares(sync.workspace(), &snapshot)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    desktop
        .service
        .revoke_share(
            &mut book,
            sync.workspace(),
            created.link.token_hash,
            created.link.expires_at,
        )
        .await
        .unwrap();
    assert_eq!(view(&created.url).await.err(), Some(404));
    assert!(
        book.note_links(sync.workspace(), sync.rid_at("Plan.md").unwrap())
            .is_empty()
    );
    // A link above 960 KiB never leaves the device.
    let (title, _) = snapshot_of("Plan", NOTE);
    let large = vec![b'a'; 990_000];
    let refused = desktop
        .service
        .create_share(
            &mut book,
            NewShare {
                workspace: sync.workspace(),
                domain: sync.domain(),
                note: sync.rid_at("Plan.md").unwrap(),
                title: &title,
                markdown: &large,
                days: 1,
            },
        )
        .await;
    assert!(matches!(refused, Err(SyncError::Invalid(ref t)) if t.contains("960 KB")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_links_work_end_to_end_on_sqlite() {
    end_to_end(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_links_work_end_to_end_on_postgres() {
    end_to_end(Server::start_postgres().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_answer_never_strands_a_link() {
    use sync_protocol::codec::Text;
    use sync_protocol::messages::ErrorBody;

    let mut pair = pair().await;
    pair.a.write("Plan.md", NOTE.as_bytes());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let workspace = pair.sync_a.workspace();
    let rid = pair.sync_a.rid_at("Plan.md").unwrap();
    let (title, markdown) = snapshot_of("Plan", NOTE);
    let new_share = || NewShare {
        workspace,
        domain: pair.sync_a.domain(),
        note: rid,
        title: &title,
        markdown: markdown.as_bytes(),
        days: DEFAULT_SHARE_DAYS,
    };
    let mut book = ShareBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();

    // A refused creation leaves nothing behind: no link in the book and no
    // secrets in the credential store.
    pair.proxy.forge(Box::new(|method, path| {
        (method == "POST" && path.ends_with("/shares")).then(|| {
            let body = ErrorBody {
                code: Text("share_limit".into()),
                message: Text("full".into()),
                retry_after_seconds: None,
                detail: None,
            };
            (403, body.to_bytes())
        })
    }));
    let refused = pair.service.create_share(&mut book, new_share()).await;
    assert!(
        matches!(&refused, Err(SyncError::Refused { code, .. }) if code == "share_limit"),
        "{refused:?}"
    );
    pair.proxy.honest();
    assert!(book.note_links(workspace, rid).is_empty());

    // Both answers to a creation are lost after the server commits it. The
    // link stays pending with its secrets, and the next settle confirms it
    // with the same bytes instead of leaving a live link the device can't
    // reach.
    pair.proxy.lose(Box::new(|method, path| {
        method == "POST" && path.ends_with("/shares")
    }));
    let lost = pair.service.create_share(&mut book, new_share()).await;
    assert!(matches!(lost, Err(SyncError::Network(_))), "{lost:?}");
    pair.proxy.honest();
    let pending = book.note_links(workspace, rid);
    assert_eq!(pending.len(), 1, "the device keeps the unconfirmed link");
    assert_eq!(pending[0].revision, 0);
    assert!(pending[0].pending.is_some());
    let token_hash = pending[0].token_hash;
    let mut book = ShareBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();
    pair.service
        .settle_shares(&mut book, workspace)
        .await
        .expect("settled");
    let link = book.link(&token_hash).cloned().unwrap();
    assert_eq!(link.revision, 1);
    assert!(link.pending.is_none());
    let url = pair.service.share_url(&book, &token_hash).unwrap();
    assert_eq!(view(&url).await.expect("the link opens").0, 1);
    let snapshot = pair
        .sync_a
        .remote_mut()
        .unwrap()
        .policy()
        .head_snapshot()
        .clone();
    let listed = pair
        .service
        .workspace_shares(workspace, &snapshot)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1, "one link, not a duplicate");
    assert_eq!(listed[0].expires_at, link.expires_at);

    // An update's answer is lost after the server commits it, and the
    // device restarts before it hears back. The next update still goes
    // through, under the same link.
    pair.proxy.lose(Box::new(|method, _| method == "PUT"));
    let lost = pair
        .service
        .update_share(&mut book, token_hash, "Plan", b"Peppers too.")
        .await;
    assert!(matches!(lost, Err(SyncError::Network(_))), "{lost:?}");
    pair.proxy.honest();
    assert_eq!(view(&url).await.unwrap().0, 2, "the server took the update");
    let mut book = ShareBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();
    assert_eq!(book.link(&token_hash).unwrap().revision, 1);
    pair.service
        .update_share(&mut book, token_hash, "Plan", b"And basil.")
        .await
        .expect("the next update goes through");
    let (revision, payload) = view(&url).await.unwrap();
    assert_eq!(revision, 3);
    assert_eq!(payload.markdown.0, b"And basil.");
    assert_eq!(book.link(&token_hash).unwrap().revision, 3);
    let puts = pair
        .proxy
        .requests()
        .into_iter()
        .filter(|r| r.method == "PUT")
        .collect::<Vec<_>>();
    assert_eq!(puts.len(), 3);
    assert_eq!(
        puts[0].body, puts[1].body,
        "the lost update went again as is"
    );

    // Revoking with an expiry the device doesn't know keeps the token hash
    // as long as any link can live.
    let now = pair.server.store.clock().now();
    pair.service
        .revoke_share(&mut book, workspace, token_hash, 0)
        .await
        .expect("revoked");
    let kept = book.revoked();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].expires_at >= now + 29 * 86_400, "{kept:?}");
}
