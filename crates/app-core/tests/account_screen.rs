//! The desktop's account backend against the real server (seam 1 and
//! seam 4): you type the server address, sign in through the browser,
//! and the screens show the account's fingerprint and this device; you
//! export a kit, whose secret shows once, and the last device's sign-out
//! warns before it leaves the credential store empty.

mod support;
mod sync_support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::account::live::{LiveAccount, UNVERIFIED_REMOVAL};
use app_core::account::{AccountAction, AccountBackend, AccountPhase, AccountScreen};
use app_core::sync::device::StoredDevice;
use app_core::sync::{KeptHeads, SyncService};
use sync_protocol::types::{from_base64url, to_base64url, to_hex};
use sync_store::{Backend as _, WriteTxn as _};
use sync_support::{
    Desktop, MemorySecrets, Pair, Proxy, Server, first_desktop, new_desktop, pair_of_accounts_on,
    reset_devices,
};

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
    // The kit's recovery identity, and the kit file itself.
    let payload = app_core::sync::recovery::open_kit(&kit, &kit_secret).unwrap();
    let kit_signing = *payload.signing_seed.expose();
    let kit_hpke = *payload.hpke_secret.expose();
    let kit_bytes = std::fs::read(&kit).unwrap();
    needles.push(("the kit file", kit_bytes.clone()));
    needles.push((
        "the kit file's last 64 bytes",
        kit_bytes[kit_bytes.len() - 64..].to_vec(),
    ));
    for (name, seed) in [
        ("the signing seed", signing),
        ("the X-Wing seed", hpke),
        ("the kit's signing seed", kit_signing),
        ("the kit's X-Wing secret", kit_hpke),
    ] {
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

/// A server that answers `device_removed` when no link removed this
/// computer (§7): the account stays, sync stops with the failed check, and
/// the screen never says that another device removed it, whether the
/// answer names a link that removes nobody or names none. Once the server
/// answers honestly again, the device connects as before.
#[test]
fn a_forged_device_removed_answer_keeps_the_account() {
    use sync_protocol::codec::{Bytes, Encode as _, Text};
    use sync_protocol::messages::{DeviceRemovedDetail, ErrorBody, server_error_status};

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server, proxy) = runtime.block_on(Proxy::in_front());
    let account_id = runtime.block_on(server.person("di@example.com"));
    let app_data = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let start = || {
        LiveAccount::start(
            app_data.path().to_owned(),
            secrets.clone(),
            Arc::new(|_| Ok(())),
        )
    };
    let account = start();
    account.act(AccountAction::SetServer(proxy.base.clone()));
    wait_for(&account, "the address", |s| s.server == proxy.base);
    account.act(AccountAction::SignIn);
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
    let genesis = StoredDevice::load(&*secrets, &proxy.base)
        .unwrap()
        .unwrap()
        .credentials()
        .unwrap()
        .unwrap()
        .reference_head
        .genesis;
    // Every route but the chain reads, which a removed session keeps,
    // answers device_removed.
    let forge = |detail: Option<Vec<u8>>| -> sync_support::Forge {
        let body = ErrorBody {
            code: Text::new("device_removed").unwrap(),
            message: Text::new("a device removed this device").unwrap(),
            retry_after_seconds: None,
            detail: detail.map(Bytes),
        }
        .to_bytes();
        let status = server_error_status("device_removed").unwrap();
        Box::new(move |_, path| {
            (path != "/v2/accounts/chains" && path != "/v2/limits").then(|| (status, body.clone()))
        })
    };

    // The backend's next refresh meets an answer naming the genesis link,
    // which removes nobody.
    proxy.forge(forge(Some(
        DeviceRemovedDetail { genesis, seqno: 1 }.to_bytes(),
    )));
    let screen = wait_for(&account, "the failed check", |s| {
        s.problem.as_deref() == Some(UNVERIFIED_REMOVAL)
    });
    assert!(
        matches!(screen.phase, AccountPhase::SignedIn(_)),
        "{screen:?}"
    );
    drop(account);
    std::thread::sleep(Duration::from_secs(3));

    // On the next launch, an answer that names no link.
    proxy.forge(forge(None));
    let account = start();
    let screen = wait_for(&account, "the offer to try again", |s| s.can_retry);
    assert_eq!(screen.problem.as_deref(), Some(UNVERIFIED_REMOVAL));
    assert!(
        !matches!(screen.phase, AccountPhase::Removed { .. }),
        "{screen:?}"
    );
    assert!(
        StoredDevice::load(&*secrets, &proxy.base)
            .unwrap()
            .is_some(),
        "the device keeps its keys"
    );
    proxy.honest();
    account.act(AccountAction::Retry);
    wait_for(
        &account,
        "the account again",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );
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

/// A server that can't be reached for a while leaves the browser sign-in
/// waiting, and it signs in once the server answers again; a confirmed
/// approval that fails to commit leaves the screen, since confirming
/// spent its offer (§7, §10).
#[test]
fn a_sign_in_outlasts_an_unreachable_server_and_a_failed_approval_leaves_the_screen() {
    use sync_protocol::codec::{Encode as _, Text};
    use sync_protocol::messages::{ErrorBody, server_error_status};

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server, proxy) = runtime.block_on(Proxy::in_front());
    let account_id = runtime.block_on(server.person("flo@example.com"));
    let app_data = tempfile::tempdir().unwrap();
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        Arc::new(MemorySecrets::default()),
        Arc::new(|_| Ok(())),
    );
    account.act(AccountAction::SetServer(proxy.base.clone()));
    wait_for(&account, "the address", |s| s.server == proxy.base);
    account.act(AccountAction::SignIn);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    // A gateway that answers outside the protocol.
    proxy.forge(Box::new(|_, path| {
        (path == "/v2/device/token").then(|| (502, b"bad gateway".to_vec()))
    }));
    let screen = wait_for(&account, "the unreachable server", |s| {
        s.problem
            .as_deref()
            .is_some_and(|p| p.contains("can't be reached"))
    });
    assert!(
        matches!(screen.phase, AccountPhase::SigningIn { .. }),
        "{screen:?}"
    );
    proxy.honest();
    runtime
        .block_on(
            server
                .store
                .decide_device_grant(account_id, &user_code, true),
        )
        .unwrap();
    let screen = wait_for(
        &account,
        "the signed-in account",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );
    assert_eq!(screen.problem, None, "the problem went with the outage");

    let mut phone = runtime.block_on(new_desktop(&server, account_id, &server.base));
    wait_for(
        &account,
        "the request",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.requests.len() == 1),
    );
    account.act(AccountAction::Approve(0));
    wait_for(
        &account,
        "the open approval",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.approval.is_some()),
    );
    runtime.block_on(phone.candidate.poll()).unwrap();
    wait_for(&account, "the approval code", |s| {
        matches!(&s.phase, AccountPhase::SignedIn(view)
            if view.approval.as_ref().is_some_and(|a| a.code.is_some()))
    });
    let body = ErrorBody {
        code: Text::new("internal").unwrap(),
        message: Text::new("the database failed").unwrap(),
        retry_after_seconds: None,
        detail: None,
    }
    .to_bytes();
    let status = server_error_status("internal").unwrap();
    proxy.forge(Box::new(move |method, path| {
        (method == "POST" && path == "/v2/accounts/chain").then(|| (status, body.clone()))
    }));
    account.act(AccountAction::ConfirmApproval);
    let screen = wait_for(&account, "the failed approval", |s| s.problem.is_some());
    assert!(
        matches!(&screen.phase, AccountPhase::SignedIn(view) if view.approval.is_none()),
        "{screen:?}"
    );
    proxy.honest();
    drop(account);
    drop(runtime);
}

