//! Moves between domains and who reads a restricted folder, end to end
//! (seam 4, sync-v2.md §8, §9): a file that crosses a root gets a new
//! `rid`, an `origin` and a reason-2 delete in one push; a file that would
//! reach more people waits for you; restricting a folder moves what it
//! holds; `set_access` and `delete_domain` take effect on every device.

mod sync_support;

use std::time::Duration;

use app_core::sync::invitations::InvitationBook;
use app_core::sync::workspace::{SkipWhy, WorkspaceSync};
use app_core::sync::{SyncError, SyncService};
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
            self.sync
                .round(&mut self.owner.service, 0)
                .await
                .expect("a round");
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
            .restrict_folder(&mut self.owner.service, "Pay", &[Access { account, level }])
            .await
            .expect("restricted");
        self.settle().await;
        domain
    }
}

async fn team<B: Backend>(server: &Server<B>) -> Team {
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
    let (outsider_sync, outsider_folder) = invite_and_join(
        &mut owner,
        &mut sync,
        &mut book,
        &mut outsider,
        Role::Editor,
    )
    .await;
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
    let mut team = team(&server).await;
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
        team.grantee_folder
            .files()
            .get("Pay/Plan.md")
            .map(Vec::as_slice),
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

    // The warning outlasts a restart until you dismiss it.
    drop(team.sync);
    let reopened = WorkspaceSync::open(
        team.folder.engine.clone(),
        team.owner.credentials.clone(),
        team.owner.secrets(),
        &team.folder.registry,
    )
    .expect("the journal opens");
    assert_eq!(reopened.moved_in().len(), 1);
}

#[tokio::test]
async fn a_file_leaving_a_restricted_folder_waits_for_review_with_the_count() {
    let server = Server::start().await;
    let mut team = team(&server).await;
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
        team.outsider_folder
            .files()
            .get("Salaries.md")
            .map(Vec::as_slice),
        Some(&b"secret\n"[..])
    );
    assert_eq!(
        team.grantee_folder
            .files()
            .get("Salaries.md")
            .map(Vec::as_slice),
        Some(&b"secret\n"[..]),
        "a reader of both applied the deferred delete after the create"
    );
    assert!(!team.grantee_folder.files().contains_key("Pay/Salaries.md"));
    assert_eq!(team.grantee_sync.status().conflicts, 0);
}

#[tokio::test]
async fn a_moved_attachment_gets_a_new_blob() {
    let server = Server::start().await;
    let mut team = team(&server).await;
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
    let mut team = team(&server).await;
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
            &[Access {
                account,
                level: Level::Write,
            }],
        )
        .await
        .expect("restricted");
    assert_eq!(
        team.folder.files().get("Pay/One.md").map(Vec::as_slice),
        Some(&b"one\n"[..])
    );

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
    assert!(
        !team
            .grantee_folder
            .files()
            .contains_key("Pay (shared)/One.md")
    );

    team.settle().await;
    let moved = team.sync.rid_at("Pay/One.md").expect("mapped");
    assert_ne!(moved, old);
    assert_eq!(team.sync.object_domain(&moved), Some(domain));
    let files = team.grantee_folder.files();
    assert!(files.contains_key("Pay/One.md") && files.contains_key("Pay/Two.md"));
    assert!(!files.keys().any(|p| p.contains("(shared)")));
    assert!(
        !team
            .outsider_folder
            .files()
            .keys()
            .any(|p| p.starts_with("Pay")),
        "outside the domain the files left"
    );
}

#[tokio::test]
async fn a_grantees_edit_waits_until_the_move_lands() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    team.folder.write("Pay/One.md", b"one\n");
    team.settle().await;
    let account = team.grantee_account();
    team.sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access {
                account,
                level: Level::Write,
            }],
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
    assert_eq!(
        team.grantee_sync.status().pending,
        0,
        "the edit uploads nothing"
    );
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
    let mut team = team(&server).await;
    team.folder.write("Pay/One.md", b"one\n");
    team.settle().await;
    // The creating device stops after the marker: the moves never leave.
    let account = team.grantee_account();
    let domain = team
        .sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access {
                account,
                level: Level::Manage,
            }],
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
    let mut team = team(&server).await;
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
    let mut team = team(&server).await;
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
    let mut team = team(&server).await;
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

