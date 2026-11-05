//! account-web in a headless browser: the real sync server serves the
//! account pages and the module, and Chrome with a virtual WebAuthn
//! authenticator signs up with a passkey, confirms the email through the
//! mailed link, adds a second passkey and signs in with each one. The pages
//! block inline scripts, and with script turned off their email-link forms
//! still sign you in.
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
use chromiumoxide::cdp::browser_protocol::emulation::SetScriptExecutionDisabledParams;
use chromiumoxide::cdp::browser_protocol::web_authn::{
    AddCredentialParams, AddVirtualAuthenticatorParams, AuthenticatorId, AuthenticatorProtocol,
    AuthenticatorTransport, Credential, EnableParams, GetCredentialsParams,
    RemoveVirtualAuthenticatorParams, VirtualAuthenticatorOptions,
};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use sync_server::accounts::Deliveries;
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

/// The real sync server with the account pages and the built module, an
/// allowlist that admits ada@example.com, and mail in an outbox.
struct Site {
    base: String,
    store: Arc<sync_store::Store<sync_store_sqlite::SqliteBackend>>,
    outbox: Outbox,
    /// The server mails after it answers; the tests wait for the sends.
    deliveries: Deliveries,
    _dir: tempfile::TempDir,
}

async fn site() -> Site {
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
    let deliveries = Deliveries::default();
    let counted = deliveries.clone();
    let address = sync_server::spawn_with(store.clone(), "127.0.0.1:0", move |origin| {
        Accounts::new(
            origin,
            Allowlist::parse("ada@example.com"),
            "Noura <noura@example.com>".into(),
            Box::new(mailer),
            Some(assets),
        )
        .with_deliveries(counted)
    })
    .await
    .unwrap();
    Site {
        base: format!("http://localhost:{}", address.port()),
        store,
        outbox,
        deliveries,
        _dir: dir,
    }
}

