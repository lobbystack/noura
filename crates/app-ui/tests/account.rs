//! Settings' Sync account section through AccessKit (sync-v2.md §7, §10,
//! §12): the server address with its HTTPS rule, sign-in, the account's
//! fingerprint, every device with its full fingerprint, and the approve,
//! remove, export, import and sign-out actions, with the keyboard path
//! through an approval on both devices.

mod support;

use std::sync::{Arc, Mutex};

use app_core::account::{
    AccountAction, AccountBackend, AccountPhase, AccountScreen, ApprovalView, DeviceEntry, Id,
    KitView, RequestEntry, SignOutView, SignedInView,
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
        phase: AccountPhase::SignedIn(view),
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
            secret: "ABCD EFGH IJKL MNOP QRST UVWX YZ23 4567".into(),
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
