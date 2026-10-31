//! Restricted folders end to end (seam 4, sync-v2.md §8, §9): restricting a
//! folder pushes `create_domain` with epoch 1, envelopes and the marker;
//! members outside the domain receive nothing of it, whatever they ask;
//! domain roots decide where new files go; and a device looks after the
//! marker file itself.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::invitations::InvitationBook;
use app_core::sync::realtime::Channel;
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::workspace::text::{Entered, LeftWhy, TextReviewKind};
use editor_core::text_mode::{CollabDocument, Utf16Splice};
use sync_protocol::matching::MARKER;
use sync_protocol::records::{Access, Level, Role, marker_content};
use sync_store::Backend;
use sync_support::{Desktop, Folder, Proxy, Server, first_desktop, invite, invite_and_join};

/// The owner's desktop with a synced folder holding `Plan.md`.
async fn owner<B: Backend>(server: &Server<B>) -> (Desktop, WorkspaceSync, Folder) {
    let mut desktop = first_desktop(server, "ana@example.com").await;
    let mut folder = Folder::new("Field notes");
    folder.write("Plan.md", b"# Plan\n");
    let mut sync = WorkspaceSync::turn_on(
        &mut desktop.service,
        folder.engine.clone(),
        "Field notes",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.round(&mut desktop.service, 0).await.expect("a round");
    (desktop, sync, folder)
}

async fn settle(sync: &mut WorkspaceSync, desktop: &mut Desktop) {
    for _ in 0..4 {
        sync.round(&mut desktop.service, 0).await.expect("a round");
    }
}

#[tokio::test]
async fn restricting_a_folder_writes_its_marker_and_routes_new_files() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    let domain = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    let marker = folder.files();
    assert_eq!(
        std::fs::read(folder.path(&format!("Pay/{MARKER}"))).unwrap(),
        marker_content(&domain).into_bytes(),
        "the single line {:?}",
        marker.keys()
    );
    assert_eq!(sync.domain_roots().len(), 1);

    folder.write("Pay/Salaries.md", b"secret\n");
    folder.write("Open.md", b"open\n");
    settle(&mut sync, &mut desktop).await;
    let inside = sync.rid_at("Pay/Salaries.md").expect("captured");
    let outside = sync.rid_at("Open.md").expect("captured");
    assert_eq!(sync.object_domain(&inside), Some(domain));
    assert_eq!(sync.object_domain(&outside), None);
    assert_eq!(sync.status().pending, 0, "everything reached the server");
    assert_eq!(sync.status().held_roots, 0);
}

#[tokio::test]
async fn a_marker_inside_another_root_is_refused() {
    let server = Server::start().await;
    let (mut desktop, mut sync, _folder) = owner(&server).await;
    sync.restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    let refused = sync
        .restrict_folder(&mut desktop.service, "Pay/Bonus", &[])
        .await;
    assert!(matches!(refused, Err(SyncError::Invalid(_))));
    let at_root = sync.restrict_folder(&mut desktop.service, "", &[]).await;
    assert!(matches!(at_root, Err(SyncError::Invalid(_))));
}

#[tokio::test]
async fn a_device_looks_after_its_marker_file() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    let domain = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    let marker = format!("Pay/{MARKER}");
    let canonical = marker_content(&domain).into_bytes();

    std::fs::remove_file(folder.path(&marker)).unwrap();
    settle(&mut sync, &mut desktop).await;
    assert_eq!(std::fs::read(folder.path(&marker)).unwrap(), canonical);

    std::fs::write(folder.path(&marker), b"domain: edited\n").unwrap();
    settle(&mut sync, &mut desktop).await;
    assert_eq!(std::fs::read(folder.path(&marker)).unwrap(), canonical);

    // A placeholder counts as present, so nothing writes it again.
    std::fs::remove_file(folder.path(&marker)).unwrap();
    std::fs::write(folder.path("Pay/.noura-domain.icloud"), b"").unwrap();
    settle(&mut sync, &mut desktop).await;
    assert!(!folder.path(&marker).exists());
    assert_eq!(sync.status().held_roots, 0);
    assert!(!folder.files().contains_key("noura-domain"));
}

#[tokio::test]
async fn a_marker_moves_with_its_folder() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    let domain = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    folder.write("Pay/Salaries.md", b"secret\n");
    settle(&mut sync, &mut desktop).await;
    let rid = sync.rid_at("Pay/Salaries.md").expect("captured");

    std::fs::create_dir_all(folder.path("Archive")).unwrap();
    std::fs::rename(folder.path("Pay"), folder.path("Archive/Pay")).unwrap();
    settle(&mut sync, &mut desktop).await;
    assert_eq!(sync.domain_roots()[0].folder, "Archive/Pay");
    assert_eq!(sync.domain_roots()[0].domain, domain);
    assert_eq!(sync.rid_at("Archive/Pay/Salaries.md"), Some(rid));
    assert_eq!(sync.status().held_roots, 0);
    assert_eq!(sync.status().pending, 0);
}

