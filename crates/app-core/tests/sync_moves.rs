//! Moves between domains and who reads a restricted folder, end to end
//! (seam 4, sync-v2.md §8, §9): a file that crosses a root gets a new
//! `rid`, an `origin` and a reason-2 delete in one push; a file that would
//! reach more people waits for you; restricting a folder moves what it
//! holds; `set_access` and `delete_domain` take effect on every device.

mod sync_support;

use std::time::Duration;

use app_core::sync::SyncError;
use app_core::sync::invitations::InvitationBook;
use app_core::sync::workspace::{SkipWhy, WorkspaceSync};
use sync_protocol::codec::Decode;
use sync_protocol::matching::MARKER;
use sync_protocol::messages::{PushRequest, RecordRef};
use sync_protocol::records::{Access, Level, Role};
use sync_protocol::types::Id;
use sync_store::Backend;
use sync_support::{Desktop, Folder, Proxy, Server, first_desktop, invite_and_join};

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
}

impl Team {
    async fn settle(&mut self) {
        for _ in 0..3 {
            self.sync.round(&mut self.owner.service, 0).await.expect("a round");
            self.grantee_sync
                .round(&mut self.grantee.service, 0)
                .await
                .expect("a round");
            self.outsider_sync
                .round(&mut self.outsider.service, 0)
                .await
                .expect("a round");
        }
    }

    fn grantee_account(&self) -> Id {
        self.grantee.credentials.account
    }

    fn outsider_account(&self) -> Id {
        self.outsider.credentials.account
    }

    /// Restricts `Pay` to the owner and the grantee, at `level`.
    async fn restrict(&mut self, level: Level) -> Id {
        let account = self.grantee_account();
        let domain = self
            .sync
            .restrict_folder(
                &mut self.owner.service,
                "Pay",
                &[Access { account, level }],
            )
            .await
            .expect("restricted");
        self.settle().await;
        domain
    }
}