/// A pair that loses a half comes undone: the file stays, nothing of the
/// move reaches the server, and the next round moves it whole.
#[tokio::test]
async fn a_pair_that_loses_a_half_comes_undone_and_tries_again() {
    use sync_protocol::codec::{Bytes, Encode, Text};
    use sync_protocol::messages::{ErrorBody, PushFailureDetail};

    let (server, proxy) = Proxy::in_front().await;
    let mut team = team(&server).await;
    let mut proxied = team.owner.connect_through(&proxy.base).await;
    let domain = team.restrict(Level::Write).await;
    let old = team.sync.rid_at("Plan.md").expect("mapped");
    rename(&team.folder, "Plan.md", "Pay/Plan.md");
    let once = std::sync::atomic::AtomicBool::new(false);
    proxy.forge(Box::new(move |method, path| {
        if method != "POST" || !path.ends_with("/ops") {
            return None;
        }
        if once.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let code = "forbidden";
        let detail = PushFailureDetail {
            record_index: 1,
            code: Text(code.to_owned()),
            record_detail: None,
        };
        let body = ErrorBody {
            code: Text(code.to_owned()),
            message: Text("refused".to_owned()),
            retry_after_seconds: None,
            detail: Some(Bytes(detail.to_bytes())),
        };
        Some((403, body.to_bytes()))
    }));
    for _ in 0..3 {
        team.sync.round(&mut proxied, 0).await.expect("a round");
    }
    proxy.honest();
    assert!(team.folder.files().contains_key("Pay/Plan.md"));
    let moved = team.sync.rid_at("Pay/Plan.md").expect("the new object");
    assert_ne!(moved, old);
    assert_eq!(team.sync.object_domain(&moved), Some(domain));
    assert_eq!(team.sync.status().pending, 0);

    team.settle().await;
    assert_eq!(team.grantee_sync.rid_at("Pay/Plan.md"), Some(moved));
    assert_eq!(team.grantee_sync.status().conflicts, 0);
    assert!(!team.grantee_folder.files().contains_key("Plan.md"));
    assert!(!team.outsider_folder.files().contains_key("Plan.md"));
}

/// A member taken off and granted again holds old keys and a cursor below
/// its new floor on the same device: it reads what's live at the floor,
/// its hold ends, and it writes again.
#[tokio::test]
async fn a_member_taken_off_and_granted_again_converges_on_the_same_device() {
    let server = Server::start().await;
    let mut team = team(&server).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Early.md", b"early\n");
    team.settle().await;
    let own = team.owner.credentials.account;
    let ben = team.grantee_account();
    let list = |ben_level: Option<Level>| {
        let mut list = vec![Access {
            account: own,
            level: Level::Manage,
        }];
        list.extend(ben_level.map(|level| Access {
            account: ben,
            level,
        }));
        list
    };
    team.sync
        .set_access(&mut team.owner.service, domain, &list(None))
        .await
        .expect("taken off");
    team.folder.write("Pay/Away.md", b"away\n");
    team.settle().await;
    assert!(!team.grantee_folder.files().contains_key("Pay/Away.md"));

    team.sync
        .set_access(&mut team.owner.service, domain, &list(Some(Level::Write)))
        .await
        .expect("granted again");
    team.folder.write("Pay/Back.md", b"back\n");
    team.settle().await;
    team.settle().await;
    let files = team.grantee_folder.files();
    assert!(files.contains_key("Pay/Early.md"));
    assert!(files.contains_key("Pay/Away.md"), "live at the new floor");
    assert!(files.contains_key("Pay/Back.md"));
    assert_eq!(team.grantee_sync.status().held_roots, 0);
    assert_eq!(team.grantee_sync.status().conflicts, 0);

    team.grantee_folder.write("Pay/Reply.md", b"reply\n");
    team.settle().await;
    assert!(team.folder.files().contains_key("Pay/Reply.md"));
}