#[tokio::test]
async fn a_grantee_reads_the_domain_and_an_outside_member_receives_nothing() {
    let server = Server::start().await;
    let server = &server;
    let (mut desktop, mut sync, folder) = owner(server).await;
    let mut book = InvitationBook::open(desktop.state.path(), desktop.id()).unwrap();
    let mut grantee = first_desktop(server, "ben@example.com").await;
    let (mut grantee_sync, grantee_folder) = invite_and_join(
        &mut desktop,
        &mut sync,
        &mut book,
        &mut grantee,
        Role::Editor,
    )
    .await;
    let mut outsider = first_desktop(server, "cy@example.com").await;
    let (mut outsider_sync, outsider_folder) = invite_and_join(
        &mut desktop,
        &mut sync,
        &mut book,
        &mut outsider,
        Role::Editor,
    )
    .await;
    settle(&mut sync, &mut desktop).await;
    let domain = sync
        .restrict_folder(
            &mut desktop.service,
            "Pay",
            &[Access {
                account: grantee.credentials.account,
                level: Level::Write,
            }],
        )
        .await
        .expect("restricted");
    folder.write("Pay/Salaries.md", b"secret\n");
    folder.write("Open.md", b"open\n");
    settle(&mut sync, &mut desktop).await;

    settle(&mut grantee_sync, &mut grantee).await;
    assert_eq!(
        grantee_folder
            .files()
            .get("Pay/Salaries.md")
            .map(Vec::as_slice),
        Some(&b"secret\n"[..])
    );
    assert!(grantee_folder.path(&format!("Pay/{MARKER}")).exists());
    grantee_folder.write("Pay/Reply.md", b"seen\n");
    settle(&mut grantee_sync, &mut grantee).await;
    let reply = grantee_sync.rid_at("Pay/Reply.md").expect("captured");
    assert_eq!(grantee_sync.object_domain(&reply), Some(domain));
    settle(&mut sync, &mut desktop).await;
    assert!(folder.files().contains_key("Pay/Reply.md"));

    settle(&mut outsider_sync, &mut outsider).await;
    let files = outsider_folder.files();
    assert!(files.contains_key("Open.md"));
    assert!(!files.keys().any(|path| path.starts_with("Pay")));
    assert!(!outsider_folder.path("Pay").exists());
    assert!(outsider_sync.domain_roots().is_empty());
}

/// An owner, a grantee of `Pay` and a member outside it, each syncing.
struct Team {
    owner: Desktop,
    sync: WorkspaceSync,
    folder: Folder,
    grantee: Desktop,
    grantee_sync: WorkspaceSync,
    grantee_folder: Folder,
    outsider: Desktop,
    outsider_sync: WorkspaceSync,
    outsider_folder: Folder,
    domain: sync_protocol::types::Id,
}

