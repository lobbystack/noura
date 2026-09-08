use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    DeviceConnection, DeviceKeys, DeviceSignIn, DeviceSignInInfo, DeviceSignInStatus,
    SyncCredentials, identifier, invalid, transport::validate_origin,
};
use crate::Result;

const CONNECTION: &str = "active_connection_v1";

/// Public device metadata. Session tokens and private keys never enter this contract.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncAccount {
    pub origin: String,
    pub device_id: String,
    pub account_id: String,
    pub signing_public_key: String,
    pub encryption_recipient: String,
    pub fingerprint: String,
}

#[derive(Debug, Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SyncAccountPoll {
    Pending {
        #[serde(rename = "retryAfter")]
        retry_after: u32,
    },
    Connected {
        account: SyncAccount,
    },
}

/// One native sign-in flow at a time; the host serializes calls to this service.
/// Restarting the host reloads the completed connection from the OS credential store.
#[derive(Default)]
pub struct SyncAccountService {
    pending: Option<DeviceSignIn>,
}

impl SyncAccountService {
    pub async fn begin(
        &mut self,
        origin: &str,
        store: &impl SyncCredentials,
    ) -> Result<DeviceSignInInfo> {
        let flow = DeviceSignIn::begin(origin, store).await?;
        let info = flow.info();
        self.pending = Some(flow);
        Ok(info)
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }

    pub fn verification_uri(&self) -> Result<String> {
        self.pending
            .as_ref()
            .map(|flow| flow.info().verification_uri)
            .ok_or_else(|| invalid("sync_signin_not_started"))
    }

    pub async fn poll(&mut self, store: &impl SyncCredentials) -> Result<SyncAccountPoll> {
        let flow = self
            .pending
            .as_mut()
            .ok_or_else(|| invalid("sync_signin_not_started"))?;
        match flow.poll(store).await? {
            DeviceSignInStatus::Pending { retry_after } => Ok(SyncAccountPoll::Pending {
                retry_after: retry_after.min(60) as u32,
            }),
            DeviceSignInStatus::Connected { connection } => {
                let account = public_account(&connection, store)?;
                // Publish the connection only after its token and identity are in native credentials.
                store.write(
                    CONNECTION,
                    &serde_json::to_string(&connection)
                        .map_err(|_| invalid("sync_serialize_failed"))?,
                )?;
                self.pending = None;
                Ok(SyncAccountPoll::Connected { account })
            }
        }
    }

    pub fn current(&self, store: &impl SyncCredentials) -> Result<Option<SyncAccount>> {
        self.connection(store)?
            .as_ref()
            .map(|connection| public_account(connection, store))
            .transpose()
    }

    pub fn connection(&self, store: &impl SyncCredentials) -> Result<Option<DeviceConnection>> {
        let Some(value) = store.read_optional(CONNECTION)? else {
            return Ok(None);
        };
        let connection: Option<DeviceConnection> =
            serde_json::from_str(&value).map_err(|_| invalid("sync_invalid_connection"))?;
        if let Some(connection) = &connection {
            validate_origin(&connection.origin)?;
            identifier(&connection.device_id)?;
            identifier(&connection.account_id)?;
            let expected = format!(
                "session_{}_{}",
                connection.device_id,
                blake3::hash(connection.origin.as_bytes()).to_hex()
            );
            if connection.token_reference != expected {
                return Err(invalid("sync_invalid_connection"));
            }
        }
        Ok(connection)
    }

    /// Revoke the remote session before clearing the local connection. Offline failure remains visible.
    /// The device identity is retained so an explicitly requested recovery export remains possible.
    pub async fn disconnect(&mut self, store: &impl SyncCredentials) -> Result<()> {
        self.cancel();
        if let Some(connection) = self.connection(store)? {
            match connection
                .transport(store)?
                .revoke_device(&connection.device_id)
                .await
            {
                Ok(()) => {}
                Err(error) if error.code == "sync_sign_in_required" => {}
                Err(error) => return Err(error),
            }
            store.write(&connection.token_reference, "")?;
        }
        store.write(CONNECTION, "null")
    }
}

fn public_account(
    connection: &DeviceConnection,
    store: &impl SyncCredentials,
) -> Result<SyncAccount> {
    let keys = DeviceKeys::load(store, &connection.device_id)?;
    Ok(SyncAccount {
        origin: connection.origin.clone(),
        device_id: connection.device_id.clone(),
        account_id: connection.account_id.clone(),
        signing_public_key: keys.signer().public_key(),
        encryption_recipient: keys.recipient(),
        fingerprint: super::device_fingerprint(
            keys.device_id(),
            &connection.account_id,
            &keys.signer().public_key(),
            &keys.recipient(),
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(RefCell<HashMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.read_optional(reference)?
                .ok_or_else(|| invalid("missing"))
        }
        fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
            Ok(self.0.borrow().get(reference).cloned().map(Zeroizing::new))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(reference.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn stored_connection_is_context_checked_and_secrets_are_absent_from_public_dto() {
        let store = Memory::default();
        let service = SyncAccountService::default();
        assert!(service.current(&store).unwrap().is_none());
        let keys = DeviceKeys::create(&store).unwrap();
        let origin = "https://sync.example.com";
        let mut connection = DeviceConnection {
            origin: origin.into(),
            device_id: keys.device_id().into(),
            account_id: "account_test".into(),
            token_reference: format!(
                "session_{}_{}",
                keys.device_id(),
                blake3::hash(origin.as_bytes()).to_hex()
            ),
        };
        store
            .write(CONNECTION, &serde_json::to_string(&connection).unwrap())
            .unwrap();
        let value = serde_json::to_value(service.current(&store).unwrap().unwrap()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert_eq!(value["deviceId"], keys.device_id());
        connection.origin = "https://other.example.com".into();
        store
            .write(CONNECTION, &serde_json::to_string(&connection).unwrap())
            .unwrap();
        assert!(service.current(&store).is_err());
        store.write(CONNECTION, "not json").unwrap();
        assert!(service.current(&store).is_err());
    }

    #[test]
    fn export_bindings() -> std::result::Result<(), Box<dyn std::error::Error>> {
        SyncAccount::export_all_to("bindings")?;
        SyncAccountPoll::export_all_to("bindings")?;
        DeviceSignInInfo::export_all_to("bindings")?;
        crate::sync::WorkspaceSyncStatus::export_all_to("bindings")?;
        crate::sync::CollaborationPresenceInput::export_all_to("bindings")?;
        crate::sync::CollaborationPresenceMember::export_all_to("bindings")?;
        crate::sync::CollaborationPresenceEvent::export_all_to("bindings")?;
        crate::sync::EncryptedPresence::export_all_to("bindings")?;
        crate::sync::SyncDevice::export_all_to("bindings")?;
        crate::sync::SyncInvitation::export_all_to("bindings")?;
        crate::sync::SyncInvitationLink::export_all_to("bindings")?;
        crate::sync::SyncInvitationRole::export_all_to("bindings")?;
        crate::sync::SyncInvitationStatus::export_all_to("bindings")?;
        crate::sync::RemoteSyncWorkspace::export_all_to("bindings")?;
        crate::sync::SyncConflict::export_all_to("bindings")?;
        crate::sync::ResolveSyncConflict::export_all_to("bindings")?;
        Ok(())
    }
}
