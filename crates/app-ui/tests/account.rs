//! Settings' Sync account section through AccessKit (sync-v2.md §7, §10,
//! §12): the server address with its HTTPS rule, sign-in, the account's
//! fingerprint, every device with its full fingerprint, and the approve,
//! remove, export, import and sign-out actions, with the keyboard path
//! through an approval on both devices.

mod support;

use std::sync::{Arc, Mutex};

use std::collections::BTreeSet;

use app_core::account::{
    AccountAction, AccountBackend, AccountPhase, AccountScreen, AlertEntry, ApprovalView,
    DeviceEntry, Id, InvitationStage, InvitationView, KitReplacementView, KitView, RecoveryView,
    RequestEntry, SignOutView, SignedInView,
};
use app_core::sync::recovery::KIT_WARNING;
use app_core::{App, settings_section};
use app_ui::NouraUi;
use egui::Key;
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::FakePicker;

/// An account backend whose screen the test sets and that records what
/// you do.
#[derive(Default)]
struct FakeAccount {
    screen: Mutex<AccountScreen>,
    actions: Mutex<Vec<AccountAction>>,
}

impl FakeAccount {
    fn show(&self, screen: AccountScreen) {
        *self.screen.lock().unwrap() = screen;
    }

    fn taken(&self) -> Vec<AccountAction> {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl AccountBackend for FakeAccount {
    fn screen(&self) -> AccountScreen {
        self.screen.lock().unwrap().clone()
    }

    fn act(&self, action: AccountAction) {
        self.actions.lock().unwrap().push(action);
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}

const LAPTOP: &str =
    "0a1b 2c3d 4e5f 6071 8293 a4b5 c6d7 e8f9 0a1b 2c3d 4e5f 6071 8293 a4b5 c6d7 e8f9";
const PHONE: &str =
    "1111 2222 3333 4444 5555 6666 7777 8888 9999 aaaa bbbb cccc dddd eeee ffff 0000";
const ACCOUNT: &str =
    "abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd abcd";

fn harness(app_data: &std::path::Path, account: Arc<FakeAccount>) -> Harness<'static, NouraUi> {
    harness_with(app_data, account, FakePicker::default())
}

fn harness_with(
    app_data: &std::path::Path,
    account: Arc<FakeAccount>,
    picker: FakePicker,
) -> Harness<'static, NouraUi> {
    let mut app = App::launch(app_data);
    app.set_account_backend(account);
    app.open_settings_at(settings_section::SYNC_ACCOUNT);
    let ui = NouraUi::new(app, Box::new(picker));
    let mut harness = Harness::builder()
        .with_size([1000.0, 900.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    harness.run();
    harness
}

fn signed_in(view: SignedInView) -> AccountScreen {
    AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::SignedIn(Box::new(view)),
        ..AccountScreen::default()
    }
}

fn devices() -> Vec<DeviceEntry> {
    vec![
        DeviceEntry {
            device: Id([1; 16]),
            fingerprint: LAPTOP.into(),
            how: "Started the account".into(),
            this_device: true,
            recovery: false,
            removed: false,
        },
        DeviceEntry {
            device: Id([2; 16]),
            fingerprint: PHONE.into(),
            how: "Approved by 0a1b 2c3d".into(),
            this_device: false,
            recovery: false,
            removed: false,
        },
    ]
}

/// A kit whose head a restore lost leads to Reset devices (§14).
#[test]
fn a_kit_the_server_lost_offers_reset_devices() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::KitLost,
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.run();
    harness.get_by_role_and_label(Role::Group, "This kit can't recover your account");
    harness
        .get_by_role_and_label(Role::Button, "Reset devices")
        .click();
    harness.run();
    assert!(matches!(
        account.taken().last(),
        Some(AccountAction::StartReset)
    ));
}

#[test]
fn a_stopped_chain_offers_reset_devices_and_a_retry() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::ChainStopped,
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.run();
    harness.get_by_role_and_label(Role::Group, "Sync stopped for your account");
    harness
        .get_by_role_and_label(Role::Button, "Reset devices")
        .click();
    harness.run();
    assert!(matches!(
        account.taken().last(),
        Some(AccountAction::StartReset)
    ));
    harness
        .get_by_role_and_label(Role::Button, "Try again")
        .click();
    harness.run();
    assert!(matches!(account.taken().last(), Some(AccountAction::Retry)));
}