async fn team<B: Backend>(server: &Server<B>) -> Team {
    let (mut owner, mut sync, folder) = self::owner(server).await;
    let mut book = InvitationBook::open(owner.state.path(), owner.id()).unwrap();
    let mut grantee = first_desktop(server, "ben@example.com").await;
    let (grantee_sync, grantee_folder) =
        invite_and_join(&mut owner, &mut sync, &mut book, &mut grantee, Role::Editor).await;
    let mut outsider = first_desktop(server, "cy@example.com").await;
    let (outsider_sync, outsider_folder) = invite_and_join(
        &mut owner,
        &mut sync,
        &mut book,
        &mut outsider,
        Role::Editor,
    )
    .await;
    settle(&mut sync, &mut owner).await;
    let domain = sync
        .restrict_folder(
            &mut owner.service,
            "Pay",
            &[Access {
                account: grantee.credentials.account,
                level: Level::Manage,
            }],
        )
        .await
        .expect("restricted");
    folder.write("Pay/Salaries.md", b"secret\n");
    settle(&mut sync, &mut owner).await;
    Team {
        owner,
        sync,
        folder,
        grantee,
        grantee_sync,
        grantee_folder,
        outsider,
        outsider_sync,
        outsider_folder,
        domain,
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn every_response_and_frame_to_an_outside_member_leaves_the_domain_out() {
    outside_member_receives_nothing(Server::start().await).await;
}

#[tokio::test]
async fn every_response_and_frame_to_an_outside_member_leaves_the_domain_out_on_postgres() {
    outside_member_receives_nothing(Server::start_postgres().await).await;
}

async fn outside_member_receives_nothing<B: Backend>(server: Server<B>) {
    let mut team = team(&server).await;
    let proxy = Proxy::start(&server.base).await;
    let mut through = team.outsider.connect_through(&proxy.base).await;
    let mut channel = Channel::open(through.client(), team.outsider_sync.workspace())
        .await
        .expect("the channel opens");

    // More of the domain while the member watches.
    team.folder.write("Pay/Bonus.md", b"more secret\n");
    team.folder.write("Open.md", b"open\n");
    settle(&mut team.sync, &mut team.owner).await;
    for _ in 0..3 {
        team.outsider_sync
            .round(&mut through, 0)
            .await
            .expect("a round");
    }
    while let Ok(Ok(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(400), channel.next()).await
    {}

    assert!(team.outsider_folder.files().contains_key("Open.md"));
    let received = proxy.received();
    assert!(received.iter().any(|(path, _)| path == "frame"));
    assert!(received.iter().any(|(path, _)| path.ends_with("/pull")));
    // The policy names every domain for every member (§8), which also
    // shows that the bytes this test looks for appear as they are.
    assert!(
        received
            .iter()
            .any(|(path, body)| path.ends_with("/policy") && contains(body, &team.domain.0)),
        "the outsider's device read the policy through the proxy"
    );
    for (path, body) in &received {
        if path.ends_with("/policy") {
            continue;
        }
        assert!(
            !contains(body, &team.domain.0),
            "{path} carries the restricted domain's ID"
        );
    }
}

/// The note in the restricted folder opens on the grantee's and owner's
/// desktops, and presence reaches only the grantee, whatever channels the
/// outside member subscribes to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_outside_a_restricted_domain_receives_no_presence_for_it() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    const NOTE: &str = "Pay/Salaries.md";
    for _ in 0..3 {
        settle(&mut team.grantee_sync, &mut team.grantee).await;
        settle(&mut team.sync, &mut team.owner).await;
    }
    let (_, ours) = team.sync.open_text(NOTE).expect("the note opens");
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let (_, theirs) = team.grantee_sync.open_text(NOTE).expect("the note opens");
    assert!(ours.is_some() && theirs.is_some());
    team.grantee_sync.presence_connected().unwrap();
    team.sync.presence_connected().unwrap();

    let mut owner = Channel::open(team.owner.service.client(), team.sync.workspace())
        .await
        .unwrap();
    let mut grantee = Channel::open(team.grantee.service.client(), team.sync.workspace())
        .await
        .unwrap();
    let mut outsider = Channel::open(team.outsider.service.client(), team.sync.workspace())
        .await
        .unwrap();
    let subscribe = team.sync.presence_frames().unwrap();
    for frame in &subscribe {
        owner.send(frame).await.unwrap();
    }
    for frame in team.grantee_sync.presence_frames().unwrap() {
        grantee.send(&frame).await.unwrap();
    }
    // The outside member names the same channel.
    outsider.send(&subscribe[0]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    team.sync.set_selections(
        NOTE,
        vec![app_core::sync::workspace::presence::SelectionRange { anchor: 1, head: 1 }],
        true,
    );
    let frames = team.sync.presence_frames().unwrap();
    let record = frames.last().expect("a record");
    owner.send(record).await.unwrap();
    let wait = std::time::Duration::from_millis(600);
    let mut reached = false;
    while let Ok(Ok(frame)) = tokio::time::timeout(wait, grantee.next()).await {
        reached |= matches!(frame, sync_protocol::messages::Frame::Presence { .. });
    }
    assert!(reached, "the grantee hears it");
    while let Ok(Ok(frame)) = tokio::time::timeout(wait, outsider.next()).await {
        assert!(
            !matches!(frame, sync_protocol::messages::Frame::Presence { .. }),
            "the outside member heard it"
        );
    }
}

#[tokio::test]
async fn a_device_that_downloads_applies_the_marker_first_and_holds_new_files() {
    let server = Server::start_with_interval(4).await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    sync.set_checkpoint_wait(std::time::Duration::ZERO);
    let domain = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    for n in 0..8 {
        folder.write(&format!("Pay/File {n}.md"), b"secret\n");
        settle(&mut sync, &mut desktop).await;
    }

    let mut b = Folder::new("Second copy");
    let mut second = WorkspaceSync::download(
        &mut desktop.service,
        b.engine.clone(),
        sync.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    // A file made before the markers arrive waits for them.
    b.write("Pay/Early.md", b"early\n");
    second
        .round(&mut desktop.service, 0)
        .await
        .expect("the first round");
    assert_eq!(second.domain_roots().len(), 1);
    assert_eq!(second.domain_roots()[0].domain, domain);
    assert!(b.path(&format!("Pay/{MARKER}")).exists());
    assert!(b.files().contains_key("Pay/File 7.md"));
    settle(&mut second, &mut desktop).await;
    let early = second.rid_at("Pay/Early.md").expect("captured");
    assert_eq!(second.object_domain(&early), Some(domain));
    assert_eq!(second.status().held_roots, 0);
}

#[tokio::test]
async fn a_received_delete_of_a_marker_keeps_the_root_and_holds_its_files() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let marker = team.sync.domain_roots()[0].rid;

    team.grantee_sync.sign_delete_of(marker).expect("signed");
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.sync, &mut team.owner).await;
    assert!(team.folder.path(&format!("Pay/{MARKER}")).exists());
    assert_eq!(team.sync.domain_roots().len(), 1, "the root stays");
    assert_eq!(team.sync.status().held_roots, 1);
    assert_eq!(team.sync.refused_objects(), [marker]);

    // A new file there uploads to no domain while the hold stands.
    team.folder.write("Pay/New.md", b"new\n");
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.rid_at("Pay/New.md"), None);

    // Restoring the marker from synced state ends the hold.
    assert_eq!(team.sync.restorable_domain("Pay"), Some(team.domain));
    team.sync
        .restore_domain_marker(team.domain)
        .expect("restored");
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.status().held_roots, 0);
    assert_eq!(team.sync.refused_objects(), []);
    settle(&mut team.sync, &mut team.owner).await;
    let new = team.sync.rid_at("Pay/New.md").expect("captured");
    assert_eq!(team.sync.object_domain(&new), Some(team.domain));
}

