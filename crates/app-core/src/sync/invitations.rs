//! Invitations on this device (docs/workspace-format/sync-v2.md §11).
//!
//! The inviting device generates the invitation ID, the token and the
//! secret. It keeps the token and the secret in its credential store until
//! the invitation ends, and in its application data the
//! [`InvitationBook`]: which invitations it sent, to which workspace, so it
//! can drop their secrets once the server no longer lists them. No request
//! carries the secret: the server gets the token's hash and the context
//! sealed under a subkey of the secret.
//!
//! The invitee's app reads the link, keeps the secret in memory, fetches the
//! context by token, opens it and shows the workspace, the inviter and the
//! role, then accepts with the token once signed in. Acceptance binds the
//! invitation to the account and grants nothing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sync_protocol::codec::{Encode, Nested, Text};
use sync_protocol::crypto::{random_array, random_nonce};
use sync_protocol::messages::{
    CreateInvitationRequest, ExpiresAtResponse, InvitationResponse, InvitationStatus,
    InvitationsResponse,
};
use sync_protocol::records::{InviteContext, InvitePayload, Role, invite_token_hash};
use sync_protocol::types::{Digest, Id, SecretKey, from_base64url, from_hex, to_base64url, to_hex};
use zeroize::Zeroizing;

use super::journal::durable_write;
use super::{SyncClient, SyncError, SyncService, random};

/// What the invitee sees for any link that doesn't open: a wrong secret, a
/// tampered context, or an invitation that ended. It never says which.
pub const INVITATION_UNUSABLE: &str =
    "This invitation link doesn't work. Ask the person who invited you for a new one.";

fn unusable() -> SyncError {
    SyncError::Invalid(INVITATION_UNUSABLE.into())
}

fn another_server() -> SyncError {
    SyncError::Invalid("This invitation is for another sync server.".into())
}

fn failed(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the list of invitations can't be used: {error}"))
}

fn store_failed(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the credential store failed: {error}"))
}

/// An invitation link's parts: the server's origin, the token in its path
/// and the secret in its fragment.
pub struct InvitationLink {
    /// The origin, such as `https://sync.noura.app`.
    pub origin: String,
    /// The 16-byte token.
    pub token: [u8; 16],
    /// The 32-byte secret, which never leaves this device.
    pub secret: SecretKey,
}

impl std::fmt::Debug for InvitationLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvitationLink")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl InvitationLink {
    /// Reads `{origin}/invite/{base64url(token)}#{base64url(secret)}`, as
    /// you open or paste it, with surrounding whitespace ignored.
    pub fn parse(link: &str) -> Result<Self, SyncError> {
        let link = link.trim();
        let (address, fragment) = link.split_once('#').ok_or_else(unusable)?;
        let (origin, token) = address.rsplit_once("/invite/").ok_or_else(unusable)?;
        let url = reqwest::Url::parse(origin).map_err(|_| unusable())?;
        if !matches!(url.scheme(), "https" | "http") || url.path() != "/" {
            return Err(unusable());
        }
        let token = from_base64url::<16>(token).map_err(|_| unusable())?;
        let secret = Zeroizing::new(from_base64url::<32>(fragment).map_err(|_| unusable())?);
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            token,
            secret: SecretKey(*secret),
        })
    }

    /// The link's text.
    pub fn url(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{}/invite/{}#{}",
            self.origin,
            to_base64url(&self.token),
            to_base64url(self.secret.expose())
        ))
    }
}

/// An invitation as the invitee's app shows it, after its context opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedInvitation {
    /// The workspace.
    pub workspace: Id,
    /// The invitation.
    pub invitation: Id,
    /// The role it grants.
    pub role: Role,
    /// Pending, or accepted already.
    pub status: InvitationStatus,
    /// The workspace's name.
    pub workspace_name: String,
    /// The inviter's name.
    pub inviter_name: String,
    /// The fingerprint of the inviting device's card, which the approval
    /// offer must come from.
    pub inviter_fingerprint: Digest,
}