#[test]
fn signed_out_shows_the_server_address_sign_in_and_recovery() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(AccountScreen {
        server: "http://sync.example.com".into(),
        server_problem: Some("The server's address must start with https://.".into()),
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_role_and_label(Role::TextInput, "Server address");
    harness.get_by_role_and_label(
        Role::Alert,
        "The server's address must start with https://.",
    );
    let sign_in = harness.get_by_role_and_label(Role::Button, "Sign in with your browser");
    assert!(
        sign_in.accesskit_node().is_disabled(),
        "plain HTTP away from loopback"
    );
    harness.get_by_role_and_label(Role::TextInput, "Kit file");
    harness.get_by_role_and_label(Role::PasswordInput, "Kit secret");
    harness.get_by_role_and_label(Role::Button, "Recover");

    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        ..AccountScreen::default()
    });
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Sign in with your browser")
        .click_accesskit();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::SignIn]);
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn signed_in_shows_the_account_its_devices_and_their_actions() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        ..SignedInView::default()
    }));
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_label("https://sync.example.com");
    harness.get_by_label(ACCOUNT);
    harness.get_by_label(LAPTOP);
    harness.get_by_label(PHONE);
    harness.get_by_label(KIT_WARNING);
    let removes = harness
        .query_all_by_role_and_label(Role::Button, "Remove")
        .count();
    assert_eq!(removes, 1, "only another device has Remove");
    harness
        .get_by_role_and_label(Role::Button, "Remove")
        .click_accesskit();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Sign out")
        .click_accesskit();
    harness.run();
    assert_eq!(
        account.taken(),
        vec![AccountAction::Remove(Id([2; 16])), AccountAction::SignOut]
    );
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn after_a_removal_the_removed_devices_changes_show_with_their_undo() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    let kept = "Device 1111 2222 made a change (set_role) in workspace 0a0b0c0d, and it stays. Your role can undo it with set_role.";
    let stuck = "Device 1111 2222 made a change (transfer_owner) in workspace 0a0b0c0d, and it stays. Your role can't undo it.";
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        removed_changes: vec![kept.into(), stuck.into()],
        offer_kit: true,
        ..SignedInView::default()
    }));
    let harness = harness(app_data.path(), account);
    harness.get_by_role_and_label(Role::Group, "Changes your removed devices made");
    harness.get_by_label(kept);
    harness.get_by_label(stuck);
    harness.get_by_label("Your devices changed. Export a new kit so it covers them.");
}

#[test]
fn the_device_list_alerts_and_signs_out_a_device_nobody_approved() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    let unknown = format!("A device that nobody approved signed in to your account: {PHONE}.");
    let changed = "Device 1111 2222 changed workspace 0a0b0c0d at revision 4 (set_role).";
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        alerts: vec![
            AlertEntry {
                text: unknown.clone(),
                end_sessions: Some(Id([9; 16])),
                check_again: None,
            },
            AlertEntry {
                text: changed.into(),
                end_sessions: None,
                check_again: None,
            },
        ],
        ..SignedInView::default()
    }));
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_role_and_label(Role::Alert, &unknown);
    harness.get_by_role_and_label(Role::Alert, changed);
    let sign_outs = harness
        .query_all_by_role_and_label(Role::Button, "Sign that device out")
        .count();
    assert_eq!(sign_outs, 1, "only the unknown card's alert offers it");
    harness
        .get_by_role_and_label(Role::Button, "Sign that device out")
        .click_accesskit();
    harness.run();
    assert_eq!(
        account.taken(),
        vec![AccountAction::EndSessions(Id([9; 16]))]
    );
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn the_keyboard_reaches_approve_and_codes_match_on_both_devices() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        requests: vec![RequestEntry {
            fingerprint: PHONE.into(),
        }],
        ..SignedInView::default()
    }));
    let mut harness = harness(app_data.path(), account.clone());
    assert!(harness.query_all_by_label(PHONE).next().is_some());
    let mut reached = false;
    for _ in 0..40 {
        harness.key_press(Key::Tab);
        harness.run();
        if harness
            .get_by_role_and_label(Role::Button, "Approve")
            .is_focused()
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "Tab reaches Approve");
    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::Approve(0)]);

    // The new device revealed its nonce: both screens show one code.
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        approval: Some(ApprovalView {
            fingerprint: PHONE.into(),
            code: Some("0123 4567".into()),
        }),
        ..SignedInView::default()
    }));
    harness.run();
    harness.get_by_label("0123 4567");
    let mut reached = false;
    for _ in 0..40 {
        harness.key_press(Key::Tab);
        harness.run();
        if harness
            .get_by_role_and_label(Role::Button, "Codes match")
            .is_focused()
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "Tab reaches Codes match");
    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::ConfirmApproval]);

    // On the new device.
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::WaitingForApproval {
            fingerprint: PHONE.into(),
            code: Some("0123 4567".into()),
            confirmed: false,
        },
        ..AccountScreen::default()
    });
    harness.run();
    harness.get_by_label(PHONE);
    harness.get_by_label("0123 4567");
    let mut reached = false;
    for _ in 0..40 {
        harness.key_press(Key::Tab);
        harness.run();
        if harness
            .get_by_role_and_label(Role::Button, "Codes match")
            .is_focused()
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "Tab reaches Codes match on the new device");
    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::ConfirmCode]);
}