/// An approval nobody confirms within 10 minutes ends on the approving
/// device too, which then takes the next request (§10).
#[test]
fn an_approval_that_expires_ends_and_the_next_request_can_be_approved() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let account_id = runtime.block_on(server.person("eve@example.com"));
    let app_data = tempfile::tempdir().unwrap();
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        Arc::new(MemorySecrets::default()),
        Arc::new(|_| Ok(())),
    );
    account.act(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address", |s| s.server == server.base);
    account.act(AccountAction::SignIn);
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
    let requested = |s: &AccountScreen| matches!(&s.phase, AccountPhase::SignedIn(view) if view.requests.len() == 1 && view.approval.is_none());
    let _first = runtime.block_on(new_desktop(&server, account_id, &server.base));
    wait_for(&account, "the first request", requested);
    account.act(AccountAction::Approve(0));
    wait_for(
        &account,
        "the open approval",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.approval.is_some()),
    );

    server.store.clock().advance(Duration::from_secs(601));
    let screen = wait_for(&account, "the ended approval", |s| {
        s.problem.is_some()
            && matches!(&s.phase, AccountPhase::SignedIn(view) if view.approval.is_none())
    });
    assert!(
        screen.problem.unwrap().contains("10 minutes"),
        "the approval says why it stopped"
    );

    let second = runtime.block_on(new_desktop(&server, account_id, &server.base));
    let shown = app_core::account::fingerprint(&second.candidate.fingerprint());
    wait_for(&account, "the second request", |s| {
        matches!(&s.phase, AccountPhase::SignedIn(view)
            if view.requests.len() == 1 && view.requests[0].fingerprint == shown)
    });
    account.act(AccountAction::Approve(0));
    let screen = wait_for(
        &account,
        "the second approval",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.approval.is_some()),
    );
    assert_eq!(screen.problem, None, "the device took the new request");
    drop(account);
    drop(runtime);
}

