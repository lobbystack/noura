//! Recovery kits (docs/workspace-format/sync-v2.md §12): exporting one
//! adds a recovery identity to your account chain, after removing the
//! previous one, records that head in every workspace with the identity's
//! envelopes, and writes the kit file, mode 0600, outside every workspace,
//! with a 20-byte secret that the app shows once as 32 base32 characters in
//! 8 groups of 4. Recovering with it after losing every device proves the
//! identity, verifies your chain and workspaces from the kit's heads, shows
//! you what changed since for you to confirm, appends `recover_device`,
//! records the heads, removes every other card unless you keep a device you
//! still hold, and rotates.
//!
//! The kit file and its secret together read everything your account reads
//! until you revoke the kit, so the app asks you to keep them apart.
//! Nothing from the kit enters workspace files, logs or the index.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use local_core::sync::SyncCredentials;
use sync_key_envelope::v2::{DeviceKeys, next_link, recovery_proof};
use sync_protocol::codec::{Decode, Encode, Nested, Sorted, Text};
use sync_protocol::crypto::random_nonce;
use sync_protocol::records::{
    AccountHead, Card, CheckpointAnchor, KIT_SECRET_LEN, KitPayload, KitWorkspace, LinkKind,
    RecoveryKit, kit_secret_text, parse_kit_secret,
};
use sync_protocol::types::{Digest, Id};
use zeroize::Zeroizing;

use super::device::{SignedIn, StoredDevice, enroll_card};
use super::removal::kind_name;
use super::{
    DeviceCredentials, KeptHeads, RemoteWorkspace, Secrets, SyncClient, SyncError, SyncService,
};

fn kit_invalid() -> SyncError {
    SyncError::Check("sync_recovery_kit_invalid")
}

fn random<T>(value: Result<T, impl std::fmt::Display>) -> Result<T, SyncError> {
    value.map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))
}

/// What the export screen says about a kit, as the app shows it before you
/// export one (§12).
pub const KIT_WARNING: &str = "The kit file and its secret together read everything your account \
     reads until you revoke the kit. Keep them in different places: store the file somewhere \
     safe and write the secret down.";

/// A kit you exported: its file and its secret, which the app shows once.
pub struct KitExport {
    /// The kit file.
    pub path: PathBuf,
    /// The secret as 32 base32 characters in 8 groups of 4.
    pub secret: Zeroizing<String>,
}

impl std::fmt::Debug for KitExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KitExport")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Whether `path` lies inside a Noura workspace: a folder that holds
/// `.noura/workspace.yaml` at or above it.
fn inside_workspace(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .any(|dir| dir.join(".noura").join("workspace.yaml").is_file())
}

/// Writes the kit to a new file at `path`, readable only by you on Unix,
/// and makes it durable.
fn write_kit(path: &Path, bytes: &[u8]) -> Result<(), SyncError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| {
        SyncError::Invalid(match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                "Choose a new file for the kit; that one exists.".to_owned()
            }
            _ => format!("The kit file couldn't be written: {e}"),
        })
    })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| SyncError::Invalid(format!("The kit file couldn't be written: {e}")))
}

/// Opens a kit file with the secret you type, ignoring spaces, hyphens and
/// letter case. A wrong secret or a tampered file reports
/// `sync_recovery_kit_invalid`.
pub fn open_kit(path: &Path, secret: &str) -> Result<KitPayload, SyncError> {
    let secret = Zeroizing::new(parse_kit_secret(secret).map_err(|_| kit_invalid())?);
    let bytes = std::fs::read(path)
        .map_err(|e| SyncError::Invalid(format!("The kit file couldn't be read: {e}")))?;
    let kit = RecoveryKit::from_bytes(&bytes).map_err(|_| kit_invalid())?;
    let payload = kit.open(&secret).map_err(|_| kit_invalid())?;
    let identity = DeviceKeys::from_seeds(&payload.signing_seed, payload.hpke_secret.clone());
    if !identity.matches(&payload.card) || payload.card.account != payload.account {
        return Err(kit_invalid());
    }
    Ok(payload)
}

