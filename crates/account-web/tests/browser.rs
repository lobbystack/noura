//! account-web in a headless browser: the real sync server serves the
//! account pages and the module, and Chrome with a virtual WebAuthn
//! authenticator signs up with a passkey, confirms the email through the
//! mailed link, adds a second passkey and signs in with each one. The pages
//! block inline scripts.
//!
//! The test needs Chrome or Chrome's headless shell, which `NOURA_CHROME`
//! names, and the module that `scripts/build-account-web.sh` builds, which
//! `NOURA_ACCOUNT_WEB_DIR` names (`target/account-web` by default). CI runs
//! it with `--ignored`:
//!
//! ```text
//! scripts/build-account-web.sh --debug
//! NOURA_CHROME=/path/to/chrome cargo test -p account-web --test browser -- --ignored
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::web_authn::{
    AddCredentialParams, AddVirtualAuthenticatorParams, AuthenticatorId, AuthenticatorProtocol,
    AuthenticatorTransport, Credential, EnableParams, GetCredentialsParams,
    RemoveVirtualAuthenticatorParams, VirtualAuthenticatorOptions,
};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use sync_server::accounts::mail::{Mail, MailTransport, SendFuture};
use sync_server::{Accounts, Allowlist};

#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<Mail>>>);

impl MailTransport for Outbox {
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a> {
        self.0.lock().unwrap().push(mail.clone());
        Box::pin(async { Ok(()) })
    }
}

fn assets() -> PathBuf {
    std::env::var_os("NOURA_ACCOUNT_WEB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/account-web")
        })
}

async fn eval<T: serde::de::DeserializeOwned>(page: &Page, script: &str) -> T {
    page.evaluate(script)
        .await
        .unwrap_or_else(|e| panic!("{script}: {e}"))
        .into_value()
        .unwrap_or_else(|e| panic!("{script}: {e}"))
}

