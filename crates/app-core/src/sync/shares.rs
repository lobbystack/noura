//! Share links on this device (docs/workspace-format/sync-v2.md §24).
//!
//! Creating a link pads and encrypts a snapshot of a note under a fresh
//! link key, signs it with a fresh link key pair and uploads it. The link's
//! fragment carries the link key and the verify key, which no request from
//! this device ever carries. The device keeps each link's token, link key
//! and signing seed in its credential store, and in its application data
//! the [`ShareBook`]: which note each of its links shares, which the server
//! never learns, and the token hash of each link it revoked until the link
//! would have expired, so a device can revoke it again after a server
//! restore (§14).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sync_protocol::codec::{Bytes, Encode, Nested, Text};
use sync_protocol::crypto::{SigningKey, random_array, random_nonce};
use sync_protocol::messages::{CreateShareRequest, ExpiresAtResponse, ShareBody, SharesResponse};
use sync_protocol::policy::Snapshot;
use sync_protocol::records::{
    Share, ShareError, SharePayload, Signed, share_fragment, share_token_hash,
};
use sync_protocol::types::{Digest, Id, SecretKey, from_hex, to_base64url, to_hex};
use zeroize::Zeroizing;

use super::journal::durable_write;
use super::{SyncClient, SyncError, SyncService, random};

/// The longest a link lives, in days, and the expiry a new link takes
/// unless you pick another.
pub const DEFAULT_SHARE_DAYS: u8 = 30;

/// One day in seconds.
const DAY: u64 = 86_400;

/// A link this device created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnLink {
    /// The token's hash, which names the link to the server.
    pub token_hash: Digest,
    /// The workspace.
    pub workspace: Id,
    /// The domain of the note.
    pub domain: Id,
    /// The note's remote ID, which survives moves and renames.
    pub note: Id,
    /// Unix seconds.
    pub expires_at: u64,
    /// The snapshot revision the server holds.
    pub revision: u64,
}

/// A link this device revoked, kept until it would have expired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokedLink {
    /// The token's hash.
    pub token_hash: Digest,
    /// The workspace.
    pub workspace: Id,
    /// When the link would have expired.
    pub expires_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    links: Vec<OwnLink>,
    revoked: Vec<RevokedLink>,
}

/// The links this device created and revoked, in a file in the app's
/// application data.
#[derive(Debug)]
pub struct ShareBook {
    path: PathBuf,
    device: Id,
    links: Vec<OwnLink>,
    revoked: Vec<RevokedLink>,
}

fn failed(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the list of share links can't be used: {error}"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl ShareBook {
    /// The book of `device` in `app_data`, empty when it has no file yet.
    pub fn open(app_data: &Path, device: Id) -> Result<Self, SyncError> {
        let path = app_data.join(format!("sync-shares-{}.json", device.to_hex()));
        let stored: Stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(failed)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(error) => return Err(failed(error)),
        };
        Ok(Self {
            path,
            device,
            links: stored.links,
            revoked: stored.revoked,
        })
    }

    /// The device whose links the book holds.
    pub fn device(&self) -> Id {
        self.device
    }

    /// The links of `note` in `workspace` that haven't expired.
    pub fn note_links(&self, workspace: Id, note: Id) -> Vec<OwnLink> {
        let now = now();
        self.links
            .iter()
            .filter(|l| l.workspace == workspace && l.note == note && l.expires_at > now)
            .cloned()
            .collect()
    }

    /// The link under `token_hash`, if this device created it.
    pub fn link(&self, token_hash: &Digest) -> Option<&OwnLink> {
        self.links.iter().find(|l| l.token_hash == *token_hash)
    }

    /// The links this device revoked that would still be live.
    pub fn revoked(&self) -> Vec<RevokedLink> {
        let now = now();
        self.revoked
            .iter()
            .filter(|r| r.expires_at > now)
            .cloned()
            .collect()
    }

    fn save(&mut self) -> Result<(), SyncError> {
        let now = now();
        self.links.retain(|l| l.expires_at > now);
        self.revoked.retain(|r| r.expires_at > now);
        let bytes = serde_json::to_vec_pretty(&Stored {
            links: self.links.clone(),
            revoked: self.revoked.clone(),
        })
        .map_err(failed)?;
        durable_write(&self.path, &bytes).map_err(failed)
    }
}

