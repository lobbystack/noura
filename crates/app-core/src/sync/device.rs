//! A desktop's sign-in and enrollment (docs/workspace-format/sync-v2.md
//! §7): the OAuth 2.0 device authorization grant through the system
//! browser, the device's keys and session token in one credential store
//! item that never syncs, enrollment with a fresh challenge, and the
//! genesis that an account's first device signs.
//!
//! The device keeps its secrets only in the credential store: its Ed25519
//! seed, its X-Wing seed and its session token. None of them reaches a
//! workspace file, the index, a log or a diagnostics report.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use local_core::sync::SyncCredentials;
use serde::{Deserialize, Serialize};
use sync_key_envelope::v2::{DeviceKeys, enroll, genesis_link};
use sync_protocol::account::AccountChain;
use sync_protocol::crypto::hash;
use sync_protocol::records::{AccountHead, Card};
use sync_protocol::types::{Id, SecretKey, from_hex, to_hex};
use zeroize::Zeroizing;

use super::{DeviceCredentials, SyncClient, SyncError};

/// How long the app waits between polls, and what each `slow_down` adds
/// (§7).
pub const POLL_SECONDS: u64 = 5;

/// Waits between polls. The app sleeps; tests move a clock.
pub trait Pause: Send + Sync {
    /// Waits `seconds`.
    fn pause(&self, seconds: u64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Waits on tokio's clock.
pub struct Sleep;

impl Pause for Sleep {
    fn pause(&self, seconds: u64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(Duration::from_secs(seconds)))
    }
}

/// Whether `address` is a server address the app takes (§7): HTTPS, or
/// plain HTTP only on a loopback host. Returns the address without a
/// trailing slash.
pub fn server_address(address: &str) -> Result<String, &'static str> {
    let address = address.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(address)
        .map_err(|_| "Enter the server's address, such as https://sync.noura.app.")?;
    let loopback = matches!(url.host(), Some(url::Host::Domain("localhost")))
        || match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => return Err("The server's address must start with https://."),
        _ => return Err("Enter the server's address, such as https://sync.noura.app."),
    }
    if url.host().is_none() || url.path() != "/" || url.query().is_some() {
        return Err("Enter only the server's address, such as https://sync.noura.app.");
    }
    Ok(address.to_owned())
}

/// The credential store item that holds a device's secrets for `server`.
pub fn device_item(server: &str) -> String {
    let digest = hash(server.as_bytes()).to_hex();
    format!("noura.sync.v2.device.{}", &digest[..32])
}

/// A signed-in device's secrets and state for one server, as the
/// credential store keeps them.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredDevice {
    /// The server's address.
    pub server: String,
    /// The account the device signed in to.
    pub account: Id,
    signing_seed: String,
    hpke_seed: String,
    session_token: String,
    /// The head from which the device verified its own account chain,
    /// once it joined it: the genesis it signed, the approver's head of
    /// the offer that approved it, or its kit's account head (§8).
    pub reference_head: Option<AccountHead>,
}

impl std::fmt::Debug for StoredDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredDevice")
            .field("server", &self.server)
            .field("account", &self.account)
            .field("reference_head", &self.reference_head)
            .finish_non_exhaustive()
    }
}

fn credential_error(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the credential store failed: {error}"))
}

impl StoredDevice {
    /// A device of `account` with `keys` and the session `token`.
    pub fn new(server: &str, account: Id, keys: &DeviceKeys, token: &str) -> Self {
        Self {
            server: server.to_owned(),
            account,
            signing_seed: to_hex(keys.signing().seed().expose()),
            hpke_seed: to_hex(keys.hpke_seed().expose()),
            session_token: token.to_owned(),
            reference_head: None,
        }
    }

    /// The device for `server` that `store` holds, if any.
    pub fn load(store: &dyn SyncCredentials, server: &str) -> Result<Option<Self>, SyncError> {
        let Some(text) = store
            .read_optional(&device_item(server))
            .map_err(credential_error)?
        else {
            return Ok(None);
        };
        if text.is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| SyncError::Invalid("the stored device doesn't parse".into()))
    }

    /// Keeps the device in `store`.
    pub fn save(&self, store: &dyn SyncCredentials) -> Result<(), SyncError> {
        let text = Zeroizing::new(serde_json::to_string(self).map_err(credential_error)?);
        store
            .write(&device_item(&self.server), &text)
            .map_err(credential_error)
    }

    /// Deletes the device for `server` from `store`.
    pub fn delete(store: &dyn SyncCredentials, server: &str) -> Result<(), SyncError> {
        store.delete(&device_item(server)).map_err(credential_error)
    }

    /// The device's keys.
    pub fn keys(&self) -> Result<DeviceKeys, SyncError> {
        let seed = |text: &str| {
            from_hex(text)
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .map(SecretKey)
                .ok_or_else(|| SyncError::Invalid("a stored seed doesn't parse".into()))
        };
        let signing = Zeroizing::new(seed(&self.signing_seed)?);
        Ok(DeviceKeys::from_seeds(&signing, seed(&self.hpke_seed)?))
    }

    /// The device's card.
    pub fn card(&self) -> Result<Card, SyncError> {
        Ok(self.keys()?.card(self.account))
    }

    /// The session token's 43 characters.
    pub fn session_token(&self) -> &str {
        &self.session_token
    }

    /// What the sync service needs, once the device joined its chain.
    pub fn credentials(&self) -> Result<Option<DeviceCredentials>, SyncError> {
        let Some(reference_head) = self.reference_head.clone() else {
            return Ok(None);
        };
        Ok(Some(DeviceCredentials {
            account: self.account,
            keys: self.keys()?,
            session_token: self.session_token.clone(),
            reference_head,
        }))
    }
}

