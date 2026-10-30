//! Seam 1: Share Link on a note opens the share dialog. On a computer
//! without a sync account, the dialog says that sharing needs an account
//! and offers nothing else (sync-v2.md §24).

mod support;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use app_core::shares::{NEEDS_ACCOUNT, ShareAction, ShareBackend, ShareScreen};
use app_core::{CommandSource, command_id};
use support::{Fixture, open_from_tree};

#[test]
fn share_link_on_a_signed_out_computer_says_sharing_needs_an_account() {
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n\nTomatoes in May.\n")]);
    let mut app = fixture.app();
    let share = app
        .commands()
        .into_iter()
        .find(|c| c.id == command_id::SHARE_LINK)
        .expect("Share Link is a command");
    assert_eq!(share.label, "Share Link…");

    // With no note open, nothing opens.
    app.run_command(
        command_id::SHARE_LINK,
        CommandSource::Palette,
        Instant::now(),
    );
    assert!(!app.share_screen().open);

    open_from_tree(&mut app, "Plan.md");
    assert!(app.run_command(
        command_id::SHARE_LINK,
        CommandSource::Palette,
        Instant::now()
    ));
    let screen = app.share_screen();
    assert!(screen.open);
    assert_eq!(screen.note, "Plan");
    assert_eq!(screen.unavailable.as_deref(), Some(NEEDS_ACCOUNT));
    assert!(screen.links.is_empty() && screen.copy.is_none());

    app.share_action(ShareAction::Create);
    assert_eq!(
        app.share_screen().unavailable.as_deref(),
        Some(NEEDS_ACCOUNT)
    );
    app.share_action(ShareAction::Close);
    assert!(!app.share_screen().open);
}

/// A backend that records what reaches it.
#[derive(Default)]
struct Recording(Mutex<Vec<ShareAction>>);

impl ShareBackend for Recording {
    fn screen(&self) -> ShareScreen {
        ShareScreen::default()
    }

    fn act(&self, action: ShareAction) {
        self.0.lock().unwrap().push(action);
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}

#[test]
fn share_link_hands_the_backend_the_note_it_shares() {
    let fixture = Fixture::new(&[(
        "Garden/Plan.md",
        b"---\nid: note_01k0000000000000000000000p\ntype: note\n---\n\n# Plan\n",
    )]);
    let mut app = fixture.app();
    let backend = Arc::new(Recording::default());
    app.set_share_backend(backend.clone());
    open_from_tree(&mut app, "Garden/Plan.md");
    app.run_command(
        command_id::SHARE_LINK,
        CommandSource::Palette,
        Instant::now(),
    );
    let actions = backend.0.lock().unwrap().clone();
    let [ShareAction::Open { path, name, text }] = actions.as_slice() else {
        panic!("one Open action: {actions:?}");
    };
    assert_eq!(path, "Garden/Plan.md");
    assert_eq!(name, "Plan");
    assert!(text.contains("# Plan"));
}