/// The credential store item that holds a link's secrets.
fn secret_item(device: &Id, token_hash: &Digest) -> String {
    format!(
        "noura.sync.v2.share.{}.{}",
        device.to_hex(),
        to_hex(&token_hash.0)
    )
}

/// A link's secrets: its token, its link key and its signing seed.
struct LinkSecrets {
    token: [u8; 16],
    key: SecretKey,
    seed: SecretKey,
}

impl LinkSecrets {
    fn generate() -> Result<Self, SyncError> {
        Ok(Self {
            token: random(random_array())?,
            key: random(SecretKey::random())?,
            seed: random(SecretKey::random())?,
        })
    }

    fn hash(&self) -> Digest {
        share_token_hash(&self.token)
    }

    fn encode(&self) -> Zeroizing<String> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(80));
        bytes.extend_from_slice(&self.token);
        bytes.extend_from_slice(self.key.expose());
        bytes.extend_from_slice(self.seed.expose());
        Zeroizing::new(to_hex(&bytes))
    }

    fn decode(text: &str) -> Option<Self> {
        let bytes = Zeroizing::new(from_hex(text)?);
        if bytes.len() != 80 {
            return None;
        }
        let mut token = [0u8; 16];
        token.copy_from_slice(&bytes[..16]);
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[16..48]);
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes[48..]);
        Some(Self {
            token,
            key: SecretKey(key),
            seed: SecretKey(seed),
        })
    }

    fn seal(
        &self,
        revision: u64,
        title: &str,
        markdown: &[u8],
    ) -> Result<Signed<Share>, SyncError> {
        let payload = SharePayload {
            title: Text(title.to_owned()),
            markdown: Bytes(markdown.to_vec()),
        };
        Share::seal(
            self.hash(),
            revision,
            &payload,
            &self.key,
            &SigningKey::from_seed(&self.seed),
            random(random_nonce())?,
        )
        .map_err(|error| match error {
            ShareError::TooLarge => SyncError::Invalid(
                "This note is too large to share. A shared note holds at most 960 KB.".into(),
            ),
            other => SyncError::Invalid(format!("the note can't be sealed: {other}")),
        })
    }

    /// The link: `{origin}/s/{token}#{link key || verify key}`.
    fn url(&self, origin: &str) -> String {
        let verify = SigningKey::from_seed(&self.seed).public();
        format!(
            "{origin}/s/{}#{}",
            to_base64url(&self.token),
            share_fragment(&self.key, &verify)
        )
    }
}

/// What Share link sends: the note and its snapshot.
#[derive(Clone, Debug)]
pub struct NewShare<'a> {
    /// The workspace.
    pub workspace: Id,
    /// The note's domain.
    pub domain: Id,
    /// The note's remote ID.
    pub note: Id,
    /// The note's title.
    pub title: &'a str,
    /// The note's Markdown, without its properties.
    pub markdown: &'a [u8],
    /// 1 to 30.
    pub days: u8,
}

/// A link you just created.
#[derive(Clone, PartialEq, Eq)]
pub struct CreatedShare {
    /// The link to copy, with its keys in the fragment.
    pub url: Zeroizing<String>,
    /// The link's record in the book.
    pub link: OwnLink,
}

impl std::fmt::Debug for CreatedShare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedShare")
            .field("link", &self.link)
            .finish_non_exhaustive()
    }
}

/// A link of the workspace as owner and admin devices see it: by token
/// hash, domain, creating device and expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceShare {
    /// The token's hash.
    pub token_hash: Digest,
    /// The domain.
    pub domain: Id,
    /// The creating device.
    pub device: Id,
    /// Unix seconds.
    pub expires_at: u64,
    /// Whether the creating device is no longer in the workspace, so a
    /// malicious server working with it could still replace the snapshot.
    pub creator_removed: bool,
}

