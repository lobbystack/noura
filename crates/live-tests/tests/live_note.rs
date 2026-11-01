//! Two desktops as people use them (sync-v2.md §22, §23, §24): each runs
//! the window, the real editor and `LiveAccount` against the real server,
//! and these tests type, select and share through AccessKit, then read the
//! other desktop's editor and file, or open the link as a browser would.

#[path = "../../app-ui/tests/support/mod.rs"]
mod support;

#[path = "../../app-core/tests/sync_support/mod.rs"]
mod sync_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_core::account::live::{ACCOUNT_FILE, LiveAccount};
use app_core::account::{AccountBackend, AccountPhase};
use app_core::folder_sync::{FolderSyncBackend, FolderSyncPhase};
use app_core::shares::{ShareBackend, ShareScreen};
use app_core::sync::Secrets;
use app_core::sync::account_heads::KEPT_HEADS_FILE;
use app_core::sync::device::StoredDevice;
use app_core::{App, Platform, command_id};
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};
use support::harness_for;
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::ShareBody;
use sync_protocol::records::{open_share, parse_share_fragment};
use sync_store_sqlite::SqliteBackend;
use sync_support::{Line, MemorySecrets, Pair, Server};

type Ui = Harness<'static, app_ui::NouraUi>;

const WAIT: Duration = Duration::from_secs(60);

/// One computer: its window and the account behind it.
struct Computer {
    harness: Ui,
    account: Arc<LiveAccount>,
    folder: PathBuf,
}

impl Computer {
    /// The app on `folder` with `device`, talking to `base`, as the desktop
    /// binary wires it up. `keep_at` is the address the device was kept
    /// under.
    fn start(
        app_data: &Path,
        folder: &Path,
        device: &sync_support::Desktop,
        base: &str,
        keep_at: &str,
    ) -> Self {
        let secrets: Arc<MemorySecrets> = device.secrets.clone();
        let state = app_data.join("sync").join(device.id().to_hex());
        std::fs::create_dir_all(&state).unwrap();
        std::fs::copy(
            device.state.path().join(KEPT_HEADS_FILE),
            state.join(KEPT_HEADS_FILE),
        )
        .unwrap();
        std::fs::write(
            app_data.join(ACCOUNT_FILE),
            serde_json::json!({"server": base}).to_string(),
        )
        .unwrap();
        let mut stored = StoredDevice::load(&*secrets, keep_at)
            .unwrap()
            .expect("the device");
        stored.server = base.to_owned();
        stored.save(&*secrets).unwrap();
        app_core::WorkspaceService::new(app_data)
            .open_folder(folder)
            .unwrap();
        let store: Secrets = secrets;
        let account = Arc::new(LiveAccount::start(
            app_data.to_owned(),
            store,
            Arc::new(|_| Ok(())),
        ));
        let backend = account.clone();
        let mut harness = harness_for(app_data, Platform::Windows, move |app: &mut App| {
            app.set_account_backend(backend.clone());
            app.set_share_backend(backend.clone());
            app.set_folder_sync_backend(backend);
        });
        harness.run_steps(3);
        Self {
            harness,
            account,
            folder: folder.to_owned(),
        }
    }

    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.folder.join(name)).unwrap()
    }

    fn open(&mut self, name: &str) {
        self.harness
            .get_by_role_and_label(Role::TreeItem, name)
            .click();
        self.harness.run_steps(4);
    }

    fn shown(&self) -> String {
        self.harness
            .get_by_role(Role::MultilineTextInput)
            .value()
            .unwrap_or_default()
    }

    fn typing(&mut self, text: &str) {
        self.harness.get_by_role(Role::MultilineTextInput).focus();
        self.harness.run_steps(2);
        self.harness.event(egui::Event::Text(text.to_owned()));
        self.harness.run_steps(3);
    }

    fn in_text_mode(&self) -> bool {
        let app = self.harness.state().app();
        let tab = app.active_tab().expect("a tab").id();
        app.note(tab).expect("a note").text_mode().is_some()
    }

    fn cursors(&self) -> Vec<app_core::sync::workspace::presence::Cursor> {
        let app = self.harness.state().app();
        app.note_cursors(app.active_tab().expect("a tab").id())
    }

    fn signed_in_and_syncing(&self) -> bool {
        matches!(
            AccountBackend::screen(&*self.account).phase,
            AccountPhase::SignedIn(_)
        ) && FolderSyncBackend::screen(&*self.account).phase == FolderSyncPhase::On
    }
}