/// Chrome, from `NOURA_CHROME`, with a fresh profile, and the task that
/// drives its DevTools connection.
async fn launch() -> (Browser, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let chrome =
        std::env::var_os("NOURA_CHROME").expect("NOURA_CHROME names Chrome or its headless shell");
    let profile = tempfile::tempdir().unwrap();
    let (browser, mut handler) = Browser::launch(
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
    (browser, events, profile)
}

/// The first link to `base` in the last mail the outbox holds.
async fn mailed_link(site: &Site) -> String {
    site.deliveries.idle().await;
    let mail = site.outbox.0.lock().unwrap().pop().expect("a mail");
    mail.text
        .split_whitespace()
        .find(|w| w.starts_with(&site.base))
        .expect("the link")
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Chrome, which NOURA_CHROME names, and the built module"]
async fn a_browser_signs_up_with_a_passkey_adds_another_and_signs_in_with_either() {
    let Site {
        base,
        outbox,
        deliveries,
        _dir,
        ..
    } = site().await;
    let (mut browser, events, _profile) = launch().await;
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
    deliveries.idle().await;
    let mail = outbox.0.lock().unwrap().pop().expect("a confirmation mail");
    let link = mail
        .text
        .split_whitespace()
        .find(|w| w.starts_with(&base))
        .expect("the link")
        .to_owned();
    assert_eq!(mail.subject, "Confirm your Noura account");
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

/// With script turned off, the module never runs and the pages still work
/// as plain forms: the email-link form mails a link, and the link's page
/// signs you in with its one button.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Chrome, which NOURA_CHROME names, and the built module"]
async fn the_email_link_forms_work_with_script_turned_off() {
    let site = site().await;
    let (mut browser, events, _profile) = launch().await;
    let page = browser.new_page("about:blank").await.unwrap();
    page.execute(SetScriptExecutionDisabledParams::new(true))
        .await
        .unwrap();
    open(&page, &format!("{}/account/sign-in", site.base)).await;
    // The module never ran: it shows the passkey button once it loads.
    let hidden: bool = eval(
        &page,
        "document.querySelector('[data-passkey-start]').hidden",
    )
    .await;
    assert!(hidden, "the module ran with script turned off");

    // The test fills the field and submits the form as the browser's own
    // submit does, with no script of the page.
    let _: bool = eval(
        &page,
        "document.querySelector('input[name=email]').value = 'Ada@Example.com', true",
    )
    .await;
    let _: bool = eval(
        &page,
        "document.querySelector('form[action=\"/account/email-link\"]').requestSubmit(), true",
    )
    .await;
    until(
        &page,
        "document.body.innerText.includes('Check your email')",
    )
    .await;
    let link = mailed_link(&site).await;
    open(&page, &link).await;
    let _: bool = eval(
        &page,
        "document.querySelector('form').requestSubmit(), true",
    )
    .await;
    until(&page, "location.pathname === '/account'").await;
    until(&page, "document.body.innerText.includes('ada@example.com')").await;
    let still_hidden: bool = eval(
        &page,
        "document.querySelector('[data-passkey-start]').hidden",
    )
    .await;
    assert!(still_hidden);

    browser.close().await.unwrap();
    let _ = browser.wait().await;
    events.abort();
}

/// A desktop signs in through the device page (§7): you sign in to the
/// account pages, enter the code the app shows in any case, and approve
/// it, and the app's next poll receives a session of your account.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Chrome, which NOURA_CHROME names, and the built module"]
async fn a_browser_approves_a_device_code_sign_in() {
    let site = site().await;
    let (mut browser, events, _profile) = launch().await;
    let page = browser.new_page("about:blank").await.unwrap();
    let grant = site
        .store
        .authorize_device(&format!("{}/device", site.base))
        .await
        .unwrap();
    let user_code = grant.user_code.as_str().to_owned();
    let device_code = grant.device_code.as_str().to_owned();

    // The device page asks you to sign in first.
    open(&page, &format!("{}/device", site.base)).await;
    until(
        &page,
        "document.body.innerText.includes('Sign in to your account first')",
    )
    .await;
    open(&page, &format!("{}/account/sign-in", site.base)).await;
    let _: bool = eval(
        &page,
        "document.querySelector('input[name=email]').value = 'ada@example.com', true",
    )
    .await;
    let _: bool = eval(
        &page,
        "document.querySelector('form[action=\"/account/email-link\"]').requestSubmit(), true",
    )
    .await;
    until(
        &page,
        "document.body.innerText.includes('Check your email')",
    )
    .await;
    let link = mailed_link(&site).await;
    open(&page, &link).await;
    let _: bool = eval(
        &page,
        "document.querySelector('form').requestSubmit(), true",
    )
    .await;
    until(&page, "location.pathname === '/account'").await;

    // Until you approve, the app's poll waits.
    let pending = site.store.poll_device_token(&device_code).await;
    assert!(
        pending
            .unwrap_err()
            .to_string()
            .contains("authorization_pending")
    );
    open(&page, &format!("{}/device", site.base)).await;
    let typed = user_code.to_lowercase();
    let _: bool = eval(
        &page,
        &format!("document.querySelector('input[name=user_code]').value = '{typed}', true"),
    )
    .await;
    let _: bool = eval(
        &page,
        "document.querySelector('form').requestSubmit(), true",
    )
    .await;
    until(
        &page,
        &format!("document.body.innerText.includes('{user_code}')"),
    )
    .await;
    let _: bool = eval(
        &page,
        "document.querySelector('button[value=approve]').click(), true",
    )
    .await;
    until(
        &page,
        "document.body.innerText.includes('Device signed in')",
    )
    .await;
    site.store
        .clock()
        .advance(std::time::Duration::from_secs(10));
    let signed_in = site
        .store
        .poll_device_token(&device_code)
        .await
        .expect("the app receives its session");
    assert_eq!(signed_in.session_token.as_str().len(), 43);

    browser.close().await.unwrap();
    let _ = browser.wait().await;
    events.abort();
}

/// Replaces the stored snapshot of a link with `bytes`, as a server that
/// tampers with it would.
async fn store_snapshot(site: &Site, token_hash: sync_protocol::types::Digest, bytes: Vec<u8>) {
    type Write = <sync_store_sqlite::SqliteBackend as sync_store::Backend>::Write;
    sync_store::Backend::write(
        site.store.backend(),
        Box::new(move |txn: &mut Write| {
            Box::pin(async move { sync_store::WriteTxn::set_share(txn, token_hash, &bytes).await })
        }),
    )
    .await
    .unwrap();
}

/// Opens `url` as a new document, even when only its fragment differs
/// from the page's, which a browser would otherwise follow in place.
async fn visit(page: &Page, url: &str) {
    open(page, "about:blank").await;
    open(page, url).await;
}

/// What the viewer page shows: its status line, whether the note shows,
/// and the note's text.
async fn viewer(page: &Page) -> (String, bool, String) {
    let status: String = eval(
        page,
        "(document.querySelector('[data-share-status]') || document.body).innerText",
    )
    .await;
    let shown: bool = eval(
        page,
        "!!document.querySelector('[data-share-note]') && !document.querySelector('[data-share-note]').hidden",
    )
    .await;
    let text: String = eval(
        page,
        "(document.querySelector('[data-share-body]') || { innerText: '' }).innerText",
    )
    .await;
    (status, shown, text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Chrome, which NOURA_CHROME names, and the built module"]
async fn a_browser_with_no_account_opens_a_share_link() {
    use sync_protocol::codec::Encode;
    use sync_protocol::records::share_fragment;
    use sync_protocol::types::{to_base64url, to_hex};
    use sync_store::conformance::{Link, World};

    let site = site().await;
    let logs = site._dir.path().join("logs");
    // No other test in this binary installs a subscriber, so the log check
    // below always runs: a failed install fails the test instead of
    // skipping it.
    sync_server::logs::init(&logs).expect("the server's log is the global subscriber");
    let world = World::on(site.store.clone(), 1).await;
    let owner = world.devices[0].session;
    let link = Link::new();
    let markdown = "# Steps\n\nTomatoes in **May**.\n\n![tracker](https://example.com/p.png)\n\n\
                    <img src=x onerror=\"window.pwned = 1\">\n\n<script>window.pwned = 2</script>\n";
    site.store
        .create_share(
            owner,
            world.workspace,
            link.creation(world.domain, 30, markdown),
        )
        .await
        .unwrap();
    let token = to_base64url(&link.token);
    let fragment = share_fragment(&link.key, &link.signing.public());
    let url = format!("{}/s/{token}#{fragment}", site.base);

    let (mut browser, events, _profile) = launch().await;
    let page = browser.new_page("about:blank").await.unwrap();

    // The notice ships with the module and names the viewer's crates.
    open(
        &page,
        &format!("{}/account/assets/THIRD_PARTY_NOTICES.txt", site.base),
    )
    .await;
    let notice: String = eval(&page, "document.body.innerText").await;
    assert!(notice.contains("pulldown-cmark") && notice.contains("ed25519-dalek 3.0.0"));

    // The note opens with no account: its title and Markdown, without the
    // image, the raw HTML or the script.
    visit(&page, &url).await;
    until(&page, "!document.querySelector('[data-share-note]').hidden").await;
    let title: String = eval(
        &page,
        "document.querySelector('[data-share-title]').innerText",
    )
    .await;
    assert_eq!(title, "Plan");
    let html: String = eval(
        &page,
        "document.querySelector('[data-share-body]').innerHTML",
    )
    .await;
    assert!(
        html.contains("<h1>Steps</h1>") && html.contains("<strong>May</strong>"),
        "{html}"
    );
    let images: u32 = eval(&page, "document.querySelectorAll('img').length").await;
    assert_eq!(images, 0, "embedded images stay out");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let pwned: bool = eval(&page, "window.pwned !== undefined").await;
    assert!(!pwned, "nothing in the note runs");
    if let Some(shots) = std::env::var_os("NOURA_SCREENSHOTS") {
        let shot = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .full_page(true)
                    .build(),
            )
            .await
            .unwrap();
        std::fs::write(PathBuf::from(shots).join("share.png"), shot).unwrap();
    }

    // The viewer page blocks inline scripts.
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

    // Update link: a reload shows the new version, and the viewer remembers
    // the highest revision it showed.
    site.store
        .update_share(
            owner,
            world.workspace,
            link.hash(),
            link.snapshot(2, "Peppers too."),
        )
        .await
        .unwrap();
    visit(&page, &url).await;
    until(&page, "!document.querySelector('[data-share-note]').hidden").await;
    let (_, _, text) = viewer(&page).await;
    assert_eq!(text, "Peppers too.");
    let key = format!("noura.share.revision.{}", to_hex(&link.hash().0));
    let kept: String = eval(&page, &format!("localStorage.getItem('{key}')")).await;
    assert_eq!(kept, "2");
    let warned: bool = eval(
        &page,
        "document.querySelector('[data-share-warning]').hidden === false",
    )
    .await;
    assert!(!warned);
    // A later visit that shows a lower revision than one shown before warns.
    let _: bool = eval(&page, &format!("localStorage.setItem('{key}', '5'), true")).await;
    visit(&page, &url).await;
    until(
        &page,
        "document.querySelector('[data-share-warning]').hidden === false",
    )
    .await;
    let warning: String = eval(
        &page,
        "document.querySelector('[data-share-warning]').innerText",
    )
    .await;
    assert!(warning.contains("older version"), "{warning}");

    // A wrong key, a tampered ciphertext and a bad signature show an error
    // page and render nothing.
    let other = Link::new();
    let wrong = format!(
        "{}/s/{token}#{}",
        site.base,
        share_fragment(&other.key, &link.signing.public())
    );
    let good = link.snapshot(3, "Fine.");
    let mut tampered = good.clone();
    tampered.body.ciphertext.0[0] ^= 1;
    let mut forged = good.clone();
    forged.sig.0[0] ^= 1;
    for (case, address, snapshot) in [
        ("a wrong key", wrong.clone(), good.clone()),
        ("a tampered ciphertext", url.clone(), tampered),
        ("a bad signature", url.clone(), forged),
    ] {
        store_snapshot(&site, link.hash(), snapshot.to_bytes()).await;
        visit(&page, &address).await;
        until(
            &page,
            "document.querySelector('[data-share-status]').getAttribute('role') === 'alert'",
        )
        .await;
        let (status, shown, text) = viewer(&page).await;
        assert!(status.contains("can't be opened"), "{case}: {status}");
        assert!(!shown && text.is_empty(), "{case} renders nothing");
    }

    // A revoked link shows a not-found page.
    site.store
        .revoke_share(owner, world.workspace, link.hash())
        .await
        .unwrap();
    visit(&page, &url).await;
    let gone: String = eval(&page, "document.body.innerText").await;
    assert!(gone.contains("This link doesn't open"), "{gone}");

    // The fragment never reached the server's log.
    let mut logged = String::new();
    for file in std::fs::read_dir(&logs).unwrap() {
        logged.push_str(&std::fs::read_to_string(file.unwrap().path()).unwrap());
    }
    assert!(logged.contains("/s/{token}"), "the log records the route");
    let other_fragment = &wrong[wrong.find('#').unwrap() + 1..];
    for secret in [
        fragment.as_str(),
        other_fragment,
        token.as_str(),
        &to_hex(link.key.expose()),
    ] {
        assert!(!logged.contains(secret), "the log holds {secret}");
    }

    browser.close().await.unwrap();
    let _ = browser.wait().await;
    events.abort();
}

/// The invitation page (§11) only sends a browser visitor to the desktop
/// app: whether the invitation is open, unknown or revoked, it loads no
/// script, so nothing on it can read the fragment that holds the secret.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Chrome, which NOURA_CHROME names, and the built module"]
async fn the_invitation_page_never_reads_the_fragment() {
    use chromiumoxide::cdp::browser_protocol::network::EventRequestWillBeSent;
    use sync_protocol::types::to_base64url;
    use sync_store::conformance::{Invite, World};

    let site = site().await;
    let world = World::on(site.store.clone(), 1).await;
    let owner = world.devices[0].session;
    let invite = Invite::new(sync_protocol::records::Role::Editor);
    site.store
        .create_invitation(owner, world.workspace, invite.creation(world.workspace))
        .await
        .unwrap();
    let token = to_base64url(&invite.token);
    let fragment = to_base64url(invite.secret.expose());
    let url = format!("{}/invite/{token}#{fragment}", site.base);
    let unknown = format!(
        "{}/invite/{}#{fragment}",
        site.base,
        to_base64url(&[7u8; 16])
    );

    let (mut browser, events, _profile) = launch().await;
    let page = browser.new_page("about:blank").await.unwrap();
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut sent = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .unwrap();
    let seen = requests.clone();
    let listener = tokio::spawn(async move {
        while let Some(event) = sent.next().await {
            seen.lock().unwrap().push(event.request.url.clone());
        }
    });

    // Checks the page `address` shows: its heading, no script element, no
    // script or module fetched, and the fragment nowhere but the address.
    async fn check(page: &Page, requests: &Mutex<Vec<String>>, address: &str, heading: &str) {
        requests.lock().unwrap().clear();
        visit(page, address).await;
        let text: String = eval(page, "document.body.innerText").await;
        assert!(text.contains(heading), "{address}: {text}");
        let scripts: u32 = eval(page, "document.scripts.length").await;
        assert_eq!(scripts, 0, "{address} holds a script");
        let fetched: Vec<String> = eval(
            page,
            "performance.getEntriesByType('resource').map(e => e.name)",
        )
        .await;
        let requested = requests.lock().unwrap().clone();
        for url in fetched.iter().chain(&requested) {
            assert!(
                !url.ends_with(".js") && !url.ends_with(".wasm"),
                "{address} loads {url}"
            );
        }
        assert!(
            requested.iter().any(|url| url.contains("/invite/")),
            "the listener saw the page's request: {requested:?}"
        );
        let fragment = &address[address.find('#').unwrap()..];
        let html: String = eval(page, "document.documentElement.outerHTML").await;
        assert!(
            !html.contains(&fragment[1..]),
            "{address} echoes the secret"
        );
        for url in &requested {
            assert!(!url.contains('#'), "a request carries the fragment: {url}");
        }
        let hash: String = eval(page, "location.hash").await;
        assert_eq!(hash, fragment, "the page leaves the address alone");
    }

    check(&page, &requests, &url, "Open this invitation in Noura").await;
    check(&page, &requests, &unknown, "This invitation doesn't open").await;
    site.store
        .revoke_invitation(owner, world.workspace, invite.invitation)
        .await
        .unwrap();
    check(&page, &requests, &url, "This invitation doesn't open").await;

    listener.abort();
    browser.close().await.unwrap();
    let _ = browser.wait().await;
    events.abort();
}