#[test]
fn the_export_screen_asks_you_to_keep_the_kit_and_secret_apart() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        offer_kit: true,
        ..SignedInView::default()
    }));
    let picker = FakePicker::default();
    picker.answer(Some(folder.path()));
    let mut harness = harness_with(app_data.path(), account.clone(), picker);
    harness.get_by_label(KIT_WARNING);
    harness.get_by_label("Your devices changed. Export a new kit so it covers them.");
    harness
        .get_by_role_and_label(Role::Button, "Export recovery kit")
        .click_accesskit();
    harness.run();
    assert_eq!(
        account.taken(),
        vec![AccountAction::ExportKit(
            folder.path().join("Noura recovery kit.noura-kit")
        )]
    );

    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        kit: Some(KitView {
            path: folder.path().join("Noura recovery kit.noura-kit"),
            secret: zeroize::Zeroizing::new("ABCD EFGH IJKL MNOP QRST UVWX YZ23 4567".into()),
        }),
        ..SignedInView::default()
    }));
    harness.run();
    harness.get_by_label("Write this secret down now. Noura won't show it again:");
    harness.get_by_label("ABCD EFGH IJKL MNOP QRST UVWX YZ23 4567");
    harness
        .get_by_role_and_label(Role::Button, "I wrote it down")
        .click_accesskit();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::DismissKit]);
}

#[test]
fn signing_out_the_last_device_warns_first() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices()[..1].to_vec(),
        sign_out: Some(SignOutView { last_device: true }),
        ..SignedInView::default()
    }));
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_role_and_label(
        Role::Alert,
        "This is your account's last device. After you sign out, you need your recovery kit, or a reset of your devices, to read your synced data again.",
    );
    harness
        .get_by_role_and_label(Role::Button, "Stay signed in")
        .click_accesskit();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::CancelSignOut]);
}

#[test]
fn a_device_that_couldnt_connect_offers_to_try_again() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    let problem = "This computer's session with the sync server ended. Sign in again with your browser, and this device keeps its keys.";
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        problem: Some(problem.into()),
        can_retry: true,
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_role_and_label(Role::Alert, problem);
    harness
        .get_by_role_and_label(Role::Button, "Try again")
        .click_accesskit();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Sign in with your browser")
        .click_accesskit();
    harness.run();
    assert_eq!(
        account.taken(),
        vec![AccountAction::Retry, AccountAction::SignIn]
    );
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn a_removed_computer_signs_in_as_a_new_device() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::Removed { by: LAPTOP.into() },
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_label(LAPTOP);
    harness
        .get_by_role_and_label(Role::Button, "Sign in as a new device")
        .click_accesskit();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::SignIn]);
}

