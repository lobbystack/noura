//! Roles and removal at seam 4 (sync-v2.md §8, §9, §15): each role's
//! rights under the authority table, `remove_member` with the rotation it
//! makes due, status 4 pages and the removal notice on every device of the
//! removed account, and a member invited again keeping its card indices.

mod sync_support;

use app_core::folder_sync::FolderSyncScreen;
use app_core::sync::invitations::{InvitationBook, NewInvitation};
use app_core::sync::members::MemberChange;
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::{RemoteWorkspace, SyncError};
use sync_protocol::messages::{PageStatus, PullCursor};
use sync_protocol::records::Role;
use sync_protocol::types::Id;
use sync_store::Backend;
use sync_support::{Desktop, Folder, Proxy, Server, approve, first_desktop, invite_and_join};

/// The owner's desktop with a synced workspace and its invitation book.
struct Owner {
    desktop: Desktop,
    sync: WorkspaceSync,
    folder: Folder,
    book: InvitationBook,
}

async fn owner<B: Backend>(server: &Server<B>) -> Owner {
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
    let book = InvitationBook::open(desktop.state.path(), desktop.id()).unwrap();
    Owner {
        desktop,
        sync,
        folder,
        book,
    }
}

impl Owner {
    async fn invite(&mut self, member: &mut Desktop, role: Role) -> (WorkspaceSync, Folder) {
        invite_and_join(
            &mut self.desktop,
            &mut self.sync,
            &mut self.book,
            member,
            role,
        )
        .await
    }

    async fn change(&mut self, change: MemberChange) -> Result<u64, SyncError> {
        self.sync
            .change_member(&mut self.desktop.service, change)
            .await
    }

