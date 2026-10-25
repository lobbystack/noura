//! The desktop's account backend against the real server (seam 1 and
//! seam 4): you type the server address, sign in through the browser,
//! and the screens show the account's fingerprint and this device; you
//! export a kit, whose secret shows once, and the last device's sign-out
//! warns before it leaves the credential store empty.

mod support;
mod sync_support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::account::live::LiveAccount;
use app_core::account::{AccountAction, AccountBackend, AccountPhase, AccountScreen};
use app_core::sync::device::StoredDevice;
use app_core::sync::{KeptHeads, SyncService};
use sync_protocol::types::{from_base64url, to_base64url, to_hex};
use sync_store::{Backend as _, WriteTxn as _};
use sync_support::{MemorySecrets, Server, first_desktop};

fn wait_for(
    account: &LiveAccount,
    what: &str,
    done: impl Fn(&AccountScreen) -> bool,
) -> AccountScreen {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let screen = account.screen();
        if done(&screen) {
            return screen;
        }
        assert!(Instant::now() < deadline, "{what} never showed: {screen:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_account_screens_sign_in_export_a_kit_and_sign_out() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let account_id = runtime.block_on(server.person("ada@example.com"));
    let app_data = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let opened = Arc::new(Mutex::new(Vec::new()));
    let record = opened.clone();
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        secrets.clone(),
        Arc::new(move |url| {
            record.lock().unwrap().push(url.to_owned());
            Ok(())
        }),
    );

    account.act(AccountAction::SetServer("http://sync.example.com".into()));
    let screen = wait_for(&account, "the HTTPS rule", |s| s.server_problem.is_some());
    assert!(screen.server_problem.unwrap().contains("https://"));
    account.act(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "a loopback address", |s| {
        s.server == server.base && s.server_problem.is_none()
    });

    account.act(AccountAction::SignIn);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    assert_eq!(
        *opened.lock().unwrap(),
        vec![format!("{}/device", server.base)]
    );
    assert!(
        runtime
            .block_on(
                server
                    .store
                    .decide_device_grant(account_id, &user_code, true)
            )
            .unwrap()
    );
    let screen = wait_for(
        &account,
        "the signed-in account",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    assert_eq!(
        view.account_fingerprint.len(),
        79,
        "64 characters in 16 groups"
    );
    assert_eq!(view.devices.len(), 1);
    assert!(view.devices[0].this_device);
    assert_eq!(view.devices[0].fingerprint.len(), 79);
    assert!(
        std::fs::read_to_string(app_data.path().join("sync-account.json"))
            .unwrap()
            .contains(&server.base),
        "the address stays for the next launch"
    );

    let outside = tempfile::tempdir().unwrap();
    let kit = outside.path().join("Noura recovery kit.noura-kit");
    account.act(AccountAction::ExportKit(kit.clone()));
    let screen = wait_for(
        &account,
        "the kit's secret",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.kit.is_some()),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    let shown = view.kit.unwrap();
    assert_eq!(shown.path, kit);
    assert_eq!(shown.secret.len(), 39);
    account.act(AccountAction::DismissKit);
    wait_for(
        &account,
        "the secret gone",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.kit.is_none()),
    );

    account.act(AccountAction::SignOut);
    let screen = wait_for(
        &account,
        "the sign-out warning",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.sign_out.is_some()),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    assert!(view.sign_out.unwrap().last_device, "the last device warns");
    account.act(AccountAction::ConfirmSignOut);
    wait_for(&account, "signed out", |s| {
        matches!(s.phase, AccountPhase::SignedOut) && !s.busy
    });
    assert!(
        StoredDevice::load(&*secrets, &server.base)
            .unwrap()
            .is_none(),
        "the keys and session token left the credential store"
    );
    drop(runtime);
}

/// A log that keeps every line any thread writes while the tests run.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The files under `folder` whose bytes hold `needle`.
fn files_holding(folder: &std::path::Path, needle: &[u8], found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_holding(&path, needle, found);
        } else if std::fs::read(&path)
            .unwrap_or_default()
            .windows(needle.len())
            .any(|window| window == needle)
        {
            found.push(path);
        }
    }
}

fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Seam 1 (§7): signing in, using a synced workspace and exporting a kit
/// leave the device's session token and both seeds, in every encoding the
/// app handles them in, and the kit's secret, out of the workspace folder
/// and its index, the app data folder, every log line and the diagnostics
/// report.
#[test]
fn the_device_secrets_reach_no_workspace_file_index_log_or_diagnostics() {
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    // Every thread logs here, the account backend's included. No other
    // test in this file sets a subscriber.
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let account_id = runtime.block_on(server.person("ada@example.com"));
    let fixture = support::Fixture::new(&[("Note.md", b"# Note\n")]);
    let system = support::FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let mut indexes = Vec::new();
    files_holding(
        &fixture.app_data.path().join("workspaces"),
        b"SQLite format 3",
        &mut indexes,
    );
    assert!(
        indexes.iter().any(|path| path.ends_with("index.sqlite")),
        "the workspace's index is in the app data folder: {indexes:?}"
    );
    let secrets = Arc::new(MemorySecrets::default());
    let account = Arc::new(LiveAccount::start(
        fixture.app_data.path().to_owned(),
        secrets.clone(),
        Arc::new(|_| Ok(())),
    ));
    app.set_account_backend(account.clone());
    app.account_action(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address", |s| s.server == server.base);
    app.account_action(AccountAction::SignIn);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    runtime
        .block_on(
            server
                .store
                .decide_device_grant(account_id, &user_code, true),
        )
        .unwrap();
    wait_for(
        &account,
        "the signed-in account",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );

    // The device syncs a workspace: it creates it, pushes a file and pulls
    // it back.
    let credentials = StoredDevice::load(&*secrets, &server.base)
        .unwrap()
        .unwrap()
        .credentials()
        .unwrap()
        .unwrap();
    runtime.block_on(async {
        let own = Arc::new(MemorySecrets::default());
        let heads = KeptHeads::open(
            &fixture.app_data.path().join("sync-test-service"),
            account_id,
            credentials.keys.device(),
            &*own,
        )
        .unwrap();
        let mut service = SyncService::connect(&server.base, credentials.clone(), heads, own)
            .await
            .unwrap();
        let mut remote = service.create_workspace("Garden").await.unwrap();
        service
            .push_file(&mut remote, "Note.md", b"# Note\n")
            .await
            .unwrap();
        let files = service.pull_files(&mut remote).await.unwrap();
        assert_eq!(files.len(), 1);
    });

    // A kit export, then a failed one that the backend logs.
    let outside = tempfile::tempdir().unwrap();
    let kit = outside.path().join("Noura recovery kit.noura-kit");
    app.account_action(AccountAction::ExportKit(kit.clone()));
    let screen = wait_for(
        &account,
        "the kit's secret",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.kit.is_some()),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    let kit_secret = view.kit.unwrap().secret.to_string();
    app.account_action(AccountAction::DismissKit);
    app.account_action(AccountAction::ExportKit(kit.clone()));
    wait_for(&account, "the refused export", |s| s.problem.is_some());

    app.copy_diagnostics();
    let report = system
        .calls()
        .into_iter()
        .find_map(|call| match call {
            support::Call::Copy(text) => Some(text),
            _ => None,
        })
        .expect("the diagnostics report");
    drop(app);

    let token = credentials.session_token.clone();
    let raw_token: [u8; 32] = from_base64url(&token).unwrap();
    let signing = *credentials.keys.signing().seed().expose();
    let hpke = *credentials.keys.hpke_seed().expose();
    let mut needles: Vec<(&str, Vec<u8>)> = vec![
        ("the token", token.as_bytes().to_vec()),
        ("the token's bytes", raw_token.to_vec()),
        ("the token in hex", to_hex(&raw_token).into_bytes()),
        ("the kit secret", kit_secret.as_bytes().to_vec()),
        (
            "the kit secret without spaces",
            kit_secret.replace(' ', "").into_bytes(),
        ),
    ];
    for (name, seed) in [("the signing seed", signing), ("the X-Wing seed", hpke)] {
        needles.push((name, seed.to_vec()));
        needles.push((name, to_hex(&seed).into_bytes()));
        needles.push((name, to_base64url(&seed).into_bytes()));
    }
    let captured = log.0.lock().unwrap().clone();
    assert!(!captured.is_empty(), "the backend logged its failed export");
    for (name, needle) in &needles {
        let mut found = Vec::new();
        for folder in [fixture.root.as_path(), fixture.app_data.path()] {
            files_holding(folder, needle, &mut found);
        }
        assert_eq!(
            found,
            Vec::<std::path::PathBuf>::new(),
            "{name} reached a file"
        );
        assert!(!holds(&captured, needle), "{name} reached the log");
        assert!(
            !holds(report.as_bytes(), needle),
            "{name} reached the diagnostics"
        );
    }
    drop(runtime);
}

/// A restore lost the device's session (§7, §14): on the next launch the
/// screen says so and offers to try again, and signing in again keeps the
/// same device.
#[test]
fn a_lost_session_offers_to_try_again_and_signing_in_keeps_the_device() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let account_id = runtime.block_on(server.person("cy@example.com"));
    let app_data = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let sign_in = |account: &LiveAccount| {
        account.act(AccountAction::SignIn);
        let screen = wait_for(account, "the sign-in code", |s| {
            matches!(s.phase, AccountPhase::SigningIn { .. })
        });
        let AccountPhase::SigningIn { user_code } = screen.phase else {
            unreachable!()
        };
        runtime
            .block_on(
                server
                    .store
                    .decide_device_grant(account_id, &user_code, true),
            )
            .unwrap();
        let screen = wait_for(
            account,
            "the signed-in account",
            |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
        );
        let AccountPhase::SignedIn(view) = screen.phase else {
            unreachable!()
        };
        view.devices
    };
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    account.act(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address", |s| s.server == server.base);
    let before = sign_in(&account);
    drop(account);
    // The backend's thread ends once it finishes what it was doing.
    std::thread::sleep(Duration::from_secs(3));

    let device = StoredDevice::load(&*secrets, &server.base)
        .unwrap()
        .unwrap()
        .card()
        .unwrap()
        .device;
    runtime
        .block_on(server.store.backend().write(Box::new(
            move |txn: &mut sync_store_sqlite::SqliteTxn| {
                Box::pin(async move {
                    txn.delete_device_sessions(account_id, device)
                        .await
                        .map(|_| ())
                })
            },
        )))
        .unwrap();

    let account = LiveAccount::start(
        app_data.path().to_owned(),
        secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    let screen = wait_for(&account, "the offer to try again", |s| s.can_retry);
    assert!(
        screen
            .problem
            .as_deref()
            .unwrap_or_default()
            .contains("session"),
        "{screen:?}"
    );
    assert!(matches!(screen.phase, AccountPhase::SignedOut));
    account.act(AccountAction::Retry);
    wait_for(&account, "the retry", |s| s.can_retry && !s.busy);

    let after = sign_in(&account);
    assert_eq!(after.len(), 1, "no new card");
    assert_eq!(after[0].fingerprint, before[0].fingerprint);
    assert!(after[0].this_device);
    drop(account);
    drop(runtime);
}

/// Recovering through the account backend (§12): the review shows the
/// changes since the kit, the members of the workspace the kit doesn't
/// list and the devices the recovery removes, then the account signs in on
/// this computer and offers a new kit.
#[test]
fn the_recovery_review_shows_unlisted_workspaces_with_their_members() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let outside = tempfile::tempdir().unwrap();
    let kit = outside.path().join("Noura recovery kit.noura-kit");
    let (laptop, secret, later) = runtime.block_on(async {
        let mut laptop = first_desktop(&server, "dee@example.com").await;
        let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
        let export = laptop
            .service
            .export_kit(&kit, &[], std::slice::from_mut(&mut workspace))
            .await
            .unwrap();
        let later = laptop.service.create_workspace("Later").await.unwrap();
        (laptop, export.secret.to_string(), later.workspace)
    });
    let account_id = laptop.credentials.account;

    let app_data = tempfile::tempdir().unwrap();
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        Arc::new(MemorySecrets::default()),
        Arc::new(|_| Ok(())),
    );
    account.act(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address", |s| s.server == server.base);
    account.act(AccountAction::Recover {
        path: kit.clone(),
        secret: secret.to_lowercase(),
    });
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    // The backend keeps answering while the code waits.
    account.act(AccountAction::SetServer(
        "https://elsewhere.example.com".into(),
    ));
    wait_for(&account, "an answer while the code waits", |s| {
        s.server == "https://elsewhere.example.com"
            && matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    account.act(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address again", |s| s.server == server.base);
    runtime
        .block_on(
            server
                .store
                .decide_device_grant(account_id, &user_code, true),
        )
        .unwrap();
    let screen = wait_for(&account, "the review", |s| {
        matches!(s.phase, AccountPhase::Recovering(_))
    });
    let AccountPhase::Recovering(review) = screen.phase else {
        unreachable!()
    };
    let short: String = later.to_hex().chars().take(8).collect();
    assert_eq!(
        review.members,
        vec![format!(
            "Workspace {short} has these members: your account."
        )]
    );
    assert!(review.unverified.is_empty(), "{review:?}");
    assert!(
        review
            .changes
            .iter()
            .any(|c| c.starts_with(&format!("Workspace {short}"))),
        "{review:?}"
    );
    assert_eq!(review.devices.len(), 1);
    assert_eq!(
        review.devices[0].1,
        app_core::account::fingerprint(&laptop.credentials.card().fingerprint())
    );

    account.act(AccountAction::ConfirmRecovery);
    let screen = wait_for(
        &account,
        "the recovered account",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty() && !s.busy),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    assert!(view.offer_kit, "the app asks for a new kit");
    assert!(view.added_members.is_empty(), "nobody joined after the kit");
    assert!(view.devices.iter().any(|d| d.this_device && !d.removed));
    drop(account);
    drop(laptop);
    drop(runtime);
}