impl SyncService {
    /// The devices that the active kit recovered, which replacing that kit
    /// takes down unless you keep them (§12).
    pub async fn kit_replacement(&mut self) -> Result<Vec<Card>, SyncError> {
        let chain = self.own_chain().await?;
        let head = chain.head_seqno();
        let Some(identity) = chain
            .active_at(head)
            .into_iter()
            .find(|s| s.recovery)
            .map(|s| s.card.device)
        else {
            return Ok(Vec::new());
        };
        let removed = chain
            .removal(self.credentials.keys.device(), identity, &[])
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
        Ok(removed
            .into_iter()
            .filter(|id| *id != identity)
            .filter_map(|id| chain.card(&id).map(|s| s.card.clone()))
            .collect())
    }

    /// Exports a recovery kit to the new file `path`, outside every
    /// workspace (§12): appends `add_recovery`, after a `remove` of the
    /// previous identity that keeps `keep` when one is active, records that
    /// head in each of `workspaces` with the identity's envelopes, and
    /// writes the kit with the account head and an anchor per workspace.
    pub async fn export_kit(
        &mut self,
        path: &Path,
        keep: &[Id],
        workspaces: &mut [RemoteWorkspace],
    ) -> Result<KitExport, SyncError> {
        if inside_workspace(path) {
            return Err(SyncError::Invalid(
                "Save the kit outside every workspace, so it never syncs.".into(),
            ));
        }
        if path.exists() {
            return Err(SyncError::Invalid(
                "Choose a new file for the kit; that one exists.".into(),
            ));
        }
        let account = self.credentials.account;
        let identity = random(DeviceKeys::generate())?;
        let chain = self.own_chain().await?;
        let mut head = chain.head();
        let mut links = Vec::new();
        if let Some(old) = chain
            .active_at(chain.head_seqno())
            .into_iter()
            .find(|s| s.recovery)
            .map(|s| s.card.device)
        {
            let remove = next_link(
                &self.credentials.keys,
                &head,
                LinkKind::Remove {
                    device: old,
                    keep: Sorted::new(keep.to_vec())
                        .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                },
            );
            head = AccountHead {
                account,
                genesis: head.genesis,
                seqno: head.seqno + 1,
                head: remove.digest(),
            };
            links.push(remove);
        }
        links.push(next_link(
            &self.credentials.keys,
            &head,
            LinkKind::AddRecovery {
                card: Nested(identity.card(account)),
            },
        ));
        let chain = self.append_links(links).await?;
        let mut anchors = Vec::with_capacity(workspaces.len());
        for remote in workspaces.iter_mut() {
            self.record_heads(remote, &[]).await?;
            let (revision, head) = remote.policy.head();
            let genesis = remote
                .policy
                .change(1)
                .map(|c| c.digest())
                .ok_or(SyncError::Check("sync_policy_rollback"))?;
            anchors.push(KitWorkspace {
                workspace: remote.workspace,
                genesis,
                revision,
                head,
                generation: self.client.restore_generation(),
                // Until checkpoints exist, the anchor names none (§10).
                anchors: Sorted::new(vec![CheckpointAnchor {
                    domain: remote.domain,
                    covered_seq: 0,
                    checkpoint: Digest::ZERO,
                }])
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            });
        }
        let payload = KitPayload {
            origin: Text::new(self.client.origin()?)
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            account,
            account_head: chain.head(),
            card: Nested(identity.card(account)),
            signing_seed: identity.signing().seed(),
            hpke_secret: identity.hpke_seed().clone(),
            workspaces: Sorted::new(anchors)
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
        };
        let secret: Zeroizing<[u8; KIT_SECRET_LEN]> =
            Zeroizing::new(random(sync_protocol::crypto::random_array())?);
        let kit = RecoveryKit::seal(
            &payload,
            &secret,
            random(Id::random())?,
            random(random_nonce())?,
        );
        write_kit(path, &kit.to_bytes())?;
        Ok(KitExport {
            path: path.to_owned(),
            secret: Zeroizing::new(kit_secret_text(&secret)),
        })
    }
}