/// While a synced workspace's pull waits up to 25 seconds for new records
/// (§15), what you do in the Workspace sync section still takes effect at
/// once: the message ends the wait.
#[test]
fn a_waiting_pull_doesnt_hold_up_what_you_do() {
    use app_core::folder_sync::{FolderSyncAction, FolderSyncBackend, FolderSyncPhase};

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let account_id = runtime.block_on(server.person("di@example.com"));
    let app_data = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    AccountBackend::act(&account, AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address", |s| s.server == server.base);
    AccountBackend::act(&account, AccountAction::SignIn);
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

    let folder = sync_support::Folder::new("Garden");
    folder.write("Note.md", b"# Note\n");
    FolderSyncBackend::attach(&account, Some(folder.engine.clone()));
    let wait_folder =
        |what: &str, done: &dyn Fn(&app_core::folder_sync::FolderSyncScreen) -> bool| {
            let started = Instant::now();
            loop {
                let screen = FolderSyncBackend::screen(&account);
                if done(&screen) {
                    return started.elapsed();
                }
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "{what} never showed: {screen:?}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
    FolderSyncBackend::act(&account, FolderSyncAction::TurnOn);
    wait_folder("sync on", &|s| {
        s.phase == FolderSyncPhase::On && s.pending == 0 && !s.busy
    });
    // The next rounds' pulls wait for the server, which has nothing new.
    std::thread::sleep(Duration::from_secs(5));
    FolderSyncBackend::act(&account, FolderSyncAction::Pause);
    let took = wait_folder("the pause", &|s| s.phase == FolderSyncPhase::Paused);
    assert!(
        took < Duration::from_secs(5),
        "the pause waited for the pull: {took:?}"
    );
    drop(account);
    drop(runtime);
}

/// Reset devices through the real backend: a wrong code stays put, the
/// right one starts a new chain and shows the new fingerprint.
/// A recovery with a kit whose account head a restore lost offers Reset
/// devices, through the real backend (§14).
#[test]
fn a_kit_the_server_lost_offers_reset_devices() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("kit");
    let (server, proxy) = runtime.block_on(Proxy::in_front());
    let (restored, account_id, secret) = runtime.block_on(async {
        let mut laptop = first_desktop(&server, "dee@example.com").await;
        let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
        let copy = server.backup("before.sqlite3").await;
        let export = laptop
            .service
            .export_kit(&path, &[], std::slice::from_mut(&mut workspace))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        (
            server.restore_behind(&copy, &proxy).await,
            laptop.credentials.account,
            export.secret.to_string(),
        )
    });

    let app_data = tempfile::tempdir().unwrap();
    let account = LiveAccount::start(
        app_data.path().to_owned(),
        Arc::new(MemorySecrets::default()),
        Arc::new(|_| Ok(())),
    );
    account.act(AccountAction::SetServer(proxy.base.clone()));
    wait_for(&account, "the address", |s| s.server == proxy.base);
    let grant = |account: &LiveAccount| {
        let screen = wait_for(account, "the sign-in code", |s| {
            matches!(s.phase, AccountPhase::SigningIn { .. })
        });
        let AccountPhase::SigningIn { user_code } = screen.phase else {
            unreachable!()
        };
        runtime
            .block_on(
                restored
                    .store
                    .decide_device_grant(account_id, &user_code, true),
            )
            .unwrap();
    };
    account.act(AccountAction::Recover { path, secret });
    grant(&account);
    wait_for(&account, "the way back", |s| {
        matches!(s.phase, AccountPhase::KitLost)
    });
    account.act(AccountAction::StartReset);
    grant(&account);
    wait_for(&account, "the code field", |s| {
        matches!(s.phase, AccountPhase::ResetCode)
    });
}

#[test]
fn reset_devices_starts_a_new_chain_from_the_emailed_code() {
    use app_core::sync::device::{Joined, join};

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Server::start());
    let account_id = runtime.block_on(server.person("ada@example.com"));
    // Another device, gone now, started the chain.
    runtime.block_on(async {
        let signed_in = server.sign_in(account_id, &server.base).await;
        let secrets = MemorySecrets::default();
        assert!(matches!(
            join(&signed_in, &secrets).await.expect("joined"),
            Joined::First(_)
        ));
    });
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
    account.act(AccountAction::SetServer(server.base.clone()));
    wait_for(&account, "the address", |s| s.server == server.base);
    account.act(AccountAction::StartReset);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    assert!(
        runtime
            .block_on(
                server
                    .store
                    .decide_device_grant(account_id, &user_code, true)
            )
            .unwrap()
    );
    wait_for(&account, "the code field", |s| {
        matches!(s.phase, AccountPhase::ResetCode)
    });
    let code = server
        .mail
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|text: &String| text.split_whitespace().nth(5).map(str::to_owned))
        .expect("the code's mail");
    account.act(AccountAction::ConfirmReset("WRONGONE".into()));
    let screen = wait_for(&account, "the refusal", |s| s.problem.is_some());
    assert!(matches!(screen.phase, AccountPhase::ResetCode));
    account.act(AccountAction::ConfirmReset(
        code.trim_end_matches('.').to_owned(),
    ));
    let screen = wait_for(
        &account,
        "the new chain",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_fingerprint.is_some()),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    assert!(view.offer_kit);
}