#[tokio::test]
async fn a_marker_move_off_its_name_holds_and_a_bootstrap_holds_each_folder() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let marker = team.sync.domain_roots()[0].rid;
    team.grantee_sync
        .sign_state_at(marker, "Pay/notes.md", b"not a marker\n")
        .expect("signed");
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.status().held_roots, 1);
    assert!(!team.folder.files().contains_key("Pay/notes.md"));

    // A new device meets the marker in that state, records a held root
    // where the domain's files sit, and writes them.
    let mut b = Folder::new("Second copy");
    let mut second = WorkspaceSync::download(
        &mut team.owner.service,
        b.engine.clone(),
        team.sync.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    settle(&mut second, &mut team.owner).await;
    assert!(second.domain_roots().is_empty());
    let held: Vec<String> = second.held_roots().into_iter().map(|h| h.0).collect();
    assert_eq!(held, ["Pay"]);
    assert!(b.files().contains_key("Pay/Salaries.md"));
    b.write("Pay/Local.md", b"local\n");
    settle(&mut second, &mut team.owner).await;
    assert_eq!(second.rid_at("Pay/Local.md"), None);
}

#[tokio::test]
async fn an_object_of_another_domain_goes_under_shared_and_keeps_its_synced_path() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.outsider_sync, &mut team.outsider).await;
    team.outsider_folder
        .write("Pay/Theirs.md", b"from outside\n");
    settle(&mut team.outsider_sync, &mut team.outsider).await;

    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(
        team.folder
            .files()
            .get("Pay (shared)/Theirs.md")
            .map(Vec::as_slice),
        Some(&b"from outside\n"[..])
    );
    assert!(!team.folder.files().contains_key("Pay/Theirs.md"));

    team.folder.write("Pay (shared)/Theirs.md", b"edited\n");
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut team.outsider_sync, &mut team.outsider).await;
    assert_eq!(
        team.outsider_folder
            .files()
            .get("Pay/Theirs.md")
            .map(Vec::as_slice),
        Some(&b"edited\n"[..])
    );
    assert!(team.folder.files().contains_key("Pay (shared)/Theirs.md"));
}

#[tokio::test]
async fn a_path_that_leaves_its_root_stays_for_review() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let rid = team.grantee_sync.rid_at("Pay/Salaries.md").expect("synced");
    team.grantee_sync
        .sign_state_at(rid, "Elsewhere/Salaries.md", b"secret\n")
        .expect("signed");
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.sync, &mut team.owner).await;
    let files = team.folder.files();
    assert!(files.contains_key("Pay/Salaries.md"));
    assert!(!files.contains_key("Elsewhere/Salaries.md"));
    assert_eq!(team.sync.status().held_roots, 0);
}

#[tokio::test]
async fn a_folder_rebuilt_without_its_journal_matches_a_restricted_domain() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    drop(team.grantee_sync);
    let engine = team.grantee_folder.engine.clone();
    let rebuilt = WorkspaceSync::rebuild(
        &mut team.grantee.service,
        engine,
        &mut team.grantee_folder.registry,
    )
    .await
    .expect("rebuilt");
    assert_eq!(rebuilt.domain_roots().len(), 1);
    assert_eq!(rebuilt.status().held_roots, 0);
    assert_eq!(rebuilt.status().held, 0);
    assert!(rebuilt.rid_at("Pay/Salaries.md").is_some());
}

#[tokio::test]
async fn a_device_keeps_its_domains_across_a_restart() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    let domain = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    drop(sync);

    let mut sync = WorkspaceSync::open(
        folder.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &folder.registry,
    )
    .expect("the journal opens");
    assert_eq!(sync.domain_roots()[0].domain, domain);
    folder.write("Pay/After.md", b"after\n");
    settle(&mut sync, &mut desktop).await;
    let rid = sync.rid_at("Pay/After.md").expect("captured");
    assert_eq!(sync.object_domain(&rid), Some(domain));
    assert_eq!(sync.status().pending, 0);
    assert_eq!(sync.status().held_roots, 0);
}

#[tokio::test]
async fn a_marker_moved_alone_over_other_files_holds_its_root() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    folder.write("Docs/Open.md", b"open\n");
    settle(&mut sync, &mut desktop).await;
    let domain = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    let open = sync.rid_at("Docs/Open.md").expect("captured");

    std::fs::rename(
        folder.path(&format!("Pay/{MARKER}")),
        folder.path(&format!("Docs/{MARKER}")),
    )
    .unwrap();
    folder.write("Pay/New.md", b"new\n");
    settle(&mut sync, &mut desktop).await;
    assert_eq!(sync.domain_roots()[0].folder, "Pay");
    assert_eq!(sync.domain_roots()[0].domain, domain);
    assert_eq!(sync.status().held_roots, 1);
    assert_eq!(sync.rid_at("Pay/New.md"), None, "it waits for review");
    assert_eq!(sync.object_domain(&open), None);
}