#[tokio::test]
async fn deleting_a_domain_keeps_the_files_and_holds_them_for_review() {
    let server = Server::start().await;
    let mut team = team(&server).await;
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
        assert_eq!(held[0].people, 1, "only the outsider would newly read it");
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

    let refused = team
        .sync
        .delete_domain(&mut team.owner.service, domain)
        .await;
    assert!(matches!(refused, Err(SyncError::Check(_))));
}

/// Moved from ticket 58: a device approved after a domain was restricted
/// holds no anchor there. A colluding writer's checkpoint off the chain,
/// which a server serves it, passes the signer and recency checks, and the
/// device catches it when the real checkpoint arrives and doesn't name it
/// (§30). The honest run of the same steps shows that nothing else stops
/// the device.
#[tokio::test]
async fn a_checkpoint_off_the_chain_in_a_restricted_domain_is_caught() {
    assert_eq!(
        bootstrap_with_side_checkpoint(true).await,
        Some("sync_chain_gap")
    );
}

#[tokio::test]
async fn a_new_device_bootstraps_a_restricted_domain_from_the_real_checkpoint() {
    assert_eq!(bootstrap_with_side_checkpoint(false).await, None);
}

/// The steps of the off-the-chain case, with the side checkpoint served or
/// not. Returns the check the device stops on, if any.
async fn bootstrap_with_side_checkpoint(serve_it: bool) -> Option<&'static str> {
    use sync_protocol::codec::Encode;
    use sync_protocol::messages::CheckpointResponse;

    let server = Server::start_with_interval(6).await;
    let mut team = team(&server).await;
    let domain = team.restrict(Level::Write).await;
    team.sync.set_checkpoint_wait(Duration::from_secs(3_600));
    team.grantee_sync
        .set_checkpoint_wait(Duration::from_secs(3_600));
    for i in 0..8 {
        team.folder.write(&format!("Pay/n{i}.md"), b"note\n");
    }
    team.settle().await;

    // The grantee, a writer, signs a checkpoint nobody pushes.
    let side = team
        .grantee_sync
        .sign_side_checkpoint(domain)
        .expect("something to checkpoint");

    // The real checkpoint commits at the same point.
    team.sync.set_checkpoint_wait(Duration::ZERO);
    for _ in 0..3 {
        team.sync
            .round(&mut team.owner.service, 0)
            .await
            .expect("a round");
    }
    team.settle().await;
    let real = team
        .sync
        .domain_checkpoint(&domain)
        .expect("the real checkpoint");
    assert_eq!(real.body.covered_seq, side.checkpoint.0.body.covered_seq);
    assert_ne!(real.digest(), side.checkpoint.0.digest());
    let seq = team
        .owner
        .service
        .client()
        .checkpoint_by_digest(team.sync.workspace(), domain, real.digest())
        .await
        .expect("served")
        .expect("the entry")
        .seq;

    // The owner's new device, which holds no anchor in the domain.
    let new = sync_support::approve(
        &server,
        &mut team.owner,
        std::slice::from_mut(team.sync.remote_mut().expect("connected")),
    )
    .await;
    let proxy = Proxy::start(&server.base).await;
    let mut service = new.connect_through(&proxy.base).await;
    let mut folder = Folder::new("Downloaded");
    let mut sync = WorkspaceSync::download(
        &mut service,
        folder.engine.clone(),
        team.sync.workspace(),
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    let encoded = domain.to_base64url();
    if serve_it {
        proxy.tamper(Box::new(move |path, body| {
            if path.ends_with("/checkpoint") && path.contains(&encoded) {
                return CheckpointResponse {
                    seq,
                    bundle: side.clone(),
                }
                .to_bytes();
            }
            body
        }));
    }
    let mut stopped = None;
    for _ in 0..4 {
        match sync.round(&mut service, 0).await {
            Ok(_) => {}
            Err(SyncError::Check(code)) => stopped = Some(code),
            Err(error) => panic!("{error:?}"),
        }
    }
    assert_eq!(sync.domain_roots().len(), 1);
    assert!(
        folder.files().contains_key("Pay/n0.md"),
        "the device installed the checkpoint's files"
    );
    stopped
}

