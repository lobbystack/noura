//! Devices at seam 4 (§7, §10, §26): a desktop signs in through the
//! device page with the OAuth device authorization grant, enrolls its
//! card, and joins its account's chain; the routes for the enrolled cards,
//! their sessions, `device_removed` and approval requests, offers and
//! reveals answer as §26 sets.

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use reqwest::StatusCode;
use support::{Client, Reply};
use sync_key_envelope::v2::{DeviceKeys, enroll, genesis_link, next_link};
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted, Text};
use sync_protocol::messages::{
    ApprovalRequest, ChainAppendRequest, ChainCursor, ChainsRequest, ChainsResponse,
    ChallengeResponse, DeviceAuthorizeResponse, DeviceRemovedDetail, DeviceState,
    DeviceTokenRequest, DeviceTokenResponse, DevicesResponse, EnrollRequest, OfferRequest,
    OffersResponse, PendingResponse, RevealBody,
};
use sync_protocol::records::{AccountLink, LinkKind, Offer, Signed, approval_commit};
use sync_protocol::types::{Digest, Id, Key, to_base64url};
use sync_store::Store;
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

struct Site {
    base: String,
    store: Arc<Store<SqliteBackend>>,
    data: PathBuf,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

async fn site() -> Site {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = Arc::new(
        sync_server::open_store(&sync_server::Config::new(
            data.clone(),
            "127.0.0.1:0".parse().unwrap(),
        ))
        .await
        .expect("the store opens"),
    );
    let address = sync_server::spawn(store.clone(), "127.0.0.1:0")
        .await
        .expect("serving");
    Site {
        base: format!("http://localhost:{}", address.port()),
        store,
        data,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        _dir: dir,
    }
}

impl Site {
    fn client(&self, token: Option<String>) -> Client {
        Client::new(self.base.clone(), token)
    }

    /// A web session of `email`'s account, as its cookie.
    async fn signed_in(&self, email: &str) -> (Id, String) {
        let link = self.store.issue_magic_link(email).await.expect("a link");
        let (account, token) = self
            .store
            .sign_in_with_magic_link(&link)
            .await
            .expect("used")
            .expect("a current link");
        (account, format!("noura_session={}", to_base64url(&token)))
    }

    async fn page(&self, path: &str, cookie: Option<&str>) -> (StatusCode, String) {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = request.send().await.expect("an answer");
        (response.status(), response.text().await.expect("a body"))
    }

    async fn form(
        &self,
        path: &str,
        fields: &[(&str, &str)],
        cookie: &str,
    ) -> (StatusCode, String) {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header("origin", &self.base)
            .header("cookie", cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .expect("an answer");
        (response.status(), response.text().await.expect("a body"))
    }

    /// Sends the device page's decision form that `confirm`, the page
    /// shown after a code entry, holds, with `decision`.
    async fn decide(&self, confirm: &str, decision: &str, cookie: &str) -> (StatusCode, String) {
        let user_code = hidden(confirm, "user_code");
        let expires = hidden(confirm, "expires");
        let confirmation = hidden(confirm, "confirmation");
        self.form(
            "/device/decide",
            &[
                ("user_code", &user_code),
                ("expires", &expires),
                ("confirmation", &confirmation),
                ("decision", decision),
            ],
            cookie,
        )
        .await
    }

    /// Signs a desktop in through the device page as `cookie`'s account and
    /// returns its session token.
    async fn device_sign_in(&self, cookie: &str) -> DeviceTokenResponse {
        let anonymous = self.client(None);
        let grant: DeviceAuthorizeResponse = anonymous
            .post("/v2/device/authorize", Vec::new())
            .await
            .decode();
        let (_, confirm) = self
            .form(
                "/device",
                &[("user_code", grant.user_code.as_str())],
                cookie,
            )
            .await;
        assert!(confirm.contains(grant.user_code.as_str()), "{confirm}");
        let (status, done) = self.decide(&confirm, "approve", cookie).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        poll(&anonymous, &grant.device_code).await.decode()
    }
}

async fn poll(client: &Client, device_code: &Text) -> Reply {
    client
        .post(
            "/v2/device/token",
            DeviceTokenRequest {
                device_code: device_code.clone(),
            }
            .to_bytes(),
        )
        .await
}

fn error_code(reply: &Reply) -> (StatusCode, String) {
    (reply.status, reply.error().code.0)
}

/// Enrolls `keys` as a device of `account` through the session `token`.
async fn enroll_device(site: &Site, token: &str, keys: &DeviceKeys, account: Id) -> Reply {
    let client = site.client(Some(token.to_owned()));
    let challenge: ChallengeResponse = client
        .post("/v2/devices/challenge", Vec::new())
        .await
        .decode();
    client
        .post(
            "/v2/devices",
            EnrollRequest {
                enroll: Nested(enroll(keys, account, &site.base, challenge.challenge)),
            }
            .to_bytes(),
        )
        .await
}

async fn append(site: &Site, token: &str, links: Vec<Signed<AccountLink>>) -> Reply {
    site.client(Some(token.to_owned()))
        .post(
            "/v2/accounts/chain",
            ChainAppendRequest {
                links: List(links.into_iter().map(Nested).collect()),
            }
            .to_bytes(),
        )
        .await
}

fn head(links: &[Signed<AccountLink>]) -> sync_protocol::records::AccountHead {
    sync_store::conformance::head_of(links)
}

/// A desktop signs in through the system browser (§7): the device page
/// shows the 8-character user code to the signed-in person, who approves
/// it, and the app's poll receives a 32-byte session token and its
/// account. The database holds the token only as its hash, and the device
/// code works once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_desktop_signs_in_through_the_device_page() {
    let site = site().await;
    let anonymous = site.client(None);
    let grant: DeviceAuthorizeResponse = anonymous
        .post("/v2/device/authorize", Vec::new())
        .await
        .decode();
    assert_eq!(
        grant.verification_uri.as_str(),
        format!("{}/device", site.base)
    );
    assert_eq!((grant.interval, grant.expires_in), (5, 600));
    let user_code = grant.user_code.as_str().to_owned();
    assert!(
        user_code.len() == 9
            && user_code.chars().enumerate().all(|(i, c)| if i == 4 {
                c == '-'
            } else {
                "BCDFGHJKLMNPQRSTVWXZ".contains(c)
            }),
        "{user_code}"
    );