#[tokio::test]
async fn a_received_marker_move_over_other_files_holds_its_root() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    team.folder.write("Docs/Open.md", b"open\n");
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let root = team.sync.domain_roots()[0].clone();

    team.grantee_sync
        .sign_state_at(
            root.rid,
            &format!("Docs/{MARKER}"),
            marker_content(&team.domain).as_bytes(),
        )
        .expect("signed");
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.domain_roots(), [root]);
    assert_eq!(team.sync.status().held_roots, 1);
    team.folder.write("Pay/New.md", b"new\n");
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.rid_at("Pay/New.md"), None);
}

#[tokio::test]
async fn a_later_marker_inside_an_earlier_root_gives_way() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    let earlier = sync
        .restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    let later = sync
        .restrict_folder(&mut desktop.service, "Legal", &[])
        .await
        .expect("restricted");
    let rid = sync
        .domain_roots()
        .iter()
        .find(|root| root.domain == later)
        .expect("recorded")
        .rid;

    // Another of this account's devices signs the later marker inside
    // the earlier root.
    sync.sign_state_at(
        rid,
        &format!("Pay/Inner/{MARKER}"),
        marker_content(&later).as_bytes(),
    )
    .expect("signed");
    settle(&mut sync, &mut desktop).await;
    let roots: Vec<_> = sync
        .domain_roots()
        .iter()
        .map(|root| (root.folder.as_str(), root.domain))
        .collect();
    assert_eq!(roots, [("Legal", later), ("Pay", earlier)]);
    assert!(!folder.path(&format!("Pay/Inner/{MARKER}")).exists());
    assert_eq!(sync.status().held_roots, 1, "the later domain's files wait");
    folder.write("Pay/Inner/Salaries.md", b"secret\n");
    settle(&mut sync, &mut desktop).await;
    let inside = sync.rid_at("Pay/Inner/Salaries.md").expect("captured");
    assert_eq!(sync.object_domain(&inside), Some(earlier));
}

#[tokio::test]
async fn a_later_root_may_hold_an_earlier_one() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    let inner = sync
        .restrict_folder(&mut desktop.service, "Team/Pay", &[])
        .await
        .expect("restricted");
    let outer = sync
        .restrict_folder(&mut desktop.service, "Team", &[])
        .await
        .expect("a later root holds the earlier one");
    folder.write("Team/Pay/Salaries.md", b"secret\n");
    folder.write("Team/Plan.md", b"plan\n");
    settle(&mut sync, &mut desktop).await;
    let salaries = sync.rid_at("Team/Pay/Salaries.md").expect("captured");
    let plan = sync.rid_at("Team/Plan.md").expect("captured");
    assert_eq!(sync.object_domain(&salaries), Some(inner));
    assert_eq!(sync.object_domain(&plan), Some(outer));
    assert_eq!(sync.status().held_roots, 0);
}

#[tokio::test]
async fn an_object_at_a_marker_path_that_no_domain_names_is_refused() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.outsider_sync, &mut team.outsider).await;
    let plan = team.outsider_sync.rid_at("Plan.md").expect("synced");
    team.outsider_sync
        .sign_state_at(
            plan,
            &format!("Docs/{MARKER}"),
            marker_content(&team.domain).as_bytes(),
        )
        .expect("signed");
    settle(&mut team.outsider_sync, &mut team.outsider).await;

    settle(&mut team.sync, &mut team.owner).await;
    assert!(!team.folder.path(&format!("Docs/{MARKER}")).exists());
    assert_eq!(team.sync.refused_objects(), [plan]);
    assert_eq!(team.sync.domain_roots().len(), 1);
    assert_eq!(team.sync.status().held_roots, 0);
}

/// How a test breaks a domain's marker before a device downloads it.
#[derive(Clone, Copy)]
enum Broken {
    Moved,
    Deleted,
    Conflicted,
}

async fn a_download_over_a_broken_marker_holds_each_folder(broken: Broken) {
    let server = Server::start().await;
    let mut team = team(&server).await;
    team.folder.write("Pay/Deep/Rates.md", b"rates\n");
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let marker = team.sync.domain_roots()[0].rid;
    match broken {
        Broken::Moved => team
            .grantee_sync
            .sign_state_at(marker, "Pay/notes.md", b"not a marker\n")
            .expect("signed"),
        Broken::Deleted => team.grantee_sync.sign_delete_of(marker).expect("signed"),
        Broken::Conflicted => {
            // Two states, neither of which saw the other.
            let path = format!("Pay/{MARKER}");
            let bytes = marker_content(&team.domain).into_bytes();
            team.grantee_sync
                .sign_state_at(marker, &path, &bytes)
                .expect("signed");
            team.sync
                .sign_state_at(marker, &path, &bytes)
                .expect("signed");
        }
    }
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.status().held_roots, 1);

    let mut b = Folder::new("Second copy");
    let mut second = WorkspaceSync::download(
        &mut team.owner.service,
        b.engine.clone(),
        team.sync.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    settle(&mut second, &mut team.owner).await;
    assert!(second.domain_roots().is_empty());
    let mut held: Vec<String> = second.held_roots().into_iter().map(|h| h.0).collect();
    held.sort();
    assert_eq!(held, ["Pay", "Pay/Deep"]);
    assert!(b.files().contains_key("Pay/Deep/Rates.md"));
    b.write("Pay/Local.md", b"local\n");
    settle(&mut second, &mut team.owner).await;
    assert_eq!(second.rid_at("Pay/Local.md"), None);

    // The hold ends only through the held domain's own valid root, and the
    // files beneath it are matched before anything uploads.
    team.sync
        .restore_domain_marker(team.domain)
        .expect("restored");
    settle(&mut team.sync, &mut team.owner).await;
    assert_eq!(team.sync.status().held_roots, 0);
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut second, &mut team.owner).await;
    assert_eq!(second.status().held_roots, 0);
    assert_eq!(second.domain_roots().len(), 1);
    assert!(b.path(&format!("Pay/{MARKER}")).exists());
    let local = second.rid_at("Pay/Local.md").expect("captured");
    assert_eq!(second.object_domain(&local), Some(team.domain));
    assert_eq!(second.status().pending, 0);
    assert_eq!(second.status().held, 0);
}