/// The section's rows come from real state: who reads each folder, a move
/// that waits with its count, and the warning for a returning grantee.
#[tokio::test]
async fn the_members_screen_reports_folders_and_what_waits() {
    use app_core::members::MembersScreen;

    let server = Server::start().await;
    let mut team = team(&server).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Salaries.md", b"secret\n");
    team.settle().await;
    let own = team.owner.credentials.account;
    let cy = team.outsider_account();
    team.sync
        .set_access(
            &mut team.owner.service,
            domain,
            &[Access {
                account: own,
                level: Level::Manage,
            }],
        )
        .await
        .expect("the grantee leaves the list");
    rename(&team.folder, "Pay/Salaries.md", "Salaries.md");
    team.sync
        .round(&mut team.owner.service, 0)
        .await
        .expect("a round");

    let policy = team.sync.remote_mut().expect("connected").policy().clone();
    let screen =
        MembersScreen::of(policy.head_snapshot(), own, &[]).with_folders(&policy, own, &team.sync);
    let folder = &screen.folders[0];
    assert_eq!(folder.label, "Pay");
    let ben = folder
        .candidates
        .iter()
        .find(|c| c.account == team.grantee_account())
        .expect("a candidate");
    assert!(ben.returning.is_some(), "it warns before a second grant");
    let new = folder
        .candidates
        .iter()
        .find(|c| c.account == cy)
        .expect("a candidate");
    assert!(new.returning.is_none());
    assert_eq!(
        screen.moves[0].label,
        "Salaries.md left a restricted folder. Moving it shows it to 2 more people."
    );
}

/// `remove_member` drops the account from each access list, as a grantee
/// and as a manager, and the signer rotates each domain it reads; a domain
/// it doesn't read rotates from another key holder (§8, §9).
#[tokio::test]
async fn removing_a_member_drops_its_grants_and_each_domain_rotates() {
    use app_core::sync::members::MemberChange;

    let server = Server::start().await;
    let mut team = team(&server).await;
    let domain = team.restrict(Level::Manage).await;
    let (ben, cy) = (team.grantee_account(), team.outsider_account());
    // A second folder the owner doesn't read, which Cy and Ben do.
    team.sync
        .change_member(
            &mut team.owner.service,
            MemberChange::SetRole {
                account: cy,
                role: Role::Admin,
            },
        )
        .await
        .expect("an admin");
    team.settle().await;
    let other = team
        .outsider_sync
        .restrict_folder(
            &mut team.outsider.service,
            "Ops",
            &[Access {
                account: ben,
                level: Level::Write,
            }],
        )
        .await
        .expect("an admin restricts a folder");
    team.outsider_folder.write("Ops/Run.md", b"run\n");
    team.settle().await;
    assert!(team.grantee_folder.files().contains_key("Ops/Run.md"));

    assert_eq!(team.sync.domain_epoch(&domain), 1);
    team.sync
        .change_member(&mut team.owner.service, MemberChange::Remove(ben))
        .await
        .expect("removed");
    assert_eq!(team.sync.domain_epoch(&domain), 2, "the signer rotated");
    let policy = team.sync.remote_mut().expect("connected").policy().clone();
    let snapshot = policy.head_snapshot();
    assert!(!snapshot.domains[&domain].access.contains_key(&ben));
    assert!(!snapshot.domains[&other].access.contains_key(&ben));

    team.sync
        .round(&mut team.owner.service, 0)
        .await
        .expect("a round");
    for _ in 0..3 {
        team.outsider_sync
            .round(&mut team.outsider.service, 0)
            .await
            .expect("a round");
    }
    assert_eq!(
        team.outsider_sync.domain_epoch(&other),
        2,
        "a key holder rotated the rest"
    );

    // An editor restricts nothing.
    let refused = team
        .grantee_sync
        .restrict_folder(&mut team.grantee.service, "Mine", &[])
        .await;
    assert!(refused.is_err());
}