async fn team<B: Backend>(server: &Server<B>, base: Option<&str>) -> Team {
    let _ = base;
    let mut owner = first_desktop(server, "ana@example.com").await;
    let mut folder = Folder::new("Field notes");
    folder.write("Plan.md", b"# Plan\n");
    let mut sync = WorkspaceSync::turn_on(
        &mut owner.service,
        folder.engine.clone(),
        "Field notes",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.round(&mut owner.service, 0).await.expect("a round");
    let mut book = InvitationBook::open(owner.state.path(), owner.id()).unwrap();
    let mut grantee = first_desktop(server, "ben@example.com").await;
    let (grantee_sync, grantee_folder) =
        invite_and_join(&mut owner, &mut sync, &mut book, &mut grantee, Role::Editor).await;
    let mut outsider = first_desktop(server, "cy@example.com").await;
    let (outsider_sync, outsider_folder) =
        invite_and_join(&mut owner, &mut sync, &mut book, &mut outsider, Role::Editor).await;
    let mut team = Team {
        owner,
        sync,
        folder,
        grantee,
        grantee_sync,
        grantee_folder,
        outsider,
        outsider_sync,
        outsider_folder,
    };
    team.grantee_sync.set_rotation_wait(Duration::ZERO);
    team.outsider_sync.set_rotation_wait(Duration::ZERO);
    team.settle().await;
    team
}

fn rename(folder: &Folder, from: &str, to: &str) {
    std::fs::create_dir_all(folder.path(to).parent().unwrap()).unwrap();
    std::fs::rename(folder.path(from), folder.path(to)).unwrap();
}

#[tokio::test]
async fn a_file_moved_into_a_restricted_folder_writes_both_records_in_one_push() {
    let (server, proxy) = Proxy::in_front().await;
    let mut team = team(&server, None).await;
    let mut proxied = team.owner.connect_through(&proxy.base).await;
    let domain = team.restrict(Level::Write).await;
    let old = team.sync.rid_at("Plan.md").expect("mapped");
    team.settle().await;

    rename(&team.folder, "Plan.md", "Pay/Plan.md");
    team.sync.round(&mut proxied, 0).await.expect("a round");

    let moved = team.sync.rid_at("Pay/Plan.md").expect("the new object");
    assert_ne!(moved, old, "a move across domains gets a new rid");
    assert_eq!(team.sync.object_domain(&moved), Some(domain));
    assert_eq!(team.sync.moved_in().len(), 1, "the app warns about the log");
    assert_eq!(team.sync.status().pending, 0);

    let pushes: Vec<Vec<RecordRef>> = proxy
        .requests()
        .iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/ops"))
        .map(|r| PushRequest::from_bytes(&r.body).expect("a push").records.0)
        .collect();
    let domains_in = |records: &Vec<RecordRef>| {
        records
            .iter()
            .filter_map(|r| match r {
                RecordRef::Operation(op) => Some(op.body.domain),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let pair = pushes
        .iter()
        .find(|records| domains_in(records).len() == 2)
        .expect("one push spans the two domains");
    assert_eq!(pair.len(), 2);

    team.settle().await;
    assert_eq!(
        team.grantee_folder.files().get("Pay/Plan.md").map(Vec::as_slice),
        Some(&b"# Plan\n"[..]),
        "a reader of both keeps one file"
    );
    assert!(!team.grantee_folder.files().contains_key("Plan.md"));
    assert!(
        !team.outsider_folder.files().contains_key("Plan.md"),
        "a device that can't read the target applies the delete at once"
    );
    let granted = team.grantee_sync.rid_at("Pay/Plan.md").expect("mapped");
    assert_eq!(granted, moved);
    assert_eq!(team.grantee_sync.status().conflicts, 0);
}

#[tokio::test]
async fn a_file_leaving_a_restricted_folder_waits_for_review_with_the_count() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Salaries.md", b"secret\n");
    team.settle().await;
    let rid = team.sync.rid_at("Pay/Salaries.md").expect("mapped");
    assert_eq!(team.sync.object_domain(&rid), Some(domain));

    rename(&team.folder, "Pay/Salaries.md", "Salaries.md");
    team.settle().await;
    let reviews = team.sync.moves_for_review().to_vec();
    assert_eq!(reviews.len(), 1, "it waits");
    assert_eq!(reviews[0].people, 1, "the member outside the folder");
    assert_eq!(reviews[0].target, None);
    assert!(
        team.sync
            .status()
            .skipped
            .iter()
            .any(|s| s.path == "Salaries.md" && s.why == SkipWhy::CrossesRoot)
    );
    assert!(
        !team.outsider_folder.files().contains_key("Salaries.md"),
        "nothing reached the wider audience"
    );

    team.sync.confirm_move(reviews[0].rid);
    team.settle().await;
    assert!(team.sync.moves_for_review().is_empty());
    assert_eq!(
        team.outsider_folder.files().get("Salaries.md").map(Vec::as_slice),
        Some(&b"secret\n"[..])
    );
    assert_eq!(
        team.grantee_folder.files().get("Salaries.md").map(Vec::as_slice),
        Some(&b"secret\n"[..]),
        "a reader of both applied the deferred delete after the create"
    );
    assert!(!team.grantee_folder.files().contains_key("Pay/Salaries.md"));
    assert_eq!(team.grantee_sync.status().conflicts, 0);
}

#[tokio::test]
async fn a_moved_attachment_gets_a_new_blob() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    team.restrict(Level::Write).await;
    let bytes = vec![7; 400_000];
    team.folder.write("Scan.png", &bytes);
    team.settle().await;
    let blobs = server.blob_files().len();
    assert_eq!(team.grantee_folder.files().get("Scan.png"), Some(&bytes));

    rename(&team.folder, "Scan.png", "Pay/Scan.png");
    team.settle().await;
    assert_eq!(
        team.grantee_folder.files().get("Pay/Scan.png"),
        Some(&bytes)
    );
    assert!(
        server.blob_files().len() > blobs,
        "the move uploaded a new blob"
    );
}

#[tokio::test]
async fn restricting_a_folder_moves_what_it_holds_and_each_file_stays_at_its_path() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    team.folder.write("Pay/One.md", b"one\n");
    team.folder.write("Pay/Two.md", b"two\n");
    team.settle().await;
    let old = team.sync.rid_at("Pay/One.md").expect("mapped");

    let account = team.grantee_account();
    let domain = team
        .sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access { account, level: Level::Write }],
        )
        .await
        .expect("restricted");
    assert_eq!(team.folder.files().get("Pay/One.md").map(Vec::as_slice), Some(&b"one\n"[..]));

    // The grantee reads the new root before any move lands, and the file
    // stays at its path rather than going under ` (shared)`.
    for _ in 0..3 {
        team.grantee_sync
            .round(&mut team.grantee.service, 0)
            .await
            .expect("a round");
    }
    assert_eq!(team.grantee_sync.domain_roots().len(), 1);
    assert!(team.grantee_folder.files().contains_key("Pay/One.md"));
    assert!(!team.grantee_folder.files().contains_key("Pay (shared)/One.md"));

    team.settle().await;
    let moved = team.sync.rid_at("Pay/One.md").expect("mapped");
    assert_ne!(moved, old);
    assert_eq!(team.sync.object_domain(&moved), Some(domain));
    let files = team.grantee_folder.files();
    assert!(files.contains_key("Pay/One.md") && files.contains_key("Pay/Two.md"));
    assert!(!files.keys().any(|p| p.contains("(shared)")));
    assert!(
        !team.outsider_folder.files().keys().any(|p| p.starts_with("Pay")),
        "outside the domain the files left"
    );
}