/// A link of your chain after the kit's head, for your review (§12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewLink {
    /// Its seqno.
    pub seqno: u64,
    /// Its kind, as §7 names it.
    pub kind: &'static str,
    /// The device that signed it.
    pub signer: Id,
}

/// A policy change after the kit's heads, or in a workspace the kit
/// doesn't list, for your review (§12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewChange {
    /// The workspace.
    pub workspace: Id,
    /// Its revision.
    pub revision: u64,
    /// Its kind, as §8 names it.
    pub kind: &'static str,
    /// The device that signed it.
    pub signer: Id,
}

/// What a recovery shows you before it goes on (§12, step 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReview {
    /// Every account link after the kit's head.
    pub links: Vec<ReviewLink>,
    /// Every policy change after the kit's heads, and every change of each
    /// workspace the kit doesn't list.
    pub changes: Vec<ReviewChange>,
    /// The members of each workspace the kit doesn't list, by workspace.
    pub members: Vec<(Id, Vec<Id>)>,
    /// The other active devices of your account, which the recovery
    /// removes unless you keep one you still hold.
    pub devices: Vec<Card>,
    /// The workspaces of your account it couldn't verify, with the error.
    pub unverified: Vec<(Id, &'static str)>,
}

/// A recovery in progress: the new device's service, signed in and
/// enrolled, with the kit's recovery identity proven and every workspace
/// verified from the kit.
pub struct Recovery {
    service: SyncService,
    kit: KitPayload,
    identity: DeviceKeys,
    workspaces: Vec<RemoteWorkspace>,
    review: RecoveryReview,
    stored: StoredDevice,
}

impl Recovery {
    /// What changed since the kit, for you to confirm, with the devices
    /// you may keep.
    pub fn review(&self) -> &RecoveryReview {
        &self.review
    }