#[tokio::test]
async fn a_download_over_a_moved_marker_holds_each_folder_until_it_is_restored() {
    a_download_over_a_broken_marker_holds_each_folder(Broken::Moved).await;
}

#[tokio::test]
async fn a_download_over_a_deleted_marker_holds_each_folder_until_it_is_restored() {
    a_download_over_a_broken_marker_holds_each_folder(Broken::Deleted).await;
}

#[tokio::test]
async fn a_download_over_a_conflicted_marker_holds_each_folder_until_it_is_restored() {
    a_download_over_a_broken_marker_holds_each_folder(Broken::Conflicted).await;
}

#[tokio::test]
async fn a_local_marker_of_a_domain_the_device_cant_read_holds_its_folder() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.outsider_sync, &mut team.outsider).await;

    // A copied folder brings the marker and a file the outsider can't place.
    team.outsider_folder
        .write("Copied/Salaries.md", b"secret\n");
    team.outsider_folder.write("Copied/Other/Pay.md", b"more\n");
    std::fs::write(
        team.outsider_folder.path(&format!("Copied/{MARKER}")),
        marker_content(&team.domain),
    )
    .unwrap();
    team.outsider_folder.write("Scrap/Note.md", b"note\n");
    std::fs::write(
        team.outsider_folder.path(&format!("Scrap/{MARKER}")),
        b"not a marker\n",
    )
    .unwrap();
    drop(team.outsider_sync);
    let engine = team.outsider_folder.engine.clone();
    let mut rebuilt = WorkspaceSync::rebuild(
        &mut team.outsider.service,
        engine,
        &mut team.outsider_folder.registry,
    )
    .await
    .expect("rebuilt");
    settle(&mut rebuilt, &mut team.outsider).await;

    assert!(rebuilt.domain_roots().is_empty());
    let mut held: Vec<String> = rebuilt.held_roots().into_iter().map(|h| h.0).collect();
    held.sort();
    assert_eq!(held, ["Copied", "Scrap"]);
    assert_eq!(rebuilt.rid_at("Copied/Salaries.md"), None);
    assert_eq!(rebuilt.rid_at("Copied/Other/Pay.md"), None);
    assert_eq!(rebuilt.rid_at("Scrap/Note.md"), None);
    assert_eq!(rebuilt.status().pending, 0);
}

#[tokio::test]
async fn a_folder_rebuilt_while_its_marker_is_invalid_matches_once_it_is_restored() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let marker = team.sync.domain_roots()[0].rid;
    team.sync.sign_delete_of(marker).expect("signed");
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    assert_eq!(team.grantee_sync.status().held_roots, 1);

    // Without a journal, one file differs and another is new.
    let salaries = team.grantee_sync.rid_at("Pay/Salaries.md").expect("synced");
    drop(team.grantee_sync);
    team.grantee_folder.write("Pay/Salaries.md", b"mine\n");
    team.grantee_folder.write("Pay/Fresh.md", b"fresh\n");
    let engine = team.grantee_folder.engine.clone();
    let mut rebuilt = WorkspaceSync::rebuild(
        &mut team.grantee.service,
        engine,
        &mut team.grantee_folder.registry,
    )
    .await
    .expect("rebuilt");
    settle(&mut rebuilt, &mut team.grantee).await;
    assert!(rebuilt.domain_roots().is_empty());
    assert_eq!(rebuilt.status().held_roots, 1);
    assert_eq!(rebuilt.rid_at("Pay/Fresh.md"), None);
    let on_disk = |folder: &Folder| folder.files().get("Pay/Salaries.md").cloned();
    assert_eq!(
        on_disk(&team.grantee_folder).as_deref(),
        Some(&b"mine\n"[..]),
        "a hold writes nothing over your file"
    );

    team.sync
        .restore_domain_marker(team.domain)
        .expect("restored");
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut rebuilt, &mut team.grantee).await;
    assert_eq!(rebuilt.status().held_roots, 0);
    assert_eq!(rebuilt.domain_roots().len(), 1);
    assert_eq!(rebuilt.rid_at("Pay/Salaries.md"), Some(salaries));
    assert_eq!(rebuilt.status().held, 1, "the differing file waits for you");
    assert_eq!(rebuilt.status().pending, 0);
    let fresh = rebuilt.rid_at("Pay/Fresh.md").expect("captured");
    assert_eq!(rebuilt.object_domain(&fresh), Some(team.domain));
    assert_eq!(
        on_disk(&team.grantee_folder).as_deref(),
        Some(&b"mine\n"[..])
    );
}