/// Opens a context the server returned for `link`. A context whose
/// cleartext fields differ from the server's answer counts as tampered.
pub fn open_context(
    link: &InvitationLink,
    response: &InvitationResponse,
) -> Result<OpenedInvitation, SyncError> {
    let context = &response.invite_context.0;
    if context.workspace != response.workspace
        || context.invitation != response.invitation
        || context.role != response.role
    {
        return Err(unusable());
    }
    let payload = context.open(&link.secret).map_err(|_| unusable())?;
    Ok(OpenedInvitation {
        workspace: context.workspace,
        invitation: context.invitation,
        role: context.role,
        status: response.status,
        workspace_name: payload.workspace_name.0,
        inviter_name: payload.inviter_name.0,
        inviter_fingerprint: payload.inviter_fingerprint,
    })
}

/// Opens `link` before you sign in, against the app's server at `server`:
/// the context route takes no session, so the app can show the workspace,
/// the inviter and the role before you sign in or sign up (§11).
pub async fn open_signed_out(
    server: &str,
    link: &InvitationLink,
) -> Result<OpenedInvitation, SyncError> {
    let origin = reqwest::Url::parse(server)
        .map_err(|_| SyncError::Invalid("the server address isn't a URL".into()))?
        .origin()
        .ascii_serialization();
    if link.origin != origin {
        return Err(another_server());
    }
    let mut client = SyncClient::connect(server, "").await?;
    client.open_invitation(link).await
}

impl SyncClient {
    /// `GET /v2/invitations/{token}`, opened with the link's secret. The
    /// link must name this client's server, and any failure, the server's
    /// included, reads as [`INVITATION_UNUSABLE`].
    pub async fn open_invitation(
        &mut self,
        link: &InvitationLink,
    ) -> Result<OpenedInvitation, SyncError> {
        if link.origin != self.origin()? {
            return Err(another_server());
        }
        let path = format!("/v2/invitations/{}", to_base64url(&link.token));
        let response: InvitationResponse = match self.call(reqwest::Method::GET, &path, None).await
        {
            Ok(response) => response,
            Err(SyncError::Network(error)) => return Err(SyncError::Network(error)),
            Err(_) => return Err(unusable()),
        };
        open_context(link, &response)
    }

    /// `POST /v2/invitations/{token}/accept` under the signed-in account,
    /// which binds the invitation to it and grants nothing.
    pub async fn accept_invitation(&mut self, link: &InvitationLink) -> Result<(), SyncError> {
        let path = format!("/v2/invitations/{}/accept", to_base64url(&link.token));
        match self
            .call_empty(reqwest::Method::POST, &path, Some(Vec::new()))
            .await
        {
            Err(SyncError::Refused { code, .. }) if code == "invitation_invalid" => Err(unusable()),
            other => other,
        }
    }
}

/// An invitation this device sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnInvitation {
    /// The invitation ID.
    pub invitation: Id,
    /// The workspace.
    pub workspace: Id,
    /// The role it grants.
    pub role: Role,
    /// Unix seconds, 0 until the server confirms the creation.
    pub expires_at: u64,
}

/// The invitations this device sent and hasn't seen end, in a file in the
/// app's application data.
#[derive(Debug)]
pub struct InvitationBook {
    path: PathBuf,
    device: Id,
    invitations: Vec<OwnInvitation>,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    invitations: Vec<OwnInvitation>,
}

impl InvitationBook {
    /// The book of `device` in `app_data`, empty when it has no file yet.
    pub fn open(app_data: &Path, device: Id) -> Result<Self, SyncError> {
        let path = app_data.join(format!("sync-invitations-{}.json", device.to_hex()));
        let stored: Stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(failed)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(error) => return Err(failed(error)),
        };
        Ok(Self {
            path,
            device,
            invitations: stored.invitations,
        })
    }

    /// The invitations this device sent that it hasn't seen end.
    pub fn invitations(&self) -> &[OwnInvitation] {
        &self.invitations
    }

    fn save(&self) -> Result<(), SyncError> {
        let bytes = serde_json::to_vec_pretty(&Stored {
            invitations: self.invitations.clone(),
        })
        .map_err(failed)?;
        durable_write(&self.path, &bytes).map_err(failed)
    }
}