#[tokio::test]
async fn a_grantees_edit_waits_until_the_move_lands() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    team.folder.write("Pay/One.md", b"one\n");
    team.settle().await;
    let account = team.grantee_account();
    team.sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access { account, level: Level::Write }],
        )
        .await
        .expect("restricted");
    // The grantee sees the root, and edits before the moves land.
    for _ in 0..3 {
        team.grantee_sync
            .round(&mut team.grantee.service, 0)
            .await
            .expect("a round");
    }
    assert_eq!(team.grantee_sync.domain_roots().len(), 1);
    team.grantee_folder.write("Pay/One.md", b"edited\n");
    team.grantee_sync
        .round(&mut team.grantee.service, 0)
        .await
        .expect("a round");
    assert_eq!(team.grantee_sync.status().pending, 0, "the edit uploads nothing");
    assert!(
        team.grantee_sync
            .status()
            .skipped
            .iter()
            .any(|s| s.why == SkipWhy::CrossesRoot)
    );
    team.settle().await;
    assert_eq!(
        team.folder.files().get("Pay/One.md").map(Vec::as_slice),
        Some(&b"edited\n"[..]),
        "once the move landed, the edit went to the file's domain"
    );
    assert_eq!(team.sync.status().unreadable, 0);
    assert_eq!(team.sync.status().conflicts, 0);
}

#[tokio::test]
async fn any_managers_device_finishes_moves_the_creating_device_left() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    team.folder.write("Pay/One.md", b"one\n");
    team.settle().await;
    // The creating device stops after the marker: the moves never leave.
    let account = team.grantee_account();
    let domain = team
        .sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access { account, level: Level::Manage }],
        )
        .await
        .expect("restricted");
    team.grantee_sync.set_move_wait(Duration::ZERO);
    for _ in 0..3 {
        team.grantee_sync
            .round(&mut team.grantee.service, 0)
            .await
            .expect("a round");
    }
    let moved = team.grantee_sync.rid_at("Pay/One.md").expect("mapped");
    assert_eq!(team.grantee_sync.object_domain(&moved), Some(domain));
    for _ in 0..3 {
        team.outsider_sync
            .round(&mut team.outsider.service, 0)
            .await
            .expect("a round");
    }
    assert!(!team.outsider_folder.files().contains_key("Pay/One.md"));
}

#[tokio::test]
async fn taking_a_member_off_rotates_and_a_grant_sets_a_floor() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Early.md", b"early\n");
    team.settle().await;
    assert!(team.grantee_folder.files().contains_key("Pay/Early.md"));

    // A grant reaches the outsider's device, with what's live at its floor.
    let own = team.owner.credentials.account;
    let access = |entries: &[(Id, Level)]| -> Vec<Access> {
        entries
            .iter()
            .map(|(account, level)| Access {
                account: *account,
                level: *level,
            })
            .collect()
    };
    let (ben, cy) = (team.grantee_account(), team.outsider_account());
    team.sync
        .set_access(
            &mut team.owner.service,
            domain,
            &access(&[(own, Level::Manage), (ben, Level::Write), (cy, Level::Read)]),
        )
        .await
        .expect("the grant commits");
    team.settle().await;
    assert!(team.outsider_folder.files().contains_key("Pay/Early.md"));

    // Taking the grantee off: the folder's key rotates, so later records
    // never reach it.
    team.sync
        .set_access(
            &mut team.owner.service,
            domain,
            &access(&[(own, Level::Manage), (cy, Level::Read)]),
        )
        .await
        .expect("the removal commits");
    team.folder.write("Pay/Later.md", b"later\n");
    team.settle().await;
    assert!(team.outsider_folder.files().contains_key("Pay/Later.md"));
    let files = team.grantee_folder.files();
    assert!(
        files.contains_key("Pay/Early.md"),
        "what the member has stays with it"
    );
    assert!(!files.contains_key("Pay/Later.md"));
    assert!(team.grantee_folder.path(&format!("Pay/{MARKER}")).exists());
    assert!(
        team.grantee_sync.status().held_roots > 0,
        "its new files in the folder wait"
    );
    team.grantee_folder.write("Pay/Mine.md", b"mine\n");
    team.settle().await;
    assert!(!team.folder.files().contains_key("Pay/Mine.md"));
    assert!(!team.outsider_folder.files().contains_key("Pay/Mine.md"));
}

