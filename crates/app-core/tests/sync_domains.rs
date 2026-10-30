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
use sync_protocol::matching::MARKER;
use sync_protocol::records::{Access, Level, Role, marker_content};
use sync_store::Backend;
use sync_support::{Desktop, Folder, Proxy, Server, first_desktop, invite_and_join};

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
    let (mut grantee_sync, grantee_folder) =
        invite_and_join(&mut desktop, &mut sync, &mut book, &mut grantee, Role::Editor).await;
    let mut outsider = first_desktop(server, "cy@example.com").await;
    let (mut outsider_sync, outsider_folder) =
        invite_and_join(&mut desktop, &mut sync, &mut book, &mut outsider, Role::Editor).await;
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
    let (outsider_sync, outsider_folder) =
        invite_and_join(&mut owner, &mut sync, &mut book, &mut outsider, Role::Editor).await;
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
    let server = Server::start().await;
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
    for (path, body) in &received {
        // The policy names every domain for every member (§8).
        if path.ends_with("/policy") {
            continue;
        }
        assert!(
            !contains(body, &team.domain.0),
            "{path} carries the restricted domain's ID"
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
    team.outsider_folder.write("Pay/Theirs.md", b"from outside\n");
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