/// The credential store item that holds an invitation's token and secret.
fn secret_item(device: &Id, invitation: &Id) -> String {
    format!(
        "noura.sync.v2.invitation.{}.{}",
        device.to_hex(),
        invitation.to_hex()
    )
}

fn encode_secrets(token: &[u8; 16], secret: &SecretKey) -> Zeroizing<String> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(48));
    bytes.extend_from_slice(token);
    bytes.extend_from_slice(secret.expose());
    Zeroizing::new(to_hex(&bytes))
}

fn decode_secrets(text: &str) -> Option<([u8; 16], SecretKey)> {
    let bytes = Zeroizing::new(from_hex(text)?);
    if bytes.len() != 48 {
        return None;
    }
    let mut token = [0u8; 16];
    token.copy_from_slice(&bytes[..16]);
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bytes[16..]);
    Some((token, SecretKey(secret)))
}

/// What Invite sends.
#[derive(Clone, Debug)]
pub struct NewInvitation<'a> {
    /// The workspace.
    pub workspace: Id,
    /// Admin, editor or viewer.
    pub role: Role,
    /// The workspace's name, which only holders of the link read.
    pub workspace_name: &'a str,
    /// Your name, as the invitee sees it.
    pub inviter_name: &'a str,
}

/// An invitation you just created.
pub struct CreatedInvitation {
    /// The link to send, with the secret in its fragment.
    pub url: Zeroizing<String>,
    /// The invitation's record in the book.
    pub invitation: OwnInvitation,
}

impl std::fmt::Debug for CreatedInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedInvitation")
            .field("invitation", &self.invitation)
            .finish_non_exhaustive()
    }
}

/// An open invitation of the workspace as owner and admin devices see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceInvitation {
    /// The invitation.
    pub invitation: Id,
    /// The role it grants.
    pub role: Role,
    /// Pending, or accepted by an account that waits for approval.
    pub status: InvitationStatus,
    /// The account that accepted it.
    pub accepted_by: Option<Id>,
    /// Unix seconds.
    pub expires_at: u64,
    /// Whether this device sent it, so only it can approve the invitee.
    pub sent_here: bool,
}

fn invitations_path(workspace: Id) -> String {
    format!("/v2/workspaces/{}/invitations", workspace.to_base64url())
}