/// Waits until `script` evaluates to true.
async fn until(page: &Page, script: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let done: bool = page
            .evaluate(script)
            .await
            .ok()
            .and_then(|r| r.into_value().ok())
            .unwrap_or(false);
        if done {
            return;
        }
        if Instant::now() > deadline {
            let body: String = eval(page, "document.body ? document.body.innerText : ''").await;
            panic!("timed out on {script}; the page shows: {body}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn open(page: &Page, url: &str) {
    page.goto(url).await.expect("the page loads");
    until(page, "document.readyState === 'complete'").await;
}

async fn authenticator(page: &Page) -> AuthenticatorId {
    let mut options = VirtualAuthenticatorOptions::new(
        AuthenticatorProtocol::Ctap2,
        AuthenticatorTransport::Internal,
    );
    options.has_resident_key = Some(true);
    options.has_user_verification = Some(true);
    options.is_user_verified = Some(true);
    options.automatic_presence_simulation = Some(true);
    page.execute(AddVirtualAuthenticatorParams::new(options))
        .await
        .expect("a virtual authenticator")
        .result
        .authenticator_id
}

async fn credentials(page: &Page, id: &AuthenticatorId) -> Vec<Credential> {
    page.execute(GetCredentialsParams::new(id.clone()))
        .await
        .unwrap()
        .result
        .credentials
}

async fn remove(page: &Page, id: AuthenticatorId) {
    page.execute(RemoveVirtualAuthenticatorParams::new(id))
        .await
        .unwrap();
}

/// Presses the passkey button of form `kind` as you would, once the module
/// showed it, after filling the form's email.
async fn passkey(page: &Page, kind: &str, email: Option<&str>) {
    let form = format!("document.querySelector('form[data-passkey={kind}]')");
    let button = format!("{form}.querySelector('[data-passkey-start]')");
    until(
        page,
        &format!("{form} !== null && {button}.hidden === false"),
    )
    .await;
    if let Some(email) = email {
        let _: bool = eval(
            page,
            &format!("{form}.querySelector('input[name=email]').value = '{email}', true"),
        )
        .await;
    }
    let _: bool = eval(page, &format!("{button}.click(), true")).await;
}

async fn sign_out(page: &Page) {
    let _: bool = eval(
        page,
        "document.querySelector('form[action=\"/account/sign-out\"]').requestSubmit(), true",
    )
    .await;
    until(page, "location.pathname === '/account/sign-in'").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Chrome, which NOURA_CHROME names, and the built module"]
async fn a_browser_signs_up_with_a_passkey_adds_another_and_signs_in_with_either() {
    let chrome =
        std::env::var_os("NOURA_CHROME").expect("NOURA_CHROME names Chrome or its headless shell");
    let assets = assets();
    assert!(
        assets.join("account_web_bg.wasm").exists(),
        "run scripts/build-account-web.sh first; {} holds no module",
        assets.display()
    );

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.unwrap();
    let store = Arc::new(
        sync_server::open_store(&sync_server::Config::new(
            data,
            "127.0.0.1:0".parse().unwrap(),
        ))
        .await
        .unwrap(),
    );
    let outbox = Outbox::default();
    let mailer = outbox.clone();
    let address = sync_server::spawn_with(store, "127.0.0.1:0", move |origin| {
        Accounts::new(
            origin,
            Allowlist::parse("ada@example.com"),
            "Noura <noura@example.com>".into(),
            Box::new(mailer),
            Some(assets),
        )
    })
    .await
    .unwrap();
    let base = format!("http://localhost:{}", address.port());

    let profile = tempfile::tempdir().unwrap();
    let (mut browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .chrome_executable(chrome)
            .user_data_dir(profile.path())
            // CI runners keep unprivileged user namespaces off, which
            // Chrome's sandbox needs; the test loads only its own server.
            .no_sandbox()
            .build()
            .expect("a browser configuration"),
    )
    .await
    .expect("Chrome starts");
    let events = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser.new_page("about:blank").await.unwrap();
    page.execute(EnableParams::default()).await.unwrap();
    let laptop = authenticator(&page).await;

    // The notice ships with the module.
    open(
        &page,
        &format!("{base}/account/assets/THIRD_PARTY_NOTICES.txt"),
    )
    .await;
    let notice: String = eval(&page, "document.body.innerText").await;
    assert!(notice.contains("webauthn-rs-proto 0.5.5"));
    assert!(notice.contains("Mozilla Public License Version 2.0"));

    // The pages run no inline script: one the page adds never runs.
    open(&page, &format!("{base}/account/sign-up")).await;
    let _: bool = eval(
        &page,
        "(() => { document.addEventListener('securitypolicyviolation', e => { window.blocked = e.effectiveDirective; }); const s = document.createElement('script'); s.textContent = 'window.inlineRan = true'; document.body.appendChild(s); })(), true",
    )
    .await;
    until(&page, "typeof window.blocked === 'string'").await;
    let blocked: String = eval(&page, "window.blocked").await;
    assert!(blocked.starts_with("script-src"), "{blocked}");
    let ran: bool = eval(&page, "window.inlineRan === true").await;
    assert!(!ran, "the content security policy blocks inline scripts");
    // The module itself loaded and showed the passkey form.
    until(
        &page,
        "document.querySelector('form[data-passkey=sign-up] [data-passkey-start]').hidden === false",
    )
    .await;
    if let Some(shots) = std::env::var_os("NOURA_SCREENSHOTS") {
        let shot = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .full_page(true)
                    .build(),
            )
            .await
            .unwrap();
        std::fs::write(PathBuf::from(shots).join("sign-up.png"), shot).unwrap();
    }

    // Sign up with a passkey, then confirm the email.
    passkey(&page, "sign-up", Some("Ada@Example.com")).await;
    until(
        &page,
        "document.querySelector('form[data-passkey=sign-up] [data-status]').innerText.includes('Check your email')",
    )
    .await;
    let mail = outbox.0.lock().unwrap().pop().expect("a confirmation mail");
    let link = mail
        .text
        .split_whitespace()
        .find(|w| w.starts_with(&base))
        .expect("the link")
        .to_owned();
    open(&page, &link).await;
    let _: bool = eval(
        &page,
        "document.querySelector('form').requestSubmit(), true",
    )
    .await;
    until(&page, "location.pathname === '/account'").await;
    until(&page, "document.body.innerText.includes('ada@example.com')").await;
    until(
        &page,
        "document.body.innerText.includes('You have 1 passkey.')",
    )
    .await;
    let first = credentials(&page, &laptop).await;
    assert_eq!(first.len(), 1);

    // Add a second passkey on another authenticator.
    remove(&page, laptop).await;
    let phone = authenticator(&page).await;
    passkey(&page, "add", None).await;
    until(
        &page,
        "document.body.innerText.includes('You have 2 passkeys.')",
    )
    .await;
    if let Some(shots) = std::env::var_os("NOURA_SCREENSHOTS") {
        let shot = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .full_page(true)
                    .build(),
            )
            .await
            .unwrap();
        std::fs::write(PathBuf::from(shots).join("account.png"), shot).unwrap();
    }

    // Sign in with the second passkey.
    sign_out(&page).await;
    passkey(&page, "sign-in", Some("ada@example.com")).await;
    until(&page, "location.pathname === '/account'").await;
    until(&page, "document.body.innerText.includes('ada@example.com')").await;

    // And with the first, back on its own authenticator.
    sign_out(&page).await;
    remove(&page, phone).await;
    let laptop = authenticator(&page).await;
    page.execute(AddCredentialParams::new(laptop.clone(), first[0].clone()))
        .await
        .unwrap();
    passkey(&page, "sign-in", Some("ada@example.com")).await;
    until(&page, "location.pathname === '/account'").await;
    until(
        &page,
        "document.body.innerText.includes('You have 2 passkeys.')",
    )
    .await;

    browser.close().await.unwrap();
    let _ = browser.wait().await;
    events.abort();
}