    /// Starts recovering on a new computer (§12, steps 1 to 3): with the
    /// session `signed_in`, an empty credential store `store` and the
    /// application data folder `state`, it enrolls a new card, proves the
    /// kit's recovery identity, checks that your chain extends the kit's
    /// head with the identity still active, and verifies each workspace
    /// the kit lists from its stored genesis and head, and each other
    /// workspace of your account through a change that records your
    /// account's head from the kit's on, signed by a card active now.
    pub async fn begin(
        signed_in: &SignedIn,
        kit: KitPayload,
        store: Secrets,
        state: &Path,
    ) -> Result<Self, SyncError> {
        if kit.account != signed_in.account {
            return Err(kit_invalid());
        }
        let identity = DeviceKeys::from_seeds(&kit.signing_seed, kit.hpke_secret.clone());
        if !identity.matches(&kit.card) {
            return Err(kit_invalid());
        }
        // A device this computer already holds for the server keeps its
        // keys: recovering never replaces them.
        if StoredDevice::load(&*store, &signed_in.server)?
            .is_some_and(|stored| stored.reference_head.is_some())
        {
            return Err(SyncError::Invalid(
                "This computer already holds a device of an account on this server. Sign in with it, or sign it out, before you recover.".into(),
            ));
        }
        let keys = random(DeviceKeys::generate())?;
        let mut stored = StoredDevice::new(
            &signed_in.server,
            signed_in.account,
            &keys,
            &signed_in.session_token,
        );
        stored.save(&*store)?;
        let mut client = SyncClient::connect(&signed_in.server, &signed_in.session_token).await?;
        if client.origin()? != kit.origin.as_str() {
            return Err(SyncError::Invalid(
                "This kit belongs to another sync server.".into(),
            ));
        }
        enroll_card(&mut client, &keys, signed_in.account).await?;
        let challenge = client.challenge().await?.challenge;
        let origin = client.origin()?;
        let card = keys.card(signed_in.account);
        match client
            .prove_recovery(recovery_proof(&identity, &origin, &card, challenge))
            .await
        {
            Ok(()) => {}
            Err(SyncError::Refused { code, .. }) if code == "forbidden" => {
                return Err(SyncError::KitRevoked);
            }
            Err(error) => return Err(error),
        }
        let credentials = DeviceCredentials {
            account: signed_in.account,
            keys: keys.clone(),
            session_token: signed_in.session_token.clone(),
            reference_head: kit.account_head.clone(),
        };
        let heads = KeptHeads::open(state, signed_in.account, keys.device(), &*store)?;
        let mut service =
            SyncService::connect(&signed_in.server, credentials, heads, store.clone()).await?;
        service.recovery = Some(identity.clone());
        let chain = service.own_chain().await?;
        let confirmed = chain.head_seqno();
        if !chain.contains(&kit.account_head) {
            return Err(SyncError::Check("sync_account_forked"));
        }
        if !chain.is_active_at(&identity.device(), confirmed) {
            return Err(SyncError::KitRevoked);
        }
        let (links, _) = service
            .client
            .whole_chain(signed_in.account, Some(chain.genesis()))
            .await?;
        let mut review = RecoveryReview {
            links: links
                .iter()
                .filter(|l| l.body.seqno > kit.account_head.seqno)
                .map(|l| ReviewLink {
                    seqno: l.body.seqno,
                    kind: link_kind(&l.body.kind),
                    signer: l.body.signer,
                })
                .collect(),
            changes: Vec::new(),
            members: Vec::new(),
            devices: chain
                .active_at(confirmed)
                .into_iter()
                .filter(|s| !s.recovery && s.card.device != keys.device())
                .map(|s| s.card.clone())
                .collect(),
            unverified: Vec::new(),
        };
        let mut workspaces = Vec::new();
        for listed in kit.workspaces.iter() {
            service
                .heads
                .set_anchor(listed.workspace, listed.revision, listed.head)?;
            service
                .heads
                .first_trusted(listed.workspace, listed.revision)?;
            let policy = service.verified_policy(listed.workspace).await?;
            if policy.change(1).map(|c| c.digest()) != Some(listed.genesis) {
                return Err(SyncError::Check("sync_policy_forked"));
            }
            for revision in (listed.revision + 1)..=policy.head().0 {
                let change = policy.change(revision).expect("verified");
                review.changes.push(ReviewChange {
                    workspace: listed.workspace,
                    revision,
                    kind: kind_name(&change.body.kind),
                    signer: change.body.signer,
                });
            }
            workspaces.push(service.open_as_identity(listed.workspace, policy).await?);
        }
        let known: BTreeSet<Id> = kit.workspaces.iter().map(|w| w.workspace).collect();
        let others: Vec<Id> = service
            .client
            .workspaces()
            .await?
            .workspaces
            .iter()
            .map(|w| w.workspace)
            .filter(|w| !known.contains(w))
            .collect();
        for workspace in others {
            match service.verified_policy(workspace).await {
                Ok(policy) => {
                    // Step 3 verified it, so the device keeps trusting it
                    // from here through steps 4 and 5 (§12).
                    let (revision, head) = policy.head();
                    service.heads.set_anchor(workspace, revision, head)?;
                    for revision in 1..=policy.head().0 {
                        let change = policy.change(revision).expect("verified");
                        review.changes.push(ReviewChange {
                            workspace,
                            revision,
                            kind: kind_name(&change.body.kind),
                            signer: change.body.signer,
                        });
                    }
                    review.members.push((
                        workspace,
                        policy.head_snapshot().members.keys().copied().collect(),
                    ));
                    workspaces.push(service.open_as_identity(workspace, policy).await?);
                }
                Err(SyncError::Check(code)) => review.unverified.push((workspace, code)),
                Err(error) => return Err(error),
            }
        }
        stored.reference_head = Some(kit.account_head.clone());
        Ok(Self {
            service,
            kit,
            identity,
            workspaces,
            review,
            stored,
        })
    }