    let (_, page) = site.page("/device", None).await;
    assert!(page.contains("Sign in to your account first"), "{page}");
    let (email_account, cookie) = site.signed_in("ada@example.com").await;
    let (_, page) = site.page("/device", Some(&cookie)).await;
    assert!(page.contains("name=\"user_code\""), "{page}");

    assert_eq!(
        error_code(&poll(&anonymous, &grant.device_code).await),
        (StatusCode::BAD_REQUEST, "authorization_pending".into())
    );
    let (status, wrong) = site
        .form("/device", &[("user_code", "BBBB-BBBB")], &cookie)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(wrong.contains("That code"), "{wrong}");
    // The page takes the code in any case, with or without its hyphen.
    let typed = user_code.to_lowercase().replace('-', "");
    let (_, confirm) = site
        .form("/device", &[("user_code", &typed)], &cookie)
        .await;
    assert!(confirm.contains(&user_code), "{confirm}");
    assert!(confirm.contains("ada@example.com"), "{confirm}");
    let (_, done) = site.decide(&confirm, "approve", &cookie).await;
    assert!(done.contains("Device signed in"), "{done}");

    let signed_in: DeviceTokenResponse = poll(&anonymous, &grant.device_code).await.decode();
    assert_eq!(signed_in.account, email_account);
    let token = signed_in.session_token.as_str().to_owned();
    assert_eq!(token.len(), 43);
    assert_eq!(
        error_code(&poll(&anonymous, &grant.device_code).await),
        (StatusCode::BAD_REQUEST, "expired_token".into()),
        "a device code works once"
    );
    // Until its device enrolls, the session takes only the challenge and
    // the enrollment, so a code nobody enrolled reads nothing.
    let unbound = site.client(Some(token.clone()));
    for path in ["/v2/workspaces", "/v2/devices", "/v2/approval-requests"] {
        assert_eq!(
            error_code(&unbound.get(path).await),
            (StatusCode::FORBIDDEN, "forbidden".into()),
            "{path} before enrollment"
        );
    }
    let chains = unbound
        .post(
            "/v2/accounts/chains",
            ChainsRequest {
                cursors: List(vec![ChainCursor {
                    account: email_account,
                    genesis: None,
                    after: 0,
                }]),
            }
            .to_bytes(),
        )
        .await;
    assert_eq!(
        error_code(&chains),
        (StatusCode::FORBIDDEN, "forbidden".into())
    );
    let challenge = unbound.post("/v2/devices/challenge", Vec::new()).await;
    assert_eq!(
        challenge.status,
        StatusCode::OK,
        "the challenge comes first"
    );