/// The owner's computer of `pair` as the app runs it: the account backend
/// over the device, its kept heads and its folder's journal. The owner's
/// sync and service end here.
fn owner_app(pair: Pair<sync_store_sqlite::SqliteBackend>) -> (LiveAccount, OwnerApp) {
    let base = pair.server.base.clone();
    owner_app_at(pair, &base)
}

/// [`owner_app`], with the app talking to `base`.
fn owner_app_at(
    pair: Pair<sync_store_sqlite::SqliteBackend>,
    base: &str,
) -> (LiveAccount, OwnerApp) {
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
    let workspace = sync_a.workspace();
    drop(sync_a);
    drop(service);
    let state = a.app_data.path().join("sync").join(laptop.id().to_hex());
    std::fs::create_dir_all(&state).unwrap();
    std::fs::copy(
        laptop
            .state
            .path()
            .join(app_core::sync::account_heads::KEPT_HEADS_FILE),
        state.join(app_core::sync::account_heads::KEPT_HEADS_FILE),
    )
    .unwrap();
    std::fs::write(
        a.app_data
            .path()
            .join(app_core::account::live::ACCOUNT_FILE),
        serde_json::json!({"server": base}).to_string(),
    )
    .unwrap();
    let secrets = laptop.secrets.clone();
    let mut stored = StoredDevice::load(&*secrets, &server.base)
        .unwrap()
        .expect("the laptop's device");
    stored.server = base.to_owned();
    stored.save(&*secrets).unwrap();
    let account = LiveAccount::start(a.app_data.path().to_owned(), secrets, Arc::new(|_| Ok(())));
    app_core::folder_sync::FolderSyncBackend::attach(&account, Some(a.engine.clone()));
    wait_for(
        &account,
        "the signed-in account",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );
    (
        account,
        OwnerApp {
            server,
            proxy,
            laptop,
            a,
            desktop,
            b,
            sync_b,
            workspace,
        },
    )
}

