use std::time::{Duration, Instant};

use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{DeviceKeys, HttpSyncTransport, SyncCredentials, invalid, transport::validate_origin};
use crate::{CoreError, ErrorCategory, Result};

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSignInInfo {
    pub verification_uri: String,
    pub user_code: String,
    #[ts(type = "number")]
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceConnection {
    pub origin: String,
    pub device_id: String,
    pub account_id: String,
    pub token_reference: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum DeviceSignInStatus {
    Pending { retry_after: u64 },
    Connected { connection: DeviceConnection },
}

/// A native RFC 8628 sign-in session. Device codes and bearer tokens never enter IPC DTOs.
pub struct DeviceSignIn {
    origin: String,
    client: Client,
    keys: DeviceKeys,
    device_code: Zeroizing<String>,
    info: DeviceSignInInfo,
    deadline: Instant,
    next_poll: Instant,
    interval: Duration,
    connection: Option<DeviceConnection>,
}

impl DeviceSignIn {
    pub async fn begin(origin: &str, store: &impl SyncCredentials) -> Result<Self> {
        validate_origin(origin)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| unavailable())?;
        let keys = DeviceKeys::create(store)?;
        let response = client
            .post(format!("{origin}/api/auth/device/code"))
            .header("Origin", origin)
            .json(&serde_json::json!({"client_id":"noura-desktop"}))
            .send()
            .await
            .map_err(|_| unavailable())?;
        let response = response.error_for_status().map_err(|_| unavailable())?;
        let value = read_json(response).await?;
        let code = string(&value, "device_code")?;
        let user_code = string(&value, "user_code")?;
        let verification_uri = string(&value, "verification_uri_complete")?;
        let url = url::Url::parse(verification_uri).map_err(|_| unavailable())?;
        let expires_in = value["expires_in"]
            .as_u64()
            .filter(|value| *value > 0 && *value <= 900)
            .ok_or_else(unavailable)?;
        let interval = value["interval"]
            .as_u64()
            .filter(|value| *value > 0 && *value <= 60)
            .ok_or_else(unavailable)?;
        if url.origin().ascii_serialization() != origin
            || url.path() != "/account/device"
            || code.len() > 256
            || user_code.len() > 32
        {
            return Err(invalid("sync_invalid_signin_response"));
        }
        Ok(Self {
            origin: origin.into(),
            client,
            keys,
            device_code: Zeroizing::new(code.into()),
            info: DeviceSignInInfo {
                verification_uri: verification_uri.into(),
                user_code: user_code.into(),
                expires_in,
            },
            deadline: Instant::now() + Duration::from_secs(expires_in),
            next_poll: Instant::now(),
            interval: Duration::from_secs(interval),
            connection: None,
        })
    }

    pub fn info(&self) -> DeviceSignInInfo {
        self.info.clone()
    }

    pub async fn poll(&mut self, store: &impl SyncCredentials) -> Result<DeviceSignInStatus> {
        if let Some(connection) = &self.connection {
            return Ok(DeviceSignInStatus::Connected {
                connection: connection.clone(),
            });
        }
        if Instant::now() >= self.deadline {
            return Err(invalid("sync_signin_expired"));
        }
        if Instant::now() < self.next_poll {
            return Ok(DeviceSignInStatus::Pending {
                retry_after: self
                    .next_poll
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    .max(1),
            });
        }
        self.next_poll = Instant::now() + self.interval;
        let response = self.client.post(format!("{}/api/auth/device/token",self.origin))
            .header("Origin",&self.origin)
            .json(&serde_json::json!({"grant_type":"urn:ietf:params:oauth:grant-type:device_code","client_id":"noura-desktop","device_code":self.device_code.as_str()}))
            .send().await.map_err(|_| unavailable())?;
        let success = response.status().is_success();
        let value = read_json(response).await?;
        if !success {
            match value["error"].as_str() {
                Some("authorization_pending") => {
                    return Ok(DeviceSignInStatus::Pending {
                        retry_after: self.interval.as_secs(),
                    });
                }
                Some("slow_down") => {
                    self.interval += Duration::from_secs(5);
                    self.next_poll = Instant::now() + self.interval;
                    return Ok(DeviceSignInStatus::Pending {
                        retry_after: self.interval.as_secs(),
                    });
                }
                Some("access_denied") => return Err(invalid("sync_signin_denied")),
                Some("expired_token") => return Err(invalid("sync_signin_expired")),
                _ => return Err(unavailable()),
            }
        }
        let account_token = Zeroizing::new(string(&value, "access_token")?.to_owned());
        let challenge = self
            .account_request(Method::POST, "/v1/device-challenges", &account_token, None)
            .await?;
        let account = string(&challenge, "accountId")?;
        let challenge = string(&challenge, "challenge")?;
        let recipient = self.keys.recipient();
        let public_key = self.keys.signer().public_key();
        let proof = self.keys.signer().sign_bytes(
            &serde_json::to_vec(&(
                "noura.device.enroll.v2",
                &self.origin,
                account,
                self.keys.device_id(),
                &public_key,
                &recipient,
                challenge,
            ))
            .map_err(|_| unavailable())?,
        );
        let enrolled = self.account_request(Method::POST,"/v1/devices",&account_token,Some(&serde_json::json!({
            "deviceId":self.keys.device_id(),"publicKey":public_key,"encryptionRecipient":recipient,"challenge":challenge,"proof":proof,
        }))).await?;
        let token = Zeroizing::new(string(&enrolled, "token")?.to_owned());
        // Validate the returned token before persisting it behind the credential boundary.
        HttpSyncTransport::new(&self.origin, token.to_string())?;
        let reference = format!(
            "session_{}_{}",
            self.keys.device_id(),
            blake3::hash(self.origin.as_bytes()).to_hex()
        );
        store.write(&reference, &token)?;
        let connection = DeviceConnection {
            origin: self.origin.clone(),
            device_id: self.keys.device_id().into(),
            account_id: account.into(),
            token_reference: reference,
        };
        self.connection = Some(connection.clone());
        // This temporary account session belongs to the device flow, not the browser's session.
        let _ = self
            .account_request(
                Method::POST,
                "/api/auth/sign-out",
                &account_token,
                Some(&serde_json::json!({})),
            )
            .await;
        Ok(DeviceSignInStatus::Connected { connection })
    }

    async fn account_request(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.origin))
            .header("Origin", &self.origin)
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| unavailable())?
            .error_for_status()
            .map_err(|_| unavailable())?;
        read_json(response).await
    }
}

impl DeviceConnection {
    pub fn transport(&self, store: &impl SyncCredentials) -> Result<HttpSyncTransport> {
        HttpSyncTransport::new(&self.origin, store.read(&self.token_reference)?.to_string())
    }
}

async fn read_json(mut response: reqwest::Response) -> Result<serde_json::Value> {
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > 64 * 1024 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}

fn string<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(unavailable)
}

fn unavailable() -> CoreError {
    CoreError::new(
        "sync_signin_unavailable",
        ErrorCategory::Credential,
        "Sign-in could not complete. Start sign-in again or check the server connection.",
        "sync_signin",
    )
}