#[tokio::test]
async fn a_remover_who_cant_read_the_folder_leaves_the_rotation_to_a_grantee() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    // The grantee creates nothing here: the owner hands the folder over,
    // then only removes entries as a workspace owner.
    let domain = team.restrict(Level::Manage).await;
    let ben = team.grantee_account();
    let cy = team.outsider_account();
    team.sync
        .hand_off_domain(&mut team.owner.service, domain, ben)
        .await
        .expect("handed off");
    team.settle().await;
    team.grantee_sync
        .set_access(
            &mut team.grantee.service,
            domain,
            &[
                Access {
                    account: ben,
                    level: Level::Manage,
                },
                Access {
                    account: cy,
                    level: Level::Write,
                },
            ],
        )
        .await
        .expect("the grant commits");
    team.settle().await;
    // The owner doesn't read the domain any more and only removes.
    team.sync
        .set_access(
            &mut team.owner.service,
            domain,
            &[Access {
                account: ben,
                level: Level::Manage,
            }],
        )
        .await
        .expect("an owner removes an entry");
    // Until a key holder rotates, the domain takes no record.
    team.grantee_folder.write("Pay/After.md", b"after\n");
    team.settle().await;
    assert!(
        team.grantee_sync.rid_at("Pay/After.md").is_some(),
        "the grantee rotated and wrote"
    );
    assert_eq!(team.grantee_sync.status().pending, 0);
    assert!(!team.outsider_folder.files().contains_key("Pay/After.md"));
    assert!(!team.folder.files().contains_key("Pay/After.md"));
}

#[tokio::test]
async fn a_hand_off_goes_in_two_changes() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    let domain = team.restrict(Level::Read).await;
    let before = team.sync.policy_revision();
    let ben = team.grantee_account();
    team.sync
        .hand_off_domain(&mut team.owner.service, domain, ben)
        .await
        .expect("handed off");
    assert_eq!(team.sync.policy_revision(), before + 2);
    team.settle().await;
    team.grantee_folder.write("Pay/New.md", b"new\n");
    team.settle().await;
    assert_eq!(team.grantee_sync.status().pending, 0);
    assert!(team.grantee_sync.rid_at("Pay/New.md").is_some());
    assert!(!team.folder.files().contains_key("Pay/New.md"));
}

#[tokio::test]
async fn deleting_a_domain_keeps_the_files_and_holds_them_for_review() {
    let server = Server::start().await;
    let mut team = team(&server, None).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Salaries.md", b"secret\n");
    team.settle().await;
    team.sync
        .delete_domain(&mut team.owner.service, domain)
        .await
        .expect("deleted");
    team.settle().await;
    for (folder, sync) in [
        (&team.folder, &team.sync),
        (&team.grantee_folder, &team.grantee_sync),
    ] {
        assert!(folder.files().contains_key("Pay/Salaries.md"));
        assert_eq!(sync.status().held_roots, 1);
        let held = sync.held_files();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].people, 3, "every member reads the workspace domain");
    }
    assert!(!team.outsider_folder.files().contains_key("Pay/Salaries.md"));

    // New files under it wait too, and nothing reaches a wider domain
    // until you confirm.
    team.folder.write("Pay/New.md", b"new\n");
    team.settle().await;
    assert!(!team.outsider_folder.files().contains_key("Pay/New.md"));

    team.sync.confirm_held("Pay").expect("confirmed");
    team.settle().await;
    assert!(team.outsider_folder.files().contains_key("Pay/Salaries.md"));
    assert_eq!(team.sync.status().held_roots, 0);
    assert!(!team.folder.path(&format!("Pay/{MARKER}")).exists());

    let refused = team.sync.delete_domain(&mut team.owner.service, domain).await;
    assert!(matches!(refused, Err(SyncError::Check(_))));
}