/// A member removed and invited again stays out of every access list until
/// a manager grants it again (§8).
#[tokio::test]
async fn a_member_invited_again_stays_out_of_restricted_folders_until_granted() {
    use app_core::sync::members::MemberChange;

    let server = Server::start().await;
    let mut team = team(&server).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Salaries.md", b"secret\n");
    team.settle().await;
    let ben = team.grantee_account();
    team.sync
        .change_member(&mut team.owner.service, MemberChange::Remove(ben))
        .await
        .expect("removed");
    let mut book = InvitationBook::open(team.owner.state.path(), team.owner.id()).unwrap();
    let (mut again, again_folder) = invite_and_join(
        &mut team.owner,
        &mut team.sync,
        &mut book,
        &mut team.grantee,
        Role::Editor,
    )
    .await;
    for _ in 0..3 {
        again
            .round(&mut team.grantee.service, 0)
            .await
            .expect("a round");
    }
    let policy = team.sync.remote_mut().expect("connected").policy().clone();
    assert!(
        !policy.head_snapshot().domains[&domain]
            .access
            .contains_key(&ben)
    );
    assert!(
        !again_folder.files().keys().any(|p| p.starts_with("Pay/")),
        "the folder stays out of its new copy"
    );
}

/// A move spans two domains' head rows in one transaction on PostgreSQL
/// too, and each domain's reader ends with one file (§15, §9).
#[tokio::test]
async fn a_move_commits_on_postgres() {
    let server = Server::start_postgres().await;
    let mut team = team(&server).await;
    team.restrict(Level::Write).await;
    rename(&team.folder, "Plan.md", "Pay/Plan.md");
    team.settle().await;
    assert!(team.grantee_folder.files().contains_key("Pay/Plan.md"));
    assert!(!team.grantee_folder.files().contains_key("Plan.md"));
    assert!(!team.outsider_folder.files().contains_key("Plan.md"));
    assert_eq!(team.sync.status().pending, 0);
}

/// A pull of `domain` from the start, as `service`'s device.
async fn first_page(
    service: &mut app_core::sync::SyncService,
    workspace: Id,
    domain: Id,
) -> sync_protocol::messages::PageStatus {
    let cursor = sync_protocol::messages::PullCursor { domain, after: 0 };
    service
        .client()
        .pull(workspace, vec![cursor], 0)
        .await
        .expect("a pull")
        .pages
        .0[0]
        .status
}

