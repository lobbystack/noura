//! The Members section and the invitation screens through the desktop's
//! `LiveAccount` against the real server (seam 1 and seam 4, sync-v2.md
//! §10, §11): an owner invites from Members, the invitee pastes the link,
//! signs up and accepts, both people compare the code, the owner approves,
//! and the new member downloads the workspace.

mod sync_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use app_core::account::live::LiveAccount;
use app_core::account::{AccountAction, AccountBackend, AccountPhase, InvitationStage};
use app_core::folder_sync::{FolderSyncAction, FolderSyncBackend, FolderSyncPhase};
use app_core::members::{MembersAction, MembersBackend, MembersScreen, Role};
use app_core::sync::invitations::INVITATION_UNUSABLE;
use sync_protocol::types::Id;
use sync_store_sqlite::SqliteBackend;
use sync_support::{Folder, MemorySecrets, Server};

fn wait<T: std::fmt::Debug>(what: &str, read: impl Fn() -> T, done: impl Fn(&T) -> bool) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let value = read();
        if done(&value) {
            return value;
        }
        assert!(Instant::now() < deadline, "{what} never came: {value:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn account(data: &tempfile::TempDir) -> LiveAccount {
    LiveAccount::start(
        data.path().to_owned(),
        Arc::new(MemorySecrets::default()),
        Arc::new(|_| Ok(())),
    )
}

/// Approves the browser sign-in on `live`'s screen as `account`.
fn approve_sign_in(
    runtime: &tokio::runtime::Runtime,
    server: &Server<SqliteBackend>,
    live: &LiveAccount,
    account: Id,
) {
    let screen = wait(
        "the sign-in code",
        || AccountBackend::screen(live),
        |s| matches!(s.phase, AccountPhase::SigningIn { .. }),
    );
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    runtime
        .block_on(server.store.decide_device_grant(account, &user_code, true))
        .unwrap();
}

fn members(live: &LiveAccount) -> MembersScreen {
    MembersBackend::screen(live)
}