/// The title and Markdown a snapshot of a note carries: its name, and its
/// text without the properties at its top.
pub fn snapshot_of(name: &str, text: &str) -> (String, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let normalized = text.replace("\r\n", "\n");
    let body = match workspace_format::frontmatter_end(&normalized) {
        Some(end) => normalized[end..].trim_start_matches('\n').to_owned(),
        None => normalized,
    };
    (name.to_owned(), body)
}

fn shares_path(workspace: Id) -> String {
    format!("/v2/workspaces/{}/shares", workspace.to_base64url())
}

impl SyncClient {
    /// `POST .../shares`, sent again with identical bytes once if the
    /// server can't be reached, so a retry returns the original result.
    async fn post_share(
        &mut self,
        workspace: Id,
        body: Vec<u8>,
    ) -> Result<ExpiresAtResponse, SyncError> {
        let path = shares_path(workspace);
        match self
            .call(reqwest::Method::POST, &path, Some(body.clone()))
            .await
        {
            Err(SyncError::Network(_)) => self.call(reqwest::Method::POST, &path, Some(body)).await,
            other => other,
        }
    }
}

impl SyncService {
    /// Share link: creates a link to a snapshot of a note that expires in
    /// `days` days. The device must be an owner or admin device active in
    /// the workspace.
    pub async fn create_share(
        &mut self,
        book: &mut ShareBook,
        share: NewShare<'_>,
    ) -> Result<CreatedShare, SyncError> {
        if !(1..=DEFAULT_SHARE_DAYS).contains(&share.days) {
            return Err(SyncError::Invalid("A link expires in 1 to 30 days.".into()));
        }
        let secrets = LinkSecrets::generate()?;
        let token_hash = secrets.hash();
        let snapshot = secrets.seal(1, share.title, share.markdown)?;
        let item = secret_item(&book.device, &token_hash);
        // The secrets reach the credential store before the link exists, so
        // a crash after the upload never leaves a link this device can't
        // update.
        self.secrets
            .write(&item, &secrets.encode())
            .map_err(|e| SyncError::Invalid(format!("the credential store failed: {e}")))?;
        let request = CreateShareRequest {
            token_hash,
            domain: share.domain,
            expires_in_days: share.days,
            share: Nested(snapshot),
        }
        .to_bytes();
        let response = match self.client.post_share(share.workspace, request).await {
            Ok(response) => response,
            Err(error) => {
                let _ = self.secrets.delete(&item);
                return Err(error);
            }
        };
        let link = OwnLink {
            token_hash,
            workspace: share.workspace,
            domain: share.domain,
            note: share.note,
            expires_at: response.expires_at,
            revision: 1,
        };
        book.links.push(link.clone());
        book.save()?;
        Ok(CreatedShare {
            url: Zeroizing::new(secrets.url(&self.client.origin()?)),
            link,
        })
    }

    /// The link of one of this device's links, to copy again.
    pub fn share_url(&self, book: &ShareBook, token_hash: &Digest) -> Result<String, SyncError> {
        let secrets = self.link_secrets(book, token_hash)?;
        Ok(secrets.url(&self.client.origin()?))
    }

    fn link_secrets(
        &self,
        book: &ShareBook,
        token_hash: &Digest,
    ) -> Result<LinkSecrets, SyncError> {
        let text = self
            .secrets
            .read_optional(&secret_item(&book.device, token_hash))
            .map_err(|e| SyncError::Invalid(format!("the credential store failed: {e}")))?;
        text.as_deref()
            .filter(|t| !t.is_empty())
            .and_then(|t| LinkSecrets::decode(t))
            .ok_or_else(|| {
                SyncError::Invalid("Only the device that created a link can update it.".into())
            })
    }

