//! Signing a desktop in (seam 4 and seam 1, sync-v2.md §7): the app opens
//! the system browser on the server's device page, polls every 5 seconds
//! and 5 more after each `slow_down`, and receives a session token; it
//! enrolls fresh keys, signs its account's genesis as the first device,
//! and keeps its keys and token only in the credential store.

mod sync_support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_core::sync::SyncError;
use app_core::sync::device::{DeviceSignIn, Joined, Pause, SignInEnd, StoredDevice, join};
use sync_store::Backend;
use sync_support::{MemorySecrets, Server};

/// Waits by moving the server's clock, so each poll arrives on time, and
/// runs `on_wait` before the poll that follows the wait with that index.
struct ServerClock<B: Backend> {
    server: Arc<sync_store::Store<B>>,
    advance: bool,
    waits: Mutex<Vec<u64>>,
    on_wait: Mutex<Box<dyn FnMut(usize) -> Option<tokio::task::JoinHandle<()>> + Send>>,
}

impl<B: Backend> ServerClock<B> {
    fn new(server: Arc<sync_store::Store<B>>, advance: bool) -> Self {
        Self {
            server,
            advance,
            waits: Mutex::new(Vec::new()),
            on_wait: Mutex::new(Box::new(|_| None)),
        }
    }

    fn on_wait(
        &self,
        action: impl FnMut(usize) -> Option<tokio::task::JoinHandle<()>> + Send + 'static,
    ) {
        *self.on_wait.lock().unwrap() = Box::new(action);
    }
}

impl<B: Backend> Pause for ServerClock<B> {
    fn pause(
        &self,
        seconds: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let index = {
                let mut waits = self.waits.lock().unwrap();
                waits.push(seconds);
                waits.len() - 1
            };
            if self.advance {
                self.server.clock().advance(Duration::from_secs(seconds));
            }
            let task = (self.on_wait.lock().unwrap())(index);
            if let Some(task) = task {
                task.await.unwrap();
            }
        })
    }
}