struct OwnerApp {
    server: Server,
    proxy: Proxy,
    laptop: Desktop,
    a: sync_support::Folder,
    desktop: Desktop,
    b: sync_support::Folder,
    sync_b: app_core::sync::workspace::WorkspaceSync,
    workspace: sync_protocol::types::Id,
}

/// A member resets and nobody asked this device to accept it yet: the
/// account screens show the changed devices with the new fingerprint.
#[test]
fn the_account_backend_shows_a_members_reset_without_a_request() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let member = pair.desktop.credentials.account;
    let (account, app) = owner_app(pair);
    let (_new, done) = runtime.block_on(reset_devices(
        &app.server,
        member,
        Arc::new(MemorySecrets::default()),
    ));
    let shown = done.fingerprint.to_hex();
    let grouped: Vec<String> = shown
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    let needle = grouped.join(" ");
    // The device list comes up between pulls, and a message ends a pull's
    // wait.
    let started = Instant::now();
    loop {
        let screen = AccountBackend::screen(&account);
        let shows = matches!(&screen.phase, AccountPhase::SignedIn(view) if view
            .alerts
            .iter()
            .any(|a| a.text.contains("changed") && a.text.contains(&needle)));
        if shows {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(45),
            "the alert never showed: {screen:?}"
        );
        app_core::folder_sync::FolderSyncBackend::act(
            &account,
            app_core::folder_sync::FolderSyncAction::DismissRestore,
        );
        std::thread::sleep(Duration::from_secs(3));
    }
    drop(account);
    drop(app);
    drop(runtime);
}