    let raw: [u8; 32] = sync_protocol::types::from_base64url(&token).unwrap();
    let database = std::fs::read(site.data.join(DATABASE_FILE)).unwrap();
    let wal = std::fs::read(site.data.join(format!("{DATABASE_FILE}-wal"))).unwrap_or_default();
    for bytes in [&database, &wal] {
        assert!(!bytes.windows(32).any(|w| w == raw), "the token is stored");
        assert!(
            !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "the token's text is stored"
        );
    }
}

/// Polling answers RFC 8628's errors with status 400: `slow_down` to a
/// poll that comes too soon, `access_denied` once you refuse the code on
/// the device page, and `expired_token` for a code that expired or was used.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polling_answers_the_rfc_8628_errors() {
    let site = site().await;
    let anonymous = site.client(None);
    let (_, cookie) = site.signed_in("bea@example.com").await;
    let grant: DeviceAuthorizeResponse = anonymous
        .post("/v2/device/authorize", Vec::new())
        .await
        .decode();
    assert_eq!(
        error_code(&poll(&anonymous, &grant.device_code).await).1,
        "authorization_pending"
    );
    assert_eq!(
        error_code(&poll(&anonymous, &grant.device_code).await),
        (StatusCode::BAD_REQUEST, "slow_down".into())
    );
    let (_, confirm) = site
        .form(
            "/device",
            &[("user_code", grant.user_code.as_str())],
            &cookie,
        )
        .await;
    let (_, refused) = site.decide(&confirm, "deny", &cookie).await;
    assert!(refused.contains("Sign-in refused"), "{refused}");
    assert_eq!(
        error_code(&poll(&anonymous, &grant.device_code).await),
        (StatusCode::BAD_REQUEST, "access_denied".into())
    );

    let late: DeviceAuthorizeResponse = anonymous
        .post("/v2/device/authorize", Vec::new())
        .await
        .decode();
    site.store
        .clock()
        .advance(std::time::Duration::from_secs(601));
    let (status, _) = site
        .form(
            "/device",
            &[("user_code", late.user_code.as_str())],
            &cookie,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an expired code shows nothing"
    );
    assert_eq!(
        error_code(&poll(&anonymous, &late.device_code).await),
        (StatusCode::BAD_REQUEST, "expired_token".into())
    );
}

/// Enrollment and removal over HTTP (§7, §26): the first device enrolls
/// and signs its account's genesis, a second enrolls and lists as a card
/// no chain lists until the first adds it, and the link that removes it
/// makes its session answer `device_removed` everywhere but the chain read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_enroll_join_the_chain_and_answer_device_removed() {
    let site = site().await;
    let (account, cookie) = site.signed_in("cai@example.com").await;
    let laptop = DeviceKeys::generate().unwrap();
    let laptop_token = site.device_sign_in(&cookie).await.session_token.0;
    let enrolled = enroll_device(&site, &laptop_token, &laptop, account).await;
    assert_eq!(enrolled.status, StatusCode::OK, "{:?}", enrolled.error());
    let genesis = genesis_link(&laptop, account);
    assert_eq!(
        append(&site, &laptop_token, vec![genesis.clone()])
            .await
            .status,
        StatusCode::OK
    );

    let phone = DeviceKeys::generate().unwrap();
    let phone_token = site.device_sign_in(&cookie).await.session_token.0;
    assert_eq!(
        enroll_device(&site, &phone_token, &phone, account)
            .await
            .status,
        StatusCode::OK
    );
    let listed: DevicesResponse = site
        .client(Some(laptop_token.clone()))
        .get("/v2/devices")
        .await
        .decode();
    let state_of = |listed: &DevicesResponse, keys: &DeviceKeys| {
        listed
            .devices
            .iter()
            .find(|d| d.card.0.device == keys.device())
            .map(|d| (d.state, d.sessions))
    };
    assert_eq!(state_of(&listed, &laptop), Some((DeviceState::Active, 1)));
    assert_eq!(state_of(&listed, &phone), Some((DeviceState::Enrolled, 1)));

    let add = next_link(
        &laptop,
        &head(std::slice::from_ref(&genesis)),
        LinkKind::AddDevice {
            card: Nested(phone.card(account)),
            offer: Digest::ZERO,
        },
    );
    assert_eq!(
        append(&site, &laptop_token, vec![add.clone()]).await.status,
        StatusCode::OK
    );
    let remove = next_link(
        &laptop,
        &head(&[genesis.clone(), add]),
        LinkKind::Remove {
            device: phone.device(),
            keep: Sorted::default(),
        },
    );
    assert_eq!(
        append(&site, &laptop_token, vec![remove]).await.status,
        StatusCode::OK
    );
    let phone_client = site.client(Some(phone_token.clone()));
    let refused = phone_client.get("/v2/workspaces").await;
    assert_eq!(
        error_code(&refused),
        (StatusCode::UNAUTHORIZED, "device_removed".into())
    );
    let detail =
        DeviceRemovedDetail::from_bytes(&refused.error().detail.expect("a detail").0).unwrap();
    assert_eq!((detail.genesis, detail.seqno), (genesis.digest(), 3));
    let chains: ChainsResponse = phone_client
        .post(
            "/v2/accounts/chains",
            ChainsRequest {
                cursors: List(vec![ChainCursor {
                    account,
                    genesis: None,
                    after: 0,
                }]),
            }
            .to_bytes(),
        )
        .await
        .decode();
    assert_eq!(
        chains.pages[0].links.len(),
        3,
        "the removed device reads its chain"
    );
    let listed: DevicesResponse = site
        .client(Some(laptop_token))
        .get("/v2/devices")
        .await
        .decode();
    assert_eq!(
        state_of(&listed, &phone),
        None,
        "a removed card leaves the list"
    );
}

