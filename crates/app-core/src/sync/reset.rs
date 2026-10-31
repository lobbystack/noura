//! Reset devices (docs/workspace-format/sync-v2.md §7): a new account chain
//! after an email check, for someone who lost every device and kit or whose
//! account chain a restore cut short. Each workspace then accepts the reset
//! as the commit of an approval, which `WorkspaceSync::accept_reset` signs.

use local_core::sync::SyncCredentials;
use sync_key_envelope::v2::{DeviceKeys, genesis_link};
use sync_protocol::account::AccountChain;
use sync_protocol::records::{AccountHead, AccountLink, Signed};
use sync_protocol::types::Digest;

use super::device::{StoredDevice, keep_retired};
use super::{DeviceCredentials, SyncClient, SyncError};

/// What a finished reset gives you: the device's new credentials and the
/// new fingerprint the app shows.
#[derive(Clone)]
pub struct ResetDone {
    /// The new device's credentials on the new chain.
    pub credentials: DeviceCredentials,
    /// The account's new fingerprint, the digest of the new genesis.
    pub fingerprint: Digest,
    /// The head an approval request for each workspace names.
    pub head: AccountHead,
    /// The device this computer held on the retired chain, whose secrets
    /// stay aside and whose kept heads no longer count
    /// ([`super::account_heads::KeptHeads::forget_retired`]).
    pub retired: Option<sync_protocol::types::Id>,
}

impl std::fmt::Debug for ResetDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResetDone")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// Asks the server to email a reset code to the account's address.
pub async fn request_code(client: &mut SyncClient) -> Result<(), SyncError> {
    client.reset_code().await
}

/// Starts a new chain for `account` with the emailed `code`: the device
/// that signed in and enrolled `keys`, which found the account's chain and
/// can't be approved, signs a genesis. The device's new credentials replace
/// what `store` keeps for this server. A wrong, used or expired code
/// changes nothing.
pub async fn reset(
    client: &mut SyncClient,
    store: &dyn SyncCredentials,
    account: sync_protocol::types::Id,
    keys: &DeviceKeys,
    token: &str,
    code: &str,
) -> Result<ResetDone, SyncError> {
    let genesis: Signed<AccountLink> = genesis_link(keys, account);
    client.account_reset(code, genesis.clone()).await?;
    let chain = AccountChain::verify(std::slice::from_ref(&genesis), &Default::default())
        .map_err(|e| SyncError::Check(e.client_code()))?;
    let head = chain.head();
    let server = client.base().to_owned();
    let retired = match StoredDevice::load(store, &server)? {
        Some(old) if old.account == account => Some(keep_retired(store, &server, &old)?),
        _ => None,
    };
    let mut stored = StoredDevice::new(&server, account, keys, token);
    stored.reference_head = Some(head.clone());
    stored.save(store)?;
    let credentials = stored
        .credentials()?
        .ok_or_else(|| SyncError::Invalid("the new device has no reference head".into()))?;
    Ok(ResetDone {
        credentials,
        fingerprint: genesis.digest(),
        head,
        retired,
    })
}