/// The whole path of a lost secret in the app: the server at the same
/// address is a fresh one, sync stops at `unauthorized`, signing in gives
/// a new account and new keys, the section says the server lost the account,
/// and Set up sync again turns the folder on as a new workspace (§14).
#[test]
fn the_app_turns_a_folder_on_after_the_server_lost_its_secret() {
    use app_core::folder_sync::{FolderSyncAction, FolderSyncBackend, FolderSyncPhase};
    use app_core::sync::device::StoredDevice;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let base = pair.proxy.base.clone();
    let old_account = pair.laptop.credentials.account;
    let (account, app) = owner_app_at(pair, &base);
    let files = app.a.files();
    let fresh = runtime.block_on(Server::fresh_behind(&app.proxy));
    let new_account = runtime.block_on(fresh.person("ana@example.com"));
    assert_ne!(new_account, old_account);

    AccountBackend::act(&account, AccountAction::SignIn);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    assert!(
        runtime
            .block_on(
                fresh
                    .store
                    .decide_device_grant(new_account, &user_code, true)
            )
            .unwrap()
    );
    let started = Instant::now();
    let screen = loop {
        let screen = FolderSyncBackend::screen(&account);
        if screen.phase == FolderSyncPhase::AccountLost {
            break screen;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the stop never showed: {screen:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let notice = screen.restore.expect("a notice");
    assert!(notice.set_up_again);
    assert!(notice.line.contains("lost your account"), "{}", notice.line);
    let stored = StoredDevice::load(&*app.laptop.secrets, &base)
        .unwrap()
        .expect("the new device");
    assert_eq!(stored.account, new_account);

    FolderSyncBackend::act(&account, FolderSyncAction::SetUpAgain);
    let started = Instant::now();
    loop {
        let screen = FolderSyncBackend::screen(&account);
        if screen.phase == FolderSyncPhase::On && screen.pending == 0 && !screen.busy {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the folder never synced: {screen:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(app.a.files(), files);
    drop(account);
    drop(app);
    drop(fresh);
    drop(runtime);
}

/// The live path of an `accept_reset` (§7, §10): a member's reset request
/// shows on the owner's screen with the alert, Accept shows the approval
/// code once the other device answers, and Codes match commits the change,
/// which the reset device verifies.
#[test]
fn the_account_backend_accepts_a_members_reset() {
    use app_core::sync::approval::CandidateStep;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let member_account = pair.desktop.credentials.account;
    let (account, app) = owner_app(pair);

    let secrets = Arc::new(MemorySecrets::default());
    let (mut member, _) = runtime.block_on(reset_devices(&app.server, member_account, secrets));
    let mut candidate = runtime
        .block_on(member.service.request_reset_approval(app.workspace))
        .expect("the request");

    // The device list comes up between pulls, and a message ends a pull's
    // wait.
    let started = Instant::now();
    let screen = loop {
        let screen = AccountBackend::screen(&account);
        if matches!(&screen.phase, AccountPhase::SignedIn(view) if view.reset_requests.len() == 1) {
            break screen;
        }
        assert!(
            started.elapsed() < Duration::from_secs(45),
            "the request never showed: {screen:?}"
        );
        app_core::folder_sync::FolderSyncBackend::act(
            &account,
            app_core::folder_sync::FolderSyncAction::DismissRestore,
        );
        std::thread::sleep(Duration::from_secs(3));
    };
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    assert!(
        view.alerts.iter().any(|a| a.text.contains("changed")),
        "{:?}",
        view.alerts
    );
    AccountBackend::act(&account, AccountAction::AcceptReset(0));
    wait_for(
        &account,
        "the acceptance",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_approval.is_some()),
    );
    let CandidateStep::Code(shown) = runtime
        .block_on(member.service.poll_invited(&mut candidate))
        .expect("polled")
    else {
        panic!("a code");
    };
    wait_for(&account, "the code", |s| {
        matches!(
            &s.phase,
            AccountPhase::SignedIn(view)
                if view.reset_approval.as_ref().and_then(|a| a.code.as_ref()) == Some(&shown)
        )
    });
    candidate.confirm();
    AccountBackend::act(&account, AccountAction::ConfirmAcceptReset);
    wait_for(
        &account,
        "the commit",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_approval.is_none()),
    );
    let started = Instant::now();
    while !runtime
        .block_on(member.service.join_invited(&candidate))
        .unwrap_or(false)
    {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never accepted"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(account);
    drop(app);
    drop(runtime);
}

/// A lost link of your own chain stops every workspace, so a restart can't
/// lean on a folder for the way back: the screens offer Reset devices with
/// none open (§14).
#[test]
fn a_restart_after_a_lost_link_of_your_chain_offers_reset_devices() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let copy = runtime.block_on(pair.server.backup("before.sqlite3"));
    let _third = runtime.block_on(sync_support::approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    ));
    let base = pair.proxy.base.clone();
    let (account, app) = owner_app_at(pair, &base);
    drop(account);
    let restored = runtime.block_on(app.server.put_back(&copy, true));
    app.proxy.retarget(&restored.base);

    let account = LiveAccount::start(
        app.a.app_data.path().to_owned(),
        app.laptop.secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    wait_for(&account, "the way back", |s| {
        matches!(s.phase, AccountPhase::ChainStopped)
    });
    // The server's device page names its own address, not the proxy's.
    AccountBackend::act(
        &account,
        AccountAction::SetServer(restored.base.clone()),
    );
    AccountBackend::act(&account, AccountAction::StartReset);
    wait_for(&account, "the sign-in", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    drop(account);
    drop(app);
    drop(restored);
    drop(runtime);
}

/// An offer that lapses ends the acceptance on the accepting device, and
/// nothing behind it in the poll waits for you to cancel.
#[test]
fn an_expired_reset_acceptance_ends_on_its_own() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let member_account = pair.desktop.credentials.account;
    let base = pair.proxy.base.clone();
    let (account, app) = owner_app_at(pair, &base);
    let (mut member, _) = runtime.block_on(reset_devices(
        &app.server,
        member_account,
        Arc::new(MemorySecrets::default()),
    ));
    runtime
        .block_on(member.service.request_reset_approval(app.workspace))
        .expect("the request");
    let started = Instant::now();
    loop {
        let screen = AccountBackend::screen(&account);
        if matches!(&screen.phase, AccountPhase::SignedIn(view) if view.reset_requests.len() == 1) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(45),
            "the request never showed: {screen:?}"
        );
        app_core::folder_sync::FolderSyncBackend::act(
            &account,
            app_core::folder_sync::FolderSyncAction::DismissRestore,
        );
        std::thread::sleep(Duration::from_secs(3));
    }
    AccountBackend::act(&account, AccountAction::AcceptReset(0));
    wait_for(
        &account,
        "the acceptance",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_approval.is_some()),
    );
    app.proxy.forge(Box::new(|_, path| {
        path.ends_with("/reveal").then(|| {
            let body = sync_protocol::messages::ErrorBody {
                code: sync_protocol::codec::Text::new("offer_expired").unwrap(),
                message: sync_protocol::codec::Text::new("the offer expired").unwrap(),
                retry_after_seconds: None,
                detail: None,
            };
            (410, sync_protocol::codec::Encode::to_bytes(&body))
        })
    }));
    wait_for(
        &account,
        "the end of the acceptance",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_approval.is_none()),
    );
    drop(account);
    drop(app);
    drop(runtime);
}

/// A restore stops the workspace for a member's chain. The account backend
/// keeps out of it: a relaunch judges no chain there, so the stop stays one
/// that an `accept_reset` ends (§14).
#[test]
fn a_stopped_workspace_stays_out_of_the_account_backend() {
    use app_core::folder_sync::FolderSyncBackend;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let member = pair.desktop.credentials.account;
    let copy = runtime.block_on(pair.server.backup("before.sqlite3"));
    let _third = runtime.block_on(sync_support::approve(
        &pair.server,
        &mut pair.desktop,
        &mut [],
    ));
    let genesis = pair.desktop.credentials.reference_head.genesis;
    runtime
        .block_on(pair.service.follow_account(member, genesis))
        .expect("the owner keeps the head");
    let base = pair.proxy.base.clone();
    let (account, app) = owner_app_at(pair, &base);
    let restored = runtime.block_on(app.server.put_back(&copy, true));
    app.proxy.retarget(&restored.base);

    let started = Instant::now();
    loop {
        if FolderSyncBackend::screen(&account).restore.is_some() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the stop never showed"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // A relaunch opens the workspaces before any folder.
    drop(account);
    std::thread::sleep(Duration::from_millis(300));
    let account = LiveAccount::start(
        app.a.app_data.path().to_owned(),
        app.laptop.secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    wait_for(
        &account,
        "the account after the relaunch",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.devices.is_empty()),
    );
    drop(account);
    let device = app.laptop.id();
    let state = app.a.app_data.path().join("sync").join(device.to_hex());
    let heads = KeptHeads::open(
        &state,
        app.laptop.credentials.account,
        device,
        &*app.laptop.secrets,
    )
    .expect("the heads open");
    assert_eq!(heads.stopped([member]), None, "no lasting refusal");
    drop(app);
    drop(restored);
    drop(runtime);
}

/// A computer on the retired chain (§7 step 3): the next read shows that
/// the account's devices were reset with the new fingerprint, it signs in
/// as a new device, the reset device approves it, and it signs in again
/// with new keys.
#[test]
fn a_retired_computer_comes_back_through_the_new_chain() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let (account, app) = owner_app(pair);
    let owner = app.laptop.credentials.account;
    let old = app.laptop.id();

    let (mut card, done) = runtime.block_on(reset_devices(
        &app.server,
        owner,
        Arc::new(MemorySecrets::default()),
    ));
    let screen = wait_for(&account, "the retired screen", |s| {
        matches!(s.phase, AccountPhase::Retired { .. })
    });
    let AccountPhase::Retired { fingerprint } = screen.phase else {
        unreachable!()
    };
    assert!(!fingerprint.is_empty());
    drop(done);

    AccountBackend::act(&account, AccountAction::SignIn);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    runtime
        .block_on(
            app.server
                .store
                .decide_device_grant(owner, &user_code, true),
        )
        .unwrap();
    let waiting = wait_for(&account, "the approval", |s| {
        matches!(s.phase, AccountPhase::WaitingForApproval { .. })
    });
    let AccountPhase::WaitingForApproval { fingerprint, .. } = waiting.phase else {
        unreachable!()
    };
    let pending = runtime
        .block_on(card.service.pending_devices())
        .expect("listed");
    assert_eq!(pending.len(), 1);
    let mut offer = runtime
        .block_on(card.service.offer_approval(&pending[0], &mut []))
        .expect("offered");
    let shown = wait_for(&account, "the code", |s| {
        matches!(
            &s.phase,
            AccountPhase::WaitingForApproval { code: Some(_), .. }
        )
    });
    let AccountPhase::WaitingForApproval {
        code: Some(shown), ..
    } = shown.phase
    else {
        unreachable!()
    };
    assert_eq!(
        runtime
            .block_on(card.service.approval_code(&mut offer))
            .expect("read"),
        Some(shown)
    );
    AccountBackend::act(&account, AccountAction::ConfirmCode);
    wait_for(&account, "the confirmation", |s| {
        matches!(
            s.phase,
            AccountPhase::WaitingForApproval {
                confirmed: true,
                ..
            }
        )
    });
    runtime
        .block_on(card.service.confirm_approval(offer, &mut []))
        .expect("committed");
    let screen = wait_for(&account, "the new device", |s| {
        matches!(s.phase, AccountPhase::SignedIn(_))
    });
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    let this = view
        .devices
        .iter()
        .find(|d| d.this_device)
        .expect("this device");
    assert_ne!(this.device, old);
    assert_eq!(this.fingerprint, fingerprint);
    drop(account);
    drop(app);
    drop(runtime);
}

/// A reset owner with no other owner or admin (§14): the reset screen
/// offers Set up sync again for the folder, and the action starts a new
/// workspace from it; the member finds the old one closed.
#[test]
fn the_account_backend_sets_up_sync_again_after_a_reset() {
    use app_core::folder_sync::{FolderSyncBackend, FolderSyncPhase};
    use app_core::sync::members::MemberChange;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut pair = runtime.block_on(async { pair_of_accounts_on(Server::start().await).await });
    let member = pair.desktop.credentials.account;
    runtime.block_on(async {
        pair.sync_a
            .change_member(
                &mut pair.service,
                MemberChange::SetRole {
                    account: member,
                    role: sync_protocol::records::Role::Editor,
                },
            )
            .await
            .expect("no other admin");
    });
    let (account, mut app) = owner_app(pair);
    let owner = app.laptop.credentials.account;
    let files = app.a.files();

    AccountBackend::act(&account, AccountAction::StartReset);
    let screen = wait_for(&account, "the sign-in code", |s| {
        matches!(s.phase, AccountPhase::SigningIn { .. })
    });
    let AccountPhase::SigningIn { user_code } = screen.phase else {
        unreachable!()
    };
    runtime
        .block_on(
            app.server
                .store
                .decide_device_grant(owner, &user_code, true),
        )
        .unwrap();
    wait_for(&account, "the code field", |s| {
        matches!(s.phase, AccountPhase::ResetCode)
    });
    let code = app
        .server
        .mail
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|text: &String| text.split_whitespace().nth(5).map(str::to_owned))
        .expect("the code's mail");
    AccountBackend::act(
        &account,
        AccountAction::ConfirmReset(code.trim_end_matches('.').to_owned()),
    );
    let screen = wait_for(
        &account,
        "the workspace that waits",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if !view.reset_waits.is_empty()),
    );
    let AccountPhase::SignedIn(view) = screen.phase else {
        unreachable!()
    };
    assert!(view.reset_waits[0].set_up_again);

    // A restart leaves nothing in memory: the wait comes back from the
    // server, and with it what this folder offers (§7, §14).
    drop(account);
    std::thread::sleep(Duration::from_millis(300));
    let account = LiveAccount::start(
        app.a.app_data.path().to_owned(),
        app.laptop.secrets.clone(),
        Arc::new(|_| Ok(())),
    );
    FolderSyncBackend::attach(&account, Some(app.a.engine.clone()));
    wait_for(
        &account,
        "the wait after the restart",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_waits.first().is_some_and(|w| w.set_up_again)),
    );

    AccountBackend::act(&account, AccountAction::SetUpAfterReset(0));
    wait_for(
        &account,
        "the wait's end",
        |s| matches!(&s.phase, AccountPhase::SignedIn(view) if view.reset_waits.is_empty()),
    );
    let started = Instant::now();
    loop {
        let screen = FolderSyncBackend::screen(&account);
        if screen.phase == FolderSyncPhase::On && screen.pending == 0 && !screen.busy {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the folder never synced: {screen:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(app.a.files(), files);

    runtime.block_on(async {
        app.sync_b
            .round(&mut app.desktop.service, 0)
            .await
            .expect("a stopped round");
    });
    assert_eq!(
        app.sync_b.restore_lost(),
        Some(&app_core::sync::workspace::restore::Lost::Closed)
    );
    assert_eq!(app.b.files(), files);
    drop(account);
    drop(app);
    drop(runtime);
}