/// `DELETE /v2/devices/{device}` ends the sessions of an enrolled card that
/// no chain lists, from an active device only (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_active_device_ends_a_pending_cards_sessions() {
    let site = site().await;
    let (account, cookie) = site.signed_in("dan@example.com").await;
    let laptop = DeviceKeys::generate().unwrap();
    let laptop_token = site.device_sign_in(&cookie).await.session_token.0;
    enroll_device(&site, &laptop_token, &laptop, account).await;
    append(&site, &laptop_token, vec![genesis_link(&laptop, account)]).await;
    let stranger = DeviceKeys::generate().unwrap();
    let stranger_token = site.device_sign_in(&cookie).await.session_token.0;
    enroll_device(&site, &stranger_token, &stranger, account).await;
    let path = format!("/v2/devices/{}", stranger.device().to_base64url());
    let stranger_client = site.client(Some(stranger_token));
    let refused = stranger_client.delete(&path).await;
    assert_eq!(error_code(&refused).1, "forbidden");
    let ended = site.client(Some(laptop_token)).delete(&path).await;
    assert_eq!(ended.status, StatusCode::OK);
    assert_eq!(
        error_code(&stranger_client.get("/v2/workspaces").await),
        (StatusCode::UNAUTHORIZED, "unauthorized".into())
    );
}

/// The approval routes (§10, §26): the candidate posts its request, the
/// approver lists it and posts an offer, the candidate reads the offer and
/// reveals its nonce, and the approver reads the nonce back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_approval_routes_carry_one_attempt() {
    let site = site().await;
    let (account, cookie) = site.signed_in("eve@example.com").await;
    let laptop = DeviceKeys::generate().unwrap();
    let laptop_token = site.device_sign_in(&cookie).await.session_token.0;
    enroll_device(&site, &laptop_token, &laptop, account).await;
    let genesis = genesis_link(&laptop, account);
    append(&site, &laptop_token, vec![genesis.clone()]).await;
    let phone = DeviceKeys::generate().unwrap();
    let phone_token = site.device_sign_in(&cookie).await.session_token.0;
    enroll_device(&site, &phone_token, &phone, account).await;

    let nc = Key([0x42; 32]);
    let card = phone.card(account);
    let commit = approval_commit(&card.fingerprint(), None, &nc);
    let phone_client = site.client(Some(phone_token));
    let laptop_client = site.client(Some(laptop_token));
    let posted = phone_client
        .post(
            "/v2/approval-requests",
            ApprovalRequest {
                commit,
                account: None,
                invitation: None,
                invite_proof: None,
                workspace: None,
            }
            .to_bytes(),
        )
        .await;
    assert_eq!(posted.status, StatusCode::OK, "{:?}", posted.error());
    let pending: PendingResponse = laptop_client.get("/v2/approval-requests").await.decode();
    assert_eq!(pending.pending.len(), 1);
    assert_eq!(pending.pending[0].card.0, card);

    let offer = Signed::sign(
        Offer {
            approver_card: Nested(laptop.card(account)),
            candidate_card_digest: card.fingerprint(),
            candidate_commit: commit,
            candidate_head: None,
            approver_head: head(std::slice::from_ref(&genesis)),
            nonce: Key([7; 32]),
            anchors: Sorted::default(),
        },
        laptop.signing(),
    );
    let sent = laptop_client
        .post(
            "/v2/approvals",
            OfferRequest {
                offer: Nested(offer.clone()),
            }
            .to_bytes(),
        )
        .await;
    assert_eq!(sent.status, StatusCode::OK, "{:?}", sent.error());
    let offers: OffersResponse = phone_client.get("/v2/approvals").await.decode();
    assert_eq!(offers.offers.len(), 1);
    assert_eq!(offers.offers[0].0, offer);
    let path = format!("/v2/approvals/{}/reveal", offer.digest().to_base64url());
    assert_eq!(error_code(&laptop_client.get(&path).await).1, "not_found");
    let revealed = phone_client.post(&path, RevealBody { nc }.to_bytes()).await;
    assert_eq!(revealed.status, StatusCode::OK, "{:?}", revealed.error());
    let read: RevealBody = laptop_client.get(&path).await.decode();
    assert_eq!(read.nc, nc);
}