/// A grantee reads nothing below the floor its grant set, and a member's
/// new device and the device its kit restores keep the member's floor
/// (§9). An account accepted after a reset waits for ticket 60, which adds
/// `accept_reset` to a push.
#[tokio::test]
async fn floors_hold_for_a_grantee_a_new_device_and_a_restored_device() {
    use app_core::sync::recovery::{Recovery, open_kit};
    use std::sync::Arc;
    use sync_protocol::messages::PageStatus;
    use sync_support::MemorySecrets;

    let server = Server::start().await;
    let mut team = team(&server).await;
    let domain = team.restrict(Level::Write).await;
    team.folder.write("Pay/Early.md", b"early\n");
    team.folder.write("Plan.md", b"# Plan, edited\n");
    team.settle().await;
    let workspace = team.sync.workspace();
    let workspace_domain = team.sync.domain();
    let (own, ben, cy) = (
        team.owner.credentials.account,
        team.grantee_account(),
        team.outsider_account(),
    );
    let list = [
        Access {
            account: own,
            level: Level::Manage,
        },
        Access {
            account: ben,
            level: Level::Write,
        },
        Access {
            account: cy,
            level: Level::Read,
        },
    ];
    team.sync
        .set_access(&mut team.owner.service, domain, &list)
        .await
        .expect("the grant commits");
    team.settle().await;
    assert_eq!(
        first_page(&mut team.outsider.service, workspace, domain).await,
        PageStatus::BeforeHistoryFloor
    );
    assert!(
        team.outsider_folder.files().contains_key("Pay/Early.md"),
        "a grantee reads what's live at the floor's checkpoint"
    );
    assert_eq!(
        first_page(&mut team.owner.service, workspace, domain).await,
        PageStatus::Ok
    );

    // Cy's new device keeps Cy's floor.
    let new = sync_support::approve(
        &server,
        &mut team.outsider,
        std::slice::from_mut(team.outsider_sync.remote_mut().expect("connected")),
    )
    .await;
    let mut service = new.connect_through(&server.base).await;
    assert_eq!(
        first_page(&mut service, workspace, workspace_domain).await,
        PageStatus::BeforeHistoryFloor
    );

    // So does the device Cy's kit restores.
    team.outsider_sync
        .round(&mut team.outsider.service, 0)
        .await
        .expect("a round");
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Noura recovery kit");
    let export = team
        .outsider
        .service
        .export_kit(
            &path,
            &[],
            std::slice::from_mut(team.outsider_sync.remote_mut().expect("connected")),
        )
        .await
        .expect("exported");
    let kit = open_kit(&path, export.secret.as_ref()).expect("the kit opens");
    let signed_in = server.sign_in(cy, &server.base).await;
    let secrets = Arc::new(MemorySecrets::default());
    let state = tempfile::tempdir().unwrap();
    let recovery = Recovery::begin(&signed_in, kit, secrets.clone(), state.path())
        .await
        .expect("the kit's identity proves");
    let (mut service, _, _) = recovery.finish(&[], &*secrets).await.expect("recovered");
    assert_eq!(
        first_page(&mut service, workspace, workspace_domain).await,
        PageStatus::BeforeHistoryFloor
    );
}

/// Runs `sync` for `rounds` rounds, with its move wait gone.
async fn run(sync: &mut WorkspaceSync, service: &mut SyncService, rounds: usize) {
    sync.set_move_wait(Duration::ZERO);
    for _ in 0..rounds {
        sync.round(service, 0).await.expect("a round");
    }
}

/// The checkpoint that covers `create_domain`, signed by `sync`'s device
/// over some workspace-domain work, so a new device bootstraps past that
/// entry.
async fn checkpoint_past_the_restriction(team: &mut Team) {
    team.sync.set_checkpoint_wait(Duration::ZERO);
    for i in 0..8 {
        team.folder.write(&format!("Filler {i}.md"), b"filler\n");
    }
    team.settle().await;
    team.settle().await;
}

/// §9 on a device that bootstrapped past `create_domain`: only an object
/// signed before the restriction waits for its move. A file an outsider
/// writes later goes under ` (shared)` there too, a bootstrapped manager
/// doesn't take it into the domain, so the outsider keeps it, and neither
/// a later write nor a stale one takes a path from the domain's own file.
#[tokio::test]
async fn an_outsiders_later_file_stays_out_of_the_root_on_a_bootstrapped_device() {
    let server = Server::start_with_interval(6).await;
    let mut team = team(&server).await;
    let account = team.grantee_account();
    // The outsider hasn't heard of the restriction when it writes `Same.md`.
    let domain = team
        .sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access {
                account,
                level: Level::Manage,
            }],
        )
        .await
        .expect("restricted");
    team.folder.write("Pay/Same.md", b"restricted\n");
    for _ in 0..3 {
        team.sync
            .round(&mut team.owner.service, 0)
            .await
            .expect("a round");
    }
    team.outsider_folder
        .write("Pay/Same.md", b"stale outside\n");
    team.outsider_sync.capture().expect("captured");
    team.outsider_sync
        .push(&mut team.outsider.service)
        .await
        .expect("pushed");
    team.settle().await;

    // Once it knows, it writes under the root anyway.
    team.outsider_folder.write("Pay/Late.md", b"late outside\n");
    team.settle().await;
    checkpoint_past_the_restriction(&mut team).await;
    assert_eq!(
        team.folder.files().get("Pay/Same.md").map(Vec::as_slice),
        Some(&b"restricted\n"[..])
    );

    // The grantee's new device, whose account manages the domain.
    let new = sync_support::approve(
        &server,
        &mut team.grantee,
        std::slice::from_mut(team.grantee_sync.remote_mut().expect("connected")),
    )
    .await;
    let mut service = new.connect_through(&server.base).await;
    let mut folder = Folder::new("Downloaded");
    let mut sync = WorkspaceSync::download(
        &mut service,
        folder.engine.clone(),
        team.sync.workspace(),
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    run(&mut sync, &mut service, 4).await;
    assert!(sync.bootstrapped_past(&domain), "it never saw the entry");

    let files = folder.files();
    assert_eq!(
        files.get("Pay/Same.md").map(Vec::as_slice),
        Some(&b"restricted\n"[..]),
        "the domain's file keeps its path: {:?}",
        files.keys().collect::<Vec<_>>()
    );
    assert!(!files.contains_key("Pay/Same (2).md"));
    assert_eq!(
        files.get("Pay (shared)/Same.md").map(Vec::as_slice),
        Some(&b"stale outside\n"[..])
    );
    assert_eq!(
        files.get("Pay (shared)/Late.md").map(Vec::as_slice),
        Some(&b"late outside\n"[..])
    );
    assert!(!files.contains_key("Pay/Late.md"));
    let late = sync.rid_at("Pay (shared)/Late.md").expect("mapped");
    assert_eq!(sync.object_domain(&late), None);

    // The bootstrapped manager moved neither, so the outsider keeps both.
    team.settle().await;
    run(&mut sync, &mut service, 2).await;
    team.settle().await;
    let outside = team.outsider_folder.files();
    assert!(outside.contains_key("Pay/Late.md"), "{:?}", outside.keys());
    assert!(outside.contains_key("Pay/Same.md"));
}

