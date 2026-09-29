use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::*;
use crate::{Result, WorkspaceEngine};

mod devices;
mod rotation;
mod sharing;

use devices::*;
use rotation::*;

pub(crate) use rotation::finish_access_transition;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncDevice {
    pub device_id: String,
    pub account_id: String,
    pub public_key: String,
    pub encryption_recipient: String,
    pub fingerprint: String,
    pub approved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum SyncInvitationRole {
    Admin,
    Editor,
    Viewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum SyncInvitationStatus {
    Pending,
    Accepted,
    Completed,
    Expired,
    Revoked,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SyncInvitation {
    pub id: String,
    pub role: SyncInvitationRole,
    pub account_id: Option<String>,
    pub status: SyncInvitationStatus,
    pub expires_at: String,
    pub devices: Vec<SyncDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SyncInvitationLink {
    pub id: String,
    pub role: SyncInvitationRole,
    pub invite_url: String,
    pub expires_at: String,
}

pub fn device_fingerprint(
    device: &str,
    account: &str,
    public_key: &str,
    recipient: &str,
) -> Result<String> {
    identifier(device)?;
    identifier(account)?;
    let public: [u8; 32] = super::crypto::decode(public_key, 32, 32)?
        .try_into()
        .map_err(|_| invalid("sync_invalid_key"))?;
    if recipient.starts_with(sync_key_envelope::RECIPIENT_PREFIX) {
        sync_key_envelope::decode_recipient(recipient)
            .map_err(|_| invalid("sync_invalid_recipient"))?;
        return Ok(sync_key_envelope::device_fingerprint(
            device, account, public, recipient,
        ));
    }
    recipient
        .parse::<age::x25519::Recipient>()
        .map_err(|_| invalid("sync_invalid_recipient"))?;
    let bytes = serde_json::to_vec(&(
        "noura.device.card",
        1,
        device,
        account,
        public_key,
        recipient,
    ))
    .map_err(|_| invalid("sync_serialize_failed"))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AccessState {
    revision: String,
    members: Vec<AccessMember>,
    objects: Vec<ObjectEpoch>,
    envelopes: Vec<StoredEnvelope>,
    devices: Vec<RemoteDevice>,
    policy: Option<AccessPolicy>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObjectEpoch {
    object_id: String,
    epoch: String,
    generation: Option<String>,
    document_mode: Option<DocumentMode>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteDevice {
    pub device_id: String,
    pub account_id: String,
    pub public_key: String,
    pub encryption_recipient: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteInvitation {
    pub id: String,
    pub role: SyncInvitationRole,
    pub account_id: Option<String>,
    pub expires_at: String,
    pub status: SyncInvitationStatus,
    pub devices: Vec<RemoteDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredEnvelope {
    object_id: String,
    epoch: String,
    device_id: String,
    wrapped_key: String,
    signing_device: String,
    signature: String,
    #[serde(default)]
    construction: KeyConstruction,
    #[serde(default)]
    recipient_public_key: Option<String>,
    #[serde(default)]
    ephemeral_public_key: Option<String>,
    #[serde(default)]
    salt: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

impl StoredEnvelope {
    /// Reconstruct the discriminated envelope for this workspace and epoch.
    fn to_key_envelope(&self, workspace: &str, object: &str, epoch: u64) -> KeyEnvelope {
        KeyEnvelope {
            workspace_id: workspace.into(),
            object_id: object.into(),
            epoch,
            device_id: self.device_id.clone(),
            wrapped_key: self.wrapped_key.clone(),
            signing_device: self.signing_device.clone(),
            signature: self.signature.clone(),
            construction: self.construction,
            recipient_public_key: self.recipient_public_key.clone(),
            ephemeral_public_key: self.ephemeral_public_key.clone(),
            salt: self.salt.clone(),
            nonce: self.nonce.clone(),
        }
    }
}

impl AccessState {
    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    /// Drop local recipient approvals for devices the relay no longer lists for
    /// this workspace. A revoked device disappears from the active roster, but
    /// its pinned recipient and account would otherwise keep it in the
    /// authorized-writer set and keep the workspace in multi-recipient sharing
    /// mode. The owner's own device is never dropped, so the owner-approval
    /// invariant holds. Pinned public keys stay in place because historical
    /// policy signatures are verified against them; dropping the approved
    /// recipient and account is enough to remove the device from the effective
    /// recipient and writer sets.
    pub(crate) fn effective_config(
        &self,
        config: &WorkspaceSyncConfig,
        own_device_id: &str,
    ) -> WorkspaceSyncConfig {
        let present: BTreeSet<&str> = self
            .devices
            .iter()
            .map(|device| device.device_id.as_str())
            .collect();
        // A verified pass always contains the connected device. If it is
        // missing, the roster is not trustworthy enough to prune local state.
        if !present.contains(own_device_id) {
            return config.clone();
        }
        let mut effective = config.clone();
        effective.approved_recipients.retain(|device, _| {
            device.as_str() == own_device_id || present.contains(device.as_str())
        });
        effective.approved_accounts.retain(|device, _| {
            device.as_str() == own_device_id || present.contains(device.as_str())
        });
        effective
    }

    pub(crate) fn verified_role(
        &self,
        engine: &WorkspaceEngine,
        device: &DeviceKeys,
        config: &WorkspaceSyncConfig,
    ) -> Result<WorkspaceRole> {
        let workspace = engine.manifest().id;
        let revision = super::transport::parse_cursor(&self.revision)?;
        if revision != super::transport::parse_cursor(&engine.sync_access_revision()?)?
            || self.objects.len() > 1000
            || self.devices.len() > 1000
        {
            return Err(invalid("sync_invalid_access_state"));
        }
        let own = self
            .devices
            .iter()
            .find(|entry| entry.device_id == device.device_id())
            .ok_or_else(|| invalid("sync_access_denied"))?;
        if own.public_key != device.signer().public_key()
            || config.approved_accounts.get(device.device_id()) != Some(&own.account_id)
            || own.encryption_recipient.as_deref() != Some(&device.recipient())
        {
            return Err(invalid("sync_device_changed"));
        }
        if let Some(policy) = &self.policy {
            let accepted = engine
                .sync_access_policy()?
                .ok_or_else(|| invalid("sync_policy_chain_changed"))?;
            if accepted.digest()? != policy.digest()? {
                return Err(invalid("sync_policy_chain_changed"));
            }
            let signer = config
                .trusted_devices
                .get(&policy.device_id)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            policy.verify(signer)?;
            if policy.revision != self.revision
                || policy.workspace_id != workspace
                || serde_json::to_value(&policy.members).ok()
                    != serde_json::to_value(&self.members).ok()
                || policy.objects.iter().any(|object| {
                    !self.objects.iter().any(|state| {
                        state.object_id == object.object_id
                            && state.epoch == object.epoch.to_string()
                    })
                })
            {
                return Err(invalid("sync_invalid_access_state"));
            }
            for object in &self.objects {
                super::transport::parse_cursor(&object.epoch)?;
                match (&object.generation, object.document_mode) {
                    (Some(generation), Some(mode)) if policy.version == 2 => {
                        identifier(generation)?;
                        if let Some(signed) = policy
                            .objects
                            .iter()
                            .find(|signed| signed.object_id == object.object_id)
                            && signed.document.as_ref()
                                != Some(&DocumentDescriptor {
                                    generation: generation.clone(),
                                    mode,
                                })
                        {
                            return Err(invalid("sync_invalid_access_state"));
                        }
                    }
                    (None, None) if policy.version == 1 => {}
                    _ => return Err(invalid("sync_invalid_access_state")),
                }
            }
        } else if engine.sync_access_policy()?.is_some()
            || revision != 0
            || self.members.len() != 1
            || self.members[0].account_id != own.account_id
            || self.members[0].role != WorkspaceRole::Owner
        {
            return Err(invalid("sync_invalid_access_state"));
        }
        self.members
            .iter()
            .find(|member| member.account_id == own.account_id)
            .map(|member| member.role)
            .ok_or_else(|| invalid("sync_access_denied"))
    }

    pub(crate) fn authorize_writers(
        &self,
        engine: &WorkspaceEngine,
        device: &DeviceKeys,
        config: &WorkspaceSyncConfig,
        secrets: &mut SyncSecrets,
    ) -> Result<WorkspaceRole> {
        let role = self.verified_role(engine, device, config)?;
        let (workspace_writers, object_writers) = authorizations(
            &self.members,
            self.policy.as_ref().map(|policy| policy.objects.as_slice()),
            config,
        );
        secrets.authorized_workspace_writers = workspace_writers;
        secrets.authorized_object_writers = object_writers;
        engine.sync_record_access_authorization(
            &self.revision,
            &secrets.authorized_workspace_writers,
            &secrets.authorized_object_writers,
        )?;
        engine.sync_load_access_authorizations(secrets)?;
        Ok(role)
    }
}

pub(crate) fn policy_authorizations(
    policy: &AccessPolicy,
    config: &WorkspaceSyncConfig,
) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    authorizations(&policy.members, Some(&policy.objects), config)
}

fn authorizations(
    members: &[AccessMember],
    objects: Option<&[AccessObject]>,
    config: &WorkspaceSyncConfig,
) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    let mut workspace_writers = BTreeSet::new();
    let mut object_writers = BTreeSet::new();
    for (device_id, account_id) in &config.approved_accounts {
        if !config.trusted_devices.contains_key(device_id) {
            continue;
        }
        if members
            .iter()
            .any(|member| member.account_id == *account_id && member.role != WorkspaceRole::Viewer)
        {
            workspace_writers.insert(device_id.clone());
        }
        if let Some(objects) = objects {
            for object in objects {
                if object.grants.iter().any(|grant| {
                    grant.account_id == *account_id && grant.role == ObjectRole::Editor
                }) {
                    object_writers.insert((object.object_id.clone(), device_id.clone()));
                }
            }
        }
    }
    (workspace_writers, object_writers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap};
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, Zeroizing<String>>>);
    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(reference)
                .cloned()
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .insert(reference.into(), Zeroizing::new(value.into()));
            Ok(())
        }
    }

    fn remote(device: &DeviceKeys, account: &str, recipient: String) -> RemoteDevice {
        RemoteDevice {
            device_id: device.device_id().into(),
            account_id: account.into(),
            public_key: device.signer().public_key(),
            encryption_recipient: Some(recipient),
        }
    }

    #[test]
    fn browser_device_fingerprint_matches_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/browser-device-v1.json"
        ))
        .unwrap();
        assert_eq!(fixture["fingerprint"]["domain"], "noura.device.card.web");
        let vector = &fixture["fingerprint"]["vectors"][0];
        assert_eq!(
            device_fingerprint(
                vector["device_id"].as_str().unwrap(),
                vector["account_id"].as_str().unwrap(),
                vector["signing_public"].as_str().unwrap(),
                vector["recipient"].as_str().unwrap(),
            )
            .unwrap(),
            vector["expected_hex"].as_str().unwrap()
        );
    }

    #[test]
    fn mixed_age_and_browser_cards_approve_and_reject_changed_browser_cards() {
        let store = Memory::default();
        let own = DeviceKeys::create(&store).unwrap();
        let browser = DeviceKeys::create_browser(&store).unwrap();
        let native = DeviceKeys::create(&store).unwrap();
        let mut config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: "workspace".into(),
            origin: "https://sync.example.com".into(),
            device_id: own.device_id().into(),
            enabled: true,
            trusted_devices: BTreeMap::from([(own.device_id().into(), own.signer().public_key())]),
            approved_recipients: BTreeMap::from([(own.device_id().into(), own.recipient())]),
            approved_accounts: BTreeMap::from([(own.device_id().into(), "account".into())]),
        };
        let browser_card = sync_device(
            remote(&browser, "account", browser.recipient()),
            &config,
            &own,
        )
        .unwrap();
        assert!(!browser_card.approved);
        assert_ne!(
            browser_card.fingerprint,
            device_fingerprint(
                browser.device_id(),
                "account",
                &browser.signer().public_key(),
                &native.recipient(),
            )
            .unwrap()
        );
        approve_device_card(&mut config, browser_card.clone()).unwrap();
        let native_card = sync_device(
            remote(&native, "account", native.recipient()),
            &config,
            &own,
        )
        .unwrap();
        approve_device_card(&mut config, native_card).unwrap();
        assert_eq!(
            config.approved_recipients.get(browser.device_id()),
            Some(&browser.recipient())
        );
        assert_eq!(
            config.approved_recipients.get(native.device_id()),
            Some(&native.recipient())
        );
        assert!(
            browser
                .recipient()
                .starts_with(sync_key_envelope::RECIPIENT_PREFIX)
        );
        assert!(
            !native
                .recipient()
                .starts_with(sync_key_envelope::RECIPIENT_PREFIX)
        );

        let replacement = DeviceKeys::create_browser(&store).unwrap();
        let changed = sync_device(
            remote(&browser, "account", replacement.recipient()),
            &config,
            &own,
        )
        .unwrap();
        assert_ne!(changed.fingerprint, browser_card.fingerprint);
        assert_eq!(
            approve_device_card(&mut config, changed).unwrap_err().code,
            "sync_device_changed"
        );
        assert_eq!(
            config.approved_recipients.get(browser.device_id()),
            Some(&browser.recipient())
        );
    }

    #[test]
    fn unapproved_browser_devices_do_not_force_a_reader_change() {
        let store = Memory::default();
        let own = DeviceKeys::create(&store).unwrap();
        let approved = DeviceKeys::create_browser(&store).unwrap();
        let pending = DeviceKeys::create(&store).unwrap();
        let mut config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: "workspace".into(),
            origin: "https://sync.example.com".into(),
            device_id: own.device_id().into(),
            enabled: true,
            trusted_devices: BTreeMap::from([(own.device_id().into(), own.signer().public_key())]),
            approved_recipients: BTreeMap::from([(own.device_id().into(), own.recipient())]),
            approved_accounts: BTreeMap::from([(own.device_id().into(), "account".into())]),
        };
        let approved_card = sync_device(
            remote(&approved, "account", approved.recipient()),
            &config,
            &own,
        )
        .unwrap();
        approve_device_card(&mut config, approved_card).unwrap();
        let members = vec![AccessMember {
            account_id: "account".into(),
            role: WorkspaceRole::Owner,
        }];
        let state = AccessState {
            revision: "1".into(),
            members: members.clone(),
            objects: vec![ObjectEpoch {
                object_id: "object".into(),
                epoch: "1".into(),
                generation: Some("generation".into()),
                document_mode: Some(DocumentMode::Text),
            }],
            envelopes: vec![
                StoredEnvelope {
                    object_id: "object".into(),
                    epoch: "1".into(),
                    device_id: own.device_id().into(),
                    wrapped_key: "own".into(),
                    signing_device: own.device_id().into(),
                    signature: "own".into(),
                    construction: KeyConstruction::Age,
                    recipient_public_key: None,
                    ephemeral_public_key: None,
                    salt: None,
                    nonce: None,
                },
                StoredEnvelope {
                    object_id: "object".into(),
                    epoch: "1".into(),
                    device_id: approved.device_id().into(),
                    wrapped_key: "browser".into(),
                    signing_device: own.device_id().into(),
                    signature: "browser".into(),
                    construction: KeyConstruction::Web,
                    recipient_public_key: Some(approved.recipient()),
                    ephemeral_public_key: Some(approved.recipient()),
                    salt: Some("salt".into()),
                    nonce: Some("nonce".into()),
                },
            ],
            devices: vec![
                remote(&own, "account", own.recipient()),
                remote(&approved, "account", approved.recipient()),
                remote(&pending, "account", pending.recipient()),
            ],
            policy: Some(AccessPolicy {
                version: 2,
                workspace_id: "workspace".into(),
                revision: "1".into(),
                previous_policy_digest: None,
                device_id: own.device_id().into(),
                members,
                objects: vec![],
                signature: String::new(),
            }),
        };
        // Both approved devices already hold an envelope, so no rotation is due.
        assert!(!readers_changed(&state, &config, &own));
        // The enrolled but unapproved device cannot be delivered to either.
        assert!(!device_is_approved(
            &remote(&pending, "account", pending.recipient()),
            &config,
            &own,
        ));
    }

    fn stored_envelope(object: &str, device: &DeviceKeys, epoch: &str) -> StoredEnvelope {
        StoredEnvelope {
            object_id: object.into(),
            epoch: epoch.into(),
            device_id: device.device_id().into(),
            wrapped_key: "wrapped".into(),
            signing_device: device.device_id().into(),
            signature: "signature".into(),
            construction: KeyConstruction::Age,
            recipient_public_key: None,
            ephemeral_public_key: None,
            salt: None,
            nonce: None,
        }
    }

    #[test]
    fn effective_config_drops_revoked_recipients_and_keeps_the_owner() {
        let store = Memory::default();
        let own = DeviceKeys::create(&store).unwrap();
        let revoked = DeviceKeys::create(&store).unwrap();
        let config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: "workspace".into(),
            origin: "https://sync.example.com".into(),
            device_id: own.device_id().into(),
            enabled: true,
            trusted_devices: BTreeMap::from([
                (own.device_id().into(), own.signer().public_key()),
                (revoked.device_id().into(), revoked.signer().public_key()),
            ]),
            approved_recipients: BTreeMap::from([
                (own.device_id().into(), own.recipient()),
                (revoked.device_id().into(), revoked.recipient()),
            ]),
            approved_accounts: BTreeMap::from([
                (own.device_id().into(), "account".into()),
                (revoked.device_id().into(), "account".into()),
            ]),
        };
        let state = AccessState {
            revision: "1".into(),
            members: vec![AccessMember {
                account_id: "account".into(),
                role: WorkspaceRole::Owner,
            }],
            objects: vec![],
            envelopes: vec![],
            devices: vec![remote(&own, "account", own.recipient())],
            policy: None,
        };
        let effective = state.effective_config(&config, own.device_id());
        assert_eq!(
            effective.approved_recipients.get(own.device_id()),
            Some(&own.recipient())
        );
        assert_eq!(
            effective.approved_accounts.get(own.device_id()),
            Some(&"account".into())
        );
        assert!(
            !effective
                .approved_recipients
                .contains_key(revoked.device_id())
        );
        assert!(
            !effective
                .approved_accounts
                .contains_key(revoked.device_id())
        );
        // Pinned public keys are retained so historical policies still verify.
        assert!(effective.trusted_devices.contains_key(revoked.device_id()));
    }

    #[test]
    fn a_revoked_devices_lingering_envelope_forces_rotation() {
        let store = Memory::default();
        let own = DeviceKeys::create(&store).unwrap();
        let approved = DeviceKeys::create_browser(&store).unwrap();
        let revoked = DeviceKeys::create(&store).unwrap();
        let config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: "workspace".into(),
            origin: "https://sync.example.com".into(),
            device_id: own.device_id().into(),
            enabled: true,
            trusted_devices: BTreeMap::from([
                (own.device_id().into(), own.signer().public_key()),
                (approved.device_id().into(), approved.signer().public_key()),
                (revoked.device_id().into(), revoked.signer().public_key()),
            ]),
            approved_recipients: BTreeMap::from([
                (own.device_id().into(), own.recipient()),
                (approved.device_id().into(), approved.recipient()),
                (revoked.device_id().into(), revoked.recipient()),
            ]),
            approved_accounts: BTreeMap::from([
                (own.device_id().into(), "account".into()),
                (approved.device_id().into(), "account".into()),
                (revoked.device_id().into(), "account".into()),
            ]),
        };
        let members = vec![AccessMember {
            account_id: "account".into(),
            role: WorkspaceRole::Owner,
        }];
        let state = AccessState {
            revision: "1".into(),
            members: members.clone(),
            objects: vec![ObjectEpoch {
                object_id: "object".into(),
                epoch: "1".into(),
                generation: Some("generation".into()),
                document_mode: Some(DocumentMode::Text),
            }],
            envelopes: vec![
                stored_envelope("object", &own, "1"),
                stored_envelope("object", &approved, "1"),
                stored_envelope("object", &revoked, "1"),
            ],
            devices: vec![
                remote(&own, "account", own.recipient()),
                remote(&approved, "account", approved.recipient()),
            ],
            policy: Some(AccessPolicy {
                version: 2,
                workspace_id: "workspace".into(),
                revision: "1".into(),
                previous_policy_digest: None,
                device_id: own.device_id().into(),
                members,
                objects: vec![],
                signature: String::new(),
            }),
        };
        // The two active devices already hold current-epoch envelopes, so an
        // ordinary reader comparison sees no change.
        assert!(!readers_changed(&state, &config, &own));
        // The revoked device's lingering envelope still triggers a rotation.
        assert!(orphan_reader(&state));
        // A rotation built from the effective set never re-wraps to the revoked
        // device, and the stale local approval is dropped.
        let effective = state.effective_config(&config, own.device_id());
        assert!(
            !effective
                .approved_recipients
                .contains_key(revoked.device_id())
        );
        assert!(
            effective
                .approved_recipients
                .contains_key(approved.device_id())
        );
    }
}