    /// Finishes the recovery you confirmed (§12, steps 4 and 5), keeping
    /// `keep` among the devices the review lists: appends `recover_device`
    /// for this device's card, records the head right after it in each
    /// verified workspace with this card's envelopes, removes every other
    /// card of your account, the recovery identity included, records the
    /// head after those links, and rotates every domain it reads. Returns
    /// the device's service, its workspaces and the members added after
    /// the kit's heads, for your review; then export a new kit.
    pub async fn finish(
        mut self,
        keep: &[Id],
        store: &dyn SyncCredentials,
    ) -> Result<(SyncService, Vec<RemoteWorkspace>, Vec<(Id, Id)>), SyncError> {
        let service = &mut self.service;
        let own = service.credentials.keys.device();
        let card = service.credentials.card();
        let chain = service.own_chain().await?;
        let recover = next_link(
            &self.identity,
            &chain.head(),
            LinkKind::RecoverDevice { card: Nested(card) },
        );
        let chain = service.append_links(vec![recover]).await?;
        // Until step 5's links commit, the device trusts no workspace other
        // than those the kit lists and step 3 verified (§12).
        service.recovery_removing = true;
        for remote in self.workspaces.iter_mut() {
            service.record_heads(remote, &[]).await?;
        }
        // Remove every other card, one root at a time, so the links take
        // down each card's descendants too.
        let keep: BTreeSet<Id> = keep.iter().copied().collect();
        let mut chain = chain;
        loop {
            let head = chain.head_seqno();
            let Some(target) = chain
                .active_at(head)
                .into_iter()
                .map(|s| (s.card.device, s.recovery))
                .find(|(device, recovery)| *device != own && (*recovery || !keep.contains(device)))
                .map(|(device, _)| device)
            else {
                break;
            };
            let cascade = chain
                .removal(own, target, &[])
                .map_err(|e| SyncError::Invalid(e.to_string()))?;
            let kept: Vec<Id> = cascade
                .iter()
                .filter(|id| keep.contains(id) && **id != target)
                .copied()
                .collect();
            let link = next_link(
                &service.credentials.keys,
                &chain.head(),
                LinkKind::Remove {
                    device: target,
                    keep: Sorted::new(kept).map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                },
            );
            chain = service.append_links(vec![link]).await?;
        }
        service.recovery = None;
        service.recovery_removing = false;
        for remote in self.workspaces.iter_mut() {
            service.record_heads(remote, &[]).await?;
            service.refresh_keys(remote).await?;
        }
        self.stored.save(store)?;
        let mut added = Vec::new();
        for remote in &self.workspaces {
            let known = self
                .kit
                .workspaces
                .iter()
                .find(|w| w.workspace == remote.workspace)
                .and_then(|w| remote.policy.at(w.revision))
                .map(|s| s.members.keys().copied().collect::<BTreeSet<Id>>())
                .unwrap_or_default();
            for member in remote.policy.head_snapshot().members.keys() {
                if !known.contains(member) && *member != self.kit.account {
                    added.push((remote.workspace, *member));
                }
            }
        }
        Ok((self.service, self.workspaces, added))
    }
}

fn link_kind(kind: &LinkKind) -> &'static str {
    match kind {
        LinkKind::Genesis { .. } => "genesis",
        LinkKind::AddDevice { .. } => "add_device",
        LinkKind::AddRecovery { .. } => "add_recovery",
        LinkKind::RecoverDevice { .. } => "recover_device",
        LinkKind::Remove { .. } => "remove",
    }
}

impl SyncService {
    /// A workspace verified during a recovery, with the keys the recovery
    /// identity's envelopes carry (§12).
    async fn open_as_identity(
        &mut self,
        workspace: Id,
        policy: sync_protocol::policy::PolicyChain,
    ) -> Result<RemoteWorkspace, SyncError> {
        let identity = self
            .recovery
            .clone()
            .ok_or_else(|| SyncError::Invalid("no recovery identity".into()))?;
        let domain = policy.head_snapshot().workspace_domain;
        let (epochs, keys) = self
            .load_keys(workspace, domain, &policy, &identity, true)
            .await?;
        RemoteWorkspace::from_parts(workspace, domain, policy, epochs, keys)
    }
}