    fn remote(&mut self) -> &mut RemoteWorkspace {
        self.sync.remote_mut().expect("connected")
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, SyncError>) -> String {
    match result {
        Ok(value) => panic!("expected a refusal, got {value:?}"),
        Err(SyncError::Check(code)) => code.to_owned(),
        Err(SyncError::Refused { code, .. }) => code,
        Err(other) => panic!("unexpected error {other:?}"),
    }
}

fn account(desktop: &Desktop) -> Id {
    desktop.credentials.account
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_role_holds_the_rights_of_section_8() {
    let server = Server::start().await;
    let mut ana = owner(&server).await;
    let workspace = ana.sync.workspace();

    // A viewer pulls but can't write: the server refuses its push.
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let (mut ben_sync, ben_folder) = ana.invite(&mut ben, Role::Viewer).await;
    assert_eq!(ben_folder.files()["Plan.md"], b"# Plan\n");
    ana.folder.write("Later.md", b"Written after Ben joined.\n");
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    ben_sync.round(&mut ben.service, 0).await.expect("a round");
    assert_eq!(
        ben_folder.files()["Later.md"],
        b"Written after Ben joined.\n"
    );
    // The client sends the viewer's edit, and the server refuses it with
    // `forbidden`, the one refusal on which the client drops what waits
    // and keeps the file as it is (§14).
    ben_folder.write("Viewer.md", b"A viewer's edit.\n");
    assert_eq!(ben_sync.capture().expect("captured"), 1);
    let proxy = Proxy::start(&server.base).await;
    let mut through = ben.connect_through(&proxy.base).await;
    assert_eq!(ben_sync.push(&mut through).await.expect("dropped"), 0);
    assert!(
        proxy
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path.ends_with("/ops")),
        "the push went out"
    );
    assert!(ben_sync.outbox().is_empty());
    assert_eq!(ben_folder.files()["Viewer.md"], b"A viewer's edit.\n");
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    assert!(!ana.folder.files().contains_key("Viewer.md"));
    // It still pulls.
    ana.folder.write("Later.md", b"Changed again.\n");
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    ben_sync.round(&mut ben.service, 0).await.expect("a round");
    assert_eq!(ben_folder.files()["Later.md"], b"Changed again.\n");

    // An admin invites an editor, but only the owner invites an admin.
    let mut cleo = first_desktop(&server, "cleo@example.com").await;
    let (mut cleo_sync, _cleo_folder) = ana.invite(&mut cleo, Role::Admin).await;
    let mut cleo_book = InvitationBook::open(cleo.state.path(), cleo.id()).unwrap();
    assert_eq!(
        code(
            cleo.service
                .create_invitation(
                    &mut cleo_book,
                    NewInvitation {
                        workspace,
                        role: Role::Admin,
                        workspace_name: "Field notes",
                        inviter_name: "Cleo",
                    },
                )
                .await
        ),
        "forbidden",
        "an admin can't invite an admin"
    );
    let mut dan = first_desktop(&server, "dan@example.com").await;
    let (mut dan_sync, dan_folder) = invite_and_join(
        &mut cleo,
        &mut cleo_sync,
        &mut cleo_book,
        &mut dan,
        Role::Editor,
    )
    .await;

    // An editor reads and writes.
    dan_folder.write("Editor.md", b"An editor's note.\n");
    dan_sync.round(&mut dan.service, 0).await.expect("a round");
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(ana.folder.files()["Editor.md"], b"An editor's note.\n");
    // An editor changes no role and removes no one else.
    assert_eq!(
        code(
            dan_sync
                .change_member(
                    &mut dan.service,
                    MemberChange::SetRole {
                        account: account(&ben),
                        role: Role::Editor,
                    },
                )
                .await
        ),
        "sync_policy_unauthorized"
    );
    assert_eq!(
        code(
            dan_sync
                .change_member(&mut dan.service, MemberChange::Remove(account(&ben)))
                .await
        ),
        "sync_policy_unauthorized"
    );
    // An editor doesn't rename the workspace; an admin does.
    assert!(
        dan_sync
            .rename(&mut dan.service, "Dan's notes")
            .await
            .is_err()
    );
    cleo_sync
        .rename(&mut cleo.service, "Field notes 2026")
        .await
        .expect("an admin renames the workspace");

    // An admin moves editors and viewers between the two roles, and
    // touches no admin or owner.
    cleo_sync
        .change_member(
            &mut cleo.service,
            MemberChange::SetRole {
                account: account(&dan),
                role: Role::Viewer,
            },
        )
        .await
        .expect("an admin makes an editor a viewer");
    for change in [
        MemberChange::SetRole {
            account: account(&dan),
            role: Role::Admin,
        },
        MemberChange::SetRole {
            account: account(&ana.desktop),
            role: Role::Editor,
        },
        MemberChange::Remove(account(&ana.desktop)),
        MemberChange::TransferOwner(account(&dan)),
    ] {
        assert_eq!(
            code(cleo_sync.change_member(&mut cleo.service, change).await),
            "sync_policy_unauthorized",
            "{change:?}"
        );
    }
    // Dan, a viewer now, is refused at the server.
    dan_sync.round(&mut dan.service, 0).await.expect("a round");
    dan_folder.write("Editor.md", b"Changed by a viewer.\n");
    assert_eq!(dan_sync.capture().expect("captured"), 1);
    assert_eq!(dan_sync.push(&mut dan.service).await.expect("dropped"), 0);
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(ana.folder.files()["Editor.md"], b"An editor's note.\n");

    // An admin removes a viewer.
    cleo_sync
        .change_member(&mut cleo.service, MemberChange::Remove(account(&ben)))
        .await
        .expect("an admin removes a viewer");

    // Only the owner adds or removes admins and transfers ownership.
    ana.change(MemberChange::SetRole {
        account: account(&dan),
        role: Role::Admin,
    })
    .await
    .expect("the owner makes an admin");
    assert_eq!(
        code(
            cleo_sync
                .change_member(&mut cleo.service, MemberChange::Remove(account(&dan)))
                .await
        ),
        "sync_policy_unauthorized",
        "an admin can't remove an admin"
    );
    assert_eq!(
        code(
            ana.change(MemberChange::Remove(account(&ana.desktop)))
                .await
        ),
        "sync_policy_unauthorized",
        "the owner transfers ownership before it leaves"
    );
    ana.change(MemberChange::TransferOwner(account(&cleo)))
        .await
        .expect("the owner transfers ownership");
    let snapshot = ana.remote().policy().head_snapshot().clone();
    assert_eq!(snapshot.role(&account(&cleo)), Some(Role::Owner));
    assert_eq!(snapshot.role(&account(&ana.desktop)), Some(Role::Admin));
    assert_eq!(snapshot.role(&account(&ben)), None);
}

async fn removal<B: Backend>(server: Server<B>) {
    let mut ana = owner(&server).await;
    let workspace = ana.sync.workspace();
    let domain = ana.sync.domain();

    // Ben's laptop joins as an editor; his phone trusts the workspace
    // through the laptop's recorded head.
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let mut phone = approve(&server, &mut ben, &mut []).await;
    let (mut laptop_sync, laptop_folder) = ana.invite(&mut ben, Role::Editor).await;
    let mut phone_folder = Folder::new("Phone");
    let mut phone_sync = WorkspaceSync::download(
        &mut phone.service,
        phone_folder.engine.clone(),
        workspace,
        &mut phone_folder.registry,
    )
    .await
    .expect("the phone trusts the workspace");
    phone_sync
        .round(&mut phone.service, 0)
        .await
        .expect("a round");
    assert_eq!(phone_folder.files()["Plan.md"], b"# Plan\n");
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    let before = ana.remote().epoch();
    let indices = ana.remote().policy().head_snapshot().card_indices.clone();

    let revision = ana
        .change(MemberChange::Remove(account(&ben)))
        .await
        .expect("the owner removes Ben");
    // The signing device rotated the workspace domain after the push, for
    // its own devices alone.
    let remote = ana.remote();
    assert_eq!(remote.epoch(), before + 1, "the workspace domain rotated");
    let snapshot = remote.policy().head_snapshot().clone();
    assert!(!snapshot.is_device(&ben.id()) && !snapshot.is_device(&phone.id()));
    assert_eq!(snapshot.readers(&domain).len(), 1);
    assert_eq!(snapshot.card_indices, indices, "indices stay");

    // Each of Ben's devices gets status 4 pages, the policy up to its
    // removal and nothing else, and shows who removed it.
    for device in [&mut ben, &mut phone] {
        let pull = device
            .service
            .client()
            .pull(workspace, vec![PullCursor { domain, after: 0 }], 0)
            .await
            .expect("a pull answers");
        assert!(
            pull.pages
                .0
                .iter()
                .all(|p| p.status == PageStatus::Forbidden),
            "status 4"
        );
        let policy = device
            .service
            .client()
            .policy(workspace, 0)
            .await
            .expect("the chain up to the removal");
        assert_eq!(
            policy.changes.0.last().unwrap().0.body.revision,
            revision,
            "up to and including the removal"
        );
        let id = device.id();
        assert_eq!(
            code(device.service.client().current_keys(workspace, id).await),
            "forbidden"
        );
    }
    laptop_folder.write("After.md", b"Written after the removal.\n");
    for (sync, service, folder) in [
        (&mut laptop_sync, &mut ben.service, &laptop_folder),
        (&mut phone_sync, &mut phone.service, &phone_folder),
    ] {
        sync.round(service, 0)
            .await
            .expect("the removal isn't an error");
        let ended = sync.status().ended.expect("sync ended");
        assert_eq!(ended.revision, revision);
        assert_eq!(ended.by_device, ana.desktop.id());
        assert_eq!(ended.by_account, Some(account(&ana.desktop)));
        assert!(ended.account_removed);
        let line = FolderSyncScreen::of(&sync.status(), false).ended.unwrap();
        assert!(
            line.contains("removed your account") && line.contains("Your files stay"),
            "{line}"
        );
        assert_eq!(folder.files()["Plan.md"], b"# Plan\n", "files stay");
    }
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    assert!(!ana.folder.files().contains_key("After.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_member_takes_every_device_out_and_rotates_on_sqlite() {
    removal(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_member_takes_every_device_out_and_rotates_on_postgres() {
    removal(Server::start_postgres().await).await;
}

/// A member that leaves can't rotate a domain it no longer reads, so
/// another key holder rotates it (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_who_leaves_is_rotated_out_by_another_key_holder() {
    let server = Server::start().await;
    let mut ana = owner(&server).await;
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let (mut ben_sync, _folder) = ana.invite(&mut ben, Role::Editor).await;
    let before = ana.remote().epoch();
    let own = account(&ben);
    ben_sync
        .change_member(&mut ben.service, MemberChange::Remove(own))
        .await
        .expect("Ben leaves");
    let ended = ben_sync.status().ended;
    assert!(
        ended.is_none() || ended.is_some_and(|e| e.account_removed),
        "leaving ends sync as a removal"
    );
    let remote = ana.sync.remote_mut().unwrap();
    assert!(
        ana.desktop
            .service
            .rotate_if_pending(remote)
            .await
            .expect("rotated")
    );
    assert_eq!(remote.epoch(), before + 1);
}

/// A member removed and invited again comes back with its account's
/// cards, which keep their card indices and continue their device chains
/// (§8, §17).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_invited_again_keeps_its_card_indices() {
    let server = Server::start().await;
    let mut ana = owner(&server).await;
    let workspace = ana.sync.workspace();
    let domain = ana.sync.domain();
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let (mut ben_sync, ben_folder) = ana.invite(&mut ben, Role::Editor).await;
    ben_folder.write("Ben.md", b"First visit.\n");
    ben_sync.round(&mut ben.service, 0).await.expect("a round");
    let index = ana.remote().policy().head_snapshot().card_indices[&ben.id()];
    let id = ben.id();
    let counter = ben
        .service
        .client()
        .chain_head(workspace, domain, id)
        .await
        .expect("the head")
        .expect("Ben wrote")
        .counter;

    ana.change(MemberChange::Remove(account(&ben)))
        .await
        .expect("removed");
    ben_sync.round(&mut ben.service, 0).await.expect("a round");
    assert!(ben_sync.status().ended.is_some());
    drop(ben_sync);

    let (mut again, again_folder) = ana.invite(&mut ben, Role::Viewer).await;
    let snapshot = ana.remote().policy().head_snapshot().clone();
    assert_eq!(snapshot.card_indices[&ben.id()], index, "the same index");
    assert_eq!(snapshot.card_indices.len(), 2, "no new index");
    assert_eq!(snapshot.role(&account(&ben)), Some(Role::Viewer));
    assert_eq!(again_folder.files()["Ben.md"], b"First visit.\n");

    // Made an editor again, the device continues its chain.
    ana.change(MemberChange::SetRole {
        account: account(&ben),
        role: Role::Editor,
    })
    .await
    .expect("an editor again");
    again_folder.write("Ben.md", b"Second visit.\n");
    again.round(&mut ben.service, 0).await.expect("a round");
    again.round(&mut ben.service, 0).await.expect("a round");
    let head = ben
        .service
        .client()
        .chain_head(workspace, domain, id)
        .await
        .expect("the head")
        .expect("Ben wrote");
    assert!(head.counter > counter, "the chain continues");
    ana.sync
        .round(&mut ana.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(ana.folder.files()["Ben.md"], b"Second visit.\n");
}
