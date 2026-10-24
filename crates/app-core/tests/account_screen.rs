//! The desktop's account backend against the real server (seam 1 and
//! seam 4): you type the server address, sign in through the browser,
//! and the screens show the account's fingerprint and this device; you
//! export a kit, whose secret shows once, and the last device's sign-out
//! warns before it leaves the credential store empty.

mod sync_support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::account::live::LiveAccount;
use app_core::account::{AccountAction, AccountBackend, AccountPhase, AccountScreen};
use app_core::sync::device::StoredDevice;
use sync_support::{MemorySecrets, Server};

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