    /// Update link: replaces the snapshot under the same token and keys
    /// and raises `snapshot_revision`. Only the device that created the
    /// link can.
    pub async fn update_share(
        &mut self,
        book: &mut ShareBook,
        token_hash: Digest,
        title: &str,
        markdown: &[u8],
    ) -> Result<(), SyncError> {
        let link = book.link(&token_hash).cloned().ok_or_else(|| {
            SyncError::Invalid("Only the device that created a link can update it.".into())
        })?;
        let secrets = self.link_secrets(book, &token_hash)?;
        let revision = link.revision + 1;
        let body = ShareBody {
            share: Nested(secrets.seal(revision, title, markdown)?),
        }
        .to_bytes();
        let path = format!(
            "{}/{}",
            shares_path(link.workspace),
            token_hash.to_base64url()
        );
        self.client
            .call_empty(reqwest::Method::PUT, &path, Some(body))
            .await?;
        if let Some(own) = book.links.iter_mut().find(|l| l.token_hash == token_hash) {
            own.revision = revision;
        }
        book.save()
    }

    /// Revokes a link of `workspace`, which deletes its snapshot. The
    /// device keeps its token hash until the link would have expired.
    pub async fn revoke_share(
        &mut self,
        book: &mut ShareBook,
        workspace: Id,
        token_hash: Digest,
        expires_at: u64,
    ) -> Result<(), SyncError> {
        let path = format!("{}/{}", shares_path(workspace), token_hash.to_base64url());
        self.client
            .call_empty(reqwest::Method::DELETE, &path, None)
            .await?;
        book.links.retain(|l| l.token_hash != token_hash);
        if !book.revoked.iter().any(|r| r.token_hash == token_hash) {
            book.revoked.push(RevokedLink {
                token_hash,
                workspace,
                expires_at,
            });
        }
        let _ = self.secrets.delete(&secret_item(&book.device, &token_hash));
        book.save()
    }

    /// Every active link of the workspace, for an owner or admin device,
    /// flagging those whose creating device `policy` no longer counts.
    pub async fn workspace_shares(
        &mut self,
        workspace: Id,
        policy: &Snapshot,
    ) -> Result<Vec<WorkspaceShare>, SyncError> {
        let response: SharesResponse = self
            .client
            .call(reqwest::Method::GET, &shares_path(workspace), None)
            .await?;
        Ok(response
            .shares
            .0
            .into_iter()
            .map(|info| WorkspaceShare {
                token_hash: info.token_hash,
                domain: info.domain,
                device: info.device,
                expires_at: info.expires_at,
                creator_removed: !policy.is_device(&info.device),
            })
            .collect())
    }
}

/// How many whole days remain until `expires_at`, at least 1 while the
/// link lives.
pub fn days_left(expires_at: u64) -> u64 {
    expires_at.saturating_sub(now()).div_ceil(DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_leaves_the_properties_out() {
        let (title, markdown) = snapshot_of(
            "Plan",
            "---\r\nid: note_1\r\ntype: note\r\n---\r\n\r\n# Plan\r\n\r\nMay.\r\n",
        );
        assert_eq!(title, "Plan");
        assert_eq!(markdown, "# Plan\n\nMay.\n");
        assert_eq!(snapshot_of("Plain", "Just text").1, "Just text");
    }

    #[test]
    fn link_secrets_round_trip_through_their_text() {
        let secrets = LinkSecrets::generate().unwrap();
        let back = LinkSecrets::decode(&secrets.encode()).unwrap();
        assert_eq!(back.token, secrets.token);
        assert_eq!(back.key.expose(), secrets.key.expose());
        assert_eq!(back.seed.expose(), secrets.seed.expose());
        assert!(LinkSecrets::decode("").is_none());
        let url = secrets.url("https://sync.example.com");
        assert!(url.starts_with("https://sync.example.com/s/"));
        let (path, fragment) = url.split_once('#').unwrap();
        assert!(!path.contains(&to_base64url(secrets.key.expose())));
        assert_eq!(fragment.len(), 86);
    }
}