impl SyncService {
    /// Invite: creates an invitation to `workspace` with `role`, open for
    /// seven days, and returns its link. The device must be an owner or
    /// admin device active in the workspace, and only an owner invites
    /// admins.
    ///
    /// The token and the secret reach the credential store, and the
    /// invitation the book, before the creation goes out. When the server
    /// can't be reached the device sends the same bytes once more, and the
    /// server returns the original expiry; a refusal forgets the
    /// invitation.
    pub async fn create_invitation(
        &mut self,
        book: &mut InvitationBook,
        invitation: NewInvitation<'_>,
    ) -> Result<CreatedInvitation, SyncError> {
        if !matches!(invitation.role, Role::Admin | Role::Editor | Role::Viewer) {
            return Err(SyncError::Invalid(
                "An invitation makes someone an admin, an editor or a viewer.".into(),
            ));
        }
        let id = random(Id::random())?;
        let token: [u8; 16] = random(random_array())?;
        let secret = random(SecretKey::random())?;
        let payload = InvitePayload {
            workspace_name: Text(invitation.workspace_name.to_owned()),
            inviter_name: Text(invitation.inviter_name.to_owned()),
            inviter_fingerprint: self.credentials.card().fingerprint(),
        };
        let context = InviteContext::seal(
            invitation.workspace,
            id,
            invitation.role,
            &payload,
            &secret,
            random(random_nonce())?,
        );
        let body = CreateInvitationRequest {
            invitation: id,
            token_hash: invite_token_hash(&token),
            role: invitation.role,
            invite_context: Nested(context),
        }
        .to_bytes();
        let item = secret_item(&book.device, &id);
        self.secrets
            .write(&item, &encode_secrets(&token, &secret))
            .map_err(store_failed)?;
        book.invitations.push(OwnInvitation {
            invitation: id,
            workspace: invitation.workspace,
            role: invitation.role,
            expires_at: 0,
        });
        if let Err(error) = book.save() {
            self.forget_invitation(book, &id);
            return Err(error);
        }
        let path = invitations_path(invitation.workspace);
        let sent: Result<ExpiresAtResponse, SyncError> = match self
            .client
            .call(reqwest::Method::POST, &path, Some(body.clone()))
            .await
        {
            Err(SyncError::Network(_)) => {
                self.client
                    .call(reqwest::Method::POST, &path, Some(body))
                    .await
            }
            other => other,
        };
        let response = match sent {
            Ok(response) => response,
            Err(error @ SyncError::Refused { .. }) => {
                self.forget_invitation(book, &id);
                let _ = book.save();
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let own = book
            .invitations
            .iter_mut()
            .find(|i| i.invitation == id)
            .ok_or_else(|| failed("the new invitation went missing"))?;
        own.expires_at = response.expires_at;
        let own = own.clone();
        book.save()?;
        let link = InvitationLink {
            origin: self.client.origin()?,
            token,
            secret,
        };
        Ok(CreatedInvitation {
            url: link.url(),
            invitation: own,
        })
    }

    /// Drops an invitation from the book and its secrets from the
    /// credential store.
    fn forget_invitation(&self, book: &mut InvitationBook, invitation: &Id) {
        book.invitations.retain(|i| i.invitation != *invitation);
        let _ = self.secrets.delete(&secret_item(&book.device, invitation));
    }

    /// The secret of an invitation this device sent, which checks the
    /// invitee's `invite_proof` before an offer.
    pub fn invitation_secret(
        &self,
        book: &InvitationBook,
        invitation: &Id,
    ) -> Result<Option<SecretKey>, SyncError> {
        if !book.invitations.iter().any(|i| i.invitation == *invitation) {
            return Ok(None);
        }
        let text = self
            .secrets
            .read_optional(&secret_item(&book.device, invitation))
            .map_err(store_failed)?;
        Ok(text
            .as_deref()
            .and_then(|t| decode_secrets(t))
            .map(|(_, secret)| secret))
    }

    /// The link of an invitation this device sent, to copy again.
    pub fn invitation_url(
        &self,
        book: &InvitationBook,
        invitation: &Id,
    ) -> Result<Zeroizing<String>, SyncError> {
        let missing = || {
            SyncError::Invalid(
                "Only the device that sent this invitation can copy its link.".into(),
            )
        };
        if !book.invitations.iter().any(|i| i.invitation == *invitation) {
            return Err(missing());
        }
        let text = self
            .secrets
            .read_optional(&secret_item(&book.device, invitation))
            .map_err(store_failed)?
            .ok_or_else(missing)?;
        let (token, secret) = decode_secrets(&text).ok_or_else(missing)?;
        Ok(InvitationLink {
            origin: self.client.origin()?,
            token,
            secret,
        }
        .url())
    }

    /// The workspace's open invitations, for an owner or admin device. The
    /// device drops the secrets of its own invitations that the server no
    /// longer lists, since those ended: approved, revoked or expired.
    pub async fn workspace_invitations(
        &mut self,
        book: &mut InvitationBook,
        workspace: Id,
    ) -> Result<Vec<WorkspaceInvitation>, SyncError> {
        let response: InvitationsResponse = self
            .client
            .call(reqwest::Method::GET, &invitations_path(workspace), None)
            .await?;
        let listed: Vec<Id> = response
            .invitations
            .0
            .iter()
            .map(|i| i.invitation)
            .collect();
        let ended: Vec<Id> = book
            .invitations
            .iter()
            .filter(|i| {
                i.workspace == workspace && i.expires_at != 0 && !listed.contains(&i.invitation)
            })
            .map(|i| i.invitation)
            .collect();
        if !ended.is_empty() {
            for invitation in &ended {
                self.forget_invitation(book, invitation);
            }
            book.save()?;
        }
        Ok(response
            .invitations
            .0
            .into_iter()
            .map(|info| WorkspaceInvitation {
                invitation: info.invitation,
                role: info.role,
                status: info.status,
                accepted_by: info.accepted_by,
                expires_at: info.expires_at,
                sent_here: book
                    .invitations
                    .iter()
                    .any(|i| i.invitation == info.invitation),
            })
            .collect())
    }

    /// Revokes an invitation of `workspace`, so its link fails, and drops
    /// its secrets when this device sent it. Revoking one that ended
    /// already succeeds.
    pub async fn revoke_invitation(
        &mut self,
        book: &mut InvitationBook,
        workspace: Id,
        invitation: Id,
    ) -> Result<(), SyncError> {
        let path = format!(
            "{}/{}",
            invitations_path(workspace),
            invitation.to_base64url()
        );
        self.client
            .call_empty(reqwest::Method::DELETE, &path, None)
            .await?;
        self.forget_invitation(book, &invitation);
        book.save()
    }

    /// Opens an invitation link on this server, as [`SyncClient::open_invitation`].
    pub async fn open_invitation(
        &mut self,
        link: &InvitationLink,
    ) -> Result<OpenedInvitation, SyncError> {
        self.client.open_invitation(link).await
    }

    /// Accepts an invitation under this device's account, as
    /// [`SyncClient::accept_invitation`].
    pub async fn accept_invitation(&mut self, link: &InvitationLink) -> Result<(), SyncError> {
        self.client.accept_invitation(link).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_round_trips_through_its_text() {
        let link = InvitationLink {
            origin: "https://sync.example.com".into(),
            token: [7; 16],
            secret: SecretKey([9; 32]),
        };
        let text = link.url();
        let back = InvitationLink::parse(&format!("  {}\n", *text)).unwrap();
        assert_eq!(back.origin, link.origin);
        assert_eq!(back.token, link.token);
        assert_eq!(back.secret.expose(), link.secret.expose());
        let (path, fragment) = text.split_once('#').unwrap();
        assert!(!path.contains(&to_base64url(link.secret.expose())));
        assert_eq!(fragment.len(), 43);
    }

    #[test]
    fn a_broken_link_reads_as_unusable() {
        for text in [
            "",
            "https://sync.example.com/invite/QUFBQUFBQUFBQUFBQUFBQQ",
            "https://sync.example.com/s/QUFBQUFBQUFBQUFBQUFBQQ#VzDPBhWIWlWMhRAWJgA6Xb7j9OkGfYzTTNWlCwXg408",
            "https://sync.example.com/invite/QUFB#VzDPBhWIWlWMhRAWJgA6Xb7j9OkGfYzTTNWlCwXg408",
            "https://sync.example.com/invite/QUFBQUFBQUFBQUFBQUFBQQ#VzDP",
            "ftp://sync.example.com/invite/QUFBQUFBQUFBQUFBQUFBQQ#VzDPBhWIWlWMhRAWJgA6Xb7j9OkGfYzTTNWlCwXg408",
        ] {
            let error = InvitationLink::parse(text).unwrap_err();
            assert_eq!(error.to_string(), INVITATION_UNUSABLE, "{text}");
        }
    }

    #[test]
    fn secrets_round_trip_through_their_text() {
        let secret = SecretKey([3; 32]);
        let (token, back) = decode_secrets(&encode_secrets(&[5; 16], &secret)).unwrap();
        assert_eq!(token, [5; 16]);
        assert_eq!(back.expose(), secret.expose());
        assert!(decode_secrets("").is_none());
    }
}
