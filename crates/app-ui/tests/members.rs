//! Settings' Members section through AccessKit (sync-v2.md §8, §11): a
//! signed-out computer says members need an account; otherwise the section
//! lists the members with the role changes and removal your role allows,
//! and the open invitations with their role and status, Copy link and
//! Revoke. A row leaves once the invitation ends, and removing a member
//! asks first and says that copies they have stay with them.

mod support;

use std::sync::{Arc, Mutex};

use app_core::members::{
    Id, InvitationEntry, InvitationStatus, LinkText, MemberEntry, MembersAction, MembersBackend,
    MembersScreen, NEEDS_ACCOUNT, Role,
};
use app_core::{App, settings_section};
use app_ui::NouraUi;
use egui::accesskit;
use egui_kittest::{Harness, kittest::Queryable};
use support::FakePicker;

#[derive(Default)]
struct FakeMembers {
    screen: Mutex<MembersScreen>,
    actions: Mutex<Vec<MembersAction>>,
}

impl FakeMembers {
    fn show(&self, screen: MembersScreen) {
        *self.screen.lock().unwrap() = screen;
    }

    fn taken(&self) -> Vec<MembersAction> {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl MembersBackend for FakeMembers {
    fn screen(&self) -> MembersScreen {
        self.screen.lock().unwrap().clone()
    }

    fn act(&self, action: MembersAction) {
        if action == MembersAction::Copied {
            self.screen.lock().unwrap().copy = None;
        }
        self.actions.lock().unwrap().push(action);
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}

fn harness(app_data: &std::path::Path, members: Arc<FakeMembers>) -> Harness<'static, NouraUi> {
    let mut app = App::launch(app_data);
    app.set_members_backend(members);
    app.open_settings_at(settings_section::MEMBERS);
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = Harness::builder()
        .with_size([1000.0, 900.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    harness.run();
    harness
}

const BEN: Id = Id([2; 16]);
const CLEO: Id = Id([3; 16]);
const PENDING: Id = Id([4; 16]);
const ACCEPTED: Id = Id([5; 16]);

fn member(account: Id, label: &str, role: Role, roles: Vec<Role>, removable: bool) -> MemberEntry {
    MemberEntry {
        account,
        label: label.into(),
        role,
        you: false,
        roles,
        removable,
        transferable: false,
    }
}

fn invitation(id: Id, label: &str, role: Role, status: InvitationStatus) -> InvitationEntry {
    InvitationEntry {
        invitation: id,
        label: label.into(),
        role,
        status,
        sent_here: true,
        returning: None,
    }
}

/// An admin's view: Ana owns the workspace, Ben edits, Cleo views, and
/// two invitations are open.
fn admin_screen() -> MembersScreen {
    MembersScreen {
        you: Some(Role::Admin),
        members: vec![
            MemberEntry {
                you: true,
                ..member(Id([1; 16]), "You, admin, 1 device", Role::Admin, vec![], false)
            },
            member(Id([9; 16]), "Account 09090909, owner, 2 devices", Role::Owner, vec![], false),
            member(BEN, "Account 02020202, editor, 1 device", Role::Editor, vec![Role::Viewer], true),
            member(CLEO, "Account 03030303, viewer, 1 device", Role::Viewer, vec![Role::Editor], true),
        ],
        invitations: vec![
            invitation(
                PENDING,
                "Editor invitation, waiting for them to accept, expires 2026-11-06 10:00",
                Role::Editor,
                InvitationStatus::Pending,
            ),
            InvitationEntry {
                sent_here: false,
                returning: Some(
                    "This account was a member before. Once you approve it, it can read what members wrote while it was away.".into(),
                ),
                ..invitation(
                    ACCEPTED,
                    "Viewer invitation, accepted, waiting for approval, expires 2026-11-07 09:30",
                    Role::Viewer,
                    InvitationStatus::Accepted,
                )
            },
        ],
        invite_roles: vec![Role::Editor, Role::Viewer],
        may_leave: true,
        ..MembersScreen::default()
    }
}

#[test]
fn a_signed_out_computer_says_members_need_an_account() {
    let app_data = tempfile::tempdir().unwrap();
    let members = Arc::new(FakeMembers::default());
    members.show(MembersScreen {
        unavailable: Some(NEEDS_ACCOUNT.into()),
        ..MembersScreen::default()
    });
    let harness = harness(app_data.path(), members.clone());
    harness.get_by_label(NEEDS_ACCOUNT);
    assert!(
        harness
            .query_by_role_and_label(accesskit::Role::Button, "Invite an editor")
            .is_none()
    );
    assert_eq!(members.taken(), vec![MembersAction::Refresh]);
}

#[test]
fn invitations_show_their_role_and_status_and_leave_once_they_end() {
    let app_data = tempfile::tempdir().unwrap();
    let members = Arc::new(FakeMembers::default());
    members.show(admin_screen());
    let mut harness = harness(app_data.path(), members.clone());
    assert_eq!(members.taken(), vec![MembersAction::Refresh]);

    let pending = harness.get_by_role_and_label(
        accesskit::Role::Group,
        "Editor invitation, waiting for them to accept, expires 2026-11-06 10:00",
    );
    pending
        .get_by_role_and_label(accesskit::Role::Button, "Copy link")
        .click();
    harness.run();
    assert_eq!(members.taken(), vec![MembersAction::CopyLink(PENDING)]);

    let accepted = harness.get_by_role_and_label(
        accesskit::Role::Group,
        "Viewer invitation, accepted, waiting for approval, expires 2026-11-07 09:30",
    );
    assert!(
        accepted
            .query_by_role_and_label(accesskit::Role::Button, "Copy link")
            .is_none(),
        "only the computer that sent it copies its link"
    );
    accepted.get_by_label("Another computer sent this one. Only it can approve the new member.");
    accepted.get_by_role_and_label(
        accesskit::Role::Alert,
        "This account was a member before. Once you approve it, it can read what members wrote while it was away.",
    );
    accepted
        .get_by_role_and_label(accesskit::Role::Button, "Revoke")
        .click();
    harness.run();
    assert_eq!(members.taken(), vec![MembersAction::Revoke(ACCEPTED)]);

    // The server stopped listing both: one was revoked, the other
    // approved or expired.
    members.show(MembersScreen {
        invitations: Vec::new(),
        notice: Some("You revoked the invitation. Its link no longer opens.".into()),
        ..admin_screen()
    });
    harness.run();
    harness.get_by_label("You revoked the invitation. Its link no longer opens.");
    harness.get_by_label("No open invitations.");
    assert!(
        harness
            .query_by_role_and_label(
                accesskit::Role::Group,
                "Editor invitation, waiting for them to accept, expires 2026-11-06 10:00",
            )
            .is_none()
    );
}

#[test]
fn an_admin_invites_editors_and_viewers_with_a_name() {
    let app_data = tempfile::tempdir().unwrap();
    let members = Arc::new(FakeMembers::default());
    members.show(admin_screen());
    let mut harness = harness(app_data.path(), members.clone());
    members.taken();
    assert!(
        harness
            .query_by_role_and_label(accesskit::Role::Button, "Invite an admin")
            .is_none(),
        "only the owner invites admins"
    );
    harness.get_by_role(accesskit::Role::TextInput).click();
    harness.run();
    harness
        .get_by_role(accesskit::Role::TextInput)
        .type_text("Cleo");
    harness.run();
    harness
        .get_by_role_and_label(accesskit::Role::Button, "Invite an editor")
        .click();
    harness.run();
    assert_eq!(
        members.taken(),
        vec![MembersAction::Invite {
            role: Role::Editor,
            your_name: "Cleo".into(),
        }]
    );

    // The new link goes to the clipboard once.
    members.show(MembersScreen {
        copy: Some(LinkText(
            "https://sync.example.com/invite/abc#def".to_owned().into(),
        )),
        ..admin_screen()
    });
    harness.run();
    assert_eq!(members.taken(), vec![MembersAction::Copied]);
}

#[test]
fn roles_change_and_a_removal_asks_first() {
    let app_data = tempfile::tempdir().unwrap();
    let members = Arc::new(FakeMembers::default());
    members.show(admin_screen());
    let mut harness = harness(app_data.path(), members.clone());
    members.taken();

    let owner =
        harness.get_by_role_and_label(accesskit::Role::Group, "Account 09090909, owner, 2 devices");
    assert!(
        owner
            .query_by_role_and_label(accesskit::Role::Button, "Remove")
            .is_none(),
        "an admin doesn't remove the owner"
    );
    harness
        .get_by_role_and_label(accesskit::Role::Group, "Account 02020202, editor, 1 device")
        .get_by_role_and_label(accesskit::Role::Button, "Make viewer")
        .click();
    harness.run();
    assert_eq!(
        members.taken(),
        vec![MembersAction::SetRole {
            account: BEN,
            role: Role::Viewer,
        }]
    );

    harness
        .get_by_role_and_label(accesskit::Role::Group, "Account 03030303, viewer, 1 device")
        .get_by_role_and_label(accesskit::Role::Button, "Remove")
        .click();
    harness.run();
    assert!(
        members.taken().is_empty(),
        "nothing goes out before you confirm"
    );
    let confirm = harness.get_by_role_and_label(accesskit::Role::Group, "Confirm");
    confirm.get_by_label(
        "Remove Account 03030303? Their devices stop syncing this workspace, and Noura changes its key. They keep copies of files they already have.",
    );
    confirm
        .get_by_role_and_label(accesskit::Role::Button, "Remove")
        .click();
    harness.run();
    assert_eq!(members.taken(), vec![MembersAction::Remove(CLEO)]);

    members.show(MembersScreen {
        members: admin_screen().members[..3].to_vec(),
        notice: Some("You removed the member. They keep copies of files they already have.".into()),
        ..admin_screen()
    });
    harness.run();
    harness.get_by_label("You removed the member. They keep copies of files they already have.");
    assert!(
        harness
            .query_by_role_and_label(accesskit::Role::Group, "Account 03030303, viewer, 1 device")
            .is_none()
    );

    // Leaving asks too.
    harness
        .get_by_role_and_label(accesskit::Role::Button, "Leave workspace")
        .scroll_to_me();
    harness.run();
    harness
        .get_by_role_and_label(accesskit::Role::Button, "Leave workspace")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(accesskit::Role::Group, "Confirm")
        .get_by_role_and_label(accesskit::Role::Button, "Leave")
        .click();
    harness.run();
    assert_eq!(members.taken(), vec![MembersAction::Leave]);
}

#[test]
fn the_owner_transfers_ownership_before_leaving() {
    let app_data = tempfile::tempdir().unwrap();
    let members = Arc::new(FakeMembers::default());
    members.show(MembersScreen {
        you: Some(Role::Owner),
        members: vec![
            MemberEntry {
                you: true,
                ..member(
                    Id([1; 16]),
                    "You, owner, 1 device",
                    Role::Owner,
                    vec![],
                    false,
                )
            },
            MemberEntry {
                transferable: true,
                ..member(
                    BEN,
                    "Account 02020202, editor, 1 device",
                    Role::Editor,
                    vec![Role::Admin, Role::Viewer],
                    true,
                )
            },
        ],
        invite_roles: vec![Role::Admin, Role::Editor, Role::Viewer],
        may_leave: false,
        ..MembersScreen::default()
    });
    let mut harness = harness(app_data.path(), members.clone());
    members.taken();
    harness.get_by_label("To leave this workspace, make another member the owner first.");
    harness.get_by_role_and_label(accesskit::Role::Button, "Invite an admin");
    let ben =
        harness.get_by_role_and_label(accesskit::Role::Group, "Account 02020202, editor, 1 device");
    ben.get_by_role_and_label(accesskit::Role::Button, "Make admin");
    ben.get_by_role_and_label(accesskit::Role::Button, "Make owner")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(accesskit::Role::Group, "Confirm")
        .get_by_role_and_label(accesskit::Role::Button, "Make owner")
        .click();
    harness.run();
    assert_eq!(members.taken(), vec![MembersAction::TransferOwner(BEN)]);
}