#[tokio::test]
async fn a_placeholder_of_a_valid_marker_holds_nothing_after_a_lost_journal() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    drop(team.grantee_sync);
    let folder = &team.grantee_folder;
    std::fs::remove_file(folder.path(&format!("Pay/{MARKER}"))).unwrap();
    std::fs::write(folder.path("Pay/..noura-domain.icloud"), b"").unwrap();
    // A placeholder elsewhere names no domain this device can read.
    folder.write("Other/Note.md", b"note\n");
    std::fs::write(folder.path("Other/.noura-domain.icloud"), b"").unwrap();

    let engine = team.grantee_folder.engine.clone();
    let mut rebuilt = WorkspaceSync::rebuild(
        &mut team.grantee.service,
        engine,
        &mut team.grantee_folder.registry,
    )
    .await
    .expect("rebuilt");
    settle(&mut rebuilt, &mut team.grantee).await;
    assert_eq!(rebuilt.domain_roots().len(), 1);
    let held: Vec<String> = rebuilt.held_roots().into_iter().map(|h| h.0).collect();
    assert_eq!(held, ["Other"]);
    assert!(rebuilt.rid_at("Pay/Salaries.md").is_some());
    assert_eq!(rebuilt.rid_at("Other/Note.md"), None);
}

#[tokio::test]
async fn a_text_mode_note_that_moves_to_another_domain_leaves_text_mode_and_enters_again() {
    let server = Server::start().await;
    let (mut desktop, mut sync, folder) = owner(&server).await;
    sync.restrict_folder(&mut desktop.service, "Pay", &[])
        .await
        .expect("restricted");
    settle(&mut sync, &mut desktop).await;
    let (_, document) = sync.open_text("Plan.md").expect("the note opens");
    let mut document = document.expect("the document");
    let rid = sync.rid_at("Plan.md").unwrap();
    document.edit(
        vec![Utf16Splice {
            start: 6,
            removed: 0,
            inserted: " kept".into(),
        }],
        true,
    );
    settle(&mut sync, &mut desktop).await;
    assert_eq!(folder.files()["Plan.md"], b"# Plan kept\n");

    std::fs::create_dir_all(folder.path("Pay")).unwrap();
    std::fs::rename(folder.path("Plan.md"), folder.path("Pay/Plan.md")).unwrap();
    settle(&mut sync, &mut desktop).await;
    assert!(!sync.in_text_mode(&rid), "the old object left text mode");
    assert_eq!(sync.status().pending, 0, "the server took the move");
    let moved = sync.rid_at("Pay/Plan.md").expect("the new object");
    assert_ne!(moved, rid);
    assert!(!sync.in_text_mode(&moved));
    let left = sync.take_left_notes();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].why, LeftWhy::Ended);

    let (entered, document) = sync.open_text("Pay/Plan.md").expect("the note opens");
    assert_eq!(
        entered,
        Entered::Yes,
        "the next edit enters text mode again"
    );
    let mut document = document.expect("the document");
    assert!(sync.in_text_mode(&moved));
    document.edit(
        vec![Utf16Splice {
            start: 0,
            removed: 0,
            inserted: "A ".into(),
        }],
        true,
    );
    settle(&mut sync, &mut desktop).await;
    assert_eq!(folder.files()["Pay/Plan.md"], b"A # Plan kept\n");
    assert_eq!(sync.status().pending, 0);
}

#[tokio::test]
async fn a_move_that_waits_for_review_keeps_a_notes_text_mode() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    let (_, document) = team
        .sync
        .open_text("Pay/Salaries.md")
        .expect("the note opens");
    let document = document.expect("the document");
    let rid = team.sync.rid_at("Pay/Salaries.md").unwrap();
    settle(&mut team.sync, &mut team.owner).await;

    // Moving it out reaches more people, so it waits for your word, while
    // another change in the same walk signs a record.
    team.folder.write("Elsewhere/Note.md", b"other\n");
    team.folder.write("Open.md", b"open\n");
    std::fs::rename(
        team.folder.path("Pay/Salaries.md"),
        team.folder.path("Salaries.md"),
    )
    .unwrap();
    team.sync
        .round(&mut team.owner.service, 0)
        .await
        .expect("a round");
    assert!(team.sync.in_text_mode(&rid), "the lineage stays");
    assert!(team.sync.text_file(&rid).unwrap().is_some());
    assert_eq!(team.sync.moves_for_review().len(), 1);
    drop(document);
}