/// A device that bootstrapped past `create_domain` still keeps a file that
/// predates the root at its path, and a manager's device finishes its move.
#[tokio::test]
async fn a_bootstrapped_manager_finishes_a_move_of_a_file_that_predates_the_root() {
    let server = Server::start_with_interval(6).await;
    let mut team = team(&server).await;
    team.folder.write("Pay/One.md", b"one\n");
    team.settle().await;
    let account = team.grantee_account();
    let domain = team
        .sync
        .restrict_folder(
            &mut team.owner.service,
            "Pay",
            &[Access {
                account,
                level: Level::Manage,
            }],
        )
        .await
        .expect("restricted");
    // The creating device stops, and the workspace goes on without it. The
    // grantee holds off too, so the move is still waiting.
    team.grantee_sync.set_move_wait(Duration::from_secs(3_600));
    team.grantee_sync.set_checkpoint_wait(Duration::ZERO);
    for i in 0..8 {
        team.grantee_folder
            .write(&format!("Filler {i}.md"), b"filler\n");
    }
    for _ in 0..4 {
        team.grantee_sync
            .round(&mut team.grantee.service, 0)
            .await
            .expect("a round");
    }
    let before = team.grantee_sync.rid_at("Pay/One.md").expect("mapped");
    assert_eq!(team.grantee_sync.object_domain(&before), None);

    let new = sync_support::approve(
        &server,
        &mut team.grantee,
        std::slice::from_mut(team.grantee_sync.remote_mut().expect("connected")),
    )
    .await;
    let mut service = new.connect_through(&server.base).await;
    let mut folder = Folder::new("Downloaded");
    let mut sync = WorkspaceSync::download(
        &mut service,
        folder.engine.clone(),
        team.sync.workspace(),
        &mut folder.registry,
    )
    .await
    .expect("the download starts");
    sync.set_move_wait(Duration::from_secs(3_600));
    for _ in 0..3 {
        sync.round(&mut service, 0).await.expect("a round");
    }
    assert!(sync.bootstrapped_past(&domain), "it never saw the entry");
    assert!(folder.files().contains_key("Pay/One.md"));
    assert!(!folder.files().keys().any(|p| p.contains("(shared)")));

    run(&mut sync, &mut service, 3).await;
    let moved = sync.rid_at("Pay/One.md").expect("mapped");
    assert_eq!(sync.object_domain(&moved), Some(domain));
}