/// A person who signed up on the server, with the account their email
/// gets.
async fn person<B: Backend>(server: &Server<B>, email: &str) -> sync_protocol::types::Id {
    let link = server.store.issue_magic_link(email).await.unwrap();
    server
        .store
        .sign_in_with_magic_link(&link)
        .await
        .unwrap()
        .expect("a current link")
        .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_device_signs_in_through_the_browser_and_starts_the_chain() {
    let server = Server::start().await;
    let account = person(&server, "ada@example.com").await;
    let opened = Arc::new(Mutex::new(Vec::new()));
    let record = opened.clone();
    let sign_in = DeviceSignIn::start(&server.base, &move |url| {
        record.lock().unwrap().push(url.to_owned());
        Ok(())
    })
    .await
    .expect("the grant starts");
    assert_eq!(
        *opened.lock().unwrap(),
        vec![format!("{}/device", server.base)],
        "the app opens the device page"
    );
    let user_code = sign_in.user_code().to_owned();
    assert_eq!(user_code.len(), 9, "two groups of 4: {user_code}");

    // You approve the code in the browser while the app polls.
    let clock = ServerClock::new(server.store.clone(), true);
    let store = server.store.clone();
    clock.on_wait(move |index| {
        (index == 1).then(|| {
            let store = store.clone();
            let code = user_code.clone();
            tokio::spawn(async move {
                assert!(
                    store
                        .decide_device_grant(account, &code, true)
                        .await
                        .unwrap()
                );
            })
        })
    });
    let signed_in = sign_in.finish(&clock).await.expect("signed in");
    assert_eq!(signed_in.account, account);
    assert_eq!(*clock.waits.lock().unwrap(), vec![5, 5]);

    let secrets = Arc::new(MemorySecrets::default());
    let credentials = match join(&signed_in, &*secrets).await.expect("enrolled") {
        Joined::First(credentials) => credentials,
        Joined::NeedsApproval { .. } => panic!("the first device starts the chain"),
    };
    let stored = StoredDevice::load(&*secrets, &server.base)
        .unwrap()
        .expect("the credential store keeps the device");
    assert_eq!(
        stored.reference_head.as_ref(),
        Some(&credentials.reference_head)
    );
    assert_eq!(credentials.reference_head.seqno, 1);

    // The device works: it reads its chain and creates a workspace.
    let state = tempfile::tempdir().unwrap();
    let heads = app_core::sync::KeptHeads::open(
        state.path(),
        account,
        credentials.keys.device(),
        &*secrets,
    )
    .unwrap();
    let mut service = app_core::sync::SyncService::connect(
        &server.base,
        credentials.clone(),
        heads,
        secrets.clone(),
    )
    .await
    .expect("connects");
    let chain = service.own_chain().await.expect("the chain verifies");
    assert!(chain.is_active_at(&credentials.keys.device(), 1));
    service
        .create_workspace("Field notes")
        .await
        .expect("the first device creates a workspace");

    // Its secrets stay in the credential store: no file in its
    // application data holds the token or either seed.
    let token = credentials.session_token.clone();
    let seeds = [
        sync_protocol::types::to_hex(credentials.keys.signing().seed().expose()),
        sync_protocol::types::to_hex(credentials.keys.hpke_seed().expose()),
    ];
    for file in files(state.path()) {
        let bytes = std::fs::read(&file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(&token), "{} holds the token", file.display());
        for seed in &seeds {
            assert!(
                !text.contains(seed.as_str()),
                "{} holds a seed",
                file.display()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polling_slows_down_and_a_refused_or_expired_code_fails_clearly() {
    let server = Server::start().await;
    let account = person(&server, "bea@example.com").await;
    let no_browser = |_: &str| Ok(());

    // A poll the server calls too soon adds 5 seconds to the interval.
    let sign_in = DeviceSignIn::start(&server.base, &no_browser)
        .await
        .unwrap();
    let code = sign_in.user_code().to_owned();
    let clock = ServerClock::new(server.store.clone(), false);
    let store = server.store.clone();
    clock.on_wait(move |index| {
        (index == 2).then(|| {
            let store = store.clone();
            let code = code.clone();
            tokio::spawn(async move {
                store.clock().advance(Duration::from_secs(60));
                store
                    .decide_device_grant(account, &code, true)
                    .await
                    .unwrap();
            })
        })
    });
    sign_in.finish(&clock).await.expect("signed in");
    let waits = clock.waits.lock().unwrap().clone();
    assert_eq!(&waits[..3], &[5, 5, 10], "{waits:?}");

    // You refuse the code in the browser.
    let sign_in = DeviceSignIn::start(&server.base, &no_browser)
        .await
        .unwrap();
    let code = sign_in.user_code().to_owned();
    let clock = ServerClock::new(server.store.clone(), true);
    let store = server.store.clone();
    clock.on_wait(move |index| {
        (index == 0).then(|| {
            let store = store.clone();
            let code = code.clone();
            tokio::spawn(async move {
                store
                    .decide_device_grant(account, &code, false)
                    .await
                    .unwrap();
            })
        })
    });
    let refused = sign_in.finish(&clock).await.expect_err("refused");
    assert!(matches!(refused, SyncError::SignIn(SignInEnd::Refused)));
    assert!(refused.to_string().contains("You refused the sign-in"));

    // Nobody approves the code within 10 minutes.
    let sign_in = DeviceSignIn::start(&server.base, &no_browser)
        .await
        .unwrap();
    let clock = ServerClock::new(server.store.clone(), false);
    let store = server.store.clone();
    clock.on_wait(move |index| {
        (index == 0).then(|| {
            let store = store.clone();
            tokio::spawn(async move { store.clock().advance(Duration::from_secs(601)) })
        })
    });
    let expired = sign_in.finish(&clock).await.expect_err("expired");
    assert!(matches!(expired, SyncError::SignIn(SignInEnd::Expired)));
    assert!(expired.to_string().contains("expired"));
}

/// Every file under `dir`.
fn files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}
