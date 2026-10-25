//! Signing a desktop in (seam 4 and seam 1, sync-v2.md §7): the app opens
//! the system browser on the server's device page, polls every 5 seconds
//! and 5 more after each `slow_down`, and receives a session token; it
//! enrolls fresh keys, signs its account's genesis as the first device,
//! and keeps its keys and token only in the credential store.

mod sync_support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_core::sync::device::{
    DeviceSignIn, Joined, Pause, SignInEnd, StoredDevice, join, retired_item,
};
use app_core::sync::{KeptHeads, SyncClient, SyncError, SyncService};
use local_core::sync::SyncCredentials as _;
use sync_store::{Backend, WriteTxn as _};
use sync_support::{MemorySecrets, Proxy, Server, approve, first_desktop};

/// What runs before the poll that follows a wait: it may start a task the
/// wait then finishes.
type OnWait = Box<dyn FnMut(usize) -> Option<tokio::task::JoinHandle<()>> + Send>;

/// Waits by moving the server's clock, so each poll arrives on time, and
/// runs `on_wait` before the poll that follows the wait with that index.
struct ServerClock<B: Backend> {
    server: Arc<sync_store::Store<B>>,
    advance: bool,
    waits: Mutex<Vec<u64>>,
    on_wait: Mutex<OnWait>,
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
        _ => panic!("the first device starts the chain"),
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

/// A device whose session row a server restore lost signs in again: it
/// enrolls its existing card, which the server binds to the new session, and
/// keeps its keys, its reference head and its workspaces (§7, §14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_whose_session_was_lost_enrolls_its_card_again() {
    let server = Server::start().await;
    let account = person(&server, "cy@example.com").await;
    let signed_in = server.sign_in(account, &server.base).await;
    let secrets = Arc::new(MemorySecrets::default());
    let first = match join(&signed_in, &*secrets).await.expect("enrolled") {
        Joined::First(credentials) => credentials,
        _ => panic!("the first device starts the chain"),
    };
    let device = first.keys.device();
    let state = tempfile::tempdir().unwrap();
    let mut service = SyncService::connect(
        &server.base,
        first.clone(),
        KeptHeads::open(state.path(), account, device, &*secrets).unwrap(),
        secrets.clone(),
    )
    .await
    .unwrap();
    let mut workspace = service.create_workspace("Notes").await.unwrap();
    service
        .push_file(&mut workspace, "Kept.md", b"# Kept\n")
        .await
        .unwrap();
    drop(service);

    // A restore loses the session's row.
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                txn.delete_device_sessions(account, device)
                    .await
                    .map(|_| ())
            })
        }))
        .await
        .unwrap();
    let refused = SyncClient::connect(&server.base, &first.session_token)
        .await
        .unwrap()
        .workspaces()
        .await
        .expect_err("the session is gone");
    assert_eq!(refused.code(), Some("unauthorized"));

    // Signing in again binds a new session to the same card.
    let again = server.sign_in(account, &server.base).await;
    let credentials = match join(&again, &*secrets).await.expect("enrolled again") {
        Joined::Again(credentials) => credentials,
        _ => panic!("the device keeps its card"),
    };
    assert_eq!(credentials.keys.device(), device);
    assert_eq!(
        credentials.keys.signing().seed().expose(),
        first.keys.signing().seed().expose()
    );
    assert_eq!(credentials.reference_head, first.reference_head);
    assert_eq!(credentials.session_token, again.session_token);
    assert_ne!(credentials.session_token, first.session_token);
    let stored = StoredDevice::load(&*secrets, &server.base)
        .unwrap()
        .unwrap();
    assert_eq!(stored.session_token(), again.session_token);

    let mut service = SyncService::connect(
        &server.base,
        credentials,
        KeptHeads::open(state.path(), account, device, &*secrets).unwrap(),
        secrets.clone(),
    )
    .await
    .unwrap();
    let chain = service.own_chain().await.unwrap();
    assert_eq!(
        chain.active_at(chain.head_seqno()).len(),
        1,
        "no new card joined the chain"
    );
    let mut remote = service.open_workspace(workspace.workspace).await.unwrap();
    let files = service.pull_files(&mut remote).await.unwrap();
    assert!(files.iter().any(|f| f.bytes == b"# Kept\n"));
    let listed = service.client().devices().await.unwrap();
    assert_eq!(listed.devices.0.len(), 1);
    assert_eq!(listed.devices.0[0].sessions, 1, "the new session alone");
}

/// Signing in again never overwrites a stored device's keys: a card a link
/// removed keeps its secrets aside and the device enrolls fresh keys, and a
/// joined device of another account stays as it is (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signing_in_again_never_overwrites_a_stored_device() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "dee@example.com").await;
    let account = laptop.credentials.account;
    let phone = approve(&server, &mut laptop, &mut []).await;
    let old = StoredDevice::load(&*phone.secrets, &server.base)
        .unwrap()
        .expect("the phone's device");
    laptop
        .service
        .remove_device(phone.id(), &[], &mut [])
        .await
        .unwrap();

    // The removed phone signs in again: its card gets id_conflict, so its
    // secrets move aside and it enrolls a new card that waits for approval.
    let signed_in = server.sign_in(account, &server.base).await;
    let keys = match join(&signed_in, &*phone.secrets).await.expect("enrolled") {
        Joined::NeedsApproval { keys, .. } => keys,
        _ => panic!("a new card waits for approval"),
    };
    assert_ne!(keys.device(), phone.id());
    let retired = phone
        .secrets
        .read_optional(&retired_item(&server.base, &phone.id()))
        .unwrap()
        .expect("the removed card's secrets stay");
    assert!(retired.contains(old.session_token()));
    let stored = StoredDevice::load(&*phone.secrets, &server.base)
        .unwrap()
        .unwrap();
    assert_eq!(stored.card().unwrap().device, keys.device());

    // A computer whose device belongs to another account keeps it.
    let other = person(&server, "eli@example.com").await;
    let signed_in = server.sign_in(other, &server.base).await;
    let refused = join(&signed_in, &*laptop.secrets)
        .await
        .err()
        .expect("another account's device stays");
    assert!(refused.to_string().contains("another account"), "{refused}");
    let kept = StoredDevice::load(&*laptop.secrets, &server.base)
        .unwrap()
        .unwrap();
    assert_eq!(kept.card().unwrap().device, laptop.id());
}

/// The app opens only the device page on the server's own address: a grant
/// that names a page elsewhere, as the server behind this proxy does, opens
/// nothing (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_page_on_another_address_isnt_opened() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let record = opened.clone();
    let refused = DeviceSignIn::start(&proxy.base, &move |url| {
        record.lock().unwrap().push(url.to_owned());
        Ok(())
    })
    .await
    .err()
    .expect("the page is elsewhere");
    assert!(
        refused.to_string().contains("outside its own address"),
        "{refused}"
    );
    assert!(opened.lock().unwrap().is_empty(), "no browser opened");
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