/// Draws both windows until `done` holds.
fn until(
    a: &mut Computer,
    b: &mut Computer,
    what: &str,
    done: impl Fn(&Computer, &Computer) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        a.harness.step();
        b.harness.step();
        if done(a, b) {
            a.harness.run_steps(2);
            b.harness.run_steps(2);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: the laptop shows {:?} over {:?}, the desktop {:?} over {:?}; the laptop's sync says {:?}, the desktop's {:?}",
            a.shown(),
            a.file("Plan.md"),
            b.shown(),
            b.file("Plan.md"),
            FolderSyncBackend::screen(&*a.account).last_error,
            FolderSyncBackend::screen(&*b.account).last_error,
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The two desktops of a pair as the app runs them, each with `Plan.md`
/// open.
struct Desks {
    laptop: Computer,
    desktop: Computer,
    _server: Server<SqliteBackend>,
    _proxy: sync_support::Proxy,
    _devices: (sync_support::Desktop, sync_support::Desktop),
    _folders: (sync_support::Folder, sync_support::Folder),
}

/// [`Desks`] over `pair`, the laptop on the server at `laptop_base`.
fn desks(pair: Pair<SqliteBackend>, laptop_base: &str) -> Desks {
    let Pair {
        server,
        proxy,
        laptop,
        service,
        a,
        sync_a,
        desktop,
        b,
        sync_b,
    } = pair;
    drop((service, sync_a, sync_b));
    let mut one = Computer::start(
        a.app_data.path(),
        a.dir.path(),
        &laptop,
        laptop_base,
        &server.base,
    );
    let mut two = Computer::start(
        b.app_data.path(),
        b.dir.path(),
        &desktop,
        &server.base,
        &server.base,
    );
    until(&mut one, &mut two, "both computers sync", |a, b| {
        a.signed_in_and_syncing() && b.signed_in_and_syncing()
    });
    one.open("Plan");
    two.open("Plan");
    Desks {
        laptop: one,
        desktop: two,
        _server: server,
        _proxy: proxy,
        _devices: (laptop, desktop),
        _folders: (a, b),
    }
}

/// What a browser with no account reads at `url`: the snapshot's version,
/// title and Markdown, opened with the keys in the fragment, or the HTTP
/// status when nothing opens.
async fn browser(url: &str) -> Result<(u64, String, String), u16> {
    let (address, fragment) = url.split_once('#').expect("a fragment");
    let (origin, token) = address.split_once("/s/").expect("a share path");
    let response = reqwest::get(format!("{origin}/v2/shares/{token}"))
        .await
        .expect("the server answers");
    if !response.status().is_success() {
        return Err(response.status().as_u16());
    }
    let body = ShareBody::from_bytes(&response.bytes().await.unwrap()).expect("a share body");
    let (link_key, verify_key) = parse_share_fragment(fragment).expect("the fragment");
    let (share, payload) =
        open_share(&body.share.0.to_bytes(), &link_key, &verify_key).expect("it opens");
    Ok((
        share.body.snapshot_revision,
        payload.title.0,
        String::from_utf8(payload.markdown.0).expect("UTF-8"),
    ))
}

impl Computer {
    /// Draws until the window puts a link on the clipboard.
    fn copied_link(&mut self) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            self.harness.step();
            let copied = self
                .harness
                .output()
                .platform_output
                .commands
                .iter()
                .find_map(|command| match command {
                    egui::OutputCommand::CopyText(text) => Some(text.clone()),
                    _ => None,
                });
            if let Some(link) = copied {
                return link;
            }
            let problem = ShareBackend::screen(&*self.account);
            assert!(Instant::now() < deadline, "no link came: {problem:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Runs Share Link… from the menu, and waits for the dialog's lists.
    fn share_dialog(&mut self) {
        self.harness
            .state_mut()
            .run_menu_command(command_id::SHARE_LINK, Instant::now());
        self.dialog_until("the dialog opens", |screen| {
            screen.open && !screen.busy && screen.unavailable.is_none()
        });
    }

    fn dialog_until(&mut self, what: &str, done: impl Fn(&ShareScreen) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.harness.step();
            let screen = ShareBackend::screen(&*self.account);
            if done(&screen) {
                self.harness.run_steps(2);
                return;
            }
            assert!(Instant::now() < deadline, "{what}: {screen:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Puts `one`'s note into its document with a first word. A note whose
/// file holds an edit that sync hasn't captured waits, so a later edit
/// asks again on `one`.
fn first_word(one: &mut Computer, other: &mut Computer, word: &str) {
    one.typing(word);
    let deadline = Instant::now() + WAIT;
    while !one.in_text_mode() {
        assert!(
            Instant::now() < deadline,
            "the first edit never brought the document"
        );
        one.harness.step();
        other.harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The two desktops, each with its note in its document.
fn both_in_their_documents(a: &mut Computer, b: &mut Computer) {
    first_word(a, b, "Ana ");
    until(a, b, "the laptop's words arrive", |_, b| {
        b.file("Plan.md").contains("Ana ") && b.shown().contains("Ana ")
    });
    first_word(b, a, "Ben ");
    until(a, b, "the desktop's words arrive", |a, _| {
        a.file("Plan.md").contains("Ben ") && a.shown().contains("Ben ")
    });
}

#[test]
fn the_share_dialog_creates_updates_and_revokes_a_link_a_browser_opens() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(sync_support::pair());
    let base = pair.server.base.clone();
    let mut desks = desks(pair, &base);
    let a = &mut desks.laptop;

    a.share_dialog();
    a.harness.get_by_label("Share \u{201c}Plan\u{201d}");
    a.harness
        .get_by_role_and_label(Role::Button, "Create and copy link")
        .click();
    let link = a.copied_link();
    assert!(link.starts_with(&format!("{base}/s/")), "{link}");
    assert_eq!(
        runtime.block_on(browser(&link)),
        Ok((1, "Plan".to_owned(), "# Plan\n".to_owned()))
    );
    a.dialog_until("the link shows", |screen| {
        screen.links.len() == 1 && screen.workspace_links.len() == 1
    });

    // Copy hands back the same link.
    a.harness
        .get_by_role_and_label(Role::Button, "Copy")
        .click();
    assert_eq!(a.copied_link(), link);

    // Update carries the note as it stands now.
    a.dialog_until("the dialog settles", |screen| !screen.busy);
    a.harness
        .get_by_role_and_label(Role::Button, "Done")
        .click();
    a.harness.run_steps(3);
    a.typing("Ana ");
    a.share_dialog();
    a.dialog_until("the link shows again", |screen| screen.links.len() == 1);
    a.harness
        .get_by_role_and_label(Role::Button, "Update")
        .click();
    a.dialog_until("the link updates", |screen| {
        screen.links.first().is_some_and(|link| link.revision == 2)
    });
    assert_eq!(
        runtime.block_on(browser(&link)),
        Ok((2, "Plan".to_owned(), "Ana # Plan\n".to_owned()))
    );

    // Revoke ends it for everyone, and the dialog lists nothing.
    a.harness
        .get_all_by_role_and_label(Role::Button, "Revoke")
        .next()
        .expect("a Revoke button")
        .click();
    a.dialog_until("the link goes", |screen| {
        screen.links.is_empty() && screen.workspace_links.is_empty()
    });
    assert_eq!(runtime.block_on(browser(&link)), Err(404));
}

#[test]
fn typing_in_one_editor_reaches_the_other_desktops_editor_and_file() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(sync_support::pair());
    let base = pair.server.base.clone();
    let mut desks = desks(pair, &base);
    let (a, b) = (&mut desks.laptop, &mut desks.desktop);
    both_in_their_documents(a, b);

    // Both type at once, and each ends with what the other typed.
    a.typing("one ");
    b.typing("two ");
    until(a, b, "both files match", |a, b| {
        let file = a.file("Plan.md");
        file.contains("one ")
            && file.contains("two ")
            && file == b.file("Plan.md")
            && a.shown() == file
            && b.shown() == file
    });
    // Each kept its place while the document took over.
    assert_eq!(a.file("Plan.md"), "Ben two Ana one # Plan\n");
}

#[test]
fn a_selection_in_one_editor_shows_in_the_others_note_and_goes_with_its_text() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(sync_support::pair());
    let base = pair.server.base.clone();
    let mut desks = desks(pair, &base);
    let (a, b) = (&mut desks.laptop, &mut desks.desktop);
    both_in_their_documents(a, b);

    // Three characters back from the laptop's caret.
    for _ in 0..3 {
        a.harness
            .key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
        a.harness.run_steps(2);
    }
    until(a, b, "the selection arrives", |_, b| {
        b.cursors().iter().any(|cursor| {
            cursor
                .selections
                .iter()
                .any(|range| range.anchor.abs_diff(range.head) == 3)
        })
    });
    b.harness
        .get_by_role_and_label(Role::Caret, "A collaborator's cursor");
    let at = |b: &Computer| {
        b.cursors()
            .iter()
            .flat_map(|cursor| cursor.selections.clone())
            .map(|range| (range.anchor, range.head))
            .max_by_key(|(anchor, head)| anchor.abs_diff(*head))
            .expect("a selection")
    };
    let before = at(b);

    // Text typed ahead of the selection moves it along.
    b.harness.key_press_modifiers(
        if cfg!(target_os = "macos") {
            egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::CTRL
        },
        egui::Key::Home,
    );
    b.harness.run_steps(2);
    b.harness.event(egui::Event::Text("new ".to_owned()));
    b.harness.run_steps(3);
    until(a, b, "the selection moves with its text", |_, b| {
        at(b) == (before.0 + 4, before.1 + 4)
    });
}

#[test]
fn edits_on_both_sides_of_a_dropped_connection_merge_when_it_returns() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(sync_support::pair());
    let line = runtime.block_on(Line::to(&pair.server.base));
    let mut desks = desks(pair, &line.base);
    let (a, b) = (&mut desks.laptop, &mut desks.desktop);
    both_in_their_documents(a, b);

    line.cut();
    a.typing("offline ");
    until(a, b, "the laptop finds itself offline", |a, _| {
        FolderSyncBackend::screen(&*a.account)
            .last_error
            .is_some_and(|error| error.contains("can't be reached"))
    });
    b.typing("online ");

    // Each keeps its typing in its file, and the other's stays away.
    until(a, b, "each file holds its own typing", |a, b| {
        a.file("Plan.md").contains("offline ") && b.file("Plan.md").contains("online ")
    });
    for _ in 0..200 {
        a.harness.step();
        b.harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!a.file("Plan.md").contains("online "));
    assert!(!b.file("Plan.md").contains("offline "));

    runtime.block_on(line.restore());
    until(a, b, "both files match", |a, b| {
        let file = a.file("Plan.md");
        file.contains("offline ")
            && file.contains("online ")
            && file == b.file("Plan.md")
            && a.shown() == file
            && b.shown() == file
    });
    assert_eq!(a.file("Plan.md"), "Ben online Ana offline # Plan\n");
    until(a, b, "the laptop's sync is well", |a, _| {
        FolderSyncBackend::screen(&*a.account).last_error.is_none()
    });
}