#[tokio::test]
async fn a_merged_path_outside_the_root_goes_to_review_until_you_keep_the_note() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let path = "Pay/Salaries.md";
    let (_, document) = team.sync.open_text(path).expect("the note opens");
    assert!(document.is_some());
    settle(&mut team.sync, &mut team.owner).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    let (_, second) = team.grantee_sync.open_text(path).expect("the note opens");
    assert!(second.is_some());
    let rid = team.sync.rid_at(path).unwrap();

    team.grantee_sync
        .sign_text_path(path, "Elsewhere/Salaries.md")
        .expect("signed");
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.sync, &mut team.owner).await;
    let reviews = team.sync.text_reviews();
    assert!(
        reviews
            .iter()
            .any(|review| review.kind == TextReviewKind::OutsideRoot),
        "{reviews:?}"
    );
    assert_eq!(team.sync.outside_notes(), [(rid, path.to_owned())]);
    assert_eq!(team.folder.files()[path], b"secret\n", "the file stays");

    team.sync.keep_last_good(rid).expect("kept");
    settle(&mut team.sync, &mut team.owner).await;
    assert!(!team.sync.in_text_mode(&rid));
    assert!(team.sync.outside_notes().is_empty());
}

/// Set up sync again starts the workspace over (§14). The owner's folder
/// holds what an old marker covers, so nothing restricted lands in the new
/// workspace domain, and each member connects the folder it synced before:
/// it holds the files under an old marker and the ones that differ from the
/// new workspace's live state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_connect_their_folders_to_a_workspace_set_up_again() {
    members_connect_their_folders(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_connect_their_folders_on_postgres() {
    members_connect_their_folders(Server::start_postgres().await).await;
}

async fn members_connect_their_folders<B: Backend>(server: Server<B>) {
    let mut team = team(&server).await;
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    settle(&mut team.outsider_sync, &mut team.outsider).await;
    assert!(team.grantee_folder.path("Pay/Salaries.md").exists());
    let old = team.sync.workspace();

    // The grantee edits a file that never reaches the old workspace.
    team.grantee_folder.write("Plan.md", b"# Edited offline\n");

    let mut fresh = team
        .sync
        .set_up_again(&mut team.owner.service, &mut team.folder.registry)
        .await
        .expect("set up");
    settle(&mut fresh, &mut team.owner).await;
    let held: Vec<String> = fresh.held_roots().into_iter().map(|h| h.0).collect();
    assert_eq!(held, ["Pay"], "the owner's old root holds its files");
    assert_eq!(fresh.rid_at("Pay/Salaries.md"), None);
    assert!(fresh.rid_at("Plan.md").is_some());

    let mut book = InvitationBook::open(team.owner.state.path(), team.owner.id()).unwrap();
    let workspace = invite(
        &mut team.owner,
        &mut fresh,
        &mut book,
        &mut team.grantee,
        Role::Editor,
    )
    .await;
    assert_ne!(workspace, old);
    settle(&mut team.grantee_sync, &mut team.grantee).await;
    assert_eq!(
        team.grantee_sync.restore_lost(),
        Some(&app_core::sync::workspace::restore::Lost::Closed)
    );
    drop(team.grantee_sync);
    let mut grantee = WorkspaceSync::rejoin(
        &mut team.grantee.service,
        team.grantee_folder.engine.clone(),
        workspace,
        &mut team.grantee_folder.registry,
    )
    .await
    .expect("the folder connects");
    settle(&mut grantee, &mut team.grantee).await;
    assert_eq!(grantee.workspace(), workspace);
    let held: Vec<String> = grantee.held_roots().into_iter().map(|h| h.0).collect();
    assert_eq!(held, ["Pay"]);
    assert_eq!(grantee.rid_at("Pay/Salaries.md"), None);
    assert_eq!(grantee.status().held, 1, "the edited plan waits for review");
    assert_eq!(
        std::fs::read(team.grantee_folder.path("Plan.md")).unwrap(),
        b"# Edited offline\n"
    );
    assert_eq!(
        std::fs::read(team.grantee_folder.path("Pay/Salaries.md")).unwrap(),
        b"secret\n"
    );
    assert_eq!(grantee.status().pending, 0);

    // The other member's files match, so it holds nothing.
    let workspace = invite(
        &mut team.owner,
        &mut fresh,
        &mut book,
        &mut team.outsider,
        Role::Editor,
    )
    .await;
    drop(team.outsider_sync);
    let mut outsider = WorkspaceSync::rejoin(
        &mut team.outsider.service,
        team.outsider_folder.engine.clone(),
        workspace,
        &mut team.outsider_folder.registry,
    )
    .await
    .expect("the folder connects");
    settle(&mut outsider, &mut team.outsider).await;
    assert_eq!(outsider.status().held, 0);
    assert_eq!(outsider.status().held_roots, 0);
    assert!(outsider.rid_at("Plan.md").is_some());

    // Nothing held reached the owner.
    settle(&mut fresh, &mut team.owner).await;
    assert_eq!(
        std::fs::read(team.folder.path("Plan.md")).unwrap(),
        b"# Plan\n"
    );
}

/// A member can't move a folder off a workspace that's still open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_of_an_open_workspace_stays_with_it() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    let other = team
        .owner
        .service
        .create_workspace("Another")
        .await
        .expect("created");
    let refused = WorkspaceSync::rejoin(
        &mut team.owner.service,
        team.folder.engine.clone(),
        other.workspace,
        &mut team.folder.registry,
    )
    .await;
    assert!(matches!(refused, Err(SyncError::Invalid(_))));
    assert!(
        team.folder
            .registry
            .of_folder(team.folder.engine.root())
            .is_some_and(|entry| entry.workspace == team.sync.workspace())
    );
}