/// The value of the hidden form field `name` on `page`.
fn hidden(page: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = page
        .find(&marker)
        .unwrap_or_else(|| panic!("no {name} on {page}"))
        + marker.len();
    let end = start + page[start..].find('"').expect("a closing quote");
    page[start..end].to_owned()
}

/// A decision on the device page needs the confirmation that the page
/// showed after you entered that code as that account, before it expired
/// (RFC 8628 §5.1): sending the decision form straight away, with
/// guessed codes, another account's confirmation or a changed expiry,
/// signs nothing in, however often it's tried.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_decision_needs_the_confirmation_of_a_code_entry() {
    let site = site().await;
    let anonymous = site.client(None);
    let (_, cookie) = site.signed_in("bea@example.com").await;
    let grant: DeviceAuthorizeResponse = anonymous
        .post("/v2/device/authorize", Vec::new())
        .await
        .decode();
    let code = grant.user_code.as_str();
    for _ in 0..12 {
        let (status, page) = site
            .form(
                "/device/decide",
                &[("user_code", code), ("decision", "approve")],
                &cookie,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(page.contains("That code"), "{page}");
    }
    let (_, confirm) = site.form("/device", &[("user_code", code)], &cookie).await;
    let expires = hidden(&confirm, "expires");
    let confirmation = hidden(&confirm, "confirmation");
    // Another account can't use bea's confirmation.
    let (_, other) = site.signed_in("cai@example.com").await;
    let (status, _) = site.decide(&confirm, "approve", &other).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A later expiry or another code breaks it.
    let later = (expires.parse::<u64>().unwrap() + 600).to_string();
    for fields in [
        [code, later.as_str(), confirmation.as_str()],
        ["BBBB-BBBB", expires.as_str(), confirmation.as_str()],
    ] {
        let (status, _) = site
            .form(
                "/device/decide",
                &[
                    ("user_code", fields[0]),
                    ("expires", fields[1]),
                    ("confirmation", fields[2]),
                    ("decision", "approve"),
                ],
                &cookie,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        error_code(&poll(&anonymous, &grant.device_code).await).1,
        "authorization_pending",
        "nothing decided the grant"
    );
    // Past its 10 minutes the confirmation stops working.
    site.store
        .clock()
        .advance(std::time::Duration::from_secs(601));
    let (status, _) = site.decide(&confirm, "approve", &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The device page takes 10 codes per signed-in account in 10 minutes, so
/// nobody guesses another computer's code (RFC 8628 §5.1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_device_page_limits_code_entries() {
    let site = site().await;
    let (_, cookie) = site.signed_in("bea@example.com").await;
    for _ in 0..10 {
        let (status, _) = site
            .form("/device", &[("user_code", "BBBB-BBBB")], &cookie)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, page) = site
        .form("/device", &[("user_code", "BBBB-BBBB")], &cookie)
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(page.contains("too many codes"), "{page}");
    // Another account still enters its codes.
    let (_, other) = site.signed_in("cai@example.com").await;
    let (status, _) = site
        .form("/device", &[("user_code", "BBBB-BBBB")], &other)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