#[test]
fn a_person_joins_from_a_pasted_link_through_the_approval() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let ana_id = runtime.block_on(server.person("ana@example.com"));
    let ben_id = runtime.block_on(server.person("ben@example.com"));

    // Ana syncs a workspace from her desktop.
    let ana_data = tempfile::tempdir().unwrap();
    let ana = account(&ana_data);
    AccountBackend::act(&ana, AccountAction::SetServer(server.base.clone()));
    wait(
        "the address",
        || AccountBackend::screen(&ana),
        |s| s.server == server.base,
    );
    AccountBackend::act(&ana, AccountAction::SignIn);
    approve_sign_in(&runtime, &server, &ana, ana_id);
    wait(
        "Ana's account",
        || AccountBackend::screen(&ana),
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );
    let folder = Folder::new("Field notes");
    folder.write("Plan.md", b"# Plan\n");
    FolderSyncBackend::attach(&ana, Some(folder.engine.clone()));
    FolderSyncBackend::act(&ana, FolderSyncAction::TurnOn);
    wait(
        "the upload",
        || FolderSyncBackend::screen(&ana),
        |s| s.phase == FolderSyncPhase::On && s.pending == 0 && s.last_success.is_some(),
    );

    MembersBackend::act(&ana, MembersAction::Refresh);
    let screen = wait(
        "the section",
        || members(&ana),
        |s| s.you == Some(Role::Owner),
    );
    assert_eq!(screen.members.len(), 1);
    MembersBackend::act(
        &ana,
        MembersAction::Invite {
            role: Role::Editor,
            your_name: "Ana".into(),
        },
    );
    let link = wait("the link", || members(&ana), |s| s.copy.is_some())
        .copy
        .unwrap()
        .0
        .to_string();
    MembersBackend::act(&ana, MembersAction::Copied);

    // Ben pastes it on a computer with no account. A link whose secret
    // changed opens nothing, and says so plainly.
    let ben_data = tempfile::tempdir().unwrap();
    let ben = account(&ben_data);
    let mut tampered = link.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    AccountBackend::act(&ben, AccountAction::OpenInvitation(tampered));
    let screen = wait(
        "the generic error",
        || AccountBackend::screen(&ben),
        |s| s.problem.is_some(),
    );
    assert_eq!(screen.problem.as_deref(), Some(INVITATION_UNUSABLE));
    assert!(screen.invitation.is_none());

    AccountBackend::act(&ben, AccountAction::OpenInvitation(link));
    let screen = wait(
        "the invitation",
        || AccountBackend::screen(&ben),
        |s| s.invitation.is_some(),
    );
    let view = screen.invitation.unwrap();
    assert_eq!(
        (
            view.workspace.as_str(),
            view.inviter.as_str(),
            view.role.as_str()
        ),
        ("Field notes", "Ana", "editor")
    );
    assert_eq!(view.stage, InvitationStage::Opened);

    // Accepting signs Ben up first, and the request follows by itself.
    AccountBackend::act(&ben, AccountAction::AcceptInvitation);
    approve_sign_in(&runtime, &server, &ben, ben_id);
    wait(
        "the request",
        || AccountBackend::screen(&ben),
        |s| {
            matches!(
                s.invitation.as_ref().map(|i| &i.stage),
                Some(InvitationStage::Waiting { .. })
            )
        },
    );

    // Ana sees the request, offers, and both screens show one code.
    let screen = wait(
        "Ben's request",
        || {
            MembersBackend::act(&ana, MembersAction::Refresh);
            std::thread::sleep(Duration::from_millis(200));
            members(&ana)
        },
        |s| !s.requests.is_empty(),
    );
    assert_eq!(screen.requests.len(), 1);
    assert_eq!(screen.requests[0].returning, None, "Ben never was a member");
    MembersBackend::act(&ana, MembersAction::Approve(screen.requests[0].commit));
    let code = |s: &app_core::account::AccountScreen| match s.invitation.as_ref().map(|i| &i.stage)
    {
        Some(InvitationStage::Waiting { code, .. }) => code.clone(),
        _ => None,
    };
    let shown = wait(
        "Ben's code",
        || AccountBackend::screen(&ben),
        |s| code(s).is_some(),
    );
    let on_ana = wait(
        "Ana's code",
        || members(&ana),
        |s| s.approval.as_ref().is_some_and(|a| a.code.is_some()),
    );
    assert_eq!(on_ana.approval.unwrap().code, code(&shown));
    assert_eq!(on_ana.members.len(), 1, "nothing committed yet");

    AccountBackend::act(&ben, AccountAction::ConfirmInvitationCode);
    MembersBackend::act(&ana, MembersAction::ConfirmApproval);
    let screen = wait(
        "the new member",
        || members(&ana),
        |s| s.approval.is_none() && s.members.len() == 2,
    );
    assert!(screen.requests.is_empty());
    assert!(screen.invitations.is_empty(), "the invitation ended");
    assert!(screen.notice.unwrap().contains("added the new member"));

    // Ben chooses a folder, and the workspace downloads into it.
    wait(
        "the join",
        || AccountBackend::screen(&ben),
        |s| {
            matches!(
                s.invitation.as_ref().map(|i| &i.stage),
                Some(InvitationStage::Joined { downloading: false })
            )
        },
    );
    let copy = Folder::new("Copy");
    AccountBackend::act(
        &ben,
        AccountAction::DownloadInvited(copy.engine.root().to_owned()),
    );
    FolderSyncBackend::attach(&ben, Some(copy.engine.clone()));
    wait(
        "the invitation to close",
        || AccountBackend::screen(&ben),
        |s| s.invitation.is_none(),
    );
    wait(
        "the file",
        || copy.files(),
        |files| files.contains_key("Plan.md"),
    );

    // A role change goes through the same backend, and Ana's section
    // lists Ben as a viewer.
    let ben_row = members(&ana)
        .members
        .iter()
        .find(|m| !m.you)
        .expect("Ben")
        .account;
    MembersBackend::act(
        &ana,
        MembersAction::SetRole {
            account: ben_row,
            role: Role::Viewer,
        },
    );
    wait(
        "the role",
        || members(&ana),
        |s| {
            s.members
                .iter()
                .any(|m| m.account == ben_row && m.role == Role::Viewer)
        },
    );
}

#[derive(Default)]
struct Waiting(std::sync::Mutex<Option<String>>);

impl FolderSyncBackend for Waiting {
    fn screen(&self) -> app_core::folder_sync::FolderSyncScreen {
        app_core::folder_sync::FolderSyncScreen {
            attention: self.0.lock().unwrap().clone(),
            ..Default::default()
        }
    }
    fn act(&self, _action: FolderSyncAction) {}
    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
    fn attach(&self, _engine: Option<Arc<app_core::folder_sync::WorkspaceEngine>>) {}
}

/// What waits for you in Members reaches the window once, wherever you
/// are, and comes back only when something new waits.
#[test]
fn the_window_says_once_what_sync_waits_for() {
    let data = tempfile::tempdir().unwrap();
    let mut app = app_core::App::launch(data.path());
    let backend = Arc::new(Waiting::default());
    app.set_folder_sync_backend(backend.clone());
    app.poll();
    assert!(app.notice().is_none());

    *backend.0.lock().unwrap() = Some("Files wait for you.".into());
    app.poll();
    assert_eq!(
        app.notice().map(|n| n.message.as_str()),
        Some("Files wait for you.")
    );
    app.dismiss_notice();
    app.poll();
    assert!(app.notice().is_none(), "it says so once");

    *backend.0.lock().unwrap() = None;
    app.poll();
    *backend.0.lock().unwrap() = Some("Files wait for you.".into());
    app.poll();
    assert!(app.notice().is_some(), "something new waits");
}
