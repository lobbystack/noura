//! The device and key routes of §26 on [`SyncClient`]: the device
//! authorization grant, enrollment, the enrolled cards, approval requests,
//! offers and reveals, recovery proofs, `keys/current`, re-wrap requests,
//! the workspace list and the checkpoint routes.

use sync_protocol::codec::{Encode, Nested, Text};
use sync_protocol::messages::{
    ApprovalRequest, ChallengeResponse, CheckpointChainResponse, CheckpointResponse,
    DeviceAuthorizeResponse, DeviceTokenRequest, DeviceTokenResponse, DevicesResponse,
    EnrollRequest, Entry, EpochsResponse, HeadResponse, KeysResponse, LiveCursor, LiveResponse,
    ManifestBucketsResponse, ManifestLeavesResponse, OfferRequest, OffersResponse, PendingResponse,
    RecoveryRequest, RevealBody, RewrapBody, WorkspacesResponse,
};
use sync_protocol::records::{Enroll, Offer, RecoveryProof, Signed};
use sync_protocol::types::{Digest, Id, Key};

use super::{SyncClient, SyncError};

fn id_text(id: &Id) -> String {
    sync_protocol::types::to_base64url(&id.0)
}

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
    pub async fn reset_code(&mut self) -> Result<(), SyncError> {
        self.call_empty(reqwest::Method::POST, "/v2/account/reset-code", None)
            .await
    }

    /// `POST /v2/account/reset`: starts a new account chain (§7).
    pub async fn account_reset(
        &mut self,
        code: &str,
        genesis: Signed<sync_protocol::records::AccountLink>,
    ) -> Result<(), SyncError> {
        let body = sync_protocol::messages::ResetRequest {
            code: sync_protocol::codec::Text::new(code.to_owned())
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            genesis: sync_protocol::codec::Nested(genesis),
        }
        .to_bytes();
        self.call_empty(reqwest::Method::POST, "/v2/account/reset", Some(body))
            .await
    }

    /// `DELETE /v2/workspaces/{w}`: closes a workspace (§14).
    pub async fn close_workspace(&mut self, workspace: Id) -> Result<(), SyncError> {
        let path = format!("/v2/workspaces/{}", id_text(&workspace));
        self.call_empty(reqwest::Method::DELETE, &path, None).await
    }

    /// `GET .../missing-blobs`: the blobs a restore left missing (§19).
    pub async fn missing_blobs(
        &mut self,
        workspace: Id,
    ) -> Result<sync_protocol::messages::MissingBlobsResponse, SyncError> {
        let path = format!("/v2/workspaces/{}/missing-blobs", id_text(&workspace));
        self.call(reqwest::Method::GET, &path, None).await
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

    /// `GET /v2/workspaces/{w}/pending-devices`: the requests of invited
    /// and reset accounts, for an owner or admin device.
    pub async fn workspace_pending_devices(
        &mut self,
        workspace: Id,
    ) -> Result<PendingResponse, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/pending-devices",
            workspace.to_base64url()
        );
        self.call(reqwest::Method::GET, &path, None).await
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

    /// `GET /v2/workspaces/{w}/domains/{d}/heads/{device}`: the server's
    /// chain head for a device, or `None` when it holds no record of that
    /// device in the domain.
    pub async fn chain_head(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
    ) -> Result<Option<HeadResponse>, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/domains/{}/heads/{}",
            workspace.to_base64url(),
            domain.to_base64url(),
            device.to_base64url()
        );
        match self.call(reqwest::Method::GET, &path, None).await {
            Ok(head) => Ok(Some(head)),
            Err(SyncError::Refused { code, .. }) if code == "not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `GET /v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}`: the
    /// device's record at `counter`, or its next record above it, or `None`
    /// when it has none there.
    pub async fn chain_position(
        &mut self,
        workspace: Id,
        domain: Id,
        device: Id,
        counter: u64,
    ) -> Result<Option<Entry>, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/domains/{}/chain/{}/{counter}",
            workspace.to_base64url(),
            domain.to_base64url(),
            device.to_base64url()
        );
        match self.call(reqwest::Method::GET, &path, None).await {
            Ok(entry) => Ok(Some(entry)),
            Err(SyncError::Refused { code, .. }) if code == "not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn domain_path(workspace: Id, domain: Id, rest: &str) -> String {
        format!(
            "/v2/workspaces/{}/domains/{}/{rest}",
            workspace.to_base64url(),
            domain.to_base64url()
        )
    }

    /// `GET .../checkpoint`: the domain's latest checkpoint bundle with its
    /// entry's sequence, or `None` while the domain holds none.
    pub async fn checkpoint(
        &mut self,
        workspace: Id,
        domain: Id,
    ) -> Result<Option<CheckpointResponse>, SyncError> {
        let path = Self::domain_path(workspace, domain, "checkpoint");
        match self.call(reqwest::Method::GET, &path, None).await {
            Ok(response) => Ok(Some(response)),
            Err(SyncError::Refused { code, .. }) if code == "not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `GET .../checkpoints/{digest}`: a checkpoint's log entry, or `None`
    /// when the server holds no such checkpoint.
    pub async fn checkpoint_by_digest(
        &mut self,
        workspace: Id,
        domain: Id,
        digest: Digest,
    ) -> Result<Option<Entry>, SyncError> {
        let path = Self::domain_path(
            workspace,
            domain,
            &format!("checkpoints/{}", digest.to_base64url()),
        );
        match self.call(reqwest::Method::GET, &path, None).await {
            Ok(entry) => Ok(Some(entry)),
            Err(SyncError::Refused { code, .. }) if code == "not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `GET .../checkpoint-chain?from&to`: the descent walk from `from`
    /// back to `to`, newest first.
    pub async fn checkpoint_chain(
        &mut self,
        workspace: Id,
        domain: Id,
        from: Digest,
        to: Digest,
    ) -> Result<CheckpointChainResponse, SyncError> {
        let path = Self::domain_path(
            workspace,
            domain,
            &format!(
                "checkpoint-chain?from={}&to={}",
                from.to_base64url(),
                to.to_base64url()
            ),
        );
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `GET .../manifest?at&buckets=all`: the 4,096 bucket hashes at a
    /// servable checkpoint.
    pub async fn manifest_buckets(
        &mut self,
        workspace: Id,
        domain: Id,
        at: u64,
    ) -> Result<ManifestBucketsResponse, SyncError> {
        let path = Self::domain_path(workspace, domain, &format!("manifest?at={at}&buckets=all"));
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `GET .../manifest?at&level=3&index&after`: one page of a bucket's
    /// leaf refs.
    pub async fn manifest_leaves(
        &mut self,
        workspace: Id,
        domain: Id,
        at: u64,
        index: u16,
        after: Option<&LiveCursor>,
    ) -> Result<ManifestLeavesResponse, SyncError> {
        let mut rest = format!("manifest?at={at}&level=3&index={index}");
        if let Some(after) = after {
            rest.push_str(&format!("&after={}", after.to_query()));
        }
        let path = Self::domain_path(workspace, domain, &rest);
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `GET .../live?at&after`, or with `rid`: one page of the operations
    /// live at a servable checkpoint.
    pub async fn live(
        &mut self,
        workspace: Id,
        domain: Id,
        at: u64,
        rid: Option<Id>,
        after: Option<&LiveCursor>,
    ) -> Result<LiveResponse, SyncError> {
        let mut rest = format!("live?at={at}");
        if let Some(rid) = rid {
            rest.push_str(&format!("&rid={}", rid.to_base64url()));
        }
        if let Some(after) = after {
            rest.push_str(&format!("&after={}", after.to_query()));
        }
        let path = Self::domain_path(workspace, domain, &rest);
        self.call(reqwest::Method::GET, &path, None).await
    }
}
