//! The share dialog through AccessKit (sync-v2.md §24): a signed-out
//! computer says that sharing needs an account; otherwise the dialog
//! creates and copies a link, lists the note's links with Copy, Update and
//! Revoke, and lists the workspace's links, flagging one whose creating
//! device left.

mod support;

use std::sync::{Arc, Mutex};

use app_core::App;
use app_core::shares::{
    Digest, LinkText, NEEDS_ACCOUNT, NoteLinkEntry, ShareAction, ShareBackend, ShareScreen,
    WorkspaceLinkEntry,
};
use app_ui::NouraUi;
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};
use support::FakePicker;

#[derive(Default)]
struct FakeShares {
    screen: Mutex<ShareScreen>,
    actions: Mutex<Vec<ShareAction>>,
}

impl FakeShares {
    fn taken(&self) -> Vec<ShareAction> {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl ShareBackend for FakeShares {
    fn screen(&self) -> ShareScreen {
        self.screen.lock().unwrap().clone()
    }

    fn act(&self, action: ShareAction) {
        if action == ShareAction::Copied {
            self.screen.lock().unwrap().copy = None;
        }
        self.actions.lock().unwrap().push(action);
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}

fn harness(app_data: &std::path::Path, shares: Arc<FakeShares>) -> Harness<'static, NouraUi> {
    let app = App::launch(app_data);
    app.set_share_backend(shares);
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = Harness::builder()
        .with_size([1000.0, 900.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    harness.run();
    // Every screen these tests show opens through here.
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    harness
}

#[test]
fn a_signed_out_computer_says_sharing_needs_an_account() {
    let app_data = tempfile::tempdir().unwrap();
    let shares = Arc::new(FakeShares::default());
    *shares.screen.lock().unwrap() = ShareScreen {
        open: true,
        note: "Plan".into(),
        unavailable: Some(NEEDS_ACCOUNT.into()),
        days: 30,
        ..ShareScreen::default()
    };
    let mut harness = harness(app_data.path(), shares.clone());
    harness.get_by_label(NEEDS_ACCOUNT);
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Create and copy link")
            .is_none()
    );
    harness.get_by_role_and_label(Role::Button, "Done").click();
    harness.run();
    assert_eq!(shares.taken(), vec![ShareAction::Close]);
}

#[test]
fn the_dialog_creates_copies_and_revokes_links() {
    let app_data = tempfile::tempdir().unwrap();
    let shares = Arc::new(FakeShares::default());
    let own = Digest([1; 32]);
    let orphan = Digest([2; 32]);
    *shares.screen.lock().unwrap() = ShareScreen {
        open: true,
        note: "Plan".into(),
        days: 30,
        links: vec![NoteLinkEntry {
            token_hash: own,
            expires: "2026-11-29 10:00".into(),
            revision: 2,
        }],
        workspace_links: vec![
            WorkspaceLinkEntry {
                token_hash: own,
                label: "Link 01010101 from this computer, expires 2026-11-29 10:00".into(),
                expires_at: 1,
                own: true,
                creator_removed: false,
            },
            WorkspaceLinkEntry {
                token_hash: orphan,
                label: "Link 02020202 from device abcdabcd, which left the workspace, expires 2026-11-02 08:00".into(),
                expires_at: 1,
                own: false,
                creator_removed: true,
            },
        ],
        copy: Some(LinkText("https://sync.example.com/s/token#keys".to_owned().into())),
        ..ShareScreen::default()
    };
    let mut harness = harness(app_data.path(), shares.clone());
    assert_eq!(
        shares.taken(),
        vec![ShareAction::Copied],
        "the new link is copied once"
    );
    harness.get_by_label("Share \u{201c}Plan\u{201d}");
    harness.get_by_label("Expires 2026-11-29 10:00, version 2");
    harness.get_by_label(
        "Link 02020202 from device abcdabcd, which left the workspace, expires 2026-11-02 08:00",
    );
    harness
        .get_by_role_and_label(Role::Button, "Create and copy link")
        .click();
    harness.run();
    assert_eq!(shares.taken(), vec![ShareAction::Create]);
    harness
        .get_by_role_and_label(Role::Button, "Update")
        .click();
    harness.run();
    assert_eq!(shares.taken(), vec![ShareAction::Update(own)]);
    let revokes = harness
        .get_all_by_role_and_label(Role::Button, "Revoke")
        .collect::<Vec<_>>();
    assert_eq!(revokes.len(), 3);
    revokes[2].click();
    harness.run();
    assert_eq!(shares.taken(), vec![ShareAction::Revoke(orphan)]);
}