#[test]
fn the_recovery_review_shows_members_unverified_workspaces_and_your_devices() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    let change = "Your account: add_device by device 22222222.";
    let members = "Workspace 33333333 has these members: your account, account 55555555.";
    let unverified = "Noura couldn't verify workspace 66666666 (sync_workspace_unverified), so the recovery leaves it out.";
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::Recovering(RecoveryView {
            changes: vec![change.into()],
            members: vec![members.into()],
            unverified: vec![unverified.into()],
            devices: vec![(Id([2; 16]), PHONE.into())],
            keep: BTreeSet::new(),
        }),
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_label(change);
    harness.get_by_role_and_label(Role::Group, "Workspaces your kit doesn't list");
    harness.get_by_label(members);
    harness.get_by_role_and_label(Role::Alert, unverified);
    harness
        .get_by_role_and_label(Role::CheckBox, &format!("Keep {PHONE}"))
        .click_accesskit();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Recover this account")
        .click_accesskit();
    harness.run();
    assert_eq!(
        account.taken(),
        vec![
            AccountAction::ToggleKeep(Id([2; 16])),
            AccountAction::ConfirmRecovery
        ]
    );
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn replacing_a_kit_lists_the_devices_it_recovered_with_keep_choices() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    let added = "Account 77777777 joined workspace 33333333 after you exported the kit.";
    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        added_members: vec![added.into()],
        kit_replacement: Some(KitReplacementView {
            path: app_data.path().join("Noura recovery kit.noura-kit"),
            devices: vec![(Id([2; 16]), PHONE.into())],
            keep: BTreeSet::new(),
        }),
        ..SignedInView::default()
    }));
    let mut harness = harness(app_data.path(), account.clone());
    harness.get_by_role_and_label(Role::Group, "Members added since your kit");
    harness.get_by_label(added);
    harness.get_by_role_and_label(Role::Group, "Replace your recovery kit");
    harness
        .get_by_role_and_label(Role::CheckBox, &format!("Keep {PHONE}"))
        .click_accesskit();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Export the new kit")
        .click_accesskit();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Keep the current kit")
        .click_accesskit();
    harness.run();
    assert_eq!(
        account.taken(),
        vec![
            AccountAction::ToggleKeep(Id([2; 16])),
            AccountAction::ConfirmExportKit,
            AccountAction::CancelExportKit
        ]
    );
    assert!(harness.unnamed_widgets().is_empty());
}

fn invitation(stage: InvitationStage) -> Option<InvitationView> {
    Some(InvitationView {
        workspace: "Field notes".into(),
        inviter: "Ana".into(),
        role: "editor".into(),
        stage,
    })
}

const LINK: &str = "https://sync.example.com/invite/AAAA#BBBB";

/// The invitee's path through the section: paste the link, read who
/// invited you, sign in and accept, compare the code, choose a folder.
#[test]
fn an_invitee_pastes_a_link_accepts_and_compares_the_code() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    let picker = FakePicker::default();
    let folder = tempfile::tempdir().unwrap();
    picker.answer(Some(folder.path()));
    let mut harness = harness_with(app_data.path(), account.clone(), picker);

    harness.get_by_label("Invitation link").focus();
    harness.run();
    harness.get_by_label("Invitation link").type_text(LINK);
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Open link")
        .click();
    harness.run();
    assert_eq!(
        account.taken().last(),
        Some(&AccountAction::OpenInvitation(LINK.into()))
    );

    // A wrong secret or a tampered context says one generic thing.
    account.show(AccountScreen {
        problem: Some(app_core::sync::invitations::INVITATION_UNUSABLE.into()),
        ..AccountScreen::default()
    });
    harness.run();
    harness.get_by_role_and_label(
        Role::Alert,
        "This invitation link doesn't work. Ask the person who invited you for a new one.",
    );

    account.show(AccountScreen {
        invitation: invitation(InvitationStage::Opened),
        ..AccountScreen::default()
    });
    harness.run();
    harness.get_by_label("Ana invited you to Field notes as an editor.");
    harness
        .get_by_role_and_label(Role::Button, "Sign in and accept")
        .click();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::AcceptInvitation]);

    account.show(AccountScreen {
        invitation: invitation(InvitationStage::Opened),
        ..signed_in(SignedInView::default())
    });
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Sign in and accept")
            .is_none()
    );
    harness.get_by_role_and_label(Role::Button, "Accept");

    account.show(AccountScreen {
        invitation: invitation(InvitationStage::Waiting {
            fingerprint: LAPTOP.into(),
            code: None,
            confirmed: false,
        }),
        ..signed_in(SignedInView::default())
    });
    harness.run();
    harness.get_by_label("Waiting for Ana to approve this computer.");
    harness.get_by_label(LAPTOP);

    account.show(AccountScreen {
        invitation: invitation(InvitationStage::Waiting {
            fingerprint: LAPTOP.into(),
            code: Some("1234 5678".into()),
            confirmed: false,
        }),
        ..signed_in(SignedInView::default())
    });
    harness.run();
    harness.get_by_label("Compare this code with Ana on a call or another channel you trust:");
    harness.get_by_label("1234 5678");
    harness
        .get_by_role_and_label(Role::Button, "Codes match")
        .click();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::ConfirmInvitationCode]);

    account.show(AccountScreen {
        invitation: invitation(InvitationStage::Joined { downloading: false }),
        ..signed_in(SignedInView::default())
    });
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Choose a folder")
        .click();
    for _ in 0..200 {
        harness.step();
        if folder.path().join(".noura").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        account.taken(),
        vec![AccountAction::DownloadInvited(folder.path().to_owned())]
    );
    let names: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![".noura"],
        "the folder stays empty for the download"
    );
}

