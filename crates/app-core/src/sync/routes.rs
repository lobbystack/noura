//! The device and key routes of §26 on [`SyncClient`]: the device
//! authorization grant, enrollment, the enrolled cards, approval requests,
//! offers and reveals, recovery proofs, `keys/current`, re-wrap requests
//! and the workspace list.

use sync_protocol::codec::{Encode, Nested, Text};
use sync_protocol::messages::{
    ApprovalRequest, ChallengeResponse, DeviceAuthorizeResponse, DeviceTokenRequest,
    DeviceTokenResponse, DevicesResponse, EnrollRequest, EpochsResponse, KeysResponse,
    OfferRequest, OffersResponse, PendingResponse, RecoveryRequest, RevealBody, RewrapBody,
    WorkspacesResponse,
};
use sync_protocol::records::{Enroll, Offer, RecoveryProof, Signed};
use sync_protocol::types::{Digest, Id, Key};

use super::{SyncClient, SyncError};

impl SyncClient {
    /// A client without a session, for the device authorization grant.
    pub async fn anonymous(base: &str) -> Result<Self, SyncError> {
        Self::connect(base, "").await
    }

    /// The server's address, without a trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Uses `token` as the session from now on.
    pub fn set_token(&mut self, token: &str) {
        self.token = token.to_owned();
    }

    /// The server's public origin as an enrollment or recovery proof names
    /// it (§7): lowercase scheme and host, the port only when it isn't the
    /// default, and no trailing slash.
    pub fn origin(&self) -> Result<String, SyncError> {
        let url = reqwest::Url::parse(&self.base)
            .map_err(|_| SyncError::Invalid("the server address isn't a URL".into()))?;
        Ok(url.origin().ascii_serialization())
    }

    /// `POST /v2/device/authorize`.
    pub async fn device_authorize(&mut self) -> Result<DeviceAuthorizeResponse, SyncError> {
        self.call(
            reqwest::Method::POST,
            "/v2/device/authorize",
            Some(Vec::new()),
        )
        .await
    }

    /// `POST /v2/device/token`.
    pub async fn device_token(
        &mut self,
        device_code: &Text,
    ) -> Result<DeviceTokenResponse, SyncError> {
        let body = DeviceTokenRequest {
            device_code: device_code.clone(),
        }
        .to_bytes();
        self.call(reqwest::Method::POST, "/v2/device/token", Some(body))
            .await
    }

    /// `POST /v2/devices/challenge`.
    pub async fn challenge(&mut self) -> Result<ChallengeResponse, SyncError> {
        self.call(
            reqwest::Method::POST,
            "/v2/devices/challenge",
            Some(Vec::new()),
        )
        .await
    }

    /// `POST /v2/devices`.
    pub async fn enroll(&mut self, proof: Signed<Enroll>) -> Result<(), SyncError> {
        let body = EnrollRequest {
            enroll: Nested(proof),
        }
        .to_bytes();
        self.call_empty(reqwest::Method::POST, "/v2/devices", Some(body))
            .await
    }

    /// `GET /v2/devices`.
    pub async fn devices(&mut self) -> Result<DevicesResponse, SyncError> {
        self.call(reqwest::Method::GET, "/v2/devices", None).await
    }

    /// `DELETE /v2/devices/{device}`.
    pub async fn end_device(&mut self, device: Id) -> Result<(), SyncError> {
        let path = format!("/v2/devices/{}", device.to_base64url());
        self.call_empty(reqwest::Method::DELETE, &path, None).await
    }

    /// `POST /v2/approval-requests`.
    pub async fn post_approval_request(
        &mut self,
        request: &ApprovalRequest,
    ) -> Result<(), SyncError> {
        self.call_empty(
            reqwest::Method::POST,
            "/v2/approval-requests",
            Some(request.to_bytes()),
        )
        .await
    }

    /// `GET /v2/approval-requests`.
    pub async fn approval_requests(&mut self) -> Result<PendingResponse, SyncError> {
        self.call(reqwest::Method::GET, "/v2/approval-requests", None)
            .await
    }

    /// `POST /v2/approvals`.
    pub async fn post_offer(&mut self, offer: &Signed<Offer>) -> Result<(), SyncError> {
        let body = OfferRequest {
            offer: Nested(offer.clone()),
        }
        .to_bytes();
        self.call_empty(reqwest::Method::POST, "/v2/approvals", Some(body))
            .await
    }

    /// `GET /v2/approvals`.
    pub async fn offers(&mut self) -> Result<OffersResponse, SyncError> {
        self.call(reqwest::Method::GET, "/v2/approvals", None).await
    }

    /// `POST /v2/approvals/{offer_digest}/reveal`.
    pub async fn reveal(&mut self, offer: Digest, nc: Key) -> Result<(), SyncError> {
        let path = format!("/v2/approvals/{}/reveal", offer.to_base64url());
        self.call_empty(
            reqwest::Method::POST,
            &path,
            Some(RevealBody { nc }.to_bytes()),
        )
        .await
    }

    /// `GET /v2/approvals/{offer_digest}/reveal`: the candidate's nonce, or
    /// `None` while it revealed nothing.
    pub async fn read_reveal(&mut self, offer: Digest) -> Result<Option<Key>, SyncError> {
        let path = format!("/v2/approvals/{}/reveal", offer.to_base64url());
        match self
            .call::<RevealBody>(reqwest::Method::GET, &path, None)
            .await
        {
            Ok(body) => Ok(Some(body.nc)),
            Err(SyncError::Refused { code, .. }) if code == "not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `POST /v2/recovery`.
    pub async fn prove_recovery(&mut self, proof: Signed<RecoveryProof>) -> Result<(), SyncError> {
        let body = RecoveryRequest {
            recovery_proof: Nested(proof),
        }
        .to_bytes();
        self.call_empty(reqwest::Method::POST, "/v2/recovery", Some(body))
            .await
    }

    /// `GET /v2/workspaces`.
    pub async fn workspaces(&mut self) -> Result<WorkspacesResponse, SyncError> {
        self.call(reqwest::Method::GET, "/v2/workspaces", None)
            .await
    }

    /// `GET /v2/workspaces/{w}/keys/current?recipient={device}`.
    pub async fn current_keys(
        &mut self,
        workspace: Id,
        recipient: Id,
    ) -> Result<KeysResponse, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/keys/current?recipient={}",
            workspace.to_base64url(),
            recipient.to_base64url()
        );
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/epochs/{epoch}`: that epoch
    /// record and the ones its `previous` links reach, newest first, as many
    /// as fit one response.
    pub async fn epochs(
        &mut self,
        workspace: Id,
        domain: Id,
        epoch: u64,
    ) -> Result<EpochsResponse, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/domains/{}/epochs/{epoch}",
            workspace.to_base64url(),
            domain.to_base64url()
        );
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `POST /v2/workspaces/{w}/domains/{d}/rewrap`.
    pub async fn rewrap(&mut self, workspace: Id, domain: Id, epoch: u64) -> Result<(), SyncError> {
        let path = format!(
            "/v2/workspaces/{}/domains/{}/rewrap",
            workspace.to_base64url(),
            domain.to_base64url()
        );
        self.call_empty(
            reqwest::Method::POST,
            &path,
            Some(RewrapBody { epoch }.to_bytes()),
        )
        .await
    }
}