/// A device authorization grant in progress: the codes the server issued,
/// and the browser page that shows them.
pub struct DeviceSignIn {
    client: SyncClient,
    device_code: sync_protocol::codec::Text,
    user_code: String,
    verification_uri: String,
    interval: u64,
}

/// What a sign-in returns: the session token and the account it belongs
/// to.
pub struct SignedIn {
    /// The server's address.
    pub server: String,
    /// The account.
    pub account: Id,
    /// The session token's 43 characters.
    pub session_token: String,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn")
            .field("server", &self.server)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl DeviceSignIn {
    /// Asks `server` for a device grant and opens the system browser on
    /// its device page with `open_browser`.
    pub async fn start(
        server: &str,
        open_browser: &dyn Fn(&str) -> std::io::Result<()>,
    ) -> Result<Self, SyncError> {
        let mut client = SyncClient::anonymous(server).await?;
        let grant = client.device_authorize().await?;
        let verification_uri = grant.verification_uri.as_str().to_owned();
        // The browser opens only a web page.
        if !crate::settings::is_web_link(&verification_uri) {
            return Err(SyncError::Invalid(
                "the server named a device page that isn't a web address".into(),
            ));
        }
        open_browser(&verification_uri)
            .map_err(|e| SyncError::Invalid(format!("the browser didn't open: {e}")))?;
        Ok(Self {
            client,
            device_code: grant.device_code,
            user_code: grant.user_code.as_str().to_owned(),
            verification_uri,
            interval: u64::from(grant.interval).max(POLL_SECONDS),
        })
    }

    /// The code the app shows, as two groups of 4.
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    /// The device page the browser opened.
    pub fn verification_uri(&self) -> &str {
        &self.verification_uri
    }

    /// Polls until you approve or refuse the code: every 5 seconds, 5
    /// more after each `slow_down`. A refused code reports
    /// [`SyncError::SignIn`] with [`SignInEnd::Refused`], an expired or
    /// used one [`SignInEnd::Expired`].
    pub async fn finish(mut self, pause: &dyn Pause) -> Result<SignedIn, SyncError> {
        loop {
            pause.pause(self.interval).await;
            match self.client.device_token(&self.device_code).await {
                Ok(token) => {
                    return Ok(SignedIn {
                        server: self.client.base().to_owned(),
                        account: token.account,
                        session_token: token.session_token.0,
                    });
                }
                Err(SyncError::Refused { code, .. }) => match code.as_str() {
                    "authorization_pending" => {}
                    "slow_down" => self.interval += POLL_SECONDS,
                    "access_denied" => return Err(SyncError::SignIn(SignInEnd::Refused)),
                    "expired_token" => return Err(SyncError::SignIn(SignInEnd::Expired)),
                    _ => {
                        return Err(SyncError::Refused {
                            code,
                            status: 400,
                            detail: None,
                        });
                    }
                },
                Err(error) => return Err(error),
            }
        }
    }
}

/// How a sign-in ended without a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInEnd {
    /// You refused the code on the device page.
    Refused,
    /// The code expired or was already used.
    Expired,
}

impl std::fmt::Display for SignInEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SignInEnd::Refused => {
                "You refused the sign-in in your browser. Sign in again to retry."
            }
            SignInEnd::Expired => "The sign-in code expired. Sign in again for a new code.",
        })
    }
}

/// Where a device stands once it enrolled.
pub enum Joined {
    /// The account had no chain: this device signed its genesis and is the
    /// account's first device.
    First(DeviceCredentials),
    /// The account has a chain: this device waits for approval from a
    /// device you already hold (§10).
    NeedsApproval {
        /// The device's keys.
        keys: DeviceKeys,
        /// The session client.
        client: SyncClient,
    },
}

/// Enrolls a signed-in device with fresh keys, keeps its secrets in
/// `store`, and, for an account without a chain, signs the genesis (§7).
pub async fn join(signed_in: &SignedIn, store: &dyn SyncCredentials) -> Result<Joined, SyncError> {
    let keys =
        DeviceKeys::generate().map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))?;
    let mut stored = StoredDevice::new(
        &signed_in.server,
        signed_in.account,
        &keys,
        &signed_in.session_token,
    );
    stored.save(store)?;
    let mut client = SyncClient::connect(&signed_in.server, &signed_in.session_token).await?;
    enroll_card(&mut client, &keys, signed_in.account).await?;
    match client.whole_chain(signed_in.account, None).await {
        Err(SyncError::Refused { code, .. }) if code == "not_found" => {
            let genesis = genesis_link(&keys, signed_in.account);
            client.append_chain(vec![genesis.clone()]).await?;
            let chain = AccountChain::verify(std::slice::from_ref(&genesis), &Default::default())
                .map_err(|e| SyncError::Check(e.client_code()))?;
            stored.reference_head = Some(chain.head());
            stored.save(store)?;
            Ok(Joined::First(
                stored.credentials()?.expect("the reference head is set"),
            ))
        }
        Ok(_) => Ok(Joined::NeedsApproval { keys, client }),
        Err(error) => Err(error),
    }
}

/// Enrolls `keys` as a device of `account` with a fresh challenge (§7).
pub async fn enroll_card(
    client: &mut SyncClient,
    keys: &DeviceKeys,
    account: Id,
) -> Result<(), SyncError> {
    let challenge = client.challenge().await?.challenge;
    let origin = client.origin()?;
    client
        .enroll(enroll(keys, account, &origin, challenge))
        .await
}