/// A computer whose chain a reset retired says so and offers to come back
/// as a new device.
#[test]
fn a_retired_device_shows_the_new_fingerprint_and_signs_in_again() {
    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::Retired {
            fingerprint: ACCOUNT.into(),
        },
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.run();
    assert!(
        harness
            .query_all_by_label("Your account's devices were reset")
            .next()
            .is_some()
    );
    harness.get_by_label(ACCOUNT);
    harness
        .get_by_role_and_label(Role::Button, "Sign in as a new device")
        .click_accesskit();
    harness.run();
    assert_eq!(account.taken(), vec![AccountAction::SignIn]);
}

/// Reset devices: the code field, then the new fingerprint with each
/// workspace that has to accept it, and the acceptance on another account's
/// device.
#[test]
fn the_reset_screens_ask_for_the_code_and_show_what_comes_next() {
    use app_core::account::ResetWait;

    let app_data = tempfile::tempdir().unwrap();
    let account = Arc::new(FakeAccount::default());
    account.show(AccountScreen {
        server: "https://sync.example.com".into(),
        phase: AccountPhase::ResetCode,
        ..AccountScreen::default()
    });
    let mut harness = harness(app_data.path(), account.clone());
    harness.run();
    assert!(
        harness
            .query_by_label("Start over with this code")
            .is_some()
    );

    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        reset_fingerprint: Some(ACCOUNT.into()),
        reset_waits: vec![ResetWait {
            workspace: "66666666".into(),
            code: Some("1234 5678".into()),
            confirmed: false,
            set_up_again: true,
        }],
        ..SignedInView::default()
    }));
    harness.run();
    assert!(harness.query_by_label("Workspace 66666666").is_some());
    harness.get_by_label("Codes match").click();
    harness.run();
    assert!(
        account
            .taken()
            .iter()
            .any(|action| matches!(action, AccountAction::ConfirmResetWait(0)))
    );
    harness.get_by_label("Set up sync again").click();
    harness.run();
    assert!(
        account
            .taken()
            .iter()
            .any(|action| matches!(action, AccountAction::SetUpAfterReset(0)))
    );

    account.show(signed_in(SignedInView {
        account_fingerprint: ACCOUNT.into(),
        devices: devices(),
        reset_requests: vec![RequestEntry {
            fingerprint: PHONE.into(),
        }],
        alerts: vec![AlertEntry {
            text: "The devices of account 11111111 changed.".into(),
            end_sessions: None,
            check_again: Some(Id([9; 16])),
        }],
        ..SignedInView::default()
    }));
    harness.run();
    harness.get_by_label("Check again").click();
    harness.run();
    assert!(
        account
            .taken()
            .iter()
            .any(|action| matches!(action, AccountAction::CheckChainAgain(_)))
    );
    assert!(
        harness
            .query_all_by_label("The devices of account 11111111 changed.")
            .next()
            .is_some()
    );
    harness.get_by_label("Accept this reset").click();
    harness.run();
    assert!(
        account
            .taken()
            .iter()
            .any(|action| matches!(action, AccountAction::AcceptReset(0)))
    );
}
